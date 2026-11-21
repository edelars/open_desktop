//! `boot-firmware`: start a firmware image on firmware board v1 (S7-03).
//!
//! The runner only reads the image, calls `od_machine::load_firmware_image`,
//! drives `run_boot` and reports. Loading, quanta, COM1 draining and virtual
//! time (`retired * 1 ns` per quantum) live in `od-machine`
//! (`docs/specs/FIRMWARE_BOOT.md` sections 8 and 11).

use crate::run_journal::{
    DEFAULT_MAX_BYTES, DEFAULT_MAX_FILES, FrameDumpConfig, JournalConfig, RunJournal, StopInfo,
};
use crate::stop_snapshot::{self, BoardMemory};
use od_machine::{
    FIRMWARE_EXECUTION_CYCLE_NS, FIRMWARE_IMAGE_BYTES, FirmwareBoardConfig, FirmwareFvApp,
    InMemoryBackend, LoadedFirmwareBoard, MachineStopReason, NVRAM_SNAPSHOT_BYTES, NvramConfig,
    NvramDevice, PagedProbeExit, QuantumProgress, SecureBootKeys, TpmConfig,
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
                    [--fv-app hello|ebs|vars|rt|acpi|gop|blk|fs|tpm] [--tpm] [--nvram-reboot-out <file>] \
                    [--nvram-in <file>] [--nvram-out <file>] \
                    [--acpi-out <file>] [--framebuffer-out <file>] [--nvme-image <raw> | --disk <path>[,ro]] \
                    [--sample-rip <hex> ...] [--stop-snapshot <file>] [--phase-log <file>] \
                    [--framebuffer-every <virtual seconds> --framebuffer-dir <dir> \
                    [--framebuffer-max-files <n>] [--framebuffer-max-bytes <n>]]\n\
\x20\x20--disk <path>,ro: the ',ro' suffix is not supported yet and is rejected \
(docs/specs/NVME_DEVICE.md 14.5).\n\
\x20\x20--disk with --nvram-reboot-out: the second boot reopens the disk after the first \
board is dropped, so the writer lock (LOCK_EX) is released between the two boots \
(limitation until S8-23).";
// Match the milestones in the given order. Repeated RIPs consume separate
// visits, and later milestones remain pending until the next expected RIP.
const MAX_RIP_SAMPLES: usize = 64;
const DUMP_USAGE: &str =
    "dump-handoff-block --out <file> [--fv-app hello|ebs|vars|rt|acpi|gop|blk|fs|tpm]";
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
    /// Smoke-suite input: the whole file is copied into memory and attached as
    /// a volatile namespace. Guest writes never reach the file.
    nvme_image: Option<String>,
    /// S8-21: RAW file attached through the stage 5 persistent backend (locked,
    /// durable flush after each write). Mutually exclusive with `nvme_image`.
    disk: Option<String>,
    tpm: bool,
    sample_rips: Vec<u64>,
    /// S8-10: JSON snapshot of the stopped machine (any stop except machine `Shutdown`).
    stop_snapshot: Option<String>,
    /// S8-11: JSON lines, one per phase event.
    phase_log: Option<String>,
    /// S8-11: periodic raw screen dumps of any boot.
    framebuffer: Option<FramebufferDumps>,
}

/// `--framebuffer-every`, `--framebuffer-dir` and the limits.
struct FramebufferDumps {
    dir: String,
    every_ns: u64,
    max_files: u64,
    max_bytes: u64,
}

/// Where the single NVMe namespace of the board comes from.
#[derive(Clone, Copy)]
enum NvmeSource<'a> {
    Memory(&'a str),
    Disk(&'a str),
}

impl Options {
    fn nvme_path(&self) -> Option<&str> {
        self.disk.as_deref().or(self.nvme_image.as_deref())
    }

    fn nvme_source(&self) -> Option<NvmeSource<'_>> {
        match (&self.disk, &self.nvme_image) {
            (Some(path), _) => Some(NvmeSource::Disk(path)),
            (None, Some(path)) => Some(NvmeSource::Memory(path)),
            (None, None) => None,
        }
    }
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
        nvram: (rt || reboot).then_some(NvramConfig::Empty),
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
        _ => Err(format!(
            "invalid --fv-app '{value}'; expected hello, ebs, vars, rt, acpi, gop, blk, fs or tpm"
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
        let tpm = (app == Some(FirmwareFvApp::Tpm)).then(|| TpmConfig {
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
    if let Some(path) = options.nvme_path()
        && let Err(error) = validate_nvme_paths(path, &options)
    {
        return emit(failure("invalid_input", "paths", error), 3);
    }
    if let Err(error) = validate_nvram_paths(&options) {
        return emit(failure("invalid_input", "paths", error), 3);
    }
    if let Err(error) = validate_diagnostic_paths(&options) {
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
    let tpm = if options.tpm || options.fv_app == Some(FirmwareFvApp::Tpm) {
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
    if options.nvme_path().is_some() {
        config.nvme = Some(Default::default());
    }
    if let Some(path) = &options.nvram_in {
        let bytes = match read_nvram_snapshot(path) {
            Ok(bytes) => bytes,
            Err(error) => return emit(failure("invalid_input", "nvram_in", error.to_string()), 3),
        };
        config.nvram = Some(NvramConfig::Snapshot(bytes));
    } else if options.nvram_out.is_some()
        || (options.nvme_path().is_some() && options.fv_app.is_none())
    {
        config.nvram.get_or_insert(NvramConfig::Empty);
    }
    // The journal creates its files (the phase log and the dump directory), so only its
    // configuration is built here; `boot` creates it once the image is loaded and the NVMe
    // controller is attached. A run refused before that point (bad arguments or paths, an
    // unreadable image or NVRAM snapshot, a loader or NVMe refusal) leaves neither file behind.
    let mut diagnostics = Diagnostics {
        stop_snapshot: options.stop_snapshot.clone(),
        journal_config: journal_config(&options),
        journal: None,
    };
    let nvme = options.nvme_source();
    let mut first = match boot(
        &image,
        &config,
        options.budget,
        &options.com1_out,
        nvme,
        &options.sample_rips,
        &mut diagnostics,
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
    let Some(reboot_out) = options.nvram_reboot_out.clone() else {
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
    // Known limitation (until S8-23): the board owns the persistent disk backend
    // and its writer lock (LOCK_EX). Dropping it releases the lock, and the second
    // boot below opens the --disk file again, so between the two boots another
    // process can take the lock and write the image.
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
        nvme,
        &[],
        &mut Diagnostics::default(),
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
    /// Like `resolve`, for a directory the run may still create (`create_dir_all`): the part
    /// that does not exist yet is appended to the resolved nearest existing ancestor, so a dump
    /// directory under missing parents is judged the same with and without `--phase-log`.
    fn resolve_creatable_dir(path: &Path) -> io::Result<Self> {
        let mut existing = path;
        let mut missing = Vec::new();
        loop {
            match fs::symlink_metadata(existing) {
                Ok(_) => break,
                Err(error) if error.kind() == io::ErrorKind::NotFound => {
                    let Some(name) = existing.file_name() else {
                        return Err(error);
                    };
                    missing.push(name);
                    existing = existing
                        .parent()
                        .filter(|parent| !parent.as_os_str().is_empty())
                        .unwrap_or(Path::new("."));
                }
                Err(error) => return Err(error),
            }
        }
        let mut resolved = Self::resolve(existing)?;
        if !missing.is_empty() {
            for name in missing.into_iter().rev() {
                resolved.absolute.push(name);
            }
            resolved.metadata = None;
        }
        Ok(resolved)
    }

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
    if options.disk.is_some() {
        // A disk is written by the guest, so it must not be any other file the
        // runner reads or writes (the in-memory `--nvme-image` is only read).
        for other in [
            Some(options.image.as_str()),
            options.nvram_in.as_deref(),
            options.nvram_out.as_deref(),
        ]
        .into_iter()
        .flatten()
        {
            let target = RebootPath::resolve(Path::new(other))
                .map_err(|error| format!("{other}: {error}"))?;
            if input.aliases(&target) {
                return Err(
                    "the --disk file must be distinct from the firmware image and NVRAM files"
                        .to_owned(),
                );
            }
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
            .chain([Some(options.image.as_str()), options.nvme_path()])
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

/// `--stop-snapshot` and `--phase-log` must not alias another file of the run, nor each other,
/// and must not lie inside (or be) the `--framebuffer-dir` directory of the screen dumps.
fn validate_diagnostic_paths(options: &Options) -> Result<(), String> {
    let new = [
        ("--stop-snapshot", options.stop_snapshot.as_deref()),
        ("--phase-log", options.phase_log.as_deref()),
    ];
    if new.iter().all(|(_, path)| path.is_none()) {
        return Ok(());
    }
    let all = [
        ("--image", Some(options.image.as_str())),
        ("--com1-out", Some(options.com1_out.as_str())),
        ("the NVMe file", options.nvme_path()),
        ("--nvram-in", options.nvram_in.as_deref()),
        ("--nvram-out", options.nvram_out.as_deref()),
        ("--acpi-out", options.acpi_out.as_deref()),
        ("--framebuffer-out", options.framebuffer_out.as_deref()),
        new[0],
        new[1],
    ];
    let resolved = all
        .iter()
        .filter_map(|(name, path)| path.map(|path| (*name, path)))
        .map(|(name, path)| {
            RebootPath::resolve(Path::new(path))
                .map(|resolved| (name, resolved))
                .map_err(|error| format!("{path}: {error}"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    for (index, (name, path)) in resolved.iter().enumerate() {
        if !matches!(*name, "--stop-snapshot" | "--phase-log") {
            continue;
        }
        if let Some((other, _)) = resolved
            .iter()
            .enumerate()
            .find(|(other_index, (_, other))| *other_index != index && path.aliases(other))
            .map(|(_, entry)| entry)
        {
            return Err(format!("{name} and {other} must be distinct files"));
        }
    }
    if let Some(dumps) = &options.framebuffer {
        let dir = RebootPath::resolve_creatable_dir(Path::new(&dumps.dir))
            .map_err(|error| format!("{}: {error}", dumps.dir))?;
        for (name, path) in &resolved {
            if matches!(*name, "--stop-snapshot" | "--phase-log")
                && path.absolute.starts_with(&dir.absolute)
            {
                return Err(format!(
                    "{name} must not be the --framebuffer-dir directory or a file inside it"
                ));
            }
        }
    }
    Ok(())
}

fn journal_config(options: &Options) -> Option<JournalConfig> {
    if options.phase_log.is_none() && options.framebuffer.is_none() {
        return None;
    }
    Some(JournalConfig {
        phase_log: options.phase_log.as_ref().map(PathBuf::from),
        frames: options.framebuffer.as_ref().map(|dumps| FrameDumpConfig {
            dir: PathBuf::from(&dumps.dir),
            every_ns: dumps.every_ns,
            max_files: dumps.max_files,
            max_bytes: dumps.max_bytes,
        }),
    })
}

/// The optional diagnostics of one boot (S8-10, S8-11). Both only read the machine.
#[derive(Default)]
struct Diagnostics {
    stop_snapshot: Option<String>,
    /// What `boot` builds the journal from; taken when the journal is created.
    journal_config: Option<JournalConfig>,
    journal: Option<RunJournal>,
}

/// The vCPU the single-processor firmware boot runs on.
const BOOT_VCPU: usize = 0;

/// Writes the snapshot of the stopped machine and says what happened. Never changes the machine:
/// it reads the vCPU registers and guest-virtual memory through the board's diagnostic API.
fn write_stop_snapshot(
    path: &str,
    board: &LoadedFirmwareBoard,
    reason: Value,
    counters: Value,
    inputs: Value,
) -> Value {
    let Some(state) = board.diagnostic_vcpu_state(BOOT_VCPU) else {
        return json!({"path": path, "written": false, "error": "the board has no boot vCPU"});
    };
    let memory = BoardMemory {
        board,
        vcpu: BOOT_VCPU,
    };
    let snapshot = stop_snapshot::snapshot(&state, &memory, reason, counters, inputs);
    let mut text = serde_json::to_string_pretty(&snapshot).expect("a JSON value serializes");
    text.push('\n');
    match fs::write(path, text) {
        Ok(()) => json!({"path": path, "written": true}),
        Err(error) => json!({"path": path, "written": false, "error": error.to_string()}),
    }
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

/// Attaches an existing RAW file through the stage 5 persistent backend: the
/// writer lock (`LOCK_EX`) is held until the backend is dropped, so a second
/// runner (or a second boot in this process while the first board is alive)
/// cannot write the same image, and every guest write is flushed durably
/// before it completes. The file is neither created, resized nor copied into
/// memory.
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
fn attach_disk(nvme: &od_machine::NvmeHandle, path: &str) -> Result<(), (Value, u8)> {
    use od_host::persistent_nvme::{
        AttachPersistentNvmeError, PersistentNvmeOpenError, attach_persistent_nvme,
    };
    use od_host::storage_io::WriterOpenError;
    attach_persistent_nvme(nvme, Path::new(path)).map_err(|error| match error {
        AttachPersistentNvmeError::Open(PersistentNvmeOpenError::WriterOpen(
            WriterOpenError::WriterBusy,
        )) => (
            failure(
                "invalid_input",
                "disk",
                format!("{path}: the disk image is locked by another writer"),
            ),
            3,
        ),
        AttachPersistentNvmeError::Open(error) => (
            failure("invalid_input", "disk", format!("{path}: {error}")),
            3,
        ),
        AttachPersistentNvmeError::Attach(error) => (
            failure("loader_error", "nvme_attach", format!("{error:?}")),
            4,
        ),
    })
}

#[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
fn attach_disk(_nvme: &od_machine::NvmeHandle, _path: &str) -> Result<(), (Value, u8)> {
    Err((
        failure(
            "invalid_input",
            "disk",
            "--disk needs the macOS arm64 host backend (od-host)".to_owned(),
        ),
        3,
    ))
}

fn boot(
    image: &[u8],
    config: &FirmwareBoardConfig,
    budget: u64,
    com1_out: &str,
    nvme: Option<NvmeSource<'_>>,
    sample_rips: &[u64],
    diagnostics: &mut Diagnostics,
) -> Result<BootRun, (Value, u8)> {
    let mut board = load_firmware_image(image, config)
        .map_err(|error| (failure("loader_error", "loader", format!("{error:?}")), 4))?;
    if let Some(NvmeSource::Disk(path)) = nvme {
        attach_disk(
            &board.nvme().expect("runner enabled the NVMe controller"),
            path,
        )?;
    }
    // Hashes of the inputs the stop snapshot records (only computed when it is requested).
    let snapshot_wanted = diagnostics.stop_snapshot.is_some();
    let mut nvme_input = Value::Null;
    if let Some(NvmeSource::Disk(path)) = nvme
        && snapshot_wanted
    {
        // The disk is written by the guest and can be tens of GiB: path and size, no hash.
        nvme_input = json!({
            "kind": "disk", "path": path,
            "bytes": fs::metadata(path).ok().map(|metadata| metadata.len()),
            "sha256": null,
        });
    }
    if let Some(NvmeSource::Memory(path)) = nvme {
        let bytes = fs::read(path)
            .map_err(|error| (failure("invalid_input", "nvme_image", error.to_string()), 3))?;
        if snapshot_wanted {
            nvme_input = json!({
                "kind": "nvme_image", "path": path,
                "bytes": bytes.len(), "sha256": sha256_hex(&bytes),
            });
        }
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
    // Every refusal of `boot` is behind us: only now do the journal's files appear.
    if let Some(config) = diagnostics.journal_config.take() {
        diagnostics.journal = Some(
            RunJournal::new(config)
                .map_err(|error| (failure("invalid_input", "diagnostics", error), 3))?,
        );
    }
    let handoff_sha256 = sha256_hex(board.handoff_block());
    let handoff_bytes = board.handoff_block().len();
    let mut samples = Vec::with_capacity(sample_rips.len());
    let run = if let Some(journal) = &mut diagnostics.journal {
        board.run_boot_with_quantum_hook(budget, |board, progress| {
            journal.at_boundary(board, progress)
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
    let inputs = || {
        json!({
            "image_sha256": sha256_hex(image),
            "handoff_sha256": handoff_sha256,
            "budget": budget,
            "fv_app": config.fv_app.map(|app| format!("{app:?}")),
            "tpm": config.tpm.is_some(),
            "nvram_in_sha256": match &config.nvram {
                Some(NvramConfig::Snapshot(bytes)) => json!(sha256_hex(bytes)),
                _ => Value::Null,
            },
            "nvme": nvme_input,
        })
    };
    let report = match run {
        Ok(report) => report,
        Err(error) => {
            // Keep the bytes the guest already wrote (even an empty transcript).
            let mut value = failure("run_error", "run", format!("{error:?}"));
            // The machine stays as the failed run left it; the diagnostics only read it.
            if let Some(journal) = &mut diagnostics.journal {
                value["journal"] = journal.finish(
                    &board,
                    StopInfo {
                        exit: "run_error".to_owned(),
                        stop_reason: Value::Null,
                        progress: None,
                    },
                );
            }
            if let Some(path) = &diagnostics.stop_snapshot {
                value["stop_snapshot"] = write_stop_snapshot(
                    path,
                    &board,
                    json!({"exit": "run_error", "exit_detail": format!("{error:?}")}),
                    Value::Null,
                    inputs(),
                );
            }
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
    let exit = match report.exit {
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
    };
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
    if let Some(journal) = &mut diagnostics.journal {
        result["journal"] = journal.finish(
            &board,
            StopInfo {
                exit: exit.to_owned(),
                stop_reason: result["stop_reason"].clone(),
                progress: Some(QuantumProgress {
                    retired: report.retired,
                    quanta: report.quanta,
                }),
            },
        );
    }
    if let Some(path) = &diagnostics.stop_snapshot {
        // A machine `Shutdown` (ACPI S5) is the clean power-off; every other stop is described.
        result["stop_snapshot"] = if report.stop_reason == Some(MachineStopReason::Shutdown) {
            json!({"path": path, "written": false, "skipped": "stop reason Shutdown"})
        } else {
            let reason = json!({
                "exit": result["exit"],
                "exit_detail": result["exit_detail"],
                "unsupported_opcode": match &report.exit {
                    PagedProbeExit::Unsupported { opcode } => json!(format!("0x{opcode:02x}")),
                    _ => Value::Null,
                },
                "stop_reason": result["stop_reason"],
                "page_fault": result["page_fault"],
                "memory_error": result["memory_error"],
                "fw_stop": result["fw_stop"],
            });
            let counters = json!({
                "retired": report.retired,
                "quanta": report.quanta,
                "virtual_ns": report.virtual_ns,
                "tsc_hz": report.tsc_hz,
                "budget": budget,
            });
            write_stop_snapshot(path, &board, reason, counters, inputs())
        };
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
    let mut disks: Vec<&String> = Vec::new();
    let mut tpm = false;
    let mut sample_rips = Vec::new();
    let mut stop_snapshot = None;
    let mut phase_log = None;
    let mut framebuffer_dir = None;
    let mut framebuffer_every = None;
    let mut framebuffer_max_files = None;
    let mut framebuffer_max_bytes = None;
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
            "--disk" => disks.push(value),
            "--nvram-in" if nvram_in.is_none() => nvram_in = Some(value.clone()),
            "--nvram-out" if nvram_out.is_none() => nvram_out = Some(value.clone()),
            "--framebuffer-out" if framebuffer_out.is_none() => {
                framebuffer_out = Some(value.clone())
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
            "--stop-snapshot" if stop_snapshot.is_none() => stop_snapshot = Some(value.clone()),
            "--phase-log" if phase_log.is_none() => phase_log = Some(value.clone()),
            "--framebuffer-dir" if framebuffer_dir.is_none() => {
                framebuffer_dir = Some(value.clone())
            }
            "--framebuffer-every" if framebuffer_every.is_none() => {
                framebuffer_every = Some(parse_virtual_seconds(value)?);
            }
            "--framebuffer-max-files" if framebuffer_max_files.is_none() => {
                framebuffer_max_files = Some(parse_positive(name, value)?);
            }
            "--framebuffer-max-bytes" if framebuffer_max_bytes.is_none() => {
                framebuffer_max_bytes = Some(parse_positive(name, value)?);
            }
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
    let disk = parse_disks(&disks)?;
    if disk.is_some() && nvme_image.is_some() {
        return Err("--disk and --nvme-image are mutually exclusive".to_owned());
    }
    if let Some(app) = storage_app
        && nvme_image.is_none()
        && disk.is_none()
    {
        return Err(format!("--fv-app {app} requires --nvme-image or --disk"));
    }
    let image = image.ok_or_else(|| format!("--image is required; usage: {USAGE}"))?;
    let com1_out = com1_out.ok_or_else(|| format!("--com1-out is required; usage: {USAGE}"))?;
    let framebuffer = match (framebuffer_dir, framebuffer_every) {
        (Some(dir), Some(every_ns)) => Some(FramebufferDumps {
            dir,
            every_ns,
            max_files: framebuffer_max_files.unwrap_or(DEFAULT_MAX_FILES),
            max_bytes: framebuffer_max_bytes.unwrap_or(DEFAULT_MAX_BYTES),
        }),
        (None, None) if framebuffer_max_files.is_none() && framebuffer_max_bytes.is_none() => None,
        _ => {
            return Err(
                "--framebuffer-every and --framebuffer-dir go together, and the \
                 --framebuffer-max-* limits need them"
                    .to_owned(),
            );
        }
    };
    if (stop_snapshot.is_some() || phase_log.is_some() || framebuffer.is_some())
        && !sample_rips.is_empty()
    {
        return Err(
            "--sample-rip cannot be combined with --stop-snapshot, --phase-log or --framebuffer-*"
                .to_owned(),
        );
    }
    if nvram_reboot_out.is_some()
        && (stop_snapshot.is_some() || phase_log.is_some() || framebuffer.is_some())
    {
        return Err(
            "--stop-snapshot, --phase-log and --framebuffer-* describe one boot and cannot be \
             combined with --nvram-reboot-out (restarts are S8-23)"
                .to_owned(),
        );
    }
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
        disk,
        tpm,
        sample_rips,
        stop_snapshot,
        phase_log,
        framebuffer,
    })
}

/// Virtual seconds (decimals allowed) as nanoseconds; positive and finite.
fn parse_virtual_seconds(value: &str) -> Result<u64, String> {
    let seconds = value
        .parse::<f64>()
        .ok()
        .filter(|seconds| seconds.is_finite() && *seconds > 0.0)
        .ok_or_else(|| {
            format!("invalid --framebuffer-every '{value}': a positive number of seconds")
        })?;
    // At least 1 ns, so the period is never zero.
    Ok(((seconds * 1e9).round() as u64).max(1))
}

fn parse_positive(name: &str, value: &str) -> Result<u64, String> {
    value
        .parse::<u64>()
        .ok()
        .filter(|number| *number > 0)
        .ok_or_else(|| format!("invalid {name} '{value}': a positive integer"))
}

/// `--disk <path>[,ro]`, repeatable; the order is the order of NVMe
/// controllers. The board has one controller (S8-20 adds more), so exactly one
/// disk is accepted; a second one is refused rather than ignored.
fn parse_disks(values: &[&String]) -> Result<Option<String>, String> {
    let [value] = values else {
        return if values.is_empty() {
            Ok(None)
        } else {
            Err(format!(
                "{} --disk options given, but the board has one NVMe controller; \
                 several disks need the multi-controller board (S8-20)",
                values.len()
            ))
        };
    };
    let (path, read_only) = match value.strip_suffix(",ro") {
        Some(path) => (path, true),
        None => (value.as_str(), false),
    };
    if path.is_empty() {
        return Err("--disk needs a file path".to_owned());
    }
    if path.contains(',') {
        return Err(format!(
            "--disk '{value}': only the ',ro' option exists and a path must not contain ','"
        ));
    }
    if read_only {
        return Err(
            "--disk <path>,ro is not supported yet: the NVMe controller has no write-protected \
             namespace (docs/specs/NVME_DEVICE.md, 14.5 'Диск только для чтения (запрос S8-21; не реализован)')"
                .to_owned(),
        );
    }
    Ok(Some(path.to_owned()))
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

    /// Unique per-test directory (pid + nanoseconds) removed on drop, also
    /// after a panic; a leftover of an earlier run cannot make creation fail.
    struct TempDir(PathBuf);

    impl TempDir {
        fn new(tag: &str) -> Self {
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let path = std::env::temp_dir().join(format!("{tag}-{}-{nanos}", std::process::id()));
            fs::create_dir(&path).unwrap();
            Self(path)
        }
    }

    impl std::ops::Deref for TempDir {
        type Target = Path;
        fn deref(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn args(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| value.to_string()).collect()
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
        let dir = TempDir::new("od-nvram-reboot-paths");
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
    fn disks(values: &[&str]) -> Result<Option<String>, String> {
        let owned: Vec<String> = values.iter().map(|value| value.to_string()).collect();
        parse_disks(&owned.iter().collect::<Vec<_>>())
    }

    #[test]
    fn disk_option_accepts_one_writable_file_and_refuses_everything_else() {
        assert_eq!(disks(&[]), Ok(None));
        assert_eq!(
            disks(&["work/stage8/target.raw"]),
            Ok(Some("work/stage8/target.raw".to_owned()))
        );
        assert!(disks(&["a.raw", "b.raw"]).unwrap_err().contains("S8-20"));
        assert!(
            disks(&["a.raw,ro"])
                .unwrap_err()
                .contains("not supported yet")
        );
        assert!(disks(&[",ro"]).unwrap_err().contains("file path"));
        assert!(
            disks(&["a,b.raw"])
                .unwrap_err()
                .contains("must not contain")
        );
        assert!(
            disks(&["a.raw,rw"])
                .unwrap_err()
                .contains("must not contain")
        );
    }

    #[test]
    fn disk_and_nvme_image_are_exclusive_and_satisfy_storage_apps() {
        let required = ["--image", "rom.bin", "--com1-out", "c", "--budget", "1"];
        let parsed = |extra: &[&str]| parse(&args(&[&required[..], extra].concat()));
        let both = parsed(&["--nvme-image", "a.raw", "--disk", "b.raw"]);
        assert!(both.err().unwrap().contains("mutually exclusive"));
        let disk = parsed(&["--disk", "b.raw", "--fv-app", "blk"]).unwrap();
        assert_eq!(disk.disk.as_deref(), Some("b.raw"));
        assert_eq!(disk.nvme_image, None);
        assert!(matches!(
            disk.nvme_source(),
            Some(NvmeSource::Disk("b.raw"))
        ));
        let memory = parsed(&["--nvme-image", "a.raw"]).unwrap();
        assert!(matches!(
            memory.nvme_source(),
            Some(NvmeSource::Memory("a.raw"))
        ));
        let missing = parsed(&["--fv-app", "fs"]).err().unwrap();
        assert!(missing.contains("--fv-app fs requires --nvme-image"));
    }

    #[test]
    fn disk_must_not_alias_firmware_or_nvram_files() {
        let dir = TempDir::new("od-disk-paths");
        let (rom, nvram, disk) = (dir.join("rom.bin"), dir.join("vars.bin"), dir.join("d.raw"));
        for path in [&rom, &nvram, &disk] {
            fs::write(path, b"x").unwrap();
        }
        let mut options = parse(&args(&[
            "--image",
            rom.to_str().unwrap(),
            "--com1-out",
            dir.join("c").to_str().unwrap(),
            "--budget",
            "1",
            "--disk",
            disk.to_str().unwrap(),
        ]))
        .unwrap();
        assert!(validate_nvme_paths(disk.to_str().unwrap(), &options).is_ok());
        assert!(validate_nvme_paths(rom.to_str().unwrap(), &options).is_err());
        options.nvram_in = Some(nvram.to_str().unwrap().to_owned());
        assert!(validate_nvme_paths(nvram.to_str().unwrap(), &options).is_err());
        // The read-only in-memory input keeps its old rules: only outputs conflict.
        options.disk = None;
        options.nvme_image = Some(rom.to_str().unwrap().to_owned());
        assert!(validate_nvme_paths(rom.to_str().unwrap(), &options).is_ok());
    }

    /// Boots a halting synthetic ROM with the file attached through the same
    /// `boot` path as the CLI, then drives a real NVMe write by MMIO.
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    fn boot_with_disk(rom: &[u8], com1: &Path, disk: &str) -> Result<BootRun, (Value, u8)> {
        let config = FirmwareBoardConfig {
            nvme: Some(Default::default()),
            ..board_config(None, false, None)
        };
        boot(
            rom,
            &config,
            4,
            com1.to_str().unwrap(),
            Some(NvmeSource::Disk(disk)),
            &[],
            &mut Diagnostics::default(),
        )
    }

    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    fn halting_rom() -> Vec<u8> {
        const HANDOFF_MINOR_MAX: u16 = 12;
        const IMAGE_BYTES: usize = 0x40_0000;
        let mut image = vec![0u8; IMAGE_BYTES];
        image[..8].copy_from_slice(b"ODFWIMG1");
        image[0x08..0x0C].copy_from_slice(&1u32.to_le_bytes());
        image[0x0C..0x10].copy_from_slice(&0x40u32.to_le_bytes());
        image[0x10..0x18].copy_from_slice(&(IMAGE_BYTES as u64).to_le_bytes());
        image[0x18..0x20].copy_from_slice(&0x0020_0000u64.to_le_bytes());
        image[0x20..0x28].copy_from_slice(&0x40u64.to_le_bytes());
        image[0x28..0x2A].copy_from_slice(&1u16.to_le_bytes());
        // The NVMe handoff descriptor needs a ROM that declares its minor version.
        image[0x2a..0x2c].copy_from_slice(&HANDOFF_MINOR_MAX.to_le_bytes());
        image[0x40] = 0xf4; // hlt
        let mut crc = 0xFFFF_FFFFu32;
        for &byte in &image {
            crc ^= u32::from(byte);
            for _ in 0..8 {
                crc = if crc & 1 != 0 {
                    (crc >> 1) ^ 0xEDB8_8320
                } else {
                    crc >> 1
                };
            }
        }
        image[0x30..0x34].copy_from_slice(&(!crc).to_le_bytes());
        image
    }

    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    #[test]
    fn disk_backend_keeps_guest_writes_across_restart_and_locks_the_file() {
        let dir = TempDir::new("od-disk-restart");
        let (disk_path, com1) = (dir.join("disk.raw"), dir.join("c1"));
        let disk = disk_path.to_str().unwrap();
        let rom = halting_rom();
        // A sparse 1 MiB image: set_len only, no data written.
        let file = File::create(&disk_path).unwrap();
        file.set_len(1 << 20).unwrap();
        drop(file);

        let mut first = boot_with_disk(&rom, &com1, disk)
            .unwrap_or_else(|(value, _)| panic!("first boot: {value}"));
        // A second writer on the same file is refused while the first lives.
        let (busy, code) = boot_with_disk(&rom, &com1, disk)
            .err()
            .expect("second writer");
        assert_eq!(code, 3);
        assert_eq!(busy["error"]["kind"], "disk");
        assert!(busy["error"]["detail"].as_str().unwrap().contains("locked"));
        // The guest-visible write goes through the controller to the file.
        let nvme = first.board.nvme().unwrap();
        crate::s5_nvme_stand::drive_nvme(&mut first.board, &nvme).unwrap();
        drop(nvme);
        drop(first);

        // Restart: the lock is gone and the sector written before is in the file.
        let bytes = fs::read(&disk_path).unwrap();
        assert_eq!(bytes.len(), 1 << 20);
        assert_eq!(
            &bytes[..512],
            crate::s5_nvme_stand::write_pattern().as_slice()
        );
        assert!(bytes[512..].iter().all(|byte| *byte == 0));
        let again = boot_with_disk(&rom, &com1, disk).ok().expect("restart");
        drop(again);
        assert_eq!(fs::read(&disk_path).unwrap(), bytes);
        // The runner never creates the image.
        let missing = dir.join("missing.raw");
        let error = boot_with_disk(&rom, &com1, missing.to_str().unwrap())
            .err()
            .expect("missing image");
        assert_eq!(error.0["error"]["kind"], "disk");
        assert!(!missing.exists());
    }
}
