//! S6-16e: the `Stage6SmpProbeV1` guest starts an application processor with INIT and SIPI.
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
    STAGE6_SMP_PROBE_COUNT_GPA, STAGE6_SMP_PROBE_FLAG_GPA, STAGE6_SMP_PROBE_IMAGE_BYTES,
    STAGE6_SMP_PROBE_INFO_GPA, STAGE6_SMP_PROBE_LOCK_GPA, STAGE6_SMP_PROBE_MARKER_AP_GPA,
    STAGE6_SMP_PROBE_MARKER_BSP_GPA, STAGE6_SMP_PROBE_MARKER_MAGIC, STAGE6_SMP_PROBE_NONCE_MAX,
    Stage6SmpProbe, Stage6SmpProbeFinish, Stage6SmpProbeLoadError, Stage6SmpProbeQuantum,
    Stage6SmpProbeRunError, load_stage6_smp_probe,
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
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let dir = std::env::temp_dir().join(format!(
        "od-stage6-smp-probe-{}-{variant}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    let output = Command::new("python3")
        .arg(root.join("tools/build/build_stage6_smp_probe.py"))
        .args(["--variant", variant, "--output-dir"])
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
        let (mut probe, uart) = n2_run(&built, quantum);
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
