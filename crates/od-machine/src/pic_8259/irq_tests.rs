//! S4-39g: IRR, ISR, INTA and EOI of the master chip. Oracles by hand from the 8259A rules
//! (fixed priority, IR0 highest; in 8086 mode the vector is T7–T3 of ICW2 with the line in
//! bits 2:0, 8259A data sheet 231468-003, S7-32 slice O).

use super::*;

const BYTE: AccessWidth = AccessWidth::Byte;

fn acc(offset: u64) -> DeviceAccess {
    DeviceAccess {
        space: IoSpace::Pio,
        offset,
        width: BYTE,
    }
}

fn out(pic: &mut Pic8259, offset: u64, value: u8) {
    pic.write(acc(offset), u64::from(value)).unwrap();
}

fn inb(pic: &mut Pic8259, offset: u64) -> u8 {
    pic.read(acc(offset)).unwrap() as u8
}

/// An initialised master (vector base `0x30`) with the given mask.
fn master(imr: u8, icw4: u8) -> (Pic8259, Pic8259Handle) {
    let (mut pic, handle) = Pic8259::new_with_handle(Pic8259Role::Master);
    out(&mut pic, 0, PIC_ICW1_ACCEPTED);
    out(&mut pic, 1, Pic8259Role::Master.icw2());
    out(&mut pic, 1, Pic8259Role::Master.icw3());
    out(&mut pic, 1, icw4);
    out(&mut pic, 1, imr);
    (pic, handle)
}

#[test]
fn an_unmasked_edge_is_pending_with_the_vector_base_plus_line() {
    let (_pic, handle) = master(0x00, PIC_ICW4_ACCEPTED);
    assert_eq!(handle.pending_vector(), None);
    handle.raise_irq(0);
    assert_eq!(handle.pending_vector(), Some(0x30));
    // Asking does not acknowledge.
    assert_eq!(handle.pending_vector(), Some(0x30));
    assert_eq!((handle.snapshot().irr, handle.snapshot().isr), (1, 0));
}

#[test]
fn a_masked_edge_stays_in_irr_and_is_delivered_after_the_unmask() {
    let (mut pic, handle) = master(0xFF, PIC_ICW4_ACCEPTED);
    handle.raise_irq(0);
    assert_eq!(handle.pending_vector(), None, "IMR bit 0 masks IR0");
    assert_eq!(handle.snapshot().irr, 1, "the edge is latched");
    out(&mut pic, 1, 0xFE);
    assert_eq!(handle.pending_vector(), Some(0x30));
}

#[test]
fn nothing_is_pending_before_the_initialisation() {
    let (_pic, handle) = Pic8259::new_with_handle(Pic8259Role::Master);
    handle.raise_irq(0);
    assert_eq!(handle.pending_vector(), None);
    assert!(!handle.acknowledge(0x30));
}

#[test]
fn inta_moves_irr_to_isr_and_the_line_is_blocked_until_the_eoi() {
    let (mut pic, handle) = master(0x00, PIC_ICW4_ACCEPTED);
    handle.raise_irq(0);
    assert!(handle.acknowledge(0x30));
    let s = handle.snapshot();
    assert_eq!((s.irr, s.isr), (0, 1));
    assert_eq!(handle.pending_vector(), None);
    // A second edge waits in IRR behind the in-service line.
    handle.raise_irq(0);
    assert_eq!((handle.snapshot().irr, handle.snapshot().isr), (1, 1));
    assert_eq!(handle.pending_vector(), None, "no EOI yet");
    // Non-specific EOI clears ISR; the coalesced request is delivered once.
    out(&mut pic, 0, 0x20);
    assert_eq!((handle.snapshot().irr, handle.snapshot().isr), (1, 0));
    assert_eq!(handle.pending_vector(), Some(0x30));
}

#[test]
fn a_wrong_vector_is_not_acknowledged_and_changes_nothing() {
    let (_pic, handle) = master(0x00, PIC_ICW4_ACCEPTED);
    handle.raise_irq(0);
    let before = handle.snapshot();
    for vector in [0x00u8, 0x2F, 0x31, 0x38, 0xFF] {
        assert!(!handle.acknowledge(vector), "{vector:#04x}");
    }
    assert_eq!(handle.snapshot(), before);
}

#[test]
fn the_priority_is_fixed_and_a_lower_line_waits_for_the_higher_eoi() {
    let (mut pic, handle) = master(0x00, PIC_ICW4_ACCEPTED);
    handle.raise_irq(3);
    handle.raise_irq(5);
    // IR3 beats IR5.
    assert_eq!(handle.pending_vector(), Some(0x33));
    assert!(handle.acknowledge(0x33));
    // IR5 is below the in-service IR3: blocked.
    assert_eq!(handle.pending_vector(), None);
    // IR1 is above IR3: it preempts.
    handle.raise_irq(1);
    assert_eq!(handle.pending_vector(), Some(0x31));
    assert!(handle.acknowledge(0x31));
    assert_eq!(handle.snapshot().isr, 0b0000_1010);
    // Non-specific EOI clears the highest priority (IR1), then IR3.
    out(&mut pic, 0, 0x20);
    assert_eq!(handle.snapshot().isr, 0b0000_1000);
    assert_eq!(handle.pending_vector(), None);
    out(&mut pic, 0, 0x20);
    assert_eq!(handle.snapshot().isr, 0);
    assert_eq!(handle.pending_vector(), Some(0x35));
}

#[test]
fn a_specific_eoi_clears_exactly_its_line_and_an_empty_isr_ignores_eoi() {
    let (mut pic, handle) = master(0x00, PIC_ICW4_ACCEPTED);
    out(&mut pic, 0, 0x20);
    out(&mut pic, 0, 0x65);
    assert_eq!(
        handle.snapshot().isr,
        0,
        "EOI with an empty ISR changes nothing"
    );
    handle.raise_irq(3);
    handle.raise_irq(5);
    assert!(handle.acknowledge(0x33));
    handle.raise_irq(1);
    assert!(handle.acknowledge(0x31));
    out(&mut pic, 0, 0x60 | 3);
    assert_eq!(handle.snapshot().isr, 0b0000_0010, "only IR3 was cleared");
    out(&mut pic, 0, 0x60 | 4);
    assert_eq!(handle.snapshot().isr, 0b0000_0010, "IR4 was not in service");
    // The rotate form clears the same line (the rotation is ignored).
    out(&mut pic, 0, 0xE0 | 1);
    assert_eq!(handle.snapshot().isr, 0);
}

#[test]
fn with_aeoi_an_inta_sets_no_isr_bit() {
    let (_pic, handle) = master(0x00, PIC_ICW4_MASTER_AEOI);
    handle.raise_irq(0);
    assert!(handle.acknowledge(0x30));
    let s = handle.snapshot();
    assert_eq!((s.irr, s.isr), (0, 0));
    handle.raise_irq(0);
    assert_eq!(handle.pending_vector(), Some(0x30), "no EOI is needed");
}

#[test]
fn ocw3_reads_irr_or_isr_and_the_mask_register_is_unchanged() {
    let (mut pic, handle) = master(0xFA, PIC_ICW4_ACCEPTED);
    handle.raise_irq(0);
    handle.raise_irq(2);
    out(&mut pic, 0, 0x0A);
    assert_eq!(inb(&mut pic, 0), 0b0000_0101, "IRR");
    out(&mut pic, 0, 0x0B);
    assert_eq!(inb(&mut pic, 0), 0, "ISR");
    assert!(handle.acknowledge(0x30));
    assert_eq!(inb(&mut pic, 0), 0b0000_0001, "ISR holds IR0");
    out(&mut pic, 0, 0x0A);
    assert_eq!(inb(&mut pic, 0), 0b0000_0100, "IRR holds IR2");
    assert_eq!(inb(&mut pic, 1), 0xFA);
}

#[test]
fn a_new_icw1_clears_irr_and_isr() {
    let (mut pic, handle) = master(0x00, PIC_ICW4_ACCEPTED);
    handle.raise_irq(0);
    assert!(handle.acknowledge(0x30));
    handle.raise_irq(0);
    out(&mut pic, 0, PIC_ICW1_ACCEPTED);
    let s = handle.snapshot();
    assert_eq!((s.irr, s.isr), (0, 0));
    assert_eq!(handle.pending_vector(), None, "not initialised again yet");
}

/// An initialised master with the given ICW2 and every line unmasked.
fn master_with_icw2(icw2: u8) -> (Pic8259, Pic8259Handle) {
    let (mut pic, handle) = Pic8259::new_with_handle(Pic8259Role::Master);
    out(&mut pic, 0, PIC_ICW1_ACCEPTED);
    out(&mut pic, 1, icw2);
    out(&mut pic, 1, Pic8259Role::Master.icw3());
    out(&mut pic, 1, PIC_ICW4_ACCEPTED);
    out(&mut pic, 1, 0x00);
    (pic, handle)
}

/// S7-32 slice O: in 8086 mode the vector is T7–T3 of ICW2 with the line number in bits 2:0,
/// and bits 2:0 of ICW2 are ignored (8259A data sheet 231468-003, "Content of Interrupt Vector
/// Byte for 8086 System Mode"; "In an 8086 system A15–A11 are inserted in the five most
/// significant bits of the vectoring byte and the 8259A sets the three least significant bits
/// according to the interrupt level. A10–A5 are ignored"). Oracles by hand: the Windows HAL
/// base `0xD8` gives `0xD8..=0xDF`; ICW2 `0xDF` and `0x37` give the same vectors as `0xD8` and
/// `0x30` (an `ICW2 + line` sum would give `0xE0`/`0x38` for line 1 instead).
#[test]
fn the_vector_is_t7_t3_of_icw2_with_the_line_in_bits_2_to_0() {
    for (icw2, base) in [
        (0xD8u8, 0xD8u8),
        (0xDF, 0xD8),
        (0x30, 0x30),
        (0x37, 0x30),
        (0xFF, 0xF8),
        (0x07, 0x00),
    ] {
        for line in 0..8u8 {
            let (mut pic, handle) = master_with_icw2(icw2);
            handle.raise_irq(line);
            let vector = base | line;
            assert_eq!(
                handle.pending_vector(),
                Some(vector),
                "{icw2:#04x} IR{line}"
            );
            // The sum `ICW2 + line` is not the vector when bits 2:0 of ICW2 are set.
            let sum = icw2.wrapping_add(line);
            if sum != vector {
                let before = handle.snapshot();
                assert!(!handle.acknowledge(sum), "{icw2:#04x} IR{line}");
                assert_eq!(handle.snapshot(), before);
            }
            assert!(handle.acknowledge(vector), "{icw2:#04x} IR{line}");
            assert_eq!(handle.snapshot().isr, 1 << line);
            out(&mut pic, 0, 0x20);
            assert_eq!(handle.snapshot().isr, 0);
        }
    }
}
