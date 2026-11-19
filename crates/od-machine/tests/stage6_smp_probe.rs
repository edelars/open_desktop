//! S6-16e/f: the `Stage6SmpProbeV2` guest starts an application processor with INIT and SIPI,
//! exchanges Fixed IPIs with it and meets the negative cases (second SIPI, IPI to a missing APIC
//! ID, NMI to the AP, one processor).
//!
//! The image comes from the real builder (`tools/build/build_stage6_smp_probe.py`, run with
//! `python3`; there is no skip when it is missing: the test fails). The expectations are the
//! builder's manifest files, derived from the protocol and the SDM and not from a run of the
//! machine (`expected_uart_n1.txt`, `expected_uart_n2.txt` with `{nonce}`), and the cells below,
//! derived from the protocol by hand:
//!
//! * LOCK cell (`LOCK BTS` of the trampoline) = 1, flag "AP up" = 1, start counter = 1;
//! * marker of each processor: magic, the APIC ID register as the processor read it (`0` for the
//!   boot processor, `1 << 24` for AP 1: Intel SDM Vol. 3A 10.4.6, the ID is in bits 31:24) and the
//!   nonce copied from the info block, zero padded;
//! * INIT applied once and SIPI started the AP once (the INIT deassert is a no-op, SMP_CONTRACT.md 5).

use od_machine::{ApLegacyRefusal, FirmwareBoardError};
use od_machine::{
    STAGE6_SMP_PROBE_AP_RX_GPA, STAGE6_SMP_PROBE_BSP_RX_GPA, STAGE6_SMP_PROBE_COUNT_GPA,
    STAGE6_SMP_PROBE_FLAG_GPA, STAGE6_SMP_PROBE_IMAGE_BYTES, STAGE6_SMP_PROBE_INFO_GPA,
    STAGE6_SMP_PROBE_LOCK_GPA, STAGE6_SMP_PROBE_MARKER_AP_GPA, STAGE6_SMP_PROBE_MARKER_BSP_GPA,
    STAGE6_SMP_PROBE_MARKER_MAGIC, STAGE6_SMP_PROBE_NMI_GPA, STAGE6_SMP_PROBE_NONCE_MAX,
    STAGE6_SMP_PROBE_VECTOR_AP, STAGE6_SMP_PROBE_VECTOR_BSP, Stage6SmpProbe, Stage6SmpProbeFinish,
    Stage6SmpProbeLoadError, Stage6SmpProbeParallelFinish, Stage6SmpProbeQuantum,
    Stage6SmpProbeRunError, load_stage6_smp_probe,
};
use od_machine::{
    STAGE6_SMP_PROBE_IDTR_BSP_GPA, STAGE6_SMP_PROBE_MAX_FIXED_QUANTUM, STAGE6_SMP_PROBE_SD_ACK_GPA,
    STAGE6_SMP_PROBE_SD_POST_GPA, STAGE6_SMP_PROBE_SD_PTE_GPA,
    STAGE6_SMP_PROBE_SD_SEEN_HANDLER_GPA, STAGE6_SMP_PROBE_SD_SEEN_MAIN_GPA,
    STAGE6_SMP_PROBE_SD_X_LINEAR, STAGE6_SMP_PROBE_VECTOR_SHOOTDOWN,
};
use std::path::{Path, PathBuf};
use std::process::Command;

const NONCE: &[u8] = b"s6-16e-nonce";
const BUDGET: u64 = 20_000_000;

struct Built {
    image: Vec<u8>,
    dir: PathBuf,
}

impl Built {
    fn expected(&self, vcpus: usize) -> Option<String> {
        let path = self.dir.join(format!("expected_uart_n{vcpus}.txt"));
        std::fs::read_to_string(path)
            .ok()
            .map(|text| text.replace("{nonce}", std::str::from_utf8(NONCE).unwrap()))
    }
}

impl Built {
    /// A hex field of one expected memory cell in `build_manifest.json` (`expected.n<vcpus>.memory`).
    /// The manifest is written by the builder from the protocol, never from a run of the machine.
    /// There is no JSON crate here: the file is the builder's `indent=2, sort_keys` output, so the
    /// first `"<field>"` after `"<cell>": {` of the section `"n<vcpus>": {` belongs to that cell.
    fn manifest_cell(&self, vcpus: usize, cell: &str, field: &str) -> Option<u64> {
        let text = std::fs::read_to_string(self.dir.join("build_manifest.json")).ok()?;
        let section = text.find(&format!("\"n{vcpus}\": {{"))?;
        let rest = &text[section..];
        let cell_at = rest.find(&format!("\"{cell}\": {{"))?;
        let rest = &rest[cell_at..];
        // The cell object ends at the first closing brace: its fields are strings, no nesting.
        let object = &rest[..rest.find('}')?];
        let field_at = object.find(&format!("\"{field}\": \"0x"))?;
        let digits = &object[field_at + format!("\"{field}\": \"0x").len()..];
        u64::from_str_radix(&digits[..digits.find('"')?], 16).ok()
    }

    /// A number of the section `n<vcpus>` of the manifest (`"ap_started": 1`).
    fn manifest_number(&self, vcpus: usize, key: &str) -> u64 {
        let text = std::fs::read_to_string(self.dir.join("build_manifest.json")).unwrap();
        let section = text.find(&format!("\"n{vcpus}\": {{")).unwrap();
        let rest = &text[section..];
        let at = rest
            .find(&format!("\"{key}\": "))
            .unwrap_or_else(|| panic!("manifest has no {key}"));
        let digits = &rest[at + key.len() + 4..];
        digits[..digits.find([',', '\n']).unwrap()]
            .trim()
            .parse()
            .unwrap()
    }

    fn manifest_text(&self) -> String {
        std::fs::read_to_string(self.dir.join("build_manifest.json")).unwrap()
    }

    fn expected_dword(&self, vcpus: usize, cell: &str) -> u32 {
        self.manifest_cell(vcpus, cell, "value")
            .unwrap_or_else(|| panic!("manifest has no value for {cell} (n{vcpus})")) as u32
    }

    fn expected_apic_id_raw(&self, vcpus: usize, cell: &str) -> u32 {
        self.manifest_cell(vcpus, cell, "apic_id_raw")
            .unwrap_or_else(|| panic!("manifest has no apic_id_raw for {cell} (n{vcpus})"))
            as u32
    }
}

impl Drop for Built {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn build(variant: &str) -> Built {
    build_scenario(variant, "normal", None)
}

fn build_scenario(variant: &str, scenario: &str, rounds: Option<u32>) -> Built {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let dir = std::env::temp_dir().join(format!(
        "od-stage6-smp-probe-{}-{variant}-{scenario}-{}-{:?}",
        std::process::id(),
        rounds.unwrap_or(0),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    let mut command = Command::new("python3");
    command
        .arg(root.join("tools/build/build_stage6_smp_probe.py"))
        .args(["--variant", variant, "--scenario", scenario]);
    if let Some(rounds) = rounds {
        command.args(["--rounds", &rounds.to_string()]);
    }
    let output = command
        .args(["--output-dir"])
        .arg(&dir)
        .output()
        .expect("python3 runs the builder");
    assert!(
        output.status.success(),
        "builder failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let image = std::fs::read(dir.join("probe.bin")).expect("the builder wrote probe.bin");
    Built { image, dir }
}

fn dword(probe: &mut Stage6SmpProbe, gpa: u64) -> u32 {
    u32::from_le_bytes(probe.read_ram(gpa, 4).try_into().unwrap())
}

fn padded_nonce() -> Vec<u8> {
    let mut bytes = NONCE.to_vec();
    bytes.resize(48, 0);
    bytes
}

fn check_marker(probe: &mut Stage6SmpProbe, gpa: u64, apic_id_register: u32) {
    assert_eq!(dword(probe, gpa), STAGE6_SMP_PROBE_MARKER_MAGIC, "magic");
    assert_eq!(dword(probe, gpa + 4), apic_id_register, "APIC ID register");
    assert_eq!(probe.read_ram(gpa + 16, 48), padded_nonce(), "nonce");
}

fn n2_run(built: &Built, quantum: Stage6SmpProbeQuantum) -> (Stage6SmpProbe, String) {
    let mut probe = load_stage6_smp_probe(&built.image, 2, NONCE).unwrap();
    let report = probe.run(quantum, BUDGET).unwrap();
    assert_eq!(report.finished, Stage6SmpProbeFinish::AllIdle);
    let uart = String::from_utf8(report.com1.clone()).unwrap();
    // The boot processor and the AP are both halted; the AP left the trampoline's legacy model.
    assert_eq!(report.cpus.len(), 2);
    assert_eq!(report.cpus[1].init_applied, 1);
    assert_eq!(report.cpus[1].started, 1);
    assert!(!report.cpus[1].legacy);
    assert_eq!(
        report.cpus[0].activity,
        od_isa::CpuActivity::Halted,
        "BSP halts at the end"
    );
    assert_eq!(report.cpus[1].activity, od_isa::CpuActivity::Halted);
    (probe, uart)
}

#[test]
fn two_processors_the_bsp_starts_the_ap_and_every_marker_comes_from_guest_code() {
    let built = build("normal");
    let expected = built.expected(2).unwrap();
    assert_eq!(
        expected,
        "BSP0 UP s6-16e-nonce\nAP1 UP s6-16e-nonce\nSMP PROBE OK s6-16e-nonce\n"
    );
    let (mut probe, uart) = n2_run(&built, Stage6SmpProbeQuantum::Fixed(1024));
    assert_eq!(uart, expected);
    assert_eq!(dword(&mut probe, STAGE6_SMP_PROBE_LOCK_GPA), 1);
    assert_eq!(dword(&mut probe, STAGE6_SMP_PROBE_FLAG_GPA), 1);
    assert_eq!(dword(&mut probe, STAGE6_SMP_PROBE_COUNT_GPA), 1);
    check_marker(&mut probe, STAGE6_SMP_PROBE_MARKER_BSP_GPA, 0);
    check_marker(&mut probe, STAGE6_SMP_PROBE_MARKER_AP_GPA, 1 << 24);
}

#[test]
fn the_run_is_the_same_under_every_quantum_and_every_seed() {
    let built = build("normal");
    let expected = built.expected(2).unwrap();
    let mut quanta = vec![
        Stage6SmpProbeQuantum::Fixed(1),
        Stage6SmpProbeQuantum::Fixed(2),
        Stage6SmpProbeQuantum::Fixed(7),
        Stage6SmpProbeQuantum::Fixed(100),
        Stage6SmpProbeQuantum::Fixed(100_000),
    ];
    quanta.extend((0..16).map(Stage6SmpProbeQuantum::Seeded));
    for quantum in quanta {
        let (mut probe, uart) = n2_run(&built, quantum.clone());
        assert_eq!(uart, expected, "{quantum:?}");
        for (gpa, cell) in [
            (STAGE6_SMP_PROBE_LOCK_GPA, "lock_entered"),
            (STAGE6_SMP_PROBE_FLAG_GPA, "flag_ap_up"),
            (STAGE6_SMP_PROBE_COUNT_GPA, "start_count"),
        ] {
            assert_eq!(
                dword(&mut probe, gpa),
                built.expected_dword(2, cell),
                "{cell} {quantum:?}"
            );
        }
        check_marker(
            &mut probe,
            STAGE6_SMP_PROBE_MARKER_BSP_GPA,
            built.expected_apic_id_raw(2, "marker_bsp"),
        );
        check_marker(
            &mut probe,
            STAGE6_SMP_PROBE_MARKER_AP_GPA,
            built.expected_apic_id_raw(2, "marker_ap"),
        );
    }
}

#[test]
fn one_processor_prints_ap_absent_and_never_a_success() {
    let built = build("normal");
    let expected = built.expected(1).unwrap();
    assert_eq!(
        expected,
        "BSP0 UP s6-16e-nonce\nSMP PROBE AP ABSENT s6-16e-nonce\n"
    );
    let mut probe = load_stage6_smp_probe(&built.image, 1, NONCE).unwrap();
    let report = probe
        .run(Stage6SmpProbeQuantum::Fixed(1024), BUDGET)
        .unwrap();
    assert_eq!(report.finished, Stage6SmpProbeFinish::AllIdle);
    let uart = String::from_utf8(report.com1).unwrap();
    assert_eq!(uart, expected);
    assert!(!uart.contains("OK"));
    // No AP: nothing was sent, no AP ran.
    assert_eq!(dword(&mut probe, STAGE6_SMP_PROBE_LOCK_GPA), 0);
    assert_eq!(dword(&mut probe, STAGE6_SMP_PROBE_FLAG_GPA), 0);
    assert_eq!(dword(&mut probe, STAGE6_SMP_PROBE_COUNT_GPA), 0);
    check_marker(&mut probe, STAGE6_SMP_PROBE_MARKER_BSP_GPA, 0);
}

#[test]
fn an_ap_that_never_raises_the_flag_makes_the_bsp_time_out() {
    let built = build("no_flag");
    let expected = built.expected(2).unwrap();
    assert_eq!(
        expected,
        "BSP0 UP s6-16e-nonce\nAP1 UP s6-16e-nonce\nSMP PROBE FAIL TIMEOUT s6-16e-nonce\n"
    );
    let mut probe = load_stage6_smp_probe(&built.image, 2, NONCE).unwrap();
    let report = probe
        .run(Stage6SmpProbeQuantum::Fixed(1024), BUDGET)
        .unwrap();
    assert_eq!(report.finished, Stage6SmpProbeFinish::AllIdle);
    assert_eq!(String::from_utf8(report.com1).unwrap(), expected);
    assert_eq!(
        dword(&mut probe, STAGE6_SMP_PROBE_FLAG_GPA),
        built.expected_dword(2, "flag_ap_up")
    );
    assert_eq!(built.expected_dword(2, "flag_ap_up"), 0);
    // The AP did start and wrote everything except the flag.
    assert_eq!(
        dword(&mut probe, STAGE6_SMP_PROBE_COUNT_GPA),
        built.expected_dword(2, "start_count")
    );
    assert_eq!(built.expected_dword(2, "start_count"), 1);
    check_marker(
        &mut probe,
        STAGE6_SMP_PROBE_MARKER_AP_GPA,
        built.expected_apic_id_raw(2, "marker_ap"),
    );
}

#[test]
fn a_trampoline_without_the_transition_to_long_mode_is_refused() {
    let built = build("no_long_mode");
    assert!(built.expected(2).is_none(), "the manifest expects an error");
    let mut probe = load_stage6_smp_probe(&built.image, 2, NONCE).unwrap();
    let error = probe
        .run(Stage6SmpProbeQuantum::Fixed(1024), BUDGET)
        .unwrap_err();
    match error {
        Stage6SmpProbeRunError::Board {
            error: FirmwareBoardError::ApLegacyRefused { cpu, refusal },
            com1,
            ..
        } => {
            assert_eq!(cpu, 1);
            assert!(
                matches!(refusal, ApLegacyRefusal::Protected(_)),
                "{refusal:?}"
            );
            // Only the boot processor spoke before the refusal.
            assert_eq!(com1, b"BSP0 UP s6-16e-nonce\n");
        }
        other => panic!("expected ApLegacyRefused, got {other:?}"),
    }
    assert_eq!(dword(&mut probe, STAGE6_SMP_PROBE_COUNT_GPA), 0);
    // The trampoline's LOCK BTS ran before the refusal.
    assert_eq!(
        dword(&mut probe, STAGE6_SMP_PROBE_LOCK_GPA),
        built.expected_dword(2, "lock_entered")
    );
    assert_eq!(built.expected_dword(2, "lock_entered"), 1);
}

#[test]
fn an_ap_that_ran_before_sipi_is_reported_by_the_bsp() {
    let built = build("normal");
    let mut probe = load_stage6_smp_probe(&built.image, 2, NONCE).unwrap();
    // The cell the trampoline's LOCK BTS sets is already 1: "the AP entered before SIPI".
    probe
        .write_ram(STAGE6_SMP_PROBE_LOCK_GPA, &1u32.to_le_bytes())
        .unwrap();
    let report = probe
        .run(Stage6SmpProbeQuantum::Fixed(1024), BUDGET)
        .unwrap();
    assert_eq!(
        String::from_utf8(report.com1).unwrap(),
        "BSP0 UP s6-16e-nonce\nSMP PROBE FAIL EARLY s6-16e-nonce\n"
    );
    assert_eq!(dword(&mut probe, STAGE6_SMP_PROBE_FLAG_GPA), 0);
}

#[test]
fn a_damaged_info_block_stops_the_boot_processor_before_any_access() {
    let built = build("normal");
    let mut probe = load_stage6_smp_probe(&built.image, 2, NONCE).unwrap();
    probe.write_ram(STAGE6_SMP_PROBE_INFO_GPA, &[0; 4]).unwrap();
    let report = probe
        .run(Stage6SmpProbeQuantum::Fixed(1024), BUDGET)
        .unwrap();
    assert_eq!(
        String::from_utf8(report.com1).unwrap(),
        "SMP PROBE FAIL INFO\n"
    );
    assert_eq!(report.cpus[1].init_applied, 0);
}

#[test]
fn the_nonce_is_not_part_of_the_image_and_changes_the_transcript_only() {
    let built = build("normal");
    assert!(
        !built
            .image
            .windows(NONCE.len())
            .any(|window| window == NONCE)
    );
    let mut other = load_stage6_smp_probe(&built.image, 2, b"another-nonce-7").unwrap();
    let report = other
        .run(Stage6SmpProbeQuantum::Fixed(1024), BUDGET)
        .unwrap();
    assert_eq!(
        String::from_utf8(report.com1).unwrap(),
        "BSP0 UP another-nonce-7\nAP1 UP another-nonce-7\nSMP PROBE OK another-nonce-7\n"
    );
}

#[test]
fn the_loader_refuses_what_the_guest_could_not_use() {
    let built = build("normal");
    let load =
        |image: &[u8], vcpus: usize, nonce: &[u8]| load_stage6_smp_probe(image, vcpus, nonce).err();
    assert_eq!(
        load(&built.image[1..], 2, NONCE),
        Some(Stage6SmpProbeLoadError::ImageSize {
            expected: STAGE6_SMP_PROBE_IMAGE_BYTES,
            actual: STAGE6_SMP_PROBE_IMAGE_BYTES - 1
        })
    );
    assert_eq!(
        load(&built.image, 0, NONCE),
        Some(Stage6SmpProbeLoadError::VcpuCount(0))
    );
    assert_eq!(
        load(&built.image, 5, NONCE),
        Some(Stage6SmpProbeLoadError::VcpuCount(5))
    );
    for nonce in [
        &b""[..],
        &[b'a'; STAGE6_SMP_PROBE_NONCE_MAX + 1][..],
        b"has space",
        b"nul\0",
    ] {
        assert!(matches!(
            load(&built.image, 2, nonce),
            Some(Stage6SmpProbeLoadError::Nonce(_))
        ));
    }
    assert!(load(&built.image, 2, &[b'a'; STAGE6_SMP_PROBE_NONCE_MAX]).is_none());
}

// ---- S6-16f: IPI exchange and negative cases --------------------------------------------------

const ROUNDS: u32 = 16;

fn quanta() -> Vec<Stage6SmpProbeQuantum> {
    let mut quanta = vec![
        Stage6SmpProbeQuantum::Fixed(1),
        Stage6SmpProbeQuantum::Fixed(2),
        Stage6SmpProbeQuantum::Fixed(7),
        Stage6SmpProbeQuantum::Fixed(100),
        Stage6SmpProbeQuantum::Fixed(100_000),
    ];
    quanta.extend((0..16).map(Stage6SmpProbeQuantum::Seeded));
    quanta
}

/// The quanta of the scenarios that do not need the long wait of `Fixed(100000)`.
fn quanta_small() -> Vec<Stage6SmpProbeQuantum> {
    let mut quanta = vec![
        Stage6SmpProbeQuantum::Fixed(1),
        Stage6SmpProbeQuantum::Fixed(2),
        Stage6SmpProbeQuantum::Fixed(7),
        Stage6SmpProbeQuantum::Fixed(100),
        Stage6SmpProbeQuantum::Fixed(1024),
    ];
    quanta.extend((0..16).map(Stage6SmpProbeQuantum::Seeded));
    quanta
}

/// Every cell of the manifest of the exchange (`n2`), read from the run.
fn check_exchange_cells(built: &Built, probe: &mut Stage6SmpProbe, context: &str) {
    for (gpa, cell) in [
        (STAGE6_SMP_PROBE_LOCK_GPA, "lock_entered"),
        (STAGE6_SMP_PROBE_FLAG_GPA, "flag_ap_up"),
        (STAGE6_SMP_PROBE_COUNT_GPA, "start_count"),
        (STAGE6_SMP_PROBE_AP_RX_GPA, "ap_rx"),
        (STAGE6_SMP_PROBE_BSP_RX_GPA, "bsp_rx"),
        (STAGE6_SMP_PROBE_NMI_GPA, "nmi_cell"),
    ] {
        assert_eq!(
            dword(probe, gpa),
            built.expected_dword(2, cell),
            "{cell} {context}"
        );
    }
    check_marker(
        probe,
        STAGE6_SMP_PROBE_MARKER_BSP_GPA,
        built.expected_apic_id_raw(2, "marker_bsp"),
    );
    check_marker(
        probe,
        STAGE6_SMP_PROBE_MARKER_AP_GPA,
        built.expected_apic_id_raw(2, "marker_ap"),
    );
}

/// No vector is pending or in service in any Local APIC (every IPI of the exchange was taken and
/// acknowledged with an EOI).
fn assert_lapics_quiet(probe: &Stage6SmpProbe, cpus: usize, context: &str) {
    for cpu in 0..cpus {
        assert_eq!(
            probe.pending_vectors(cpu),
            vec![],
            "IRR/ISR of LAPIC {cpu} {context}"
        );
    }
}

#[test]
fn the_exchange_of_k_fixed_ipis_leaves_both_counters_at_k_and_every_lapic_quiet() {
    let built = build_scenario("normal", "exchange", None);
    let expected = built.expected(2).unwrap();
    assert_eq!(
        expected,
        "BSP0 UP s6-16e-nonce\nAP1 UP s6-16e-nonce\nSMP PROBE OK K=16 s6-16e-nonce\n"
    );
    assert_eq!(built.expected_dword(2, "ap_rx"), ROUNDS);
    assert_eq!(built.expected_dword(2, "bsp_rx"), ROUNDS);
    let (mut probe, uart) = n2_run(&built, Stage6SmpProbeQuantum::Fixed(1024));
    assert_eq!(uart, expected);
    check_exchange_cells(&built, &mut probe, "Fixed(1024)");
    assert_lapics_quiet(&probe, 2, "after the exchange");
    // Both vectors of the exchange are the plan's: the numbers are the image's gates.
    assert_eq!(STAGE6_SMP_PROBE_VECTOR_AP, 0x30);
    assert_eq!(STAGE6_SMP_PROBE_VECTOR_BSP, 0x31);
}

#[test]
fn the_exchange_gives_the_same_transcript_and_cells_under_every_quantum_and_seed() {
    let built = build_scenario("normal", "exchange", None);
    let expected = built.expected(2).unwrap();
    for quantum in quanta() {
        let mut probe = load_stage6_smp_probe(&built.image, 2, NONCE).unwrap();
        let report = probe.run(quantum.clone(), BUDGET).unwrap();
        assert_eq!(
            report.finished,
            Stage6SmpProbeFinish::AllIdle,
            "{quantum:?}"
        );
        assert_eq!(
            String::from_utf8(report.com1.clone()).unwrap(),
            expected,
            "{quantum:?}"
        );
        // At most one wake of the AP per IPI of the exchange (an IPI that comes before the AP
        // halts needs no wake, so the count depends on the schedule), INIT once, SIPI once.
        assert!(report.cpus[1].wakes <= u64::from(ROUNDS), "{quantum:?}");
        assert_eq!(report.cpus[1].init_applied, 1);
        assert_eq!(report.cpus[1].started, 1);
        assert_eq!(report.cpus[0].init_applied, 0);
        check_exchange_cells(&built, &mut probe, &format!("{quantum:?}"));
        assert_lapics_quiet(&probe, 2, &format!("{quantum:?}"));
    }
}

#[test]
fn different_seeds_cut_the_exchange_differently_and_the_same_seed_repeats_it_exactly() {
    let built = build_scenario("normal", "exchange", None);
    let run = |seed: u64| {
        let mut probe = load_stage6_smp_probe(&built.image, 2, NONCE).unwrap();
        probe
            .run_recorded(Stage6SmpProbeQuantum::Seeded(seed), BUDGET)
            .unwrap()
    };
    let (a, b, again) = (run(3), run(4), run(3));
    assert_eq!(a, again, "one seed, one run (counters, schedule, bytes)");
    assert_eq!(a.com1, b.com1);
    assert_ne!(a.turns, b.turns, "the seeds cut the turns differently");
    assert_ne!(a.schedule, b.schedule);
}

#[test]
fn the_exchange_takes_other_round_counts_from_the_build() {
    for rounds in [1u32, 100] {
        let built = build_scenario("normal", "exchange", Some(rounds));
        let expected = built.expected(2).unwrap();
        assert!(expected.contains(&format!("OK K={rounds} ")), "{expected}");
        let (mut probe, uart) = n2_run(&built, Stage6SmpProbeQuantum::Fixed(1024));
        assert_eq!(uart, expected);
        assert_eq!(dword(&mut probe, STAGE6_SMP_PROBE_AP_RX_GPA), rounds);
        assert_eq!(dword(&mut probe, STAGE6_SMP_PROBE_BSP_RX_GPA), rounds);
    }
}

#[test]
fn one_processor_prints_ap_absent_in_every_scenario() {
    for scenario in ["exchange", "resipi", "bad-ipi", "nmi", "shootdown"] {
        let built = build_scenario("normal", scenario, None);
        let expected = built.expected(1).unwrap();
        assert_eq!(
            expected,
            "BSP0 UP s6-16e-nonce\nSMP PROBE AP ABSENT s6-16e-nonce\n"
        );
        let mut probe = load_stage6_smp_probe(&built.image, 1, NONCE).unwrap();
        let report = probe
            .run(Stage6SmpProbeQuantum::Fixed(1024), BUDGET)
            .unwrap();
        assert_eq!(report.finished, Stage6SmpProbeFinish::AllIdle, "{scenario}");
        assert_eq!(String::from_utf8(report.com1).unwrap(), expected);
        assert_eq!(dword(&mut probe, STAGE6_SMP_PROBE_AP_RX_GPA), 0);
        assert_eq!(dword(&mut probe, STAGE6_SMP_PROBE_BSP_RX_GPA), 0);
        assert_lapics_quiet(&probe, 1, scenario);
    }
}

#[test]
fn a_second_sipi_to_the_running_ap_is_ignored_and_the_exchange_goes_on() {
    let built = build_scenario("normal", "resipi", None);
    let plain = build_scenario("normal", "exchange", None);
    assert_ne!(built.image, plain.image, "the second SIPI is in the image");
    let expected = built.expected(2).unwrap();
    assert_eq!(
        expected,
        "BSP0 UP s6-16e-nonce\nAP1 UP s6-16e-nonce\nSMP PROBE OK K=16 s6-16e-nonce\n"
    );
    assert_eq!(built.manifest_number(2, "ap_started"), 1);
    assert_eq!(built.manifest_number(2, "ap_init_applied"), 1);
    for quantum in quanta_small() {
        let mut probe = load_stage6_smp_probe(&built.image, 2, NONCE).unwrap();
        let report = probe.run(quantum.clone(), BUDGET).unwrap();
        assert_eq!(
            report.finished,
            Stage6SmpProbeFinish::AllIdle,
            "{quantum:?}"
        );
        assert_eq!(String::from_utf8(report.com1.clone()).unwrap(), expected);
        // The AP did not start again: SIPI applied once, the trampoline ran once (start counter 1,
        // the manifest value), no second INIT.
        assert_eq!(
            report.cpus[1].started,
            built.manifest_number(2, "ap_started"),
            "{quantum:?}"
        );
        assert_eq!(
            report.cpus[1].init_applied,
            built.manifest_number(2, "ap_init_applied")
        );
        assert!(!report.cpus[1].legacy);
        check_exchange_cells(&built, &mut probe, &format!("{quantum:?}"));
        assert_lapics_quiet(&probe, 2, &format!("{quantum:?}"));
    }
}

/// The text of a device error of the Local APIC as the board reports it: a refused MMIO write at
/// the window of the LAPIC (`0xFEE00000`), error `UnsupportedConfiguration` (SMP_CONTRACT 5: an
/// ICR value outside the closed set is an explicit device error and changes no state).
fn assert_lapic_device_error(error: &FirmwareBoardError, cpu: usize) {
    match error {
        FirmwareBoardError::VcpuQuantum { cpu: at, detail } => {
            assert_eq!(*at, cpu);
            assert!(
                detail.contains("MMIO device") && detail.contains("write failed"),
                "{detail}"
            );
            assert!(detail.contains("UnsupportedConfiguration"), "{detail}");
            assert!(detail.contains("4276093696"), "0xFEE00000: {detail}");
        }
        other => panic!("expected a device error of the LAPIC, got {other:?}"),
    }
}

#[test]
fn an_ipi_to_a_missing_apic_id_ends_the_run_with_a_device_error_and_changes_no_lapic() {
    let built = build_scenario("normal", "bad-ipi", None);
    let expected = built.expected(2).unwrap();
    assert_eq!(
        expected,
        "BSP0 UP s6-16e-nonce\nAP1 UP s6-16e-nonce\nSMP PROBE BAD-IPI s6-16e-nonce\n"
    );
    for quantum in quanta_small() {
        let mut probe = load_stage6_smp_probe(&built.image, 2, NONCE).unwrap();
        assert_lapics_quiet(&probe, 2, "before the run");
        let error = probe.run(quantum.clone(), BUDGET).unwrap_err();
        match error {
            Stage6SmpProbeRunError::Board { error, com1, cpus } => {
                assert_lapic_device_error(&error, 0);
                // The marker is the last thing the guest printed: nothing after it, no failure line.
                assert_eq!(String::from_utf8(com1).unwrap(), expected, "{quantum:?}");
                // The AP was started once and halted; nothing was sent to it.
                assert_eq!(cpus[1].started, 1);
                assert_eq!(cpus[1].wakes, 0, "no IPI reached the AP");
                assert_eq!(cpus[1].nmi_delivered, 0);
                assert_eq!(cpus[1].activity, od_isa::CpuActivity::Halted);
            }
            other => panic!("expected a board error, got {other:?}"),
        }
        // IRR and ISR of both LAPICs are as before the run: empty; the counters of the exchange
        // are untouched; the AP's own marker and the start counter are as after a normal start.
        assert_lapics_quiet(&probe, 2, &format!("after the refused ICR {quantum:?}"));
        for (gpa, cell) in [
            (STAGE6_SMP_PROBE_COUNT_GPA, "start_count"),
            (STAGE6_SMP_PROBE_AP_RX_GPA, "ap_rx"),
            (STAGE6_SMP_PROBE_BSP_RX_GPA, "bsp_rx"),
        ] {
            assert_eq!(
                dword(&mut probe, gpa),
                built.expected_dword(2, cell),
                "{cell}"
            );
        }
    }
    assert!(built.manifest_text().contains("LapicDeviceError"));
}

#[test]
fn the_missing_apic_id_is_the_first_one_past_the_processors_whatever_their_number() {
    let built = build_scenario("normal", "bad-ipi", None);
    for vcpus in [3usize, 4] {
        let mut probe = load_stage6_smp_probe(&built.image, vcpus, NONCE).unwrap();
        let error = probe
            .run(Stage6SmpProbeQuantum::Fixed(1024), BUDGET)
            .unwrap_err();
        match error {
            Stage6SmpProbeRunError::Board { error, com1, cpus } => {
                assert_lapic_device_error(&error, 0);
                assert_eq!(
                    String::from_utf8(com1).unwrap(),
                    built.expected(2).unwrap(),
                    "N={vcpus}"
                );
                // Only the AP the BSP started ran; the others never got SIPI.
                assert_eq!(cpus[1].started, 1);
                assert!(cpus[2..].iter().all(|cpu| cpu.started == 0), "{cpus:?}");
            }
            other => panic!("expected a board error, got {other:?}"),
        }
        assert_lapics_quiet(&probe, vcpus, &format!("N={vcpus}"));
    }
}

#[test]
fn an_nmi_to_the_ap_on_the_board_tables_runs_the_handler_of_vector_2() {
    let built = build_scenario("normal", "nmi", None);
    let expected = built.expected(2).unwrap();
    assert_eq!(
        expected,
        "BSP0 UP s6-16e-nonce\nAP1 UP s6-16e-nonce\nSMP PROBE NMI OK s6-16e-nonce\n"
    );
    for quantum in quanta_small() {
        let mut probe = load_stage6_smp_probe(&built.image, 2, NONCE).unwrap();
        let report = probe.run(quantum.clone(), BUDGET).unwrap();
        assert_eq!(String::from_utf8(report.com1.clone()).unwrap(), expected);
        assert_eq!(report.cpus[1].nmi_delivered, 1, "{quantum:?}");
        assert_eq!(report.cpus[0].nmi_delivered, 0);
        assert_eq!(
            dword(&mut probe, STAGE6_SMP_PROBE_NMI_GPA),
            built.expected_dword(2, "nmi_cell")
        );
        assert_eq!(built.expected_dword(2, "nmi_cell"), 1);
    }
}

/// The page tables of the AP at another root than the board's `page_table_root`: the trampoline's
/// `MOV EAX, 0x2000; MOV CR3, EAX` is patched to a copy of the PML4 page at `0x10000`.
fn patch_ap_cr3(probe: &mut Stage6SmpProbe, new_root: u32) {
    let pml4 = probe.read_ram(0x2000, 0x1000);
    probe.write_ram(u64::from(new_root), &pml4).unwrap();
    let trampoline = probe.read_ram(0x8200, 0x100);
    let pattern = [0xB8, 0x00, 0x20, 0x00, 0x00, 0x0F, 0x22, 0xD8];
    let at = trampoline
        .windows(pattern.len())
        .position(|window| window == pattern)
        .expect("mov eax, PML4; mov cr3, eax in the protected-mode block");
    probe
        .write_ram(0x8200 + at as u64 + 1, &new_root.to_le_bytes())
        .unwrap();
}

#[test]
fn an_nmi_to_an_ap_on_its_own_cr3_is_delivered_and_the_handler_of_vector_2_runs() {
    let built = build_scenario("normal", "nmi", None);
    let expected = built.expected(2).unwrap();
    for quantum in quanta_small() {
        let mut probe = load_stage6_smp_probe(&built.image, 2, NONCE).unwrap();
        patch_ap_cr3(&mut probe, 0x1_0000);
        let report = probe.run(quantum.clone(), BUDGET).unwrap();
        // The board walks the NMI delivery on the CR3 of the vCPU (0x10000, not the board's root):
        // the SDM transcript is complete, the handler of vector 2 stored its cell.
        assert_eq!(
            String::from_utf8(report.com1).unwrap(),
            expected,
            "{quantum:?}"
        );
        assert_eq!(report.cpus[1].nmi_delivered, 1, "{quantum:?}");
        assert_eq!(report.cpus[0].nmi_delivered, 0);
        assert_eq!(
            dword(&mut probe, STAGE6_SMP_PROBE_NMI_GPA),
            built.expected_dword(2, "nmi_cell")
        );
    }
}

#[test]
fn the_bsp_loads_its_idtr_with_lidt_rsp_disp8_and_the_descriptor_decides_whether_the_reply_arrives()
{
    let built = build_scenario("normal", "exchange", None);
    let expected = built.expected(2).unwrap();
    // The descriptor (limit u16, base u64) at 0x8150 is what the guest's `LIDT [RSP+0]` loads.
    let mut probe = load_stage6_smp_probe(&built.image, 2, NONCE).unwrap();
    assert_eq!(
        probe.read_ram(STAGE6_SMP_PROBE_IDTR_BSP_GPA, 10),
        [0xFF, 0x03, 0x00, 0x98, 0, 0, 0, 0, 0, 0]
    );
    let report = probe
        .run(Stage6SmpProbeQuantum::Fixed(1024), BUDGET)
        .unwrap();
    assert_eq!(String::from_utf8(report.com1).unwrap(), expected);
    // The same image with a limit of 0 in the descriptor: the LIDT is executed, the BSP has no gate
    // for vector 0x31 and the reply of the AP cannot be delivered.
    let mut probe = load_stage6_smp_probe(&built.image, 2, NONCE).unwrap();
    probe
        .write_ram(STAGE6_SMP_PROBE_IDTR_BSP_GPA, &[0, 0])
        .unwrap();
    let outcome = probe.run(Stage6SmpProbeQuantum::Fixed(1024), BUDGET);
    match outcome {
        Err(Stage6SmpProbeRunError::Board { error, com1, .. }) => {
            match error {
                FirmwareBoardError::ExternalInterruptDelivery {
                    vector, failure, ..
                } => {
                    assert_eq!(vector, STAGE6_SMP_PROBE_VECTOR_BSP);
                    assert!(failure.contains("GateBeyondIdtLimit"), "{failure}");
                }
                other => panic!("expected a refused delivery, got {other:?}"),
            }
            assert_eq!(
                String::from_utf8(com1).unwrap(),
                "BSP0 UP s6-16e-nonce\nAP1 UP s6-16e-nonce\n"
            );
        }
        other => panic!("a limit of 0 must stop the exchange, got {other:?}"),
    }
}

#[test]
fn the_recorded_maximum_and_5_million_fixed_give_ok_and_6_million_a_timeout_never_a_false_ok() {
    let built = build("normal");
    let expected = built.expected(2).unwrap();
    // 4M is the recorded maximum; 5M still ends OK in the `normal` scenario (the AP's turn comes
    // before the wait of 5.24M BSP steps runs out), 6M is past the wait: a timeout.
    for quantum in [STAGE6_SMP_PROBE_MAX_FIXED_QUANTUM, 5_000_000] {
        let mut probe = load_stage6_smp_probe(&built.image, 2, NONCE).unwrap();
        let report = probe
            .run(Stage6SmpProbeQuantum::Fixed(quantum), 40_000_000)
            .unwrap();
        assert_eq!(
            String::from_utf8(report.com1).unwrap(),
            expected,
            "Fixed({quantum})"
        );
    }
    let mut probe = load_stage6_smp_probe(&built.image, 2, NONCE).unwrap();
    let report = probe
        .run(Stage6SmpProbeQuantum::Fixed(6_000_000), 40_000_000)
        .unwrap();
    let uart = String::from_utf8(report.com1).unwrap();
    assert!(!uart.contains("SMP PROBE OK"), "{uart}");
    assert!(uart.contains("FAIL TIMEOUT"), "{uart}");
}

// ---- S6-17: shootdown by the guest on shared page tables ----------------------------------------

/// Every cell of the manifest of the shootdown scenario (`n2`), read from the run: the counters of
/// acknowledgements and posts, the last values of X the AP read, and the frame of the PTE of X.
fn check_shootdown_cells(built: &Built, probe: &mut Stage6SmpProbe, context: &str) {
    for (gpa, cell) in [
        (STAGE6_SMP_PROBE_SD_ACK_GPA, "sd_ack"),
        (STAGE6_SMP_PROBE_SD_POST_GPA, "sd_post"),
        (STAGE6_SMP_PROBE_SD_SEEN_HANDLER_GPA, "sd_seen_handler"),
        (STAGE6_SMP_PROBE_SD_SEEN_MAIN_GPA, "sd_seen_main"),
    ] {
        assert_eq!(
            dword(probe, gpa),
            built.expected_dword(2, cell),
            "{cell} {context}"
        );
    }
    // The accessed bit of the walks of the AP is in the low bits: the frame is compared.
    assert_eq!(
        dword(probe, STAGE6_SMP_PROBE_SD_PTE_GPA) & !0xFFF,
        built.expected_dword(2, "pte_x_frame"),
        "frame of the PTE of X {context}"
    );
    check_exchange_cells_started(built, probe, context);
}

/// The cells of the start of the AP (the shootdown scenario has no exchange counters).
fn check_exchange_cells_started(built: &Built, probe: &mut Stage6SmpProbe, context: &str) {
    for (gpa, cell) in [
        (STAGE6_SMP_PROBE_LOCK_GPA, "lock_entered"),
        (STAGE6_SMP_PROBE_FLAG_GPA, "flag_ap_up"),
        (STAGE6_SMP_PROBE_COUNT_GPA, "start_count"),
    ] {
        assert_eq!(
            dword(probe, gpa),
            built.expected_dword(2, cell),
            "{cell} {context}"
        );
    }
    check_marker(
        probe,
        STAGE6_SMP_PROBE_MARKER_AP_GPA,
        built.expected_apic_id_raw(2, "marker_ap"),
    );
}

#[test]
fn the_shootdown_of_k_rounds_gives_the_ap_the_new_frame_after_every_invlpg() {
    let built = build_scenario("normal", "shootdown", None);
    let expected = built.expected(2).unwrap();
    assert_eq!(
        expected,
        "BSP0 UP s6-16e-nonce\nAP1 UP s6-16e-nonce\nSMP PROBE SHOOTDOWN OK K=16 s6-16e-nonce\n"
    );
    // K = 16 is even: the last round maps X back to frame A (the data page, 0xC000).
    assert_eq!(built.expected_dword(2, "sd_ack"), ROUNDS);
    assert_eq!(built.expected_dword(2, "sd_post"), ROUNDS);
    assert_eq!(built.expected_dword(2, "sd_seen_main"), 0xA1A1_A1A1);
    assert_eq!(built.expected_dword(2, "pte_x_frame"), 0xC000);
    let (mut probe, uart) = n2_run(&built, Stage6SmpProbeQuantum::Fixed(1024));
    assert_eq!(uart, expected);
    check_shootdown_cells(&built, &mut probe, "Fixed(1024)");
    assert_lapics_quiet(&probe, 2, "after the shootdown");
    assert_eq!(STAGE6_SMP_PROBE_VECTOR_SHOOTDOWN, 0x32);
    assert_eq!(STAGE6_SMP_PROBE_SD_X_LINEAR, 0x20_1100);
}

#[test]
fn the_shootdown_gives_the_same_transcript_and_cells_under_every_quantum_and_seed() {
    let built = build_scenario("normal", "shootdown", None);
    let expected = built.expected(2).unwrap();
    for quantum in quanta() {
        let mut probe = load_stage6_smp_probe(&built.image, 2, NONCE).unwrap();
        let report = probe.run(quantum.clone(), BUDGET).unwrap();
        assert_eq!(
            report.finished,
            Stage6SmpProbeFinish::AllIdle,
            "{quantum:?}"
        );
        assert_eq!(
            String::from_utf8(report.com1.clone()).unwrap(),
            expected,
            "{quantum:?}"
        );
        // At most one wake of the AP per round; INIT once, SIPI once.
        assert!(report.cpus[1].wakes <= u64::from(ROUNDS), "{quantum:?}");
        assert_eq!(report.cpus[1].init_applied, 1);
        assert_eq!(report.cpus[1].started, 1);
        assert!(!report.cpus[1].legacy);
        check_shootdown_cells(&built, &mut probe, &format!("{quantum:?}"));
        assert_lapics_quiet(&probe, 2, &format!("{quantum:?}"));
    }
}

#[test]
fn the_shootdown_takes_other_round_counts_from_the_build_and_ends_on_the_matching_frame() {
    // Odd K ends on frame B (the info page, 0x7000, value 0xB2B2B2B2), even K on frame A.
    for (rounds, frame, value) in [
        (1u32, 0x7000u32, 0xB2B2_B2B2u32),
        (2, 0xC000, 0xA1A1_A1A1),
        (3, 0x7000, 0xB2B2_B2B2),
        (100, 0xC000, 0xA1A1_A1A1),
    ] {
        let built = build_scenario("normal", "shootdown", Some(rounds));
        let expected = built.expected(2).unwrap();
        assert!(
            expected.contains(&format!("SHOOTDOWN OK K={rounds} ")),
            "{expected}"
        );
        assert_eq!(built.expected_dword(2, "pte_x_frame"), frame);
        assert_eq!(built.expected_dword(2, "sd_seen_main"), value);
        let (mut probe, uart) = n2_run(&built, Stage6SmpProbeQuantum::Fixed(1024));
        assert_eq!(uart, expected);
        check_shootdown_cells(&built, &mut probe, &format!("K={rounds}"));
    }
}

/// Replace the one occurrence of `pattern` in the image with `NOP`s (`0x90`).
fn nop_out(image: &mut [u8], pattern: &[u8]) {
    let hits: Vec<usize> = image
        .windows(pattern.len())
        .enumerate()
        .filter(|(_, window)| *window == pattern)
        .map(|(at, _)| at)
        .collect();
    assert_eq!(hits.len(), 1, "exactly one {pattern:02x?} in the image");
    image[hits[0]..hits[0] + pattern.len()].fill(0x90);
}

#[test]
fn a_bsp_that_does_not_change_the_pte_is_caught_because_the_ap_reads_through_the_shared_table() {
    let built = build_scenario("normal", "shootdown", None);
    // `mov [0x6008], eax` (the PTE of X in the shared PT) removed from the BSP's round.
    let mut image = built.image.clone();
    nop_out(&mut image, &[0x89, 0x04, 0x25, 0x08, 0x60, 0x00, 0x00]);
    let mut probe = load_stage6_smp_probe(&image, 2, NONCE).unwrap();
    let report = probe
        .run(Stage6SmpProbeQuantum::Fixed(1024), BUDGET)
        .unwrap();
    let uart = String::from_utf8(report.com1).unwrap();
    assert!(!uart.contains("SHOOTDOWN OK"), "{uart}");
    assert!(
        uart.ends_with("SMP PROBE FAIL SHOOTDOWN s6-16e-nonce\n"),
        "{uart}"
    );
}

#[test]
fn without_the_invlpg_the_machine_without_a_tlb_still_gives_the_new_frame_so_the_probe_is_not_sensitive_to_it()
 {
    // S6-17 step 5: no translation cache of the vCPU outlives a quantum (the quantum of the board
    // has no fetch cache and the data walk is uncached), so the property "after its INVLPG the AP
    // does not use the old translation" holds trivially. This test records exactly that: the
    // `INVLPG [r8]` (`41 0F 01 38`) of the handler is replaced with NOPs and the run is still OK.
    // It is not a proof that the INVLPG works; when a TLB appears, this test must start failing and
    // be replaced by the sensitivity check of the plan.
    let built = build_scenario("normal", "shootdown", None);
    let expected = built.expected(2).unwrap();
    let mut image = built.image.clone();
    nop_out(&mut image, &[0x41, 0x0F, 0x01, 0x38]);
    for quantum in quanta_small() {
        let mut probe = load_stage6_smp_probe(&image, 2, NONCE).unwrap();
        let report = probe.run(quantum.clone(), BUDGET).unwrap();
        assert_eq!(
            String::from_utf8(report.com1).unwrap(),
            expected,
            "{quantum:?}"
        );
    }
}

// ---- S6-24: stop every vCPU, take the state, reset, restore, go on ---------------------------

/// The low 64 KiB of RAM: everything the probe image, its stacks and its tables touch.
const RAM_COMPARED: usize = 0x1_0000;

/// The exchange run in one piece through a session: the report, the low RAM and the virtual time at
/// the end.
fn continuous(
    built: &Built,
    quantum: Stage6SmpProbeQuantum,
) -> (od_machine::Stage6SmpProbeReport, Vec<u8>, u64) {
    let mut probe = load_stage6_smp_probe(&built.image, 2, NONCE).unwrap();
    let report = {
        let mut session = probe.session(quantum, true).unwrap();
        session.run(BUDGET).unwrap()
    };
    assert_eq!(report.finished, Stage6SmpProbeFinish::AllIdle);
    let (ram, now) = (probe.read_ram(0, RAM_COMPARED), probe.now_ns());
    (report, ram, now)
}

/// The same run, cut after `cut` retired instructions (all vCPUs together): stop every vCPU, take
/// the state; with `reset` the CPU complex and the scheduler go to the state of power-on (RAM,
/// devices and the clock stay) and the state is put back; then the run goes on with the rest of
/// the budget. The last value says what waited in the Local APIC of the AP at the stop:
/// `(lapic_pending(1), a request in its IRR that was not accepted yet)`.
fn cut_run(
    built: &Built,
    quantum: Stage6SmpProbeQuantum,
    cut: u64,
    reset: bool,
) -> (
    od_machine::Stage6SmpProbeReport,
    Vec<u8>,
    u64,
    (Option<bool>, bool),
) {
    let mut probe = load_stage6_smp_probe(&built.image, 2, NONCE).unwrap();
    let waiting;
    let report = {
        let mut session = probe.session(quantum.clone(), true).unwrap();
        let first = session.run(cut).unwrap();
        assert_eq!(first.finished, Stage6SmpProbeFinish::BudgetExhausted);
        session.pause(8).unwrap();
        assert!(session.paused());
        let snapshot = session.snapshot().unwrap();
        waiting = (
            session.lapic_pending(1),
            session
                .pending_vectors(1)
                .iter()
                .any(|&(_, irr, isr)| irr && !isr),
        );
        // A paused machine does not run and does not move its clock.
        let stopped = session.run(BUDGET).unwrap();
        assert_eq!(
            stopped.finished,
            Stage6SmpProbeFinish::Stopped,
            "{quantum:?}"
        );
        assert_eq!(stopped.turns, first.turns, "{quantum:?}");
        assert_eq!(
            stopped.cpus.iter().map(|c| c.retired).sum::<u64>(),
            cut,
            "{quantum:?}"
        );
        if reset {
            let activities: Vec<_> = (0..2).map(|cpu| session.activity(cpu)).collect();
            session.reset().unwrap();
            // The application processor waits for SIPI again; no LAPIC holds anything pending.
            assert_eq!(
                session.activity(1),
                Some(od_isa::CpuActivity::WaitForSipi),
                "{quantum:?} cut {cut} (before: {activities:?})"
            );
            assert_eq!(session.activity(0), Some(od_isa::CpuActivity::Running));
            assert_eq!(session.lapic_pending(0), Some(false));
            assert_eq!(session.lapic_pending(1), Some(false));
            session.restore(&snapshot).unwrap();
            assert_eq!(
                (0..2).map(|cpu| session.activity(cpu)).collect::<Vec<_>>(),
                activities,
                "{quantum:?} cut {cut}"
            );
        }
        session.resume();
        session.run(BUDGET - cut).unwrap()
    };
    assert_eq!(
        report.finished,
        Stage6SmpProbeFinish::AllIdle,
        "{quantum:?}"
    );
    let (ram, now) = (probe.read_ram(0, RAM_COMPARED), probe.now_ns());
    (report, ram, now, waiting)
}

#[test]
fn a_run_stopped_on_a_safe_point_and_put_back_gives_the_run_that_was_never_stopped() {
    let built = build_scenario("normal", "exchange", None);
    let expected = built.expected(2).unwrap();
    // Cut points across the whole run: before the AP is started (INIT and SIPI pending or just
    // applied), inside the trampoline, in the middle of the exchange, near the end.
    let mut checked = 0;
    let (mut ap_waiting, mut ap_request_waiting) = (0, 0);
    for seed in 0..24u64 {
        let quantum = Stage6SmpProbeQuantum::Seeded(seed);
        let whole = continuous(&built, quantum.clone());
        let total: u64 = whole.0.cpus.iter().map(|cpu| cpu.retired).sum();
        assert_eq!(String::from_utf8(whole.0.com1.clone()).unwrap(), expected);
        for (index, cut) in [total / 40, total / 6, total / 2, total - 1]
            .into_iter()
            .enumerate()
        {
            let cut = cut.max(1);
            let reset = (seed + index as u64) % 2 == 0;
            let split = cut_run(&built, quantum.clone(), cut, reset);
            assert_eq!(split.0, whole.0, "{quantum:?} cut {cut} reset {reset}");
            assert_eq!(split.1, whole.1, "low RAM, {quantum:?} cut {cut}");
            assert_eq!(split.2, whole.2, "virtual time, {quantum:?} cut {cut}");
            ap_waiting += usize::from(split.3.0 == Some(true));
            ap_request_waiting += usize::from(split.3.1);
            checked += 1;
        }
    }
    assert_eq!(checked, 96);
    // The test proves what the plan says only if some stop found something waiting in the AP's
    // Local APIC (an INIT, a SIPI, a request) that the run then took after the restore.
    assert!(ap_waiting > 0, "no cut had anything waiting in LAPIC 1");
    assert!(
        ap_request_waiting > 0,
        "no cut had an unaccepted request in the IRR of LAPIC 1"
    );
}

#[test]
fn a_run_stopped_and_put_back_under_fixed_quanta_gives_the_same_cells_and_quiet_lapics() {
    let built = build_scenario("normal", "exchange", None);
    for quantum in [
        Stage6SmpProbeQuantum::Fixed(1),
        Stage6SmpProbeQuantum::Fixed(7),
        Stage6SmpProbeQuantum::Fixed(1024),
    ] {
        let whole = continuous(&built, quantum.clone());
        let total: u64 = whole.0.cpus.iter().map(|cpu| cpu.retired).sum();
        let cut = total / 2;
        // The very first instruction, too: the boot processor has run once, the AP waits.
        assert_eq!(cut_run(&built, quantum.clone(), 1, true).0, whole.0);
        let split = cut_run(&built, quantum.clone(), cut, true);
        assert_eq!(split.0, whole.0, "{quantum:?}");
        assert_eq!(split.1, whole.1, "{quantum:?}");
        assert_eq!(split.2, whole.2, "{quantum:?}");
        // The cells of the protocol are those of the manifest: the exchange finished.
        let mut probe = load_stage6_smp_probe(&built.image, 2, NONCE).unwrap();
        {
            let mut session = probe.session(quantum.clone(), false).unwrap();
            session.run(cut).unwrap();
            session.pause(1).unwrap();
            let snapshot = session.snapshot().unwrap();
            session.reset().unwrap();
            session.restore(&snapshot).unwrap();
            session.resume();
            session.run(BUDGET - cut).unwrap();
        }
        check_exchange_cells(&built, &mut probe, &format!("{quantum:?}"));
        assert_lapics_quiet(&probe, 2, &format!("{quantum:?}"));
    }
}

#[test]
fn a_snapshot_is_refused_unless_the_machine_is_stopped_and_a_restore_unless_it_stands_where_it_stood()
 {
    let built = build_scenario("normal", "exchange", None);
    let mut probe = load_stage6_smp_probe(&built.image, 2, NONCE).unwrap();
    let mut session = probe
        .session(Stage6SmpProbeQuantum::Fixed(100), false)
        .unwrap();
    // The whole exchange is about 3100 instructions: two cuts inside it.
    let first = session.run(1_000).unwrap();
    assert_eq!(first.finished, Stage6SmpProbeFinish::BudgetExhausted);
    // Not stopped: no snapshot (a flag nobody acknowledged is not a stop).
    assert!(session.snapshot().is_err());
    session.pause(1).unwrap();
    let snapshot = session.snapshot().unwrap();
    // The explicit resume ends the stop; the old proof does not give a snapshot of a running
    // machine, and a restore into a running machine is refused.
    session.resume();
    assert!(session.snapshot().is_err());
    assert!(session.restore(&snapshot).is_err());
    // The clock moved: the deadlines of the snapshot are not those of this machine.
    session.run(500).unwrap();
    session.pause(1).unwrap();
    let error = session.restore(&snapshot).unwrap_err();
    assert!(error.starts_with("ClockMoved"), "{error}");
}

// ---- the parallel mode (S6-34): one host thread per vCPU ------------------------------------------
//
// The same image and the same expectations (the builder's manifest, from the protocol), run by the
// thread runner instead of the interleaving scheduler. There is no schedule to name and no seed: the
// result of the guest, not the interleaving, is what is checked, every time. Command of the long
// form: `OD_S6_PAR_RUNS=2000 cargo test --release -p od-machine --offline --test stage6_smp_probe -- --ignored
// parallel_probe_many_runs --nocapture`.

const PARALLEL_WAIT: std::time::Duration = std::time::Duration::from_secs(120);

fn parallel_quanta() -> Vec<u64> {
    vec![1, 7, 100, 1024]
}

/// Run `built` on 2 vCPU threads with `quantum` steps per call and check the transcript, the
/// vCPUs at the end, and every Local APIC (nothing pending or in service).
fn parallel_run_ok(built: &Built, scenario_expected: &str, quantum: u64) -> Stage6SmpProbe {
    let mut probe = load_stage6_smp_probe(&built.image, 2, NONCE).unwrap();
    let report = probe
        .run_parallel(quantum, BUDGET, PARALLEL_WAIT)
        .unwrap_or_else(|error| panic!("quantum {quantum}: {error:?}"));
    assert_eq!(
        report.finished,
        Stage6SmpProbeParallelFinish::AllIdle,
        "quantum {quantum}: {report:?}"
    );
    assert_eq!(report.idle_heals, 0, "quantum {quantum}: {report:?}");
    assert_eq!(
        String::from_utf8(report.com1.clone()).unwrap(),
        scenario_expected,
        "quantum {quantum}: {report:?}"
    );
    assert_eq!(report.cpus.len(), 2);
    assert_eq!(report.cpus[1].init_applied, 1, "quantum {quantum}");
    assert_eq!(report.cpus[1].started, 1, "quantum {quantum}");
    assert_eq!(report.cpus[0].init_applied, 0);
    assert!(!report.cpus[1].legacy, "the AP reached long mode");
    assert_eq!(report.cpus[0].activity, od_isa::CpuActivity::Halted);
    assert_eq!(report.cpus[1].activity, od_isa::CpuActivity::Halted);
    probe
}

#[test]
fn the_probe_runs_in_the_parallel_mode_in_every_scenario_under_every_quantum() {
    let normal = build("normal");
    let exchange = build_scenario("normal", "exchange", None);
    let resipi = build_scenario("normal", "resipi", None);
    let nmi = build_scenario("normal", "nmi", None);
    let shootdown = build_scenario("normal", "shootdown", None);
    let rounds = std::env::var("OD_S6_PAR_RUNS")
        .ok()
        .and_then(|text| text.parse::<usize>().ok())
        .unwrap_or(6);
    for quantum in parallel_quanta() {
        for round in 0..rounds {
            let context = format!("quantum {quantum} round {round}");
            let mut probe = parallel_run_ok(&normal, &normal.expected(2).unwrap(), quantum);
            assert_eq!(dword(&mut probe, STAGE6_SMP_PROBE_LOCK_GPA), 1, "{context}");
            assert_eq!(dword(&mut probe, STAGE6_SMP_PROBE_FLAG_GPA), 1, "{context}");
            assert_eq!(
                dword(&mut probe, STAGE6_SMP_PROBE_COUNT_GPA),
                1,
                "{context}"
            );
            check_marker(&mut probe, STAGE6_SMP_PROBE_MARKER_BSP_GPA, 0);
            check_marker(&mut probe, STAGE6_SMP_PROBE_MARKER_AP_GPA, 1 << 24);

            let mut probe = parallel_run_ok(&exchange, &exchange.expected(2).unwrap(), quantum);
            check_exchange_cells(&exchange, &mut probe, &context);
            assert_lapics_quiet(&probe, 2, &context);

            let mut probe = parallel_run_ok(&resipi, &resipi.expected(2).unwrap(), quantum);
            check_exchange_cells(&resipi, &mut probe, &context);
            assert_lapics_quiet(&probe, 2, &context);

            let mut probe = parallel_run_ok(&nmi, &nmi.expected(2).unwrap(), quantum);
            assert_eq!(
                dword(&mut probe, STAGE6_SMP_PROBE_NMI_GPA),
                nmi.expected_dword(2, "nmi_cell"),
                "{context}"
            );

            let mut probe = parallel_run_ok(&shootdown, &shootdown.expected(2).unwrap(), quantum);
            check_shootdown_cells(&shootdown, &mut probe, &context);
            assert_lapics_quiet(&probe, 2, &context);
        }
    }
}

/// The long form of the test above: many runs of the exchange and the shootdown under the
/// quanta of the parallel mode (the counts and the command are in `docs/testing/SMP.md`).
#[test]
#[ignore = "long: OD_S6_PAR_RUNS=2000 cargo test --release -p od-machine --offline --test stage6_smp_probe -- --ignored parallel_probe_many_runs --nocapture"]
fn parallel_probe_many_runs() {
    let runs = std::env::var("OD_S6_PAR_RUNS")
        .ok()
        .and_then(|text| text.parse::<usize>().ok())
        .unwrap_or(1000);
    let exchange = build_scenario("normal", "exchange", None);
    let shootdown = build_scenario("normal", "shootdown", None);
    let started = std::time::Instant::now();
    for run in 0..runs {
        let quantum = parallel_quanta()[run % parallel_quanta().len()];
        let mut probe = parallel_run_ok(&exchange, &exchange.expected(2).unwrap(), quantum);
        check_exchange_cells(
            &exchange,
            &mut probe,
            &format!("run {run} quantum {quantum}"),
        );
        assert_lapics_quiet(&probe, 2, "exchange");
        let mut probe = parallel_run_ok(&shootdown, &shootdown.expected(2).unwrap(), quantum);
        check_shootdown_cells(
            &shootdown,
            &mut probe,
            &format!("run {run} quantum {quantum}"),
        );
        assert_lapics_quiet(&probe, 2, "shootdown");
    }
    eprintln!(
        "parallel probe: {runs} runs of exchange and shootdown, {:.1} s",
        started.elapsed().as_secs_f64()
    );
}

#[test]
fn one_processor_prints_ap_absent_in_the_parallel_mode_too() {
    let built = build_scenario("normal", "exchange", None);
    let mut probe = load_stage6_smp_probe(&built.image, 1, NONCE).unwrap();
    let report = probe.run_parallel(1024, BUDGET, PARALLEL_WAIT).unwrap();
    assert_eq!(report.finished, Stage6SmpProbeParallelFinish::AllIdle);
    assert_eq!(
        String::from_utf8(report.com1).unwrap(),
        built.expected(1).unwrap()
    );
}

// ---- S6-35: the state of the processors on a machine the parallel mode has stopped ------------------
//
// The probe runs on one thread per vCPU, every vCPU is stopped on a safe point after a number of
// retired instructions drawn from the seed (S6-34), the state of the vCPUs and of the Local APICs is
// taken there, the run ends and the machine stands where it was stopped. Then, in the same machine,
// the CPU complex is reset to power-on (RAM and devices untouched), the state is put back, and the
// guest goes on in the INTERLEAVED mode with a fixed seed. What is checked is the result of the
// guest (the transcript, the cells of the protocol, the Local APICs quiet) and the state itself,
// before and after the restore; the byte-for-byte equality with an uninterrupted run is not asked:
// the parallel mode is not deterministic. This is not a snapshot of the machine: no RAM, no devices,
// no file (`S9-24`/`S9-25`). Long form: `OD_S6_SNAP_RUNS=500 cargo test --release -p od-machine --offline
// --test stage6_smp_probe -- --ignored parallel_stop_and_restore_many_runs --nocapture`.

/// SplitMix64 finaliser: the stop point and the quantum of a round come from its seed.
fn mix(seed: u64) -> u64 {
    let mut z = seed.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

#[derive(Default, Debug)]
struct StopRoundStats {
    rounds: usize,
    /// The probe had ended before the stop was asked for.
    ended_before_the_stop: usize,
    /// The stop found the AP started and the exchange not finished.
    inside_the_exchange: usize,
    /// The stop found a request in the IRR or ISR of some Local APIC.
    with_a_pending_request: usize,
    /// The stop found the AP not started yet (INIT/SIPI possibly still pending).
    before_the_ap_started: usize,
}

/// One round: `seed` gives the quantum of the threads, the retired count at which the run is
/// stopped and the seed of the interleaved run that goes on.
fn parallel_stop_round(
    built: &Built,
    expected: &str,
    check_cells: &dyn Fn(&Built, &mut Stage6SmpProbe, &str),
    seed: u64,
    stats: &mut StopRoundStats,
) {
    let context = format!("seed {seed}");
    let quantum = [1u64, 7, 100][(mix(seed) % 3) as usize];
    let stop_after = 1 + mix(seed ^ 0xA5A5) % 1800;
    let mut probe = load_stage6_smp_probe(&built.image, 2, NONCE).unwrap();
    let stop = probe
        .run_parallel_to_a_stop(
            quantum,
            stop_after,
            PARALLEL_WAIT,
            Stage6SmpProbeQuantum::Seeded(seed),
        )
        .unwrap_or_else(|error| panic!("{context} quantum {quantum}: {error:?}"));
    assert!(stop.threads_joined, "{context}: no thread is left");
    stats.rounds += 1;
    let Some(snapshot) = stop.snapshot else {
        // The probe was over before the stop: the guest did its work on threads, as in S6-34.
        stats.ended_before_the_stop += 1;
        assert_eq!(
            String::from_utf8(stop.com1).unwrap(),
            expected,
            "{context} (ended before the stop)"
        );
        return;
    };
    // What the guest had done by the stop, read from the machine as it stands.
    let flag = dword(&mut probe, STAGE6_SMP_PROBE_FLAG_GPA);
    let bsp_rx = dword(&mut probe, STAGE6_SMP_PROBE_BSP_RX_GPA);
    stats.before_the_ap_started += usize::from(flag == 0);
    stats.inside_the_exchange += usize::from(flag == 1 && bsp_rx < ROUNDS);
    let pending: Vec<_> = (0..2).map(|cpu| snapshot.pending_vectors(cpu)).collect();
    stats.with_a_pending_request += usize::from(pending.iter().any(|vectors| !vectors.is_empty()));

    let mut session = probe
        .session(Stage6SmpProbeQuantum::Seeded(seed), false)
        .unwrap();
    // The machine stands as the controller saw it at the stop: the interleaved stop of the same
    // machine finds the same vCPUs, Local APICs and clock.
    session.pause(8).unwrap();
    let at_rest = session.snapshot().unwrap();
    assert!(
        snapshot.same_processor_state(&at_rest),
        "{context}: the machine moved after the stop"
    );
    assert_eq!(snapshot.now_ns(), at_rest.now_ns(), "{context}");
    // The pending requests (IRR/ISR): the same before the reset as in the snapshot ...
    for (cpu, vectors) in pending.iter().enumerate() {
        assert_eq!(
            &session.pending_vectors(cpu),
            vectors,
            "{context}: IRR/ISR of LAPIC {cpu} before the reset"
        );
    }
    // ... gone after the reset of the CPU complex (RAM and devices stay) ...
    let activities: Vec<_> = (0..2).map(|cpu| session.activity(cpu)).collect();
    session.reset().unwrap();
    assert_eq!(
        session.activity(1),
        Some(od_isa::CpuActivity::WaitForSipi),
        "{context}"
    );
    for cpu in 0..2 {
        assert_eq!(session.lapic_pending(cpu), Some(false), "{context}");
        assert_eq!(session.pending_vectors(cpu), vec![], "{context}");
    }
    // ... and all of them back after the restore, none twice and none missing.
    session.restore(&snapshot).unwrap();
    assert_eq!(
        (0..2).map(|cpu| session.activity(cpu)).collect::<Vec<_>>(),
        activities,
        "{context}"
    );
    for (cpu, vectors) in pending.iter().enumerate() {
        assert_eq!(
            &session.pending_vectors(cpu),
            vectors,
            "{context}: IRR/ISR of LAPIC {cpu} after the restore"
        );
    }
    assert!(
        snapshot.same_processor_state(&session.snapshot().unwrap()),
        "{context}: the restored state is not the state at the stop"
    );
    // The guest goes on in the interleaved mode, and ends right.
    session.resume();
    let report = session.run(BUDGET).unwrap();
    assert_eq!(report.finished, Stage6SmpProbeFinish::AllIdle, "{context}");
    assert_eq!(
        String::from_utf8(report.com1).unwrap(),
        expected,
        "{context} quantum {quantum} stop after {stop_after}"
    );
    assert!(report.cpus.iter().all(|cpu| !cpu.legacy), "{context}");
    for cpu in 0..2 {
        assert_eq!(
            session.activity(cpu),
            Some(od_isa::CpuActivity::Halted),
            "{context}"
        );
    }
    drop(session);
    check_cells(built, &mut probe, &context);
    assert_lapics_quiet(&probe, 2, &context);
}

fn parallel_stop_rounds(rounds: usize) -> StopRoundStats {
    let exchange = build_scenario("normal", "exchange", None);
    let shootdown = build_scenario("normal", "shootdown", None);
    let exchange_expected = exchange.expected(2).unwrap();
    let shootdown_expected = shootdown.expected(2).unwrap();
    let mut stats = StopRoundStats::default();
    for seed in 0..rounds as u64 {
        if seed % 2 == 0 {
            parallel_stop_round(
                &exchange,
                &exchange_expected,
                &|built, probe, context| check_exchange_cells(built, probe, context),
                seed,
                &mut stats,
            );
        } else {
            parallel_stop_round(
                &shootdown,
                &shootdown_expected,
                &|built, probe, context| check_shootdown_cells(built, probe, context),
                seed,
                &mut stats,
            );
        }
    }
    stats
}

#[test]
fn a_parallel_run_stopped_at_a_random_moment_is_reset_restored_and_finished_in_the_interleaved_mode()
 {
    let stats = parallel_stop_rounds(8);
    eprintln!("parallel stop and restore: {stats:?}");
    assert!(stats.rounds - stats.ended_before_the_stop >= 4, "{stats:?}");
}

/// The long form (50 rounds by default, `OD_S6_SNAP_RUNS` changes it; exchange and shootdown by
/// turns, seeds `0..rounds`): the stop must land inside the exchange in a good part of the rounds.
#[test]
#[ignore = "long: OD_S6_SNAP_RUNS=500 cargo test --release -p od-machine --offline --test stage6_smp_probe -- --ignored parallel_stop_and_restore_many_runs --nocapture"]
fn parallel_stop_and_restore_many_runs() {
    let rounds = std::env::var("OD_S6_SNAP_RUNS")
        .ok()
        .and_then(|text| text.parse::<usize>().ok())
        .unwrap_or(50);
    let started = std::time::Instant::now();
    let stats = parallel_stop_rounds(rounds);
    eprintln!(
        "parallel stop and restore: {rounds} rounds, {:.1} s, {stats:?}",
        started.elapsed().as_secs_f64()
    );
    assert!(stats.inside_the_exchange >= rounds / 4, "{stats:?}");
    assert!(stats.with_a_pending_request >= 1, "{stats:?}");
}
