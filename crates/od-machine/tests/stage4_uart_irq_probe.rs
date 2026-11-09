//! S4-24e: UART IRQ4 -> IOAPIC INTIN4 -> Local APIC -> guest handler.
//!
//! The images are assembled here byte-for-byte (mirroring the Python builder's
//! validated encodings) so these tests need no toolchain; the builder-to-runner
//! tests exercise the real clang/lld kernel.

use od_isa::Register;
use od_machine::{
    STAGE4_UART_IRQ_PROBE_IMAGE_BYTES, Stage4UartIrqProbe, Stage4UartIrqProbeExit,
    Stage4UartIrqProbeLoadError, UartIrqTraceKind as K, load_stage4_uart_irq_probe,
};

const BASE: usize = 0x1000;

#[derive(Clone, Copy)]
struct Variant {
    ier: u8,
    late_unmask: bool,
    omit_eoi: bool,
    omit_rbr_read: bool,
}

const NORMAL: Variant = Variant {
    ier: 1,
    late_unmask: false,
    omit_eoi: false,
    omit_rbr_read: false,
};

fn out8(port: u32, value: u8) -> Vec<u8> {
    let mut code = vec![0xba];
    code.extend(port.to_le_bytes());
    code.extend([0xb0, value, 0xee]);
    code
}

fn unmask() -> Vec<u8> {
    [
        &[0xbf, 0x00, 0x00, 0xc0, 0xfe][..],
        &[0xb8, 0x18, 0, 0, 0],
        &[0x89, 0x07],
        &[0xb8, 0x51, 0, 0, 0],
        &[0x89, 0x47, 0x10],
    ]
    .concat()
}

fn entry(v: Variant) -> Vec<u8> {
    let mut code = vec![0x0f, 0x01, 0x5c, 0x24, 0x00];
    code.extend([0xbf, 0x00, 0x00, 0xe0, 0xfe, 0xb8, 0xff, 0x01, 0, 0]);
    code.extend([0x89, 0x87, 0xf0, 0, 0, 0]);
    if !v.late_unmask {
        code.extend(unmask());
    }
    for (port, value) in [
        (0x3fb, 0x83),
        (0x3f8, 0x01),
        (0x3f9, 0x00),
        (0x3fb, 0x03),
        (0x3fc, 0x08),
        (0x3f9, v.ier),
    ] {
        code.extend(out8(port, value));
    }
    if v.late_unmask {
        code.extend(unmask());
    }
    code.extend([0xfb, 0xf4, 0xeb, 0xfd]);
    code
}

fn handler(v: Variant) -> Vec<u8> {
    let mut code = vec![0xba, 0xfa, 0x03, 0, 0, 0xec, 0xba, 0xf8, 0x03, 0, 0];
    if !v.omit_rbr_read {
        code.push(0xec);
    }
    code.push(0xee);
    if !v.omit_eoi {
        code.extend([0xbf, 0x00, 0x00, 0xe0, 0xfe, 0xb8, 0, 0, 0, 0]);
        code.extend([0x89, 0x87, 0xb0, 0, 0, 0]);
    }
    code.extend([0x48, 0xcf]);
    code
}

fn put(image: &mut [u8], gpa: usize, bytes: &[u8]) {
    image[gpa - BASE..gpa - BASE + bytes.len()].copy_from_slice(bytes);
}

fn image(v: Variant) -> Vec<u8> {
    let mut image = vec![0; STAGE4_UART_IRQ_PROBE_IMAGE_BYTES];
    for (i, d) in [0_u64, 0x00af_9b00_0000_ffff, 0x00cf_9300_0000_ffff]
        .iter()
        .enumerate()
    {
        put(&mut image, 0x2000 + 8 * i, &d.to_le_bytes());
    }
    let entry = entry(v);
    let handler_gpa = 0x4000 + entry.len();
    let mut gate = Vec::new();
    gate.extend((handler_gpa as u16).to_le_bytes());
    gate.extend(8_u16.to_le_bytes());
    gate.extend([0, 0x8e, 0, 0]);
    gate.extend([0; 8]);
    put(&mut image, 0x3000 + 0x51 * 16, &gate);
    put(&mut image, 0x4000, &entry);
    put(&mut image, handler_gpa, &handler(v));
    let mut idtr = 0x0fff_u16.to_le_bytes().to_vec();
    idtr.extend(0x3000_u64.to_le_bytes());
    put(&mut image, 0x6ff0, &idtr);
    image
}

fn session(v: Variant, rx: &[(u64, u8)]) -> Stage4UartIrqProbe {
    let mut probe = load_stage4_uart_irq_probe(&image(v)).unwrap();
    for (key, (deadline, byte)) in rx.iter().enumerate() {
        probe.schedule_rx(key as u64, *deadline, *byte).unwrap();
    }
    probe
}

fn kinds(probe: &Stage4UartIrqProbe) -> Vec<K> {
    probe.trace().iter().map(|e| e.kind.clone()).collect()
}

fn position(kinds: &[K], wanted: &K) -> usize {
    kinds
        .iter()
        .position(|k| k == wanted)
        .unwrap_or_else(|| panic!("{wanted:?} missing from {kinds:?}"))
}

fn count(probe: &Stage4UartIrqProbe, f: impl Fn(&K) -> bool) -> usize {
    probe.trace().iter().filter(|e| f(&e.kind)).count()
}

/// The acceptance predicate the runner also applies: idle halt, exact echo,
/// one entry and one EOI per byte, and a clean LAPIC afterwards.
fn accepts(probe: &Stage4UartIrqProbe, exit: &Stage4UartIrqProbeExit, expected: &[u8]) -> bool {
    let entries = count(probe, |k| matches!(k, K::IrqEntry { .. }));
    let eois = count(probe, |k| matches!(k, K::Eoi { .. }));
    let irets = count(probe, |k| matches!(k, K::Iretq));
    *exit == Stage4UartIrqProbeExit::IdleHalt
        && probe.serial_bytes() == expected
        && entries == expected.len()
        && eois == expected.len()
        && irets == expected.len()
        && probe.vector_state() == (false, false)
}

#[test]
fn rx_byte_runs_irr_isr_handler_eoi_iretq_and_returns_to_hlt() {
    let mut probe = session(NORMAL, &[(1000, 0x41)]);
    let report = probe.run(500, || false);
    assert_eq!(report.exit, Stage4UartIrqProbeExit::IdleHalt);
    assert!(accepts(&probe, &report.exit, &[0x41]));
    let kinds = kinds(&probe);
    let order = [
        K::Lidt,
        K::Sti,
        K::Hlt,
        K::IdleAdvance { to_ns: 1000 },
        K::RxDelivered { byte: 0x41 },
        K::UartLine { high: true },
        K::Irr { vector: 0x51 },
        K::IrqEntry { vector: 0x51 },
        K::PortIn {
            port: 0x3fa,
            value: 0x04,
        },
        K::PortIn {
            port: 0x3f8,
            value: 0x41,
        },
        K::UartLine { high: false },
        K::PortOut {
            port: 0x3f8,
            value: 0x41,
        },
        K::Eoi { vector: 0x51 },
        K::Iretq,
    ];
    let positions: Vec<usize> = order.iter().map(|k| position(&kinds, k)).collect();
    assert!(positions.windows(2).all(|w| w[0] < w[1]), "{kinds:?}");
    // Exactly one HLT after IRETQ returns, then the guest idles.
    assert_eq!(count(&probe, |k| matches!(k, K::Hlt)), 2);
    assert_eq!(
        kinds.iter().rposition(|k| *k == K::Hlt).unwrap(),
        kinds.len() - 1
    );
    let state = probe.state();
    assert_eq!(state.activity, Some(od_isa::CpuActivity::Halted));
    assert_eq!(state.gpr[Register::Rsp as usize], 0x6ff0);
    assert_eq!(state.rflags & (1 << 9), 1 << 9);
    // The handler starts right after the entry code; IRETQ returns to the
    // instruction after the first HLT (the idle JMP), then HLT runs again.
    let handler_rip = 0x4000 + entry(NORMAL).len() as u64;
    let rip_of = |wanted: &K| {
        probe
            .trace()
            .iter()
            .find(|e| e.kind == *wanted)
            .unwrap()
            .rip
    };
    assert_eq!(rip_of(&K::IrqEntry { vector: 0x51 }), handler_rip);
    assert_eq!(rip_of(&K::Iretq), 0x4000 + entry(NORMAL).len() as u64 - 2);
    assert_eq!(probe.clock_ns(), 1000);
    assert_eq!(probe.uart().peek_iir(), 0x01);
    assert!(!probe.uart().interrupt_line());
}

#[test]
fn rx_due_before_the_guest_enables_ier_is_latched_then_delivered_after_sti() {
    let mut probe = session(NORMAL, &[(0, 0x5a)]);
    let report = probe.run(500, || false);
    assert!(
        accepts(&probe, &report.exit, &[0x5a]),
        "{:?}",
        kinds(&probe)
    );
    let kinds = kinds(&probe);
    // The byte is waiting before IER is written; the edge appears at the IER
    // write, is held pending with IF clear, and is delivered after STI;HLT.
    assert!(position(&kinds, &K::Irr { vector: 0x51 }) < position(&kinds, &K::Sti));
    assert!(position(&kinds, &K::Sti) < position(&kinds, &K::IrqEntry { vector: 0x51 }));
    assert_eq!(count(&probe, |k| matches!(k, K::IdleAdvance { .. })), 0);
}

#[test]
fn two_bytes_are_served_one_irq_each_with_eoi_between() {
    let mut probe = session(NORMAL, &[(1000, 0x41), (2000, 0x42)]);
    let report = probe.run(1000, || false);
    assert!(accepts(&probe, &report.exit, &[0x41, 0x42]));
    let entries: Vec<usize> = probe
        .trace()
        .iter()
        .enumerate()
        .filter(|(_, e)| matches!(e.kind, K::IrqEntry { .. }))
        .map(|(i, _)| i)
        .collect();
    let eoi: Vec<usize> = probe
        .trace()
        .iter()
        .enumerate()
        .filter(|(_, e)| matches!(e.kind, K::Eoi { .. }))
        .map(|(i, _)| i)
        .collect();
    assert!(entries[0] < eoi[0] && eoi[0] < entries[1] && entries[1] < eoi[1]);
}

#[test]
fn negative_ier_zero_raises_no_irq_and_never_enters_the_handler() {
    let mut probe = session(Variant { ier: 0, ..NORMAL }, &[(1000, 0x41)]);
    let report = probe.run(500, || false);
    assert_eq!(report.exit, Stage4UartIrqProbeExit::IdleHalt);
    // The byte is waiting in RBR and visible in LSR, but nothing is signalled.
    assert_eq!(probe.uart().received_byte(), Some(0x41));
    assert_eq!(probe.uart().peek_iir(), 0x01);
    assert_eq!(count(&probe, |k| matches!(k, K::RxDelivered { .. })), 1);
    assert_eq!(count(&probe, |k| matches!(k, K::UartLine { .. })), 0);
    assert_eq!(count(&probe, |k| matches!(k, K::Irr { .. })), 0);
    assert_eq!(count(&probe, |k| matches!(k, K::IrqEntry { .. })), 0);
    assert!(probe.serial_bytes().is_empty());
    assert_eq!(probe.vector_state(), (false, false));
    assert!(!accepts(&probe, &report.exit, &[0x41]));
}

#[test]
fn negative_masked_intin4_loses_the_edge_and_unmasking_does_not_replay_it() {
    // RX is due before the guest runs. The late-unmask guest enables IER while
    // INTIN4 is still masked (edge ignored), then unmasks with the UART output
    // still asserted: S4-26a/b define no replay of a masked edge.
    let mut probe = session(
        Variant {
            late_unmask: true,
            ..NORMAL
        },
        &[(0, 0x41)],
    );
    let report = probe.run(500, || false);
    assert_eq!(report.exit, Stage4UartIrqProbeExit::IdleHalt);
    let kinds = kinds(&probe);
    assert!(kinds.contains(&K::UartLine { high: true }));
    assert_eq!(count(&probe, |k| matches!(k, K::Irr { .. })), 0);
    assert_eq!(count(&probe, |k| matches!(k, K::IrqEntry { .. })), 0);
    assert!(probe.serial_bytes().is_empty());
    assert_eq!(probe.vector_state(), (false, false));
    // The UART condition is still pending (output high, byte unread).
    assert!(probe.uart().interrupt_line());
    assert_eq!(probe.uart().received_byte(), Some(0x41));
    assert!(!accepts(&probe, &report.exit, &[0x41]));
}

#[test]
fn negative_second_irq_without_eoi_is_not_delivered_while_isr_is_set() {
    let mut probe = session(
        Variant {
            omit_eoi: true,
            ..NORMAL
        },
        &[(1000, 0x41), (2000, 0x42)],
    );
    let report = probe.run(1000, || false);
    assert_eq!(report.exit, Stage4UartIrqProbeExit::IdleHalt);
    // The second edge latches in the IRR but the first vector is still in
    // service, so the CPU never enters the handler a second time.
    assert_eq!(count(&probe, |k| matches!(k, K::IrqEntry { .. })), 1);
    assert_eq!(count(&probe, |k| matches!(k, K::Irr { .. })), 2);
    assert_eq!(count(&probe, |k| matches!(k, K::Eoi { .. })), 0);
    assert_eq!(probe.vector_state(), (true, true));
    assert_eq!(probe.serial_bytes(), [0x41]);
    assert!(!accepts(&probe, &report.exit, &[0x41, 0x42]));
}

#[test]
fn injected_regressions_break_the_acceptance_predicate() {
    for (name, variant) in [
        (
            "no EOI",
            Variant {
                omit_eoi: true,
                ..NORMAL
            },
        ),
        (
            "no RBR read",
            Variant {
                omit_rbr_read: true,
                ..NORMAL
            },
        ),
    ] {
        let mut probe = session(variant, &[(1000, 0x41), (2000, 0x42)]);
        let report = probe.run(1000, || false);
        assert!(
            !accepts(&probe, &report.exit, &[0x41, 0x42]),
            "{name} must fail the predicate"
        );
    }
    let mut good = session(NORMAL, &[(1000, 0x41), (2000, 0x42)]);
    let report = good.run(1000, || false);
    assert!(accepts(&good, &report.exit, &[0x41, 0x42]));

    // Without the RBR read the handler echoes IIR (0x04) instead of the byte,
    // the receive register stays full and the second byte stays queued.
    let mut probe = session(
        Variant {
            omit_rbr_read: true,
            ..NORMAL
        },
        &[(1000, 0x41), (2000, 0x42)],
    );
    probe.run(1000, || false);
    assert_eq!(probe.serial_bytes(), [0x04]);
    assert_eq!(probe.uart().received_byte(), Some(0x41));
    assert_eq!(probe.pending_rx(), 1);
}

#[test]
fn budget_stop_and_split_runs_are_deterministic() {
    let rx = [(1000, 0x41), (2000, 0x42)];
    let mut whole = session(NORMAL, &rx);
    let whole_report = whole.run(1000, || false);
    let mut split = session(NORMAL, &rx);
    let mut quanta = 0;
    let mut retired = 0;
    loop {
        let report = split.run(3, || false);
        quanta += report.quanta;
        retired += report.retired;
        if report.exit != Stage4UartIrqProbeExit::BudgetExhausted {
            assert_eq!(report.exit, Stage4UartIrqProbeExit::IdleHalt);
            break;
        }
        assert_eq!(report.quanta, 3);
    }
    assert_eq!(kinds(&whole), kinds(&split));
    assert_eq!(whole.state(), split.state());
    assert_eq!(quanta, whole_report.quanta);
    assert_eq!(retired, whole_report.retired);

    // STOP precedes budget and executes nothing.
    let mut stopped = session(NORMAL, &rx);
    let report = stopped.run(10, || true);
    assert_eq!(report.exit, Stage4UartIrqProbeExit::Stopped);
    assert_eq!((report.quanta, report.retired), (0, 0));
    assert!(stopped.trace().is_empty());
    let mut zero = session(NORMAL, &rx);
    assert_eq!(
        zero.run(0, || false).exit,
        Stage4UartIrqProbeExit::BudgetExhausted
    );
    // A completed idle run stays idle on further calls.
    let again = whole.run(10, || false);
    assert_eq!(again.exit, Stage4UartIrqProbeExit::IdleHalt);
    assert_eq!(again.retired, 0);
}

#[test]
fn guest_that_halts_with_if_clear_never_wakes() {
    // Skip STI: mutate the idle sequence into NOP; HLT; JMP.
    let mut image = image(NORMAL);
    // STI is the third byte from the end of the entry code (`FB F4 EB FD`).
    let sti = 0x4000 + entry(NORMAL).len() - 4;
    assert_eq!(image[sti - BASE], 0xfb);
    image[sti - BASE] = 0x90;
    let mut probe = load_stage4_uart_irq_probe(&image).unwrap();
    probe.schedule_rx(0, 1000, 0x41).unwrap();
    let report = probe.run(500, || false);
    assert_eq!(report.exit, Stage4UartIrqProbeExit::IdleHalt);
    assert_eq!(count(&probe, |k| matches!(k, K::IrqEntry { .. })), 0);
    // Edge latched, but IF=0 blocks entry.
    assert_eq!(probe.vector_state(), (true, false));
}

#[test]
fn loader_rejects_images_outside_the_fixed_profile() {
    let good = image(NORMAL);
    assert!(load_stage4_uart_irq_probe(&good).is_ok());
    assert!(matches!(
        load_stage4_uart_irq_probe(&good[1..]),
        Err(Stage4UartIrqProbeLoadError::ImageSize { .. })
    ));
    let mutate = |gpa: usize, value: u8| {
        let mut bad = good.clone();
        bad[gpa - BASE] = value;
        bad
    };
    for (what, bad) in [
        ("GDT", mutate(0x200f, 0x01)),
        ("extra IDT gate", mutate(0x3000, 1)),
        ("gate type", mutate(0x3000 + 0x51 * 16 + 5, 0x8f)),
        ("gate selector", mutate(0x3000 + 0x51 * 16 + 2, 0x10)),
        ("entry LIDT", mutate(0x4000, 0x90)),
        ("IDTR limit", mutate(0x6ff0, 0x10)),
        ("stray byte", mutate(0x5000, 1)),
        ("stack byte", mutate(0x6000, 1)),
    ] {
        assert!(
            matches!(
                load_stage4_uart_irq_probe(&bad),
                Err(Stage4UartIrqProbeLoadError::Layout(_))
            ),
            "{what} must be rejected"
        );
    }
}
