//! Command ring commands (`docs/specs/XHCI_DEVICE.md` section 9.1): Enable Slot, Disable Slot,
//! Address Device, Configure Endpoint, Evaluate Context, Stop Endpoint, Reset Endpoint, Set TR
//! Dequeue Pointer, Reset Device and No Op. Every other command type is answered with TRB Error.
//!
//! A command works on the model's slot table and mirrors the result into the driver's Output
//! Device Context in guest RAM. A guest RAM access that fails is reported as `Err(DmaError)`:
//! the controller turns it into a Host System Error, not into a completion code.

use super::contexts::{
    self, Context, endpoint as ep, input_endpoint, input_slot, output_endpoint, output_slot,
    read_context, slot as sc, write_context,
};
use super::ports::Port;
use super::regs::{MAX_PORTS, MAX_SLOTS};
use super::slots::{DCI_EP0, DCI_INTERRUPT_IN, Endpoint, EndpointState, Slot, SlotState};
use super::trb::{
    CC_CONTEXT_STATE_ERROR, CC_NO_SLOTS_AVAILABLE, CC_PARAMETER_ERROR, CC_SLOT_NOT_ENABLED,
    CC_STOPPED, CC_STOPPED_LENGTH_INVALID, CC_SUCCESS, CC_TRB_ERROR, CC_USB_TRANSACTION_ERROR,
    COMMAND_TYPES, TRB_TYPE_ADDRESS_DEVICE, TRB_TYPE_CONFIGURE_ENDPOINT, TRB_TYPE_DISABLE_SLOT,
    TRB_TYPE_ENABLE_SLOT, TRB_TYPE_EVALUATE_CONTEXT, TRB_TYPE_LINK, TRB_TYPE_NO_OP_COMMAND,
    TRB_TYPE_RESET_DEVICE, TRB_TYPE_RESET_ENDPOINT, TRB_TYPE_SET_TR_DEQUEUE_POINTER,
    TRB_TYPE_STOP_ENDPOINT, Trb, transfer_event,
};
use crate::nvme::{DmaError, GuestDma};

/// Endpoint Context type of a control endpoint and of an Interrupt IN endpoint.
const EP_TYPE_CONTROL: u32 = 4;
const EP_TYPE_INTERRUPT_IN: u32 = 7;
/// Interval (power of two of 125 microseconds) the model accepts for Full-speed interrupt
/// endpoints: 1 ms to 128 ms.
const INTERVAL_RANGE: std::ops::RangeInclusive<u32> = 3..=10;
/// Smallest Max Packet Size of a control endpoint.
const MIN_EP0_PACKET_SIZE: u32 = 8;
/// Input Control Context flags: A0/A1 (Slot and EP0) and DCI 3.
const FLAGS_SLOT_AND_EP0: u32 = 0b11;
const FLAG_INTERRUPT_IN: u32 = 1 << DCI_INTERRUPT_IN;
/// TRB control bits: BSR (Address Device) and DC (Configure Endpoint).
const TRB_BSR_OR_DC: u32 = 1 << 9;

/// Result of one command: the Command Completion Event fields and, for Stop Endpoint, the
/// Transfer Event written before it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct CommandResult {
    pub(super) code: u32,
    pub(super) slot_id: u32,
    pub(super) transfer_event: Option<Trb>,
}

impl CommandResult {
    fn new(code: u32, slot_id: u32) -> Self {
        Self {
            code,
            slot_id,
            transfer_event: None,
        }
    }
}

type Reply = Result<CommandResult, DmaError>;

/// What the commands work on: the slot table, the ports, CONFIG.MaxSlotsEn, DCBAAP and guest RAM.
pub(super) struct CommandEnv<'a> {
    pub(super) slots: &'a mut [Slot; MAX_SLOTS],
    pub(super) ports: &'a mut [Port; MAX_PORTS],
    pub(super) max_slots_enabled: u8,
    pub(super) dcbaap: u64,
    pub(super) dma: &'a mut dyn GuestDma,
}

pub(super) fn execute(env: &mut CommandEnv<'_>, trb: &Trb) -> Reply {
    match trb.trb_type() {
        TRB_TYPE_ENABLE_SLOT => Ok(enable_slot(env, trb)),
        TRB_TYPE_DISABLE_SLOT => Ok(disable_slot(env, trb)),
        TRB_TYPE_ADDRESS_DEVICE => address_device(env, trb),
        TRB_TYPE_CONFIGURE_ENDPOINT => configure_endpoint(env, trb),
        TRB_TYPE_EVALUATE_CONTEXT => evaluate_context(env, trb),
        TRB_TYPE_STOP_ENDPOINT => stop_endpoint(env, trb),
        TRB_TYPE_RESET_ENDPOINT => reset_endpoint(env, trb),
        TRB_TYPE_SET_TR_DEQUEUE_POINTER => set_tr_dequeue_pointer(env, trb),
        TRB_TYPE_RESET_DEVICE => reset_device(env, trb),
        TRB_TYPE_NO_OP_COMMAND => Ok(CommandResult::new(CC_SUCCESS, 0)),
        // A command type the ring allows but the model does not implement keeps the Slot ID of
        // the TRB; a type not allowed in the command ring reports 0.
        kind if COMMAND_TYPES.contains(&kind) => {
            Ok(CommandResult::new(CC_TRB_ERROR, trb.slot_id()))
        }
        _ => Ok(CommandResult::new(CC_TRB_ERROR, 0)),
    }
}

/// Output Device Context of a slot from DCBAA[slot]: 64-byte aligned and not 0.
pub(super) fn output_context_of(
    dcbaap: u64,
    dma: &mut dyn GuestDma,
    slot_id: u32,
) -> Result<u64, DmaError> {
    let mut raw = [0u8; 8];
    dma.read(dcbaap.wrapping_add(8 * u64::from(slot_id)), &mut raw)?;
    let address = u64::from_le_bytes(raw);
    if address == 0 || !address.is_multiple_of(64) {
        return Err(DmaError);
    }
    Ok(address)
}

/// Writes EP State into an Output Endpoint Context.
pub(super) fn mirror_endpoint_state(
    dma: &mut dyn GuestDma,
    output: u64,
    dci: u32,
    state: EndpointState,
) -> Result<(), DmaError> {
    let address = output_endpoint(output, dci);
    let mut context = read_context(dma, address)?;
    ep::set_state(&mut context, state.context_value());
    write_context(dma, address, &context)
}

impl CommandEnv<'_> {
    /// Index of an enabled slot (Enabled or any later state).
    fn slot_index(&self, slot_id: u32) -> Option<usize> {
        let index = (slot_id as usize).checked_sub(1)?;
        self.slots
            .get(index)
            .filter(|slot| slot.is_enabled())
            .map(|_| index)
    }

    fn output_context(&mut self, slot_id: u32) -> Result<u64, DmaError> {
        output_context_of(self.dcbaap, self.dma, slot_id)
    }

    fn update_context(
        &mut self,
        address: u64,
        change: impl FnOnce(&mut Context),
    ) -> Result<(), DmaError> {
        let mut context = read_context(self.dma, address)?;
        change(&mut context);
        write_context(self.dma, address, &context)
    }
}

/// Enable Slot: the smallest free Slot ID in `1..=min(MaxSlotsEn, 8)`; the Slot Type
/// (DW3 20:16) must be 0, the only protocol type the model reports.
fn enable_slot(env: &mut CommandEnv<'_>, trb: &Trb) -> CommandResult {
    if (trb.0[3] >> 16) & 0x1F != 0 {
        return CommandResult::new(CC_TRB_ERROR, 0);
    }
    let limit = usize::from(env.max_slots_enabled).min(MAX_SLOTS);
    match env.slots[..limit]
        .iter()
        .position(|slot| !slot.is_enabled())
    {
        Some(index) => {
            env.slots[index] = Slot {
                state: SlotState::Enabled,
                ..Slot::disabled()
            };
            CommandResult::new(CC_SUCCESS, index as u32 + 1)
        }
        None => CommandResult::new(CC_NO_SLOTS_AVAILABLE, 0),
    }
}

/// Disable Slot: a slot that is not enabled is Slot Not Enabled. DCBAA[n] is guest memory and
/// stays as the driver left it.
fn disable_slot(env: &mut CommandEnv<'_>, trb: &Trb) -> CommandResult {
    let slot_id = trb.slot_id();
    match env.slot_index(slot_id) {
        Some(index) => {
            env.slots[index] = Slot::disabled();
            CommandResult::new(CC_SUCCESS, slot_id)
        }
        None => CommandResult::new(CC_SLOT_NOT_ENABLED, slot_id),
    }
}

/// Slot Context checks of Address Device (xHCI 6.2.2.1): Route String 0, a root port of the
/// model, Interrupter Target 0, Context Entries 1.
fn slot_context_valid(context: &Context) -> bool {
    let port = sc::root_hub_port(context) as usize;
    sc::route_string(context) == 0
        && (1..=MAX_PORTS).contains(&port)
        && sc::interrupter_target(context) == 0
        && sc::context_entries(context) == 1
}

/// Dequeue pointer field of an endpoint context: nonzero, 16-byte aligned (bits 3:1 are
/// reserved or SCT and must be 0).
fn dequeue_valid(context: &Context) -> bool {
    let raw = ep::dequeue_raw(context);
    raw & 0xE == 0 && raw & !0xF != 0
}

/// EP0 checks of Address Device (xHCI 6.2.3.1): control endpoint, Max Packet Size of at least 8,
/// a dequeue pointer with DCS = 1, no streams.
fn ep0_context_valid(context: &Context) -> bool {
    ep::endpoint_type(context) == EP_TYPE_CONTROL
        && ep::max_packet_size(context) >= MIN_EP0_PACKET_SIZE
        && dequeue_valid(context)
        && ep::dequeue_raw(context) & 1 == 1
        && ep::mult_and_streams(context) == 0
}

/// Address Device: BSR = 0 sends SET_ADDRESS (address = Slot ID) and ends in Addressed,
/// BSR = 1 ends in Default with address 0.
fn address_device(env: &mut CommandEnv<'_>, trb: &Trb) -> Reply {
    let slot_id = trb.slot_id();
    let reply = |code| Ok(CommandResult::new(code, slot_id));
    let input = trb.parameter() & !0xF;
    if !input.is_multiple_of(64) {
        return reply(CC_TRB_ERROR);
    }
    let block_set_address = trb.0[3] & TRB_BSR_OR_DC != 0;
    let Some(index) = env.slot_index(slot_id) else {
        return reply(CC_SLOT_NOT_ENABLED);
    };
    let state_allowed = match env.slots[index].state {
        SlotState::Enabled => true,
        SlotState::Default => !block_set_address,
        _ => false,
    };
    if !state_allowed {
        return reply(CC_CONTEXT_STATE_ERROR);
    }
    let control = read_context(env.dma, input)?;
    let slot_context = read_context(env.dma, input_slot(input))?;
    let ep0_context = read_context(env.dma, input_endpoint(input, DCI_EP0))?;
    if contexts::drop_flags(&control) != 0
        || contexts::add_flags(&control) != FLAGS_SLOT_AND_EP0
        || !slot_context_valid(&slot_context)
        || !ep0_context_valid(&ep0_context)
    {
        return reply(CC_PARAMETER_ERROR);
    }
    let port = sc::root_hub_port(&slot_context) as usize;
    // SET_ADDRESS fails when the port is disabled or the device already has an address (it
    // answers to that address, not to 0); the slot keeps its state.
    if !env.ports[port - 1].is_enabled()
        || (!block_set_address && env.ports[port - 1].device_address() != 0)
    {
        return reply(CC_USB_TRANSACTION_ERROR);
    }
    let output = env.output_context(slot_id)?;
    let (new_state, address) = if block_set_address {
        (SlotState::Default, 0)
    } else {
        (SlotState::Addressed, slot_id)
    };
    let mut output_slot_context = slot_context;
    sc::set_device_address(&mut output_slot_context, address);
    sc::set_slot_state(&mut output_slot_context, new_state.context_value());
    write_context(env.dma, output_slot(output), &output_slot_context)?;
    let mut output_ep0 = ep0_context;
    ep::set_state(&mut output_ep0, EndpointState::Running.context_value());
    write_context(env.dma, output_endpoint(output, DCI_EP0), &output_ep0)?;
    env.ports[port - 1].set_device_address(address as u8);
    let slot = &mut env.slots[index];
    slot.state = new_state;
    slot.port = port as u8;
    slot.ep0 = Endpoint {
        state: EndpointState::Running,
        dequeue: ep::dequeue_raw(&ep0_context) & !0xF,
        dcs: true,
    };
    slot.interrupt_in = Endpoint::disabled();
    reply(CC_SUCCESS)
}

/// Report length of the device behind a root port: port 1 is the keyboard (8 bytes), port 2 the
/// absolute pointer (6 bytes) (contract section 6).
fn report_length(port: u8) -> u32 {
    if port == 1 { 8 } else { 6 }
}

/// Interrupt IN endpoint checks of Configure Endpoint: EP State 0 in the input context,
/// Interrupt IN, Max Packet Size of at least the report, Interval in range, a dequeue pointer,
/// no Mult, streams or bursts.
fn interrupt_in_context_valid(context: &Context, port: u8) -> bool {
    ep::state(context) == 0
        && ep::endpoint_type(context) == EP_TYPE_INTERRUPT_IN
        && ep::max_packet_size(context) >= report_length(port)
        && INTERVAL_RANGE.contains(&ep::interval(context))
        && ep::mult_and_streams(context) == 0
        && ep::max_burst(context) == 0
        && dequeue_valid(context)
}

/// Configure Endpoint: adds, drops or replaces the Interrupt IN endpoint (DCI 3), or with DC = 1
/// deconfigures the slot.
fn configure_endpoint(env: &mut CommandEnv<'_>, trb: &Trb) -> Reply {
    let slot_id = trb.slot_id();
    let reply = |code| Ok(CommandResult::new(code, slot_id));
    let deconfigure = trb.0[3] & TRB_BSR_OR_DC != 0;
    let input = trb.parameter() & !0xF;
    // With DC = 1 the Input Context Pointer is ignored.
    if !deconfigure && !input.is_multiple_of(64) {
        return reply(CC_TRB_ERROR);
    }
    let Some(index) = env.slot_index(slot_id) else {
        return reply(CC_SLOT_NOT_ENABLED);
    };
    let slot = env.slots[index];
    if !matches!(slot.state, SlotState::Addressed | SlotState::Configured) {
        return reply(CC_CONTEXT_STATE_ERROR);
    }
    let was_configured = slot.interrupt_in.state != EndpointState::Disabled;
    if deconfigure {
        if !was_configured && slot.state == SlotState::Addressed {
            return reply(CC_SUCCESS);
        }
        let output = env.output_context(slot_id)?;
        write_context(env.dma, output_endpoint(output, DCI_INTERRUPT_IN), &[0; 8])?;
        env.update_context(output_slot(output), |context| {
            sc::set_context_entries(context, 1);
            sc::set_slot_state(context, SlotState::Addressed.context_value());
        })?;
        let slot = &mut env.slots[index];
        slot.state = SlotState::Addressed;
        slot.interrupt_in = Endpoint::disabled();
        return reply(CC_SUCCESS);
    }
    let control = read_context(env.dma, input)?;
    let (drop, add) = (
        contexts::drop_flags(&control),
        contexts::add_flags(&control),
    );
    // D0, D1 and A1 must be 0 and A0 1; the only endpoint the model can add is DCI 3.
    if drop & FLAGS_SLOT_AND_EP0 != 0
        || add & FLAGS_SLOT_AND_EP0 != 1
        || add & !(1 | FLAG_INTERRUPT_IN) != 0
    {
        return reply(CC_PARAMETER_ERROR);
    }
    // D = 1 for a Disabled endpoint is ignored, as are drop flags of the DCIs the model lacks.
    let drop_interrupt_in = drop & FLAG_INTERRUPT_IN != 0;
    let add_interrupt_in = add & FLAG_INTERRUPT_IN != 0;
    let mut added = None;
    if add_interrupt_in {
        // A = 1 without D = 1 for an endpoint that is not Disabled is undefined in xHCI.
        if was_configured && !drop_interrupt_in {
            return reply(CC_PARAMETER_ERROR);
        }
        let context = read_context(env.dma, input_endpoint(input, DCI_INTERRUPT_IN))?;
        if !interrupt_in_context_valid(&context, slot.port) {
            return reply(CC_PARAMETER_ERROR);
        }
        added = Some(context);
    }
    let output = env.output_context(slot_id)?;
    let configured = added.is_some() || (was_configured && !drop_interrupt_in);
    if let Some(mut context) = added {
        ep::set_state(&mut context, EndpointState::Running.context_value());
        write_context(env.dma, output_endpoint(output, DCI_INTERRUPT_IN), &context)?;
    } else if drop_interrupt_in && was_configured {
        write_context(env.dma, output_endpoint(output, DCI_INTERRUPT_IN), &[0; 8])?;
    }
    let new_state = if configured {
        SlotState::Configured
    } else {
        SlotState::Addressed
    };
    env.update_context(output_slot(output), |context| {
        sc::set_context_entries(context, if configured { DCI_INTERRUPT_IN } else { 1 });
        sc::set_slot_state(context, new_state.context_value());
    })?;
    let slot = &mut env.slots[index];
    slot.state = new_state;
    if let Some(context) = added {
        slot.interrupt_in = Endpoint {
            state: EndpointState::Running,
            dequeue: ep::dequeue_raw(&context) & !0xF,
            dcs: ep::dequeue_raw(&context) & 1 != 0,
        };
    } else if drop_interrupt_in {
        slot.interrupt_in = Endpoint::disabled();
    }
    reply(CC_SUCCESS)
}

/// Evaluate Context: A0 updates Max Exit Latency, A1 the Max Packet Size of EP0; flags for
/// DCI 2 and above are ignored unless they exceed the input Context Entries.
fn evaluate_context(env: &mut CommandEnv<'_>, trb: &Trb) -> Reply {
    let slot_id = trb.slot_id();
    let reply = |code| Ok(CommandResult::new(code, slot_id));
    let input = trb.parameter() & !0xF;
    if !input.is_multiple_of(64) {
        return reply(CC_TRB_ERROR);
    }
    let Some(index) = env.slot_index(slot_id) else {
        return reply(CC_SLOT_NOT_ENABLED);
    };
    if !env.slots[index].state.is_addressable() {
        return reply(CC_CONTEXT_STATE_ERROR);
    }
    let control = read_context(env.dma, input)?;
    let slot_context = read_context(env.dma, input_slot(input))?;
    let add = contexts::add_flags(&control);
    let beyond_entries = (add >> 2).checked_ilog2().map(|bit| bit + 2);
    if contexts::drop_flags(&control) != 0
        || beyond_entries.is_some_and(|dci| dci > sc::context_entries(&slot_context))
        || (add & 1 != 0 && sc::interrupter_target(&slot_context) != 0)
    {
        return reply(CC_PARAMETER_ERROR);
    }
    let ep0_context = if add & 0b10 != 0 {
        Some(read_context(env.dma, input_endpoint(input, DCI_EP0))?)
    } else {
        None
    };
    if add & 0b11 == 0 {
        return reply(CC_SUCCESS);
    }
    let output = env.output_context(slot_id)?;
    if add & 1 != 0 {
        env.update_context(output_slot(output), |context| {
            sc::set_max_exit_latency(context, sc::max_exit_latency(&slot_context));
            sc::set_interrupter_target(context, 0);
        })?;
    }
    if let Some(input_ep0) = ep0_context {
        env.update_context(output_endpoint(output, DCI_EP0), |context| {
            ep::set_max_packet_size(context, ep::max_packet_size(&input_ep0));
        })?;
    }
    reply(CC_SUCCESS)
}

/// Endpoint ID (DCI) of a command TRB: DW3 20:16.
fn endpoint_id(trb: &Trb) -> u32 {
    (trb.0[3] >> 16) & 0x1F
}

/// Slot checks shared by the endpoint commands: Slot Not Enabled for a Disabled slot, Context
/// State Error unless it is Default, Addressed or Configured.
fn endpoint_command_slot(env: &CommandEnv<'_>, slot_id: u32) -> Result<usize, CommandResult> {
    let Some(index) = env.slot_index(slot_id) else {
        return Err(CommandResult::new(CC_SLOT_NOT_ENABLED, slot_id));
    };
    if !env.slots[index].state.is_addressable() {
        return Err(CommandResult::new(CC_CONTEXT_STATE_ERROR, slot_id));
    }
    Ok(index)
}

/// Stop Endpoint: a Running endpoint becomes Stopped, its TR Dequeue Pointer and DCS go to the
/// Output Endpoint Context, and a Transfer Event (Stopped, or Stopped - Length Invalid when
/// the ring has no valid TRB) precedes the Command Completion Event (xHCI 4.6.9).
fn stop_endpoint(env: &mut CommandEnv<'_>, trb: &Trb) -> Reply {
    let slot_id = trb.slot_id();
    let dci = endpoint_id(trb);
    let index = match endpoint_command_slot(env, slot_id) {
        Ok(index) => index,
        Err(result) => return Ok(result),
    };
    let endpoint = match env.slots[index].endpoint(dci) {
        Some(endpoint) if endpoint.state == EndpointState::Running => *endpoint,
        // Disabled (any DCI the model lacks), Halted, Stopped and Error endpoints give Context
        // State Error; Endpoint Not Enabled is a doorbell code.
        _ => return Ok(CommandResult::new(CC_CONTEXT_STATE_ERROR, slot_id)),
    };
    let output = env.output_context(slot_id)?;
    let mut raw = [0u8; 16];
    // A Link TRB is not a transfer: the ring has no TD to stop.
    let valid_trb = env.dma.read(endpoint.dequeue, &mut raw).is_ok()
        && Trb::from_bytes(&raw).cycle() == endpoint.dcs
        && Trb::from_bytes(&raw).trb_type() != TRB_TYPE_LINK;
    let (code, remaining) = if valid_trb {
        (CC_STOPPED, Trb::from_bytes(&raw).0[2] & 0x1_FFFF)
    } else {
        (CC_STOPPED_LENGTH_INVALID, 0)
    };
    env.update_context(output_endpoint(output, dci), |context| {
        ep::set_state(context, EndpointState::Stopped.context_value());
        ep::set_dequeue(context, endpoint.dequeue, endpoint.dcs);
    })?;
    if let Some(endpoint) = env.slots[index].endpoint_mut(dci) {
        endpoint.state = EndpointState::Stopped;
    }
    Ok(CommandResult {
        code: CC_SUCCESS,
        slot_id,
        transfer_event: Some(transfer_event(
            endpoint.dequeue,
            code,
            remaining,
            slot_id,
            dci,
        )),
    })
}

/// Reset Endpoint: Halted becomes Stopped; the dequeue pointer is left for Set TR Dequeue
/// Pointer. The device's halt is cleared by the driver with CLEAR_FEATURE(ENDPOINT_HALT).
fn reset_endpoint(env: &mut CommandEnv<'_>, trb: &Trb) -> Reply {
    let slot_id = trb.slot_id();
    let dci = endpoint_id(trb);
    let index = match endpoint_command_slot(env, slot_id) {
        Ok(index) => index,
        Err(result) => return Ok(result),
    };
    if env.slots[index].endpoint_state(dci) != EndpointState::Halted {
        return Ok(CommandResult::new(CC_CONTEXT_STATE_ERROR, slot_id));
    }
    let output = env.output_context(slot_id)?;
    env.update_context(output_endpoint(output, dci), |context| {
        ep::set_state(context, EndpointState::Stopped.context_value());
    })?;
    if let Some(endpoint) = env.slots[index].endpoint_mut(dci) {
        endpoint.state = EndpointState::Stopped;
    }
    Ok(CommandResult::new(CC_SUCCESS, slot_id))
}

/// Set TR Dequeue Pointer: for a Stopped or Error endpoint; the ring continues at the new
/// pointer and DCS, and the endpoint is Stopped.
fn set_tr_dequeue_pointer(env: &mut CommandEnv<'_>, trb: &Trb) -> Reply {
    let slot_id = trb.slot_id();
    let dci = endpoint_id(trb);
    let raw = trb.parameter();
    // Streams are not supported (MaxPStreams = 0): a Stream ID is a TRB error; the stream context
    // type (SCT, bits 3:1) must be 0.
    if trb.0[2] >> 16 != 0 {
        return Ok(CommandResult::new(CC_TRB_ERROR, slot_id));
    }
    if (raw >> 1) & 0x7 != 0 {
        return Ok(CommandResult::new(CC_PARAMETER_ERROR, slot_id));
    }
    let index = match endpoint_command_slot(env, slot_id) {
        Ok(index) => index,
        Err(result) => return Ok(result),
    };
    if !matches!(
        env.slots[index].endpoint_state(dci),
        EndpointState::Stopped | EndpointState::Error
    ) {
        return Ok(CommandResult::new(CC_CONTEXT_STATE_ERROR, slot_id));
    }
    let pointer = raw & !0xF;
    let dcs = raw & 1 != 0;
    let mut probe = [0u8; 16];
    if pointer == 0 || env.dma.read(pointer, &mut probe).is_err() {
        return Ok(CommandResult::new(CC_PARAMETER_ERROR, slot_id));
    }
    let output = env.output_context(slot_id)?;
    env.update_context(output_endpoint(output, dci), |context| {
        ep::set_state(context, EndpointState::Stopped.context_value());
        ep::set_dequeue(context, pointer, dcs);
    })?;
    if let Some(endpoint) = env.slots[index].endpoint_mut(dci) {
        *endpoint = Endpoint {
            state: EndpointState::Stopped,
            dequeue: pointer,
            dcs,
        };
    }
    Ok(CommandResult::new(CC_SUCCESS, slot_id))
}

/// Reset Device: an Addressed or Configured slot returns to Default with address 0; every
/// endpoint but EP0 is Disabled, EP0 keeps its configuration and is Running again.
fn reset_device(env: &mut CommandEnv<'_>, trb: &Trb) -> Reply {
    let slot_id = trb.slot_id();
    let reply = |code| Ok(CommandResult::new(code, slot_id));
    let Some(index) = env.slot_index(slot_id) else {
        return reply(CC_SLOT_NOT_ENABLED);
    };
    if !matches!(
        env.slots[index].state,
        SlotState::Addressed | SlotState::Configured
    ) {
        return reply(CC_CONTEXT_STATE_ERROR);
    }
    let output = env.output_context(slot_id)?;
    let slot = env.slots[index];
    env.update_context(output_slot(output), |context| {
        sc::set_context_entries(context, 1);
        sc::set_device_address(context, 0);
        sc::set_slot_state(context, SlotState::Default.context_value());
    })?;
    // EP0 keeps Max Packet Size, EP Type, CErr, the dequeue pointer and Average TRB Length.
    let old = read_context(env.dma, output_endpoint(output, DCI_EP0))?;
    let mut ep0_context: Context = [0; 8];
    ep::set_state(&mut ep0_context, EndpointState::Running.context_value());
    ep0_context[1] = old[1] & 0xFFFF_003E;
    ep::set_dequeue(&mut ep0_context, slot.ep0.dequeue, slot.ep0.dcs);
    ep0_context[4] = old[4] & 0xFFFF;
    write_context(env.dma, output_endpoint(output, DCI_EP0), &ep0_context)?;
    if slot.interrupt_in.state != EndpointState::Disabled {
        write_context(env.dma, output_endpoint(output, DCI_INTERRUPT_IN), &[0; 8])?;
    }
    // The device is reset with the slot: it answers to address 0 again.
    if let Some(port) = (slot.port as usize).checked_sub(1) {
        env.ports[port].set_device_address(0);
    }
    let slot = &mut env.slots[index];
    slot.state = SlotState::Default;
    slot.ep0.state = EndpointState::Running;
    slot.interrupt_in = Endpoint::disabled();
    reply(CC_SUCCESS)
}
