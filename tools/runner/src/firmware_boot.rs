//! `boot-firmware`: start a firmware image on firmware board v1 (S7-03).
//!
//! The runner only reads the image, calls `od_machine::load_firmware_image`,
//! drives `run_boot` and reports. Loading, quanta, COM1 draining and virtual
//! time (`retired * 1 ns` per quantum) live in `od-machine`
//! (`docs/specs/FIRMWARE_BOOT.md` sections 8 and 11).

use od_machine::{
    FIRMWARE_EXECUTION_CYCLE_NS, FIRMWARE_IMAGE_BYTES, FirmwareBoardConfig, FirmwareFvApp,
    InMemoryBackend, LoadedFirmwareBoard, MachineStopReason, NVRAM_SNAPSHOT_BYTES, NvramConfig,
    NvramDevice, PagedProbeExit, PlatformResetKind, SecureBootKeys, TpmConfig,
    firmware_handoff_block, load_firmware_image,
};
use od_testkit::sha256_hex;
use serde_json::{Value, json};
use std::fs::{self, File, Metadata, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};
use std::time::Instant;

const USAGE: &str = "boot-firmware --image <bin> --com1-out <file> --budget <steps> \
                    [--fv-app hello|ebs|vars|rt|acpi|gop|blk|fs|tpm|reboot] [--tpm] [--nvram-reboot-out <file>] \
                    [--reboot-cycles <n> [--reboot-kind cold|warm]] \
                    [--nvram-in <file>] [--nvram-out <file>] \
                    [--acpi-out <file>] [--framebuffer-out <file>] [--nvme-image <raw>] \
                    [--cpu-loop firmware|unified] [--sample-rip <hex> ...]";
// Match the milestones in the given order. Repeated RIPs consume separate
// visits, and later milestones remain pending until the next expected RIP.
const MAX_RIP_SAMPLES: usize = 64;
const DUMP_USAGE: &str =
    "dump-handoff-block --out <file> [--fv-app hello|ebs|vars|rt|acpi|gop|blk|fs|tpm|reboot]";
const PREPARE_NVRAM_USAGE: &str =
    "prepare-secure-boot-nvram setup|defaults --out <new file> [--nvram-in <protected snapshot>]";

struct Options {
    image: String,
    com1_out: String,
    budget: u64,
    fv_app: Option<FirmwareFvApp>,
    nvram_reboot_out: Option<String>,
    nvram_in: Option<String>,
    nvram_out: Option<String>,
    acpi_out: Option<String>,
    framebuffer_out: Option<String>,
    nvme_image: Option<String>,
    tpm: bool,
    /// S7-26: boot, reset in process and boot again this many times (`--fv-app reboot`).
    reboot_cycles: Option<u64>,
    reboot_kind: PlatformResetKind,
    sample_rips: Vec<u64>,
    /// S7-31c: `--cpu-loop unified` runs the ROM and the guest it starts in the CPL0 profile set
    /// loop on a board with the Linux profile (ADR-0013 variant (b)).
    unified: bool,
}

fn tpm_config(snapshot: Option<Vec<u8>>) -> io::Result<TpmConfig> {
    let mut config = TpmConfig {
        entropy: vec![0; 48],
        snapshot,
    };
    File::open("/dev/urandom")?.read_exact(&mut config.entropy)?;
    Ok(config)
}

fn board_config(
    fv_app: Option<FirmwareFvApp>,
    reboot: bool,
    tpm: Option<TpmConfig>,
) -> FirmwareBoardConfig {
    let rt = fv_app == Some(FirmwareFvApp::Rt);
    let acpi = fv_app == Some(FirmwareFvApp::Acpi);
    let acpi_present = rt || acpi || tpm.is_some();
    FirmwareBoardConfig {
        fv_app,
        acpi_pm: acpi_present,
        acpi_tables: acpi_present,
        tpm,
        rtc_base_unix_seconds: (rt || acpi).then_some(1_704_067_200),
        pci_ecam: acpi,
        // Stable fixture identity for the diagnostic firmware-volume application.
        system_uuid: if acpi {
            *b"\x79\xa3\x31\x49\x27\x43\x80\x43\xa2\x9b\x88\x19\x20\x31\x6b\x40"
        } else {
            [0; 16]
        },
        serial: if acpi {
            b"OD-S7-13".to_vec()
        } else {
            Vec::new()
        },
        nvram: (rt || reboot || fv_app == Some(FirmwareFvApp::Reboot))
            .then_some(NvramConfig::Empty),
        nvme: matches!(fv_app, Some(FirmwareFvApp::Blk | FirmwareFvApp::Fs)).then(Default::default),
        ..FirmwareBoardConfig::default()
    }
}

fn parse_app(value: &str) -> Result<FirmwareFvApp, String> {
    match value {
        "hello" => Ok(FirmwareFvApp::Hello),
        "ebs" => Ok(FirmwareFvApp::Ebs),
        "vars" => Ok(FirmwareFvApp::Vars),
        "rt" => Ok(FirmwareFvApp::Rt),
        "acpi" => Ok(FirmwareFvApp::Acpi),
        "gop" => Ok(FirmwareFvApp::Gop),
        "blk" => Ok(FirmwareFvApp::Blk),
        "fs" => Ok(FirmwareFvApp::Fs),
        "tpm" => Ok(FirmwareFvApp::Tpm),
        "reboot" => Ok(FirmwareFvApp::Reboot),
        _ => Err(format!(
            "invalid --fv-app '{value}'; expected hello, ebs, vars, rt, acpi, gop, blk, fs, tpm or reboot"
        )),
    }
}

pub fn dump_handoff(args: &[String]) -> ExitCode {
    let run = || -> Result<(), String> {
        let mut out = None;
        let mut app = None;
        for option in args.chunks(2) {
            match option {
                [name, value] if name == "--out" && out.is_none() => out = Some(value),
                [name, value] if name == "--fv-app" && app.is_none() => {
                    app = Some(parse_app(value)?);
                }
                _ => return Err(format!("usage: {DUMP_USAGE}")),
            }
        }
        let path = out.ok_or_else(|| format!("usage: {DUMP_USAGE}"))?;
        // Only the descriptor is serialized. No TPM is created by this command.
        let tpm =
            matches!(app, Some(FirmwareFvApp::Tpm | FirmwareFvApp::Reboot)).then(|| TpmConfig {
                entropy: Vec::new(),
                snapshot: None,
            });
        let block = firmware_handoff_block(&board_config(app, false, tpm))
            .map_err(|error| format!("{error:?}"))?;
        fs::write(path, block).map_err(|error| format!("{path}: {error}"))
    };
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("dump-handoff-block: {error}");
            ExitCode::from(3)
        }
    }
}

pub fn run(args: &[String]) -> ExitCode {
    let options = match parse(args) {
        Ok(options) => options,
        Err(detail) => return emit(failure("invalid_input", "arguments", detail), 3),
    };
    if let Some(reboot_out) = &options.nvram_reboot_out
        && let Err(error) = validate_reboot_paths(&options.image, &options.com1_out, reboot_out)
    {
        return emit(failure("invalid_input", "paths", error), 3);
    }
    if let Some(path) = &options.acpi_out
        && let Err(error) = validate_output_paths(&options.image, &options.com1_out, path)
    {
        return emit(failure("invalid_input", "paths", error), 3);
    }
    if let Some(path) = &options.framebuffer_out
        && let Err(error) = validate_framebuffer_paths(&options.image, &options.com1_out, path)
    {
        return emit(failure("invalid_input", "paths", error), 3);
    }
    if let Some(path) = &options.nvme_image
        && let Err(error) = validate_nvme_paths(path, &options)
    {
        return emit(failure("invalid_input", "paths", error), 3);
    }
    if let Err(error) = validate_nvram_paths(&options) {
        return emit(failure("invalid_input", "paths", error), 3);
    }
    let mut image = Vec::with_capacity(FIRMWARE_IMAGE_BYTES + 1);
    let read = File::open(&options.image).and_then(|file| {
        file.take(FIRMWARE_IMAGE_BYTES as u64 + 1)
            .read_to_end(&mut image)
    });
    if let Err(error) = read {
        return emit(failure("invalid_input", "image_file", error.to_string()), 3);
    }
    let tpm = if options.tpm
        || matches!(
            options.fv_app,
            Some(FirmwareFvApp::Tpm | FirmwareFvApp::Reboot)
        ) {
        match tpm_config(None) {
            Ok(config) => Some(config),
            Err(error) => {
                return emit(failure("loader_error", "tpm_entropy", error.to_string()), 4);
            }
        }
    } else {
        None
    };
    let mut config = board_config(options.fv_app, options.nvram_reboot_out.is_some(), tpm);
    if options.unified {
        // The Linux profile needs the PM block, the ACPI tables and the ECAM window.
        config.linux_profile = true;
        config.acpi_pm = true;
        config.acpi_tables = true;
        config.pci_ecam = true;
    }
    if options.nvme_image.is_some() {
        config.nvme = Some(Default::default());
    }
    if let Some(path) = &options.nvram_in {
        let bytes = match read_nvram_snapshot(path) {
            Ok(bytes) => bytes,
            Err(error) => return emit(failure("invalid_input", "nvram_in", error.to_string()), 3),
        };
        config.nvram = Some(NvramConfig::Snapshot(bytes));
    } else if options.nvram_out.is_some()
        || (options.nvme_image.is_some() && options.fv_app.is_none())
    {
        config.nvram.get_or_insert(NvramConfig::Empty);
    }
    if let Some(cycles) = options.reboot_cycles {
        return run_reboot_cycles(&options, &image, &config, cycles);
    }
    let nvme_image = options.nvme_image.as_deref();
    let mut first = match boot(
        &image,
        &config,
        options.budget,
        &options.com1_out,
        nvme_image,
        &options.sample_rips,
        options.unified,
    ) {
        Ok(run) => run,
        Err((value, code)) => return emit(value, code),
    };
    if let Some(path) = &options.acpi_out {
        if !first.finished_app(b"OD-FW image acpi: EFI_SUCCESS\r\n") {
            first.report["status"] = json!("error");
            first.report["error"] = json!({
                "kind": "acpi_out",
                "detail": "firmware did not complete the acpi application",
            });
            return emit(first.report, first.code.max(5));
        }
        let write = validate_output_paths(&options.image, &options.com1_out, path)
            .and_then(|()| {
                first
                    .board
                    .read_acpi_reservations()
                    .map_err(|error| format!("{error:?}"))
            })
            .and_then(|bytes| fs::write(path, bytes).map_err(|error| error.to_string()));
        if let Err(error) = write {
            return emit(failure("run_error", "acpi_out", error), 5);
        }
    }
    if let Some(path) = &options.framebuffer_out {
        if !first.finished_app(b"OD-FW image gop: EFI_SUCCESS\r\n") {
            first.report["status"] = json!("error");
            first.report["error"] = json!({
                "kind": "framebuffer_out",
                "detail": "firmware did not complete the gop application",
            });
            return emit(first.report, first.code.max(5));
        }
        if let Err(error) = validate_framebuffer_paths(&options.image, &options.com1_out, path) {
            return emit(failure("invalid_input", "paths", error), 3);
        }
        let display = first.board.display();
        let mode = display.current_mode();
        let mut pixels = vec![0; mode.frame_bytes() as usize];
        let frame = match display.read_frame_into(first.board.bus_mut(), &mut pixels) {
            Ok(frame) => frame,
            Err(error) => {
                return emit(
                    failure("run_error", "framebuffer_out", format!("{error:?}")),
                    5,
                );
            }
        };
        if let Err(error) = fs::write(path, &pixels) {
            return emit(
                failure("run_error", "framebuffer_out", error.to_string()),
                5,
            );
        }
        first.report["framebuffer"] = json!({
            "width": frame.mode.width, "height": frame.mode.height, "stride": frame.mode.stride,
            "format": frame.mode.format, "generation": frame.generation,
            "mode_change_count": frame.mode_change_count, "sha256": sha256_hex(&pixels),
        });
    }
    if let Some(path) = &options.nvram_out {
        if let Err(error) = validate_nvram_paths(&options) {
            return emit(failure("invalid_input", "paths", error), 3);
        }
        let Some(snapshot) = first.board.nvram_snapshot() else {
            return emit(
                failure("run_error", "nvram_out", "NVRAM was not bound".to_owned()),
                5,
            );
        };
        if let Err(error) = fs::write(path, &snapshot) {
            return emit(failure("run_error", "nvram_out", error.to_string()), 5);
        }
        first.report["nvram_snapshot_sha256"] = json!(sha256_hex(&snapshot));
        // The new snapshot path now has an inode. A previously absent reboot
        // output can resolve to it on a case-insensitive filesystem.
        if options.nvram_reboot_out.is_some()
            && let Err(error) = validate_nvram_paths(&options)
        {
            return emit(failure("invalid_input", "paths", error), 3);
        }
    }
    let Some(reboot_out) = options.nvram_reboot_out else {
        return emit(first.report, first.code);
    };
    if !first.returned_to_firmware() {
        let mut report = first.report;
        if first.code == 0 {
            report["status"] = json!("error");
            report["error"] =
                json!({"kind": "nvram_reboot", "detail": "first boot did not finish vars"});
        }
        return emit(report, first.code.max(5));
    }
    let Some(snapshot) = first.board.nvram_snapshot() else {
        return emit(
            failure("run_error", "nvram", "NVRAM was not bound".into()),
            5,
        );
    };
    let snapshot_sha256 = sha256_hex(&snapshot);
    let BootRun {
        report: first_report,
        board,
        ..
    } = first;
    // The first output now exists. Check aliases again on case-insensitive filesystems.
    if let Err(error) = validate_reboot_paths(&options.image, &options.com1_out, &reboot_out) {
        let mut value = failure("invalid_input", "paths", error);
        value["initial_boot"] = first_report;
        return emit(value, 3);
    }
    let tpm = match board
        .tpm_state_snapshot()
        .map(|snapshot| tpm_config(Some(snapshot)))
        .transpose()
    {
        Ok(tpm) => tpm,
        Err(error) => {
            let mut value = failure("loader_error", "tpm_entropy", error.to_string());
            value["initial_boot"] = first_report;
            return emit(value, 4);
        }
    };
    drop(board);
    let restored = FirmwareBoardConfig {
        nvram: Some(NvramConfig::Snapshot(snapshot)),
        tpm,
        ..config
    };
    let second = match boot(
        &image,
        &restored,
        options.budget,
        &reboot_out,
        nvme_image,
        &[],
        false,
    ) {
        Ok(run) => run,
        Err((mut value, code)) => {
            value["initial_boot"] = first_report;
            return emit(value, code);
        }
    };
    let completed = second.returned_to_firmware();
    emit(
        json!({
            "schema_version": 1,
            "command": "boot-firmware",
            "status": if completed { "completed" } else { "error" },
            "exit": second.report["exit"],
            "nvram_snapshot_sha256": snapshot_sha256,
            "boots": [first_report, second.report],
        }),
        if completed { 0 } else { second.code.max(5) },
    )
}

/// Builds a cold-start Secure Boot store through the machine's provisioning API.
pub fn prepare_secure_boot_nvram(args: &[String]) -> ExitCode {
    let (mut report, code) = match prepare_nvram(args) {
        Ok(bytes) => (
            json!({"schema_version": 1, "status": "prepared", "snapshot_sha256": sha256_hex(&bytes)}),
            0,
        ),
        Err(error) => (failure("invalid_input", "secure_boot_nvram", error), 3),
    };
    report["command"] = json!("prepare-secure-boot-nvram");
    emit(report, code)
}

fn prepare_nvram(args: &[String]) -> Result<Vec<u8>, String> {
    let mut out = None;
    let mut input = None;
    let usage = || format!("usage: {PREPARE_NVRAM_USAGE}");
    let (mode, options) = args.split_first().ok_or_else(usage)?;
    let mut pairs = options.chunks_exact(2);
    if !pairs.remainder().is_empty() {
        return Err(usage());
    }
    for pair in &mut pairs {
        match pair[0].as_str() {
            "--out" if out.is_none() => out = Some(Path::new(&pair[1])),
            "--nvram-in" if input.is_none() => input = Some(&pair[1]),
            _ => return Err(usage()),
        }
    }
    let out = out.ok_or_else(usage)?;
    let prior = input
        .map(|path| read_nvram_snapshot(path).map_err(|error| format!("{path}: {error}")))
        .transpose()?;
    let certificate;
    let keys = match mode.as_str() {
        "setup" => SecureBootKeys::SetupMode,
        "defaults" => {
            certificate = generate_platform_certificate()?;
            SecureBootKeys::Defaults {
                public_platform_key: &certificate,
            }
        }
        _ => return Err(usage()),
    };
    let device = match prior {
        Some(bytes) => NvramDevice::reset_secure_boot_keys(&bytes, keys),
        None => NvramDevice::with_secure_boot_keys(keys),
    }
    .map_err(|error| error.to_string())?;
    let snapshot = device.nvram_snapshot();
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(out)
        .map_err(|error| format!("{}: {error}", out.display()))?;
    if let Err(error) = file.write_all(&snapshot) {
        drop(file);
        let _ = fs::remove_file(out);
        return Err(format!("{}: {error}", out.display()));
    }
    Ok(snapshot)
}

fn generate_platform_certificate() -> Result<Vec<u8>, String> {
    let output = Command::new("openssl")
        .args([
            "req",
            "-x509",
            "-newkey",
            "rsa:2048",
            "-nodes",
            "-batch",
            "-sha256",
            "-days",
            "365",
            "-subj",
            "/CN=Open Desktop VM Platform Key",
            "-keyout",
            "/dev/null",
            "-outform",
            "DER",
        ])
        .stdin(Stdio::null())
        .output()
        .map_err(|error| format!("openssl: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "openssl: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(output.stdout)
}

fn read_nvram_snapshot(path: &str) -> io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    File::open(path)?
        .take(NVRAM_SNAPSHOT_BYTES as u64 + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() != NVRAM_SNAPSHOT_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "NVRAM snapshot has an invalid size",
        ));
    }
    Ok(bytes)
}

struct RebootPath {
    absolute: PathBuf,
    metadata: Option<Metadata>,
}

impl RebootPath {
    fn resolve(path: &Path) -> io::Result<Self> {
        let mut path = path.to_path_buf();
        for _ in 0..40 {
            match fs::canonicalize(&path) {
                Ok(absolute) => {
                    let metadata = fs::metadata(&absolute)?;
                    return Ok(Self {
                        absolute,
                        metadata: Some(metadata),
                    });
                }
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => return Err(error),
            }
            let parent = path
                .parent()
                .filter(|parent| !parent.as_os_str().is_empty())
                .unwrap_or(Path::new("."));
            match fs::symlink_metadata(&path) {
                Ok(metadata) if metadata.is_symlink() => {
                    path = parent.join(fs::read_link(&path)?);
                }
                Err(error) if error.kind() == io::ErrorKind::NotFound => {
                    let name = path.file_name().ok_or(error)?;
                    return Ok(Self {
                        absolute: fs::canonicalize(parent)?.join(name),
                        metadata: None,
                    });
                }
                Err(error) => return Err(error),
                Ok(_) => return Err(io::Error::other("path changed during validation")),
            }
        }
        Err(io::Error::other("too many symbolic links"))
    }

    fn aliases(&self, other: &Self) -> bool {
        if self.absolute == other.absolute {
            return true;
        }
        #[cfg(unix)]
        if let (Some(left), Some(right)) = (&self.metadata, &other.metadata) {
            use std::os::unix::fs::MetadataExt;
            return left.dev() == right.dev() && left.ino() == right.ino();
        }
        false
    }
}

fn validate_reboot_paths(image: &str, first: &str, second: &str) -> Result<(), String> {
    validate_distinct_paths(
        image,
        first,
        second,
        "the image and both COM1 output paths must be distinct",
    )
}

fn validate_output_paths(image: &str, com1: &str, acpi: &str) -> Result<(), String> {
    validate_distinct_paths(
        image,
        com1,
        acpi,
        "the image, COM1 and ACPI output paths must be distinct",
    )
}

fn validate_framebuffer_paths(image: &str, com1: &str, framebuffer: &str) -> Result<(), String> {
    validate_distinct_paths(
        image,
        com1,
        framebuffer,
        "the image, COM1 and framebuffer output paths must be distinct",
    )
}

fn validate_nvme_paths(image: &str, options: &Options) -> Result<(), String> {
    let input =
        RebootPath::resolve(Path::new(image)).map_err(|error| format!("{image}: {error}"))?;
    for output in [
        Some(options.com1_out.as_str()),
        options.nvram_reboot_out.as_deref(),
        options.acpi_out.as_deref(),
        options.framebuffer_out.as_deref(),
    ]
    .into_iter()
    .flatten()
    {
        let target =
            RebootPath::resolve(Path::new(output)).map_err(|error| format!("{output}: {error}"))?;
        if input.aliases(&target) {
            return Err("the NVMe input and all output paths must be distinct".to_owned());
        }
    }
    Ok(())
}

fn validate_nvram_paths(options: &Options) -> Result<(), String> {
    let outputs = [
        Some(options.com1_out.as_str()),
        options.nvram_reboot_out.as_deref(),
        options.acpi_out.as_deref(),
        options.framebuffer_out.as_deref(),
    ];
    let resolve = |path: &str| {
        RebootPath::resolve(Path::new(path)).map_err(|error| format!("{path}: {error}"))
    };
    if let Some(input) = options.nvram_in.as_deref() {
        let input = resolve(input)?;
        for other in outputs
            .into_iter()
            .chain([options.nvram_out.as_deref()])
            .flatten()
        {
            if input.aliases(&resolve(other)?) {
                return Err("the NVRAM input and all output paths must be distinct".to_owned());
            }
        }
    }
    if let Some(output) = options.nvram_out.as_deref() {
        let output = resolve(output)?;
        for other in outputs
            .into_iter()
            .chain([Some(options.image.as_str()), options.nvme_image.as_deref()])
            .flatten()
        {
            if output.aliases(&resolve(other)?) {
                return Err("the NVRAM output and all other paths must be distinct".to_owned());
            }
        }
    }
    Ok(())
}

fn validate_distinct_paths(
    image: &str,
    first: &str,
    second: &str,
    alias_error: &str,
) -> Result<(), String> {
    let resolve = |path: &str| {
        RebootPath::resolve(Path::new(path)).map_err(|error| format!("{path}: {error}"))
    };
    let image = resolve(image)?;
    let first = resolve(first)?;
    let second = resolve(second)?;
    if image.aliases(&first) || image.aliases(&second) || first.aliases(&second) {
        return Err(alias_error.to_owned());
    }
    Ok(())
}

struct BootRun {
    board: LoadedFirmwareBoard,
    report: Value,
    code: u8,
    fw_stop: Option<(u8, u64)>,
}

impl BootRun {
    fn returned_to_firmware(&self) -> bool {
        self.code == 6 && self.fw_stop == Some((0x21, 0))
    }

    fn finished_app(&self, success_line: &[u8]) -> bool {
        self.returned_to_firmware()
            && self
                .board
                .com1_transcript()
                .windows(success_line.len())
                .any(|line| line == success_line)
    }
}

fn exit_label(exit: &PagedProbeExit) -> &'static str {
    match exit {
        PagedProbeExit::Halted => "halted",
        PagedProbeExit::Shutdown => "shutdown",
        PagedProbeExit::BudgetExhausted => "budget_exhausted",
        PagedProbeExit::Stopped => "stopped",
        PagedProbeExit::Rejected(_) => "rejected",
        PagedProbeExit::Unsupported { .. } => "unsupported",
        PagedProbeExit::Fault(_) => "fault",
        PagedProbeExit::Backend(_) => "backend_error",
        PagedProbeExit::PortIo(_) => "pio_error",
        PagedProbeExit::IntegrationError(_) => "integration_error",
    }
}

/// Resident set of this process in KiB, from `ps` (the same on macOS and Linux).
fn rss_kib() -> Option<u64> {
    let output = Command::new("ps")
        .args(["-o", "rss=", "-p", &std::process::id().to_string()])
        .output()
        .ok()?;
    String::from_utf8_lossy(&output.stdout).trim().parse().ok()
}

/// Physical footprint of this process in KiB, from the macOS `footprint` tool: unlike RSS it
/// counts memory the compressor holds, so it does not shrink when idle pages are compressed.
/// `None` where the tool does not exist.
fn footprint_kib() -> Option<u64> {
    let output = Command::new("footprint")
        .args([
            "-p",
            &std::process::id().to_string(),
            "--format",
            "bytes",
            "--noCategories",
        ])
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&output.stdout);
    let rest = &text[text.find("Footprint: ")? + "Footprint: ".len()..];
    let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
    Some(digits.parse::<u64>().ok()? / 1024)
}

/// S7-26: `cycles` boots of one board in one process. Every boot of `--fv-app reboot` ends
/// with `ResetSystem`, which the machine reports as a `Reset` stop; the board is then reset by
/// `LoadedFirmwareBoard::platform_reset` (RAM, CPU and devices; NVRAM and the persistent TPM state
/// stay) and runs again. Nothing is rebuilt, so the memory of this process must stay flat. COM1
/// of all cycles goes to `--com1-out` in order; `boots[i].com1_offset` says where cycle `i`
/// begins. `rss_kib_*` (from `ps`) and `footprint_kib_*` (macOS `footprint`, which also counts
/// compressed memory) are the memory of this process at the start, after each cycle and at the end.
fn run_reboot_cycles(
    options: &Options,
    image: &[u8],
    config: &FirmwareBoardConfig,
    cycles: u64,
) -> ExitCode {
    let mut board = match load_firmware_image(image, config) {
        Ok(board) => board,
        Err(error) => return emit(failure("loader_error", "loader", format!("{error:?}")), 4),
    };
    let rss_start = rss_kib();
    let footprint_start = footprint_kib();
    let started = Instant::now();
    let mut com1: Vec<u8> = Vec::new();
    let mut boots = Vec::new();
    let mut rss_samples = Vec::new();
    let mut footprint_samples = Vec::new();
    let mut failed: Option<(&str, String)> = None;
    for cycle in 1..=cycles {
        let run = match board.run_boot(options.budget) {
            Ok(run) => run,
            Err(error) => {
                failed = Some(("run", format!("cycle {cycle}: {error:?}")));
                break;
            }
        };
        let reset = run.exit == PagedProbeExit::Stopped
            && run.stop_reason == Some(MachineStopReason::Reset);
        boots.push(json!({
            "cycle": cycle,
            "exit": exit_label(&run.exit),
            "stop_reason": run.stop_reason.map(|reason| format!("{reason:?}")),
            "retired": run.retired,
            "quanta": run.quanta,
            "virtual_ns": run.virtual_ns,
            "com1_offset": com1.len(),
            "com1_bytes": run.com1.len(),
        }));
        com1.extend_from_slice(&run.com1);
        if !reset {
            failed = Some((
                "reboot",
                format!(
                    "cycle {cycle} did not end with a Reset stop: {:?}",
                    run.exit
                ),
            ));
            break;
        }
        if let Err(error) = board.platform_reset(options.reboot_kind) {
            failed = Some(("platform_reset", format!("cycle {cycle}: {error:?}")));
            break;
        }
        rss_samples.push(rss_kib());
        footprint_samples.push(footprint_kib());
        // Progress for a long run; the report on stdout is written once, at the end.
        eprintln!(
            "reboot cycle {cycle}/{cycles}: retired {} wall_ms {} rss_kib {:?} footprint_kib {:?}",
            run.retired,
            started.elapsed().as_millis(),
            rss_samples.last().copied().flatten(),
            footprint_samples.last().copied().flatten(),
        );
    }
    let rss_end = rss_kib();
    let footprint_end = footprint_kib();
    if let Err(error) = fs::write(&options.com1_out, &com1) {
        return emit(failure("invalid_input", "com1_out", error.to_string()), 3);
    }
    let mut report = json!({
        "schema_version": 1,
        "command": "boot-firmware",
        "status": if failed.is_none() { "completed" } else { "error" },
        "reboot_cycles": cycles,
        "reboot_kind": format!("{:?}", options.reboot_kind),
        "image_sha256": sha256_hex(image),
        "budget": options.budget,
        "com1_bytes": com1.len(),
        "com1_out": options.com1_out,
        "rss_kib_start": rss_start,
        "rss_kib_end": rss_end,
        "rss_kib_after_cycle": rss_samples,
        "footprint_kib_start": footprint_start,
        "footprint_kib_end": footprint_end,
        "footprint_kib_after_cycle": footprint_samples,
        "wall_ms": started.elapsed().as_millis() as u64,
        "boots": boots,
    });
    let code = match failed {
        None => 0,
        Some((kind, detail)) => {
            report["error"] = json!({"kind": kind, "detail": detail});
            5
        }
    };
    emit(report, code)
}

fn boot(
    image: &[u8],
    config: &FirmwareBoardConfig,
    budget: u64,
    com1_out: &str,
    nvme_image: Option<&str>,
    sample_rips: &[u64],
    unified: bool,
) -> Result<BootRun, (Value, u8)> {
    let mut board = load_firmware_image(image, config)
        .map_err(|error| (failure("loader_error", "loader", format!("{error:?}")), 4))?;
    if let Some(path) = nvme_image {
        let bytes = fs::read(path)
            .map_err(|error| (failure("invalid_input", "nvme_image", error.to_string()), 3))?;
        let backend = InMemoryBackend::from_bytes(bytes).map_err(|error| {
            (
                failure("invalid_input", "nvme_image", format!("{error:?}")),
                3,
            )
        })?;
        board
            .nvme()
            .expect("runner enabled the NVMe controller")
            .attach_backend(Box::new(backend))
            .map_err(|error| {
                (
                    failure("loader_error", "nvme_attach", format!("{error:?}")),
                    4,
                )
            })?;
    }
    let handoff_sha256 = sha256_hex(board.handoff_block());
    let handoff_bytes = board.handoff_block().len();
    let mut samples = Vec::with_capacity(sample_rips.len());
    let mut unified_run = None;
    let run = if unified {
        // S7-31c: the ROM and the guest in the CPL0 profile set loop (ADR-0013 variant (b)).
        od_machine::run_firmware_unified(&mut board, budget).map(|run| {
            let report = run.run;
            unified_run = Some(json!({
                "set_exit_detail": run.set_exit_detail,
                "members": run.members,
                "exceptions_delivered": run.exceptions_delivered,
                "page_faults_delivered": run.page_faults_delivered,
                "cr3_changes": run.cr3_changes,
                "cpl3_retired": run.cpl3_retired,
                "cpuid_divergence_leaf": run.cpuid_divergence_leaf,
                "rip_physical": run.rip_physical.map(|address| format!("0x{address:x}")),
                "rip_bytes": run.rip_bytes.iter().map(|byte| format!("{byte:02x}")).collect::<String>(),
            }));
            report
        })
    } else if sample_rips.is_empty() {
        board.run_boot(budget)
    } else {
        let start = Instant::now();
        board.run_boot_observed(budget, |rip, retired, quanta| {
            if sample_rips.get(samples.len()) == Some(&rip) {
                samples.push((rip, start.elapsed().as_nanos(), retired, quanta));
            }
        })
    };
    let report = match run {
        Ok(report) => report,
        Err(error) => {
            // Keep the bytes the guest already wrote (even an empty transcript).
            let mut value = failure("run_error", "run", format!("{error:?}"));
            if let Err(write_error) = std::fs::write(com1_out, board.com1_transcript()) {
                value["com1_out_error"] = json!(write_error.to_string());
            }
            value["com1_bytes"] = json!(board.com1_transcript().len());
            value["com1_out"] = json!(com1_out);
            return Err((value, 5));
        }
    };
    if let Err(error) = std::fs::write(com1_out, &report.com1) {
        return Err((failure("invalid_input", "com1_out", error.to_string()), 3));
    }
    let fw_stop = parse_fw_stop(&report.com1);
    let exit = exit_label(&report.exit);
    let halted = matches!(report.exit, PagedProbeExit::Halted);
    let rt = config.fv_app == Some(FirmwareFvApp::Rt);
    let hardware_stop =
        matches!(report.exit, PagedProbeExit::Stopped) && report.stop_reason.is_some();
    let completed = if rt {
        hardware_stop && report.stop_reason == Some(MachineStopReason::Shutdown)
    } else {
        halted || hardware_stop
    };
    let (status, code) = match (completed, &fw_stop) {
        (true, None) => ("completed", 0),
        (true, Some(_)) => ("firmware_stop", 6),
        (false, _) => ("error", 5),
    };
    let state = board.state();
    let mut result = json!({
        "schema_version": 1,
        "command": "boot-firmware",
        "status": status,
        "exit": exit,
        "exit_detail": if halted || completed { Value::Null } else { json!(format!("{:?}", report.exit)) },
        "retired": report.retired,
        "quanta": report.quanta,
        "rip": format!("0x{:016x}", state.rip),
        "activity": state.activity.map(|activity| format!("{activity:?}")),
        "budget": budget,
        "tsc_hz": report.tsc_hz,
        "time_step_ns_per_instruction": FIRMWARE_EXECUTION_CYCLE_NS,
        "virtual_ns": report.virtual_ns,
        "image_sha256": sha256_hex(image),
        "handoff_sha256": handoff_sha256,
        "handoff_bytes": handoff_bytes,
        "com1_bytes": report.com1.len(),
        "com1_out": com1_out,
        "fw_stop": fw_stop.map(|(code, detail)| json!({
            "code": format!("0x{code:02x}"),
            "detail": format!("0x{detail:016x}"),
        })),
        "page_fault": report.page_fault.map(|fault| format!("{fault:?}")),
        "memory_error": report.memory_error.map(|error| format!("{error:?}")),
    });
    if rt || report.stop_reason.is_some() {
        result["stop_reason"] = json!(report.stop_reason.map(|reason| match reason {
            MachineStopReason::Reset => "Reset",
            MachineStopReason::Shutdown => "Shutdown",
        }));
    }
    if let Some(unified_run) = unified_run {
        result["cpu_loop"] = json!("unified");
        result["unified"] = unified_run;
    }
    if !sample_rips.is_empty() {
        result["rip_samples"] = json!(
            samples
                .into_iter()
                .map(|(rip, host_ns, retired, quanta)| json!({
                    "rip": format!("0x{rip:016x}"),
                    "host_ns": host_ns,
                    "retired": retired,
                    "quanta": quanta,
                }))
                .collect::<Vec<_>>()
        );
    }
    Ok(BootRun {
        board,
        report: result,
        code,
        fw_stop,
    })
}

fn parse(args: &[String]) -> Result<Options, String> {
    let (mut image, mut com1_out, mut budget) = (None, None, None);
    let mut fv_app = None;
    let mut nvram_reboot_out = None;
    let mut nvram_in = None;
    let mut nvram_out = None;
    let mut acpi_out = None;
    let mut framebuffer_out = None;
    let mut nvme_image = None;
    let mut tpm = false;
    let mut reboot_cycles = None;
    let mut reboot_kind = PlatformResetKind::Cold;
    let mut sample_rips = Vec::new();
    let mut unified = false;
    let mut index = 0;
    while index < args.len() {
        let name = args[index].as_str();
        if name == "--tpm" && !tpm {
            tpm = true;
            index += 1;
            continue;
        }
        let value = args
            .get(index + 1)
            .ok_or_else(|| format!("missing value for {name}; usage: {USAGE}"))?;
        match name {
            "--image" if image.is_none() => image = Some(value.clone()),
            "--com1-out" if com1_out.is_none() => com1_out = Some(value.clone()),
            "--acpi-out" if acpi_out.is_none() => acpi_out = Some(value.clone()),
            "--nvme-image" if nvme_image.is_none() => nvme_image = Some(value.clone()),
            "--nvram-in" if nvram_in.is_none() => nvram_in = Some(value.clone()),
            "--nvram-out" if nvram_out.is_none() => nvram_out = Some(value.clone()),
            "--framebuffer-out" if framebuffer_out.is_none() => {
                framebuffer_out = Some(value.clone())
            }
            "--reboot-cycles" if reboot_cycles.is_none() => {
                reboot_cycles = Some(
                    value
                        .parse::<u64>()
                        .ok()
                        .filter(|cycles| *cycles > 0)
                        .ok_or_else(|| format!("invalid --reboot-cycles '{value}'"))?,
                );
            }
            "--reboot-kind" => {
                reboot_kind = match value.as_str() {
                    "cold" => PlatformResetKind::Cold,
                    "warm" => PlatformResetKind::Warm,
                    _ => {
                        return Err(format!(
                            "invalid --reboot-kind '{value}'; expected cold or warm"
                        ));
                    }
                };
            }
            "--nvram-reboot-out" if nvram_reboot_out.is_none() => {
                nvram_reboot_out = Some(value.clone())
            }
            "--budget" if budget.is_none() => {
                budget = Some(
                    value
                        .parse::<u64>()
                        .map_err(|_| format!("invalid --budget '{value}'"))?,
                );
            }
            "--fv-app" if fv_app.is_none() => {
                fv_app = Some(parse_app(value)?);
            }
            "--cpu-loop" => match value.as_str() {
                "unified" => unified = true,
                "firmware" => unified = false,
                _ => {
                    return Err(format!(
                        "invalid --cpu-loop '{value}'; expected firmware or unified"
                    ));
                }
            },
            "--sample-rip" if sample_rips.len() < MAX_RIP_SAMPLES => {
                let hex = value.strip_prefix("0x").unwrap_or(value);
                let rip = u64::from_str_radix(hex, 16)
                    .map_err(|_| format!("invalid --sample-rip '{value}'"))?;
                sample_rips.push(rip);
            }
            other => {
                return Err(format!(
                    "unexpected or duplicate option {other}; usage: {USAGE}"
                ));
            }
        }
        index += 2;
    }
    if reboot_cycles.is_some() {
        if fv_app != Some(FirmwareFvApp::Reboot) {
            return Err("--reboot-cycles requires --fv-app reboot".to_owned());
        }
        if nvram_reboot_out.is_some()
            || nvram_in.is_some()
            || nvram_out.is_some()
            || nvme_image.is_some()
            || !sample_rips.is_empty()
        {
            return Err(
                "--reboot-cycles does not combine with the NVRAM, NVMe and sample options"
                    .to_owned(),
            );
        }
    }
    if acpi_out.is_some() && fv_app != Some(FirmwareFvApp::Acpi) {
        return Err("--acpi-out requires --fv-app acpi".to_owned());
    }
    if framebuffer_out.is_some() && fv_app != Some(FirmwareFvApp::Gop) {
        return Err("--framebuffer-out requires --fv-app gop".to_owned());
    }
    let storage_app = match fv_app {
        Some(FirmwareFvApp::Blk) => Some("blk"),
        Some(FirmwareFvApp::Fs) => Some("fs"),
        _ => None,
    };
    if let Some(app) = storage_app
        && nvme_image.is_none()
    {
        return Err(format!("--fv-app {app} requires --nvme-image"));
    }
    let image = image.ok_or_else(|| format!("--image is required; usage: {USAGE}"))?;
    let com1_out = com1_out.ok_or_else(|| format!("--com1-out is required; usage: {USAGE}"))?;
    if let Some(reboot_out) = &nvram_reboot_out {
        if !sample_rips.is_empty() {
            return Err("--sample-rip requires a single boot".to_owned());
        }
        if fv_app != Some(FirmwareFvApp::Vars) {
            return Err("--nvram-reboot-out requires --fv-app vars".to_owned());
        }
        if reboot_out == &com1_out || reboot_out == &image || com1_out == image {
            return Err("the image and both COM1 output paths must be distinct".to_owned());
        }
    }
    Ok(Options {
        image,
        com1_out,
        budget: budget.ok_or_else(|| format!("--budget is required; usage: {USAGE}"))?,
        fv_app,
        nvram_reboot_out,
        nvram_in,
        nvram_out,
        acpi_out,
        framebuffer_out,
        nvme_image,
        tpm,
        reboot_cycles,
        reboot_kind,
        sample_rips,
        unified,
    })
}

/// `(code, detail)` of the `OD-FW STOP code=0xNN detail=0xDDDDDDDDDDDDDDDD`
/// line, but only when it is the LAST non-empty line of the COM1 transcript
/// (FIRMWARE_BOOT.md section 12: the runner parses the last line). A STOP line
/// followed by more text is not a STOP.
fn parse_fw_stop(com1: &[u8]) -> Option<(u8, u64)> {
    let text = String::from_utf8_lossy(com1);
    let line = text.lines().rev().find(|line| !line.is_empty())?;
    let rest = line.strip_prefix("OD-FW STOP code=0x")?;
    let (code, detail) = rest.split_once(" detail=0x")?;
    if code.len() != 2 || detail.len() != 16 {
        return None;
    }
    Some((
        u8::from_str_radix(code, 16).ok()?,
        u64::from_str_radix(detail, 16).ok()?,
    ))
}

fn failure(status: &str, kind: &str, detail: String) -> Value {
    json!({
        "schema_version": 1,
        "command": "boot-firmware",
        "status": status,
        "exit": status,
        "error": {"kind": kind, "detail": detail},
    })
}

fn emit(value: Value, code: u8) -> ExitCode {
    println!("{value}");
    ExitCode::from(code)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| value.to_string()).collect()
    }

    #[test]
    fn cpu_loop_selects_the_unified_loop_and_defaults_to_the_firmware_profile() {
        let required = args(&[
            "--image",
            "rom.bin",
            "--com1-out",
            "out.com1",
            "--budget",
            "10",
        ]);
        assert!(!parse(&required).unwrap().unified);
        for (value, unified) in [("unified", true), ("firmware", false)] {
            let mut given = required.clone();
            given.extend(args(&["--cpu-loop", value]));
            assert_eq!(parse(&given).unwrap().unified, unified);
        }
        let mut invalid = required;
        invalid.extend(args(&["--cpu-loop", "kvm"]));
        assert!(
            parse(&invalid)
                .err()
                .unwrap()
                .contains("invalid --cpu-loop")
        );
    }

    #[test]
    fn firmware_boot_samples_allow_ordered_repeats_in_one_boot_only() {
        let required = args(&[
            "--image",
            "rom.bin",
            "--com1-out",
            "boot.com1",
            "--budget",
            "4",
        ]);
        let mut requested = required.clone();
        for _ in 0..MAX_RIP_SAMPLES {
            requested.extend(args(&["--sample-rip", "0x200040"]));
        }
        assert_eq!(
            parse(&requested).unwrap().sample_rips,
            [0x200040; MAX_RIP_SAMPLES]
        );
        requested.extend(args(&["--sample-rip", "200041"]));
        assert!(parse(&requested).is_err());

        let mut invalid = required.clone();
        invalid.extend(args(&["--sample-rip", "not-hex"]));
        assert!(
            parse(&invalid)
                .err()
                .unwrap()
                .contains("invalid --sample-rip")
        );

        let mut reboot = required;
        reboot.extend(args(&[
            "--sample-rip",
            "200040",
            "--fv-app",
            "vars",
            "--nvram-reboot-out",
            "second.com1",
        ]));
        assert!(
            parse(&reboot)
                .err()
                .unwrap()
                .contains("requires a single boot")
        );
    }

    #[test]
    fn reboot_cycles_need_the_reboot_app_and_exclude_the_other_modes() {
        let required = [
            "--image",
            "rom.bin",
            "--com1-out",
            "first.com1",
            "--budget",
            "4000000",
        ];
        for (extra, expected) in [
            (
                &["--reboot-cycles", "5"][..],
                Some("requires --fv-app reboot"),
            ),
            (
                &["--reboot-cycles", "5", "--fv-app", "vars"][..],
                Some("requires --fv-app reboot"),
            ),
            (
                &["--reboot-cycles", "0", "--fv-app", "reboot"][..],
                Some("invalid --reboot-cycles"),
            ),
            (
                &["--reboot-cycles", "x", "--fv-app", "reboot"][..],
                Some("invalid --reboot-cycles"),
            ),
            (
                &[
                    "--reboot-cycles",
                    "5",
                    "--reboot-kind",
                    "tepid",
                    "--fv-app",
                    "reboot",
                ][..],
                Some("invalid --reboot-kind"),
            ),
            (
                &[
                    "--reboot-cycles",
                    "5",
                    "--fv-app",
                    "reboot",
                    "--nvram-out",
                    "x.nv",
                ][..],
                Some("does not combine"),
            ),
            (
                &[
                    "--reboot-cycles",
                    "5",
                    "--fv-app",
                    "reboot",
                    "--sample-rip",
                    "0x200040",
                ][..],
                Some("does not combine"),
            ),
            (&["--reboot-cycles", "5", "--fv-app", "reboot"][..], None),
        ] {
            let input = args(&[&required[..], extra].concat());
            match expected {
                Some(error) => assert!(parse(&input).err().unwrap().contains(error)),
                None => {
                    let options = parse(&input).unwrap();
                    assert_eq!(options.reboot_cycles, Some(5));
                    assert_eq!(options.reboot_kind, PlatformResetKind::Cold);
                }
            }
        }
        let warm = args(
            &[
                &required[..],
                &[
                    "--reboot-cycles",
                    "2",
                    "--reboot-kind",
                    "warm",
                    "--fv-app",
                    "reboot",
                ][..],
            ]
            .concat(),
        );
        assert_eq!(parse(&warm).unwrap().reboot_kind, PlatformResetKind::Warm);
        // Without the flag the app runs once and stops on its reset.
        let single = args(&[&required[..], &["--fv-app", "reboot"][..]].concat());
        assert_eq!(parse(&single).unwrap().reboot_cycles, None);
    }

    #[test]
    fn nvram_reboot_requires_vars_and_distinct_output_paths() {
        let required = [
            "--image",
            "rom.bin",
            "--com1-out",
            "first.com1",
            "--budget",
            "4000000",
        ];
        for (extra, expected) in [
            (
                &["--nvram-reboot-out", "second.com1"][..],
                Some("requires --fv-app vars"),
            ),
            (
                &["--nvram-reboot-out", "first.com1", "--fv-app", "vars"][..],
                Some("must be distinct"),
            ),
            (
                &["--nvram-reboot-out", "rom.bin", "--fv-app", "vars"][..],
                Some("must be distinct"),
            ),
            (
                &["--nvram-reboot-out", "second.com1", "--fv-app", "vars"][..],
                None,
            ),
        ] {
            let input = args(&[&required[..], extra].concat());
            match expected {
                Some(error) => assert!(parse(&input).err().unwrap().contains(error)),
                None => assert_eq!(
                    parse(&input).unwrap().nvram_reboot_out.as_deref(),
                    Some("second.com1")
                ),
            }
        }
    }

    #[cfg(unix)]
    #[test]
    fn nvram_reboot_resolves_rom_and_output_aliases_before_writing() {
        use std::os::unix::fs::symlink;
        let dir =
            std::env::temp_dir().join(format!("od-nvram-reboot-paths-{}", std::process::id()));
        fs::create_dir(&dir).unwrap();
        let rom = dir.join("rom.bin");
        let first = dir.join("first.com1");
        let second = dir.join("second.com1");
        fs::write(&rom, b"original ROM").unwrap();
        let check = |first: &Path, second: &Path| {
            validate_reboot_paths(
                rom.to_str().unwrap(),
                first.to_str().unwrap(),
                second.to_str().unwrap(),
            )
        };
        assert!(check(&first, &second).is_ok());
        assert!(check(&first, &dir.join("./rom.bin")).is_err());
        assert!(check(&first, &dir.join("./first.com1")).is_err());
        fs::hard_link(&rom, &second).unwrap();
        assert!(check(&first, &second).is_err());
        fs::remove_file(&second).unwrap();
        symlink("first.com1", &second).unwrap();
        assert!(check(&first, &second).is_err());
        fs::write(&first, b"first transcript").unwrap();
        assert!(check(&first, &second).is_err());
        assert_eq!(fs::read(&rom).unwrap(), b"original ROM");
        assert_eq!(fs::read(&first).unwrap(), b"first transcript");
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn fw_stop_is_recognised_only_as_the_last_non_empty_line() {
        // Earlier STOP lines do not count; only the last line is parsed.
        let transcript = b"boot\r\nOD-FW STOP code=0x02 detail=0x0000000000000000\r\n\
            OD-FW STOP code=0x10 detail=0x000000000000000e\r\n";
        assert_eq!(parse_fw_stop(transcript), Some((0x10, 0xe)));
        // Trailing blank lines are ignored.
        assert_eq!(
            parse_fw_stop(b"OD-FW STOP code=0x10 detail=0x000000000000000e\r\n\r\n\r\n"),
            Some((0x10, 0xe))
        );
        // A STOP followed by more text is NOT a STOP.
        assert_eq!(
            parse_fw_stop(b"OD-FW STOP code=0x10 detail=0x000000000000000e\r\nmore\r\n"),
            None
        );
        assert_eq!(
            parse_fw_stop(
                b"OD-FW STOP code=0x10 detail=0x000000000000000e\r\n\
                  OD-FW STOP code=0x10 detail=0x000000000000000e\r\nx"
            ),
            None
        );
        assert_eq!(parse_fw_stop(b"no stop here\r\n"), None);
        assert_eq!(parse_fw_stop(b""), None);
        // Malformed widths and prefixes are not a STOP line.
        assert_eq!(
            parse_fw_stop(b"OD-FW STOP code=0x1 detail=0x0000000000000000\r\n"),
            None
        );
        assert_eq!(parse_fw_stop(b"OD-FW STOP code=0x10 detail=0x1\r\n"), None);
        assert_eq!(
            parse_fw_stop(b"xOD-FW STOP code=0x10 detail=0x0000000000000000\r\n"),
            None
        );
        // A STOP line without the trailing newline still counts.
        assert_eq!(
            parse_fw_stop(b"OD-FW STOP code=0x30 detail=0xffffffffffffffff"),
            Some((0x30, u64::MAX))
        );
    }

    #[test]
    fn arguments_are_strict() {
        let ok = parse(&args(&["--image", "a", "--com1-out", "b", "--budget", "7"])).unwrap();
        assert_eq!(
            (ok.image.as_str(), ok.com1_out.as_str(), ok.budget),
            ("a", "b", 7)
        );
        for bad in [
            args(&[]),
            args(&["--image", "a", "--com1-out", "b"]),
            args(&["--image", "a", "--com1-out", "b", "--budget", "x"]),
            args(&[
                "--image",
                "a",
                "--image",
                "c",
                "--com1-out",
                "b",
                "--budget",
                "1",
            ]),
            args(&["--image", "a", "--com1-out", "b", "--budget"]),
            args(&[
                "--image",
                "a",
                "--com1-out",
                "b",
                "--budget",
                "1",
                "--extra",
                "2",
            ]),
        ] {
            assert!(parse(&bad).is_err(), "{bad:?}");
        }
    }
}
