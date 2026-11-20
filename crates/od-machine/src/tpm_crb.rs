//! Memory-mapped TPM 2.0 command/response buffer, using PTP 1.07 register semantics.

use std::sync::{Arc, Mutex, MutexGuard};

use od_tpm::command::HEADER_SIZE;
use od_tpm::marshal::limits::{MAX_COMMAND_SIZE, MAX_RESPONSE_SIZE};
use od_tpm::response::{Rc, error_response};
use od_tpm::{StateError, Tpm};
use zeroize::{Zeroize, Zeroizing};

use crate::bus::{AccessWidth, BusDevice, DeviceAccess, DeviceError, IoSpace};
use crate::virtual_clock::VirtualClockHandle;

pub const TPM_CRB_BASE: u64 = 0xfed4_0000;
pub const TPM_CRB_LEN: u64 = 0x5000;
const LOCALITY_LEN: u64 = 0x1000;
const BUFFER_OFFSET: u64 = 0x80;
const BUFFER_ADDRESS: u64 = TPM_CRB_BASE + BUFFER_OFFSET;
const BUFFER_LEN: usize = (LOCALITY_LEN - BUFFER_OFFSET) as usize;
const _: () = assert!(BUFFER_LEN >= HEADER_SIZE);

const LOC_STATE: u64 = 0x00;
const LOC_CTRL: u64 = 0x08;
const LOC_STS: u64 = 0x0c;
const INTF_ID: u64 = 0x30;
const CTRL_REQ: u64 = 0x40;
const CTRL_STS: u64 = 0x44;
const CTRL_START: u64 = 0x4c;
const START: u64 = 1;
const RSP_RETRY: u64 = 1 << 1;
const NEXT_CHUNK: u64 = 1 << 2;
const CMD_SIZE: u64 = 0x58;
const CMD_LADDR: u64 = 0x5c;
const RSP_SIZE: u64 = 0x64;
const RSP_ADDR: u64 = 0x68;

const REG_VALID: u64 = 1 << 7;
const LOC_ASSIGNED: u64 = 1 << 1;
const HASH_LOCALITY: u8 = 4;
const HASH_START: u8 = 1 << 0;
const HASH_DATA: u8 = 1 << 1;
const HASH_END: u8 = 1 << 2;
const HASH_DATA_END: u8 = HASH_DATA | HASH_END;
const RESET_ESTABLISHMENT: u8 = 1 << 3;
// MMIO CRB, interface version 3 with chunking (PTP 1.07 Table 24).
const INTERFACE_ID: u64 = 1 | (3 << 4) | (1 << 10) | (1 << 11) | (1 << 14) | (1 << 17);

/// Seed and optional persistent state supplied by the owning host.
#[derive(Clone, Eq, PartialEq)]
pub struct TpmConfig {
    /// At least 48 bytes of entropy, including when reopening a snapshot.
    pub entropy: Vec<u8>,
    /// Unencrypted TPM persistent state returned by `tpm_state_snapshot`.
    pub snapshot: Option<Vec<u8>>,
}

impl std::fmt::Debug for TpmConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TpmConfig").finish_non_exhaustive()
    }
}

impl Drop for TpmConfig {
    fn drop(&mut self) {
        self.entropy.zeroize();
        self.snapshot.zeroize();
    }
}

pub(crate) struct TpmCrb {
    handle: TpmCrbHandle,
    clock: VirtualClockHandle,
}

#[derive(Clone)]
pub(crate) struct TpmCrbHandle {
    state: Arc<Mutex<State>>,
}

/// Trusted platform access to the H-CRTM and establishment fields of CRB `TPM_LOC_CTRL`.
pub struct TpmPlatformPort<'a> {
    device: &'a TpmCrbHandle,
    clock: &'a VirtualClockHandle,
}

impl TpmPlatformPort<'_> {
    /// Delivers a platform locality control write. `buffer` begins with the big-endian length
    /// for `HASH_DATA`, followed by that many octets in the locality 4 command window.
    pub fn write_control(&self, locality: u8, bits: u8, buffer: &[u8]) {
        let now_ms = self.clock.snapshot().now_ns() / 1_000_000;
        lock(&self.device.state).platform_control(locality, bits, buffer, now_ms);
    }
}

impl TpmCrb {
    pub(crate) fn new(
        entropy: &[u8],
        snapshot: Option<&[u8]>,
        clock: VirtualClockHandle,
    ) -> Result<Self, StateError> {
        let tpm = match snapshot {
            Some(bytes) => Tpm::restore(bytes, entropy)?,
            None => Tpm::new(entropy)?,
        };
        Ok(Self {
            handle: TpmCrbHandle {
                state: Arc::new(Mutex::new(State {
                    tpm,
                    granted: false,
                    phase: Phase::Idle,
                    buffer: Zeroizing::new([0; BUFFER_LEN]),
                    command: CommandChunks::First,
                    response: Zeroizing::new(Vec::new()),
                    next_response: 0,
                    #[cfg(test)]
                    fail_startup: false,
                })),
            },
            clock,
        })
    }

    pub(crate) fn handle(&self) -> TpmCrbHandle {
        self.handle.clone()
    }
}

impl TpmCrbHandle {
    pub(crate) fn platform_port<'a>(
        &'a self,
        clock: &'a VirtualClockHandle,
    ) -> TpmPlatformPort<'a> {
        TpmPlatformPort {
            device: self,
            clock,
        }
    }

    #[cfg(test)]
    pub(crate) fn fail_startup(&self) {
        lock(&self.state).fail_startup = true;
    }

    /// Contains unencrypted TPM secrets. The host must protect the returned bytes.
    pub(crate) fn persistent_snapshot(&self, now_ns: u64) -> Vec<u8> {
        let mut state = lock(&self.state);
        state.tpm.advance_power_time(now_ns / 1_000_000);
        state.tpm.persistent_snapshot()
    }

    pub(crate) fn read(&self, access: DeviceAccess) -> Result<u64, DeviceError> {
        let access = Access::checked(access)?;
        Ok(lock(&self.state).read(access))
    }

    pub(crate) fn write(
        &self,
        access: DeviceAccess,
        value: u64,
        sample_time: &dyn Fn() -> Result<u64, DeviceError>,
    ) -> Result<(), DeviceError> {
        let access = Access::checked(access)?;
        lock(&self.state).write(access, value, sample_time)
    }
}

fn lock(state: &Mutex<State>) -> MutexGuard<'_, State> {
    state
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

struct Access {
    locality: u8,
    offset: u64,
    width: usize,
    shift: u32,
}

impl Access {
    fn checked(access: DeviceAccess) -> Result<Self, DeviceError> {
        if access.space != IoSpace::Mmio {
            return Err(DeviceError::UnsupportedSpace);
        }
        if access.offset >= TPM_CRB_LEN {
            return Err(DeviceError::UnknownOffset);
        }
        let width = u64::from(access.width.bytes());
        if access.offset % width != 0 {
            return Err(DeviceError::Misaligned);
        }
        let at = access.offset % LOCALITY_LEN;
        let offset = match at {
            BUFFER_OFFSET.. => at,
            INTF_ID..=0x37 => INTF_ID,
            0x38..=0x3f => 0x38,
            RSP_ADDR..=0x6f => RSP_ADDR,
            _ if access.width == AccessWidth::Qword => return Err(DeviceError::CrossesRegister),
            _ => at & !3,
        };
        Ok(Self {
            locality: (access.offset / LOCALITY_LEN) as u8,
            offset,
            width: width as usize,
            shift: ((at - offset) * 8) as u32,
        })
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    Idle,
    Ready,
    Receiving(usize),
    Complete,
}

#[derive(Default)]
enum CommandChunks {
    #[default]
    First,
    More {
        size: usize,
        bytes: Zeroizing<Vec<u8>>,
    },
    Invalid,
}

struct State {
    tpm: Tpm,
    granted: bool,
    phase: Phase,
    buffer: Zeroizing<[u8; BUFFER_LEN]>,
    command: CommandChunks,
    response: Zeroizing<Vec<u8>>,
    next_response: usize,
    #[cfg(test)]
    fail_startup: bool,
}

impl State {
    fn read(&self, access: Access) -> u64 {
        let offset = access.offset;
        let platform_active = self.tpm.platform_hash_active();
        let active = access.locality == 0 && self.granted;
        let active_locality = if platform_active {
            LOC_ASSIGNED | u64::from(HASH_LOCALITY) << 2
        } else if self.granted {
            LOC_ASSIGNED
        } else {
            0
        };
        let value = match offset {
            LOC_STATE => REG_VALID | active_locality | u64::from(!self.tpm.platform_established()),
            LOC_STS => u64::from(active || platform_active && access.locality == HASH_LOCALITY),
            INTF_ID => INTERFACE_ID,
            0x10 | 0x14 => u64::from(u32::MAX),
            _ if active => match offset {
                CTRL_STS => {
                    u64::from(self.tpm.in_failure_mode())
                        | u64::from(self.phase == Phase::Idle) << 1
                }
                CMD_SIZE | RSP_SIZE => BUFFER_LEN as u64,
                CMD_LADDR | RSP_ADDR => BUFFER_ADDRESS,
                BUFFER_OFFSET.. if self.phase == Phase::Complete => {
                    let start = (offset - BUFFER_OFFSET) as usize;
                    let mut bytes = [0; 8];
                    bytes[..access.width]
                        .copy_from_slice(&self.buffer[start..start + access.width]);
                    u64::from_le_bytes(bytes)
                }
                _ => 0,
            },
            _ => 0,
        };
        (value >> access.shift) & (u64::MAX >> (64 - access.width * 8))
    }

    fn write(
        &mut self,
        access: Access,
        value: u64,
        sample_time: &dyn Fn() -> Result<u64, DeviceError>,
    ) -> Result<(), DeviceError> {
        let offset = access.offset;
        let value = value << access.shift;
        if access.locality != 0 || self.tpm.platform_hash_active() {
            return Ok(());
        }
        if offset == LOC_CTRL {
            match value & 3 {
                1 => self.granted = true,
                2 => {
                    self.granted = false;
                    self.go_idle();
                }
                _ => {}
            }
            return Ok(());
        }
        if !self.granted {
            return Ok(());
        }

        match offset {
            CTRL_REQ => match value & 3 {
                1 if self.phase == Phase::Idle => self.phase = Phase::Ready,
                1 if matches!(self.phase, Phase::Receiving(_)) => {
                    self.go_idle();
                    self.phase = Phase::Ready;
                }
                2 => self.go_idle(),
                _ => {}
            },
            CTRL_START => match (value & (START | RSP_RETRY | NEXT_CHUNK), self.phase) {
                (START, Phase::Receiving(written)) => {
                    let now_ms = sample_time()? / 1_000_000;
                    self.execute(written, now_ms);
                }
                (NEXT_CHUNK, Phase::Receiving(written)) => self.receive_chunk(written),
                (NEXT_CHUNK, Phase::Complete) => self.show_response(self.next_response),
                (RSP_RETRY, Phase::Complete) => self.show_response(0),
                _ => {}
            },
            BUFFER_OFFSET.. => {
                let start = (offset - BUFFER_OFFSET) as usize;
                if start == 0 && matches!(self.phase, Phase::Ready | Phase::Receiving(_)) {
                    self.phase = Phase::Receiving(0);
                }
                if self.phase == Phase::Receiving(start) {
                    let end = start + access.width;
                    self.buffer[start..end].copy_from_slice(&value.to_le_bytes()[..access.width]);
                    self.phase = Phase::Receiving(end);
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn go_idle(&mut self) {
        self.buffer.zeroize();
        self.command = CommandChunks::First;
        self.response = Zeroizing::new(Vec::new());
        self.next_response = 0;
        self.phase = Phase::Idle;
    }

    fn platform_control(&mut self, locality: u8, bits: u8, buffer: &[u8], now_ms: u64) {
        let active = self.tpm.platform_hash_active();
        if bits == RESET_ESTABLISHMENT
            && matches!(locality, 3 | HASH_LOCALITY)
            && !active
            && matches!(self.phase, Phase::Ready | Phase::Idle)
        {
            self.tpm.platform_reset_establishment();
            return;
        }
        if locality != HASH_LOCALITY || self.granted {
            return;
        }
        if bits == HASH_START {
            self.tpm.platform_hash_start();
            return;
        }
        if !active || !matches!(bits, HASH_DATA | HASH_END | HASH_DATA_END) {
            return;
        }
        if bits & HASH_DATA != 0 {
            let data = buffer.split_first_chunk::<2>().and_then(|(length, bytes)| {
                let length = usize::from(u16::from_be_bytes(*length));
                if length <= BUFFER_LEN - 2 {
                    bytes.get(..length)
                } else {
                    None
                }
            });
            let Some(data) = data else {
                self.tpm.platform_hash_failure();
                self.go_idle();
                return;
            };
            self.tpm.platform_hash_data(data);
        }
        if bits & HASH_END != 0 {
            self.tpm.platform_hash_end(now_ms);
            self.go_idle();
        }
    }

    fn declared_command_size(&self) -> usize {
        u32::from_be_bytes(self.buffer[2..6].try_into().unwrap()) as usize
    }

    fn receive_chunk(&mut self, written: usize) {
        let command = std::mem::take(&mut self.command);
        self.command = match command {
            CommandChunks::First if written == BUFFER_LEN => {
                let size = self.declared_command_size();
                if (BUFFER_LEN + 1..=MAX_COMMAND_SIZE).contains(&size) {
                    let mut bytes = Zeroizing::new(Vec::with_capacity(size));
                    bytes.extend_from_slice(&self.buffer[..]);
                    CommandChunks::More { size, bytes }
                } else {
                    CommandChunks::Invalid
                }
            }
            CommandChunks::More { size, mut bytes }
                if written == BUFFER_LEN && size - bytes.len() > BUFFER_LEN =>
            {
                bytes.extend_from_slice(&self.buffer[..]);
                CommandChunks::More { size, bytes }
            }
            _ => CommandChunks::Invalid,
        };
        self.buffer.zeroize();
        self.phase = Phase::Receiving(0);
    }

    fn execute(&mut self, written: usize, now_ms: u64) {
        let command = std::mem::take(&mut self.command);
        let response = match command {
            CommandChunks::First => self.execute_first_chunk(written, now_ms),
            CommandChunks::More { size, mut bytes } if size - bytes.len() <= written => {
                let tail = size - bytes.len();
                bytes.extend_from_slice(&self.buffer[..tail]);
                self.tpm.execute(0, &bytes, now_ms)
            }
            _ => error_response(Rc::COMMAND_SIZE),
        };
        self.complete(response);
    }

    fn execute_first_chunk(&mut self, written: usize, now_ms: u64) -> Vec<u8> {
        // The TPM validates the tag and declared length. A dword write may contain padding
        // after the end of a well-formed command, but unwritten bytes cannot form a command.
        let received = if written >= HEADER_SIZE {
            let size = self.declared_command_size();
            written.min(size.max(HEADER_SIZE))
        } else {
            written
        };
        #[cfg(test)]
        if self.fail_startup
            && self.buffer[..received] == [0x80, 1, 0, 0, 0, 12, 0, 0, 1, 0x44, 0, 0]
        {
            return error_response(Rc::FAILURE);
        }
        self.tpm.execute(0, &self.buffer[..received], now_ms)
    }

    fn complete(&mut self, response: Vec<u8>) {
        debug_assert!(response.len() <= MAX_RESPONSE_SIZE);
        self.response = Zeroizing::new(response);
        self.show_response(0);
        self.phase = Phase::Complete;
    }

    fn show_response(&mut self, offset: usize) {
        let count = (self.response.len() - offset).min(BUFFER_LEN);
        if count == 0 {
            return;
        }
        self.buffer.zeroize();
        self.buffer[..count].copy_from_slice(&self.response[offset..offset + count]);
        self.next_response = offset + count;
    }
}

impl BusDevice for TpmCrb {
    fn read(&mut self, access: DeviceAccess) -> Result<u64, DeviceError> {
        self.handle.read(access)
    }

    fn write(&mut self, access: DeviceAccess, value: u64) -> Result<(), DeviceError> {
        let clock = &self.clock;
        self.handle
            .write(access, value, &|| Ok(clock.snapshot().now_ns()))
    }

    fn write_at_time(
        &mut self,
        access: DeviceAccess,
        value: u64,
        sample_time: &dyn Fn() -> Result<u64, DeviceError>,
    ) -> Result<(), DeviceError> {
        self.handle.write(access, value, sample_time)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    const STARTUP: [u8; 12] = [0x80, 1, 0, 0, 0, 12, 0, 0, 1, 0x44, 0, 0];
    const RANDOM: [u8; 12] = [0x80, 1, 0, 0, 0, 12, 0, 0, 1, 0x7b, 0, 16];

    fn access(offset: u64, width: AccessWidth) -> DeviceAccess {
        DeviceAccess {
            space: IoSpace::Mmio,
            offset,
            width,
        }
    }

    fn read(crb: &mut TpmCrb, offset: u64) -> u64 {
        crb.read(access(offset, AccessWidth::Dword)).unwrap()
    }

    fn write(crb: &mut TpmCrb, offset: u64, value: u64) {
        crb.write(access(offset, AccessWidth::Dword), value)
            .unwrap();
    }

    #[test]
    fn hcrtm_controls_require_platform_access_and_malformed_data_is_fatal() {
        let mut crb = fresh();
        let trust = crb.handle();
        let clock = crb.clock.clone();
        let platform = trust.platform_port(&clock);
        let locality_four = u64::from(HASH_LOCALITY) * LOCALITY_LEN;
        write(&mut crb, locality_four + LOC_CTRL, u64::from(HASH_START));
        assert_eq!(read(&mut crb, LOC_STATE), 0x81);
        platform.write_control(HASH_LOCALITY, HASH_START, &[]);
        assert_eq!(read(&mut crb, LOC_STATE), 0x93);
        write(&mut crb, LOC_CTRL, 1);
        write(&mut crb, locality_four + LOC_CTRL, u64::from(HASH_END));
        assert_eq!(read(&mut crb, locality_four + LOC_STS), 1);
        assert_eq!(read(&mut crb, LOC_STS), 0);
        platform.write_control(HASH_LOCALITY, HASH_DATA_END, &[0, 2, 0xff]);
        assert_eq!(read(&mut crb, LOC_STATE), 0x81);
        write(&mut crb, LOC_CTRL, 1);
        assert_eq!(read(&mut crb, CTRL_STS) & 1, 1);
        write(&mut crb, CTRL_REQ, 1);
        send(&mut crb, &RANDOM);
        write(&mut crb, CTRL_START, 1);
        assert_eq!(response_code(&mut crb), Rc::FAILURE.0);
    }

    fn send(crb: &mut TpmCrb, command: &[u8]) {
        for (index, byte) in command.iter().enumerate() {
            crb.write(
                access(BUFFER_OFFSET + index as u64, AccessWidth::Byte),
                u64::from(*byte),
            )
            .unwrap();
        }
    }

    fn response_code(crb: &mut TpmCrb) -> u32 {
        let mut bytes = [0; 4];
        for (i, byte) in bytes.iter_mut().enumerate() {
            *byte = crb
                .read(access(BUFFER_OFFSET + 6 + i as u64, AccessWidth::Byte))
                .unwrap() as u8;
        }
        u32::from_be_bytes(bytes)
    }
    fn fresh() -> TpmCrb {
        TpmCrb::new(
            &[1; 48],
            None,
            VirtualClockHandle::new(1_000_000_000).unwrap(),
        )
        .unwrap()
    }

    #[test]
    fn crb_locality_and_control_area_follow_the_fixed_profile() {
        let mut crb = fresh();
        for locality in 0..5 {
            let base = locality * LOCALITY_LEN;
            assert_eq!(read(&mut crb, base + LOC_STATE), 0x81);
            assert_eq!(
                crb.read(access(base + INTF_ID, AccessWidth::Qword))
                    .unwrap(),
                0x0002_4c31
            );
            assert_eq!(read(&mut crb, base + CMD_SIZE), 0);
            if locality != 0 {
                write(&mut crb, base + LOC_CTRL, 1);
                assert_eq!(read(&mut crb, base + LOC_STS), 0);
            }
        }
        write(&mut crb, INTF_ID, 1 << 19);
        assert_eq!(read(&mut crb, INTF_ID + LOCALITY_LEN), 0x0002_4c31);

        write(&mut crb, LOC_CTRL, 1);
        assert_eq!(read(&mut crb, LOC_STATE + 4 * LOCALITY_LEN), 0x83);
        assert_eq!(read(&mut crb, LOC_STS), 1);
        assert_eq!(read(&mut crb, CTRL_STS), 2);
        assert_eq!(read(&mut crb, CMD_SIZE), 3968);
        assert_eq!(read(&mut crb, CMD_LADDR), 0xfed4_0080);
        assert_eq!(read(&mut crb, CMD_LADDR + 4), 0);
        assert_eq!(read(&mut crb, RSP_SIZE), 3968);
        assert_eq!(
            crb.read(access(RSP_ADDR, AccessWidth::Qword)).unwrap(),
            BUFFER_ADDRESS
        );
        write(&mut crb, CMD_SIZE, 1);
        assert_eq!(read(&mut crb, CMD_SIZE), 3968);
        write(&mut crb, LOCALITY_LEN + LOC_CTRL, 4);
        assert_eq!(read(&mut crb, LOC_STS), 1);
        assert_eq!(read(&mut crb, LOCALITY_LEN + CTRL_STS), 0);
        write(&mut crb, LOC_CTRL, 2);
        assert_eq!(read(&mut crb, LOC_STATE), 0x81);
        assert_eq!(read(&mut crb, LOC_STS), 0);
    }

    #[test]
    fn crb_executes_only_a_received_command_and_retries_bad_clock_atomically() {
        let mut crb = fresh();
        let samples = Cell::new(0);
        let start = access(CTRL_START, AccessWidth::Dword);
        let sample = || {
            samples.set(samples.get() + 1);
            Ok(2_000_000)
        };
        write(&mut crb, LOC_CTRL, 1);
        crb.write_at_time(start, 1, &sample).unwrap();
        write(&mut crb, CTRL_REQ, 1);
        crb.write_at_time(start, 1, &sample).unwrap();
        assert_eq!(samples.get(), 0);
        send(&mut crb, &STARTUP);
        write(&mut crb, CTRL_REQ, 1);
        crb.write_at_time(start, 1, &sample).unwrap();
        assert_eq!(samples.get(), 0);
        send(&mut crb, &STARTUP);
        assert_eq!(
            crb.write_at_time(start, 1, &|| Err(DeviceError::ClockOverflow)),
            Err(DeviceError::ClockOverflow)
        );
        assert_eq!(read(&mut crb, BUFFER_OFFSET), 0);
        crb.write_at_time(start, 1, &sample).unwrap();
        assert_eq!(samples.get(), 1);
        assert_eq!(read(&mut crb, CTRL_START), 0);
        assert_eq!(response_code(&mut crb), Rc::SUCCESS.0);
        crb.write_at_time(start, 1, &sample).unwrap();
        assert_eq!(samples.get(), 1);

        write(&mut crb, CTRL_REQ, 2);
        assert_eq!(read(&mut crb, CTRL_STS), 2);
        assert_eq!(read(&mut crb, BUFFER_OFFSET), 0);
        write(&mut crb, CTRL_REQ, 1);
        for (index, chunk) in RANDOM.chunks_exact(4).enumerate() {
            write(
                &mut crb,
                BUFFER_OFFSET + 4 * index as u64,
                u64::from(u32::from_le_bytes(chunk.try_into().unwrap())),
            );
        }
        crb.write_at_time(start, 1, &sample).unwrap();
        assert_eq!(samples.get(), 2);
        assert_eq!(response_code(&mut crb), Rc::SUCCESS.0);
        assert_eq!(
            crb.read(access(BUFFER_OFFSET + 10, AccessWidth::Word))
                .unwrap(),
            0x1000
        );
        assert_eq!(read(&mut crb, BUFFER_OFFSET + 28) >> 16, 0);

        write(&mut crb, LOC_CTRL, 2);
        write(&mut crb, LOC_CTRL, 1);
        assert_eq!(read(&mut crb, BUFFER_OFFSET), 0);
    }

    #[test]
    fn crb_rejects_invalid_accesses_and_returns_tpm_rc_for_truncated_command() {
        let mut crb = fresh();
        assert_eq!(
            crb.read(access(TPM_CRB_LEN, AccessWidth::Dword)),
            Err(DeviceError::UnknownOffset)
        );
        assert_eq!(
            crb.write(access(CTRL_REQ, AccessWidth::Qword), 1),
            Err(DeviceError::CrossesRegister)
        );
        assert_eq!(
            crb.write(access(CTRL_REQ + 1, AccessWidth::Dword), 1),
            Err(DeviceError::Misaligned)
        );
        write(&mut crb, LOC_CTRL, 1);
        write(&mut crb, CTRL_REQ, 1);
        send(&mut crb, &STARTUP[..10]);
        write(&mut crb, CTRL_START, 1);
        assert_eq!(response_code(&mut crb), Rc::COMMAND_SIZE.0);
        assert_eq!(read(&mut crb, CTRL_STS), 0);
    }

    #[test]
    fn crb_assembles_commands_through_the_final_chunk() {
        let mut crb = fresh();
        write(&mut crb, LOC_CTRL, 1);
        write(&mut crb, CTRL_REQ, 1);
        send(&mut crb, &STARTUP);
        write(&mut crb, CTRL_START, START);
        assert_eq!(response_code(&mut crb), Rc::SUCCESS.0);

        let samples = Cell::new(0);
        let start = access(CTRL_START, AccessWidth::Dword);
        let sample = || {
            samples.set(samples.get() + 1);
            Ok(2_000_000)
        };
        for size in [BUFFER_LEN, BUFFER_LEN + 1, 2 * BUFFER_LEN, MAX_COMMAND_SIZE] {
            let mut command = vec![0x39; size];
            command[0..2].copy_from_slice(&0x8001u16.to_be_bytes());
            command[2..6].copy_from_slice(&(size as u32).to_be_bytes());
            command[6..10].copy_from_slice(&od_tpm::codes::cc::STIR_RANDOM.to_be_bytes());
            command[10..12].copy_from_slice(&((size - 12) as u16).to_be_bytes());
            write(&mut crb, CTRL_REQ, 2);
            write(&mut crb, CTRL_REQ, 1);
            let before = samples.get();
            let mut chunks = command.chunks(BUFFER_LEN).peekable();
            while let Some(chunk) = chunks.next() {
                send(&mut crb, chunk);
                if chunks.peek().is_some() {
                    crb.write_at_time(start, START | NEXT_CHUNK, &sample)
                        .unwrap();
                    crb.write_at_time(start, NEXT_CHUNK, &sample).unwrap();
                    assert_eq!(samples.get(), before);
                    assert_eq!(read(&mut crb, CTRL_START), 0);
                }
            }
            crb.write_at_time(start, START, &sample).unwrap();
            // The dispatcher saw the complete frame and rejected TPM2B_SENSITIVE_DATA.
            assert_eq!(response_code(&mut crb), Rc::SIZE.in_param(1).0);
            assert_eq!(samples.get(), before + 1);
            crb.write_at_time(start, START, &sample).unwrap();
            crb.write_at_time(start, RSP_RETRY, &sample).unwrap();
            assert_eq!(response_code(&mut crb), Rc::SIZE.in_param(1).0);
            assert_eq!(samples.get(), before + 1);
        }
    }

    #[test]
    fn crb_discards_incomplete_and_oversized_chunk_sequences() {
        let mut crb = fresh();
        write(&mut crb, LOC_CTRL, 1);
        let mut command = [0; BUFFER_LEN];
        command[..STARTUP.len()].copy_from_slice(&STARTUP);
        for (size, bytes, extra_chunk) in [
            (BUFFER_LEN + 8, BUFFER_LEN - 1, false),
            (BUFFER_LEN + 8, BUFFER_LEN, false),
            (BUFFER_LEN + 8, BUFFER_LEN, true),
            (MAX_COMMAND_SIZE + 1, BUFFER_LEN, false),
            (BUFFER_LEN, BUFFER_LEN, false),
        ] {
            write(&mut crb, CTRL_REQ, 2);
            write(&mut crb, CTRL_REQ, 1);
            command[2..6].copy_from_slice(&(size as u32).to_be_bytes());
            send(&mut crb, &command[..bytes]);
            write(&mut crb, CTRL_START, NEXT_CHUNK);
            if extra_chunk {
                send(&mut crb, &command);
                write(&mut crb, CTRL_START, NEXT_CHUNK);
            }
            write(&mut crb, CTRL_START, START);
            assert_eq!(response_code(&mut crb), Rc::COMMAND_SIZE.0);
        }
        write(&mut crb, CTRL_REQ, 2);
        write(&mut crb, CTRL_REQ, 1);
        command[2..6].copy_from_slice(&((BUFFER_LEN + 8) as u32).to_be_bytes());
        send(&mut crb, &command);
        write(&mut crb, CTRL_START, NEXT_CHUNK);
        write(&mut crb, LOC_CTRL, 2);
        write(&mut crb, LOC_CTRL, 1);
        write(&mut crb, CTRL_REQ, 1);
        send(&mut crb, &STARTUP);
        write(&mut crb, CTRL_START, START);
        assert_eq!(response_code(&mut crb), Rc::SUCCESS.0);
    }

    #[test]
    fn crb_pages_responses_and_replays_from_the_first_byte() {
        let mut crb = fresh();
        write(&mut crb, LOC_CTRL, 1);
        for size in [
            BUFFER_LEN,
            BUFFER_LEN + 1,
            4031,
            4991,
            2 * BUFFER_LEN,
            MAX_RESPONSE_SIZE,
        ] {
            // Transport fixtures for frames requiring two or three response pages.
            let mut response: Vec<u8> = (0..size)
                .map(|offset| ((offset / BUFFER_LEN + offset % 251) & 255) as u8)
                .collect();
            response[..2].copy_from_slice(&0x8002u16.to_be_bytes());
            response[2..6].copy_from_slice(&(size as u32).to_be_bytes());
            response[6..10].fill(0);
            lock(&crb.handle.state).complete(response.clone());
            for _ in 0..2 {
                for (index, chunk) in response.chunks(BUFFER_LEN).enumerate() {
                    if index > 0 {
                        write(&mut crb, CTRL_START, NEXT_CHUNK);
                    }
                    assert_eq!(read(&mut crb, CTRL_START), 0);
                    for offset in 0..BUFFER_LEN {
                        let got = crb
                            .read(access(BUFFER_OFFSET + offset as u64, AccessWidth::Byte))
                            .unwrap() as u8;
                        assert_eq!(got, chunk.get(offset).copied().unwrap_or_default());
                    }
                }
                write(&mut crb, CTRL_START, RSP_RETRY);
            }
            write(&mut crb, LOC_CTRL, 2);
            write(&mut crb, LOC_CTRL, 1);
            write(&mut crb, CTRL_START, RSP_RETRY);
            assert_eq!(read(&mut crb, BUFFER_OFFSET), 0);
        }
    }

    #[test]
    fn powered_idle_time_survives_persistent_snapshot_and_transport_resets() {
        const READ_CLOCK: [u8; 10] = [0x80, 1, 0, 0, 0, 10, 0, 0, 1, 0x81];
        let transact = |crb: &mut TpmCrb, command: &[u8]| {
            write(crb, LOC_CTRL, 1);
            write(crb, CTRL_REQ, 2);
            write(crb, CTRL_REQ, 1);
            send(crb, command);
            write(crb, CTRL_START, 1);
            assert_eq!(response_code(crb), Rc::SUCCESS.0);
        };
        let read_clock = |crb: &mut TpmCrb| {
            transact(crb, &READ_CLOCK);
            let mut bytes = [0u8; 8];
            for (i, byte) in bytes.iter_mut().enumerate() {
                *byte = crb
                    .read(access(BUFFER_OFFSET + 18 + i as u64, AccessWidth::Byte))
                    .unwrap() as u8;
            }
            u64::from_be_bytes(bytes)
        };
        let mut crb = fresh();
        transact(&mut crb, &STARTUP);
        crb.clock.advance_standalone(13_000_000).unwrap();
        assert_eq!(read_clock(&mut crb), 13);
        crb.clock.advance_standalone(4_000_000).unwrap();
        let snapshot = Zeroizing::new(
            crb.handle()
                .persistent_snapshot(crb.clock.snapshot().now_ns()),
        );
        let new_clock = VirtualClockHandle::new(1_000_000_000).unwrap();
        let mut restored = TpmCrb::new(&[2; 48], Some(&snapshot), new_clock.clone()).unwrap();
        assert_eq!(read(&mut restored, LOC_STS), 0);
        assert_eq!(read(&mut restored, BUFFER_OFFSET), 0);
        transact(&mut restored, &STARTUP);
        new_clock.advance_standalone(3_000_000).unwrap();
        assert_eq!(read_clock(&mut restored), 20);
    }

    // ML-DSA-65 through the CRB window (ADR-0009, decision 7, item 1). The commands are marshaled
    // here by hand, so the device is tested with frames that no helper of `od-tpm` shaped.

    const SAMPLE_NS: u64 = 2_000_000;
    const SAMPLE_MS: u64 = SAMPLE_NS / 1_000_000;
    const NV_INDEX: u32 = 0x0100_0001;

    fn tpm2b(data: &[u8]) -> Vec<u8> {
        [&(data.len() as u16).to_be_bytes()[..], data].concat()
    }

    /// A command frame; `passwords` are the password sessions of the authorization area (`None`
    /// for a command without sessions).
    fn command(code: u32, handles: &[u32], passwords: Option<&[&[u8]]>, params: &[u8]) -> Vec<u8> {
        use od_tpm::codes::{handle::RS_PW, st};
        let mut body = Vec::new();
        for handle in handles {
            body.extend_from_slice(&handle.to_be_bytes());
        }
        if let Some(passwords) = passwords {
            let mut area = Vec::new();
            for password in passwords {
                area.extend_from_slice(&RS_PW.to_be_bytes());
                area.extend_from_slice(&0u16.to_be_bytes());
                area.push(1);
                area.extend_from_slice(&tpm2b(password));
            }
            body.extend_from_slice(&(area.len() as u32).to_be_bytes());
            body.extend_from_slice(&area);
        }
        body.extend_from_slice(params);
        let tag = if passwords.is_some() {
            st::SESSIONS
        } else {
            st::NO_SESSIONS
        };
        let mut frame = tag.to_be_bytes().to_vec();
        frame.extend_from_slice(&((10 + body.len()) as u32).to_be_bytes());
        frame.extend_from_slice(&code.to_be_bytes());
        frame.extend_from_slice(&body);
        frame
    }

    fn read_page(crb: &mut TpmCrb) -> Vec<u8> {
        let mut page = Vec::with_capacity(BUFFER_LEN);
        for offset in (0..BUFFER_LEN).step_by(4) {
            let word = read(crb, BUFFER_OFFSET + offset as u64) as u32;
            page.extend_from_slice(&word.to_le_bytes());
        }
        page
    }

    /// One command through the CRB as a driver does it: the command in windows (`NEXT_CHUNK` after
    /// each full window), `START`, then the response in pages (`NEXT_CHUNK` for each next page).
    /// Returns the response and the number of pages it took.
    fn transact(crb: &mut TpmCrb, command: &[u8]) -> (Vec<u8>, usize) {
        let start = access(CTRL_START, AccessWidth::Dword);
        let sample = || Ok(SAMPLE_NS);
        write(crb, LOC_CTRL, 1);
        write(crb, CTRL_REQ, 2);
        write(crb, CTRL_REQ, 1);
        let mut windows = command.chunks(BUFFER_LEN).peekable();
        while let Some(window) = windows.next() {
            for (index, word) in window.chunks(4).enumerate() {
                let mut bytes = [0; 4];
                bytes[..word.len()].copy_from_slice(word);
                let value = u64::from(u32::from_le_bytes(bytes));
                write(crb, BUFFER_OFFSET + 4 * index as u64, value);
            }
            if windows.peek().is_some() {
                crb.write_at_time(start, NEXT_CHUNK, &sample).unwrap();
            }
        }
        crb.write_at_time(start, START, &sample).unwrap();
        assert_eq!(read(crb, CTRL_START), 0, "the command was executed");
        let mut response = read_page(crb);
        let size = u32::from_be_bytes(response[2..6].try_into().unwrap()) as usize;
        let mut pages = 1;
        while response.len() < size {
            write(crb, CTRL_START, NEXT_CHUNK);
            response.extend_from_slice(&read_page(crb));
            pages += 1;
        }
        response.truncate(size);
        (response, pages)
    }

    fn rc_of(response: &[u8]) -> u32 {
        u32::from_be_bytes(response[6..10].try_into().unwrap())
    }

    /// The exchange of the decision: an ML-DSA-65 key signs the NV_Certify attestation of a 512-octet
    /// index (the response is larger than the window), and `VerifySequenceComplete` checks the
    /// 3309-octet signature, first a damaged one (`TPM_RC_SIGNATURE`), then the good one (a
    /// `TPM_ST_MESSAGE_VERIFIED` ticket). Every command goes through `exchange`; the responses are returned in
    /// order: Startup, CreatePrimary, NV_DefineSpace, NV_Write, NV_Certify, VerifySequenceStart,
    /// SequenceUpdate, damaged and good VerifySequenceComplete.
    fn mldsa_exchange(mut exchange: impl FnMut(&[u8]) -> Vec<u8>) -> (Vec<Vec<u8>>, Vec<usize>) {
        use od_tpm::codes::{alg, cc, handle::RH_OWNER};
        use od_tpm::nv::attr;
        let mut log = Vec::new();
        let mut sizes = Vec::new();
        let mut step = |frame: Vec<u8>| {
            sizes.push(frame.len());
            let response = exchange(&frame);
            log.push(response.clone());
            response
        };
        step(STARTUP.to_vec());
        // CreatePrimary: Owner (empty password), userAuth "pw", ML-DSA-65 with allowExternalMu.
        let template = [
            &0x00a1u16.to_be_bytes()[..],
            &alg::SHA256.to_be_bytes(),
            &0x0004_0072u32.to_be_bytes(),
            &tpm2b(b""),
            &2u16.to_be_bytes(),
            &[1],
            &tpm2b(b""),
        ]
        .concat();
        let params = [
            tpm2b(&[tpm2b(b"pw"), tpm2b(b"")].concat()),
            tpm2b(&template),
            tpm2b(b""),
            0u32.to_be_bytes().to_vec(),
        ]
        .concat();
        let created = step(command(
            cc::CREATE_PRIMARY,
            &[RH_OWNER],
            Some(&[b""]),
            &params,
        ));
        let key = u32::from_be_bytes(created[10..14].try_into().unwrap());
        // A 512-octet NV index, owner read and write, filled with a pattern.
        let nv_public = [
            &NV_INDEX.to_be_bytes()[..],
            &alg::SHA256.to_be_bytes(),
            &(attr::OWNERWRITE | attr::OWNERREAD).to_be_bytes(),
            &tpm2b(b""),
            &512u16.to_be_bytes(),
        ]
        .concat();
        step(command(
            cc::NV_DEFINE_SPACE,
            &[RH_OWNER],
            Some(&[b""]),
            &[tpm2b(b""), tpm2b(&nv_public)].concat(),
        ));
        let data: Vec<u8> = (0..512).map(|i| (i % 251) as u8).collect();
        step(command(
            cc::NV_WRITE,
            &[RH_OWNER, NV_INDEX],
            Some(&[b""]),
            &[tpm2b(&data), 0u16.to_be_bytes().to_vec()].concat(),
        ));
        // NV_Certify of the whole index: qualifying data, scheme NULL, size 512, offset 0.
        let certified = step(command(
            cc::NV_CERTIFY,
            &[key, RH_OWNER, NV_INDEX],
            Some(&[b"pw", b""]),
            &[
                tpm2b(&[0x5a; 64]),
                alg::NULL.to_be_bytes().to_vec(),
                512u16.to_be_bytes().to_vec(),
                0u16.to_be_bytes().to_vec(),
            ]
            .concat(),
        ));
        assert_eq!(rc_of(&certified), 0);
        // responseParameterSize, then TPM2B_ATTEST and TPMT_SIGNATURE (TPM_ALG_MLDSA, 3309 octets).
        let parameters = u32::from_be_bytes(certified[10..14].try_into().unwrap()) as usize;
        let parameters = &certified[14..14 + parameters];
        let attest_size = usize::from(u16::from_be_bytes(parameters[..2].try_into().unwrap()));
        let attest = parameters[2..2 + attest_size].to_vec();
        let signature = parameters[2 + attest_size..].to_vec();
        assert_eq!(signature[..4], [0x00, 0xa1, 0x0c, 0xed]);
        assert_eq!(signature.len(), 4 + 3309);
        // Verify the attestation with the same key: a verify sequence over the attestation.
        let started = step(command(
            cc::VERIFY_SEQUENCE_START,
            &[key],
            None,
            &[tpm2b(b"vseq"), tpm2b(b""), tpm2b(b"")].concat(),
        ));
        let sequence = u32::from_be_bytes(started[10..14].try_into().unwrap());
        step(command(
            cc::SEQUENCE_UPDATE,
            &[sequence],
            Some(&[b"vseq"]),
            &tpm2b(&attest),
        ));
        let mut damaged = signature.clone();
        damaged[1500] ^= 1;
        for signature in [damaged, signature] {
            step(command(
                cc::VERIFY_SEQUENCE_COMPLETE,
                &[sequence, key],
                Some(&[b"vseq"]),
                &signature,
            ));
        }
        drop(step);
        (log, sizes)
    }

    #[test]
    fn crb_carries_mldsa_certify_response_in_pages_and_the_3309_octet_signature_in_a_command() {
        let crb_pages = std::cell::RefCell::new(Vec::new());
        let mut crb = fresh();
        let (via_crb, command_sizes) = mldsa_exchange(|frame| {
            let (response, pages) = transact(&mut crb, frame);
            crb_pages.borrow_mut().push(pages);
            response
        });
        let mut tpm = Tpm::new(&[1; 48]).unwrap();
        let (direct, _) = mldsa_exchange(|frame| tpm.execute(0, frame, SAMPLE_MS));
        // The device passes the bytes through: every response equals the direct call of `od-tpm`
        // (same entropy, same key, same input, same time).
        assert_eq!(via_crb, direct);
        for (index, response) in via_crb.iter().enumerate() {
            let expected = match index {
                7 => Rc::SIGNATURE.0,
                _ => 0,
            };
            assert_eq!(rc_of(response), expected, "response {index}");
        }
        // NV_Certify (index 4): more than the 3968 octets of the window, so two pages; the size field
        // covers the whole frame; every other response fits one page.
        let certified = &via_crb[4];
        assert!(certified.len() > BUFFER_LEN && certified.len() <= MAX_RESPONSE_SIZE);
        assert_eq!(
            u32::from_be_bytes(certified[2..6].try_into().unwrap()) as usize,
            certified.len()
        );
        assert_eq!(
            *crb_pages.borrow(),
            [1, 1, 1, 1, 2, 1, 1, 1, 1],
            "pages of each response"
        );
        // VerifySequenceComplete (index 7 damaged, 8 good) carries the 3313-octet TPMT_SIGNATURE
        // (3348 octets in all, below the window, so it goes in one write of the window). A damaged
        // signature is TPM_RC_SIGNATURE and keeps the sequence; the good one gives the
        // TPM_ST_MESSAGE_VERIFIED ticket (0x8026).
        assert!(
            (3313..=BUFFER_LEN).contains(&command_sizes[8]),
            "{}",
            command_sizes[8]
        );
        assert_eq!(via_crb[7].len(), 10);
        let good = &via_crb[8];
        let parameters = u32::from_be_bytes(good[10..14].try_into().unwrap()) as usize;
        assert_eq!(good[14..16], [0x80, 0x26], "TPM_ST_MESSAGE_VERIFIED");
        assert!(parameters >= 6);
    }
}
