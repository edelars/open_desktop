//! S4-29d: guest TSC calibration on the 8254 channel 2 in virtual time.
//!
//! The image is assembled here from the guest code bytes (the Python builder
//! validates the same bytes against the clang/lld build, see
//! `tools/build/test_build_stage4_pit_calibrate_probe.py`), so these tests need
//! no toolchain. Every expected number comes from the PIT counting formula
//! `floor(t_ns * 1193182 / 1e9)` and the port-tick rule (1 us per read of
//! `0x61`, value sampled before the tick), evaluated by the builder's reference
//! model or by the closed forms in the tests; none is copied from a run.

use od_machine::{
    PIT_PORT_B_READ_TICK_NS, PitChannel, PitMode, STAGE4_PIT_CALIBRATE_FAILED_FREQUENCY,
    STAGE4_PIT_CALIBRATE_PROBE_IMAGE_BYTES, STAGE4_PIT_CALIBRATE_RECORD_BYTES,
    Stage4PitCalibrateExit, Stage4PitCalibrateInjection, Stage4PitCalibrateProbe,
    Stage4PitCalibrateProbeLoadError, Stage4PitCalibrateRecord, load_stage4_pit_calibrate_probe,
    pit_ticks,
};

const BASE: usize = 0x1000;
/// Guest code of the accepted profile, as encoded by the builder
/// (`expected_code(omit_gate=False)`); the Python test pins it to the builder.
const GUEST_CODE_HEX: &str = concat!(
    "bafb030000b083eebaf8030000b001eebaf9030000b000eebafb030000b003eeba61000000ec24fd0c01eeba43000000",
    "b0b0eeba42000000b09beeba42000000b02eee0f3148c1e2204809d04889c34889c648c7c7ffffffff31ed4531e44531",
    "edba61000000eca82075270f3148c1e2204809d04889c24829f24889c64839fa73034889d74839ea76034889d549ffc4",
    "ebcf4989f64929de4981fce8030000721e488d04bf4801c04839c577124c89f031d2b90a00000048f7f14989c7eb0d41",
    "bd0100000049c7c7ffffffffbaf8030000b041eebaf80300004488e8ee4c89f8e8250100004c89f0e81d0100004c89e0",
    "e8150100004889f8e80d0100004889e8e805010000ba61000000ec24fd0c01eeba43000000b0b0eeba42000000b0ffee",
    "ba42000000b0ffee4531ed31c9ba61000000ecba43000000b080eeba42000000ecec3cff75110f3148c1e2204809d048",
    "89c648ffc1ebd64989ce4889f34883f905770641bd0200000031c9ba61000000ecba43000000b080eeba42000000ecec",
    "3cfe75110f3148c1e2204809d04889c648ffc1ebd64989cc4883f905770641bd030000004989f74929df4585ed75184c",
    "89f8b9de34120048f7e1b900e8030048f7f14889c7eb0748c7c7ffffffffbaf8030000b042eebaf80300004488e8ee48",
    "89f8e8230000004c89f8e81b0000004c89f0e8130000004c89e0e80b000000baf8030000b05aeef4ebfd4989c241bb08",
    "000000baf80300004c89d0ee49c1ea0849ffcb75f3c3",
);
/// After the four COM1 `OUT8` (8 bytes each) comes `GATE_HIGH` (11 bytes); the
/// omit-gate variant drops it.
const UART_INIT_BYTES: usize = 32;
const GATE_HIGH_BYTES: usize = 11;
const BUDGET: u64 = 400_000;

fn code(omit_gate: bool) -> Vec<u8> {
    let bytes: Vec<u8> = (0..GUEST_CODE_HEX.len() / 2)
        .map(|i| u8::from_str_radix(&GUEST_CODE_HEX[2 * i..2 * i + 2], 16).unwrap())
        .collect();
    assert_eq!(
        bytes[UART_INIT_BYTES], 0xba,
        "GATE_HIGH follows the UART setup"
    );
    if omit_gate {
        [
            &bytes[..UART_INIT_BYTES],
            &bytes[UART_INIT_BYTES + GATE_HIGH_BYTES..],
        ]
        .concat()
    } else {
        bytes
    }
}

fn put(image: &mut [u8], gpa: usize, bytes: &[u8]) {
    image[gpa - BASE..gpa - BASE + bytes.len()].copy_from_slice(bytes);
}

fn image(omit_gate: bool) -> Vec<u8> {
    let mut image = vec![0; STAGE4_PIT_CALIBRATE_PROBE_IMAGE_BYTES];
    for (i, d) in [0_u64, 0x00af_9b00_0000_ffff, 0x00cf_9300_0000_ffff]
        .iter()
        .enumerate()
    {
        put(&mut image, 0x2000 + 8 * i, &d.to_le_bytes());
    }
    put(&mut image, 0x4000, &code(omit_gate));
    image
}

fn probe(injection: Stage4PitCalibrateInjection) -> Stage4PitCalibrateProbe {
    load_stage4_pit_calibrate_probe(&image(false), injection).unwrap()
}

/// Reference-model serial record (builder `reference_model(1000)`): the
/// formula-derived bytes of a run with the 1 us port tick.
const EXPECTED_SERIAL_HEX: &str = concat!(
    "410040420f000000000080969800000000001027000000000000e803000000000000e80300000000",
    "0000420031380f0000000000f043030000000000d600000000000000d5000000000000005a"
);
/// Reference model with a 100 us tick per read (negative control).
const COARSE_SERIAL_HEX: &str = concat!(
    "4101ffffffffffffffff80969800000000006400000000000000a086010000000000a0860100000000",
    "004203ffffffffffffffff400d030000000000020000000000000001000000000000005a"
);

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Smallest `j` with `floor(j * tick_ns * 1193182 / 1e9) - start >= count`: the
/// first read of port B (at time `j * tick_ns`) that sees OUT2 high.
fn first_out2_read(start_tick: u64, count: u64, tick_ns: u64) -> u64 {
    (1..)
        .find(|j| pit_ticks(j * tick_ns) - start_tick >= count)
        .unwrap()
}

#[test]
fn the_port_tick_constant_is_one_microsecond() {
    assert_eq!(PIT_PORT_B_READ_TICK_NS, 1_000);
}

#[test]
fn calibration_completes_and_matches_the_formula_derived_values() {
    let mut p = probe(Stage4PitCalibrateInjection::None);
    let report = p.run(BUDGET, || false);
    assert_eq!(report.exit, Stage4PitCalibrateExit::Halted);
    assert_eq!(report.exit.name(), "halted");
    assert!(
        report.steps < BUDGET,
        "budget not exhausted: {}",
        report.steps
    );
    assert_eq!(
        report.retired, report.steps,
        "every step retires one instruction"
    );
    assert_eq!(p.retired_total(), report.retired);
    assert_eq!(hex(p.serial_bytes()), EXPECTED_SERIAL_HEX);
    assert_eq!(p.serial_bytes().len(), STAGE4_PIT_CALIBRATE_RECORD_BYTES);

    let record = p.record().expect("a complete record");
    assert!(record.guest_accepted());
    // Phase A. The first read of 0x61 (gate read-modify-write) is at t = 0 and
    // costs 1 us, so the count is loaded at t = 1000 ns, T0 = floor(1000 *
    // 1193182 / 1e9) = 1. OUT2 rises when T(t) - T0 >= 11931; the loop reads at
    // t = 1000 * j, j >= 1, and stops at the first read that sees OUT2.
    let start = pit_ticks(1_000);
    assert_eq!(start, 1);
    let j = first_out2_read(start, 11_931, 1_000);
    assert_eq!(j, 10_001);
    assert_eq!(
        record.pitcnt,
        j - 1,
        "pitcnt counts the reads that saw OUT2 low"
    );
    assert_eq!(
        record.delta_tsc_a,
        1_000 * record.pitcnt,
        "t2 - t1, one tick per poll"
    );
    assert_eq!((record.tscmin, record.tscmax), (1_000, 1_000));
    assert_eq!(
        record.freq_khz_a,
        record.delta_tsc_a / 10,
        "delta / ms, ms = 10"
    );
    assert_eq!(record.freq_khz_a, 1_000_000, "1 GHz TSC, in kHz");
    // Phase B. Count 0xffff loaded at t = (reads_a + 1) us; the MSB is 0xff for
    // 256 PIT ticks, then 0xfe for 256 ticks.
    assert_eq!((record.stage1_passes, record.stage2_passes), (214, 213));
    assert_eq!(
        record.freq_khz_b,
        record.delta_tsc_b * 1_193_182 / (256 * 1_000)
    );
    assert_eq!(record.delta_tsc_b, 214_000);
    assert_eq!(record.freq_khz_b, 997_425);
    // Reads of 0x61 and the end time of the clock.
    assert_eq!(p.port_b_reads(), 10_432);
    assert_eq!(p.clock_ns(), 10_432_000);
    assert_eq!(p.clock_ns(), p.port_b_reads() * PIT_PORT_B_READ_TICK_NS);
    // Final PIT state: channel 2 in mode 0 holding the 0xffff count, gate high.
    let s = p.pit().snapshot();
    assert!(s.gate2 && !s.speaker_data);
    assert_eq!(s.channel2.mode, Some(PitMode::TerminalCount));
    assert_eq!(s.channel2.run.unwrap().reload, 0xffff);
    assert!(!p.pit().out(PitChannel::Channel2));
}

#[test]
fn the_msb_control_threshold_trips_when_time_moves_too_coarsely() {
    // 100 us per read: pit_calibrate_tsc sees ~100 polls (< loopmin) and the MSB
    // passes of quick_pit_calibrate fall to the "more than 5" threshold.
    let mut p = probe(Stage4PitCalibrateInjection::CoarsePortTick {
        ns_per_read: 100_000,
    });
    let report = p.run(BUDGET, || false);
    assert_eq!(report.exit, Stage4PitCalibrateExit::Halted);
    assert_eq!(hex(p.serial_bytes()), COARSE_SERIAL_HEX);
    let record = p.record().unwrap();
    assert!(!record.guest_accepted());
    assert_eq!((record.status_a, record.status_b), (1, 3));
    assert_eq!(record.freq_khz_a, STAGE4_PIT_CALIBRATE_FAILED_FREQUENCY);
    assert_eq!(record.freq_khz_b, STAGE4_PIT_CALIBRATE_FAILED_FREQUENCY);
    // T0 = floor(100000 * 1193182 / 1e9) = 119; first OUT2 read j: 100 us per read.
    let j = first_out2_read(pit_ticks(100_000), 11_931, 100_000);
    assert_eq!(record.pitcnt, j - 1);
    assert_eq!(record.pitcnt, 100);
    assert!(record.pitcnt < 1_000, "below loopmin");
    assert!(record.stage1_passes <= 5 && record.stage2_passes <= 5);
    assert_eq!(p.port_b_reads(), 108);
    assert_eq!(p.clock_ns(), 10_800_000);
}

/// Instructions of the guest before the polling loop of phase A: COM1 setup
/// (four `OUT8`, 12), gate read-modify-write (5), three `OUT8` (9), RDTSC64 (3) and six register inits.
const PHASE_A_PROLOGUE: u64 = 12 + 5 + 9 + 3 + 6;
/// With frozen time the first iteration of the loop is 17 instructions (the
/// minimum register is written once) and every later one is 16; the `IN` is
/// the second instruction of an iteration.
fn frozen_reads(budget: u64) -> u64 {
    let rem = budget - PHASE_A_PROLOGUE;
    let mut reads = 1; // the read of the gate read-modify-write
    if rem >= 2 {
        reads += 1; // first iteration
        if rem >= 17 {
            let rest = rem - 17;
            reads += rest / 16 + u64::from(rest % 16 >= 2);
        }
    }
    reads
}

#[test]
fn injected_regression_without_port_tick_spins_until_the_budget_runs_out() {
    let mut p = probe(Stage4PitCalibrateInjection::NoPortTick);
    let report = p.run(100_000, || false);
    assert_eq!(report.exit, Stage4PitCalibrateExit::BudgetExhausted);
    assert_eq!(report.exit.name(), "budget_exhausted");
    assert_eq!((report.steps, report.retired), (100_000, 100_000));
    assert_eq!(p.clock_ns(), 0, "time never moves while polling");
    assert_eq!(p.port_b_reads(), frozen_reads(100_000));
    assert_eq!(p.port_b_reads(), 6_249);
    assert!(p.serial_bytes().is_empty());
    assert!(p.record().is_none());
    assert!(!p.pit().out(PitChannel::Channel2), "OUT2 never rises");
    // The same budget on the accepted profile finishes: the regression is
    // distinguishable by (stop, steps, retired, reads of 0x61).
    let mut ok = probe(Stage4PitCalibrateInjection::None);
    let ok_report = ok.run(BUDGET, || false);
    assert_eq!(ok_report.exit.name(), "halted");
    assert_ne!(report.exit, ok_report.exit);
    assert_ne!(ok_report.steps, report.steps);
    assert!(ok.port_b_reads() > p.port_b_reads());
    // A larger budget does not rescue the regression either.
    let mut again = probe(Stage4PitCalibrateInjection::NoPortTick);
    let big = again.run(BUDGET, || false);
    assert_eq!(big.exit, Stage4PitCalibrateExit::BudgetExhausted);
    assert_eq!(big.steps, BUDGET);
    assert_eq!(again.port_b_reads(), frozen_reads(BUDGET));
    assert_eq!(again.clock_ns(), 0);
}

#[test]
fn loading_channel2_without_raising_gate2_is_refused_by_the_model() {
    // Obligation (a) of S4-29c: the fixture raises gate 2 before the load.
    let mut p =
        load_stage4_pit_calibrate_probe(&image(true), Stage4PitCalibrateInjection::None).unwrap();
    let report = p.run(BUDGET, || false);
    assert_eq!(report.exit.name(), "terminal");
    assert!(
        matches!(&report.exit, Stage4PitCalibrateExit::Terminal(cause)
            if format!("{cause:?}").contains("port I/O error")),
        "{:?}",
        report.exit
    );
    // COM1 setup (12) + control word (3) + mov edx, mov al of the refused first
    // data write (2) + the refused OUT, which does not retire.
    assert_eq!((report.steps, report.retired), (18, 17));
    let s = p.pit().snapshot();
    assert!(!s.gate2);
    assert_eq!(s.channel2.run, None);
    assert_eq!(p.port_b_reads(), 0);
    assert!(p.serial_bytes().is_empty());
}

#[test]
fn the_run_is_deterministic_resumable_and_stoppable() {
    let mut one = probe(Stage4PitCalibrateInjection::None);
    let a = one.run(BUDGET, || false);
    let mut two = probe(Stage4PitCalibrateInjection::None);
    // Resume across budget exhaustion: the same final state.
    let first = two.run(1_000, || false);
    assert_eq!(first.exit, Stage4PitCalibrateExit::BudgetExhausted);
    assert_eq!(first.steps, 1_000);
    let rest = two.run(BUDGET, || false);
    assert_eq!(rest.exit, Stage4PitCalibrateExit::Halted);
    assert_eq!(first.steps + rest.steps, a.steps);
    assert_eq!(two.steps_total(), one.steps_total());
    assert_eq!(two.serial_bytes(), one.serial_bytes());
    assert_eq!(
        (two.clock_ns(), two.port_b_reads()),
        (one.clock_ns(), one.port_b_reads())
    );
    // After HLT the finished state is sticky.
    assert_eq!(one.run(10, || false).exit, Stage4PitCalibrateExit::Halted);
    // A stop predicate ends the run with the committed prefix.
    let mut three = probe(Stage4PitCalibrateInjection::None);
    let mut polls = 0;
    let stopped = three.run(BUDGET, || {
        polls += 1;
        polls > 500
    });
    assert_eq!(stopped.exit, Stage4PitCalibrateExit::Stopped);
    assert_eq!(stopped.exit.name(), "stopped");
    assert_eq!(stopped.steps, 500);
}

#[test]
fn the_loader_validates_the_fixed_layout() {
    let load =
        |image: &[u8]| load_stage4_pit_calibrate_probe(image, Stage4PitCalibrateInjection::None);
    assert!(matches!(
        load(&[0; 16]),
        Err(Stage4PitCalibrateProbeLoadError::ImageSize { .. })
    ));
    let mut bad_gdt = image(false);
    bad_gdt[0x1000 + 8] ^= 1;
    assert!(matches!(
        load(&bad_gdt),
        Err(Stage4PitCalibrateProbeLoadError::Layout(_))
    ));
    let mut stray = image(false);
    stray[0x3000 - BASE] = 1;
    assert!(matches!(
        load(&stray),
        Err(Stage4PitCalibrateProbeLoadError::Layout(_))
    ));
    let mut empty = image(false);
    put(&mut empty, 0x4000, &[0; 8]);
    assert!(matches!(
        load(&empty),
        Err(Stage4PitCalibrateProbeLoadError::Layout(_))
    ));
}

#[test]
fn the_record_decoder_is_strict() {
    let mut p = probe(Stage4PitCalibrateInjection::None);
    p.run(BUDGET, || false);
    let bytes = p.serial_bytes().to_vec();
    assert!(Stage4PitCalibrateRecord::decode(&bytes).is_some());
    assert!(Stage4PitCalibrateRecord::decode(&bytes[..76]).is_none());
    for index in [0, 42, 76] {
        let mut bad = bytes.clone();
        bad[index] ^= 0xff;
        assert!(
            Stage4PitCalibrateRecord::decode(&bad).is_none(),
            "marker {index}"
        );
    }
}
