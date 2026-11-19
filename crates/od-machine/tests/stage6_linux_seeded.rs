use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use od_isa::cpuid::CpuidProfile;
use od_machine::{
    FirmwareBoardConfig, LinuxCpl0SetMembers, LinuxDirectInput, load_linux_direct_board,
    run_linux_direct_board_set_user_mode,
};

const SEED: u64 = 0x5eed;
const KERNEL_SHA256: &str = "9f55ef7253055a9cf68f8a04323c93dead2fca617386d842fbde9ca7b178b27e";
const MEMBERS: LinuxCpl0SetMembers = LinuxCpl0SetMembers {
    cpuid_profile: CpuidProfile::X8664V1Stage4A,
    ..LinuxCpl0SetMembers::linux_default()
};

fn hex(digest: [u8; 32]) -> String {
    let mut result = String::with_capacity(64);
    for byte in digest {
        write!(&mut result, "{byte:02x}").unwrap();
    }
    result
}

fn work_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../work")
}

fn kernel() -> Vec<u8> {
    std::fs::read(work_dir().join("stage4/linux-7.0.0-38.38/vmlinuz-7.0.0-38-generic"))
        .expect("staged S6-10 Linux kernel")
}

fn input<'a>(kernel: &'a [u8], initrd: &'a [u8], cmdline: &'static [u8]) -> LinuxDirectInput<'a> {
    let hash = FirmwareBoardConfig {
        pci_ecam: true,
        acpi_pm: true,
        acpi_tables: true,
        ..FirmwareBoardConfig::default()
    }
    .acpi_manifest_hash()
    .unwrap();
    assert_eq!(
        hex(hash),
        "2bd690ae0817705b1b128675f9628dd7212fee74ab97c88da3599a1ae8e398f2"
    );
    LinuxDirectInput {
        bzimage: kernel,
        initrd,
        cmdline,
        acpi_blob: include_bytes!(
            "../../../tests/fixtures/guest/stage4/acpi_blob/stage4_acpi_blob.bin"
        ),
        expected_acpi_manifest_hash: hash,
        ram_bytes: 128 << 20,
    }
}

#[test]
#[ignore = "uses the locally staged S6-10 Linux kernel and initramfs"]
fn seeded_interleaved_repeats_a_real_linux_prefix() {
    const BUDGET: u64 = 300_000_000;
    let kernel = kernel();
    let initrd = std::fs::read(work_dir().join("stage4/initramfs/initramfs.cpio"))
        .expect("staged S6-10 initrd");
    let input = input(
        &kernel,
        &initrd,
        b"console=ttyS0,115200 earlycon=uart8250,io,0x3f8 8250.nr_uarts=1 od.nonce=s6-21-seeded nokaslr",
    );
    let run = |seed| {
        let mut boot = load_linux_direct_board(&input).unwrap();
        match seed {
            Some(seed) => boot.board.enable_seeded_linux_interleaved(seed).unwrap(),
            None => boot.board.enable_linux_interleaved().unwrap(),
        }
        let report = run_linux_direct_board_set_user_mode(&mut boot, &input, BUDGET, MEMBERS)
            .expect("run Linux with the selected policy");
        assert_eq!(hex(report.inputs.bzimage_sha256), KERNEL_SHA256);
        assert_eq!(
            hex(report.inputs.initrd_sha256.unwrap()),
            "ee9183a4eded3c16a8c6a7a81c6d5bb79be8bb023f793f7cd61694ea2bd3e588"
        );
        assert_eq!(report.exit_kind, "budget_exhausted");
        assert_eq!(report.retired, BUDGET);
        println!(
            "seed={seed:?} retired={} quanta={} deadline_cuts={} virtual_ns={} rip={:#x} com1_bytes={}",
            report.retired,
            report.quanta,
            report.quantum_cut_by_deadline,
            report.virtual_ns,
            report.rip,
            report.com1.len()
        );
        (report, boot.board.state().clone())
    };

    std::thread::scope(|scope| {
        let fixed = scope.spawn(|| run(None));
        let first = scope.spawn(|| run(Some(SEED)));
        let second = scope.spawn(|| run(Some(SEED)));
        let (fixed, _) = fixed.join().unwrap();
        let (first, state) = first.join().unwrap();
        let (second, second_state) = second.join().unwrap();
        assert!(first.quanta > fixed.quanta);
        assert_eq!(first, second);
        assert_eq!(state, second_state);
    });
}

#[test]
#[ignore = "uses the staged Linux kernel and S6-21 observation initramfs"]
fn seeded_interleaved_reaches_guest_init() {
    const BUDGET: u64 = 2_500_000_000;
    let kernel = kernel();
    let initrd = std::fs::read(work_dir().join("stage6/smp-init/initramfs.cpio"))
        .expect("staged S6-21 observation initrd");
    let input = input(
        &kernel,
        &initrd,
        b"console=ttyS0,115200 earlycon=uart8250,io,0x3f8 8250.nr_uarts=1 od.nonce=SMPUpObservation20261119 nokaslr",
    );
    let mut boot = load_linux_direct_board(&input).expect("load the S6-21 guest");
    boot.board.enable_seeded_linux_interleaved(SEED).unwrap();
    // Match the stage4a user-mode members selected by linux-direct beyond the boot prefix.
    let members = LinuxCpl0SetMembers {
        msr_tdx_keyid_partitioning_read: true,
        fp_state: true,
        int3: true,
        ..MEMBERS
    };
    let report = run_linux_direct_board_set_user_mode(&mut boot, &input, BUDGET, members)
        .expect("run the S6-21 guest with seed");
    let transcript = String::from_utf8_lossy(&report.com1);
    println!("{transcript}");
    println!(
        "seed={SEED} exit={} retired={} cpl3_retired={} quanta={} virtual_ns={} rip={:#x}",
        report.exit_kind,
        report.retired,
        report.cpl3_retired,
        report.quanta,
        report.virtual_ns,
        report.rip,
    );
    assert_eq!(hex(report.inputs.bzimage_sha256), KERNEL_SHA256);
    assert_eq!(
        hex(report.inputs.initrd_sha256.unwrap()),
        "5481c1399df1ebce44316a32a2c29857f596ac7e7b5e2c8af3e93cc4e21a276d"
    );
    assert_eq!(
        report.exit_kind,
        "budget_exhausted",
        "{}; RIP bytes {:02x?}; RCX={:#x}",
        report.exit_detail,
        report.rip_bytes,
        report.gpr[od_isa::Register::Rcx as usize],
    );
    assert!(report.cpl3_retired > 0);
    assert!(transcript.contains("Run /init as init process"));
    assert!(transcript.contains("OD_SMP_V1 SMPUpObservation20261119"));
    assert!(transcript.contains("OD_SMP_END"));
}
