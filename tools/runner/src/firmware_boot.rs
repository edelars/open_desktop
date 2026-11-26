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
    NvramDevice, PagedProbeExit, PlatformResetKind, QuantumProgress, SecureBootKeys, TpmConfig,
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
                    [--fv-app hello|ebs|vars|rt|acpi|gop|blk|fs|tpm|reboot] [--tpm] [--xhci [--xhci-keys <list>]] [--nvram-reboot-out <file>] \
                    [--reboot-cycles <n> [--reboot-kind cold|warm]] \
                    [--nvram-in <file>] [--nvram-out <file>] \
                    [--acpi-out <file>] [--framebuffer-out <file>] [(--nvme-image <raw> | --disk <path>[,ro]) [--nvme-pci]] \
                    [--cpu-loop firmware|unified [--guest linux|windows]] [--ram-mib <n>] [--mark-rip <hex>] [--sample-rip <hex> ...] [--op-histogram-out <file>] [--no-turbo] [--turbo-stats] \
                    [--stop-snapshot <file>] [--phase-log <file>] \
                    [--framebuffer-every <virtual seconds> --framebuffer-dir <dir> \
                    [--framebuffer-max-files <n>] [--framebuffer-max-bytes <n>]]\n\
\x20\x20--disk <path>,ro: the file is opened read-only under a shared lock and the namespace is \
write protected: the guest sees NSATTR bit 0 and every Write is refused with Namespace is Write \
Protected (docs/specs/NVME_DEVICE.md 14.6); the file is never modified.\n\
\x20\x20--disk with --nvram-reboot-out: the second boot reopens the disk after the first \
board is dropped, so the writer lock (LOCK_EX) is released between the two boots \
(limitation until S8-23).\n\
\x20\x20--cpu-loop unified runs through the turbo engine (a template JIT) where the host has one; \
--no-turbo runs every instruction through the general interpreter step (the reference mode), and \
--turbo-stats prints the statistics of the engine to stderr at the end of the run.\n\
\x20\x20--nvme-pci (--cpu-loop unified, with --nvme-image or --disk): the NVMe controller is a PCI \
function (bus 0, slot 4, BAR0 0xFC004000, INTx to GSI 16) instead of the firmware's fixed window; \
the firmware finds it through ECAM and the report records nvme_pci: true.";
// Match the milestones in the given order. Repeated RIPs consume separate
// visits, and later milestones remain pending until the next expected RIP.
const MAX_RIP_SAMPLES: usize = 64;
const MAX_TRACE_CALLS: usize = 64;
const DUMP_USAGE: &str =
    "dump-handoff-block --out <file> [--fv-app hello|ebs|vars|rt|acpi|gop|blk|fs|tpm|reboot]";
const PREPARE_NVRAM_USAGE: &str =
    "prepare-secure-boot-nvram setup|defaults --out <new file> [--nvram-in <protected snapshot>]";

struct Options {
    image: String,
    /// S7-32: `--guest linux|windows` (unified loop only): the CPUID profile of the run.
    guest: od_machine::UnifiedGuest,
    /// S7-32: `--ram-mib <n>`: the guest RAM (a multiple of 2 MiB, 128 MiB by default).
    ram_mib: Option<u64>,
    /// S7-32: `--nvme-pci` (unified loop with an NVMe source): the controller is a PCI function
    /// (slot 4, BAR0 0xFC004000) instead of the firmware's fixed window.
    nvme_pci: bool,
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
    /// durable flush after each write), or read-only with `disk_read_only`.
    /// Mutually exclusive with `nvme_image`.
    disk: Option<String>,
    /// S8-21: `--disk <path>,ro`: shared read lease and a write-protected namespace.
    disk_read_only: bool,
    tpm: bool,
    /// S7-34: the board gets the PCI ECAM window and the xHCI controller with the keyboard on
    /// port 1 and the absolute pointer on port 2 (`--xhci`; XHCI_DEVICE.md).
    xhci: bool,
    /// S7-34 slice C: HID key events (Usage ID, pressed) fed to the keyboard behind the xHCI
    /// controller while the ROM runs (`--xhci-keys`, see [`parse_xhci_keys`]).
    xhci_keys: Vec<(u8, bool)>,
    /// S7-26: boot, reset in process and boot again this many times (`--fv-app reboot`).
    reboot_cycles: Option<u64>,
    reboot_kind: PlatformResetKind,
    sample_rips: Vec<u64>,
    /// S8-10: JSON snapshot of the stopped machine (any stop except machine `Shutdown`).
    stop_snapshot: Option<String>,
    /// S8-11: JSON lines, one per phase event.
    phase_log: Option<String>,
    /// S8-11: periodic raw screen dumps of any boot.
    framebuffer: Option<FramebufferDumps>,
    /// S7-31c: `--cpu-loop unified` runs the ROM and the guest it starts in the CPL0 profile set
    /// loop on a board with the Linux profile (ADR-0013 variant (b)).
    unified: bool,
    /// S7-31d: `--mark-rip` (unified loop only): record the first boundary at this RIP.
    mark_rip: Option<u64>,
    /// S7-31q: `--trace-call NAME=0xADDR[:guid=N][:name=N]` (unified loop only): watch the entry of
    /// a ROM function (physical address); N (0..=3) is the argument that points to a GUID or to a
    /// UCS-2 name. The report lists each call with its GUID and name, never values.
    trace_calls: Vec<od_machine::CallTraceTarget>,
    /// S7-31q: `--trace-system-table <hex>`: the guest address of `EFI_SYSTEM_TABLE`, for the `slot=` targets.
    trace_system_table: Option<u64>,
    /// PERF-20: `--op-histogram-out <file>` (unified loop only): the histogram of the retired
    /// operations (operation, operand kind, operand size, CPL) as JSON.
    op_histogram_out: Option<String>,
    /// `--no-turbo` (unified loop only): the loop runs the general interpreter step only (the
    /// reference mode); by default it runs through the turbo engine where the host has one.
    no_turbo: bool,
    /// `--turbo-stats` (unified loop only): the statistics of the turbo engine go to stderr at the
    /// end of the run.
    turbo_stats: bool,
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
    Disk { path: &'a str, read_only: bool },
}

impl Options {
    fn nvme_path(&self) -> Option<&str> {
        self.disk.as_deref().or(self.nvme_image.as_deref())
    }

    fn nvme_source(&self) -> Option<NvmeSource<'_>> {
        match (&self.disk, &self.nvme_image) {
            (Some(path), _) => Some(NvmeSource::Disk {
                path,
                read_only: self.disk_read_only,
            }),
            (None, Some(path)) => Some(NvmeSource::Memory(path)),
            (None, None) => None,
        }
    }
}

fn parse_trace_call(value: &str) -> Result<od_machine::CallTraceTarget, String> {
    let invalid = || {
        format!(
            "invalid --trace-call '{value}'; expected NAME=0xADDR[:guid=N][:name=N][:slot=OFFSET]"
        )
    };
    let (name, rest) = value.split_once('=').ok_or_else(invalid)?;
    let mut parts = rest.split(':');
    let address = parts.next().ok_or_else(invalid)?;
    let physical = u64::from_str_radix(address.strip_prefix("0x").unwrap_or(address), 16)
        .map_err(|_| invalid())?;
    let (mut guid_arg, mut name_arg, mut rt_slot) = (None, None, None);
    for part in parts {
        let (key, number) = part.split_once('=').ok_or_else(invalid)?;
        match key {
            "guid" | "name" => {
                let index: u8 = number
                    .parse()
                    .ok()
                    .filter(|index| *index < 4)
                    .ok_or_else(invalid)?;
                let slot = if key == "guid" {
                    &mut guid_arg
                } else {
                    &mut name_arg
                };
                if slot.replace(index).is_some() {
                    return Err(invalid());
                }
            }
            "slot" if rt_slot.is_none() => {
                rt_slot = Some(number.parse::<u16>().map_err(|_| invalid())?)
            }
            _ => return Err(invalid()),
        }
    }
    if name.is_empty() || (physical == 0 && rt_slot.is_none()) {
        return Err(invalid());
    }
    Ok(od_machine::CallTraceTarget {
        name: name.to_owned(),
        physical,
        guid_arg,
        name_arg,
        rt_slot,
    })
}

fn tpm_config(snapshot: Option<Vec<u8>>) -> io::Result<TpmConfig> {
    let mut config = TpmConfig {
        entropy: vec![0; 48],
        snapshot,
    };
    File::open("/dev/urandom")?.read_exact(&mut config.entropy)?;
    Ok(config)
}

/// The explicit UTC base of the board RTC: 2024-01-01T00:00:00Z.
const RTC_BASE_UNIX_SECONDS: u64 = 1_704_067_200;

/// The board the unified loop runs the Linux profile on.
fn apply_unified(config: &mut FirmwareBoardConfig) {
    // The Linux profile needs the PM block, the ACPI tables and the ECAM window.
    config.linux_profile = true;
    config.acpi_pm = true;
    config.acpi_tables = true;
    config.pci_ecam = true;
    // Without an RTC base the CMOS ports are absent (reads 0xFF, so UIP is always set) and the
    // kernel's mc146818_get_time times out: "Unable to read current time from RTC" (S7-31o,
    // ISSUE-641). The same explicit UTC base as the fv_app boards; never a host date.
    config
        .rtc_base_unix_seconds
        .get_or_insert(RTC_BASE_UNIX_SECONDS);
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
        rtc_base_unix_seconds: (rt || acpi).then_some(RTC_BASE_UNIX_SECONDS),
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
        apply_unified(&mut config);
    }
    if let Some(mib) = options.ram_mib {
        // The board validates the range and the 2 MiB multiple (`validate_config`); only the
        // conversion is checked here.
        let Some(bytes) = mib.checked_mul(1 << 20) else {
            return emit(
                failure(
                    "invalid_input",
                    "arguments",
                    format!("--ram-mib {mib} is too large"),
                ),
                3,
            );
        };
        config.ram_bytes = bytes;
    }
    if options.nvme_path().is_some() {
        config.nvme = Some(Default::default());
        config.nvme_pci = options.nvme_pci;
    }
    if options.xhci {
        config.pci_ecam = true;
        config.xhci = Some(Default::default());
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
    if let Some(cycles) = options.reboot_cycles {
        return run_reboot_cycles(&options, &image, &config, cycles);
    }
    // The journal creates its files (the phase log and the dump directory), so only its
    // configuration is built here; `boot` creates it once the image is loaded and the NVMe
    // controller is attached. A run refused before that point (bad arguments or paths, an
    // unreadable image or NVRAM snapshot, a loader or NVMe refusal) leaves neither file behind.
    let mut diagnostics = Diagnostics {
        stop_snapshot: options.stop_snapshot.clone(),
        op_histogram_out: options.op_histogram_out.clone(),
        no_turbo: options.no_turbo,
        turbo_stats: options.turbo_stats,
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
        options.unified.then_some(options.guest),
        options.mark_rip,
        &options.trace_calls,
        options.trace_system_table,
        &options.xhci_keys,
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
        // The unified loop has no FV application: the frame is whatever the guest left (S7-31p).
        if !options.unified && !first.finished_app(b"OD-FW image gop: EFI_SUCCESS\r\n") {
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
        None,
        None,
        &[],
        None,
        &[],
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
        // A disk can be written by the guest (and a read-only one is not a
        // reason to share a file with the ROM or NVRAM), so it must not be any
        // other file the runner reads or writes (the in-memory `--nvme-image`
        // is only read).
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
        ("--op-histogram-out", options.op_histogram_out.as_deref()),
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
        new[2],
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
        if !matches!(
            *name,
            "--stop-snapshot" | "--phase-log" | "--op-histogram-out"
        ) {
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
            if matches!(
                *name,
                "--stop-snapshot" | "--phase-log" | "--op-histogram-out"
            ) && path.absolute.starts_with(&dir.absolute)
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
    /// PERF-20: where the unified loop writes the histogram of retired operations.
    op_histogram_out: Option<String>,
    /// The unified loop runs the general interpreter step only (`--no-turbo`).
    no_turbo: bool,
    /// The statistics of the turbo engine go to stderr after the unified run (`--turbo-stats`).
    turbo_stats: bool,
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

/// With `read_only` (`--disk <path>,ro`) the file is opened read-only under a
/// shared lock (`LOCK_SH`) and the namespace is write protected (NVME_DEVICE.md
/// 14.6): a writer is refused while it lives and the file is never modified.
///
/// Otherwise attaches an existing RAW file through the stage 5 persistent backend: the
/// writer lock (`LOCK_EX`) is held until the backend is dropped, so a second
/// runner (or a second boot in this process while the first board is alive)
/// cannot write the same image, and every guest write is flushed durably
/// before it completes. The file is neither created, resized nor copied into
/// memory.
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
fn attach_disk(
    nvme: &od_machine::NvmeHandle,
    path: &str,
    read_only: bool,
) -> Result<(), (Value, u8)> {
    use od_host::persistent_nvme::{
        AttachPersistentNvmeError, PersistentNvmeOpenError, attach_persistent_nvme,
        attach_read_only_nvme,
    };
    use od_host::storage_io::{LeaseOpenError, WriterOpenError};
    let attached = if read_only {
        attach_read_only_nvme(nvme, Path::new(path))
    } else {
        attach_persistent_nvme(nvme, Path::new(path))
    };
    attached.map_err(|error| match error {
        AttachPersistentNvmeError::Open(PersistentNvmeOpenError::WriterOpen(
            WriterOpenError::WriterBusy,
        )) => (
            failure(
                "invalid_input",
                "disk",
                format!("{path}: the disk image is locked by another writer or by a read-only run"),
            ),
            3,
        ),
        AttachPersistentNvmeError::Open(PersistentNvmeOpenError::ReadLease(
            LeaseOpenError::LeaseBusy,
        )) => (
            failure(
                "invalid_input",
                "disk",
                format!("{path}: the disk image is locked by a writer"),
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
fn attach_disk(
    _nvme: &od_machine::NvmeHandle,
    _path: &str,
    _read_only: bool,
) -> Result<(), (Value, u8)> {
    Err((
        failure(
            "invalid_input",
            "disk",
            "--disk needs the macOS arm64 host backend (od-host)".to_owned(),
        ),
        3,
    ))
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

/// First injected key event: after the keyboard is enumerated (the port reset alone takes 50 ms of
/// virtual time, 50e6 instructions) and well before an application waits for input.
const XHCI_KEYS_START: u64 = 60_000_000;
/// Virtual time between two injected events, in instructions (1 ns each): 2 ms.
const XHCI_KEYS_SPACING: u64 = 2_000_000;

/// `--xhci-keys`: comma-separated HID Usage IDs (hex, page 0x07, 0xE0..=0xE7 modifiers). `HH` is a
/// press followed by a release, `HH+` a press only, `HH-` a release only. Example: `e1+,04,e1-`
/// types a capital A with the left Shift key.
fn parse_xhci_keys(value: &str) -> Result<Vec<(u8, bool)>, String> {
    let mut keys = Vec::new();
    for item in value.split(',') {
        let (digits, action) = match item.as_bytes().last() {
            Some(b'+') => (&item[..item.len() - 1], Some(true)),
            Some(b'-') => (&item[..item.len() - 1], Some(false)),
            _ => (item, None),
        };
        let usage = u8::from_str_radix(digits, 16)
            .ok()
            .filter(|usage| matches!(usage, 0x04..=0xdd | 0xe0..=0xe7) && !digits.is_empty())
            .ok_or_else(|| {
                format!("invalid --xhci-keys item '{item}'; expected a hex Usage ID 04..DD or E0..E7 with optional + or -")
            })?;
        match action {
            Some(pressed) => keys.push((usage, pressed)),
            None => keys.extend([(usage, true), (usage, false)]),
        }
    }
    Ok(keys)
}

/// Feeds the `--xhci-keys` events and watches the controller while the run is observed: whether
/// it ran and, at the first quantum boundary after it stopped (the firmware stops it at
/// ExitBootServices, XHCI_DEVICE.md 12.3), how many guest RAM accesses it had made.
struct XhciWatch<'a> {
    keys: &'a [(u8, bool)],
    injected: usize,
    refused: usize,
    last_quanta: u64,
    seen_running: bool,
    ram_accesses_at_stop: Option<u64>,
}

impl<'a> XhciWatch<'a> {
    fn new(keys: &'a [(u8, bool)]) -> Self {
        Self {
            keys,
            injected: 0,
            refused: 0,
            last_quanta: u64::MAX,
            seen_running: false,
            ram_accesses_at_stop: None,
        }
    }

    fn step(&mut self, handle: &od_machine::XhciHandle, retired: u64, quanta: u64) {
        if let Some(&(usage, pressed)) = self.keys.get(self.injected)
            && retired >= XHCI_KEYS_START + self.injected as u64 * XHCI_KEYS_SPACING
        {
            if handle
                .push_input(od_machine::InputEvent::Key { usage, pressed })
                .is_err()
            {
                self.refused += 1;
            }
            self.injected += 1;
        }
        if quanta == self.last_quanta {
            return;
        }
        self.last_quanta = quanta;
        let snapshot = handle.snapshot();
        if snapshot.usbcmd & 1 != 0 {
            self.seen_running = true;
        } else if self.seen_running && self.ram_accesses_at_stop.is_none() {
            self.ram_accesses_at_stop = Some(snapshot.ram_accesses);
        }
    }

    fn report(&self, last: &od_machine::XhciSnapshot) -> Value {
        json!({
            "keys_total": self.keys.len(),
            "keys_injected": self.injected,
            "keys_refused": self.refused,
            "ran": self.seen_running,
            "ram_accesses_at_stop": self.ram_accesses_at_stop,
            "final": {
                "usbcmd": format!("0x{:08x}", last.usbcmd),
                "usbsts": format!("0x{:08x}", last.usbsts),
                "crcr": format!("0x{:016x}", last.crcr),
                "dcbaap": format!("0x{:016x}", last.dcbaap),
                "iman": format!("0x{:08x}", last.iman),
                "erstba": format!("0x{:016x}", last.erstba),
                "pci_command": format!("0x{:04x}", last.pci_command),
                "bar0": format!("0x{:016x}", last.bar0),
                "ram_accesses": last.ram_accesses,
            },
        })
    }
}

fn boot(
    image: &[u8],
    config: &FirmwareBoardConfig,
    budget: u64,
    com1_out: &str,
    nvme: Option<NvmeSource<'_>>,
    sample_rips: &[u64],
    diagnostics: &mut Diagnostics,
    unified: Option<od_machine::UnifiedGuest>,
    mark_rip: Option<u64>,
    trace_calls: &[od_machine::CallTraceTarget],
    trace_system_table: Option<u64>,
    xhci_keys: &[(u8, bool)],
) -> Result<BootRun, (Value, u8)> {
    let mut board = load_firmware_image(image, config)
        .map_err(|error| (failure("loader_error", "loader", format!("{error:?}")), 4))?;
    if let Some(NvmeSource::Disk { path, read_only }) = nvme {
        attach_disk(
            &board.nvme().expect("runner enabled the NVMe controller"),
            path,
            read_only,
        )?;
    }
    // Hashes of the inputs the stop snapshot records (only computed when it is requested).
    let snapshot_wanted = diagnostics.stop_snapshot.is_some();
    let mut nvme_input = Value::Null;
    if let Some(NvmeSource::Disk { path, read_only }) = nvme
        && snapshot_wanted
    {
        // A writable disk is written by the guest (a read-only one cannot be) and can be tens of
        // GiB: path, read_only and size, no hash.
        nvme_input = json!({
            "kind": "disk", "path": path, "read_only": read_only,
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
    let xhci = board.xhci();
    let mut watch = XhciWatch::new(xhci_keys);
    let mut unified_run = None;
    let mut op_histogram_result: Option<Result<Value, String>> = None;
    let op_histogram_path = diagnostics.op_histogram_out.clone();
    let run = if let Some(guest) = unified {
        // The unified loop runs through the turbo engine (ADR-0022) unless `--no-turbo` asks for
        // the general step only: the runner owns the process's executable region and gives it to
        // the board. Without a region (another host, the region taken) the loop runs the general
        // step only and the result is the same.
        if diagnostics.no_turbo {
            board.disable_turbo();
        } else {
            match od_host::JitArena::new(od_machine::TURBO_ARENA_BYTES) {
                Ok(arena) => {
                    if !board.enable_turbo(Box::new(arena)) {
                        eprintln!("turbo: not available on this host; the general step runs alone");
                    }
                }
                Err(error) => eprintln!(
                    "turbo: no executable region ({error:?}); the general step runs alone"
                ),
            }
        }
        // S7-31c: the ROM and the guest in the CPL0 profile set loop (ADR-0013 variant (b)).
        let unified_run = od_machine::run_firmware_unified_guest(
            &mut board,
            budget,
            mark_rip,
            trace_calls.to_vec(),
            trace_system_table,
            op_histogram_path.is_some(),
            guest,
        ).map(|run| {
            let report = run.run;
            if let (Some(histogram), Some(path)) = (&run.op_histogram, &op_histogram_path) {
                op_histogram_result = Some(write_op_histogram(path, histogram, report.retired));
            }
            unified_run = Some(json!({
                "set_exit_detail": run.set_exit_detail,
                "members": run.members,
                "exceptions_delivered": run.exceptions_delivered,
                "page_faults_delivered": run.page_faults_delivered,
                "cr3_changes": run.cr3_changes,
                "cpl3_retired": run.cpl3_retired,
                "cpuid_divergence_leaf": run.cpuid_divergence_leaf,
                "rip_physical": run.rip_physical.map(|address| format!("0x{address:x}")),
                "rip_fetch_error": run.rip_fetch_error,
                "final_state": {
                    "rflags": format!("0x{:x}", run.final_state.rflags),
                    "rax": format!("0x{:x}", run.final_state.rax),
                    "rsp": format!("0x{:x}", run.final_state.rsp),
                    "gdtr": run.final_state.gdtr.map(|(base, limit)| json!({
                        "base": format!("0x{base:x}"),
                        "limit": format!("0x{limit:x}"),
                    })),
                    "cs_selector": run.final_state.cs_selector.map(|selector| format!("0x{selector:x}")),
                    "ss_selector": run.final_state.ss_selector.map(|selector| format!("0x{selector:x}")),
                    "ss_cache": run.final_state.ss_cache,
                    "sti_shadow": run.final_state.sti_shadow,
                },
                "interrupts": {
                    "interrupt_flag": run.interrupts.interrupt_flag,
                    "activity": run.interrupts.activity,
                    "source": run.interrupts.source,
                    "pic_imr": run.interrupts.pic_imr.map(|value| format!("0x{value:02x}")),
                    "pic_irr": run.interrupts.pic_irr.map(|value| format!("0x{value:02x}")),
                    "pic_isr": run.interrupts.pic_isr.map(|value| format!("0x{value:02x}")),
                    "pic_icw2": run.interrupts.pic_icw2.map(|value| format!("0x{value:02x}")),
                    "pic": run.interrupts.pic,
                    "pit_channel0": run.interrupts.pit_channel0,
                    "pit_channel0_programmed": run.interrupts.pit_channel0_programmed,
                    "halt_stop": run.interrupts.halt_stop,
                    "apic_declared": run.interrupts.apic_declared,
                    "irqs_delivered": run.interrupts.irqs_delivered,
                    "irq0_edges": run.interrupts.irq0_edges,
                    "irq_refused_masked": run.interrupts.irq_refused_masked,
                    "irq_refused_shadow": run.interrupts.irq_refused_shadow,
                    "halt_wakes": run.interrupts.halt_wakes,
                    "halt_idle_ns": run.interrupts.halt_idle_ns,
                },
                "rip_walk": run.rip_walk.iter().map(|entry| json!({
                    "level": entry.level,
                    "entry_gpa": format!("0x{:x}", entry.entry_gpa),
                    "raw": format!("0x{:016x}", entry.raw),
                })).collect::<Vec<_>>(),
                "first_page_fault": run.first_page_fault.as_ref().map(|fault| json!({
                    "cr2": format!("0x{:x}", fault.cr2),
                    "error_code": format!("0x{:x}", fault.error_code),
                    "translation_error": fault.translation_error,
                    "walk": fault.walk.iter().map(|entry| json!({
                        "level": entry.level,
                        "entry_gpa": format!("0x{:x}", entry.entry_gpa),
                        "raw": format!("0x{:016x}", entry.raw),
                    })).collect::<Vec<_>>(),
                })),
                "marker_rip": run.marker_rip.map(|rip| format!("0x{rip:x}")),
                "checkpoints": run.checkpoints.iter().map(|record| json!({
                    "id": record.id,
                    "name": record.name,
                    "kind": record.kind,
                    "rip": format!("0x{:x}", record.rip),
                    "retired": record.retired,
                    "quanta": record.quanta,
                    "virtual_ns": record.virtual_ns,
                    "cpl": record.cpl,
                })).collect::<Vec<_>>(),
                "marker": run.marker.map(|marker| json!({
                    "retired": marker.retired,
                    "quanta": marker.quanta,
                    "virtual_ns": marker.virtual_ns,
                })),
                "rip_bytes": run.rip_bytes.iter().map(|byte| format!("{byte:02x}")).collect::<String>(),
                "call_trace": run.call_trace.as_ref().map(|trace| json!({
                    "total": trace.total,
                    "dropped": trace.total - trace.entries.len() as u64,
                    "calls": trace.entries.iter().map(|call| json!({
                        "name": call.name,
                        "retired": call.retired,
                        "cpl": call.cpl,
                        "caller": call.caller.map(|address| format!("0x{address:x}")),
                        "guid": call.guid.map(|bytes| bytes.iter().map(|byte| format!("{byte:02x}")).collect::<String>()),
                        "variable_name": call.variable_name,
                    })).collect::<Vec<_>>(),
                })),
            }));
            report
        });
        if diagnostics.turbo_stats {
            match board.turbo_report() {
                Some(report) => eprintln!("turbo: {report}"),
                None => eprintln!("turbo: no engine (--no-turbo, or a host without one)"),
            }
        }
        unified_run
    } else if let Some(journal) = &mut diagnostics.journal {
        board.run_boot_with_quantum_hook(budget, |board, progress| {
            journal.at_boundary(board, progress)
        })
    } else if sample_rips.is_empty() && xhci.is_none() {
        board.run_boot(budget)
    } else {
        let start = Instant::now();
        board.run_boot_observed(budget, |rip, retired, quanta| {
            if sample_rips.get(samples.len()) == Some(&rip) {
                samples.push((rip, start.elapsed().as_nanos(), retired, quanta));
            }
            if let Some(handle) = &xhci {
                watch.step(handle, retired, quanta);
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
    if let Some(handle) = &xhci {
        result["xhci"] = watch.report(&handle.snapshot());
    }
    if let Some(unified_run) = unified_run {
        result["cpu_loop"] = json!("unified");
        result["unified"] = unified_run;
        result["guest"] = json!(match unified.expect("a unified run has a guest") {
            od_machine::UnifiedGuest::Linux => "linux",
            od_machine::UnifiedGuest::Windows => "windows",
        });
    }
    result["ram_bytes"] = json!(config.ram_bytes);
    if config.nvme_pci {
        result["nvme_pci"] = json!(true);
    }
    match op_histogram_result {
        Some(Ok(summary)) => result["op_histogram"] = summary,
        Some(Err(error)) => return Err((failure("run_error", "op_histogram", error), 5)),
        None => {}
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
    let mut xhci = false;
    let mut xhci_keys: Option<Vec<(u8, bool)>> = None;
    let mut reboot_cycles = None;
    let mut reboot_kind = PlatformResetKind::Cold;
    let mut sample_rips = Vec::new();
    let mut stop_snapshot = None;
    let mut phase_log = None;
    let mut framebuffer_dir = None;
    let mut framebuffer_every = None;
    let mut framebuffer_max_files = None;
    let mut framebuffer_max_bytes = None;
    let mut cpu_loop: Option<bool> = None;
    let mut mark_rip: Option<u64> = None;
    let mut trace_calls: Vec<od_machine::CallTraceTarget> = Vec::new();
    let mut trace_system_table: Option<u64> = None;
    let mut op_histogram_out = None;
    let mut no_turbo = false;
    let mut turbo_stats = false;
    let mut guest: Option<od_machine::UnifiedGuest> = None;
    let mut ram_mib: Option<u64> = None;
    let mut nvme_pci = false;
    let mut index = 0;
    while index < args.len() {
        let name = args[index].as_str();
        if name == "--tpm" && !tpm {
            tpm = true;
            index += 1;
            continue;
        }
        if name == "--nvme-pci" && !nvme_pci {
            nvme_pci = true;
            index += 1;
            continue;
        }
        if name == "--xhci" && !xhci {
            xhci = true;
            index += 1;
            continue;
        }
        if name == "--no-turbo" && !no_turbo {
            no_turbo = true;
            index += 1;
            continue;
        }
        if name == "--turbo-stats" && !turbo_stats {
            turbo_stats = true;
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
            "--xhci-keys" if xhci_keys.is_none() => xhci_keys = Some(parse_xhci_keys(value)?),
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
            "--guest" if guest.is_none() => {
                guest = Some(match value.as_str() {
                    "linux" => od_machine::UnifiedGuest::Linux,
                    "windows" => od_machine::UnifiedGuest::Windows,
                    _ => {
                        return Err(format!(
                            "invalid --guest '{value}'; expected linux or windows"
                        ));
                    }
                });
            }
            "--ram-mib" if ram_mib.is_none() => {
                ram_mib = Some(
                    value
                        .parse::<u64>()
                        .ok()
                        .filter(|mib| *mib > 0 && mib % 2 == 0)
                        .ok_or_else(|| {
                            format!("invalid --ram-mib '{value}'; expected an even number of MiB")
                        })?,
                );
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
            "--cpu-loop" if cpu_loop.is_none() => match value.as_str() {
                "unified" => cpu_loop = Some(true),
                "firmware" => cpu_loop = Some(false),
                _ => {
                    return Err(format!(
                        "invalid --cpu-loop '{value}'; expected firmware or unified"
                    ));
                }
            },
            "--trace-system-table" if trace_system_table.is_none() => {
                let hex = value.strip_prefix("0x").unwrap_or(value);
                trace_system_table = Some(
                    u64::from_str_radix(hex, 16)
                        .map_err(|_| format!("invalid --trace-system-table '{value}'"))?,
                );
            }
            "--op-histogram-out" if op_histogram_out.is_none() => {
                op_histogram_out = Some(value.clone())
            }
            "--trace-call" if trace_calls.len() < MAX_TRACE_CALLS => {
                trace_calls.push(parse_trace_call(value)?)
            }
            "--mark-rip" if mark_rip.is_none() => {
                let hex = value.strip_prefix("0x").unwrap_or(value);
                mark_rip = Some(
                    u64::from_str_radix(hex, 16)
                        .map_err(|_| format!("invalid --mark-rip '{value}'"))?,
                );
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
    let unified = cpu_loop == Some(true);
    if xhci_keys.is_some() && !xhci {
        return Err("--xhci-keys requires --xhci".to_owned());
    }
    if unified && xhci {
        return Err(
            "--xhci is served by the firmware profile loop, not --cpu-loop unified".to_owned(),
        );
    }
    if unified {
        // The unified loop has no per-instruction RIP observer, and the reboot paths run the
        // firmware profile loop; refuse rather than run another loop or ignore the option.
        if !sample_rips.is_empty() {
            return Err("--cpu-loop unified does not support --sample-rip".to_owned());
        }
        if reboot_cycles.is_some() || nvram_reboot_out.is_some() {
            return Err(
                "--cpu-loop unified does not combine with --reboot-cycles or --nvram-reboot-out"
                    .to_owned(),
            );
        }
    }
    if mark_rip.is_some() && !unified {
        return Err("--mark-rip requires --cpu-loop unified".to_owned());
    }
    if !trace_calls.is_empty() && !unified {
        return Err("--trace-call requires --cpu-loop unified".to_owned());
    }
    if op_histogram_out.is_some() && !unified {
        return Err("--op-histogram-out requires --cpu-loop unified".to_owned());
    }
    if no_turbo && !unified {
        return Err("--no-turbo requires --cpu-loop unified".to_owned());
    }
    if turbo_stats && !unified {
        return Err("--turbo-stats requires --cpu-loop unified".to_owned());
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
    if framebuffer_out.is_some() && fv_app != Some(FirmwareFvApp::Gop) && !unified {
        return Err("--framebuffer-out requires --fv-app gop or --cpu-loop unified".to_owned());
    }
    let storage_app = match fv_app {
        Some(FirmwareFvApp::Blk) => Some("blk"),
        Some(FirmwareFvApp::Fs) => Some("fs"),
        _ => None,
    };
    let (disk, disk_read_only) = match parse_disks(&disks)? {
        Some((path, read_only)) => (Some(path), read_only),
        None => (None, false),
    };
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
    // The run journal (`--phase-log`, `--framebuffer-dir`/`-every`) is driven only by the firmware profile loop with
    // the quantum hook. The unified loop and the xHCI key watch (both served before the journal branch in `boot`) and
    // the reboot cycles (which return before the diagnostics are built) never feed it: refuse rather than
    // silently write nothing. `--stop-snapshot` is written after any single boot, so only the reboot cycles lose it.
    let journal_requested = phase_log.is_some() || framebuffer.is_some();
    if unified && journal_requested {
        return Err(
            "--cpu-loop unified does not support --phase-log or --framebuffer-dir/--framebuffer-every \
             (the unified loop has no run journal); use --cpu-loop firmware"
                .to_owned(),
        );
    }
    if xhci_keys.is_some() && journal_requested {
        return Err(
            "--xhci-keys cannot be combined with --phase-log or --framebuffer-* (the journal loop \
             does not feed the xHCI key watch)"
                .to_owned(),
        );
    }
    if reboot_cycles.is_some() && (stop_snapshot.is_some() || journal_requested) {
        return Err(
            "--reboot-cycles cannot be combined with --stop-snapshot, --phase-log or --framebuffer-* \
             (the cycles run no diagnostics)"
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
    if nvme_pci && !(unified && (disk.is_some() || nvme_image.is_some())) {
        return Err("--nvme-pci requires --cpu-loop unified and --nvme-image or --disk".to_owned());
    }
    if guest.is_some() && !unified {
        return Err("--guest requires --cpu-loop unified".to_owned());
    }
    Ok(Options {
        image,
        guest: guest.unwrap_or_default(),
        ram_mib,
        nvme_pci,
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
        disk_read_only,
        tpm,
        xhci,
        xhci_keys: xhci_keys.unwrap_or_default(),
        reboot_cycles,
        reboot_kind,
        sample_rips,
        stop_snapshot,
        phase_log,
        framebuffer,
        unified,
        mark_rip,
        trace_calls,
        trace_system_table,
        op_histogram_out,
        no_turbo,
        turbo_stats,
    })
}

/// PERF-20: writes the histogram of retired operations as JSON and returns its summary for the
/// run report. The keys come by descending count with their share and the running share; the
/// totals by CPL are given apart; `sum_equals_retired` says whether the counted instructions are
/// those the run retired. A count of executions, not a profile of time.
fn write_op_histogram(
    path: &str,
    histogram: &od_machine::OpHistogramReport,
    retired: u64,
) -> Result<Value, String> {
    let share = |count: u64| {
        if histogram.total == 0 {
            0.0
        } else {
            count as f64 / histogram.total as f64
        }
    };
    let mut running = 0u64;
    let entries = histogram
        .entries
        .iter()
        .map(|entry| {
            running += entry.count;
            json!({
                "key": format!(
                    "{}/{}/{}/cpl{}",
                    entry.operation, entry.operands, entry.operand_bytes, entry.cpl
                ),
                "operation": entry.operation,
                "operands": entry.operands,
                "operand_bytes": entry.operand_bytes,
                "cpl": entry.cpl,
                "count": entry.count,
                "share": share(entry.count),
                "cumulative_share": share(running),
            })
        })
        .collect::<Vec<_>>();
    let by_cpl =
        |cpl: usize| json!({"count": histogram.by_cpl[cpl], "share": share(histogram.by_cpl[cpl])});
    let sum_equals_retired = histogram.total == retired;
    let document = json!({
        "retired": retired,
        "total": histogram.total,
        "sum_equals_retired": sum_equals_retired,
        "keys": histogram.entries.len(),
        "by_cpl": {"cpl0": by_cpl(0), "cpl1": by_cpl(1), "cpl2": by_cpl(2), "cpl3": by_cpl(3)},
        "entries": entries,
    });
    let bytes = serde_json::to_vec_pretty(&document).map_err(|error| error.to_string())?;
    fs::write(path, bytes).map_err(|error| format!("{path}: {error}"))?;
    Ok(json!({
        "path": path,
        "total": histogram.total,
        "retired": retired,
        "sum_equals_retired": sum_equals_retired,
        "keys": histogram.entries.len(),
        "cpl0": histogram.by_cpl[0],
        "cpl3": histogram.by_cpl[3],
    }))
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
fn parse_disks(values: &[&String]) -> Result<Option<(String, bool)>, String> {
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
    Ok(Some((path.to_owned(), read_only)))
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
    fn xhci_keys_expand_to_press_release_pairs_and_explicit_edges() {
        assert_eq!(
            parse_xhci_keys("e1+,04,e1-").unwrap(),
            [(0xe1, true), (0x04, true), (0x04, false), (0xe1, false)]
        );
        assert_eq!(
            parse_xhci_keys("28").unwrap(),
            [(0x28, true), (0x28, false)]
        );
        for bad in ["", "04,", "zz", "03", "de", "e8", "+", "04++", "104"] {
            assert!(parse_xhci_keys(bad).is_err(), "{bad:?} must be refused");
        }
    }

    #[test]
    fn xhci_keys_need_the_controller_and_the_firmware_loop() {
        let base = |extra: &[&str]| {
            let mut values = args(&[
                "--image",
                "rom.bin",
                "--com1-out",
                "com1.txt",
                "--budget",
                "10",
            ]);
            values.extend(extra.iter().map(|value| (*value).to_owned()));
            values
        };
        assert!(parse(&base(&["--xhci-keys", "04"])).is_err());
        let options = parse(&base(&["--xhci", "--xhci-keys", "04"])).unwrap();
        assert_eq!(options.xhci_keys, [(0x04, true), (0x04, false)]);
        assert!(parse(&base(&["--xhci"])).unwrap().xhci_keys.is_empty());
        assert!(parse(&base(&["--xhci", "--xhci-keys", "04", "--xhci-keys", "05"])).is_err());
        assert!(parse(&base(&["--xhci", "--cpu-loop", "unified"])).is_err());
    }

    #[test]
    fn trace_call_is_parsed_and_needs_the_unified_loop() {
        let base = |extra: &[&str]| {
            let mut values: Vec<String> = [
                "--image",
                "rom.bin",
                "--com1-out",
                "com1.txt",
                "--budget",
                "10",
            ]
            .iter()
            .map(|value| (*value).to_owned())
            .collect();
            values.extend(extra.iter().map(|value| (*value).to_owned()));
            values
        };
        assert!(
            parse(&base(&[
                "--trace-call",
                "GetVariable=0x22ff00:guid=1:name=0"
            ]))
            .is_err()
        );
        let options = parse(&base(&[
            "--cpu-loop",
            "unified",
            "--trace-call",
            "GetVariable=0x22ff00:guid=1:name=0",
            "--trace-call",
            "ExitBootServices=0x2223e0",
        ]))
        .unwrap();
        assert_eq!(options.trace_calls.len(), 2);
        assert_eq!(
            (
                options.trace_calls[0].physical,
                options.trace_calls[0].guid_arg,
                options.trace_calls[0].name_arg
            ),
            (0x22ff00, Some(1), Some(0))
        );
        assert_eq!(
            (
                options.trace_calls[1].guid_arg,
                options.trace_calls[1].name_arg
            ),
            (None, None)
        );
        for bad in [
            "x",
            "=0x1",
            "N=zz",
            "N=0x0",
            "N=0x1:slot=x",
            "N=0x1:slot=1:slot=2",
            "N=0x1:guid=4",
            "N=0x1:guid=1:guid=2",
            "N=0x1:other=1",
        ] {
            assert!(parse_trace_call(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn framebuffer_out_needs_the_gop_app_or_the_unified_loop() {
        // S7-31p: the unified loop dumps the display the guest left; no FV application finishes there.
        let base = |extra: &[&str]| {
            let mut values: Vec<String> = [
                "--image",
                "rom.bin",
                "--com1-out",
                "com1.txt",
                "--budget",
                "10",
            ]
            .iter()
            .map(|value| (*value).to_owned())
            .collect();
            values.extend(extra.iter().map(|value| (*value).to_owned()));
            values
        };
        assert!(parse(&base(&["--framebuffer-out", "fb.bin"])).is_err());
        let options = parse(&base(&[
            "--framebuffer-out",
            "fb.bin",
            "--cpu-loop",
            "unified",
        ]))
        .unwrap();
        assert!(options.unified && options.framebuffer_out.as_deref() == Some("fb.bin"));
        assert!(parse(&base(&["--framebuffer-out", "fb.bin", "--fv-app", "gop"])).is_ok());
    }

    #[test]
    fn op_histogram_out_needs_the_unified_loop_and_is_taken_once() {
        let required = args(&[
            "--image",
            "rom.bin",
            "--com1-out",
            "out.com1",
            "--budget",
            "10",
        ]);
        let mut given = required.clone();
        given.extend(args(&["--op-histogram-out", "h.json"]));
        assert!(
            parse(&given)
                .err()
                .unwrap()
                .contains("--op-histogram-out requires --cpu-loop unified")
        );
        given.extend(args(&["--cpu-loop", "unified"]));
        assert_eq!(
            parse(&given).unwrap().op_histogram_out.as_deref(),
            Some("h.json")
        );
        given.extend(args(&["--op-histogram-out", "other.json"]));
        assert!(parse(&given).is_err());
    }

    /// The required arguments of the unified loop, then `extra`.
    fn unified_with(extra: &[&str]) -> Vec<String> {
        let mut given = args(&[
            "--image",
            "rom.bin",
            "--com1-out",
            "out.com1",
            "--budget",
            "10",
            "--cpu-loop",
            "unified",
        ]);
        given.extend(args(extra));
        given
    }

    /// `--no-turbo` and `--turbo-stats` belong to the unified loop, are off by default and are
    /// taken once each; without `--cpu-loop unified` (the firmware profile loop, given or by
    /// default) each is refused.
    #[test]
    fn no_turbo_and_turbo_stats_need_the_unified_loop_are_taken_once_and_are_off_by_default() {
        let options = parse(&unified_with(&[])).unwrap();
        assert!(!options.no_turbo && !options.turbo_stats);
        let options = parse(&unified_with(&["--no-turbo"])).unwrap();
        assert!(options.no_turbo && !options.turbo_stats);
        let options = parse(&unified_with(&["--turbo-stats"])).unwrap();
        assert!(!options.no_turbo && options.turbo_stats);
        let options = parse(&unified_with(&["--no-turbo", "--turbo-stats"])).unwrap();
        assert!(options.no_turbo && options.turbo_stats);
        for flag in ["--no-turbo", "--turbo-stats"] {
            assert!(parse(&unified_with(&[flag, flag])).is_err(), "{flag} twice");
            let required = [
                "--image",
                "rom.bin",
                "--com1-out",
                "out.com1",
                "--budget",
                "10",
            ];
            for firmware_loop in [&[][..], &["--cpu-loop", "firmware"][..]] {
                let mut given = args(&required);
                given.extend(args(firmware_loop));
                given.extend(args(&[flag]));
                let error = parse(&given).err().unwrap();
                assert!(
                    error.contains(&format!("{flag} requires --cpu-loop unified")),
                    "{firmware_loop:?} {flag}: {error}"
                );
            }
        }
    }

    /// The flags of the removed acceleration engine of the unified loop (the fast path, the
    /// predecoded block cache, the data translation cache and their checks) are gone: the parser
    /// refuses each like any unknown flag, in both loops.
    #[test]
    fn the_removed_flags_of_the_old_unified_engine_are_refused() {
        for flag in [
            "--fast-path",
            "--fast-path-verify",
            "--no-block-cache",
            "--block-cache-prototype",
            "--block-stats",
            "--block-cache-verify",
            "--block-boundary-verify",
            "--data-cache-prototype",
            "--data-cache-verify",
        ] {
            assert!(parse(&unified_with(&[flag])).is_err(), "{flag}");
            let mut firmware_loop = args(&[
                "--image",
                "rom.bin",
                "--com1-out",
                "out.com1",
                "--budget",
                "10",
            ]);
            firmware_loop.push(flag.to_owned());
            assert!(parse(&firmware_loop).is_err(), "{flag}");
        }
    }

    #[test]
    fn op_histogram_file_lists_keys_by_count_with_running_shares_and_sums() {
        let dir = TempDir::new("op-histogram");
        let path = dir.join("h.json");
        let entry = |operation: &str, operands, cpl, count| od_machine::OpHistogramEntry {
            operation: operation.to_owned(),
            operands,
            operand_bytes: 8,
            cpl,
            count,
        };
        let histogram = od_machine::OpHistogramReport {
            entries: vec![
                entry("Mov", "mem", 0, 6),
                entry("Add", "reg", 3, 3),
                entry("Nop", "none", 0, 1),
            ],
            total: 10,
            by_cpl: [7, 0, 0, 3],
        };
        let summary = write_op_histogram(path.to_str().unwrap(), &histogram, 10).unwrap();
        assert_eq!(summary["sum_equals_retired"], json!(true));
        let document: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        assert_eq!(document["total"], json!(10));
        assert_eq!(document["by_cpl"]["cpl0"]["count"], json!(7));
        assert_eq!(document["by_cpl"]["cpl3"]["count"], json!(3));
        assert_eq!(document["entries"][0]["key"], json!("Mov/mem/8/cpl0"));
        assert_eq!(document["entries"][1]["cumulative_share"], json!(0.9));
        assert_eq!(document["entries"][2]["cumulative_share"], json!(1.0));
        let mismatch = write_op_histogram(path.to_str().unwrap(), &histogram, 11).unwrap();
        assert_eq!(mismatch["sum_equals_retired"], json!(false));
    }

    #[test]
    fn the_unified_board_has_an_explicit_rtc_base_and_keeps_a_given_one() {
        // S7-31o: without a base the CMOS ports are absent and the kernel cannot read the time.
        let mut config = board_config(None, false, None);
        assert_eq!(config.rtc_base_unix_seconds, None);
        apply_unified(&mut config);
        assert_eq!(config.rtc_base_unix_seconds, Some(1_704_067_200));
        assert!(config.linux_profile && config.acpi_pm && config.acpi_tables && config.pci_ecam);
        let mut given = board_config(None, false, None);
        given.rtc_base_unix_seconds = Some(7);
        apply_unified(&mut given);
        assert_eq!(given.rtc_base_unix_seconds, Some(7));
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
    fn cpu_loop_is_refused_where_it_would_be_ignored_or_run_another_loop() {
        let base = |extra: &[&str]| {
            let mut values = args(&[
                "--image",
                "rom.bin",
                "--com1-out",
                "out.com1",
                "--budget",
                "10",
            ]);
            values.extend(args(extra));
            values
        };
        // A repeated option is refused, as for the other options.
        assert!(
            parse(&base(&["--cpu-loop", "unified", "--cpu-loop", "firmware"]))
                .err()
                .unwrap()
                .contains("unexpected or duplicate option --cpu-loop")
        );
        // RIP samples need the observer of the firmware profile loop.
        assert!(
            parse(&base(&["--cpu-loop", "unified", "--sample-rip", "200040"]))
                .err()
                .unwrap()
                .contains("does not support --sample-rip")
        );
        // The reboot paths run the firmware profile loop.
        for extra in [
            &["--fv-app", "reboot", "--reboot-cycles", "2"][..],
            &["--fv-app", "vars", "--nvram-reboot-out", "second.com1"][..],
        ] {
            let mut values = base(&["--cpu-loop", "unified"]);
            values.extend(args(extra));
            assert!(
                parse(&values)
                    .err()
                    .unwrap()
                    .contains("does not combine with --reboot-cycles"),
                "{extra:?}"
            );
        }
        // A marker belongs to the unified loop, and is taken once.
        assert!(
            parse(&base(&["--mark-rip", "200040"]))
                .err()
                .unwrap()
                .contains("--mark-rip requires --cpu-loop unified")
        );
        assert_eq!(
            parse(&base(&["--cpu-loop", "unified", "--mark-rip", "0x200040"]))
                .unwrap()
                .mark_rip,
            Some(0x200040)
        );
        assert!(
            parse(&base(&[
                "--cpu-loop",
                "unified",
                "--mark-rip",
                "200040",
                "--mark-rip",
                "200041"
            ]))
            .is_err()
        );
        assert!(
            parse(&base(&["--cpu-loop", "unified", "--mark-rip", "zz"]))
                .err()
                .unwrap()
                .contains("invalid --mark-rip")
        );
        // The firmware loop with the same options stays accepted.
        assert!(parse(&base(&["--cpu-loop", "firmware", "--sample-rip", "200040"])).is_ok());
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
    fn diagnostics_that_the_chosen_loop_would_silently_drop_are_refused() {
        // S8 merge review D2: the journal belongs to the firmware loop, the xHCI key watch and the reboot cycles
        // never feed it, and the cycles run no diagnostics at all.
        let base = |extra: &[&str]| {
            let mut values = args(&[
                "--image",
                "rom.bin",
                "--com1-out",
                "com1.txt",
                "--budget",
                "10",
            ]);
            values.extend(args(extra));
            values
        };
        let journal = [
            &["--phase-log", "phase.jsonl"][..],
            &["--framebuffer-dir", "frames", "--framebuffer-every", "1"][..],
        ];
        for extra in journal {
            let mut unified = base(&["--cpu-loop", "unified"]);
            unified.extend(args(extra));
            assert!(
                parse(&unified)
                    .err()
                    .unwrap()
                    .contains("--cpu-loop unified does not support --phase-log"),
                "{extra:?}"
            );
            let mut keys = base(&["--xhci", "--xhci-keys", "04"]);
            keys.extend(args(extra));
            assert!(
                parse(&keys)
                    .err()
                    .unwrap()
                    .contains("--xhci-keys cannot be combined with --phase-log"),
                "{extra:?}"
            );
            // The same options alone stay valid, and so do the key watch and the unified loop without a journal.
            assert!(parse(&base(extra)).is_ok(), "{extra:?}");
            let mut cycles = base(&["--fv-app", "reboot", "--reboot-cycles", "2"]);
            cycles.extend(args(extra));
            assert!(
                parse(&cycles)
                    .err()
                    .unwrap()
                    .contains("--reboot-cycles cannot be combined with --stop-snapshot"),
                "{extra:?}"
            );
        }
        let mut cycles = base(&["--fv-app", "reboot", "--reboot-cycles", "2"]);
        cycles.extend(args(&["--stop-snapshot", "stop.json"]));
        assert!(
            parse(&cycles)
                .err()
                .unwrap()
                .contains("--reboot-cycles cannot be combined with --stop-snapshot")
        );
        assert!(parse(&base(&["--xhci", "--xhci-keys", "04"])).is_ok());
        assert!(parse(&base(&["--cpu-loop", "unified"])).is_ok());
        assert!(
            parse(&base(&[
                "--cpu-loop",
                "unified",
                "--stop-snapshot",
                "stop.json"
            ]))
            .is_ok()
        );
        assert!(
            parse(&base(&[
                "--xhci",
                "--xhci-keys",
                "04",
                "--stop-snapshot",
                "stop.json"
            ]))
            .is_ok()
        );
        assert!(parse(&base(&["--fv-app", "reboot", "--reboot-cycles", "2"])).is_ok());
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
    fn disks(values: &[&str]) -> Result<Option<(String, bool)>, String> {
        let owned: Vec<String> = values.iter().map(|value| value.to_string()).collect();
        parse_disks(&owned.iter().collect::<Vec<_>>())
    }

    #[test]
    fn disk_option_accepts_one_file_with_an_optional_ro_and_refuses_everything_else() {
        assert_eq!(disks(&[]), Ok(None));
        assert_eq!(
            disks(&["work/stage8/target.raw"]),
            Ok(Some(("work/stage8/target.raw".to_owned(), false)))
        );
        assert_eq!(
            disks(&["work/stage8/media.raw,ro"]),
            Ok(Some(("work/stage8/media.raw".to_owned(), true)))
        );
        assert!(disks(&["a.raw", "b.raw"]).unwrap_err().contains("S8-20"));
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
            Some(NvmeSource::Disk {
                path: "b.raw",
                read_only: false
            })
        ));
        let read_only = parsed(&["--disk", "b.raw,ro", "--fv-app", "blk"]).unwrap();
        assert!(matches!(
            read_only.nvme_source(),
            Some(NvmeSource::Disk {
                path: "b.raw",
                read_only: true
            })
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
        boot_with_disk_mode(rom, com1, disk, false)
    }

    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    fn boot_with_disk_mode(
        rom: &[u8],
        com1: &Path,
        disk: &str,
        read_only: bool,
    ) -> Result<BootRun, (Value, u8)> {
        let config = FirmwareBoardConfig {
            nvme: Some(Default::default()),
            ..board_config(None, false, None)
        };
        boot(
            rom,
            &config,
            4,
            com1.to_str().unwrap(),
            Some(NvmeSource::Disk {
                path: disk,
                read_only,
            }),
            &[],
            &mut Diagnostics::default(),
            None,
            None,
            &[],
            None,
            &[],
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

    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    #[test]
    fn read_only_disk_refuses_guest_writes_and_leaves_the_file_unchanged() {
        let dir = TempDir::new("od-disk-ro");
        let (disk_path, com1) = (dir.join("media.raw"), dir.join("c1"));
        let disk = disk_path.to_str().unwrap();
        let rom = halting_rom();
        // Content that the stand's write pattern at LBA 0 would visibly change.
        let content: Vec<u8> = (0..1usize << 20).map(|index| (index % 251) as u8).collect();
        fs::write(&disk_path, &content).unwrap();
        let hash_before = sha256_hex(&content);

        let mut first = boot_with_disk_mode(&rom, &com1, disk, true)
            .unwrap_or_else(|(value, _)| panic!("read-only boot: {value}"));
        // Readers share the file; a writer is refused while a reader lives.
        let second = boot_with_disk_mode(&rom, &com1, disk, true)
            .ok()
            .expect("a second read-only run shares the file");
        drop(second);
        let (busy, code) = boot_with_disk(&rom, &com1, disk)
            .err()
            .expect("a writer next to a reader");
        assert_eq!(code, 3);
        assert_eq!(busy["error"]["kind"], "disk");
        assert!(busy["error"]["detail"].as_str().unwrap().contains("locked"));
        // The stand writes sector 0: the controller refuses it with Namespace is
        // Write Protected (generic 0x20, DNR = 1).
        let nvme = first.board.nvme().unwrap();
        let error = crate::s5_nvme_stand::drive_nvme(&mut first.board, &nvme)
            .err()
            .expect("the guest write must be refused");
        let status = error
            .strip_prefix("NVMe command completed with status 0x")
            .and_then(|hex| u32::from_str_radix(hex, 16).ok())
            .unwrap_or_else(|| panic!("unexpected error: {error}"));
        assert_eq!(status & 0x07FF, 0x020, "SCT 0, SC 0x20: {error}");
        assert_ne!(status & 0x4000, 0, "DNR is set: {error}");
        drop(nvme);
        drop(first);

        let after = fs::read(&disk_path).unwrap();
        assert_eq!(sha256_hex(&after), hash_before);
        assert_eq!(after, content);
        // Once the reader is gone a writer can lock the file again.
        drop(
            boot_with_disk(&rom, &com1, disk)
                .ok()
                .expect("writer after the reader"),
        );
    }
}
