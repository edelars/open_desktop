use std::fmt::Write as _;
use std::path::Path;

use od_isa::cpuid::CpuidProfile;
use od_machine::{
    FirmwareBoardConfig, LinuxCpl0SetMembers, LinuxDirectInput, load_linux_direct_board,
    run_linux_direct_board_set_user_mode,
};

const BUDGET: u64 = 300_000_000;
const SEED: u64 = 0x5eed;

fn hex(digest: [u8; 32]) -> String {
    let mut result = String::with_capacity(64);
    for byte in digest {
        write!(&mut result, "{byte:02x}").unwrap();
    }
    result
}

#[test]
#[ignore = "uses the locally staged S6-10 Linux kernel and initramfs"]
fn seeded_interleaved_repeats_a_real_linux_prefix() {
    let stage4 = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../work/stage4");
    let kernel = std::fs::read(stage4.join("linux-7.0.0-38.38/vmlinuz-7.0.0-38-generic"))
        .expect("staged S6-10 Linux kernel");
    let initrd =
        std::fs::read(stage4.join("initramfs/initramfs.cpio")).expect("staged S6-10 initrd");
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
    let input = LinuxDirectInput {
        bzimage: &kernel,
        initrd: &initrd,
        cmdline: b"console=ttyS0,115200 earlycon=uart8250,io,0x3f8 8250.nr_uarts=1 od.nonce=s6-21-seeded nokaslr",
        acpi_blob: include_bytes!(
            "../../../tests/fixtures/guest/stage4/acpi_blob/stage4_acpi_blob.bin"
        ),
        expected_acpi_manifest_hash: hash,
        ram_bytes: 128 << 20,
    };
    let members = LinuxCpl0SetMembers {
        cpuid_profile: CpuidProfile::X8664V1Stage4A,
        ..LinuxCpl0SetMembers::linux_default()
    };
    let run = |seed| {
        let mut boot = load_linux_direct_board(&input).unwrap();
        match seed {
            Some(seed) => boot.board.enable_seeded_linux_interleaved(seed).unwrap(),
            None => boot.board.enable_linux_interleaved().unwrap(),
        }
        let report = run_linux_direct_board_set_user_mode(&mut boot, &input, BUDGET, members)
            .expect("run Linux with the selected policy");
        assert_eq!(
            hex(report.inputs.bzimage_sha256),
            "9f55ef7253055a9cf68f8a04323c93dead2fca617386d842fbde9ca7b178b27e"
        );
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
