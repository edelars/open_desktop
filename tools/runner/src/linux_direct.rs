//! `linux-direct`: load a Linux bzImage, an initramfs and the ACPI blob onto the board and write
//! JSON evidence. Without `--run` the guest is NOT started (S4-37L6); with `--run` it is run
//! until the budget or the first stop (S4-38L1).
//!
//! The runner reads the files, checks the content hash of the kernel and calls
//! `od_machine::load_linux_direct_board`, which validates every input in a fixed order and
//! leaves the board at the 64-bit entry state of the Linux boot protocol. Placement, E820,
//! `boot_params`, GDT and page tables live in `od-machine` (`linux_*.rs`).
//!
//! `--run --budget N` (S4-38L1) then runs the board for at most N instructions through
//! `od_machine::run_linux_direct_board` (2 MiB walk, base CPU profile, Linux-profile ticks) and
//! adds a `stop` object (the stop record: exit kind, RIP and its bytes, control registers,
//! registers, counters, COM1, input hashes); `guest_started` is then true (it means "the run was
//! requested by `--run`", also for `--budget 0`) and `retired` is the count of the run. Nothing
//! else in the evidence changes.
//!
//! `--run --budget N --set-cpl0` (S4-38L3) runs the board through the CPL0 profile set of od-isa
//! instead (`od_machine::run_linux_direct_board_set`; the members are CLI, the CPU-owned CR3 and,
//! since S4-38L4, CPUID with the DiagnosticV1 answers). The run stops BEFORE a `CPUID` of leaf 1
//! or 0x8000_0001 (`exit_kind` `cpuid_divergence`, `cpuid_divergence_leaf`): the instruction does
//! not execute and nothing is answered in its place; other leaves are answered by DiagnosticV1,
//! and what follows past that edge is diagnostic. A `MOV CR3` continues the run; the `stop` object
//! then also says `path: cpl0_set`, the set members, the set's own final exit (`set_exit_detail`)
//! and how many CR3 changes happened
//! (`cr3_changes`, `last_cr3_change`; a `MOV CR3` that ended the run in an integration error is
//! counted and `cr3` is its CR3). Since S4-38L5c / S4-38L9v a guest `#PF` or `#GP(0)` is delivered
//! through the guest IDT when one is loaded; `exceptions_delivered` counts both and the
//! per-vector fields report their breakdown (a delivery is not a retired instruction). An
//! exception that could not be delivered is still the `fault` stop, with the reason in
//! `set_exit_detail`. The `paged_probe` path carries zero counts; `schema_version` stays 1 (the
//! fields are additive).
//!
//! `--diagnostic-past-c7` (S4-38L7, only with `--run --set-cpl0`) is an opt-in DIAGNOSTIC mode:
//! the `cpuid_divergence` boundary is off, `CPUID` leaf 1 and 0x8000_0001 execute with the
//! DiagnosticV1 answer like every other leaf (nothing substituted, no kernel patch) and the run
//! goes on to its next stop, to inventory the forms the kernel executes past C7. The result is NOT
//! counted: the evidence says `diagnostic_past_c7: true` (top level and in `stop`) and
//! `acceptance: "diagnostic_not_counted"`. These markers are always present and set right after
//! the arguments are parsed (also in refusals): without the flag `diagnostic_past_c7` is false
//! and `acceptance` is `"counted"`; the marker follows the request of the flag, not its effect.
//!
//! Order of checks (an earlier failure hides a later one). Steps 1-8 all happen before the guest
//! is started: their JSON says `"guest_started": false, "retired": 0`. A failure of the run
//! itself (`run_failed`, after step 8, only with `--run`) is reported separately: its evidence
//! has `"guest_started": true` and `retired` is the count known at the error (`null` when
//! unknown, with a note in the detail):
//!
//! ```text
//! 1. arguments, nonce shape, RAM size cap        invalid_input            exit 3
//! 2. vmlinuz readable, sha256 == expected        vmlinuz_sha256_mismatch  exit 4
//! 3. initramfs readable, size (by metadata, before   initrd_too_large         exit 4
//!    reading) not larger than RAM, sha256 matches    initramfs_sha256_mismatch
//! 4. od-machine: bzImage header (truncated file  bzimage_truncated | bzimage_payload_out_of_file
//!    or payload beyond the end)                  | bzimage_header_invalid
//! 5. od-machine: the board configuration must be  board_assembly_failed
//!    valid (checked while its manifest hash is
//!    computed), then the expected manifest hash
//!    must be the hash of the board, then the blob   acpi_manifest_mismatch | acpi_blob_invalid
//! 6. od-machine: command line, initrd placement  cmdline_* | initrd_too_large | placement_invalid
//! 7. od-machine: E820, boot_params, board        e820_* | boot_params_invalid | board_assembly_failed
//!    assembly, E820 against the board handoff    | e820_handoff_mismatch
//! 8. od-machine: writes and entry state          guest_memory_access | load_failed
//!                                                | entry_state_invalid
//! ```
//!
//! `invalid_input` (exit 3) from a bad argument is only printed; an unwritable `--out` is printed
//! too (there is no file to write it to). Every `refused` and `loaded` result is printed and
//! written to `--out`.

use od_isa::cpuid::CpuidProfile;
use od_machine::{
    LinuxCpl0SetMembers, LinuxDirectBoot, LinuxDirectError, LinuxDirectInput,
    LinuxExecutionObservations, LinuxStopRecord, MachineStopReason, NvmeBackend, NvmeConfig,
    load_linux_direct_board_with_nvme_and_vcpus, load_linux_direct_board_with_vcpus,
    run_linux_direct_board, run_linux_direct_board_set_observed,
    run_linux_direct_board_set_user_mode_interleaved,
    run_linux_direct_board_set_user_mode_with_delay_tsc_summary,
};
use od_testkit::sha256_hex;
use serde_json::{Value, json};
use std::fs::File;
use std::io::{Read, Write};
use std::process::ExitCode;

#[cfg(test)]
use od_machine::{load_linux_direct_board, run_linux_direct_board_set};

mod checkpoints;
pub(crate) mod series;

const USAGE: &str = "linux-direct --vmlinuz <file> --initramfs <file> --acpi-blob <file> \
--nonce <token> [--nonce-bootconfig] --expected-vmlinuz-sha256 <hex> --expected-acpi-manifest-hash <hex> \
 [--vcpus 1|2] [--seed <u64> (stage4a or stage4a-apic user mode)] \
 [--rootfs-raw <working-copy.raw> (--vcpus 2|4, seeded stage4a-apic user mode, new report path)] \
--ram-mib <n> --out <json> --expected-initramfs-sha256 <hex> [--run --budget <n> [--set-cpl0 [--diagnostic-past-c7] [--cpuid-profile diagnostic|stage4a|stage4a-apic] [--user-mode [--linux-norandmaps] [--linux-no-randomize-kstack-offset]] [--checkpoint-table <oracle.json>] [--stack-context-out <json>] [--execution-observations-out <json> [--execution-observations-delay-call-summary] [--execution-observations-elf-stack-copy] [--execution-observations-rdtsc-prefix] [--execution-observations-quantum-trace] [--execution-observations-post-iret-index <n>] [--execution-observations-rdtsc-rip <hex> [--execution-observations-rdtsc-stride <n>] [--execution-observations-rdtsc-entry-rip <hex>] [--execution-observations-rdtsc-window <start:end>]]]]]";

/// Kernel command line of the S4-29g audit (`docs/reports/STAGE_4_S4-29G_2026-11-09.md`); the
/// nonce is appended as `od.nonce=<nonce>` unless sent through bootconfig.
const CMDLINE_PREFIX: &str = "console=ttyS0,115200 earlycon=uart8250,io,0x3f8 8250.nr_uarts=1";
/// `nokaslr` is added here, by the runner (decision of S4-37: the loader does not edit it).
const CMDLINE_SUFFIX: &str = "nokaslr";
const MAX_NONCE_CHARS: usize = 64;
/// A kernel file larger than this is refused unread (the real vmlinuz is 17 MiB).
const MAX_VMLINUZ_BYTES: u64 = 256 << 20;
const MIB: u64 = 1 << 20;
/// Largest `--ram-mib`. `assemble_board` still builds the firmware boot tables with 4 KiB leaves
/// (they are dropped for the Linux profile), which costs minutes and gigabytes on a big RAM;
/// the loader test of the full range stops at 8 GiB for the same reason.
const MAX_RAM_MIB: u64 = 8192;

#[derive(Debug, Eq, PartialEq)]
struct Options {
    vmlinuz: String,
    initramfs: String,
    acpi_blob: String,
    rootfs_raw: Option<String>,
    nonce: String,
    /// Carry the nonce in the initrd bootconfig instead of the early command line.
    nonce_bootconfig: bool,
    expected_vmlinuz_sha256: String,
    expected_acpi_manifest_hash: [u8; 32],
    expected_initramfs_sha256: String,
    ram_mib: u64,
    out: String,
    /// `--run --budget N`: run the guest for at most N instructions after loading.
    budget: Option<u64>,
    vcpu_count: usize,
    /// `--seed N`: use the seeded interleaved scheduler for a Stage 6 run.
    interleaved_seed: Option<u64>,
    /// `--set-cpl0`: run through the CPL0 profile set instead of the base probe profile.
    set_cpl0: bool,
    /// `--diagnostic-past-c7` (S4-38L7): CPUID boundary off, result diagnostic and not counted.
    diagnostic_past_c7: bool,
    /// `--cpuid-profile diagnostic|stage4a|stage4a-apic` (S4-38L9a, S4-38L8d: the APIC variant; only with
    /// `--set-cpl0`): the CPUID profile of the set; `diagnostic` (DiagnosticV1, with the C7 boundary) when absent.
    cpuid_profile: CpuidProfile,
    /// `--user-mode` (S4-39c, only with `--set-cpl0 --cpuid-profile stage4a|stage4a-apic`, S4-38L9-um): a committed `IRETQ`
    /// to CPL3 continues with the CPL3 entry instead of stopping at `user_entry`.
    user_mode: bool,
    /// Opt-in user-stack diagnostic: append Linux's `norandmaps` cmdline parameter.
    linux_norandmaps: bool,
    /// Opt-in stack-unwind diagnostic: append `randomize_kstack_offset=off` to the Linux cmdline.
    linux_no_randomize_kstack_offset: bool,
    /// `--checkpoint-table <s4-38l2-oracle.json>` (S4-38L4i, only with `--run --set-cpl0`): record
    /// the checkpoints C0..C17 (observation only) and add the `checkpoints` key.
    checkpoint_table: Option<String>,
    /// Separate, opt-in sidecar for bounded console-output stack context; single run only.
    stack_context_out: Option<String>,
    /// Separate, opt-in sidecar for CPL0 RDTSC sites and delivered #PF events; single user-mode run only.
    execution_observations_out: Option<String>,
    /// Zero-based delivered-#PF ordinal for the opt-in post-IRET page-walk snapshot.
    execution_observations_post_iret_index: Option<u64>,
    /// Selected CPL0 RDTSC RIP for bounded post-instruction register samples.
    execution_observations_rdtsc_rip: Option<u64>,
    /// Optional delay-function entry RIP, captured at every matching CPL0 RDTSC.
    execution_observations_rdtsc_entry_rip: Option<u64>,
    /// Capture every Nth hit at the selected CPL0 RDTSC RIP.
    execution_observations_rdtsc_stride: Option<u64>,
    /// Inclusive 1-based selected-site hit range for focused poll samples.
    execution_observations_rdtsc_window: Option<od_machine::RdtscSampleWindow>,
    /// Capture every CPL0 RDTSC through the fixed Linux delay_tsc entry sentinel.
    execution_observations_rdtsc_prefix: bool,
    execution_observations_delay_call_summary: bool,
    /// Capture one strictly gated, bounded instruction-path quantum for the S5-07 follow-up.
    execution_observations_quantum_trace: bool,
    execution_observations_elf_stack_copy: bool,
}

pub fn run(args: &[String]) -> ExitCode {
    if args.iter().any(|argument| argument == "--test-runs") {
        return series::run(args);
    }
    let options = match parse(args) {
        Ok(options) => options,
        Err(detail) => {
            let value = refusal(
                "invalid_input",
                "arguments",
                detail,
                json!({"retired": 0, "guest_started": false}),
            );
            println!("{value}");
            return ExitCode::from(3);
        }
    };
    let (rootfs, mut report_file) = if let Some(path) = &options.rootfs_raw {
        let backend = match open_rootfs(path) {
            Ok(backend) => backend,
            Err(detail) => {
                let value = refusal(
                    "invalid_input",
                    "rootfs_raw",
                    detail,
                    json!({"retired": 0, "guest_started": false}),
                );
                println!("{value}");
                return ExitCode::from(3);
            }
        };
        // Reserve the output without following a symlink or truncating an existing file, then
        // keep that exact descriptor through the run. The report can never overwrite the RAW.
        let file = match File::create_new(&options.out) {
            Ok(file) => file,
            Err(error) => {
                let value = refusal(
                    "invalid_input",
                    "out_file",
                    error.to_string(),
                    json!({"retired": 0, "guest_started": false}),
                );
                println!("{value}");
                return ExitCode::from(3);
            }
        };
        (Some(backend), Some(file))
    } else {
        (None, None)
    };
    let (mut value, code) = evaluate_with_rootfs(&options, &DiagnosticEnv::from_process(), rootfs);
    let skip_report_write = value
        .get("_skip_report_write")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    if let Some(object) = value.as_object_mut() {
        object.remove("_skip_report_write");
    }
    if skip_report_write {
        println!("{value}");
        return ExitCode::from(code);
    }
    let sidecar_collides_with_report = options
        .stack_context_out
        .as_ref()
        .into_iter()
        .chain(options.execution_observations_out.as_ref())
        .any(|sidecar| match_existing_file_identity(sidecar, &options.out).unwrap_or(true));
    if sidecar_collides_with_report {
        let value = refusal(
            "invalid_input",
            "stack_context_sidecar",
            "the sidecar and primary report resolve to the same filesystem object".into(),
            json!({"retired": 0, "guest_started": false}),
        );
        println!("{value}");
        return ExitCode::from(3);
    }
    let text = format!("{value:#}\n");
    let written = match &mut report_file {
        Some(file) => file.write_all(text.as_bytes()),
        None => std::fs::write(&options.out, &text),
    };
    if let Err(error) = written {
        let value = refusal(
            "invalid_input",
            "out_file",
            error.to_string(),
            json!({"retired": 0, "guest_started": false}),
        );
        println!("{value}");
        return ExitCode::from(3);
    }
    println!("{value}");
    ExitCode::from(code)
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
fn open_rootfs(path: &str) -> Result<Box<dyn NvmeBackend + Send>, String> {
    use od_host::persistent_nvme::PersistentNvmeBackend;
    PersistentNvmeBackend::open(std::path::Path::new(path))
        .map(|backend| Box::new(backend) as Box<dyn NvmeBackend + Send>)
        .map_err(|error| format!("{path}: {error}"))
}

#[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
fn open_rootfs(_path: &str) -> Result<Box<dyn NvmeBackend + Send>, String> {
    Err("--rootfs-raw requires the macOS arm64 persistent storage backend".into())
}

/// Runs the whole pipeline and returns the JSON evidence and the exit code (0 loaded, 4 refused,
/// 3 unreadable input). Without `--run` the guest is not started; with `--run` it runs until the
/// budget or its first stop, after the load has succeeded.
fn evaluate(options: &Options) -> (Value, u8) {
    evaluate_with_rootfs(options, &DiagnosticEnv::from_process(), None)
}

/// The environment variables of the diagnostic runs, read once (S4-13, S4-13b); a test gives its
/// own instead of touching the environment shared by the parallel tests.
#[derive(Default)]
struct DiagnosticEnv {
    no_fetch_cache: Option<std::ffi::OsString>,
    fetch_cache_stats: Option<std::ffi::OsString>,
    /// S4-13c: shadow caches, only together with `OD_FETCH_CACHE_STATS`.
    fetch_cache_sweep: Option<std::ffi::OsString>,
    /// S4-13d: `2m` makes one fetch cache entry per 2 MiB leaf (default: per 4 KiB page).
    fetch_cache_tag: Option<std::ffi::OsString>,
}

impl DiagnosticEnv {
    fn from_process() -> Self {
        Self {
            no_fetch_cache: std::env::var_os(NO_FETCH_CACHE_ENV),
            fetch_cache_stats: std::env::var_os(FETCH_CACHE_STATS_ENV),
            fetch_cache_sweep: std::env::var_os(FETCH_CACHE_SWEEP_ENV),
            fetch_cache_tag: std::env::var_os(FETCH_CACHE_TAG_ENV),
        }
    }
}

#[cfg(test)]
fn evaluate_with_env(options: &Options, env: &DiagnosticEnv) -> (Value, u8) {
    evaluate_with_rootfs(options, env, None)
}

fn evaluate_with_rootfs(
    options: &Options,
    env: &DiagnosticEnv,
    rootfs: Option<Box<dyn NvmeBackend + Send>>,
) -> (Value, u8) {
    let mut evidence = json!({
        "retired": 0,
        "guest_started": false,
        "nonce": options.nonce,
        "ram_mib": options.ram_mib,
    });
    if options.budget.is_some()
        && options.vcpu_count > 1
        && env
            .fetch_cache_stats
            .as_ref()
            .is_some_and(|path| !path.is_empty())
    {
        return (
            refusal(
                "invalid_input",
                "arguments",
                "fetch cache statistics require --vcpus 1".into(),
                evidence,
            ),
            3,
        );
    }
    mark_acceptance(
        &mut evidence,
        options.diagnostic_past_c7,
        options.linux_norandmaps,
        options.linux_no_randomize_kstack_offset,
        options.cpuid_profile,
    );
    let io_failure = |evidence: Value, stage: &str, detail: String| {
        (refusal("invalid_input", stage, detail, evidence), 3)
    };

    // 2. The kernel file and its content hash.
    let vmlinuz = match read_capped(&options.vmlinuz, MAX_VMLINUZ_BYTES) {
        Ok(bytes) => bytes,
        Err(detail) => return io_failure(evidence, "vmlinuz_file", detail),
    };
    let vmlinuz_sha = sha256_hex(&vmlinuz);
    evidence["inputs"]["vmlinuz"] = json!({
        "path": options.vmlinuz,
        "bytes": vmlinuz.len(),
        "sha256": vmlinuz_sha,
        "expected_sha256": options.expected_vmlinuz_sha256,
    });
    if vmlinuz_sha != options.expected_vmlinuz_sha256 {
        return (
            refusal(
                "vmlinuz_sha256_mismatch",
                "vmlinuz_sha256",
                format!(
                    "sha256 {vmlinuz_sha} differs from the expected {}",
                    options.expected_vmlinuz_sha256
                ),
                evidence,
            ),
            4,
        );
    }

    // 3. The initramfs: readable, and not larger than guest RAM (a cheap bound before the
    //    placement of od-machine decides the exact limit). The size comes from the file
    //    metadata, so an oversized file is refused before a single byte is read into memory.
    let ram_bytes = options.ram_mib * MIB;
    let mut initramfs = match read_capped(&options.initramfs, ram_bytes) {
        Ok(bytes) => bytes,
        Err(detail) if detail == TOO_LARGE => {
            evidence["inputs"]["initramfs"] = json!({"path": options.initramfs});
            return (
                refusal(
                    "initrd_too_large",
                    "initrd_size",
                    format!("the file is larger than guest RAM ({ram_bytes} bytes)"),
                    evidence,
                ),
                4,
            );
        }
        Err(detail) => return io_failure(evidence, "initramfs_file", detail),
    };
    let initramfs_sha = sha256_hex(&initramfs);
    evidence["inputs"]["initramfs"] = json!({
        "path": options.initramfs,
        "bytes": initramfs.len(),
        "sha256": initramfs_sha,
    });
    let expected = &options.expected_initramfs_sha256;
    evidence["inputs"]["initramfs"]["expected_sha256"] = json!(expected);
    if &initramfs_sha != expected {
        return (
            refusal(
                "initramfs_sha256_mismatch",
                "initramfs_sha256",
                format!("sha256 {initramfs_sha} differs from the expected {expected}"),
                evidence,
            ),
            4,
        );
    }

    // S4-38L4i: the oracle table of the checkpoints, checked against the kernel that runs, before
    // anything is loaded or started.
    let checkpoint_setup = match options.checkpoint_table.as_deref() {
        Some(path) => match checkpoints::load(path, &vmlinuz_sha, &initramfs) {
            Ok(setup) => Some(setup),
            Err(checkpoints::TableError::Invalid(detail)) => {
                return io_failure(evidence, "checkpoint_table", detail);
            }
            Err(checkpoints::TableError::VmlinuzMismatch(detail)) => {
                return (
                    refusal(
                        "checkpoint_table_vmlinuz_mismatch",
                        "checkpoint_table",
                        detail,
                        evidence,
                    ),
                    4,
                );
            }
        },
        None => None,
    };

    let acpi_blob = match read_capped(&options.acpi_blob, 1 << 20) {
        Ok(bytes) => bytes,
        Err(detail) => return io_failure(evidence, "acpi_blob_file", detail),
    };
    evidence["inputs"]["acpi_blob"] = json!({
        "path": options.acpi_blob,
        "bytes": acpi_blob.len(),
        "sha256": sha256_hex(&acpi_blob),
        "expected_manifest_hash": hex(&options.expected_acpi_manifest_hash),
    });

    // 6 (cmdline). The runner builds the line; od-machine checks it.
    let (mut cmdline, loaded_initrd_sha256) = if options.nonce_bootconfig {
        let trailer = nonce_bootconfig_trailer(&options.nonce, initramfs.len() as u64);
        let loaded_bytes = initramfs.len() + trailer.len();
        if loaded_bytes as u64 > ram_bytes {
            return (
                refusal(
                    "initrd_too_large",
                    "initrd_size",
                    format!(
                        "the initrd with bootconfig is larger than guest RAM ({ram_bytes} bytes)"
                    ),
                    evidence,
                ),
                4,
            );
        }
        initramfs.extend_from_slice(&trailer);
        let loaded_hash = sha256_hex(&initramfs);
        evidence["inputs"]["initramfs"]["nonce_bootconfig"] = json!({
            "trailer_sha256": sha256_hex(&trailer),
            "loaded_sha256": loaded_hash,
            "loaded_bytes": loaded_bytes,
        });
        (nonce_bootconfig_cmdline(), loaded_hash)
    } else {
        (
            cmdline(
                &options.nonce,
                options.linux_norandmaps,
                options.linux_no_randomize_kstack_offset,
            ),
            initramfs_sha,
        )
    };
    if rootfs.is_some() {
        cmdline.push_str(" rdinit=/s6-smp-init");
    }
    evidence["cmdline"] = json!(cmdline);

    // 4-8. od-machine.
    let machine_input = LinuxDirectInput {
        bzimage: &vmlinuz,
        initrd: &initramfs,
        cmdline: cmdline.as_bytes(),
        acpi_blob: &acpi_blob,
        expected_acpi_manifest_hash: options.expected_acpi_manifest_hash,
        ram_bytes,
    };
    let loaded = if let Some(backend) = &rootfs {
        let namespace_sectors = backend.sector_count();
        evidence["inputs"]["rootfs_raw"] = json!({
            "path": options.rootfs_raw,
            "namespace_sectors": namespace_sectors,
        });
        load_linux_direct_board_with_nvme_and_vcpus(
            &machine_input,
            NvmeConfig {
                namespace_sectors,
                ..NvmeConfig::default()
            },
            options.vcpu_count,
        )
    } else {
        load_linux_direct_board_with_vcpus(&machine_input, options.vcpu_count)
    };
    match loaded {
        Ok(mut boot) => {
            if let Some(backend) = rootfs {
                let nvme = boot.board.nvme().expect("loaded PCI/NVMe board");
                if let Err(error) = nvme.attach_backend(backend) {
                    return (
                        refusal(
                            "nvme_attach_failed",
                            "rootfs_raw",
                            format!("{error:?}"),
                            evidence,
                        ),
                        4,
                    );
                }
            }
            describe(
                &mut evidence,
                &boot,
                &cmdline,
                &vmlinuz,
                &loaded_initrd_sha256,
            );
            if let Some(seed) = options.interleaved_seed {
                if options.vcpu_count == 1
                    && let Err(error) = boot.board.enable_seeded_linux_interleaved(seed)
                {
                    return (
                        refusal(
                            "run_failed",
                            "scheduler",
                            format!("could not enable the interleaved scheduler: {error:?}"),
                            evidence,
                        ),
                        4,
                    );
                }
                evidence["interleaved_scheduler"] = json!({
                    "seed": seed,
                    "vcpus": boot.board.vcpu_count(),
                });
            }
            if let Some(budget) = options.budget {
                let mut sidecar_file = None;
                let mut execution_observations_file = None;
                if let Some(path) = &options.stack_context_out {
                    let file = match std::fs::OpenOptions::new()
                        .write(true)
                        .create_new(true)
                        .open(path)
                    {
                        Ok(file) => file,
                        Err(error) => {
                            let mut value = refusal(
                                "diagnostic_sidecar_reservation_failed",
                                "stack_context_sidecar",
                                format!("{path}: {error}"),
                                evidence,
                            );
                            // Existing files, including dangling symlinks, are preserved when
                            // destination identity cannot be established.
                            value["_skip_report_write"] = json!(true);
                            return (value, 4);
                        }
                    };
                    match file_matches_existing_path(&file, path, &options.out) {
                        Ok(false) => sidecar_file = Some(file),
                        Ok(true) => {
                            let cleanup = discard_sidecar_reservation(path, file);
                            let mut value = refusal(
                                "diagnostic_sidecar_path_collision",
                                "stack_context_sidecar",
                                "--out resolves to the reserved sidecar file".into(),
                                evidence,
                            );
                            if cleanup.is_err() {
                                value["_skip_report_write"] = json!(true);
                            }
                            return (value, 4);
                        }
                        Err(detail) => {
                            let cleanup = discard_sidecar_reservation(path, file);
                            let mut value = refusal(
                                "diagnostic_sidecar_identity_failed",
                                "stack_context_sidecar",
                                detail,
                                evidence,
                            );
                            // If identity could not be established, avoid writing through an
                            // output alias that might refer to the reservation.
                            value["_skip_report_write"] = json!(true);
                            let _ = cleanup;
                            return (value, 4);
                        }
                    }
                    boot.board.enable_console_output_context_diagnostic();
                }
                if let Some(path) = &options.execution_observations_out {
                    let file = match std::fs::OpenOptions::new()
                        .write(true)
                        .create_new(true)
                        .open(path)
                    {
                        Ok(file) => file,
                        Err(error) => {
                            let cleanup = cleanup_sidecar_reservation(
                                options.stack_context_out.as_deref(),
                                &mut sidecar_file,
                            );
                            let mut value = refusal(
                                "diagnostic_sidecar_reservation_failed",
                                "execution_observations_sidecar",
                                format!("{path}: {error}"),
                                evidence,
                            );
                            value["_skip_report_write"] = json!(true);
                            let _ = cleanup;
                            return (value, 4);
                        }
                    };
                    match file_matches_existing_path(&file, path, &options.out) {
                        Ok(false) => execution_observations_file = Some(file),
                        Ok(true) => {
                            let cleanup = discard_sidecar_reservation(path, file);
                            let prior_cleanup = cleanup_sidecar_reservation(
                                options.stack_context_out.as_deref(),
                                &mut sidecar_file,
                            );
                            let mut value = refusal(
                                "diagnostic_sidecar_path_collision",
                                "execution_observations_sidecar",
                                "--out resolves to the reserved sidecar file".into(),
                                evidence,
                            );
                            if cleanup.is_err() || prior_cleanup.is_err() {
                                value["_skip_report_write"] = json!(true);
                            }
                            return (value, 4);
                        }
                        Err(detail) => {
                            let cleanup = discard_sidecar_reservation(path, file);
                            let prior_cleanup = cleanup_sidecar_reservation(
                                options.stack_context_out.as_deref(),
                                &mut sidecar_file,
                            );
                            let mut value = refusal(
                                "diagnostic_sidecar_identity_failed",
                                "execution_observations_sidecar",
                                detail,
                                evidence,
                            );
                            value["_skip_report_write"] = json!(true);
                            let _ = cleanup;
                            let _ = prior_cleanup;
                            return (value, 4);
                        }
                    }
                }
                // S4-13b: the opt-in sidecar of the fetch cache counters. Reserved before the run
                // like the other sidecars; the default run (no `OD_FETCH_CACHE_STATS`) does none
                // of this and its report is the one it always was.
                let fetch_stats_path = match fetch_cache_stats_path(env.fetch_cache_stats.clone()) {
                    Ok(path) => path,
                    Err(detail) => {
                        let cleanup = cleanup_sidecar_reservation(
                            options.stack_context_out.as_deref(),
                            &mut sidecar_file,
                        );
                        let mut value = refusal(
                            "diagnostic_sidecar_reservation_failed",
                            "fetch_cache_stats_sidecar",
                            detail,
                            evidence,
                        );
                        value["_skip_report_write"] = json!(true);
                        let _ = cleanup;
                        return (value, 4);
                    }
                };
                let mut fetch_stats_file = None;
                if let Some(path) = &fetch_stats_path {
                    let reserved = std::fs::OpenOptions::new()
                        .write(true)
                        .create_new(true)
                        .open(path)
                        .map_err(|error| format!("{path}: {error}"))
                        .and_then(|file| {
                            // `--out` is written later; it must not be the same file.
                            match file_matches_existing_path(&file, path, &options.out) {
                                Ok(false) => Ok(file),
                                Ok(true) => {
                                    let _ = discard_sidecar_reservation(path, file);
                                    Err("--out resolves to the reserved sidecar file".to_string())
                                }
                                Err(detail) => {
                                    let _ = discard_sidecar_reservation(path, file);
                                    Err(detail)
                                }
                            }
                        });
                    match reserved {
                        Ok(file) => fetch_stats_file = Some(file),
                        Err(detail) => {
                            let stack_cleanup = cleanup_sidecar_reservation(
                                options.stack_context_out.as_deref(),
                                &mut sidecar_file,
                            );
                            let observations_cleanup = execution_observations_file
                                .take()
                                .zip(options.execution_observations_out.as_deref())
                                .map(|(file, path)| discard_sidecar_reservation(path, file));
                            let mut value = refusal(
                                "diagnostic_sidecar_reservation_failed",
                                "fetch_cache_stats_sidecar",
                                detail,
                                evidence,
                            );
                            value["_skip_report_write"] = json!(true);
                            let _ = (stack_cleanup, observations_cleanup);
                            return (value, 4);
                        }
                    }
                    boot.board.enable_fetch_cache_report();
                    // S4-13c: the shadow caches ride on the sidecar and add one key to it.
                    if fetch_cache_sweep_enabled(env.fetch_cache_sweep.as_deref()) {
                        boot.board
                            .enable_fetch_cache_sweep(od_machine::SweepConfig::full_grid());
                    }
                }
                // S4-13d: the 2 MiB leaf tag of the real cache, only with `OD_FETCH_CACHE_TAG=2m`
                // and, like the sweep, only together with the sidecar (`OD_FETCH_CACHE_STATS`);
                // without it the request is ignored and said so on stderr.
                let (apply_tag, tag_warning) = fetch_cache_leaf_2m_tag_decision(
                    env.fetch_cache_tag.as_deref(),
                    fetch_stats_path.is_some(),
                );
                if let Some(warning) = tag_warning {
                    eprintln!("{warning}");
                }
                if apply_tag {
                    boot.board.enable_fetch_cache_leaf_2m_tag();
                }
                let members = LinuxCpl0SetMembers {
                    // S4-13 diagnostics: `OD_NO_FETCH_CACHE=1` runs without the fetch
                    // translation cache (same results, more page walks); the sidecar of
                    // `OD_FETCH_CACHE_STATS` says which way the run went.
                    fetch_cache: fetch_cache_enabled(env.no_fetch_cache.as_deref()),
                    diagnostic_past_c7: options.diagnostic_past_c7,
                    cpuid_profile: options.cpuid_profile,
                    // S4-38L9c: with the TSC-only subset the kernel's `tdx_init` reads
                    // MSR 0x87 (MKTME KeyID partitioning); the member answers 0:0. Off for
                    // the DiagnosticV1 default, whose run does not get there.
                    msr_tdx_keyid_partitioning_read: options.cpuid_profile.is_stage4a_family(),
                    // S4-38L9d: with the subset that declares FPU and FXSR the kernel's
                    // `fpu__init_system` runs `FNINIT` and `FXSAVE`; the FP member
                    // implements them. Off for the DiagnosticV1 default.
                    fp_state: options.cpuid_profile.is_stage4a_family(),
                    // S4-38L9e: the kernel's `int3_selftest` executes `INT3` and waits for
                    // `#BP` through the IDT; the INT3 member delivers it. Off for the
                    // DiagnosticV1 default (its member list stays as it was).
                    int3: options.cpuid_profile.is_stage4a_family(),
                    ..LinuxCpl0SetMembers::linux_default()
                };
                let run_started = std::time::Instant::now();
                let result = if options.vcpu_count > 1 {
                    run_linux_direct_board_set_user_mode_interleaved(
                        &mut boot,
                        &machine_input,
                        budget,
                        members,
                        options.interleaved_seed.expect("validated SMP seed"),
                    )
                    .map(|report| {
                        evidence["interleaved_scheduler"]["terminal_vcpu"] =
                            json!(report.terminal_vcpu);
                        evidence["interleaved_scheduler"]["cpu_stats"] = Value::Array(
                            report
                                .vcpus
                                .iter()
                                .map(|cpu| {
                                    json!({
                                        "index": cpu.index,
                                        "retired": cpu.retired,
                                        "activity": format!("{:?}", cpu.activity),
                                    "mode": format!("{:?}", cpu.mode),
                                        "rip": cpu.rip,
                                        "rflags": cpu.rflags,
                                        "gpr": cpu.gpr,
                                        "cr3": cpu.cr3,
                                    "guest_efer": cpu.guest_efer,
                                    })
                                })
                                .collect(),
                        );
                        report.stop
                    })
                } else if options.execution_observations_out.is_some() {
                    run_linux_direct_board_set_user_mode_with_delay_tsc_summary(
                        &mut boot,
                        &machine_input,
                        budget,
                        members,
                        options.execution_observations_post_iret_index,
                        options.execution_observations_rdtsc_rip,
                        options.execution_observations_rdtsc_stride.unwrap_or(1),
                        options.execution_observations_rdtsc_entry_rip,
                        options.execution_observations_rdtsc_window,
                        options
                            .execution_observations_rdtsc_prefix
                            .then_some(od_machine::LINUX_RDTSC_PREFIX_SENTINEL_RIP),
                        options.execution_observations_quantum_trace,
                        options.execution_observations_elf_stack_copy,
                        options.execution_observations_delay_call_summary,
                    )
                } else if options.set_cpl0 {
                    run_linux_direct_board_set_observed(
                        &mut boot,
                        &machine_input,
                        budget,
                        members,
                        options.user_mode,
                        checkpoint_setup.as_ref().map(|setup| setup.table.clone()),
                    )
                } else {
                    run_linux_direct_board(&mut boot, &machine_input, budget)
                };
                let run_wall_seconds = run_started.elapsed().as_secs_f64();
                if let Some(path) = &fetch_stats_path {
                    let sweep_report = boot.board.take_fetch_cache_sweep();
                    let final_ram_sha256 = boot.board.ram_sha256();
                    let write_error = match (
                        boot.board.take_fetch_cache_report(),
                        fetch_stats_file.as_mut(),
                    ) {
                        (Some(report), Some(file)) => write_fetch_cache_stats_sidecar(
                            file,
                            path,
                            &report,
                            sweep_report.as_ref(),
                            result.is_ok(),
                            run_wall_seconds,
                            final_ram_sha256,
                        )
                        .err(),
                        // Only the CPL0 set loop (`--set-cpl0`) builds the report; the empty
                        // reservation is removed rather than left looking like a result.
                        (None, _) => {
                            let cleanup = fetch_stats_file
                                .take()
                                .and_then(|file| discard_sidecar_reservation(path, file).err());
                            Some(match cleanup {
                                None => "the run left no fetch cache report (needs --set-cpl0)"
                                    .to_string(),
                                Some(error) => format!(
                                    "the run left no fetch cache report (needs --set-cpl0); {error}"
                                ),
                            })
                        }
                        (_, None) => Some("sidecar reservation is missing".into()),
                    };
                    evidence["fetch_cache_stats_sidecar"] = json!({
                        "path": path,
                        "write_error": write_error,
                    });
                }
                if let Some(path) = &options.stack_context_out {
                    let snapshots = boot.board.take_console_output_context_diagnostic();
                    let uart_diagnostic = boot.board.take_uart_output_context_diagnostic();
                    let write_error = match sidecar_file.as_mut() {
                        Some(file) => write_stack_context_sidecar(
                            file,
                            path,
                            &snapshots,
                            uart_diagnostic.as_ref(),
                        )
                        .err(),
                        None => Some("sidecar reservation is missing".into()),
                    };
                    evidence["stack_context_sidecar"] = json!({
                        "path": path,
                        "capture_scope": "console-output and bounded UART byte context",
                        "unwind_context": false,
                        "snapshots": snapshots.len(),
                        "uart_provenance_samples": uart_diagnostic.as_ref().map_or(0, |capture| capture.samples.len()),
                        "uart_provenance_end": uart_diagnostic.as_ref().map(|capture| uart_context_end_name(capture.end)),
                        "write_error": write_error,
                    });
                }
                if let Some(path) = &options.execution_observations_out {
                    let mut successful_metadata = None;
                    let write_error = if result.is_err() {
                        let cleanup_error = execution_observations_file
                            .take()
                            .and_then(|file| discard_sidecar_reservation(path, file).err());
                        Some(cleanup_error.unwrap_or_else(|| {
                            "run failed before observations were complete".into()
                        }))
                    } else {
                        match (result.as_ref().ok(), execution_observations_file.as_mut()) {
                            (Some(record), Some(file)) => {
                                match record.execution_observations.as_ref() {
                                    Some(observations) => {
                                        match write_execution_observations_sidecar(
                                            file,
                                            path,
                                            observations,
                                        ) {
                                            Ok((byte_count, sha256)) => {
                                                successful_metadata =
                                                    Some(execution_observations_sidecar_metadata(
                                                        path,
                                                        observations,
                                                        byte_count,
                                                        sha256,
                                                    ));
                                                None
                                            }
                                            Err(error) => Some(error),
                                        }
                                    }
                                    None => Some("run returned no execution observations".into()),
                                }
                            }
                            (_, None) => Some("sidecar reservation is missing".into()),
                            (None, _) => Some("run result is unavailable".into()),
                        }
                    };
                    let mut metadata = successful_metadata.unwrap_or_else(|| json!({
                        "path": path,
                        "capture_scope": "CPL0 RDTSC instruction RIP counts and successfully delivered #PF events, with optional selected-site and entry-site samples",
                    }));
                    metadata["write_error"] = json!(write_error);
                    evidence["execution_observations_sidecar"] = metadata;
                }
                match result {
                    Ok(record) => {
                        evidence["guest_started"] = json!(true);
                        evidence["retired"] = json!(record.retired);
                        evidence["stop"] = describe_stop(&record, budget);
                        if let Some(setup) = &checkpoint_setup {
                            evidence["checkpoints"] = checkpoints::evidence(setup, &record);
                        }
                    }
                    Err(error) => return (run_failure(&error, evidence), 4),
                }
            }
            let mut value = json!({
                "schema_version": 1,
                "command": "linux-direct",
                "status": "loaded",
            });
            merge(&mut value, evidence);
            (value, 0)
        }
        Err(error) => (machine_refusal(&error, evidence), 4),
    }
}

/// S4-38L7: the acceptance marker, set right after the arguments are parsed so that every result
/// (a refusal of an early step, of the machine or of the run included) carries it. Always
/// present: `acceptance` is `counted` without diagnostic flags and `diagnostic_not_counted` with
/// `--diagnostic-past-c7`, `--linux-norandmaps`, or `--linux-no-randomize-kstack-offset`. The
/// marker follows the requested flags, not their observed effect.
///
/// S4-38L9a: with `--cpuid-profile stage4a` the CPUID boundary is off by the profile (it declares
/// a subset of the target: TSC, since S4-38L9d also FPU and FXSR) and `cpuid_profile` says
/// `X8664V1Stage4A` (the key is absent for the default profile, so its evidence is unchanged).
/// `diagnostic_past_c7` still follows its flag.
///
/// S4-39j: the profile alone no longer makes the result not counted. Stage4a is the counted
/// profile of the plan. Diagnostic flags control whether a result is counted; the board and
/// `exit_kind` do not depend on them.
fn mark_acceptance(
    evidence: &mut Value,
    diagnostic_past_c7: bool,
    linux_norandmaps: bool,
    linux_no_randomize_kstack_offset: bool,
    cpuid_profile: CpuidProfile,
) {
    let subset = cpuid_profile != CpuidProfile::DiagnosticV1;
    evidence["diagnostic_past_c7"] = json!(diagnostic_past_c7);
    evidence["linux_norandmaps"] = json!(linux_norandmaps);
    evidence["linux_no_randomize_kstack_offset"] = json!(linux_no_randomize_kstack_offset);
    let diagnostic = diagnostic_past_c7 || linux_norandmaps || linux_no_randomize_kstack_offset;
    // S4-38L8d: the APIC profile is never counted (only the checkpoint L8d is its status; the PASS
    // of S4-39j stays with `stage4a`).
    let apic = cpuid_profile == CpuidProfile::X8664V1Stage4AApic;
    evidence["acceptance"] = json!(if diagnostic || apic {
        "diagnostic_not_counted"
    } else {
        "counted"
    });
    if subset {
        evidence["cpuid_profile"] = json!(format!("{cpuid_profile:?}"));
    }
    // With both flags the note carries both explanations (subset and C7).
    let mut parts = Vec::new();
    if apic {
        parts.push(
            "CPUID profile X8664V1Stage4AApic declares a subset of the target (TSC, FPU, FXSR and \
             APIC; checkpoint C7 boundary off by the profile; PIT -> IOAPIC INTIN2 -> Local APIC \
             timer line and the clock owner on)",
        );
    } else if subset {
        parts.push(
            "CPUID profile X8664V1Stage4A declares a subset of the target (TSC, FPU and FXSR; \
             checkpoint C7 boundary off by the profile)",
        );
    }
    if diagnostic_past_c7 {
        parts.push(if subset {
            "--diagnostic-past-c7 also turns the checkpoint C7 boundary off by itself, so this run \
             does not show that the profile alone avoids it"
        } else {
            "DiagnosticV1 answers CPUID leaf 1 and 0x80000001 (checkpoint C7 boundary off); not a \
             profile of the target CPU"
        });
    }
    if linux_norandmaps {
        parts.push(
            "--linux-norandmaps disables Linux address-space randomization for the user-stack diagnostic",
        );
    }
    if linux_no_randomize_kstack_offset {
        parts.push(
            "--linux-no-randomize-kstack-offset disables Linux kernel stack offset randomization for this diagnostic",
        );
    }
    if !parts.is_empty() {
        let verdict = if apic {
            "the result is NOT counted (a checkpoint L8d record only; the S4-39j PASS stays with stage4a)"
        } else if diagnostic {
            "the result is NOT counted"
        } else {
            "the result is counted (stage4a is the counted profile)"
        };
        evidence["diagnostic_note"] = json!(format!("{}; {verdict}", parts.join("; ")));
    }
}

/// The refusal of a failed `--run` (`run_failed`): the run was requested and may have executed
/// quanta, so the evidence must not claim `guest_started: false` / `retired: 0`. `retired` is the
/// count known at the error; when it is not known it is `null` and the detail says so.
fn run_failure(error: &LinuxDirectError, mut evidence: Value) -> Value {
    evidence["guest_started"] = json!(true);
    let mut detail = error.detail.clone();
    match error.retired {
        Some(retired) => evidence["retired"] = json!(retired),
        None => {
            evidence["retired"] = Value::Null;
            detail.push_str("; instructions retired before the error: unknown");
        }
    }
    refusal(error.code, error.stage, detail, evidence)
}

/// Marker detail of [`read_capped`] for a file above the cap.
const TOO_LARGE: &str = "file larger than the limit";

fn read_capped(path: &str, limit: u64) -> Result<Vec<u8>, String> {
    let file = File::open(path).map_err(|error| format!("{path}: {error}"))?;
    let size = file
        .metadata()
        .map_err(|error| format!("{path}: {error}"))?
        .len();
    if size > limit {
        return Err(TOO_LARGE.to_string());
    }
    // The size above is checked before reading; `take` still guards a file that grows meanwhile.
    let mut bytes = Vec::new();
    file.take(limit.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(|error| format!("{path}: {error}"))?;
    if bytes.len() as u64 > limit {
        return Err(TOO_LARGE.to_string());
    }
    Ok(bytes)
}

/// The kernel command line: the S4-29g template with the operator's nonce, `nokaslr`, and the
/// optional Linux user-stack and kernel-stack diagnostics.
fn cmdline(nonce: &str, linux_norandmaps: bool, linux_no_randomize_kstack_offset: bool) -> String {
    let mut base = format!("{CMDLINE_PREFIX} od.nonce={nonce} {CMDLINE_SUFFIX}");
    if linux_norandmaps {
        base.push_str(" norandmaps");
    }
    if linux_no_randomize_kstack_offset {
        base.push_str(" randomize_kstack_offset=off");
    }
    base
}

fn nonce_bootconfig_cmdline() -> String {
    format!("{CMDLINE_PREFIX} {CMDLINE_SUFFIX} bootconfig")
}

/// Linux bootconfig footer (Linux 7.0, Documentation/admin-guide/bootconfig.rst).
/// The caller has already validated the nonce as a single unquoted value.
fn nonce_bootconfig_trailer(nonce: &str, base_bytes: u64) -> Vec<u8> {
    const MAGIC: &[u8] = b"#BOOTCONFIG\n";
    const FOOTER_BYTES: usize = 2 * size_of::<u32>() + MAGIC.len();

    let mut trailer = format!("kernel.od.nonce = {nonce}\n\0").into_bytes();
    let unaligned_bytes = base_bytes % 4 + (trailer.len() + FOOTER_BYTES) as u64;
    let padding = (4 - unaligned_bytes % 4) % 4;
    trailer.resize(trailer.len() + padding as usize, 0);
    let size = trailer.len() as u32;
    let checksum: u32 = trailer.iter().copied().map(u32::from).sum();
    trailer.extend_from_slice(&size.to_le_bytes());
    trailer.extend_from_slice(&checksum.to_le_bytes());
    trailer.extend_from_slice(MAGIC);
    trailer
}

fn merge(target: &mut Value, source: Value) {
    if let (Some(target), Value::Object(source)) = (target.as_object_mut(), source) {
        target.extend(source);
    }
}

fn refusal(code: &str, stage: &str, detail: String, evidence: Value) -> Value {
    let mut value = json!({
        "schema_version": 1,
        "command": "linux-direct",
        "status": "refused",
        "error": {"code": code, "stage": stage, "detail": detail},
    });
    merge(&mut value, evidence);
    value
}

fn machine_refusal(error: &LinuxDirectError, evidence: Value) -> Value {
    refusal(error.code, error.stage, error.detail.clone(), evidence)
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn hex64(value: u64) -> String {
    format!("0x{value:x}")
}

fn kind_name(kind: u32) -> &'static str {
    match kind {
        1 => "ram",
        2 => "reserved",
        3 => "acpi",
        4 => "nvs",
        _ => "unknown",
    }
}

/// Fills the evidence of a loaded board.
fn describe(
    evidence: &mut Value,
    boot: &LinuxDirectBoot,
    cmdline: &str,
    vmlinuz: &[u8],
    loaded_initrd_sha256: &str,
) {
    let header = &boot.header;
    evidence["header"] = json!({
        "version": format!("0x{:04x}", header.version),
        "xloadflags": format!("0x{:04x}", header.xloadflags),
        "relocatable": header.relocatable,
        "kernel_alignment": hex64(u64::from(header.kernel_alignment)),
        "pref_address": hex64(header.pref_address),
        "init_size": hex64(u64::from(header.init_size)),
        "protected_mode_offset": hex64(header.protected_mode_offset),
        "pm_code_size": hex64(header.pm_code_size),
        "payload_offset": hex64(u64::from(header.payload_offset)),
        "payload_length": hex64(u64::from(header.payload_length)),
        "cmdline_size": header.cmdline_size,
        "initrd_addr_max": hex64(u64::from(header.initrd_addr_max)),
    });
    let mut plan = serde_json::Map::new();
    for zone in &boot.zones {
        plan.insert(
            zone.name.to_string(),
            json!({"start": hex64(zone.start), "end": hex64(zone.end)}),
        );
    }
    evidence["plan"] = Value::Object(plan);
    evidence["e820"] = Value::Array(
        boot.e820
            .iter()
            .map(|entry| {
                json!({
                    "addr": hex64(entry.addr),
                    "size": hex64(entry.size),
                    "end": hex64(entry.addr.saturating_add(entry.size)),
                    "type": entry.kind,
                    "type_name": kind_name(entry.kind),
                })
            })
            .collect(),
    );
    evidence["e820_count"] = json!(boot.e820.len());
    evidence["acpi_rsdp_addr"] = json!(hex64(boot.acpi_rsdp_addr));
    evidence["acpi"] = json!({
        "version": boot.acpi.version,
        "manifest_hash": hex(&boot.acpi.manifest_hash),
        "load_base": hex64(boot.acpi.load_base),
        "payload_len": boot.acpi.payload_len,
        "rsdp_address": hex64(boot.acpi.rsdp_address),
        "facs_address": hex64(boot.acpi.facs_address),
        "tables": boot.acpi.tables.iter().map(|table| json!({
            "signature": String::from_utf8_lossy(&table.signature),
            "address": hex64(boot.acpi.load_base + u64::from(table.payload_offset)),
            "length": table.length,
        })).collect::<Vec<_>>(),
    });
    // Read back from guest memory by od-machine (not the input values): the command line as the
    // kernel will see it, the first 8 bytes at `acpi_rsdp_addr`, and the hashes of the kernel
    // and initrd bytes that were really written.
    evidence["cmdline_in_memory"] = json!(String::from_utf8_lossy(&boot.cmdline_in_memory));
    evidence["cmdline_in_memory_matches"] = json!(boot.cmdline_in_memory == cmdline.as_bytes());
    let rsdp_signature = String::from_utf8_lossy(&boot.rsdp_signature_in_memory).into_owned();
    evidence["acpi_rsdp_signature_in_memory"] = json!(rsdp_signature);
    evidence["acpi_rsdp_signature_matches"] = json!(&boot.rsdp_signature_in_memory == b"RSD PTR ");
    let kernel_input = pm_code(header, vmlinuz).map(sha256_hex);
    let kernel_memory = hex(&boot.kernel_sha256_in_memory);
    let initrd_memory = boot.initrd_sha256_in_memory.as_ref().map(|hash| hex(hash));
    // With bootconfig, the input payload is the file plus the generated trailer.
    evidence["memory_sha256"] = json!({
        "kernel_pm_code": kernel_memory,
        "kernel_pm_code_matches_file": kernel_input.as_deref() == Some(kernel_memory.as_str()),
        "initrd": initrd_memory,
        "initrd_matches_file": initrd_memory.as_deref() == Some(loaded_initrd_sha256),
    });
    let entry = &boot.entry;
    evidence["entry"] = json!({
        "rip": hex64(entry.rip),
        "rsi": hex64(entry.rsi),
        "rsp": hex64(entry.rsp),
        "rflags": hex64(entry.rflags),
        "cs": entry.cs.map(|v| format!("0x{v:x}")),
        "ss": entry.ss.map(|v| format!("0x{v:x}")),
        "ds": format!("0x{:x}", entry.ds),
        "es": format!("0x{:x}", entry.es),
        "cr0": entry.cr0.map(hex64),
        "cr3": hex64(entry.cr3),
        "cr4": entry.cr4.map(hex64),
        "efer": hex64(entry.efer),
        "gdtr_base": hex64(entry.gdtr_base),
        "gdtr_limit": hex64(u64::from(entry.gdtr_limit)),
    });
    evidence["retired"] = json!(boot.retired);
}

/// Register names in the order of `LinuxStopRecord::gpr` (`od_isa::Register`).
const GPR_NAMES: [&str; 16] = [
    "rax", "rcx", "rdx", "rbx", "rsp", "rbp", "rsi", "rdi", "r8", "r9", "r10", "r11", "r12", "r13",
    "r14", "r15",
];

/// Space-separated lowercase hex bytes (`fc fa 31 c0`).
fn hex_spaced(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<Vec<_>>()
        .join(" ")
}

/// The stop record of a `--run` (S4-38L1) as JSON.
fn describe_stop(record: &LinuxStopRecord, budget: u64) -> Value {
    let mut gpr = serde_json::Map::new();
    for (name, value) in GPR_NAMES.iter().zip(record.gpr) {
        gpr.insert((*name).to_string(), json!(hex64(value)));
    }
    let inputs = &record.inputs;
    let mut stop = json!({
        "budget": budget,
        "exit_kind": record.exit_kind,
        "exit_detail": record.exit_detail,
        "unsupported_opcode": record.unsupported_opcode.map(|opcode| format!("0x{opcode:02x}")),
        "rip": hex64(record.rip),
        "rip_gpa": record.rip_gpa.map(hex64),
        "rip_bytes": hex_spaced(&record.rip_bytes),
        "rip_bytes_error": record.rip_bytes_error,
        "mode": record.mode,
        "cpl": record.cpl,
        "cr0": record.cr0.map(hex64),
        "cr3": hex64(record.cr3),
        "cr4": record.cr4.map(hex64),
        "efer": hex64(record.efer),
        "rflags": hex64(record.rflags),
        "gpr": Value::Object(gpr),
        "retired": record.retired,
        "quanta": record.quanta,
        "virtual_ns": record.virtual_ns,
        "tsc_hz": record.tsc_hz,
        "com1_hex": hex(&record.com1),
        "com1_text": String::from_utf8_lossy(&record.com1),
        "path": record.path,
        "cpu_profile": if record.path == "cpl0_set" {
            "Cpl0ProfileSet (S2-45) of od-isa under Cpl0BaseLong64V1, 2 MiB walk rebuilt on MOV CR3; \
             members in set_members"
        } else {
            "PagedCpl0ProbeV1 (base paged profile, no CPUID/MSR) with a bare RDTSC \
             (step_paged_probe_with_rdtsc) under the Linux profile, 2 MiB walk"
        },
        "set_members": record.set_members,
        "set_exit_detail": record.set_exit_detail,
        // S4-38L4: the CPUID leaf (EAX) the run stopped before (`exit_kind` `cpuid_divergence`),
        // hex, or null. The CPUID did not execute and nothing was answered for it.
        "cpuid_divergence_leaf": record.cpuid_divergence_leaf.map(|leaf| hex64(u64::from(leaf))),
        "exceptions_delivered": record.exceptions_delivered,
        // S4-38L7: the run had the CPUID boundary off (`--diagnostic-past-c7`): diagnostic only,
        // not counted.
        "diagnostic_past_c7": record.diagnostic_past_c7,
        "cr3_changes": record.cr3_changes,
        "last_cr3_change": record.last_cr3_change.map(|(old, new)| json!({
            "old_cr3": hex64(old),
            "new_cr3": hex64(new),
        })),
        "linux_profile": record.linux_profile,
        "page_fault": record.page_fault,
        "memory_error": record.memory_error,
        "inputs": {
            "vmlinuz_sha256": hex(&inputs.bzimage_sha256),
            "initramfs_sha256": inputs.initrd_sha256.as_ref().map(|hash| hex(hash)),
            "acpi_blob_sha256": hex(&inputs.acpi_blob_sha256),
            "acpi_manifest_hash": hex(&inputs.acpi_manifest_hash),
            "kernel_pm_code_sha256_in_memory": hex(&inputs.kernel_pm_code_sha256_in_memory),
            "initramfs_sha256_in_memory":
                inputs.initrd_sha256_in_memory.as_ref().map(|hash| hex(hash)),
        },
    });
    stop["machine_stop_reason"] = machine_stop_reason_json(record.stop_reason);
    stop["pm_timer_reads"] = json!(record.pm_timer_reads);
    stop["rdtsc_reads"] = json!(record.rdtsc_reads);
    stop["rdtsc_reads_by_cpl"] = json!(record.rdtsc_reads_by_cpl);
    stop["pit_port_b_reads"] = json!(record.pit_port_b_reads);
    stop["pit_counter_port_reads"] = json!(record.pit_counter_port_reads);
    // Keep the vector breakdown outside the large literal to stay under `json!`'s recursion limit.
    stop["page_faults_delivered"] = json!(record.page_faults_delivered);
    stop["general_protection_faults_delivered"] = json!(record.general_protection_faults_delivered);
    // S4-38L9I: the validated frame of the `IRETQ` the run stopped at (`exit_kind`
    // `user_entry`). The key exists only then, so the stop record of every other run (the base
    // DiagnosticV1 run included) is unchanged. The IRETQ did not retire: `rip` is on it. Set
    // apart from the literal above to stay under the `json!` macro recursion limit.
    if let Some(entry) = record.user_entry {
        stop["user_entry"] = json!({
            "rip": hex64(entry.rip),
            "cs": format!("0x{:x}", entry.cs),
            "rflags": hex64(entry.rflags),
            "rsp": hex64(entry.rsp),
            "ss": format!("0x{:x}", entry.ss),
        });
    }
    // S4-39c: under `--user-mode` the stop record also says that the run was one, and how many
    // instructions retired in CPL3 (`cpl` above is the CPL at the stop). The keys exist only then,
    // so the record of every other run is unchanged.
    if record.user_mode {
        stop["user_mode"] = json!(true);
        stop["cpl3_retired"] = json!(record.cpl3_retired);
        // S4-39f: the last system calls (`SYSCALL` nr / return address, `SYSRETQ` result); an
        // entry that has not returned has `rax_return: null`. Only under `--user-mode`.
        stop["syscall_journal"] = json!({
            "capacity": od_machine::SYSCALL_JOURNAL_CAPACITY,
            "entered": record.syscall_journal.entered,
            "returned": record.syscall_journal.returned,
            "entries": record
                .syscall_journal
                .entries
                .iter()
                .map(|entry| json!({
                    "nr": entry.nr,
                    "rip": hex64(entry.rip),
                    "rax_return": entry.rax_return.map(hex64),
                }))
                .collect::<Vec<_>>(),
        });
    }
    // S4-38L8b: the counters of the board interrupt hook; all 0 while the board has no source.
    stop["irqs_delivered"] = json!(record.irqs_delivered);
    stop["irq0_edges"] = json!(record.irq0_edges);
    stop["irq_refused_masked"] = json!(record.irq_refused_masked);
    stop["irq_refused_shadow"] = json!(record.irq_refused_shadow);
    stop["quantum_cut_by_deadline"] = json!(record.quantum_cut_by_deadline);
    // S4-39g: the wake of a CPU halted by HLT (IF = 1) by the PIT -> 8259 source.
    stop["halt_wakes"] = json!(record.halt_wakes);
    stop["halt_idle_ns"] = json!(record.halt_idle_ns);
    // S4-38L7f: the guest-visible IA32_EFER (`efer` is the walker's); set apart from the
    // literal above to stay under the `json!` macro recursion limit.
    stop["guest_efer"] = json!(hex64(record.guest_efer));
    // S4-38L7w: the guest PCI CONFIG_ADDRESS (port 0xCF8).
    stop["pci_config_address"] = json!(hex64(u64::from(record.pci_config_address)));
    // S4-38L7ao: the guest IA32_STAR (write-only for the guest).
    stop["guest_star"] = json!(hex64(record.guest_star));
    // S4-38L7ap: the guest IA32_LSTAR (write-only for the guest).
    stop["guest_lstar"] = json!(hex64(record.guest_lstar));
    // S4-38L7aq: the guest IA32_CSTAR (write-only for the guest).
    stop["guest_cstar"] = json!(hex64(record.guest_cstar));
    // S4-38L7ar: the guest IA32_SYSENTER_CS (write-only for the guest; the full 64-bit value).
    stop["guest_sysenter_cs"] = json!(hex64(record.guest_sysenter_cs));
    // S4-38L7as: the guest IA32_SYSENTER_ESP and IA32_SYSENTER_EIP (write-only for the guest).
    stop["guest_sysenter_esp"] = json!(hex64(record.guest_sysenter_esp));
    stop["guest_sysenter_eip"] = json!(hex64(record.guest_sysenter_eip));
    // S4-38L7at: the guest IA32_SYSCALL_MASK (write-only for the guest; the full 64-bit value).
    stop["guest_syscall_mask"] = json!(hex64(record.guest_syscall_mask));
    // S4-38L7au: the guest IA32_KERNEL_GS_BASE (write-only for the guest; the full 64-bit value).
    stop["guest_kernel_gs_base"] = json!(hex64(record.guest_kernel_gs_base));
    stop
}

fn machine_stop_reason_json(reason: Option<MachineStopReason>) -> Value {
    match reason {
        Some(MachineStopReason::Reset) => json!("reset"),
        Some(MachineStopReason::Shutdown) => json!("shutdown"),
        None => Value::Null,
    }
}

/// The protected-mode code of the bzImage file: the bytes the loader copies to the kernel zone.
fn pm_code<'a>(header: &od_machine::LinuxBzImageHeader, file: &'a [u8]) -> Option<&'a [u8]> {
    let start = usize::try_from(header.protected_mode_offset).ok()?;
    let len = usize::try_from(header.pm_code_size).ok()?;
    file.get(start..start.checked_add(len)?)
}

fn parse(args: &[String]) -> Result<Options, String> {
    let mut vmlinuz = None;
    let mut initramfs = None;
    let mut acpi_blob = None;
    let mut rootfs_raw = None;
    let mut nonce = None;
    let mut nonce_bootconfig = false;
    let mut expected_vmlinuz = None;
    let mut expected_manifest = None;
    let mut expected_initramfs = None;
    let mut ram_mib = None;
    let mut out = None;
    let mut run = false;
    let mut set_cpl0 = false;
    let mut diagnostic_past_c7 = false;
    let mut user_mode = false;
    let mut linux_norandmaps = false;
    let mut linux_no_randomize_kstack_offset = false;
    let mut budget = None;
    let mut vcpus = None;
    let mut seed = None;
    let mut cpuid_profile_text: Option<String> = None;
    let mut checkpoint_table: Option<String> = None;
    let mut stack_context_out = None;
    let mut execution_observations_out = None;
    let mut execution_observations_post_iret_index = None;
    let mut execution_observations_rdtsc_rip = None;
    let mut execution_observations_rdtsc_entry_rip = None;
    let mut execution_observations_rdtsc_stride = None;
    let mut execution_observations_rdtsc_window = None;
    let mut execution_observations_rdtsc_prefix = false;
    let mut execution_observations_delay_call_summary = false;
    let mut execution_observations_quantum_trace = false;
    let mut execution_observations_elf_stack_copy = false;
    let mut index = 0;
    while index < args.len() {
        let name = args[index].as_str();
        // Handle switches before consuming option values.
        if name == "--nonce-bootconfig" {
            if std::mem::replace(&mut nonce_bootconfig, true) {
                return Err("duplicate option --nonce-bootconfig".into());
            }
            index += 1;
            continue;
        }
        if name == "--run" {
            if std::mem::replace(&mut run, true) {
                return Err(format!("duplicate option --run; usage: {USAGE}"));
            }
            index += 1;
            continue;
        }
        if name == "--execution-observations-rdtsc-prefix" {
            if std::mem::replace(&mut execution_observations_rdtsc_prefix, true) {
                return Err(format!(
                    "duplicate option --execution-observations-rdtsc-prefix; usage: {USAGE}"
                ));
            }
            index += 1;
            continue;
        }
        if name == "--execution-observations-delay-call-summary" {
            if std::mem::replace(&mut execution_observations_delay_call_summary, true) {
                return Err(format!(
                    "duplicate option --execution-observations-delay-call-summary; usage: {USAGE}"
                ));
            }
            index += 1;
            continue;
        }
        if name == "--execution-observations-quantum-trace" {
            if std::mem::replace(&mut execution_observations_quantum_trace, true) {
                return Err(format!(
                    "duplicate option --execution-observations-quantum-trace; usage: {USAGE}"
                ));
            }
            index += 1;
            continue;
        }
        if name == "--execution-observations-elf-stack-copy" {
            if std::mem::replace(&mut execution_observations_elf_stack_copy, true) {
                return Err(format!(
                    "duplicate option --execution-observations-elf-stack-copy; usage: {USAGE}"
                ));
            }
            index += 1;
            continue;
        }
        if name == "--set-cpl0" {
            if std::mem::replace(&mut set_cpl0, true) {
                return Err(format!("duplicate option --set-cpl0; usage: {USAGE}"));
            }
            index += 1;
            continue;
        }
        if name == "--diagnostic-past-c7" {
            if std::mem::replace(&mut diagnostic_past_c7, true) {
                return Err(format!(
                    "duplicate option --diagnostic-past-c7; usage: {USAGE}"
                ));
            }
            index += 1;
            continue;
        }
        if name == "--user-mode" {
            if std::mem::replace(&mut user_mode, true) {
                return Err(format!("duplicate option --user-mode; usage: {USAGE}"));
            }
            index += 1;
            continue;
        }
        if name == "--linux-norandmaps" {
            if std::mem::replace(&mut linux_norandmaps, true) {
                return Err(format!(
                    "duplicate option --linux-norandmaps; usage: {USAGE}"
                ));
            }
            index += 1;
            continue;
        }
        if name == "--linux-no-randomize-kstack-offset" {
            if std::mem::replace(&mut linux_no_randomize_kstack_offset, true) {
                return Err(format!(
                    "duplicate option --linux-no-randomize-kstack-offset; usage: {USAGE}"
                ));
            }
            index += 1;
            continue;
        }
        // A value that looks like the next flag is a forgotten value, not a value.
        let value = args
            .get(index + 1)
            .filter(|value| !value.starts_with("--"))
            .ok_or_else(|| format!("флаг {name} требует значение"))?;
        let slot = match name {
            "--vmlinuz" => &mut vmlinuz,
            "--initramfs" => &mut initramfs,
            "--acpi-blob" => &mut acpi_blob,
            "--nonce" => &mut nonce,
            "--expected-vmlinuz-sha256" => &mut expected_vmlinuz,
            "--expected-acpi-manifest-hash" => &mut expected_manifest,
            "--expected-initramfs-sha256" => &mut expected_initramfs,
            "--ram-mib" => &mut ram_mib,
            "--out" => &mut out,
            "--budget" => &mut budget,
            "--vcpus" => &mut vcpus,
            "--rootfs-raw" => &mut rootfs_raw,
            "--seed" => &mut seed,
            "--cpuid-profile" => &mut cpuid_profile_text,
            "--checkpoint-table" => &mut checkpoint_table,
            "--stack-context-out" => &mut stack_context_out,
            "--execution-observations-out" => &mut execution_observations_out,
            "--execution-observations-post-iret-index" => {
                let parsed = value
                    .parse::<u64>()
                    .ok()
                    .filter(|_| !value.starts_with('+'))
                    .ok_or_else(|| {
                        format!("invalid --execution-observations-post-iret-index '{value}'")
                    })?;
                if execution_observations_post_iret_index
                    .replace(parsed)
                    .is_some()
                {
                    return Err(format!("duplicate option {name}; usage: {USAGE}"));
                }
                index += 2;
                continue;
            }
            "--execution-observations-rdtsc-rip" => {
                let hex = value
                    .strip_prefix("0x")
                    .or_else(|| value.strip_prefix("0X"))
                    .filter(|digits| !digits.is_empty())
                    .ok_or_else(|| {
                        format!("invalid --execution-observations-rdtsc-rip '{value}': expected 0x-prefixed hexadecimal")
                    })?;
                let parsed = u64::from_str_radix(hex, 16)
                    .map_err(|_| format!("invalid --execution-observations-rdtsc-rip '{value}'"))?;
                if execution_observations_rdtsc_rip.replace(parsed).is_some() {
                    return Err(format!("duplicate option {name}; usage: {USAGE}"));
                }
                index += 2;
                continue;
            }
            "--execution-observations-rdtsc-entry-rip" => {
                let hex = value
                    .strip_prefix("0x")
                    .or_else(|| value.strip_prefix("0X"))
                    .filter(|digits| !digits.is_empty())
                    .ok_or_else(|| {
                        format!("invalid --execution-observations-rdtsc-entry-rip '{value}': expected 0x-prefixed hexadecimal")
                    })?;
                let parsed = u64::from_str_radix(hex, 16).map_err(|_| {
                    format!("invalid --execution-observations-rdtsc-entry-rip '{value}'")
                })?;
                if execution_observations_rdtsc_entry_rip
                    .replace(parsed)
                    .is_some()
                {
                    return Err(format!("duplicate option {name}; usage: {USAGE}"));
                }
                index += 2;
                continue;
            }
            "--execution-observations-rdtsc-stride" => {
                let parsed = value.parse::<u64>().ok().filter(|stride| *stride > 0).ok_or_else(|| {
                    format!("invalid --execution-observations-rdtsc-stride '{value}': expected a positive integer")
                })?;
                if execution_observations_rdtsc_stride
                    .replace(parsed)
                    .is_some()
                {
                    return Err(format!("duplicate option {name}; usage: {USAGE}"));
                }
                index += 2;
                continue;
            }
            "--execution-observations-rdtsc-window" => {
                let parsed = parse_rdtsc_sample_window(value)?;
                if execution_observations_rdtsc_window
                    .replace(parsed)
                    .is_some()
                {
                    return Err(format!("duplicate option {name}; usage: {USAGE}"));
                }
                index += 2;
                continue;
            }
            other => {
                return Err(format!("unexpected option {other}; usage: {USAGE}"));
            }
        };
        if slot.replace(value.clone()).is_some() {
            return Err(format!("duplicate option {name}; usage: {USAGE}"));
        }
        index += 2;
    }
    let required = |value: Option<String>, name: &str| {
        value.ok_or_else(|| format!("{name} is required; usage: {USAGE}"))
    };
    let budget = match (run, budget) {
        // `u64::from_str` accepts a leading '+'; the budget is plain decimal digits.
        (true, Some(text)) => Some(
            text.parse::<u64>()
                .ok()
                .filter(|_| !text.starts_with('+'))
                .ok_or_else(|| format!("invalid --budget '{text}'"))?,
        ),
        (true, None) => return Err(format!("--run needs --budget; usage: {USAGE}")),
        (false, Some(_)) => return Err(format!("--budget needs --run; usage: {USAGE}")),
        (false, None) => None,
    };
    if set_cpl0 && !run {
        return Err(format!("--set-cpl0 needs --run; usage: {USAGE}"));
    }
    if diagnostic_past_c7 && !set_cpl0 {
        return Err(format!(
            "--diagnostic-past-c7 needs --run and --set-cpl0; usage: {USAGE}"
        ));
    }
    if cpuid_profile_text.is_some() && !set_cpl0 {
        return Err(format!(
            "--cpuid-profile needs --run and --set-cpl0; usage: {USAGE}"
        ));
    }
    if checkpoint_table.is_some() && !set_cpl0 {
        return Err(format!(
            "--checkpoint-table needs --run and --set-cpl0; usage: {USAGE}"
        ));
    }
    let cpuid_profile = match cpuid_profile_text.as_deref() {
        None | Some("diagnostic") => CpuidProfile::DiagnosticV1,
        Some("stage4a") => CpuidProfile::X8664V1Stage4A,
        // S4-38L8d: stage4a plus the APIC bit; the board gets the IOAPIC timer line.
        Some("stage4a-apic") => CpuidProfile::X8664V1Stage4AApic,
        Some(other) => {
            return Err(format!(
                "invalid --cpuid-profile '{other}': diagnostic, stage4a or stage4a-apic; usage: {USAGE}"
            ));
        }
    };
    let decimal = |flag: &str, text: String| {
        text.parse::<u64>()
            .ok()
            .filter(|_| !text.starts_with('+'))
            .ok_or_else(|| format!("invalid {flag} '{text}': expected an unsigned decimal integer"))
    };
    let vcpu_count = match vcpus.map(|text| decimal("--vcpus", text)).transpose()? {
        None | Some(1) => 1,
        Some(2) => 2,
        Some(4) if rootfs_raw.is_some() => 4,
        Some(count) => return Err(format!("unsupported --vcpus {count}: expected 1 or 2")),
    };
    if rootfs_raw.is_some() && !(run && vcpu_count > 1) {
        return Err("--rootfs-raw needs --run with --vcpus 2 or 4".into());
    }
    if run
        && vcpu_count > 1
        && !(seed.is_some()
            && set_cpl0
            && cpuid_profile == CpuidProfile::X8664V1Stage4AApic
            && user_mode)
    {
        return Err(format!(
            "--vcpus {vcpu_count} needs --seed, --set-cpl0, --cpuid-profile stage4a-apic and --user-mode"
        ));
    }
    if vcpu_count > 1
        && (checkpoint_table.is_some()
            || stack_context_out.is_some()
            || execution_observations_out.is_some())
    {
        return Err("checkpoint and observation sidecars require --vcpus 1".into());
    }
    if vcpu_count > 1 && diagnostic_past_c7 {
        return Err("--diagnostic-past-c7 requires --vcpus 1".into());
    }
    let interleaved_seed = seed.map(|text| decimal("--seed", text)).transpose()?;
    if interleaved_seed.is_some()
        && !(budget.is_some_and(|n| n > 0)
            && set_cpl0
            && cpuid_profile.is_stage4a_family()
            && user_mode)
    {
        return Err(
            "--seed needs --run with a positive --budget, --set-cpl0, --cpuid-profile stage4a or stage4a-apic, and --user-mode"
                .into(),
        );
    }
    // S4-39c: the user mode needs the CPUID subset the kernel boots with; `linux_default` (the
    // `diagnostic` profile) never turns it on.
    if user_mode && !cpuid_profile.is_stage4a_family() {
        // S4-38L9-um: `stage4a-apic` has the user mode too (ISSUE-090 for the APIC profile: the
        // CPL3 interrupt goes through the Local APIC IRR and the IDT); its result stays
        // `diagnostic_not_counted`. The diagnostic profile has none.
        return Err(format!(
            "--user-mode needs --run, --set-cpl0 and --cpuid-profile stage4a or stage4a-apic; usage: {USAGE}"
        ));
    }
    if nonce_bootconfig
        && !(run
            && set_cpl0
            && cpuid_profile == CpuidProfile::X8664V1Stage4A
            && user_mode
            && !diagnostic_past_c7
            && !linux_norandmaps
            && !linux_no_randomize_kstack_offset
            && checkpoint_table.is_none())
    {
        return Err(
            "--nonce-bootconfig needs the counted --run --set-cpl0 --cpuid-profile stage4a --user-mode profile without Linux diagnostics or a checkpoint table".into(),
        );
    }
    if linux_norandmaps
        && !(run && set_cpl0 && cpuid_profile == CpuidProfile::X8664V1Stage4A && user_mode)
    {
        return Err(
            "--linux-norandmaps needs --run, --set-cpl0, --cpuid-profile stage4a and --user-mode"
                .into(),
        );
    }
    if linux_no_randomize_kstack_offset
        && !(run && set_cpl0 && cpuid_profile == CpuidProfile::X8664V1Stage4A && user_mode)
    {
        return Err(
            "--linux-no-randomize-kstack-offset needs --run, --set-cpl0, --cpuid-profile stage4a and --user-mode"
                .into(),
        );
    }
    if stack_context_out.is_some() && !(run && set_cpl0) {
        return Err(format!(
            "--stack-context-out needs --run and --set-cpl0; usage: {USAGE}"
        ));
    }
    if execution_observations_out.is_some() && !(run && set_cpl0 && user_mode) {
        return Err(format!(
            "--execution-observations-out needs --run, --set-cpl0 and --user-mode; usage: {USAGE}"
        ));
    }
    if execution_observations_rdtsc_prefix && execution_observations_out.is_none() {
        return Err(format!(
            "--execution-observations-rdtsc-prefix needs --execution-observations-out; usage: {USAGE}"
        ));
    }
    if execution_observations_delay_call_summary {
        if execution_observations_out.is_none() {
            return Err(format!(
                "--execution-observations-delay-call-summary needs --execution-observations-out; usage: {USAGE}"
            ));
        }
        if execution_observations_rdtsc_rip
            .is_some_and(|rip| rip != od_machine::LINUX_DELAY_TSC_POLL_RIP)
            || execution_observations_rdtsc_entry_rip
                .is_some_and(|rip| rip != od_machine::LINUX_DELAY_TSC_ENTRY_RIP)
            || execution_observations_rdtsc_stride.is_some_and(|stride| stride != 1)
            || execution_observations_rdtsc_window.is_some()
        {
            return Err("--execution-observations-delay-call-summary conflicts with selected RIP, stride, or window options".into());
        }
        execution_observations_rdtsc_rip = Some(od_machine::LINUX_DELAY_TSC_POLL_RIP);
        execution_observations_rdtsc_entry_rip = Some(od_machine::LINUX_DELAY_TSC_ENTRY_RIP);
        execution_observations_rdtsc_stride = Some(1);
    }
    if execution_observations_quantum_trace && execution_observations_out.is_none() {
        return Err(format!(
            "--execution-observations-quantum-trace needs --execution-observations-out; usage: {USAGE}"
        ));
    }
    if execution_observations_elf_stack_copy && execution_observations_out.is_none() {
        return Err(format!(
            "--execution-observations-elf-stack-copy needs --execution-observations-out; usage: {USAGE}"
        ));
    }
    if execution_observations_post_iret_index.is_some() && execution_observations_out.is_none() {
        return Err(format!(
            "--execution-observations-post-iret-index needs --execution-observations-out; usage: {USAGE}"
        ));
    }
    if execution_observations_rdtsc_rip.is_some() && execution_observations_out.is_none() {
        return Err(format!(
            "--execution-observations-rdtsc-rip needs --execution-observations-out; usage: {USAGE}"
        ));
    }
    if execution_observations_rdtsc_entry_rip.is_some()
        && (execution_observations_out.is_none() || execution_observations_rdtsc_rip.is_none())
    {
        return Err(format!(
            "--execution-observations-rdtsc-entry-rip needs --execution-observations-out and --execution-observations-rdtsc-rip; usage: {USAGE}"
        ));
    }
    if execution_observations_rdtsc_entry_rip.is_some()
        && execution_observations_rdtsc_entry_rip == execution_observations_rdtsc_rip
    {
        return Err("entry and selected RDTSC RIPs must differ".into());
    }
    if execution_observations_rdtsc_stride.is_some()
        && (execution_observations_out.is_none() || execution_observations_rdtsc_rip.is_none())
    {
        return Err(format!(
            "--execution-observations-rdtsc-stride needs --execution-observations-out and --execution-observations-rdtsc-rip; usage: {USAGE}"
        ));
    }
    if execution_observations_rdtsc_window.is_some()
        && (execution_observations_out.is_none() || execution_observations_rdtsc_rip.is_none())
    {
        return Err(format!(
            "--execution-observations-rdtsc-window needs --execution-observations-out and --execution-observations-rdtsc-rip; usage: {USAGE}"
        ));
    }
    if let Some(window) = execution_observations_rdtsc_window {
        validate_rdtsc_sample_window_capacity(
            window,
            execution_observations_rdtsc_stride.unwrap_or(1),
        )?;
    }
    // The board loop accepts both observers, but the public execution-observation runs of
    // od-machine do not take a checkpoint table and the pair is untested (ISSUE-505); refuse it
    // explicitly rather than drop one of the two.
    if execution_observations_out.is_some() && checkpoint_table.is_some() {
        return Err(format!(
            "--execution-observations-out cannot be combined with --checkpoint-table; usage: {USAGE}"
        ));
    }
    if stack_context_out.is_some()
        && stack_context_out.as_ref() == execution_observations_out.as_ref()
    {
        return Err("diagnostic sidecar paths must be distinct".into());
    }
    let nonce = required(nonce, "--nonce")?;
    if nonce.is_empty()
        || nonce.chars().count() > MAX_NONCE_CHARS
        || !nonce
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
    {
        return Err(format!(
            "invalid --nonce: 1..={MAX_NONCE_CHARS} characters from [A-Za-z0-9._-]"
        ));
    }
    let ram_text = required(ram_mib, "--ram-mib")?;
    let ram_mib = ram_text
        .parse::<u64>()
        .ok()
        .filter(|mib| *mib > 0)
        .ok_or_else(|| format!("invalid --ram-mib '{ram_text}'"))?;
    if ram_mib > MAX_RAM_MIB {
        return Err(format!(
            "--ram-mib {ram_mib} is above the limit of {MAX_RAM_MIB} MiB: the board builds \
             firmware boot tables with 4 KiB leaves, which takes minutes and gigabytes of memory \
             on a larger RAM"
        ));
    }
    Ok(Options {
        vmlinuz: required(vmlinuz, "--vmlinuz")?,
        initramfs: required(initramfs, "--initramfs")?,
        acpi_blob: required(acpi_blob, "--acpi-blob")?,
        rootfs_raw,
        nonce,
        nonce_bootconfig,
        expected_vmlinuz_sha256: parse_sha256(&required(
            expected_vmlinuz,
            "--expected-vmlinuz-sha256",
        )?)?,
        expected_acpi_manifest_hash: parse_hash(&required(
            expected_manifest,
            "--expected-acpi-manifest-hash",
        )?)?,
        expected_initramfs_sha256: parse_sha256(&required(
            expected_initramfs,
            "--expected-initramfs-sha256",
        )?)?,
        ram_mib,
        out: required(out, "--out")?,
        budget,
        vcpu_count,
        interleaved_seed,
        set_cpl0,
        diagnostic_past_c7,
        cpuid_profile,
        user_mode,
        linux_norandmaps,
        linux_no_randomize_kstack_offset,
        checkpoint_table,
        stack_context_out,
        execution_observations_out,
        execution_observations_post_iret_index,
        execution_observations_rdtsc_rip,
        execution_observations_rdtsc_entry_rip,
        execution_observations_rdtsc_stride,
        execution_observations_rdtsc_window,
        execution_observations_rdtsc_prefix,
        execution_observations_delay_call_summary,
        execution_observations_quantum_trace,
        execution_observations_elf_stack_copy,
    })
}

pub(super) fn parse_rdtsc_sample_window(
    value: &str,
) -> Result<od_machine::RdtscSampleWindow, String> {
    let (first, last) = value.split_once(':').ok_or_else(|| {
        format!("invalid --execution-observations-rdtsc-window '{value}': expected START:END")
    })?;
    if last.contains(':') {
        return Err(format!(
            "invalid --execution-observations-rdtsc-window '{value}': expected START:END"
        ));
    }
    let parse = |part: &str| part.parse::<u64>().ok().filter(|_| !part.starts_with('+'));
    let first_hit = parse(first).ok_or_else(|| {
        format!("invalid --execution-observations-rdtsc-window '{value}': START must be positive")
    })?;
    let last_hit = parse(last).ok_or_else(|| {
        format!("invalid --execution-observations-rdtsc-window '{value}': END must be positive")
    })?;
    if first_hit == 0 || last_hit < first_hit {
        return Err(format!(
            "invalid --execution-observations-rdtsc-window '{value}': require 1 <= START <= END"
        ));
    }
    Ok(od_machine::RdtscSampleWindow {
        first_hit,
        last_hit,
    })
}

pub(super) fn validate_rdtsc_sample_window_capacity(
    window: od_machine::RdtscSampleWindow,
    stride: u64,
) -> Result<(), String> {
    let stride = stride.max(1);
    let candidates = window.last_hit / stride - (window.first_hit - 1) / stride;
    if candidates > od_machine::LINUX_RDTSC_SAMPLE_CAPACITY as u64 {
        return Err(format!(
            "--execution-observations-rdtsc-window selects {candidates} samples at stride {stride}, exceeding the {} sample cap",
            od_machine::LINUX_RDTSC_SAMPLE_CAPACITY
        ));
    }
    Ok(())
}

fn match_existing_file_identity(first: &str, second: &str) -> Result<bool, String> {
    let first_metadata = match std::fs::metadata(first) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(format!("could not inspect {first}: {error}")),
    };
    let second_metadata = match std::fs::metadata(second) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(format!("could not inspect {second}: {error}")),
    };
    metadata_is_same_file(&first_metadata, &second_metadata, first, second)
}

fn file_matches_existing_path(
    file: &File,
    reservation_path: &str,
    path: &str,
) -> Result<bool, String> {
    let reserved_metadata = file.metadata().map_err(|error| {
        format!("could not inspect reserved sidecar {reservation_path}: {error}")
    })?;
    let reservation_path_metadata = std::fs::metadata(reservation_path).map_err(|error| {
        format!("could not inspect reserved sidecar path {reservation_path}: {error}")
    })?;
    if !metadata_is_same_file(
        &reserved_metadata,
        &reservation_path_metadata,
        reservation_path,
        reservation_path,
    )? {
        return Err("sidecar reservation path changed after it was opened".into());
    }
    match_existing_file_identity(reservation_path, path)
}

#[cfg(unix)]
fn metadata_is_same_file(
    first: &std::fs::Metadata,
    second: &std::fs::Metadata,
    _first_path: &str,
    _second_path: &str,
) -> Result<bool, String> {
    use std::os::unix::fs::MetadataExt;
    Ok(first.dev() == second.dev() && first.ino() == second.ino())
}

#[cfg(not(unix))]
fn metadata_is_same_file(
    _first: &std::fs::Metadata,
    _second: &std::fs::Metadata,
    first_path: &str,
    second_path: &str,
) -> Result<bool, String> {
    let first = std::fs::canonicalize(first_path)
        .map_err(|error| format!("could not resolve {first_path}: {error}"))?;
    let second = std::fs::canonicalize(second_path)
        .map_err(|error| format!("could not resolve {second_path}: {error}"))?;
    Ok(first == second)
}

fn discard_sidecar_reservation(path: &str, file: File) -> Result<(), String> {
    if !file_matches_existing_path(&file, path, path)? {
        return Err("sidecar reservation path identity changed".into());
    }
    drop(file);
    std::fs::remove_file(path).map_err(|error| format!("could not remove {path}: {error}"))
}

fn cleanup_sidecar_reservation(path: Option<&str>, file: &mut Option<File>) -> Result<(), String> {
    let Some(reserved_file) = file.take() else {
        return Ok(());
    };
    let path = path.ok_or_else(|| "sidecar reservation has no path".to_string())?;
    discard_sidecar_reservation(path, reserved_file)
}

fn write_stack_context_sidecar(
    file: &mut File,
    path: &str,
    snapshots: &[od_machine::ConsoleOutputContextSnapshot],
    uart_diagnostic: Option<&od_machine::UartOutputContextDiagnostic>,
) -> Result<(), String> {
    let value = json!({
        "schema_version": 2,
        "capture_scope": "console-output and bounded UART byte context",
        "unwind_context": false,
        "limits": {
            "snapshots": 4,
            "line_bytes": 192,
            "frame_links": 32,
            "stack_window_bytes": 4096
        },
        "snapshots": snapshots.iter().map(|snapshot| json!({
            "trigger": match snapshot.trigger {
                od_machine::ConsoleContextTrigger::CallTraceLine => "call_trace_line",
                od_machine::ConsoleContextTrigger::OptionalFrameLine => "optional_frame_line",
            },
            "rip": hex64(snapshot.rip),
            "rbp": hex64(snapshot.rbp),
            "cr3": hex64(snapshot.cr3),
            "frames": snapshot.frames.iter().map(|frame| json!({
                "frame_pointer": hex64(frame.frame_pointer),
                "return_address": hex64(frame.return_address),
            })).collect::<Vec<_>>(),
            "ended": frame_chain_end_name(snapshot.ended),
        })).collect::<Vec<_>>(),
        "uart_provenance": uart_diagnostic.map(|capture| json!({
            "trigger": "first_OD_NONCE_equals",
            "marker_offset": capture.marker_offset,
            "marker_prefix_before_trigger_unobserved": capture.marker_prefix_before_trigger_unobserved,
            "end": uart_context_end_name(capture.end),
            "byte_limit": od_machine::MAX_UART_PROVENANCE_BYTES,
            "samples": capture.samples.iter().map(|sample| json!({
                "offset": sample.offset,
                "byte": sample.byte,
                "retired": sample.retired,
                "rip_phase": "post_out_instruction_boundary",
                "rip": hex64(sample.rip),
                "rsp": hex64(sample.rsp),
                "rbp": hex64(sample.rbp),
                "cr3": hex64(sample.cr3),
                "cs_selector": sample.cs_selector.map(|selector| format!("0x{selector:04x}")),
                "cpl": sample.cs_selector.map(|selector| selector & 3),
                "frames": sample.frames.iter().map(|frame| json!({
                    "frame_pointer": hex64(frame.frame_pointer),
                    "return_address": hex64(frame.return_address),
                })).collect::<Vec<_>>(),
                "ended": frame_chain_end_name(sample.ended),
            })).collect::<Vec<_>>(),
        })),
    });
    file_matches_existing_path(file, path, path)?;
    let bytes = serde_json::to_vec_pretty(&value).map_err(|error| error.to_string())?;
    use std::io::Write;
    if let Err(error) = file.write_all(&bytes).and_then(|()| file.write_all(b"\n")) {
        let cleanup = match file_matches_existing_path(file, path, path) {
            Ok(true) => {
                std::fs::remove_file(path).map_err(|cleanup_error| cleanup_error.to_string())
            }
            Ok(false) => Err("reservation path identity changed".into()),
            Err(cleanup_error) => Err(cleanup_error),
        };
        return Err(match cleanup {
            Ok(()) => format!("{path}: {error}"),
            Err(cleanup_error) => {
                format!("{path}: {error}; partial sidecar cleanup failed: {cleanup_error}")
            }
        });
    }
    Ok(())
}

/// Environment variable that turns the fetch cache off for a diagnostic run (S4-13).
const NO_FETCH_CACHE_ENV: &str = "OD_NO_FETCH_CACHE";
/// Environment variable with the path of the opt-in fetch cache sidecar (S4-13b).
const FETCH_CACHE_STATS_ENV: &str = "OD_FETCH_CACHE_STATS";
/// Environment variable that adds the shadow cache sweep (S4-13c) to the fetch cache sidecar.
const FETCH_CACHE_SWEEP_ENV: &str = "OD_FETCH_CACHE_SWEEP";
/// Environment variable that gives the real fetch cache the 2 MiB leaf tag (S4-13d); only `2m`
/// does, unset, empty and any other value keep the 4 KiB tag.
const FETCH_CACHE_TAG_ENV: &str = "OD_FETCH_CACHE_TAG";

/// Whether the 2 MiB leaf tag is requested: the value is exactly `2m`.
fn fetch_cache_leaf_2m_tag_enabled(value: Option<&std::ffi::OsStr>) -> bool {
    value.is_some_and(|value| value == "2m")
}

/// What a run does with `OD_FETCH_CACHE_TAG`: whether the 2 MiB tag is applied and the warning
/// to print. Like `OD_FETCH_CACHE_SWEEP` the tag is a diagnostic that rides on the sidecar: with
/// `2m` but without `OD_FETCH_CACHE_STATS` it is NOT applied (the default 4 KiB tag stays) and
/// the warning says so.
fn fetch_cache_leaf_2m_tag_decision(
    tag: Option<&std::ffi::OsStr>,
    sidecar_requested: bool,
) -> (bool, Option<String>) {
    if !fetch_cache_leaf_2m_tag_enabled(tag) {
        return (false, None);
    }
    if sidecar_requested {
        return (true, None);
    }
    (
        false,
        Some(format!(
            "warning: {FETCH_CACHE_TAG_ENV}=2m is ignored without {FETCH_CACHE_STATS_ENV} \
             (the 2 MiB tag is a diagnostic that needs the sidecar); the 4 KiB tag stays"
        )),
    )
}

/// Whether the sweep is requested: set, not empty and not `0`.
fn fetch_cache_sweep_enabled(value: Option<&std::ffi::OsStr>) -> bool {
    value.is_some_and(|value| !value.is_empty() && value != "0")
}

/// Whether the run keeps the fetch translation cache, from the value of `OD_NO_FETCH_CACHE`
/// (S4-13b). Unset, empty or `0` keep it (the default); any other value turns it off. Before
/// S4-13b any value, an empty one and `0` included, turned it off.
fn fetch_cache_enabled(no_fetch_cache: Option<&std::ffi::OsStr>) -> bool {
    match no_fetch_cache {
        None => true,
        Some(value) => value.is_empty() || value == "0",
    }
}

/// The sidecar path from the value of `OD_FETCH_CACHE_STATS`: unset or empty is no sidecar; a
/// value that is not UTF-8 is refused (the report paths are strings).
fn fetch_cache_stats_path(value: Option<std::ffi::OsString>) -> Result<Option<String>, String> {
    match value {
        None => Ok(None),
        Some(value) if value.is_empty() => Ok(None),
        Some(value) => value
            .into_string()
            .map(Some)
            .map_err(|_| format!("{FETCH_CACHE_STATS_ENV} is not valid UTF-8")),
    }
}

fn per_instruction(count: u64, retired: u64) -> Value {
    if retired == 0 {
        Value::Null
    } else {
        json!(count as f64 / retired as f64)
    }
}

fn fetch_cache_phase_json(phase: &od_machine::LinuxFetchCachePhase) -> Value {
    let (walks, cache) = (&phase.walks, &phase.cache);
    let lookups = cache.hits + cache.misses;
    json!({
        "calls": phase.calls,
        "retired": phase.retired,
        "walks": {
            "fetch": walks.fetch,
            "data_read": walks.data_read,
            "data_write": walks.data_write,
        },
        "walks_per_instruction": {
            "fetch": per_instruction(walks.fetch, phase.retired),
            "data_read": per_instruction(walks.data_read, phase.retired),
            "data_write": per_instruction(walks.data_write, phase.retired),
        },
        "cache": {
            "hits": cache.hits,
            "misses": cache.misses,
            "inserts": cache.inserts,
            "write_flushes": cache.write_flushes,
            "context_flushes": cache.context_flushes,
            "host_flushes": cache.host_flushes,
            "invlpg_flushes": cache.invlpg_flushes,
            "control_flushes": cache.control_flushes,
            "fault_flushes": cache.fault_flushes,
            "ignored_table_writes": cache.ignored_table_writes,
            "lookups_per_instruction": per_instruction(lookups, phase.retired),
            "hit_rate": if lookups == 0 { Value::Null } else { json!(cache.hits as f64 / lookups as f64) },
        },
    })
}

/// The content of the `OD_FETCH_CACHE_STATS` sidecar (S4-13b). `run_completed` is false when the
/// run ended in `run_failed`: the counters are then what was counted up to the failure.
fn fetch_cache_stats_json(
    report: &od_machine::LinuxFetchCacheReport,
    sweep: Option<&od_machine::FetchSweepReport>,
    run_completed: bool,
    run_wall_seconds: f64,
) -> Value {
    let mut value = json!({
        "schema_version": 1,
        "capture_scope": "fetch translation cache counters and tracked page walks of the CPL0 set loop, summed over its calls; measurement only",
        "fetch_cache_enabled": report.cache_enabled,
        "fetch_cache_tag": if report.leaf_2m_tag { "2m" } else { "4k" },
        "run_completed": run_completed,
        "run_wall_seconds": run_wall_seconds,
        "phase_rule": report.phase_rule,
        "c5_reached": report.c5_reached,
        "notes": [
            "walks: tracked walks of the instruction adapter only (fetch bytes not served by the cache, data reads, data writes); the read-only peeks and the cache fill walk are not counted; exception delivery (#PF, #GP: IDT, GDT, TSS and frame accesses) is counted as data reads and writes; external interrupt delivery is not counted",
            "cache: counters of one cache per call (quantum), summed; its lookups include the peeks of the loop",
            "phase 1 is before the boundary at which checkpoint C5 is recorded, phase 2 from it on; without the checkpoint recorder nothing is split",
        ],
        "total": fetch_cache_phase_json(&report.total),
        "before_c5": fetch_cache_phase_json(&report.before_c5),
        "after_c5": fetch_cache_phase_json(&report.after_c5),
    });
    // S4-13c: present only when the sweep was asked for.
    if let Some(sweep) = sweep {
        value["fetch_cache_sweep"] = fetch_cache_sweep_json(sweep, report);
    }
    value
}

/// The counters of one cache (real or shadow) in a phase.
fn sweep_counters_json(stats: &od_machine::FetchCacheStats) -> Value {
    let lookups = stats.hits + stats.misses;
    json!({
        "hits": stats.hits,
        "misses": stats.misses,
        "inserts": stats.inserts,
        "write_flushes": stats.write_flushes,
        "context_flushes": stats.context_flushes,
        "host_flushes": stats.host_flushes,
        "invlpg_flushes": stats.invlpg_flushes,
        "control_flushes": stats.control_flushes,
        "fault_flushes": stats.fault_flushes,
        "ignored_table_writes": stats.ignored_table_writes,
        "hit_rate": if lookups == 0 { Value::Null } else { json!(stats.hits as f64 / lookups as f64) },
    })
}

/// The `fetch_cache_sweep` key of the sidecar (S4-13c): every shadow's counters by phase, and the
/// check that the reference shadow counted exactly like the real cache of the run.
fn fetch_cache_sweep_json(
    sweep: &od_machine::FetchSweepReport,
    report: &od_machine::LinuxFetchCacheReport,
) -> Value {
    use od_machine::{SweepInvalidation, SweepLifecycle, SweepReplacement, SweepTag};
    let configs: serde_json::Map<String, Value> = sweep
        .configs
        .iter()
        .map(|entry| {
            let config = &entry.config;
            (
                entry.label.clone(),
                json!({
                    "capacity": config.capacity,
                    "replacement": match config.replacement {
                        SweepReplacement::Fifo => "fifo",
                        SweepReplacement::Lru => "lru",
                    },
                    "tag": match config.tag {
                        SweepTag::Page4K => "4k_page",
                        SweepTag::Leaf2M => "2m_leaf",
                    },
                    "lifecycle": match config.lifecycle {
                        SweepLifecycle::PerCall => "per_call",
                        SweepLifecycle::Persistent => "persistent",
                    },
                    "invalidation": match config.invalidation {
                        SweepInvalidation::WriteWatch => "write_watch",
                        SweepInvalidation::GuestOnly => "guest_only",
                    },
                    "phase1_before_c5": sweep_counters_json(&entry.before_c5),
                    "phase2_after_c5": sweep_counters_json(&entry.after_c5),
                    "total": sweep_counters_json(&entry.total),
                }),
            )
        })
        .collect();
    let reference = sweep.find(od_machine::SweepConfig::REFERENCE);
    let self_check = match reference {
        Some(reference) => json!({
            "reference": reference.label,
            "total_equals_real": reference.total == report.total.cache,
            "phase1_equals_real": reference.before_c5 == report.before_c5.cache,
            "phase2_equals_real": reference.after_c5 == report.after_c5.cache,
            "all_ten_counters_compared": true,
        }),
        None => Value::Null,
    };
    json!({
        "schema_version": 1,
        "capture_scope": "shadow fetch caches fed with the event stream of the real cache; diagnostics only, their output is not part of any board report",
        "c5_reached": sweep.c5_reached,
        "notes": [
            "each shadow counts the ten counters of the real cache; phase 1 is before the boundary of checkpoint C5, phase 2 from it on",
            "per_call: emptied at the start of every call (quantum), like the real cache of the board; persistent: kept across calls with the S4-14i rules (a changed translation context, or a write to a watched table frame outside the loop, flushes it)",
            "4k_page: one entry tags one 4 KiB page (the real cache); 2m_leaf: a walk that ended at a 2 MiB leaf is one entry for the 2 MiB page",
            "write_watch: a committed write to a table frame of an entry drops every entry (the real cache); guest_only: such a write is only counted, INVLPG, a delivered #PF and a change of CR0/CR4 flush. guest_only is a hypothesis: a shadow keeps serving translations the run itself would not trust, and the writes of the host between calls are seen only through the page generations",
            "INVLPG for the guest_only shadows is detected by a read-only peek at RIP that bypasses the real cache",
            "a real GuestOnly cache finds INVLPG by a peek through the cache (the Peek lookup is added to its hits/misses); a guest_only shadow under a WriteWatch run bypasses the cache, so the hits/misses of a guest_only shadow on the board can differ from those of a real GuestOnly cache; the set of flushing events (INVLPG, #PF, CR0/CR4, MOV CR3) is the same",
            "the shadows receive the lookup trace of the run and the walk result of the real cache; this equals an independent cache only for a WriteWatch run; on a fetch miss each shadow decides hit or miss itself and takes the insert only if it missed",
        ],
        "events": {
            "calls": sweep.events.calls,
            "lookups": sweep.events.lookups,
            "bulk_lookups": sweep.events.bulk_lookups,
            "inserts": sweep.events.inserts,
            "writes": sweep.events.writes,
            "writes_checked_against_shadows": sweep.events.writes_checked,
        },
        "watched_table_pages": sweep.watched_table_pages,
        "distinct_translation_contexts": sweep.distinct_contexts,
        "self_check": self_check,
        "configs": configs,
    })
}

fn write_fetch_cache_stats_sidecar(
    file: &mut File,
    path: &str,
    report: &od_machine::LinuxFetchCacheReport,
    sweep: Option<&od_machine::FetchSweepReport>,
    run_completed: bool,
    run_wall_seconds: f64,
    final_ram_sha256: Result<[u8; 32], String>,
) -> Result<(), String> {
    let mut value = fetch_cache_stats_json(report, sweep, run_completed, run_wall_seconds);
    // S4-13d: the end state of guest RAM, to compare two runs of the same guest.
    // A hash that could not be computed is `null` with the reason, never a hash of part of RAM.
    match &final_ram_sha256 {
        Ok(hash) => value["final_ram_sha256"] = json!(hex(hash)),
        Err(reason) => {
            value["final_ram_sha256"] = Value::Null;
            value["final_ram_sha256_error"] = json!(reason);
        }
    }
    file_matches_existing_path(file, path, path)?;
    let bytes = serde_json::to_vec_pretty(&value).map_err(|error| error.to_string())?;
    use std::io::Write;
    file.write_all(&bytes)
        .and_then(|()| file.write_all(b"\n"))
        .map_err(|error| format!("{path}: {error}"))
}

fn write_execution_observations_sidecar(
    file: &mut File,
    path: &str,
    observations: &LinuxExecutionObservations,
) -> Result<(usize, String), String> {
    let alignment_point_json = |point: &Option<od_machine::ElfStackAlignmentPoint>| {
        point.as_ref().map(|point| {
            json!({
                "rip": hex64(point.rip),
                "retired_instructions_before_current": point.retired_instructions_before_current,
                "cr3": hex64(point.cr3),
                "cs_selector": point.cs_selector.map(|value| format!("0x{value:04x}")),
                "cpl": point.cpl,
                "r15": hex64(point.r15),
                "rbp": hex64(point.rbp),
                "rsp": hex64(point.rsp),
                "pointer": hex64(point.pointer),
                "instruction_bytes": point.instruction_bytes.as_ref().map(|bytes| bytes.iter().map(|byte| format!("{byte:02x}")).collect::<String>()),
                "error": point.error,
            })
        })
    };
    let elf_stack_copy = observations.elf_stack_copy.as_ref().map(|snapshot| {
        json!({
            "profile": snapshot.profile,
            "status": elf_stack_copy_status_name(snapshot.status),
            "rip": snapshot.rip.map(hex64),
            "cr3": snapshot.cr3.map(hex64),
            "cs_selector": snapshot.cs_selector.map(|value| format!("0x{value:04x}")),
            "cpl": snapshot.cpl,
            "registers": {
                "r15": snapshot.r15.map(hex64),
                "rbp": snapshot.rbp.map(hex64),
                "rsp": snapshot.rsp.map(hex64),
                "r13": snapshot.r13.map(hex64),
                "rdi": snapshot.rdi.map(hex64),
                "rsi": snapshot.rsi.map(hex64),
                "rdx": snapshot.rdx.map(hex64),
            },
            "bprm_p_address": snapshot.bprm_p_address.map(hex64),
            "bprm_p": snapshot.bprm_p.map(hex64),
            "aligned_stack_pointer": snapshot.aligned_stack_pointer.map(hex64),
            "copy_destination": snapshot.copy_destination.map(hex64),
            "copy_length": snapshot.copy_length.map(hex64),
            "instruction_bytes": snapshot.instruction_bytes.as_ref().map(|bytes| bytes.iter().map(|byte| format!("{byte:02x}")).collect::<String>()),
            "return_rip": snapshot.return_rip.map(hex64),
            "platform_string_matches": snapshot.platform_string_matches,
            "alignment": {
                "status": snapshot.alignment.status,
                "before_call": alignment_point_json(&snapshot.alignment.before_call),
                "after_return": alignment_point_json(&snapshot.alignment.after_return),
                "before_omitted": snapshot.alignment.before_omitted,
                "after_omitted": snapshot.alignment.after_omitted,
                "matched_copy": snapshot.alignment.matched_copy,
            },
            "pre_copy_cr3_ledger": snapshot.pre_copy_cr3_ledger.as_ref().map(|ledger| json!({
                "status": match ledger.status {
                    od_machine::ElfStackCopyCr3LedgerStatus::Complete => "complete",
                    od_machine::ElfStackCopyCr3LedgerStatus::IncompleteOverflow => "incomplete_overflow",
                    od_machine::ElfStackCopyCr3LedgerStatus::RunEndedBeforeTarget => "run_ended_before_target",
                },
                "event_count": ledger.event_count,
                "events_omitted": ledger.events_omitted,
                "events": ledger.events.iter().map(|event| json!({
                    "sequence": event.sequence,
                    "rip": hex64(event.rip),
                    "old_cr3": hex64(event.old_cr3),
                    "new_cr3": hex64(event.new_cr3),
                    "cpl": event.cpl,
                    "retired_instructions_through_current": event.retired_instructions_through_current,
                })).collect::<Vec<_>>(),
                "boundary_markers": ledger.boundary_markers.iter().map(|marker| json!({
                    "name": marker.name,
                    "rip": hex64(marker.rip),
                    "retired_instructions_before_current": marker.retired_instructions_before_current,
                })).collect::<Vec<_>>(),
                "target_event": {
                    "status": ledger.target_event.status,
                    "sequence": ledger.target_event.sequence,
                    "rip": ledger.target_event.rip.map(hex64),
                    "cr3": ledger.target_event.cr3.map(hex64),
                    "retired_instructions": ledger.target_event.retired_instructions,
                    "rsp": ledger.target_event.rsp.map(hex64),
                    "rbp": ledger.target_event.rbp.map(hex64),
                    "chain_status": ledger.target_event.chain_status.map(|status| match status {
                        od_machine::ElfStackCopyTargetCr3ChainStatus::Complete => "complete",
                        od_machine::ElfStackCopyTargetCr3ChainStatus::Unreadable => "unreadable",
                        od_machine::ElfStackCopyTargetCr3ChainStatus::Malformed => "malformed",
                        od_machine::ElfStackCopyTargetCr3ChainStatus::FrameLimit => "frame_limit",
                    }),
                    "frames": ledger.target_event.frames.iter().map(|frame| json!({
                        "rbp": hex64(frame.rbp),
                        "saved_rbp_slot": hex64(frame.saved_rbp_slot),
                        "saved_rbp": hex64(frame.saved_rbp),
                        "return_address_slot": hex64(frame.return_address_slot),
                        "return_address": hex64(frame.return_address),
                    })).collect::<Vec<_>>(),
                    "error": ledger.target_event.error,
                },
                "mmu_gather": {
                    "status": elf_mmu_gather_capture_status(&ledger.mmu_gather),
                    "association": "prefix_candidates",
                    "copy_destination": ledger.mmu_gather.copy_destination.map(hex64),
                    "destination_in_candidate_pages": ledger.mmu_gather.destination_in_candidate_pages,
                    "setup_count": ledger.mmu_gather.setup_count,
                    "setups_omitted": ledger.mmu_gather.setups_omitted,
                    "setups": ledger.mmu_gather.setups.iter().map(elf_mmu_gather_setup_json).collect::<Vec<_>>(),
                    "unpaired_omitted": ledger.mmu_gather.unpaired_omitted,
                    "unpaired": ledger.mmu_gather.unpaired.iter().map(elf_mmu_gather_unpaired_json).collect::<Vec<_>>(),
                },
            })),
            "invocation": snapshot.invocation.as_ref().map(elf_stack_copy_invocation_json),
            "mismatch_reasons": snapshot.mismatch_reasons,
            "error": snapshot.error,
            "probes": snapshot.probes.iter().map(|probe| json!({
                "name": probe.name,
                "address": hex64(probe.address),
                "page_base": hex64(probe.page_base),
                "access": probe.access,
                "cpl": probe.cpl,
                "entries": probe.entries.iter().map(|entry| json!({
                    "level": format!("{:?}", entry.level),
                    "index": entry.index,
                    "entry_address": hex64(entry.entry_address.get()),
                    "entry_value": hex64(entry.entry_value),
                })).collect::<Vec<_>>(),
                "outcome": probe.outcome,
                "translated_physical": probe.translated_physical.map(hex64),
                "error": probe.error,
            })).collect::<Vec<_>>(),
        })
    });
    let rdtsc_sites = observations
        .rdtsc_cpl0_sites
        .iter()
        .map(|site| json!({ "rip": hex64(site.rip), "reads": site.reads }))
        .collect::<Vec<_>>();
    let poll_samples = observations
        .rdtsc_site_samples
        .iter()
        .map(|sample| {
            json!({
                "rip": hex64(sample.rip),
                "hit_ordinal": sample.hit_ordinal,
                "requested_cycles": hex64(sample.requested_cycles),
                "start_tsc": hex64(sample.start_tsc),
                "observed_tsc": hex64(sample.observed_tsc),
                "elapsed_cycles": hex64(sample.elapsed_cycles),
                "rbp": hex64(sample.rbp),
                "rsp": hex64(sample.rsp),
                "cr3": sample.cr3.map(hex64),
            })
        })
        .collect::<Vec<_>>();
    let entry_samples = observations
        .rdtsc_entry_samples
        .iter()
        .map(|sample| {
            json!({
                "rip": hex64(sample.rip),
                "hit_ordinal": sample.hit_ordinal,
                "requested_cycles": hex64(sample.requested_cycles),
                "observed_tsc": hex64(sample.observed_tsc),
                "rbp": hex64(sample.rbp),
                "rsp": hex64(sample.rsp),
                "cr3": sample.cr3.map(hex64),
            })
        })
        .collect::<Vec<_>>();
    let delay_tsc_call_summary = observations.delay_tsc_call_summary.as_ref().map(|summary| {
        json!({
            "entry_rip": hex64(summary.entry_rip),
            "poll_rip": hex64(summary.poll_rip),
            "entry_count": summary.entry_count,
            "entries_omitted": summary.entries_omitted,
            "poll_count": summary.poll_count,
            "unmatched_polls": summary.unmatched_polls,
            "ambiguous_polls": summary.ambiguous_polls,
            "entries": summary.entries.iter().map(|entry| json!({
                "ordinal": entry.ordinal,
                "requested_cycles": hex64(entry.requested_cycles),
                "entry_tsc": hex64(entry.entry_tsc),
                "cr3": entry.cr3.map(hex64),
                "rbp": hex64(entry.rbp),
                "rsp": hex64(entry.rsp),
                "frame_links": entry.frame_links.iter().map(|link| json!({
                    "frame_pointer": hex64(link.frame_pointer),
                    "return_address": hex64(link.return_address),
                })).collect::<Vec<_>>(),
                "frame_chain_end": frame_chain_end_name(entry.frame_chain_end),
                "frame_chain_incomplete": entry.frame_chain_end
                    != od_machine::FrameChainEnd::NullFramePointer,
                "poll_count": entry.poll_count,
                "first_poll_ordinal": entry.first_poll_ordinal,
                "last_poll_ordinal": entry.last_poll_ordinal,
                "last_elapsed_cycles": entry.last_elapsed_cycles.map(hex64),
                "terminal_threshold_seen": entry.terminal_threshold_seen,
                "poll_sequence_status": delay_tsc_poll_sequence_status_name(entry.poll_sequence_status),
            })).collect::<Vec<_>>(),
        })
    });
    let entry_clock_trace_samples = observations
        .rdtsc_entry_clock_trace_samples
        .iter()
        .map(|sample| {
            json!({
                "rip": hex64(sample.rip),
                "hit_ordinal": sample.hit_ordinal,
                "initial_now_ns": sample.initial_now_ns,
                "now_ns": sample.now_ns,
                "callback_tsc": hex64(sample.callback_tsc),
                "tsc_hz": sample.tsc_hz,
                "retired_instructions_charged": sample.retired_instructions_charged,
                "retired_ns_charged": sample.retired_ns_charged,
                "quantum_instructions_executed": sample.quantum_instructions_executed,
                "retired_instructions_through_rdtsc": sample.retired_instructions_through_rdtsc,
                "pm_timer_ns_charged": sample.pm_timer_ns_charged,
                "pit_ns_charged": sample.pit_ns_charged,
                "rdtsc_ns_charged": sample.rdtsc_ns_charged,
                "idle_ns_advanced": sample.idle_ns_advanced,
                "quantum_ordinal": sample.quantum_ordinal,
                "irqs_delivered": sample.irqs_delivered,
                "unaccounted_elapsed_ns": sample.unaccounted_elapsed_ns,
            })
        })
        .collect::<Vec<_>>();
    let rdtsc_prefix_samples = observations
        .rdtsc_prefix_samples
        .iter()
        .map(|sample| {
            let clock = &sample.clock;
            json!({
                "ordinal": sample.ordinal,
                "rip": hex64(sample.rip),
                "cpl": sample.cpl,
                "cr3": sample.cr3.map(hex64),
                "rsp": hex64(sample.rsp),
                "rbp": hex64(sample.rbp),
                "page_faults_delivered_through": sample.page_faults_delivered_through,
                "general_protection_faults_delivered_through": sample.general_protection_faults_delivered_through,
                "clock": {
                    "rip": hex64(clock.rip),
                    "hit_ordinal": clock.hit_ordinal,
                    "initial_now_ns": clock.initial_now_ns,
                    "now_ns": clock.now_ns,
                    "callback_tsc": hex64(clock.callback_tsc),
                    "tsc_hz": clock.tsc_hz,
                    "retired_instructions_charged": clock.retired_instructions_charged,
                    "retired_ns_charged": clock.retired_ns_charged,
                    "quantum_instructions_executed": clock.quantum_instructions_executed,
                    "retired_instructions_through_rdtsc": clock.retired_instructions_through_rdtsc,
                    "pm_timer_ns_charged": clock.pm_timer_ns_charged,
                    "pit_ns_charged": clock.pit_ns_charged,
                    "rdtsc_ns_charged": clock.rdtsc_ns_charged,
                    "idle_ns_advanced": clock.idle_ns_advanced,
                    "quantum_ordinal": clock.quantum_ordinal,
                    "irqs_delivered": clock.irqs_delivered,
                    "unaccounted_elapsed_ns": clock.unaccounted_elapsed_ns,
                },
            })
        })
        .collect::<Vec<_>>();
    let page_fault_events = observations
        .page_fault_events
        .iter()
        .map(|event| {
            json!({
                "rip": hex64(event.rip),
                "cr2": hex64(event.cr2),
                "error_code": format!("0x{:08x}", event.error_code),
                "access": event.access,
                "cpl": event.cpl,
                "faulting_context": {
                    "cr3": hex64(event.faulting_context.cr3),
                    "rsp": hex64(event.faulting_context.rsp),
                    "rbp": hex64(event.faulting_context.rbp),
                    "rcx": hex64(event.faulting_context.rcx),
                    "rsi": hex64(event.faulting_context.rsi),
                    "rdi": hex64(event.faulting_context.rdi),
                    "cs_selector": event.faulting_context.cs_selector.map(|selector| format!("0x{selector:04x}")),
                    "frame_chain": event.faulting_context.frame_chain.iter().map(|frame| json!({
                        "frame_pointer": hex64(frame.frame_pointer),
                        "return_address": hex64(frame.return_address),
                    })).collect::<Vec<_>>(),
                    "frame_chain_end": format!("{:?}", event.faulting_context.frame_chain_end),
                    "page_walk": event.faulting_context.page_walk.as_ref().map(|walk| json!({
                        "source": "original_faulting_guest_access",
                        "entry_values": "raw_before_accessed_dirty_updates",
                        "reason": format!("{:?}", walk.reason),
                        "entries": walk.entries.iter().map(|entry| json!({
                            "level": format!("{:?}", entry.level),
                            "index": entry.index,
                            "entry_address": hex64(entry.entry_address.get()),
                            "entry_value": hex64(entry.entry_value),
                        })).collect::<Vec<_>>(),
                    })),
                },
            })
        })
        .collect::<Vec<_>>();
    let post_iret_page_walk = observations.post_iret_page_walk.as_ref().map(|snapshot| {
        json!({
            "event_ordinal": snapshot.event_ordinal,
            "fault_rip": hex64(snapshot.fault_rip),
            "fault_cr2": hex64(snapshot.fault_cr2),
            "fault_access": snapshot.fault_access,
            "fault_cpl": snapshot.fault_cpl,
            "post_iret_context": {
                "rip": hex64(snapshot.post_iret_rip),
                "cs": snapshot.post_iret_cs.map(|selector| format!("0x{selector:04x}")),
                "cpl": snapshot.post_iret_cpl,
                "rsp": hex64(snapshot.post_iret_rsp),
                "cr3": hex64(snapshot.post_iret_cr3),
            },
            "translation_status": snapshot.translation_status,
            "translated_physical": snapshot.translated_physical.map(hex64),
            "translation_error": snapshot.translation_error,
            "entries": snapshot.entries.iter().map(|entry| json!({
                "level": format!("{:?}", entry.level),
                "index": entry.index,
                "entry_address": hex64(entry.entry_address.get()),
                "entry_value": hex64(entry.entry_value),
            })).collect::<Vec<_>>(),
        })
    });
    let instruction_path_trace = observations.instruction_path_trace.as_ref().map(|trace| {
        let status = match trace.status {
            od_machine::InstructionPathTraceStatus::NotRequested => "not_requested",
            od_machine::InstructionPathTraceStatus::AwaitingStart => "awaiting_start",
            od_machine::InstructionPathTraceStatus::Captured => "captured",
            od_machine::InstructionPathTraceStatus::GateMismatch => "gate_mismatch",
            od_machine::InstructionPathTraceStatus::MissedStart => "missed_start",
            od_machine::InstructionPathTraceStatus::MissedEnd => "missed_end",
            od_machine::InstructionPathTraceStatus::CapacityExhausted => "capacity_exhausted",
        };
        json!({
            "requested": trace.requested,
            "status": status,
            "quantum_ordinal": trace.quantum_ordinal,
            "retired_instructions_charged": trace.retired_instructions_charged,
            "rdtsc_cpl0_hits_before_dispatch": trace.rdtsc_cpl0_hits_before_dispatch,
            "quantum_instructions_executed": trace.quantum_instructions_executed,
            "rdtsc_cpl0_hits_after_quantum": trace.rdtsc_cpl0_hits_after_quantum,
            "omitted_records": trace.omitted_records,
            "header": trace.header.as_ref().map(|header| json!({
                "rip": hex64(header.rip),
                "rflags": hex64(header.rflags),
                "gpr": header.gpr.map(hex64),
                "cr3": header.cr3.map(hex64),
                "cs_selector": header.cs_selector.map(|value| format!("0x{value:04x}")),
                "frame_chain": header.frame_chain.iter().map(|frame| json!({
                    "frame_pointer": hex64(frame.frame_pointer),
                    "return_address": hex64(frame.return_address),
                })).collect::<Vec<_>>(),
                "frame_chain_end": format!("{:?}", header.frame_chain_end),
            })),
            "records": trace.records.iter().map(|record| json!({
                "rip": hex64(record.rip),
                "next_rip": hex64(record.next_rip),
                "control_flow": record.control_flow.map(|flow| json!({
                    "kind": flow.kind,
                    "condition": flow.condition,
                    "taken": flow.taken,
                    "rflags_before": hex64(flow.rflags_before),
                    "rflags_after": hex64(flow.rflags_after),
                    "destination_registers": flow.destination_registers.iter().flatten().map(|value| json!({
                        "register": value.register,
                        "value": hex64(value.value),
                    })).collect::<Vec<_>>(),
                    "destination": hex64(flow.destination),
                })),
            })).collect::<Vec<_>>(),
        })
    });
    let value = json!({
        "schema_version": 21,
        "capture_scope": "CPL0 RDTSC instruction RIP counts and delivered #PF events with pre-delivery registers, bounded frame-pointer context, original faulting page-walk traces, an optional selected post-IRET page-walk snapshot, optional post-instruction registers at one selected CPL0 RDTSC RIP, an optional bounded every-hit entry-RDTSC stream with clock ledger, an optional ordered RDTSC prefix through a sentinel RIP, an optional inclusive hit-ordinal window, an optional strictly gated single-quantum instruction path trace, an optional bounded delay_tsc per-call summary, an optional ordered pre-copy CR3-write ledger, bounded read-only MMU-gather candidate-page walks, and two exact read-only ELF stack-alignment instruction anchors",
        "limits": {
            "rdtsc_cpl0_sites": od_machine::LINUX_RDTSC_SITE_CAPACITY,
            "rdtsc_site_samples": od_machine::LINUX_RDTSC_SAMPLE_CAPACITY,
            "rdtsc_entry_samples": od_machine::LINUX_RDTSC_SAMPLE_CAPACITY,
            "delay_tsc_call_summaries": od_machine::LINUX_DELAY_TSC_CALL_CAPACITY,
            "delay_tsc_frame_links_per_entry": 4,
            "rdtsc_entry_clock_trace_samples": od_machine::LINUX_RDTSC_CLOCK_TRACE_CAPACITY,
            "rdtsc_prefix_samples": od_machine::LINUX_RDTSC_PREFIX_CAPACITY,
            "instruction_path_trace_records": od_machine::LINUX_INSTRUCTION_PATH_TRACE_CAPACITY,
            "page_fault_events": od_machine::LINUX_PAGE_FAULT_EVENT_CAPACITY,
            "frame_links_per_page_fault": od_machine::LINUX_PAGE_FAULT_FRAME_LINK_CAPACITY,
            "page_walk_entries_per_fault": od_machine::LINUX_PAGE_FAULT_WALK_ENTRY_CAPACITY,
            "post_iret_page_walk_entries": od_machine::LINUX_PAGE_FAULT_WALK_ENTRY_CAPACITY,
            "elf_stack_copy_probes": od_machine::LINUX_ELF_STACK_COPY_PROBE_CAPACITY,
            "elf_stack_copy_walk_entries_per_probe": od_machine::LINUX_ELF_STACK_COPY_WALK_ENTRY_CAPACITY,
            "elf_stack_copy_cr3_events": od_machine::LINUX_ELF_STACK_COPY_CR3_EVENT_CAPACITY,
            "elf_stack_copy_rdtsc_sites": od_machine::LINUX_ELF_STACK_COPY_RDTSC_SITE_CAPACITY,
            "elf_stack_copy_page_fault_events": od_machine::LINUX_ELF_STACK_COPY_PF_EVENT_CAPACITY,
            "elf_stack_copy_retired_duration": od_machine::LINUX_ELF_STACK_COPY_RETIRED_CAPACITY,
            "elf_stack_copy_pre_copy_cr3_events": od_machine::LINUX_ELF_STACK_COPY_PRE_COPY_CR3_EVENT_CAPACITY,
            "elf_stack_alignment_points_per_run": 2,
        },
        "rdtsc_cpl0_sites": rdtsc_sites,
        "rdtsc_cpl0_unattributed_reads": observations.rdtsc_cpl0_unattributed_reads,
        "rdtsc_cpl0_site_overflow_reads": observations.rdtsc_cpl0_site_overflow_reads,
        "rdtsc_sample_rip": observations.rdtsc_sample_rip.map(hex64),
        "rdtsc_sample_stride": observations.rdtsc_sample_stride,
        "rdtsc_sample_window": observations.rdtsc_sample_window.map(|window| json!({
            "first_hit": window.first_hit,
            "last_hit": window.last_hit,
        })),
        "rdtsc_sample_site_hits": observations.rdtsc_sample_site_hits,
        "rdtsc_site_samples": poll_samples,
        "rdtsc_site_samples_omitted": observations.rdtsc_site_samples_omitted,
        "rdtsc_entry_sample_rip": observations.rdtsc_entry_sample_rip.map(hex64),
        "rdtsc_entry_sample_hits": observations.rdtsc_entry_sample_hits,
        "rdtsc_entry_samples": entry_samples,
        "rdtsc_entry_samples_omitted": observations.rdtsc_entry_samples_omitted,
        "delay_tsc_call_summary": delay_tsc_call_summary,
        "rdtsc_entry_clock_trace_samples": entry_clock_trace_samples,
        "rdtsc_entry_clock_trace_missing": observations.rdtsc_entry_clock_trace_missing,
        "rdtsc_prefix_sentinel_rip": observations.rdtsc_prefix_sentinel_rip.map(hex64),
        "rdtsc_prefix_status": rdtsc_prefix_status_name(observations.rdtsc_prefix_status),
        "rdtsc_prefix_samples": rdtsc_prefix_samples,
        "rdtsc_prefix_event_count": observations.rdtsc_prefix_event_count,
        "rdtsc_prefix_missing": observations.rdtsc_prefix_missing,
        "instruction_path_trace": instruction_path_trace,
        "page_faults_delivered": observations.page_faults_delivered,
        "page_fault_events": page_fault_events,
        "page_fault_events_omitted": observations.page_fault_events_omitted,
        "post_iret_selected_ordinal": observations.post_iret_selected_ordinal,
        "post_iret_page_walk_status": observations.post_iret_page_walk_status,
        "post_iret_page_walk": post_iret_page_walk,
        "elf_stack_copy": elf_stack_copy,
    });
    file_matches_existing_path(file, path, path)?;
    let mut bytes = serde_json::to_vec_pretty(&value).map_err(|error| error.to_string())?;
    bytes.push(b'\n');
    use std::io::Write;
    if let Err(error) = file.write_all(&bytes).and_then(|()| file.flush()) {
        let cleanup = match file_matches_existing_path(file, path, path) {
            Ok(true) => {
                std::fs::remove_file(path).map_err(|cleanup_error| cleanup_error.to_string())
            }
            Ok(false) => Err("reservation path identity changed".into()),
            Err(cleanup_error) => Err(cleanup_error),
        };
        return Err(match cleanup {
            Ok(()) => format!("{path}: {error}"),
            Err(cleanup_error) => {
                format!("{path}: {error}; partial sidecar cleanup failed: {cleanup_error}")
            }
        });
    }
    file_matches_existing_path(file, path, path)?;
    Ok((bytes.len(), od_testkit::sha256_hex(&bytes)))
}

fn elf_stack_copy_status_name(status: od_machine::ElfStackCopyStatus) -> &'static str {
    match status {
        od_machine::ElfStackCopyStatus::NotRequested => "not_requested",
        od_machine::ElfStackCopyStatus::AwaitingTarget => "awaiting_target",
        od_machine::ElfStackCopyStatus::Captured => "captured",
        od_machine::ElfStackCopyStatus::GateMismatch => "gate_mismatch",
        od_machine::ElfStackCopyStatus::ReadError => "read_error",
        od_machine::ElfStackCopyStatus::RunEndedBeforeTarget => "run_ended_before_target",
    }
}

fn elf_mmu_gather_context_json(context: od_machine::ElfMmuGatherContext) -> Value {
    json!({
        "rip": hex64(context.rip),
        "retired_instructions": context.retired_instructions,
        "cr3": hex64(context.cr3),
        "cr3_changes": context.cr3_changes,
        "page_faults": context.page_faults,
        "rbp": hex64(context.rbp),
        "rsp": hex64(context.rsp),
        "tlb_pointer": context.tlb_pointer.map(hex64),
        "saved_rbp": context.saved_rbp.map(hex64),
        "saved_return_rip": context.saved_return_rip.map(hex64),
    })
}

fn elf_mmu_gather_raw_json(raw: od_machine::ElfMmuGatherRaw) -> Value {
    json!({
        "tlb": hex64(raw.tlb),
        "mm": hex64(raw.mm),
        "start": hex64(raw.start),
        "end": hex64(raw.end),
        "flags": format!("0x{:08x}", raw.flags),
        "mm_tlb_flush_nested": raw.mm_tlb_flush_nested,
    })
}

fn elf_mmu_setup_data_json(data: &od_machine::ElfMmuGatherSetupData) -> Value {
    json!({
        "stack_shift": hex64(data.stack_shift),
        "vma": hex64(data.vma),
        "vm_start": data.vm_start.map(hex64),
        "vm_end": data.vm_end.map(hex64),
        "relocated_start": data.relocated_start.map(hex64),
        "aligned_start": data.aligned_start.map(hex64),
        "candidate_pages": data.candidate_pages.map(|address| address.map(hex64)),
        "candidate_error": data.candidate_error,
    })
}

fn elf_mmu_gather_page_walk_json(walk: &od_machine::ElfMmuGatherPageWalk) -> Value {
    json!({
        "address": walk.address.map(hex64),
        "access": walk.access,
        "cpl": walk.cpl,
        "origin": walk.origin,
        "profile": walk.profile,
        "cr0_wp": walk.cr0_wp,
        "efer_nxe": walk.efer_nxe,
        "status": walk.status,
        "entries": walk.entries.iter().map(|entry| json!({
            "level": format!("{:?}", entry.level),
            "index": entry.index,
            "entry_address": hex64(entry.entry_address.get()),
            "entry_value": hex64(entry.entry_value),
        })).collect::<Vec<_>>(),
        "terminal_level": walk.terminal_level.map(|level| format!("{level:?}")),
        "leaf_size": walk.leaf_size,
        "translated_physical": walk.translated_physical.map(hex64),
        "error": walk.error,
    })
}

fn elf_mmu_phase_json<T>(
    phase: &od_machine::ElfMmuGatherPhase<T>,
    encode: impl Fn(&T) -> Value,
) -> Value {
    match phase {
        od_machine::ElfMmuGatherPhase::NotObserved => json!({"status": "not_observed"}),
        od_machine::ElfMmuGatherPhase::Captured { context, value } => json!({
            "status": "captured",
            "context": elf_mmu_gather_context_json(*context),
            "value": encode(value),
        }),
        od_machine::ElfMmuGatherPhase::ReadError {
            context,
            partial,
            error,
        } => json!({
            "status": "read_error",
            "context": elf_mmu_gather_context_json(*context),
            "partial": partial.as_ref().map(&encode),
            "error": error,
        }),
    }
}

fn elf_mmu_gather_setup_json(setup: &od_machine::ElfMmuGatherInvocation) -> Value {
    let needs_flush = match &setup.actual_post {
        od_machine::ElfMmuGatherPhase::Captured { value, .. } => Some(value.flags & 0x08f4 != 0),
        od_machine::ElfMmuGatherPhase::ReadError {
            partial: Some(value),
            ..
        } => Some(value.flags & 0x08f4 != 0),
        _ => None,
    };
    json!({
        "sequence": setup.sequence,
        "association_mismatch": setup.association_mismatch,
        "closure": match setup.closure {
            od_machine::ElfMmuGatherClosure::Active => "active",
            od_machine::ElfMmuGatherClosure::Returned => "returned",
            od_machine::ElfMmuGatherClosure::Superseded => "superseded",
            od_machine::ElfMmuGatherClosure::RunEnded => "run_ended",
        },
        "setup": elf_mmu_phase_json(&setup.setup, elf_mmu_setup_data_json),
        "setup_page_walks": setup.setup_page_walks.iter().map(elf_mmu_gather_page_walk_json).collect::<Vec<_>>(),
        "initial": elf_mmu_phase_json(&setup.initial, |raw| elf_mmu_gather_raw_json(*raw)),
        "initial_page_walks": setup.initial_page_walks.as_ref().map(|walks| walks.iter().map(elf_mmu_gather_page_walk_json).collect::<Vec<_>>()),
        "actual_post": elf_mmu_phase_json(&setup.actual_post, |raw| elf_mmu_gather_raw_json(*raw)),
        "returned": elf_mmu_phase_json(&setup.returned, |_| json!(true)),
        "return_page_walks": setup.return_page_walks.as_ref().map(|walks| walks.iter().map(elf_mmu_gather_page_walk_json).collect::<Vec<_>>()),
        "needs_flush": needs_flush,
    })
}

fn elf_mmu_gather_capture_status(capture: &od_machine::ElfMmuGatherCapture) -> &'static str {
    if capture.setups_omitted != 0 || capture.unpaired_omitted != 0 {
        return "truncated";
    }
    if capture.setups.is_empty() && capture.unpaired.is_empty() {
        return "not_seen";
    }
    let complete = capture.unpaired.is_empty()
        && capture.setups.iter().all(|invocation| {
            invocation.closure == od_machine::ElfMmuGatherClosure::Returned
                && !invocation.association_mismatch
                && matches!(
                    invocation.setup,
                    od_machine::ElfMmuGatherPhase::Captured { .. }
                )
                && invocation
                    .setup_page_walks
                    .iter()
                    .all(elf_mmu_gather_page_walk_is_complete)
                && matches!(
                    invocation.initial,
                    od_machine::ElfMmuGatherPhase::Captured { .. }
                )
                && invocation
                    .initial_page_walks
                    .as_ref()
                    .is_some_and(|walks| walks.iter().all(elf_mmu_gather_page_walk_is_complete))
                && matches!(
                    invocation.actual_post,
                    od_machine::ElfMmuGatherPhase::Captured { .. }
                )
                && matches!(
                    invocation.returned,
                    od_machine::ElfMmuGatherPhase::Captured { .. }
                )
                && invocation
                    .return_page_walks
                    .as_ref()
                    .is_some_and(|walks| walks.iter().all(elf_mmu_gather_page_walk_is_complete))
        });
    if complete { "captured" } else { "incomplete" }
}

fn elf_mmu_gather_page_walk_is_complete(walk: &od_machine::ElfMmuGatherPageWalk) -> bool {
    matches!(walk.status, "translated" | "non_present" | "protection")
}

fn elf_mmu_gather_unpaired_json(event: &od_machine::ElfMmuGatherUnpaired) -> Value {
    json!({
        "phase": event.phase,
        "status": event.status,
        "context": elf_mmu_gather_context_json(event.context),
        "raw": event.raw.map(elf_mmu_gather_raw_json),
        "page_walks": event.page_walks.as_ref().map(|walks| walks.iter().map(elf_mmu_gather_page_walk_json).collect::<Vec<_>>()),
        "error": event.error,
    })
}

fn elf_stack_copy_invocation_json(
    capture: &od_machine::ElfStackCopyInvocation,
) -> serde_json::Value {
    let counters = |snapshot: &od_machine::ElfStackCopyCounterSnapshot| {
        json!({
            "cr3": hex64(snapshot.cr3),
            "delivered_page_faults": snapshot.delivered_page_faults,
            "delivered_exceptions": snapshot.delivered_exceptions,
            "cr3_changes": snapshot.cr3_changes,
            "rdtsc_reads": snapshot.rdtsc_reads,
            "retired_instructions_through_current": snapshot.retired_instructions_through_current,
        })
    };
    json!({
        "status": elf_stack_copy_invocation_status_name(capture.status),
        "return_rip": capture.return_rip.map(hex64),
        "entry": capture.entry.as_ref().map(counters),
        "return_boundary": capture.return_boundary.as_ref().map(counters),
        "stop": capture.stop.as_ref().map(counters),
        "cr3_events": capture.cr3_events.iter().map(|event| json!({
            "rip": hex64(event.rip),
            "old_cr3": hex64(event.old_cr3),
            "new_cr3": hex64(event.new_cr3),
            "retired_instructions_through_current": event.retired_instructions_through_current,
        })).collect::<Vec<_>>(),
        "cr3_events_omitted": capture.cr3_events_omitted,
        "page_fault_events": capture.page_fault_events.iter().map(|event| json!({
            "rip": hex64(event.rip),
            "cr2": hex64(event.cr2),
            "error_code": event.error_code,
            "access": event.access,
            "cpl": event.cpl,
            "retired_instructions_through_current": event.retired_instructions_through_current,
        })).collect::<Vec<_>>(),
        "page_fault_events_omitted": capture.page_fault_events_omitted,
        "rdtsc_sites": capture.rdtsc_sites.iter().map(|site| json!({ "rip": hex64(site.rip), "reads": site.reads })).collect::<Vec<_>>(),
        "rdtsc_site_overflow_reads": capture.rdtsc_site_overflow_reads,
        "retired_duration": capture.retired_duration,
        "retired_duration_cap": capture.retired_duration_cap,
    })
}

fn elf_stack_copy_invocation_status_name(
    status: od_machine::ElfStackCopyInvocationStatus,
) -> &'static str {
    match status {
        od_machine::ElfStackCopyInvocationStatus::NotStarted => "not_started",
        od_machine::ElfStackCopyInvocationStatus::InProgress => "in_progress",
        od_machine::ElfStackCopyInvocationStatus::Complete => "complete",
        od_machine::ElfStackCopyInvocationStatus::IncompleteGate => "incomplete_gate",
        od_machine::ElfStackCopyInvocationStatus::IncompleteOverflow => "incomplete_overflow",
        od_machine::ElfStackCopyInvocationStatus::IncompleteRunEnded => "incomplete_run_ended",
    }
}

fn delay_tsc_poll_sequence_status_name(
    status: od_machine::DelayTscPollSequenceStatus,
) -> &'static str {
    match status {
        od_machine::DelayTscPollSequenceStatus::ThresholdSeen => "threshold_seen",
        od_machine::DelayTscPollSequenceStatus::Partial => "partial",
    }
}

fn execution_observations_sidecar_metadata(
    path: &str,
    observations: &LinuxExecutionObservations,
    byte_count: usize,
    sha256: String,
) -> Value {
    json!({
        "path": path,
        "capture_scope": "CPL0 RDTSC instruction RIP counts and delivered #PF events, with bounded optional RDTSC streams and a strictly gated single-quantum instruction path trace",
        "write_error": null,
        "selected_rip": observations.rdtsc_sample_rip.map(hex64),
        "stride": observations.rdtsc_sample_stride,
        "sample_window": observations.rdtsc_sample_window.map(|window| json!({
            "first_hit": window.first_hit,
            "last_hit": window.last_hit,
        })),
        "selected_site_hits": observations.rdtsc_sample_site_hits,
        "sample_count": observations.rdtsc_site_samples.len(),
        "samples_omitted": observations.rdtsc_site_samples_omitted,
        "entry_rip": observations.rdtsc_entry_sample_rip.map(hex64),
        "entry_hits": observations.rdtsc_entry_sample_hits,
        "entry_sample_count": observations.rdtsc_entry_samples.len(),
        "entry_samples_omitted": observations.rdtsc_entry_samples_omitted,
        "delay_tsc_call_summary_entry_count": observations.delay_tsc_call_summary.as_ref().map(|summary| summary.entry_count),
        "delay_tsc_call_summary_retained_count": observations.delay_tsc_call_summary.as_ref().map(|summary| summary.entries.len()),
        "delay_tsc_call_summary_entries_omitted": observations.delay_tsc_call_summary.as_ref().map(|summary| summary.entries_omitted),
        "delay_tsc_call_summary_poll_count": observations.delay_tsc_call_summary.as_ref().map(|summary| summary.poll_count),
        "delay_tsc_call_summary_unmatched_polls": observations.delay_tsc_call_summary.as_ref().map(|summary| summary.unmatched_polls),
        "delay_tsc_call_summary_ambiguous_polls": observations.delay_tsc_call_summary.as_ref().map(|summary| summary.ambiguous_polls),
        "entry_clock_trace_sample_count": observations.rdtsc_entry_clock_trace_samples.len(),
        "entry_clock_trace_missing": observations.rdtsc_entry_clock_trace_missing,
        "rdtsc_prefix_sentinel_rip": observations.rdtsc_prefix_sentinel_rip.map(hex64),
        "rdtsc_prefix_status": rdtsc_prefix_status_name(observations.rdtsc_prefix_status),
        "rdtsc_prefix_sample_count": observations.rdtsc_prefix_samples.len(),
        "rdtsc_prefix_event_count": observations.rdtsc_prefix_event_count,
        "rdtsc_prefix_missing": observations.rdtsc_prefix_missing,
        "instruction_path_trace_status": observations.instruction_path_trace.as_ref().map(|trace| match trace.status {
            od_machine::InstructionPathTraceStatus::NotRequested => "not_requested",
            od_machine::InstructionPathTraceStatus::AwaitingStart => "awaiting_start",
            od_machine::InstructionPathTraceStatus::Captured => "captured",
            od_machine::InstructionPathTraceStatus::GateMismatch => "gate_mismatch",
            od_machine::InstructionPathTraceStatus::MissedStart => "missed_start",
            od_machine::InstructionPathTraceStatus::MissedEnd => "missed_end",
            od_machine::InstructionPathTraceStatus::CapacityExhausted => "capacity_exhausted",
        }),
        "instruction_path_trace_record_count": observations.instruction_path_trace.as_ref().map(|trace| trace.records.len()),
        "instruction_path_trace_omitted_records": observations.instruction_path_trace.as_ref().map(|trace| trace.omitted_records),
        "elf_stack_copy_status": observations.elf_stack_copy.as_ref().map(|snapshot| elf_stack_copy_status_name(snapshot.status)),
        "byte_count": byte_count,
        "sha256": sha256,
    })
}

fn uart_context_end_name(end: od_machine::UartOutputContextEnd) -> &'static str {
    match end {
        od_machine::UartOutputContextEnd::MarkerNotFound => "marker_not_found",
        od_machine::UartOutputContextEnd::RunEnded => "run_end",
        od_machine::UartOutputContextEnd::ByteLimit => "byte_limit",
    }
}

fn rdtsc_prefix_status_name(status: od_machine::RdtscPrefixStatus) -> &'static str {
    match status {
        od_machine::RdtscPrefixStatus::NotRequested => "not_requested",
        od_machine::RdtscPrefixStatus::SentinelSeen => "sentinel_seen",
        od_machine::RdtscPrefixStatus::CapacityExhausted => "capacity_exhausted",
        od_machine::RdtscPrefixStatus::RunEndedBeforeSentinel => "run_ended_before_sentinel",
    }
}

fn frame_chain_end_name(end: od_machine::FrameChainEnd) -> &'static str {
    match end {
        od_machine::FrameChainEnd::NullFramePointer => "null_frame_pointer",
        od_machine::FrameChainEnd::NonCanonicalFramePointer => "non_canonical_frame_pointer",
        od_machine::FrameChainEnd::UnalignedFramePointer => "unaligned_frame_pointer",
        od_machine::FrameChainEnd::OutsideStackWindow => "outside_stack_window",
        od_machine::FrameChainEnd::RepeatedFramePointer => "repeated_frame_pointer",
        od_machine::FrameChainEnd::NonMonotonicFramePointer => "non_monotonic_frame_pointer",
        od_machine::FrameChainEnd::ReadFailed => "read_failed",
        od_machine::FrameChainEnd::NonCanonicalReturnAddress => "non_canonical_return_address",
        od_machine::FrameChainEnd::FrameLimit => "frame_limit",
    }
}

/// A lowercase 64-digit hex string.
fn parse_sha256(text: &str) -> Result<String, String> {
    let lower = text.to_ascii_lowercase();
    if lower.len() != 64 || !lower.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(format!("'{text}' is not a 64-digit hex SHA-256"));
    }
    Ok(lower)
}

fn parse_hash(text: &str) -> Result<[u8; 32], String> {
    let lower = parse_sha256(text)?;
    let mut hash = [0u8; 32];
    for (index, byte) in hash.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&lower[index * 2..index * 2 + 2], 16)
            .map_err(|_| format!("'{text}' is not hex"))?;
    }
    Ok(hash)
}

/// The manifest hash of the board this command assembles (ECAM, PM block, ACPI tables); the
/// operator passes it as `--expected-acpi-manifest-hash`, tests compute it here.
#[cfg(test)]
fn board_manifest_hash() -> [u8; 32] {
    od_machine::FirmwareBoardConfig {
        pci_ecam: true,
        acpi_pm: true,
        acpi_tables: true,
        ..od_machine::FirmwareBoardConfig::default()
    }
    .acpi_manifest_hash()
    .expect("manifest hash")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    const BLOB: &[u8] =
        include_bytes!("../../../tests/fixtures/guest/stage4/acpi_blob/stage4_acpi_blob.bin");
    const PREF: u64 = 0x0100_0000;
    const PM_OFFSET: usize = 3 * 512;
    const PM_CODE: usize = 0x3000;

    fn put16(b: &mut [u8], at: usize, v: u16) {
        b[at..at + 2].copy_from_slice(&v.to_le_bytes());
    }
    fn put32(b: &mut [u8], at: usize, v: u32) {
        b[at..at + 4].copy_from_slice(&v.to_le_bytes());
    }
    fn put64(b: &mut [u8], at: usize, v: u64) {
        b[at..at + 8].copy_from_slice(&v.to_le_bytes());
    }

    /// A small valid bzImage: pref 16 MiB, `init_size` 1 MiB, 2 MiB alignment, 0x3000 bytes of
    /// protected-mode code after 3 setup sectors, `cmdline_size` 0x7FF.
    fn bzimage() -> Vec<u8> {
        let mut b = vec![0u8; PM_OFFSET + PM_CODE];
        for (i, byte) in b[PM_OFFSET..].iter_mut().enumerate() {
            *byte = (i % 251) as u8 ^ 0x5A;
        }
        b[0x1F1] = 2;
        b[0x201] = 0x6A;
        put32(&mut b, 0x1F4, (PM_CODE / 16) as u32);
        put16(&mut b, 0x1FE, 0xAA55);
        b[0x202..0x206].copy_from_slice(b"HdrS");
        put16(&mut b, 0x206, 0x020F);
        b[0x211] = 1;
        put32(&mut b, 0x22C, 0x7FFF_FFFF);
        put32(&mut b, 0x230, 0x0020_0000);
        b[0x234] = 1;
        put16(&mut b, 0x236, 0x7F);
        put32(&mut b, 0x238, 0x7FF);
        put32(&mut b, 0x248, 0x1000);
        put32(&mut b, 0x24C, 0x2000);
        put64(&mut b, 0x258, PREF);
        put32(&mut b, 0x260, 0x0010_0000);
        b
    }

    /// `bzimage()` with `code` at the 64-bit entry (`startup_64`, offset 0x200 of the kernel).
    fn bzimage_with_entry(code: &[u8]) -> Vec<u8> {
        let mut file = bzimage();
        let at = PM_OFFSET + 0x200;
        file[at..at + code.len()].copy_from_slice(code);
        file
    }

    /// A scratch directory with the input files of one case.
    struct Case {
        dir: PathBuf,
        options: Options,
    }

    impl Case {
        fn new(name: &str, vmlinuz: &[u8], initramfs: &[u8], blob: &[u8]) -> Self {
            let dir = std::env::temp_dir().join(format!(
                "od-runner-linux-direct-{}-{name}",
                std::process::id()
            ));
            std::fs::create_dir_all(&dir).unwrap();
            let write = |file: &str, bytes: &[u8]| {
                let path = dir.join(file);
                std::fs::write(&path, bytes).unwrap();
                path.to_string_lossy().into_owned()
            };
            let options = Options {
                vmlinuz: write("vmlinuz", vmlinuz),
                initramfs: write("initramfs.cpio", initramfs),
                acpi_blob: write("acpi.bin", blob),
                nonce: "N0nce-1".to_string(),
                expected_vmlinuz_sha256: sha256_hex(vmlinuz),
                expected_acpi_manifest_hash: board_manifest_hash(),
                expected_initramfs_sha256: sha256_hex(initramfs),
                ram_mib: 128,
                out: dir.join("out.json").to_string_lossy().into_owned(),
                budget: None,
                vcpu_count: 1,
                interleaved_seed: None,
                rootfs_raw: None,
                set_cpl0: false,
                diagnostic_past_c7: false,
                cpuid_profile: CpuidProfile::DiagnosticV1,
                user_mode: false,
                nonce_bootconfig: false,
                linux_norandmaps: false,
                linux_no_randomize_kstack_offset: false,
                checkpoint_table: None,
                stack_context_out: None,
                execution_observations_out: None,
                execution_observations_post_iret_index: None,
                execution_observations_rdtsc_rip: None,
                execution_observations_rdtsc_entry_rip: None,
                execution_observations_rdtsc_stride: None,
                execution_observations_rdtsc_window: None,
                execution_observations_rdtsc_prefix: false,
                execution_observations_delay_call_summary: false,
                execution_observations_quantum_trace: false,
                execution_observations_elf_stack_copy: false,
            };
            Self { dir, options }
        }

        fn good(name: &str) -> Self {
            Self::new(name, &bzimage(), &[0xAB; 4688], BLOB)
        }
    }

    impl Drop for Case {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    fn code_of(value: &Value) -> (&str, &str) {
        (
            value["error"]["code"].as_str().unwrap_or("<none>"),
            value["error"]["stage"].as_str().unwrap_or("<none>"),
        )
    }

    fn assert_refused(value: &Value, exit: u8, code: &str, stage: &str) {
        assert_eq!(exit, 4, "{value:#}");
        assert_eq!(value["status"], "refused");
        assert_eq!(code_of(value), (code, stage), "{value:#}");
        // The guest is never involved in a refusal.
        assert_eq!(value["retired"], 0);
        assert_eq!(value["guest_started"], false);
    }

    #[test]
    fn positive_path_loads_the_board_and_reports_without_running_the_guest() {
        let case = Case::good("positive");
        let (value, exit) = evaluate(&case.options);
        assert_eq!(exit, 0, "{value:#}");
        assert_eq!(value["status"], "loaded");
        assert_eq!(value["retired"], 0);
        assert_eq!(value["guest_started"], false);
        assert_eq!(value["nonce"], "N0nce-1");
        assert_eq!(
            value["cmdline"],
            "console=ttyS0,115200 earlycon=uart8250,io,0x3f8 8250.nr_uarts=1 \
             od.nonce=N0nce-1 nokaslr"
        );
        assert_eq!(value["cmdline_in_memory_matches"], true);
        // The evidence carries what was read back from guest memory, not the input values.
        assert_eq!(value["cmdline_in_memory"], value["cmdline"]);
        assert_eq!(value["acpi_rsdp_signature_in_memory"], "RSD PTR ");
        assert_eq!(value["acpi_rsdp_signature_matches"], true);
        let file = bzimage();
        assert_eq!(
            value["memory_sha256"]["kernel_pm_code"],
            sha256_hex(&file[PM_OFFSET..PM_OFFSET + PM_CODE])
        );
        assert_eq!(value["memory_sha256"]["kernel_pm_code_matches_file"], true);
        assert_eq!(value["memory_sha256"]["initrd"], sha256_hex(&[0xAB; 4688]));
        assert_eq!(value["memory_sha256"]["initrd_matches_file"], true);
        // The RSDP is the first byte of the ACPI payload at 0xE_0000.
        assert_eq!(value["acpi_rsdp_addr"], "0xe0000");
        assert_eq!(value["acpi"]["rsdp_address"], "0xe0000");
        assert_eq!(value["acpi"]["version"], 2);
        assert_eq!(value["plan"]["acpi_tables"]["start"], "0xe0000");
        assert_eq!(value["plan"]["acpi_facs"]["start"], "0xef000");
        assert_eq!(value["plan"]["kernel"]["start"], "0x1000000");
        assert_eq!(value["plan"]["kernel"]["end"], "0x1100000");
        // Page-aligned start below the top of low RAM; the end is the exact initrd length.
        assert_eq!(value["plan"]["initrd"]["start"], "0x7ffe000");
        assert_eq!(value["plan"]["initrd"]["end"], "0x7fff250");
        assert_eq!(value["inputs"]["vmlinuz"]["sha256"], sha256_hex(&bzimage()));
        assert_eq!(value["inputs"]["initramfs"]["bytes"], 4688);
        // Entry state of the Linux boot protocol.
        let entry = &value["entry"];
        assert_eq!(entry["rip"], "0x1000200");
        assert_eq!(entry["rsi"], "0x100000");
        assert_eq!(entry["rsp"], "0x150000");
        assert_eq!(entry["cs"], "0x10");
        assert_eq!(entry["ds"], "0x18");
        assert_eq!(entry["cr0"], "0x80010033");
        assert_eq!(entry["cr3"], "0x130000");
        assert_eq!(entry["cr4"], "0x20");
        assert_eq!(entry["efer"], "0xd00");
    }

    #[test]
    fn run_writes_valid_json_evidence_to_the_out_file() {
        let case = Case::good("run");
        let o = &case.options;
        let args: Vec<String> = [
            "--vmlinuz",
            &o.vmlinuz,
            "--initramfs",
            &o.initramfs,
            "--acpi-blob",
            &o.acpi_blob,
            "--nonce",
            &o.nonce,
            "--expected-vmlinuz-sha256",
            &o.expected_vmlinuz_sha256,
            "--expected-acpi-manifest-hash",
            &hex(&o.expected_acpi_manifest_hash),
            "--ram-mib",
            "128",
            "--out",
            &o.out,
            "--expected-initramfs-sha256",
            &o.expected_initramfs_sha256,
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        assert_eq!(run(&args), ExitCode::SUCCESS);
        let text = std::fs::read_to_string(&o.out).unwrap();
        let value: Value = serde_json::from_str(&text).expect("valid JSON");
        assert_eq!(value["schema_version"], 1);
        assert_eq!(value["command"], "linux-direct");
        assert_eq!(value["status"], "loaded");
        assert_eq!(value["retired"], 0);
        assert_eq!(value["acpi_rsdp_addr"], "0xe0000");
        // A refusal is written too, with the error code.
        let mut bad = args.clone();
        let at = bad
            .iter()
            .position(|a| a == "--expected-vmlinuz-sha256")
            .unwrap()
            + 1;
        bad[at] = "0".repeat(64);
        assert_eq!(run(&bad), ExitCode::from(4));
        let value: Value = serde_json::from_str(&std::fs::read_to_string(&o.out).unwrap()).unwrap();
        assert_eq!(value["status"], "refused");
        assert_eq!(value["error"]["code"], "vmlinuz_sha256_mismatch");
    }

    #[test]
    fn e820_dump_has_the_ecam_window_reserved_and_the_mmio_windows() {
        let case = Case::good("e820");
        let (value, exit) = evaluate(&case.options);
        assert_eq!(exit, 0, "{value:#}");
        let e820 = value["e820"].as_array().unwrap();
        assert_eq!(value["e820_count"], e820.len());
        assert_eq!(e820.len(), 9);
        let find = |addr: &str| {
            e820.iter()
                .find(|entry| entry["addr"] == addr)
                .unwrap_or_else(|| panic!("no E820 entry at {addr}: {value:#}"))
        };
        // ECAM: [0xFC10_0000, +1 MiB), reserved (type 2).
        let ecam = find("0xfc100000");
        assert_eq!(ecam["size"], "0x100000");
        assert_eq!(ecam["end"], "0xfc200000");
        assert_eq!(ecam["type"], 2);
        assert_eq!(ecam["type_name"], "reserved");
        // Framebuffer and ROM windows of the board map are reserved too.
        let framebuffer = find("0xf8000000");
        assert_eq!(
            (framebuffer["end"].as_str(), framebuffer["type"].as_u64()),
            (Some("0xfa400000"), Some(2))
        );
        let rom = find("0xffc00000");
        assert_eq!(
            (rom["end"].as_str(), rom["type"].as_u64()),
            (Some("0x100000000"), Some(2))
        );
        // ACPI reclaim and NVS entries for the blob.
        assert_eq!(find("0xe0000")["type"], 3);
        assert_eq!(find("0xef000")["type"], 4);
        // Entries are sorted and disjoint.
        let mut previous_end = 0u64;
        for entry in e820 {
            let addr = u64::from_str_radix(&entry["addr"].as_str().unwrap()[2..], 16).unwrap();
            let end = u64::from_str_radix(&entry["end"].as_str().unwrap()[2..], 16).unwrap();
            assert!(addr >= previous_end && end > addr, "{entry}");
            previous_end = end;
        }
    }

    #[test]
    fn a_foreign_manifest_hash_is_refused_before_the_board_is_built() {
        let mut case = Case::good("manifest");
        case.options.expected_acpi_manifest_hash[0] ^= 1;
        let (value, exit) = evaluate(&case.options);
        assert_refused(&value, exit, "acpi_manifest_mismatch", "acpi_blob");
        assert!(value.get("e820").is_none() && value.get("entry").is_none());
        // A blob that is not a blob at all is a different code.
        let mut broken = BLOB.to_vec();
        broken[0] ^= 0xFF;
        let case = Case::new("manifest-magic", &bzimage(), &[1; 64], &broken);
        let (value, exit) = evaluate(&case.options);
        assert_refused(&value, exit, "acpi_blob_invalid", "acpi_blob");
    }

    #[test]
    fn a_truncated_vmlinuz_is_refused_with_matching_hash() {
        // The operator pins the hash of what is on disk: the header check must still refuse.
        let file = bzimage();
        for (name, keep) in [
            ("t300", 0x300),
            ("t-short", 0x100),
            ("t-last", file.len() - 1),
        ] {
            let case = Case::new(name, &file[..keep], &[1; 64], BLOB);
            let (value, exit) = evaluate(&case.options);
            assert_refused(&value, exit, "bzimage_truncated", "bzimage_header");
            assert!(value.get("entry").is_none());
        }
    }

    #[test]
    fn a_payload_beyond_the_end_is_refused() {
        let mut file = bzimage();
        // payload_length reaches past the protected-mode code.
        put32(&mut file, 0x24C, PM_CODE as u32);
        let case = Case::new("payload", &file, &[1; 64], BLOB);
        let (value, exit) = evaluate(&case.options);
        assert_refused(
            &value,
            exit,
            "bzimage_payload_out_of_file",
            "bzimage_header",
        );
    }

    #[test]
    fn a_foreign_vmlinuz_hash_is_refused_first() {
        let mut case = Case::good("sha");
        case.options.expected_vmlinuz_sha256 = "0".repeat(64);
        let (value, exit) = evaluate(&case.options);
        assert_refused(&value, exit, "vmlinuz_sha256_mismatch", "vmlinuz_sha256");
        // Nothing past the hash was looked at: no initramfs, no plan.
        assert!(value["inputs"].get("initramfs").is_none());
        // The same holds when the file is also truncated (hash before header).
        let file = bzimage();
        let mut case = Case::new("sha-trunc", &file[..0x300], &[1; 64], BLOB);
        case.options.expected_vmlinuz_sha256 = sha256_hex(&file);
        let (value, exit) = evaluate(&case.options);
        assert_refused(&value, exit, "vmlinuz_sha256_mismatch", "vmlinuz_sha256");
    }

    #[test]
    fn a_big_initramfs_is_refused() {
        // Larger than guest RAM: refused by the cheap bound, before od-machine is called.
        let case = Case::new("initrd-ram", &bzimage(), &vec![0u8; 129 << 20], BLOB);
        let (value, exit) = evaluate(&case.options);
        assert_refused(&value, exit, "initrd_too_large", "initrd_size");
        // Inside RAM but with no room above the kernel window: refused by the placement.
        let case = Case::new("initrd-room", &bzimage(), &vec![0u8; 120 << 20], BLOB);
        let (value, exit) = evaluate(&case.options);
        assert_refused(&value, exit, "initrd_too_large", "placement");
        assert!(
            value["error"]["detail"]
                .as_str()
                .unwrap()
                .contains("NoRoomForInitrd")
        );
        // Just inside the limit loads.
        let case = Case::new("initrd-fit", &bzimage(), &vec![0u8; 100 << 20], BLOB);
        let (value, exit) = evaluate(&case.options);
        assert_eq!(exit, 0, "{value:#}");
    }

    #[test]
    fn the_initramfs_size_is_checked_before_it_is_read() {
        let dir = std::env::temp_dir().join(format!(
            "od-runner-linux-direct-{}-sparse",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("huge");
        // A sparse 1 TiB file: refused from its metadata, nothing is read into memory.
        std::fs::File::create(&path)
            .unwrap()
            .set_len(1 << 40)
            .unwrap();
        let path = path.to_string_lossy().into_owned();
        assert_eq!(read_capped(&path, 128 * MIB), Err(TOO_LARGE.to_string()));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_wrong_initramfs_hash_is_refused() {
        let mut case = Case::good("initramfs-sha");
        case.options.expected_initramfs_sha256 = "f".repeat(64);
        let (value, exit) = evaluate(&case.options);
        assert_refused(
            &value,
            exit,
            "initramfs_sha256_mismatch",
            "initramfs_sha256",
        );
    }

    #[test]
    fn a_missing_file_is_invalid_input_with_exit_3() {
        let mut case = Case::good("missing");
        case.options.initramfs = case.dir.join("nope").to_string_lossy().into_owned();
        let (value, exit) = evaluate(&case.options);
        assert_eq!(exit, 3, "{value:#}");
        assert_eq!(code_of(&value), ("invalid_input", "initramfs_file"));
        assert_eq!(value["retired"], 0);
    }

    #[test]
    fn arguments_are_strict() {
        let hash = "a".repeat(64);
        let base = |extra: &[&str]| {
            let mut v: Vec<String> = [
                "--vmlinuz",
                "k",
                "--initramfs",
                "i",
                "--acpi-blob",
                "b",
                "--nonce",
                "n1",
                "--expected-vmlinuz-sha256",
                hash.as_str(),
                "--expected-acpi-manifest-hash",
                hash.as_str(),
                "--ram-mib",
                "128",
                "--out",
                "o",
                "--expected-initramfs-sha256",
                hash.as_str(),
            ]
            .iter()
            .map(|s| s.to_string())
            .collect();
            v.extend(extra.iter().map(|s| s.to_string()));
            v
        };
        let ok = parse(&base(&[])).unwrap();
        assert_eq!((ok.ram_mib, ok.nonce.as_str()), (128, "n1"));
        assert_eq!(ok.expected_initramfs_sha256, hash);
        for bad in [
            base(&["--extra", "1"]),
            base(&["--nonce", "again"]),
            base(&["--expected-initramfs-sha256", &hash]),
            base(&["--ram-mib"]),
            base(&["--out"]),
            Vec::new(),
        ] {
            assert!(parse(&bad).is_err(), "{bad:?}");
        }
        // The initramfs hash is required, like the kernel's.
        let mut without = base(&[]);
        without.truncate(without.len() - 2);
        assert!(
            parse(&without)
                .unwrap_err()
                .contains("--expected-initramfs-sha256 is required")
        );
        let mut bad_hash = base(&[]);
        *bad_hash.last_mut().unwrap() = "xyz".to_string();
        assert!(parse(&bad_hash).is_err());
        // A value that starts with `--` is the next flag, not a value.
        let mut args = base(&[]);
        let at = args.iter().position(|a| a == "--out").unwrap() + 1;
        args[at] = "--ram-mib".to_string();
        assert_eq!(parse(&args).unwrap_err(), "флаг --out требует значение");
        // A flag with no value at the end says the same.
        assert_eq!(
            parse(&base(&["--out"])).unwrap_err(),
            "флаг --out требует значение"
        );
        // The RAM size is capped.
        let mut args = base(&[]);
        let at = args.iter().position(|a| a == "--ram-mib").unwrap() + 1;
        args[at] = MAX_RAM_MIB.to_string();
        assert_eq!(parse(&args).unwrap().ram_mib, MAX_RAM_MIB);
        args[at] = (MAX_RAM_MIB + 1).to_string();
        assert!(
            parse(&args)
                .unwrap_err()
                .contains("above the limit of 8192 MiB")
        );
        // Nonce shape: it becomes a command line token.
        for nonce in ["", "a b", "a=b", "x\0y", &"n".repeat(65)] {
            let mut args = base(&[]);
            let at = args.iter().position(|a| a == "--nonce").unwrap() + 1;
            args[at] = nonce.to_string();
            assert!(parse(&args).is_err(), "{nonce:?}");
        }
        let mut args = base(&[]);
        let at = args.iter().position(|a| a == "--ram-mib").unwrap() + 1;
        for ram in ["0", "x", "-1"] {
            args[at] = ram.to_string();
            assert!(parse(&args).is_err(), "{ram}");
        }
    }

    #[test]
    fn run_and_budget_are_parsed_together() {
        let hash = "a".repeat(64);
        let base = |extra: &[&str]| {
            let mut v: Vec<String> = [
                "--vmlinuz",
                "k",
                "--initramfs",
                "i",
                "--acpi-blob",
                "b",
                "--nonce",
                "n1",
                "--expected-vmlinuz-sha256",
                hash.as_str(),
                "--expected-acpi-manifest-hash",
                hash.as_str(),
                "--ram-mib",
                "128",
                "--out",
                "o",
                "--expected-initramfs-sha256",
                hash.as_str(),
            ]
            .iter()
            .map(|s| s.to_string())
            .collect();
            v.extend(extra.iter().map(|s| s.to_string()));
            v
        };
        assert_eq!(parse(&base(&[])).unwrap().budget, None);
        assert_eq!(
            parse(&base(&["--run", "--budget", "1000"])).unwrap().budget,
            Some(1000)
        );
        // `--run` may stand anywhere, and a budget of 0 is allowed.
        assert_eq!(
            parse(&base(&["--budget", "0", "--run"])).unwrap().budget,
            Some(0)
        );
        // `--set-cpl0` (S4-38L3) is a flag without a value and needs `--run`.
        assert!(!parse(&base(&["--run", "--budget", "1"])).unwrap().set_cpl0);
        assert!(
            parse(&base(&["--set-cpl0", "--run", "--budget", "1"]))
                .unwrap()
                .set_cpl0
        );
        for bad in [
            base(&["--set-cpl0"]),
            base(&["--set-cpl0", "--budget", "1"]),
            base(&["--run", "--set-cpl0", "--set-cpl0", "--budget", "1"]),
            base(&["--run", "--budget", "1", "--set-cpl0", "x"]),
            base(&["--run"]),
            base(&["--budget", "5"]),
            base(&["--run", "--run", "--budget", "5"]),
            base(&["--run", "--budget", "x"]),
            base(&["--run", "--budget", "-1"]),
            base(&["--run", "--budget", "+5"]),
            base(&["--run", "--budget", ""]),
            base(&["--run", "--budget"]),
        ] {
            assert!(parse(&bad).is_err(), "{bad:?}");
        }
    }

    fn run_case(name: &str, code: &[u8], budget: u64) -> Value {
        let mut case = Case::new(name, &bzimage_with_entry(code), &[0xAB; 4688], BLOB);
        case.options.budget = Some(budget);
        let (value, exit) = evaluate(&case.options);
        assert_eq!(exit, 0, "{value:#}");
        assert_eq!(value["status"], "loaded");
        value
    }

    #[test]
    fn without_run_the_guest_is_not_started_and_there_is_no_stop_record() {
        let case = Case::good("no-run");
        let (value, exit) = evaluate(&case.options);
        assert_eq!(exit, 0, "{value:#}");
        assert_eq!(value["guest_started"], false);
        assert_eq!(value["retired"], 0);
        assert!(value.get("stop").is_none());
    }

    #[test]
    fn run_reports_the_stop_record_of_each_exit() {
        // CLD retires in the base profile; CLI is the unsupported boundary.
        let value = run_case("run-cld", &[0xFC, 0xFA, 0x31, 0xC0], 100);
        assert_eq!(value["guest_started"], true);
        assert_eq!(value["retired"], 1);
        let stop = &value["stop"];
        assert_eq!(stop["exit_kind"], "unsupported");
        assert_eq!(stop["machine_stop_reason"], Value::Null);
        assert_eq!(stop["unsupported_opcode"], "0xfa");
        assert_eq!(stop["rip"], "0x1000201");
        assert_eq!(stop["rip_gpa"], "0x1000201");
        assert!(stop["rip_bytes"].as_str().unwrap().starts_with("fa 31 c0"));
        assert_eq!(stop["retired"], 1);
        assert_eq!(stop["budget"], 100);
        assert_eq!(
            (stop["mode"].as_str(), stop["cpl"].as_u64()),
            (Some("long64"), Some(0))
        );
        assert_eq!(stop["cr3"], "0x130000");
        assert_eq!(stop["linux_profile"], true);
        assert_eq!(
            stop["inputs"]["vmlinuz_sha256"],
            sha256_hex(&bzimage_with_entry(&[0xFC, 0xFA, 0x31, 0xC0]))
        );
        assert_eq!(
            stop["inputs"]["initramfs_sha256"],
            sha256_hex(&[0xAB; 4688])
        );
        // The entry registers of the loader stay as they were.
        assert_eq!(value["entry"]["rip"], "0x1000200");

        // Budget 0: stops before the first fetch.
        let value = run_case("run-b0", &[0xF4], 0);
        assert_eq!(value["stop"]["exit_kind"], "budget_exhausted");
        assert_eq!(value["retired"], 0);

        // HLT as the first byte; and MOV/OUT/HLT writes COM1.
        let value = run_case("run-hlt", &[0xF4], 100);
        assert_eq!(value["stop"]["exit_kind"], "halted");
        assert_eq!(value["retired"], 1);
        assert_eq!(value["stop"]["rip"], "0x1000201");
        let value = run_case(
            "run-com1",
            &[0xb0, b'X', 0x66, 0xba, 0xf8, 0x03, 0xee, 0xf4],
            100,
        );
        assert_eq!(value["stop"]["exit_kind"], "halted");
        assert_eq!(value["stop"]["com1_text"], "X");
        assert_eq!(value["stop"]["com1_hex"], "58");
        assert_eq!(value["retired"], 4);
    }

    #[test]
    fn stop_json_reports_per_run_virtual_time_source_reads() {
        let code = [
            0x66, 0xba, 0x61, 0x00, 0xb0, 0x01, 0xee, 0x66, 0xba, 0x43, 0x00, 0xb0, 0xa0, 0xee,
            0x66, 0xba, 0x42, 0x00, 0xb0, 0x10, 0xee, 0x0f, 0x31, 0x66, 0xba, 0x61, 0x00, 0xec,
            0x66, 0xba, 0x42, 0x00, 0xec, 0x66, 0xba, 0x08, 0x06, 0xec, 0xf4,
        ];
        let mut case = Case::new(
            "run-virtual-time-source-reads",
            &bzimage_with_entry(&code),
            &[0xAB; 4688],
            BLOB,
        );
        case.options.budget = Some(64);
        case.options.set_cpl0 = true;
        let (value, exit) = evaluate(&case.options);
        assert_eq!(exit, 0, "{value:#}");
        assert_eq!(value["status"], "loaded");
        assert_eq!(value["stop"]["exit_kind"], "unsupported", "{value:#}");
        assert_eq!(value["stop"]["unsupported_opcode"], "0xf4");
        assert_eq!(value["stop"]["pm_timer_reads"], 1);
        assert_eq!(value["stop"]["rdtsc_reads"], 1);
        assert_eq!(value["stop"]["rdtsc_reads_by_cpl"], json!([1, 0, 0, 0]));
        assert_eq!(value["stop"]["pit_port_b_reads"], 1);
        assert_eq!(value["stop"]["pit_counter_port_reads"], 1);
    }

    #[test]
    fn linux_direct_json_preserves_shutdown_reason_separately_from_exit_kind() {
        let run_pm_case = |name: &str, code: &[u8]| {
            let mut case = Case::new(name, &bzimage_with_entry(code), &[0xAB; 4688], BLOB);
            case.options.budget = Some(100);
            case.options.set_cpl0 = true;
            let (value, exit) = evaluate(&case.options);
            assert_eq!(exit, 0, "{value:#}");
            value
        };
        // PM1_CNT SLP_TYP=S5 and SLP_EN. Both PM paths keep the established `stopped` exit kind;
        // the shutdown reason is additive evidence and does not change scenario acceptance.
        let shutdown = run_pm_case(
            "run-shutdown-reason",
            &[0x66, 0xBA, 0x05, 0x06, 0xB0, 0x34, 0xEE, 0xF4],
        );
        assert_eq!(shutdown["stop"]["exit_kind"], "stopped");
        assert_eq!(shutdown["stop"]["machine_stop_reason"], "shutdown");

        let halt = run_case("run-no-pm-reason", &[0xF4], 100);
        assert_eq!(halt["stop"]["exit_kind"], "halted");
        assert_eq!(halt["stop"]["machine_stop_reason"], Value::Null);

        // The serializer mapping covers Reset, but this Linux-direct instruction profile does
        // not admit the reset register's byte-width OUT form, so this is not guest reset evidence.
        assert_eq!(
            machine_stop_reason_json(Some(MachineStopReason::Reset)),
            "reset"
        );
        assert_eq!(machine_stop_reason_json(None), Value::Null);
    }

    #[test]
    fn a_failed_run_is_not_reported_as_not_started() {
        // `run_failed` cannot be provoked through the CLI (it needs a clock at its maximum), so
        // the evidence builder is checked directly with the error of the machine.
        let error = LinuxDirectError {
            code: "run_failed",
            stage: "run",
            detail: "Clock(TimeOverflow)".to_string(),
            retired: Some(7),
        };
        let base = json!({"retired": 0, "guest_started": false, "nonce": "n"});
        let value = run_failure(&error, base.clone());
        assert_eq!(value["status"], "refused");
        assert_eq!(code_of(&value), ("run_failed", "run"));
        assert_eq!(value["guest_started"], true);
        assert_eq!(value["retired"], 7);
        assert_eq!(value["error"]["detail"], "Clock(TimeOverflow)");
        assert_eq!(value["nonce"], "n");
        // An unknown count is null with a note, never a false 0.
        let unknown = LinuxDirectError {
            retired: None,
            ..error
        };
        let value = run_failure(&unknown, base);
        assert_eq!(value["guest_started"], true);
        assert_eq!(value["retired"], Value::Null);
        assert!(
            value["error"]["detail"]
                .as_str()
                .unwrap()
                .ends_with("instructions retired before the error: unknown")
        );
    }

    #[test]
    fn set_cpl0_runs_the_set_and_reports_its_path_and_members() {
        // CLD; CLI; XOR EAX,EAX; MOV DS,AX: the set moves the stop past MOV DS.
        let code = [0xFC, 0xFA, 0x31, 0xC0, 0x8E, 0xD8];
        let mut case = Case::new("set-cpl0", &bzimage_with_entry(&code), &[0xAB; 4688], BLOB);
        case.options.budget = Some(64);
        case.options.set_cpl0 = true;
        let (value, exit) = evaluate(&case.options);
        assert_eq!(exit, 0, "{value:#}");
        let stop = &value["stop"];
        assert_eq!(stop["path"], "cpl0_set");
        assert_eq!(
            stop["set_members"],
            json!([
                "cli",
                "cpu_owned_cr3",
                "cpuid:DiagnosticV1",
                "rdtsc",
                "msr:EferReadOnlyV1(3328)",
                "msr:fs_gs_base",
                "msr:star_write",
                "msr:lstar_write",
                "msr:cstar_write",
                "msr:sysenter_cs_write",
                "msr:sysenter_stack_write",
                "msr:syscall_mask_write",
                "msr:kernel_gs_base_write",
                "pio_u8",
                "sti",
                "popf_cpl0",
                "pci_config",
                "irq"
            ])
        );
        assert_eq!(stop["cpuid_divergence_leaf"], Value::Null);
        // S4-38L8b: the interrupt hook counters are in the record and zero (no source yet).
        for counter in [
            "irqs_delivered",
            "irq0_edges",
            "irq_refused_masked",
            "irq_refused_shadow",
            "quantum_cut_by_deadline",
            "halt_wakes",
            "halt_idle_ns",
        ] {
            assert_eq!(stop[counter], 0, "{counter}");
        }
        // Since S2-46 the null `MOV DS, AX` retires over the loader's segment descriptor caches
        // (four instructions in all); the stop is the REX-prefix filler after the code, whose
        // 15-byte window has no opcode: #GP(0).
        assert_eq!(stop["exit_kind"], "fault", "{value:#}");
        assert_eq!(
            stop["exit_detail"],
            "Fault(ArchitecturalFault { vector: 13, error_code: Some(0), linear_address: None })"
        );
        assert_eq!(stop["retired"], 4);
        assert_eq!(stop["rip"], "0x1000206");
        assert_eq!(stop["cr3_changes"], 0);
        assert_eq!(stop["last_cr3_change"], Value::Null);
        // The default path retires CLD and stops at CLI.
        let value = run_case("set-cpl0-control", &code, 64);
        assert_eq!(value["stop"]["path"], "paged_probe");
        assert_eq!(value["stop"]["unsupported_opcode"], "0xfa");
        assert_eq!(value["stop"]["retired"], 1);
        assert_eq!(value["stop"]["set_members"], json!([]));
    }

    #[test]
    fn set_cpl0_reports_the_guest_efer_after_a_wrmsr_of_sce() {
        // MOV ECX,0xC0000080; MOV EAX,0xD01; XOR EDX,EDX; WRMSR (S4-38L7f).
        let code = [
            0xB9, 0x80, 0x00, 0x00, 0xC0, 0xB8, 0x01, 0x0D, 0x00, 0x00, 0x31, 0xD2, 0x0F, 0x30,
        ];
        let mut case = Case::new("set-efer", &bzimage_with_entry(&code), &[0xAB; 4688], BLOB);
        case.options.budget = Some(4);
        case.options.set_cpl0 = true;
        let (value, exit) = evaluate(&case.options);
        assert_eq!(exit, 0, "{value:#}");
        let stop = &value["stop"];
        assert_eq!(stop["retired"], 4, "{value:#}");
        assert_eq!(stop["guest_efer"], "0xd01");
        // `efer` stays the walker's EFER.
        assert_eq!(stop["efer"], "0xd00");
        // Without the set nothing writes EFER: both read 0xd00.
        let value = run_case("efer-control", &[0xF4], 10);
        assert_eq!(value["stop"]["guest_efer"], "0xd00");
        assert_eq!(value["stop"]["efer"], "0xd00");
    }

    #[test]
    fn set_cpl0_reports_the_guest_star_after_a_wrmsr_of_ia32_star() {
        // MOV ECX,0xC0000081; XOR EAX,EAX; MOV EDX,0x230010; WRMSR (S4-38L7ao).
        let code = [
            0xB9, 0x81, 0x00, 0x00, 0xC0, 0x31, 0xC0, 0xBA, 0x10, 0x00, 0x23, 0x00, 0x0F, 0x30,
        ];
        let mut case = Case::new("set-star", &bzimage_with_entry(&code), &[0xAB; 4688], BLOB);
        case.options.budget = Some(4);
        case.options.set_cpl0 = true;
        let (value, exit) = evaluate(&case.options);
        assert_eq!(exit, 0, "{value:#}");
        let stop = &value["stop"];
        assert_eq!(stop["retired"], 4, "{value:#}");
        assert_eq!(stop["guest_star"], "0x23001000000000");
        // Without the set nothing writes STAR.
        let value = run_case("star-control", &[0xF4], 10);
        assert_eq!(value["stop"]["guest_star"], "0x0");
    }

    #[test]
    fn set_cpl0_reports_the_guest_lstar_after_a_wrmsr_of_ia32_lstar() {
        // MOV ECX,0xC0000082; MOV EAX,0x81000080; MOV EDX,0xFFFFFFFF; WRMSR (S4-38L7ap).
        let code = [
            0xB9, 0x82, 0x00, 0x00, 0xC0, 0xB8, 0x80, 0x00, 0x00, 0x81, 0xBA, 0xFF, 0xFF, 0xFF,
            0xFF, 0x0F, 0x30,
        ];
        let mut case = Case::new("set-lstar", &bzimage_with_entry(&code), &[0xAB; 4688], BLOB);
        case.options.budget = Some(4);
        case.options.set_cpl0 = true;
        let (value, exit) = evaluate(&case.options);
        assert_eq!(exit, 0, "{value:#}");
        let stop = &value["stop"];
        assert_eq!(stop["retired"], 4, "{value:#}");
        assert_eq!(stop["guest_lstar"], "0xffffffff81000080");
        assert_eq!(stop["guest_star"], "0x0");
        // Without the set nothing writes LSTAR.
        let value = run_case("lstar-control", &[0xF4], 10);
        assert_eq!(value["stop"]["guest_lstar"], "0x0");
    }

    #[test]
    fn set_cpl0_reports_the_guest_cstar_after_a_wrmsr_of_ia32_cstar() {
        // MOV ECX,0xC0000083; MOV EAX,0x81001AB0; MOV EDX,0xFFFFFFFF; WRMSR (S4-38L7aq).
        let code = [
            0xB9, 0x83, 0x00, 0x00, 0xC0, 0xB8, 0xB0, 0x1A, 0x00, 0x81, 0xBA, 0xFF, 0xFF, 0xFF,
            0xFF, 0x0F, 0x30,
        ];
        let mut case = Case::new("set-cstar", &bzimage_with_entry(&code), &[0xAB; 4688], BLOB);
        case.options.budget = Some(4);
        case.options.set_cpl0 = true;
        let (value, exit) = evaluate(&case.options);
        assert_eq!(exit, 0, "{value:#}");
        let stop = &value["stop"];
        assert_eq!(stop["retired"], 4, "{value:#}");
        assert_eq!(stop["guest_cstar"], "0xffffffff81001ab0");
        assert_eq!(stop["guest_star"], "0x0");
        assert_eq!(stop["guest_lstar"], "0x0");
        // Without the set nothing writes CSTAR.
        let value = run_case("cstar-control", &[0xF4], 10);
        assert_eq!(value["stop"]["guest_cstar"], "0x0");
    }

    #[test]
    fn set_cpl0_reports_the_guest_sysenter_cs_after_a_wrmsr_of_ia32_sysenter_cs() {
        // MOV ECX,0x174; MOV EAX,0x10; XOR EDX,EDX; WRMSR (S4-38L7ar).
        let code = [
            0xB9, 0x74, 0x01, 0x00, 0x00, 0xB8, 0x10, 0x00, 0x00, 0x00, 0x31, 0xD2, 0x0F, 0x30,
        ];
        let mut case = Case::new(
            "set-sysenter-cs",
            &bzimage_with_entry(&code),
            &[0xAB; 4688],
            BLOB,
        );
        case.options.budget = Some(4);
        case.options.set_cpl0 = true;
        let (value, exit) = evaluate(&case.options);
        assert_eq!(exit, 0, "{value:#}");
        let stop = &value["stop"];
        assert_eq!(stop["retired"], 4, "{value:#}");
        assert_eq!(stop["guest_sysenter_cs"], "0x10");
        assert_eq!(stop["guest_star"], "0x0");
        assert_eq!(stop["guest_lstar"], "0x0");
        assert_eq!(stop["guest_cstar"], "0x0");
        // Without the set nothing writes SYSENTER_CS.
        let value = run_case("sysenter-cs-control", &[0xF4], 10);
        assert_eq!(value["stop"]["guest_sysenter_cs"], "0x0");
    }

    #[test]
    fn set_cpl0_reports_the_guest_sysenter_esp_and_eip_after_their_wrmsr() {
        // MOV ECX,0x175; MOV EAX,0x3000; MOV EDX,0xFFFFFE00; WRMSR; MOV ECX,0x176;
        // MOV EAX,0x81000010; WRMSR (S4-38L7as).
        let code = [
            0xB9, 0x75, 0x01, 0x00, 0x00, 0xB8, 0x00, 0x30, 0x00, 0x00, 0xBA, 0x00, 0xFE, 0xFF,
            0xFF, 0x0F, 0x30, 0xB9, 0x76, 0x01, 0x00, 0x00, 0xB8, 0x10, 0x00, 0x00, 0x81, 0x0F,
            0x30,
        ];
        let mut case = Case::new(
            "set-sysenter-stack",
            &bzimage_with_entry(&code),
            &[0xAB; 4688],
            BLOB,
        );
        case.options.budget = Some(7);
        case.options.set_cpl0 = true;
        let (value, exit) = evaluate(&case.options);
        assert_eq!(exit, 0, "{value:#}");
        let stop = &value["stop"];
        assert_eq!(stop["retired"], 7, "{value:#}");
        assert_eq!(stop["guest_sysenter_esp"], "0xfffffe0000003000");
        assert_eq!(stop["guest_sysenter_eip"], "0xfffffe0081000010");
        assert_eq!(stop["guest_sysenter_cs"], "0x0");
        assert_eq!(stop["guest_star"], "0x0");
        assert_eq!(stop["guest_lstar"], "0x0");
        assert_eq!(stop["guest_cstar"], "0x0");
        // Without the set nothing writes them.
        let value = run_case("sysenter-stack-control", &[0xF4], 10);
        assert_eq!(value["stop"]["guest_sysenter_esp"], "0x0");
        assert_eq!(value["stop"]["guest_sysenter_eip"], "0x0");
    }

    #[test]
    fn set_cpl0_reports_the_guest_syscall_mask_after_its_wrmsr() {
        // MOV ECX,0xC0000084; MOV EAX,0x257FD5; XOR EDX,EDX; WRMSR (S4-38L7at).
        let code = [
            0xB9, 0x84, 0x00, 0x00, 0xC0, 0xB8, 0xD5, 0x7F, 0x25, 0x00, 0x31, 0xD2, 0x0F, 0x30,
        ];
        let mut case = Case::new(
            "set-syscall-mask",
            &bzimage_with_entry(&code),
            &[0xAB; 4688],
            BLOB,
        );
        case.options.budget = Some(4);
        case.options.set_cpl0 = true;
        let (value, exit) = evaluate(&case.options);
        assert_eq!(exit, 0, "{value:#}");
        let stop = &value["stop"];
        assert_eq!(stop["retired"], 4, "{value:#}");
        assert_eq!(stop["guest_syscall_mask"], "0x257fd5");
        assert_eq!(stop["guest_sysenter_cs"], "0x0");
        assert_eq!(stop["guest_sysenter_esp"], "0x0");
        assert_eq!(stop["guest_sysenter_eip"], "0x0");
        assert_eq!(stop["guest_star"], "0x0");
        assert_eq!(stop["guest_lstar"], "0x0");
        assert_eq!(stop["guest_cstar"], "0x0");
        // Without the set nothing writes it.
        let value = run_case("syscall-mask-control", &[0xF4], 10);
        assert_eq!(value["stop"]["guest_syscall_mask"], "0x0");
    }

    #[test]
    fn set_cpl0_reports_the_guest_kernel_gs_base_after_its_wrmsr() {
        // MOV ECX,0xC0000102; MOV EAX,0x12345678; MOV EDX,0xFFFF8880; WRMSR (S4-38L7au).
        let code = [
            0xB9, 0x02, 0x01, 0x00, 0xC0, 0xB8, 0x78, 0x56, 0x34, 0x12, 0xBA, 0x80, 0x88, 0xFF,
            0xFF, 0x0F, 0x30,
        ];
        let mut case = Case::new(
            "set-kernel-gs-base",
            &bzimage_with_entry(&code),
            &[0xAB; 4688],
            BLOB,
        );
        case.options.budget = Some(4);
        case.options.set_cpl0 = true;
        let (value, exit) = evaluate(&case.options);
        assert_eq!(exit, 0, "{value:#}");
        let stop = &value["stop"];
        assert_eq!(stop["retired"], 4, "{value:#}");
        assert_eq!(stop["guest_kernel_gs_base"], "0xffff888012345678");
        assert_eq!(stop["guest_syscall_mask"], "0x0");
        assert_eq!(stop["guest_sysenter_cs"], "0x0");
        assert_eq!(stop["guest_sysenter_esp"], "0x0");
        assert_eq!(stop["guest_sysenter_eip"], "0x0");
        assert_eq!(stop["guest_star"], "0x0");
        assert_eq!(stop["guest_lstar"], "0x0");
        assert_eq!(stop["guest_cstar"], "0x0");
        // Without the set nothing writes it.
        let value = run_case("kernel-gs-base-control", &[0xF4], 10);
        assert_eq!(value["stop"]["guest_kernel_gs_base"], "0x0");
    }

    #[test]
    fn set_cpl0_reports_the_pci_config_address_after_an_out_to_0xcf8() {
        // MOV EAX,0x80001000; MOV EDX,0xCF8; OUT DX,EAX (S4-38L7w).
        let code = [
            0xB8, 0x00, 0x10, 0x00, 0x80, 0xBA, 0xF8, 0x0C, 0x00, 0x00, 0xEF,
        ];
        let mut case = Case::new("set-pci", &bzimage_with_entry(&code), &[0xAB; 4688], BLOB);
        case.options.budget = Some(3);
        case.options.set_cpl0 = true;
        let (value, exit) = evaluate(&case.options);
        assert_eq!(exit, 0, "{value:#}");
        let stop = &value["stop"];
        assert_eq!(stop["retired"], 3, "{value:#}");
        assert_eq!(stop["pci_config_address"], "0x80001000");
        // Without the set nothing writes CONFIG_ADDRESS.
        let value = run_case("pci-control", &[0xF4], 10);
        assert_eq!(value["stop"]["pci_config_address"], "0x0");
    }

    #[test]
    fn set_cpl0_stops_before_a_divergent_cpuid_leaf() {
        // CLI; MOV EAX, 1; CPUID; HLT: the CPUID boundary is the stop, before the instruction.
        let code = [0xFA, 0xB8, 0x01, 0x00, 0x00, 0x00, 0x0F, 0xA2, 0xF4];
        let mut case = Case::new(
            "set-cpl0-cpuid",
            &bzimage_with_entry(&code),
            &[0xAB; 4688],
            BLOB,
        );
        case.options.budget = Some(64);
        case.options.set_cpl0 = true;
        let (value, exit) = evaluate(&case.options);
        assert_eq!(exit, 0, "{value:#}");
        let stop = &value["stop"];
        assert_eq!(stop["exit_kind"], "cpuid_divergence", "{value:#}");
        assert_eq!(stop["cpuid_divergence_leaf"], "0x1");
        assert_eq!(stop["unsupported_opcode"], Value::Null);
        assert_eq!(stop["retired"], 2);
        assert_eq!(value["retired"], 2);
        assert_eq!(stop["rip"], "0x1000206");
        assert!(stop["rip_bytes"].as_str().unwrap().starts_with("0f a2 f4"));
        assert_eq!(stop["gpr"]["rax"], "0x1");
        assert_eq!(stop["set_exit_detail"], "CpuidDivergence { leaf: 1 }");
    }

    #[test]
    fn set_cpl0_reports_an_iretq_to_cpl3_as_a_user_entry_stop_with_its_frame() {
        // S4-38L9I: DPL 3 descriptors over the board GDT entries 0x10/0x18 (selectors 0x13 and
        // 0x1b), then PUSH SS, RSP, RFLAGS, CS, RIP and IRETQ: the frame is the stop.
        let mut code = Vec::new();
        for (at, qword) in [
            (0x12_0010u32, 0x00af_fb00_0000_ffff_u64),
            (0x12_0018, 0x00cf_f300_0000_ffff),
        ] {
            code.extend([0x48, 0xB8]);
            code.extend(qword.to_le_bytes());
            code.extend([0x48, 0x89, 0x04, 0x25]);
            code.extend(at.to_le_bytes());
        }
        code.extend([0x6A, 0x1B]);
        code.extend([0x68, 0x00, 0xF0, 0xFF, 0x7F]);
        code.extend([0x68, 0x02, 0x02, 0x00, 0x00]);
        code.extend([0x6A, 0x13]);
        code.extend([0x68, 0x00, 0x10, 0x00, 0x00]);
        code.extend([0x48, 0xCF]);
        let mut case = Case::new(
            "set-user-entry",
            &bzimage_with_entry(&code),
            &[0xAB; 4688],
            BLOB,
        );
        case.options.budget = Some(64);
        case.options.set_cpl0 = true;
        let (value, exit) = evaluate(&case.options);
        assert_eq!(exit, 0, "{value:#}");
        let stop = &value["stop"];
        assert_eq!(stop["exit_kind"], "user_entry", "{value:#}");
        assert_eq!(stop["user_entry"]["rip"], "0x1000");
        assert_eq!(stop["user_entry"]["cs"], "0x13");
        assert_eq!(stop["user_entry"]["rflags"], "0x202");
        assert_eq!(stop["user_entry"]["rsp"], "0x7ffff000");
        assert_eq!(stop["user_entry"]["ss"], "0x1b");
        assert_eq!(stop["unsupported_opcode"], Value::Null);
        assert_eq!(stop["cpuid_divergence_leaf"], Value::Null);
        assert_eq!(stop["retired"], 9, "{value:#}");
        assert!(stop["rip_bytes"].as_str().unwrap().starts_with("48 cf"));
        assert!(
            stop["set_exit_detail"]
                .as_str()
                .unwrap()
                .starts_with("UserEntry(")
        );
        // Every other stop carries no frame.
        let value = run_case("user-entry-control", &[0xF4], 10);
        assert!(value["stop"].get("user_entry").is_none(), "{value:#}");
    }

    /// `xor eax, eax; mov ds, ax; mov es, ax` (the `startup_64` prologue: null data segments, which
    /// the committed return to CPL3 needs), the board GDT entries 0x10/0x18 as DPL 3 descriptors,
    /// then the frame (CS 0x13, SS 0x1b, RIP 0x1000, RFLAGS 0x202) and the `IRETQ`.
    fn iretq_to_cpl3_program() -> Vec<u8> {
        let mut code = vec![0x31, 0xC0, 0x8E, 0xD8, 0x8E, 0xC0];
        for (at, qword) in [
            (0x12_0010u32, 0x00af_fb00_0000_ffff_u64),
            (0x12_0018, 0x00cf_f300_0000_ffff),
        ] {
            code.extend([0x48, 0xB8]);
            code.extend(qword.to_le_bytes());
            code.extend([0x48, 0x89, 0x04, 0x25]);
            code.extend(at.to_le_bytes());
        }
        code.extend([0x6A, 0x1B]);
        code.extend([0x68, 0x00, 0xF0, 0xFF, 0x7F]);
        code.extend([0x68, 0x02, 0x02, 0x00, 0x00]);
        code.extend([0x6A, 0x13]);
        code.extend([0x68, 0x00, 0x10, 0x00, 0x00]);
        code.extend([0x48, 0xCF]);
        code
    }

    #[test]
    fn user_mode_flag_needs_the_stage4a_profile_and_defaults_off() {
        let run = ["--run", "--budget", "1", "--set-cpl0"];
        let with = |tail: &[&str]| parse_base(&[&run[..], tail].concat());
        assert!(!with(&[]).unwrap().user_mode);
        assert!(!with(&["--cpuid-profile", "stage4a"]).unwrap().user_mode);
        let ok = with(&["--cpuid-profile", "stage4a", "--user-mode"]).unwrap();
        assert!(ok.user_mode);
        assert_eq!(ok.cpuid_profile, CpuidProfile::X8664V1Stage4A);
        for bad in [
            vec!["--user-mode"],
            vec!["--cpuid-profile", "diagnostic", "--user-mode"],
            vec!["--cpuid-profile", "stage4a", "--user-mode", "--user-mode"],
        ] {
            assert!(with(&bad).is_err(), "{bad:?}");
        }
        // Needs --run and --set-cpl0 as well.
        assert!(parse_base(&["--user-mode"]).is_err());
        assert!(parse_base(&["--run", "--budget", "1", "--user-mode"]).is_err());
        assert!(
            parse_base(&[
                "--run",
                "--budget",
                "1",
                "--cpuid-profile",
                "stage4a",
                "--user-mode"
            ])
            .is_err()
        );
    }

    #[test]
    fn nonce_bootconfig_loads_an_aligned_trailer_without_changing_the_file_or_early_cmdline() {
        let mut case = Case::good("nonce-bootconfig");
        let fixture = std::fs::read(&case.options.initramfs).unwrap();
        let profile = [
            "--run",
            "--budget",
            "1",
            "--set-cpl0",
            "--cpuid-profile",
            "stage4a",
            "--user-mode",
            "--nonce-bootconfig",
        ];
        assert!(parse_base(&profile).unwrap().nonce_bootconfig);
        assert!(parse_base(&["--nonce-bootconfig"]).is_err());
        assert!(parse_base(&[&profile[..], &["--nonce-bootconfig"]].concat()).is_err());

        case.options.nonce_bootconfig = true;
        let (first, code) = evaluate(&case.options);
        assert_eq!(code, 0, "{first}");
        case.options.nonce = "N0nce-2".into();
        let (second, code) = evaluate(&case.options);
        assert_eq!(code, 0, "{second}");
        assert_eq!(first["cmdline"], second["cmdline"]);
        assert_eq!(first["cmdline"], nonce_bootconfig_cmdline());
        for result in [&first, &second] {
            assert_eq!(
                result["inputs"]["initramfs"]["sha256"],
                sha256_hex(&fixture)
            );
            let payload = result["inputs"]["initramfs"]["nonce_bootconfig"].clone();
            let len = payload["loaded_bytes"].as_u64().unwrap();
            assert!(len > fixture.len() as u64);
            assert_eq!(len % 4, 0);
            assert_eq!(result["memory_sha256"]["initrd"], payload["loaded_sha256"]);
            assert_eq!(result["memory_sha256"]["initrd_matches_file"], true);
        }
        assert_ne!(
            first["memory_sha256"]["initrd"],
            second["memory_sha256"]["initrd"]
        );
        assert_eq!(std::fs::read(&case.options.initramfs).unwrap(), fixture);
    }

    #[test]
    fn linux_norandmaps_is_opt_in_and_requires_the_full_user_mode_profile() {
        assert!(!parse_base(&[]).unwrap().linux_norandmaps);
        let enabled = parse_base(&[
            "--run",
            "--budget",
            "1",
            "--set-cpl0",
            "--cpuid-profile",
            "stage4a",
            "--user-mode",
            "--linux-norandmaps",
        ])
        .unwrap();
        assert!(enabled.linux_norandmaps);

        for bad in [
            vec!["--linux-norandmaps"],
            vec!["--run", "--budget", "1", "--linux-norandmaps"],
            vec!["--run", "--budget", "1", "--set-cpl0", "--linux-norandmaps"],
            vec![
                "--run",
                "--budget",
                "1",
                "--set-cpl0",
                "--cpuid-profile",
                "stage4a",
                "--linux-norandmaps",
            ],
            vec![
                "--run",
                "--budget",
                "1",
                "--set-cpl0",
                "--cpuid-profile",
                "diagnostic",
                "--user-mode",
                "--linux-norandmaps",
            ],
            vec![
                "--run",
                "--budget",
                "1",
                "--set-cpl0",
                "--cpuid-profile",
                "stage4a",
                "--user-mode",
                "--linux-norandmaps",
                "--linux-norandmaps",
            ],
        ] {
            assert!(parse_base(&bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn linux_cmdline_diagnostics_are_opt_in_and_compose() {
        assert_eq!(
            cmdline("N0nce-1", false, false),
            "console=ttyS0,115200 earlycon=uart8250,io,0x3f8 8250.nr_uarts=1 od.nonce=N0nce-1 nokaslr"
        );
        assert_eq!(
            cmdline("N0nce-1", true, false),
            "console=ttyS0,115200 earlycon=uart8250,io,0x3f8 8250.nr_uarts=1 od.nonce=N0nce-1 nokaslr norandmaps"
        );
        assert_eq!(
            cmdline("N0nce-1", false, true),
            "console=ttyS0,115200 earlycon=uart8250,io,0x3f8 8250.nr_uarts=1 od.nonce=N0nce-1 nokaslr randomize_kstack_offset=off"
        );
        assert_eq!(
            cmdline("N0nce-1", true, true),
            "console=ttyS0,115200 earlycon=uart8250,io,0x3f8 8250.nr_uarts=1 od.nonce=N0nce-1 nokaslr norandmaps randomize_kstack_offset=off"
        );
    }

    #[test]
    fn linux_no_randomize_kstack_offset_requires_the_full_user_mode_profile() {
        assert!(!parse_base(&[]).unwrap().linux_no_randomize_kstack_offset);
        let enabled = parse_base(&[
            "--run",
            "--budget",
            "1",
            "--set-cpl0",
            "--cpuid-profile",
            "stage4a",
            "--user-mode",
            "--linux-no-randomize-kstack-offset",
        ])
        .unwrap();
        assert!(enabled.linux_no_randomize_kstack_offset);
        let composed = parse_base(&[
            "--run",
            "--budget",
            "1",
            "--set-cpl0",
            "--cpuid-profile",
            "stage4a",
            "--user-mode",
            "--linux-norandmaps",
            "--linux-no-randomize-kstack-offset",
        ])
        .unwrap();
        assert!(composed.linux_norandmaps && composed.linux_no_randomize_kstack_offset);

        for bad in [
            vec!["--linux-no-randomize-kstack-offset"],
            vec![
                "--run",
                "--budget",
                "1",
                "--linux-no-randomize-kstack-offset",
            ],
            vec![
                "--run",
                "--budget",
                "1",
                "--set-cpl0",
                "--linux-no-randomize-kstack-offset",
            ],
            vec![
                "--run",
                "--budget",
                "1",
                "--set-cpl0",
                "--cpuid-profile",
                "stage4a",
                "--linux-no-randomize-kstack-offset",
            ],
            vec![
                "--run",
                "--budget",
                "1",
                "--set-cpl0",
                "--cpuid-profile",
                "diagnostic",
                "--user-mode",
                "--linux-no-randomize-kstack-offset",
            ],
            vec![
                "--run",
                "--budget",
                "1",
                "--set-cpl0",
                "--cpuid-profile",
                "stage4a",
                "--user-mode",
                "--linux-no-randomize-kstack-offset",
                "--linux-no-randomize-kstack-offset",
            ],
        ] {
            assert!(parse_base(&bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn user_mode_runs_the_board_into_cpl3_and_the_record_says_so() {
        let mut case = Case::new(
            "set-user-mode",
            &bzimage_with_entry(&iretq_to_cpl3_program()),
            &[0xAB; 4688],
            BLOB,
        );
        case.options.budget = Some(64);
        case.options.set_cpl0 = true;
        case.options.cpuid_profile = CpuidProfile::X8664V1Stage4A;
        case.options.user_mode = true;
        let (value, exit) = evaluate(&case.options);
        assert_eq!(exit, 0, "{value:#}");
        let stop = &value["stop"];
        // The `IRETQ` retired and the run went on in CPL3: its first fetch (the board page is
        // supervisor-only) is a #PF with P, U and I/D set; the guest has no IDT here, so it stays
        // a plain fault (with an IDT the CPU delivery refuses it from CPL3, see od-machine).
        assert_eq!(stop["exit_kind"], "user_mode_fault", "{value:#}");
        assert_eq!(stop["cpl"], 3);
        assert_eq!(stop["user_mode"], true);
        assert_eq!(stop["cpl3_retired"], 0);
        // S4-39f: the syscall journal exists under `--user-mode` (empty here: no system call).
        assert_eq!(stop["syscall_journal"]["entered"], 0);
        assert_eq!(stop["syscall_journal"]["returned"], 0);
        assert_eq!(stop["syscall_journal"]["capacity"], 32);
        assert!(
            stop["syscall_journal"]["entries"]
                .as_array()
                .unwrap()
                .is_empty()
        );
        assert_eq!(stop["rip"], "0x1000");
        assert_eq!(stop["retired"], 13, "{value:#}");
        assert!(stop.get("user_entry").is_none(), "{value:#}");
        assert!(
            stop["exit_detail"]
                .as_str()
                .unwrap()
                .contains("vector: 14, error_code: Some(21)"),
            "{value:#}"
        );
        let members = stop["set_members"].as_array().unwrap();
        // S4-39e: the stage4a members carry the EFER, STAR, LSTAR and SYSCALL_MASK cells, so the
        // user-mode set also has the `syscall` member, listed right after `user_mode`.
        assert_eq!(members.last().unwrap(), "syscall");
        assert_eq!(members[members.len() - 2], "user_mode");
        // The same program without the flag stops at the boundary and has none of the new keys.
        case.options.user_mode = false;
        let (value, exit) = evaluate(&case.options);
        assert_eq!(exit, 0, "{value:#}");
        let stop = &value["stop"];
        assert_eq!(stop["exit_kind"], "user_entry", "{value:#}");
        assert_eq!(stop["cpl"], 0);
        assert!(stop.get("user_mode").is_none(), "{value:#}");
        assert!(stop.get("cpl3_retired").is_none(), "{value:#}");
        assert!(stop.get("syscall_journal").is_none(), "{value:#}");
        assert!(
            !stop["set_members"]
                .as_array()
                .unwrap()
                .iter()
                .any(|name| name == "user_mode")
        );
    }

    /// S4-38L9-um: the same program under `stage4a-apic --user-mode` reaches CPL3 (no refusal
    /// before the board is changed), carries the APIC members and the IRQ source, and the result
    /// stays `diagnostic_not_counted`.
    #[test]
    fn user_mode_under_the_apic_profile_runs_into_cpl3_and_is_not_counted() {
        let mut case = Case::new(
            "set-user-mode-apic",
            &bzimage_with_entry(&iretq_to_cpl3_program()),
            &[0xAB; 4688],
            BLOB,
        );
        case.options.budget = Some(64);
        case.options.set_cpl0 = true;
        case.options.cpuid_profile = CpuidProfile::X8664V1Stage4AApic;
        case.options.user_mode = true;
        let (value, exit) = evaluate(&case.options);
        assert_eq!(exit, 0, "{value:#}");
        assert_eq!(value["acceptance"], "diagnostic_not_counted", "{value:#}");
        assert_eq!(value["cpuid_profile"], "X8664V1Stage4AApic");
        let stop = &value["stop"];
        assert_eq!(stop["exit_kind"], "user_mode_fault", "{value:#}");
        assert_eq!(stop["cpl"], 3);
        assert_eq!(stop["user_mode"], true);
        let members = stop["set_members"].as_array().unwrap();
        for name in [
            "cpuid:X8664V1Stage4AApic",
            "halt",
            "irq",
            "clock_owner",
            "user_mode",
        ] {
            assert!(
                members.iter().any(|member| member == name),
                "{name}: {members:?}"
            );
        }
    }

    fn cpuid_leaf_case(name: &str, leaf: u8, diagnostic: bool) -> (Value, u8) {
        // CLI; MOV EAX, leaf; CPUID; HLT.
        let code = [0xFA, 0xB8, leaf, 0x00, 0x00, 0x00, 0x0F, 0xA2, 0xF4];
        let mut case = Case::new(name, &bzimage_with_entry(&code), &[0xAB; 4688], BLOB);
        case.options.budget = Some(64);
        case.options.set_cpl0 = true;
        case.options.diagnostic_past_c7 = diagnostic;
        evaluate(&case.options)
    }

    #[test]
    fn diagnostic_past_c7_runs_leaf_1_and_marks_the_result_as_not_counted() {
        let (value, exit) = cpuid_leaf_case("diag-c7", 1, true);
        assert_eq!(exit, 0, "{value:#}");
        // The flag is visible, at the top level and in `stop`, and the result is not counted.
        assert_eq!(value["diagnostic_past_c7"], true);
        assert_eq!(value["acceptance"], "diagnostic_not_counted");
        assert_eq!(value["schema_version"], 1);
        let stop = &value["stop"];
        assert_eq!(stop["diagnostic_past_c7"], true);
        // The CPUID executed: no boundary, the DiagnosticV1 tuple of leaf 1 is in the registers.
        assert_ne!(stop["exit_kind"], "cpuid_divergence", "{value:#}");
        assert_eq!(stop["cpuid_divergence_leaf"], Value::Null);
        assert_eq!(stop["gpr"]["rax"], "0x600");
        assert_eq!(stop["gpr"]["rbx"], "0x100");
        assert_eq!(stop["gpr"]["rcx"], "0x0");
        assert_eq!(stop["gpr"]["rdx"], "0x0");
        // CLI, MOV, CPUID retired (RIP+2) and the run went on to the next stop: the HLT, which
        // the runner set does not admit.
        assert_eq!(stop["retired"], 3, "{value:#}");
        assert_eq!(stop["rip"], "0x1000208");
        assert_eq!(stop["exit_kind"], "unsupported");
        // Same members as without the flag.
        assert_eq!(
            stop["set_members"],
            json!([
                "cli",
                "cpu_owned_cr3",
                "cpuid:DiagnosticV1",
                "rdtsc",
                "msr:EferReadOnlyV1(3328)",
                "msr:fs_gs_base",
                "msr:star_write",
                "msr:lstar_write",
                "msr:cstar_write",
                "msr:sysenter_cs_write",
                "msr:sysenter_stack_write",
                "msr:syscall_mask_write",
                "msr:kernel_gs_base_write",
                "pio_u8",
                "sti",
                "popf_cpl0",
                "pci_config",
                "irq"
            ])
        );
        // Leaf 0x80000001 as well.
        let code = [0xFA, 0xB8, 0x01, 0x00, 0x00, 0x80, 0x0F, 0xA2, 0xF4];
        let mut case = Case::new(
            "diag-c7-ext",
            &bzimage_with_entry(&code),
            &[0xAB; 4688],
            BLOB,
        );
        case.options.budget = Some(64);
        case.options.set_cpl0 = true;
        case.options.diagnostic_past_c7 = true;
        let (value, _) = evaluate(&case.options);
        assert_ne!(value["stop"]["exit_kind"], "cpuid_divergence", "{value:#}");
        assert_eq!(value["stop"]["gpr"]["rax"], "0x0");
    }

    #[test]
    fn without_the_flag_leaf_1_still_stops_at_c7_and_is_not_marked() {
        let (value, exit) = cpuid_leaf_case("no-diag-c7", 1, false);
        assert_eq!(exit, 0, "{value:#}");
        assert_eq!(value["stop"]["exit_kind"], "cpuid_divergence");
        assert_eq!(value["stop"]["diagnostic_past_c7"], false);
        assert_eq!(value["stop"]["rip"], "0x1000206");
        assert_eq!(value["stop"]["gpr"]["rax"], "0x1");
        assert_eq!(value["diagnostic_past_c7"], false, "{value:#}");
        assert_eq!(value["acceptance"], "counted", "{value:#}");
        assert!(value.get("diagnostic_note").is_none(), "{value:#}");
    }

    #[test]
    fn diagnostic_markers_are_on_every_result_including_refusals_and_run_failed() {
        // Set before anything is loaded: a run failure carries them, with and without the flag.
        let error = LinuxDirectError {
            code: "run_failed",
            stage: "run",
            detail: "Clock(TimeOverflow)".to_string(),
            retired: Some(7),
        };
        for (flag, acceptance) in [(true, "diagnostic_not_counted"), (false, "counted")] {
            let mut base = json!({"retired": 0, "guest_started": false, "nonce": "n"});
            mark_acceptance(&mut base, flag, false, false, CpuidProfile::DiagnosticV1);
            let value = run_failure(&error, base);
            assert_eq!(value["status"], "refused");
            assert_eq!(value["diagnostic_past_c7"], flag, "{value:#}");
            assert_eq!(value["acceptance"], acceptance, "{value:#}");
        }
        // An early refusal (foreign vmlinuz hash) of a run with the flag is marked too.
        let code = [0xFA, 0xF4];
        let mut case = Case::new(
            "diag-refusal",
            &bzimage_with_entry(&code),
            &[0xAB; 4688],
            BLOB,
        );
        case.options.budget = Some(1);
        case.options.set_cpl0 = true;
        case.options.diagnostic_past_c7 = true;
        case.options.expected_vmlinuz_sha256 = "0".repeat(64);
        let (value, exit) = evaluate(&case.options);
        assert_eq!(exit, 4, "{value:#}");
        assert_eq!(value["status"], "refused");
        assert_eq!(value["diagnostic_past_c7"], true);
        assert_eq!(value["acceptance"], "diagnostic_not_counted");
    }

    #[test]
    fn diagnostic_past_c7_does_not_change_other_cpuid_leaves() {
        // Leaf 0 (vendor "OpenDesktop"): the same registers with and without the flag.
        let (plain, _) = cpuid_leaf_case("leaf0-plain", 0, false);
        let (diag, _) = cpuid_leaf_case("leaf0-diag", 0, true);
        assert_eq!(plain["stop"]["exit_kind"], diag["stop"]["exit_kind"]);
        assert_eq!(plain["stop"]["gpr"], diag["stop"]["gpr"]);
        assert_eq!(plain["stop"]["retired"], diag["stop"]["retired"]);
        assert_eq!(plain["stop"]["set_members"], diag["stop"]["set_members"]);
        assert_eq!(diag["stop"]["gpr"]["rbx"], "0x6e65704f");
    }

    #[test]
    fn diagnostic_past_c7_needs_run_and_set_cpl0() {
        let base = |extra: &[&str]| -> Vec<String> {
            let mut args: Vec<String> = [
                "--vmlinuz",
                "k",
                "--initramfs",
                "i",
                "--acpi-blob",
                "a",
                "--nonce",
                "n",
                "--expected-vmlinuz-sha256",
                &"0".repeat(64),
                "--expected-acpi-manifest-hash",
                &"0".repeat(64),
                "--expected-initramfs-sha256",
                &"0".repeat(64),
                "--ram-mib",
                "128",
                "--out",
                "o",
            ]
            .iter()
            .map(|s| s.to_string())
            .collect();
            args.extend(extra.iter().map(|s| s.to_string()));
            args
        };
        let ok = parse(&base(&[
            "--run",
            "--budget",
            "1",
            "--set-cpl0",
            "--diagnostic-past-c7",
        ]))
        .unwrap();
        assert!(ok.diagnostic_past_c7);
        assert!(
            !parse(&base(&["--run", "--budget", "1", "--set-cpl0"]))
                .unwrap()
                .diagnostic_past_c7
        );
        for bad in [
            vec!["--diagnostic-past-c7"],
            vec!["--run", "--budget", "1", "--diagnostic-past-c7"],
            vec![
                "--run",
                "--budget",
                "1",
                "--set-cpl0",
                "--diagnostic-past-c7",
                "--diagnostic-past-c7",
            ],
        ] {
            assert!(parse(&base(&bad)).is_err(), "{bad:?}");
        }
    }

    // ---- S4-38L9a: --cpuid-profile ----

    fn parse_base(extra: &[&str]) -> Result<Options, String> {
        let mut args: Vec<String> = [
            "--vmlinuz",
            "k",
            "--initramfs",
            "i",
            "--acpi-blob",
            "a",
            "--nonce",
            "n",
            "--expected-vmlinuz-sha256",
            &"0".repeat(64),
            "--expected-acpi-manifest-hash",
            &"0".repeat(64),
            "--expected-initramfs-sha256",
            &"0".repeat(64),
            "--ram-mib",
            "128",
            "--out",
            "o",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        args.extend(extra.iter().map(|s| s.to_string()));
        parse(&args)
    }

    #[test]
    fn linux_direct_seed_and_vcpu_arguments() {
        let profile = [
            "--run",
            "--budget",
            "512",
            "--set-cpl0",
            "--cpuid-profile",
            "stage4a",
            "--user-mode",
        ];
        assert!(parse_base(&profile).unwrap().interleaved_seed.is_none());
        for (text, seed) in [("0", 0), ("18446744073709551615", u64::MAX)] {
            let options =
                parse_base(&[&profile[..], &["--seed", text, "--vcpus", "1"]].concat()).unwrap();
            assert_eq!(options.interleaved_seed, Some(seed));
        }
        for tail in [
            &["--seed", "1", "--seed", "2"][..],
            &["--seed", "18446744073709551616"][..],
            &["--seed", "+1"][..],
            &["--vcpus", "0"][..],
            &["--vcpus", "2"][..],
            &["--vcpus", "1", "--vcpus", "1"][..],
        ] {
            assert!(
                parse_base(&[&profile[..], tail].concat()).is_err(),
                "{tail:?}"
            );
        }
        assert!(parse_base(&["--seed", "1"]).is_err());
        let mut apic_profile = profile;
        apic_profile[5] = "stage4a-apic";
        let apic_options = parse_base(&[&apic_profile[..], &["--seed", "1"]].concat()).unwrap();
        assert_eq!(apic_options.cpuid_profile, CpuidProfile::X8664V1Stage4AApic);
        assert_eq!(apic_options.interleaved_seed, Some(1));
        let smp = [&apic_profile[..], &["--vcpus", "2", "--seed", "24301"]].concat();
        let smp_options = parse_base(&smp).unwrap();
        assert_eq!(smp_options.vcpu_count, 2);
        assert_eq!(smp_options.interleaved_seed, Some(24301));
        assert!(parse_base(&["--vcpus", "2"]).is_ok());
        assert!(parse_base(&["--vcpus", "4"]).is_err());
        assert!(parse_base(&["--vcpus", "2", "--rootfs-raw", "working.raw"]).is_err());
        for count in ["2", "4"] {
            let disk = [
                &apic_profile[..],
                &[
                    "--vcpus",
                    count,
                    "--seed",
                    "24301",
                    "--rootfs-raw",
                    "working.raw",
                ],
            ]
            .concat();
            let options = parse_base(&disk).unwrap();
            assert_eq!(options.rootfs_raw.as_deref(), Some("working.raw"));
            assert_eq!(options.vcpu_count.to_string(), count);
        }
        assert!(parse_base(&[&smp[..], &["--stack-context-out", "trace.json"]].concat()).is_err());
        let mut wrong_profile = profile;
        wrong_profile[5] = "diagnostic";
        assert!(parse_base(&[&wrong_profile[..], &["--seed", "1"]].concat()).is_err());
        let mut zero_budget = profile;
        zero_budget[2] = "0";
        assert!(parse_base(&[&zero_budget[..], &["--seed", "1"]].concat()).is_err());
    }

    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    #[test]
    fn linux_direct_raw_run_reserves_a_distinct_report_and_uses_image_geometry() {
        use std::os::unix::fs::symlink;

        let expected_manifest = "a15686d043b7ce44650d6e847d1c965fdc5e5a38d8938a9c1eda120d5dfe81af";
        let blob =
            include_bytes!("../../../tests/fixtures/guest/stage6/smp-init/acpi-nvme-2-vcpus.bin");
        let case = Case::new("disk-smp", &bzimage_with_entry(&[0xf4]), &[], blob);
        let raw = case.dir.join("working.raw");
        let bytes = vec![0x35; 16 * 512];
        std::fs::write(&raw, &bytes).unwrap();
        let hardlink = case.dir.join("report-hardlink");
        let symlink_path = case.dir.join("report-symlink");
        std::fs::hard_link(&raw, &hardlink).unwrap();
        symlink(&raw, &symlink_path).unwrap();

        let args = |out: &str| {
            [
                "--vmlinuz",
                &case.options.vmlinuz,
                "--initramfs",
                &case.options.initramfs,
                "--acpi-blob",
                &case.options.acpi_blob,
                "--nonce",
                &case.options.nonce,
                "--expected-vmlinuz-sha256",
                &case.options.expected_vmlinuz_sha256,
                "--expected-initramfs-sha256",
                &case.options.expected_initramfs_sha256,
                "--expected-acpi-manifest-hash",
                expected_manifest,
                "--ram-mib",
                "128",
                "--out",
                out,
                "--rootfs-raw",
                raw.to_str().unwrap(),
                "--run",
                "--budget",
                "1",
                "--set-cpl0",
                "--cpuid-profile",
                "stage4a-apic",
                "--user-mode",
                "--vcpus",
                "2",
                "--seed",
                "24301",
            ]
            .map(String::from)
        };
        for alias in [&raw, &hardlink, &symlink_path] {
            assert_eq!(run(&args(alias.to_str().unwrap())), ExitCode::from(3));
            assert_eq!(std::fs::read(alias).unwrap(), bytes);
        }
        assert_eq!(run(&args(&case.options.out)), ExitCode::SUCCESS);
        let report: Value =
            serde_json::from_slice(&std::fs::read(&case.options.out).unwrap()).unwrap();
        assert_eq!(report["inputs"]["rootfs_raw"]["namespace_sectors"], 16);
        assert_eq!(report["guest_started"], true);
        assert_eq!(report["retired"], 1);
        assert_eq!(
            report["interleaved_scheduler"]["cpu_stats"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
        assert!(
            report["cmdline_in_memory"]
                .as_str()
                .unwrap()
                .ends_with(" rdinit=/s6-smp-init")
        );
        assert_eq!(std::fs::read(&raw).unwrap(), bytes);
    }

    #[test]
    fn linux_direct_seed_changes_quanta_and_repeats_the_stop_record() {
        let code = vec![0xFC; 1024]; // CLD
        let mut case = Case::new(
            "seeded-interleaved",
            &bzimage_with_entry(&code),
            &[0xAB; 4688],
            BLOB,
        );
        case.options.budget = Some(512);
        case.options.set_cpl0 = true;
        case.options.cpuid_profile = CpuidProfile::X8664V1Stage4A;
        case.options.user_mode = true;
        let env = DiagnosticEnv::default();
        let (plain, exit) = evaluate_with_env(&case.options, &env);
        assert_eq!(exit, 0, "{plain:#}");
        assert!(plain.get("interleaved_scheduler").is_none());
        case.options.interleaved_seed = Some(24301);
        let (seeded, exit) = evaluate_with_env(&case.options, &env);
        assert_eq!(exit, 0, "{seeded:#}");
        assert_eq!(
            seeded["interleaved_scheduler"],
            json!({"seed": 24301, "vcpus": 1})
        );
        assert_eq!(seeded["stop"]["exit_kind"], "budget_exhausted");
        assert_eq!(seeded["stop"]["retired"], 512);
        assert_eq!(seeded["stop"]["gpr"], plain["stop"]["gpr"]);
        assert_eq!(seeded["stop"]["rip"], plain["stop"]["rip"]);
        assert!(
            seeded["stop"]["quanta"].as_u64().unwrap() > plain["stop"]["quanta"].as_u64().unwrap()
        );
        let (repeat, exit) = evaluate_with_env(&case.options, &env);
        assert_eq!(exit, 0, "{repeat:#}");
        assert_eq!(seeded["stop"], repeat["stop"]);
    }

    #[test]
    fn cpuid_profile_flag_parses_and_defaults_to_diagnostic_v1() {
        let run = ["--run", "--budget", "1", "--set-cpl0"];
        let with = |tail: &[&str]| parse_base(&[&run[..], tail].concat());
        assert_eq!(with(&[]).unwrap().cpuid_profile, CpuidProfile::DiagnosticV1);
        assert_eq!(
            with(&["--cpuid-profile", "diagnostic"])
                .unwrap()
                .cpuid_profile,
            CpuidProfile::DiagnosticV1
        );
        assert_eq!(
            with(&["--cpuid-profile", "stage4a"]).unwrap().cpuid_profile,
            CpuidProfile::X8664V1Stage4A
        );
        assert!(with(&["--cpuid-profile", "stage4a", "--diagnostic-past-c7"]).is_ok());
        for bad in [
            vec!["--cpuid-profile", "full"],
            vec!["--cpuid-profile"],
            vec!["--cpuid-profile", "stage4a", "--cpuid-profile", "stage4a"],
        ] {
            assert!(with(&bad).is_err(), "{bad:?}");
        }
        // Needs --run and --set-cpl0.
        assert!(parse_base(&["--cpuid-profile", "stage4a"]).is_err());
        assert!(parse_base(&["--run", "--budget", "1", "--cpuid-profile", "stage4a"]).is_err());
    }

    #[test]
    fn stack_context_sidecar_is_single_cpl0_run_only() {
        let run = ["--run", "--budget", "1", "--set-cpl0"];
        let accepted =
            parse_base(&[&run[..], &["--stack-context-out", "context.json"]].concat()).unwrap();
        assert_eq!(accepted.stack_context_out.as_deref(), Some("context.json"));
        assert!(parse_base(&["--stack-context-out", "context.json"]).is_err());
        assert_eq!(
            parse_base(&[&run[..], &["--stack-context-out", "o"]].concat())
                .unwrap()
                .stack_context_out
                .as_deref(),
            Some("o")
        );
        assert!(
            parse_base(&[
                "--run",
                "--budget",
                "1",
                "--stack-context-out",
                "context.json"
            ])
            .is_err()
        );
    }

    #[test]
    fn execution_observations_with_checkpoint_table_is_refused() {
        let error = parse_base(&[
            "--run",
            "--budget",
            "1",
            "--set-cpl0",
            "--cpuid-profile",
            "stage4a",
            "--user-mode",
            "--execution-observations-out",
            "observations.json",
            "--checkpoint-table",
            "oracle.json",
        ])
        .unwrap_err();
        assert!(
            error.contains("cannot be combined with --checkpoint-table"),
            "{error}"
        );
    }

    #[test]
    fn execution_observations_sidecar_is_restricted_to_one_user_mode_run() {
        let run = [
            "--run",
            "--budget",
            "1",
            "--set-cpl0",
            "--cpuid-profile",
            "stage4a",
            "--user-mode",
        ];
        let accepted = parse_base(
            &[
                &run[..],
                &["--execution-observations-out", "observations.json"],
            ]
            .concat(),
        )
        .unwrap();
        assert_eq!(
            accepted.execution_observations_out.as_deref(),
            Some("observations.json")
        );
        let prefix = parse_base(
            &[
                &run[..],
                &[
                    "--execution-observations-out",
                    "observations.json",
                    "--execution-observations-rdtsc-prefix",
                ],
            ]
            .concat(),
        )
        .unwrap();
        assert!(prefix.execution_observations_rdtsc_prefix);
        let delay_summary = parse_base(
            &[
                &run[..],
                &[
                    "--execution-observations-out",
                    "observations.json",
                    "--execution-observations-delay-call-summary",
                ],
            ]
            .concat(),
        )
        .unwrap();
        assert!(delay_summary.execution_observations_delay_call_summary);
        assert_eq!(
            delay_summary.execution_observations_rdtsc_rip,
            Some(od_machine::LINUX_DELAY_TSC_POLL_RIP)
        );
        assert_eq!(
            delay_summary.execution_observations_rdtsc_entry_rip,
            Some(od_machine::LINUX_DELAY_TSC_ENTRY_RIP)
        );
        assert!(
            parse_base(&[&run[..], &["--execution-observations-delay-call-summary"]].concat())
                .is_err()
        );
        let elf_stack_copy = parse_base(
            &[
                &run[..],
                &[
                    "--execution-observations-out",
                    "observations.json",
                    "--execution-observations-elf-stack-copy",
                ],
            ]
            .concat(),
        )
        .unwrap();
        assert!(elf_stack_copy.execution_observations_elf_stack_copy);
        assert!(
            parse_base(&[&run[..], &["--execution-observations-elf-stack-copy"]].concat()).is_err()
        );
        let combined = parse_base(
            &[
                &run[..],
                &[
                    "--execution-observations-out",
                    "observations.json",
                    "--execution-observations-quantum-trace",
                    "--execution-observations-elf-stack-copy",
                ],
            ]
            .concat(),
        )
        .unwrap();
        assert!(combined.execution_observations_quantum_trace);
        assert!(combined.execution_observations_elf_stack_copy);
        let quantum_trace = parse_base(
            &[
                &run[..],
                &[
                    "--execution-observations-out",
                    "observations.json",
                    "--execution-observations-rdtsc-rip",
                    "0xffffffff8271b34d",
                    "--execution-observations-rdtsc-entry-rip",
                    "0xffffffff8271b31b",
                    "--execution-observations-quantum-trace",
                ],
            ]
            .concat(),
        )
        .unwrap();
        assert!(quantum_trace.execution_observations_quantum_trace);
        assert!(
            parse_base(
                &[
                    &run[..],
                    &[
                        "--execution-observations-out",
                        "observations.json",
                        "--execution-observations-quantum-trace",
                    ],
                ]
                .concat(),
            )
            .is_ok()
        );
        assert!(
            parse_base(&[&run[..], &["--execution-observations-quantum-trace"]].concat()).is_err()
        );
        assert!(
            parse_base(&[&run[..], &["--execution-observations-rdtsc-prefix"]].concat()).is_err()
        );
        let selected_rip = parse_base(
            &[
                &run[..],
                &[
                    "--execution-observations-out",
                    "observations.json",
                    "--execution-observations-rdtsc-rip",
                    "0xffffffff8271b34d",
                ],
            ]
            .concat(),
        )
        .unwrap();
        assert_eq!(
            selected_rip.execution_observations_rdtsc_rip,
            Some(0xffff_ffff_8271_b34d)
        );
        let selected_stride = parse_base(
            &[
                &run[..],
                &[
                    "--execution-observations-out",
                    "observations.json",
                    "--execution-observations-rdtsc-rip",
                    "0xffffffff8271b34d",
                    "--execution-observations-rdtsc-stride",
                    "512",
                ],
            ]
            .concat(),
        )
        .unwrap();
        assert_eq!(
            selected_stride.execution_observations_rdtsc_stride,
            Some(512)
        );
        let selected_entry = parse_base(
            &[
                &run[..],
                &[
                    "--execution-observations-out",
                    "observations.json",
                    "--execution-observations-rdtsc-rip",
                    "0xffffffff8271b34d",
                    "--execution-observations-rdtsc-entry-rip",
                    "0xffffffff8271b31b",
                ],
            ]
            .concat(),
        )
        .unwrap();
        assert_eq!(
            selected_entry.execution_observations_rdtsc_entry_rip,
            Some(0xffff_ffff_8271_b31b)
        );
        let selected_window = parse_base(
            &[
                &run[..],
                &[
                    "--execution-observations-out",
                    "observations.json",
                    "--execution-observations-rdtsc-rip",
                    "0xffffffff8271b34d",
                    "--execution-observations-rdtsc-window",
                    "1150000:1200000",
                ],
            ]
            .concat(),
        )
        .unwrap();
        assert_eq!(
            selected_window.execution_observations_rdtsc_window,
            Some(od_machine::RdtscSampleWindow {
                first_hit: 1_150_000,
                last_hit: 1_200_000,
            })
        );
        assert!(parse_rdtsc_sample_window("0:10").is_err());
        assert!(parse_rdtsc_sample_window("10:9").is_err());
        assert!(parse_rdtsc_sample_window("10").is_err());
        assert!(parse_rdtsc_sample_window("10:11:12").is_err());
        assert!(parse_rdtsc_sample_window("1:+10").is_err());
        assert!(parse_rdtsc_sample_window("1:18446744073709551616").is_err());
        let too_many = od_machine::RdtscSampleWindow {
            first_hit: 1,
            last_hit: od_machine::LINUX_RDTSC_SAMPLE_CAPACITY as u64 + 1,
        };
        assert!(validate_rdtsc_sample_window_capacity(too_many, 1).is_err());
        assert!(validate_rdtsc_sample_window_capacity(too_many, 2).is_ok());
        let mut entry_without_selected = run.to_vec();
        entry_without_selected.extend([
            "--execution-observations-out",
            "observations.json",
            "--execution-observations-rdtsc-entry-rip",
            "0x1234",
        ]);
        assert!(
            parse_base(&entry_without_selected)
                .unwrap_err()
                .contains("needs")
        );
        let mut zero_stride = run.to_vec();
        zero_stride.extend([
            "--execution-observations-out",
            "observations.json",
            "--execution-observations-rdtsc-rip",
            "0x1234",
            "--execution-observations-rdtsc-stride",
            "0",
        ]);
        assert!(
            parse_base(&zero_stride)
                .unwrap_err()
                .contains("positive integer")
        );
        let mut stride_without_rip = run.to_vec();
        stride_without_rip.extend([
            "--execution-observations-out",
            "observations.json",
            "--execution-observations-rdtsc-stride",
            "2",
        ]);
        assert!(
            parse_base(&stride_without_rip)
                .unwrap_err()
                .contains("needs")
        );
        let mut duplicate_stride = run.to_vec();
        duplicate_stride.extend([
            "--execution-observations-out",
            "observations.json",
            "--execution-observations-rdtsc-rip",
            "0x1234",
            "--execution-observations-rdtsc-stride",
            "2",
            "--execution-observations-rdtsc-stride",
            "4",
        ]);
        assert!(
            parse_base(&duplicate_stride)
                .unwrap_err()
                .contains("duplicate")
        );
        let selected = parse_base(
            &[
                &run[..],
                &[
                    "--execution-observations-out",
                    "observations.json",
                    "--execution-observations-post-iret-index",
                    "30",
                ],
            ]
            .concat(),
        )
        .unwrap();
        assert_eq!(selected.execution_observations_post_iret_index, Some(30));
        assert!(parse_base(&["--execution-observations-post-iret-index", "30"]).is_err());
        assert!(parse_base(&["--execution-observations-rdtsc-rip", "0x1234"]).is_err());
        assert!(
            parse_base(
                &[
                    &run[..],
                    &[
                        "--execution-observations-out",
                        "observations.json",
                        "--execution-observations-rdtsc-rip",
                        "1234"
                    ],
                ]
                .concat(),
            )
            .is_err()
        );
        assert!(
            parse_base(
                &[
                    &run[..],
                    &[
                        "--execution-observations-out",
                        "observations.json",
                        "--execution-observations-post-iret-index",
                        "-1",
                    ],
                ]
                .concat(),
            )
            .is_err()
        );
        assert!(
            parse_base(
                &[
                    &run[..],
                    &[
                        "--execution-observations-out",
                        "observations.json",
                        "--execution-observations-post-iret-index",
                        "30",
                        "--execution-observations-post-iret-index",
                        "31",
                    ],
                ]
                .concat(),
            )
            .is_err()
        );
        assert!(parse_base(&["--execution-observations-out", "observations.json"]).is_err());
        assert!(parse_base(&[&run[..6], &["--execution-observations-out", "o"]].concat()).is_err());
        assert!(parse_base(&[&run[..4], &["--execution-observations-out", "o"]].concat()).is_err());
        assert!(
            parse_base(
                &[
                    &run[..],
                    &[
                        "--execution-observations-out",
                        "o",
                        "--execution-observations-out",
                        "o2"
                    ]
                ]
                .concat()
            )
            .is_err()
        );
        assert!(
            parse_base(
                &[
                    &run[..],
                    &[
                        "--stack-context-out",
                        "same",
                        "--execution-observations-out",
                        "same"
                    ]
                ]
                .concat()
            )
            .is_err()
        );
    }

    #[test]
    fn sidecar_collision_guard_uses_filesystem_identity_for_aliases() {
        let root = std::env::temp_dir().join(format!(
            "od-runner-stack-context-paths-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        let sub = root.join("sub");
        let real = root.join("real");
        let alias = root.join("alias");
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::create_dir_all(&real).unwrap();
        let ordinary = root.join("out.json");
        let dotdot = sub.join("../out.json");
        let ordinary_file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&ordinary)
            .unwrap();
        assert!(
            file_matches_existing_path(
                &ordinary_file,
                ordinary.to_str().unwrap(),
                dotdot.to_str().unwrap()
            )
            .unwrap()
        );

        let case_alias = root.join("OUT.JSON");
        let same_case_alias = file_matches_existing_path(
            &ordinary_file,
            ordinary.to_str().unwrap(),
            case_alias.to_str().unwrap(),
        )
        .unwrap();
        if same_case_alias {
            assert!(std::fs::metadata(&case_alias).is_ok());
        } else {
            // On a case-sensitive volume this is a distinct future destination.
            let _case_file = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&case_alias)
                .unwrap();
            assert!(
                !file_matches_existing_path(
                    &ordinary_file,
                    ordinary.to_str().unwrap(),
                    case_alias.to_str().unwrap()
                )
                .unwrap()
            );
        }

        #[cfg(unix)]
        {
            let dangling = root.join("dangling.json");
            std::os::unix::fs::symlink(root.join("missing-target.json"), &dangling).unwrap();
            assert!(
                std::fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(&dangling)
                    .is_err()
            );

            std::os::unix::fs::symlink(&real, &alias).unwrap();
            let sidecar_path = real.join("sidecar.json");
            let sidecar_file = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&sidecar_path)
                .unwrap();
            assert!(
                file_matches_existing_path(
                    &sidecar_file,
                    sidecar_path.to_str().unwrap(),
                    alias.join("sidecar.json").to_str().unwrap()
                )
                .unwrap()
            );
        }
        drop(ordinary_file);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn cleanup_sidecar_reservation_removes_only_the_reserved_file() {
        let root =
            std::env::temp_dir().join(format!("od-runner-sidecar-cleanup-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();

        let owned_path = root.join("owned.json");
        let mut owned = Some(
            std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&owned_path)
                .unwrap(),
        );
        cleanup_sidecar_reservation(Some(owned_path.to_str().unwrap()), &mut owned).unwrap();
        assert!(owned.is_none());
        assert!(!owned_path.exists());

        let replaced_path = root.join("replaced.json");
        let mut replaced = Some(
            std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&replaced_path)
                .unwrap(),
        );
        std::fs::remove_file(&replaced_path).unwrap();
        std::fs::write(&replaced_path, b"replacement").unwrap();
        assert!(
            cleanup_sidecar_reservation(Some(replaced_path.to_str().unwrap()), &mut replaced)
                .is_err()
        );
        assert_eq!(std::fs::read(&replaced_path).unwrap(), b"replacement");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn stack_context_sidecar_serializes_as_separate_console_context_evidence() {
        let path = std::env::temp_dir().join(format!(
            "od-runner-stack-context-{}-{}.json",
            std::process::id(),
            1
        ));
        let _ = std::fs::remove_file(&path);
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .unwrap();
        write_stack_context_sidecar(&mut file, path.to_str().unwrap(), &[], None).unwrap();
        let value: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(
            value["capture_scope"],
            "console-output and bounded UART byte context"
        );
        assert_eq!(value["unwind_context"], false);
        assert_eq!(value["snapshots"].as_array().unwrap().len(), 0);
        assert!(value["uart_provenance"].is_null());
        assert!(
            std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)
                .is_err()
        );
        drop(file);
        std::fs::remove_file(path).unwrap();
    }

    // ---- S4-13b: the fetch cache switch and the opt-in sidecar ----

    #[test]
    fn od_no_fetch_cache_unset_empty_and_zero_keep_the_cache_anything_else_turns_it_off() {
        use std::ffi::OsStr;
        assert!(fetch_cache_enabled(None));
        assert!(fetch_cache_enabled(Some(OsStr::new(""))));
        assert!(fetch_cache_enabled(Some(OsStr::new("0"))));
        assert!(!fetch_cache_enabled(Some(OsStr::new("1"))));
        assert!(!fetch_cache_enabled(Some(OsStr::new("true"))));
        assert!(!fetch_cache_enabled(Some(OsStr::new("00"))));
    }

    #[test]
    fn fetch_cache_stats_path_is_the_opt_in() {
        use std::ffi::OsString;
        assert_eq!(fetch_cache_stats_path(None), Ok(None));
        assert_eq!(fetch_cache_stats_path(Some(OsString::new())), Ok(None));
        assert_eq!(
            fetch_cache_stats_path(Some(OsString::from("/tmp/stats.json"))),
            Ok(Some("/tmp/stats.json".to_string()))
        );
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStringExt;
            assert!(fetch_cache_stats_path(Some(OsString::from_vec(vec![0xff, 0xfe]))).is_err());
        }
    }

    #[test]
    fn fetch_cache_stats_sidecar_names_the_cache_state_the_rule_and_the_rates() {
        let cache = od_machine::FetchCacheStats {
            hits: 90,
            misses: 10,
            inserts: 4,
            write_flushes: 1,
            context_flushes: 2,
            host_flushes: 3,
            ..Default::default()
        };
        let phase = od_machine::LinuxFetchCachePhase {
            calls: 5,
            retired: 20,
            walks: od_machine::LinuxWalkCounts {
                fetch: 10,
                data_read: 40,
                data_write: 5,
            },
            cache,
        };
        let report = od_machine::LinuxFetchCacheReport {
            cache_enabled: false,
            leaf_2m_tag: false,
            phase_rule: od_machine::FETCH_CACHE_PHASE_RULE_C5,
            c5_reached: true,
            total: phase,
            before_c5: phase,
            after_c5: od_machine::LinuxFetchCachePhase::default(),
        };
        let value = fetch_cache_stats_json(&report, None, true, 1.5);
        // A run without the cache is told apart from the ordinary one.
        assert_eq!(value["fetch_cache_enabled"], false);
        assert_eq!(value["run_completed"], true);
        assert_eq!(value["phase_rule"], "c5_first_fetch_at_e_entry");
        assert_eq!(value["c5_reached"], true);
        let total = &value["total"];
        assert_eq!(total["calls"], 5);
        assert_eq!(total["retired"], 20);
        assert_eq!(total["walks"]["fetch"], 10);
        assert_eq!(total["walks"]["data_read"], 40);
        assert_eq!(total["walks"]["data_write"], 5);
        assert_eq!(total["walks_per_instruction"]["fetch"], 0.5);
        assert_eq!(total["walks_per_instruction"]["data_read"], 2.0);
        assert_eq!(total["cache"]["hits"], 90);
        assert_eq!(total["cache"]["misses"], 10);
        assert_eq!(total["cache"]["hit_rate"], 0.9);
        assert_eq!(total["cache"]["lookups_per_instruction"], 5.0);
        // An empty phase has no rates, not a division by zero.
        let after = &value["after_c5"];
        assert_eq!(after["retired"], 0);
        assert_eq!(after["walks_per_instruction"]["fetch"], Value::Null);
        assert_eq!(after["cache"]["hit_rate"], Value::Null);
    }

    fn run_case_with_env(name: &str, code: &[u8], budget: u64, env: &DiagnosticEnv) -> Value {
        let mut case = Case::new(name, &bzimage_with_entry(code), &[0xAB; 4688], BLOB);
        case.options.budget = Some(budget);
        // The report is built by the CPL0 set loop only.
        case.options.set_cpl0 = true;
        let (value, exit) = evaluate_with_env(&case.options, env);
        assert_eq!(exit, 0, "{value:#}");
        value
    }

    /// The opt-in adds the sidecar (and its entry in the evidence) and changes nothing else: the
    /// evidence of a run with `OD_FETCH_CACHE_STATS` equals the default one but for that entry,
    /// with the cache on and off. Without the variable there is no trace of the sidecar.
    #[test]
    fn fetch_cache_stats_opt_in_adds_only_its_sidecar() {
        // 20 CLI, then an HLT the default set does not run: the run stops there, after 20
        // instructions and enough bytes for the cache to hit.
        let mut code = vec![0xFA; 20];
        code.push(0xF4);
        for no_fetch_cache in [None, Some("1")] {
            let name = format!("fetch-stats-{}", no_fetch_cache.unwrap_or("on"));
            let plain_env = DiagnosticEnv {
                no_fetch_cache: no_fetch_cache.map(Into::into),
                fetch_cache_stats: None,
                fetch_cache_sweep: None,
                fetch_cache_tag: None,
            };
            let plain = run_case_with_env(&format!("{name}-plain"), &code, 100, &plain_env);
            assert!(
                plain.get("fetch_cache_stats_sidecar").is_none(),
                "{plain:#}"
            );
            assert_eq!(plain["stop"]["exit_kind"], "unsupported");
            assert_eq!(plain["retired"], 20);

            let dir = std::env::temp_dir().join(format!(
                "od-runner-fetch-stats-{}-{name}",
                std::process::id()
            ));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            let sidecar = dir.join("stats.json");
            let env = DiagnosticEnv {
                no_fetch_cache: plain_env.no_fetch_cache.clone(),
                fetch_cache_stats: Some(sidecar.clone().into_os_string()),
                fetch_cache_sweep: None,
                fetch_cache_tag: None,
            };
            let mut observed = run_case_with_env(&format!("{name}-stats"), &code, 100, &env);
            let entry = observed
                .as_object_mut()
                .unwrap()
                .remove("fetch_cache_stats_sidecar")
                .expect("the opt-in leaves its entry");
            assert_eq!(entry["write_error"], Value::Null, "{entry:#}");
            let differing: Vec<&String> = observed
                .as_object()
                .unwrap()
                .iter()
                // `inputs` holds the paths of the case's own input files.
                .filter(|(key, value)| {
                    key.as_str() != "inputs" && plain.get(key.as_str()) != Some(*value)
                })
                .map(|(key, _)| key)
                .collect();
            assert_eq!(differing, Vec::<&String>::new());
            assert_eq!(
                observed.as_object().unwrap().len(),
                plain.as_object().unwrap().len()
            );

            let written: Value = serde_json::from_slice(&std::fs::read(&sidecar).unwrap()).unwrap();
            assert_eq!(written["fetch_cache_enabled"], no_fetch_cache.is_none());
            assert_eq!(written["run_completed"], true);
            assert_eq!(written["phase_rule"], "none_checkpoints_disabled");
            assert_eq!(written["c5_reached"], false);
            assert_eq!(written["total"]["retired"], 20);
            assert_eq!(written["before_c5"]["retired"], 20);
            assert!(written["total"]["walks"]["fetch"].as_u64().unwrap() > 0);
            let hits = written["total"]["cache"]["hits"].as_u64().unwrap();
            assert_eq!(hits > 0, no_fetch_cache.is_none(), "{written:#}");
            let _ = std::fs::remove_dir_all(&dir);
        }
    }

    #[test]
    fn od_fetch_cache_tag_is_exactly_2m_or_the_4k_default() {
        use std::ffi::OsStr;
        assert!(!fetch_cache_leaf_2m_tag_enabled(None));
        for value in ["", "0", "1", "4k", "2M", "2m "] {
            assert!(
                !fetch_cache_leaf_2m_tag_enabled(Some(OsStr::new(value))),
                "{value:?}"
            );
        }
        assert!(fetch_cache_leaf_2m_tag_enabled(Some(OsStr::new("2m"))));
    }

    /// S4-13d: like the sweep, the tag needs the sidecar: `2m` without `OD_FETCH_CACHE_STATS` is
    /// not applied and warns; with the sidecar it is applied silently; anything but `2m` is the
    /// 4 KiB default without a warning.
    #[test]
    fn od_fetch_cache_tag_2m_without_the_sidecar_is_ignored_with_a_warning() {
        use std::ffi::OsStr;
        let two_m = Some(OsStr::new("2m"));
        let (apply, warning) = fetch_cache_leaf_2m_tag_decision(two_m, false);
        assert!(!apply);
        let warning = warning.expect("a warning");
        assert!(
            warning.contains("OD_FETCH_CACHE_TAG=2m")
                && warning.contains("OD_FETCH_CACHE_STATS")
                && warning.contains("4 KiB"),
            "{warning}"
        );
        assert_eq!(fetch_cache_leaf_2m_tag_decision(two_m, true), (true, None));
        for sidecar in [false, true] {
            for value in [None, Some(OsStr::new("")), Some(OsStr::new("4k"))] {
                assert_eq!(
                    fetch_cache_leaf_2m_tag_decision(value, sidecar),
                    (false, None)
                );
            }
        }
    }

    /// S4-13d: `OD_FETCH_CACHE_TAG=2m` gives the real cache the 2 MiB leaf tag and changes nothing
    /// in the evidence of the run (it equals the 4 KiB run but for `inputs` and the sidecar
    /// entry, whose path differs); the sidecar names the tag and carries the end state hash of
    /// guest RAM, equal for the 4 KiB and the 2 MiB tag and for a run without the cache.
    #[test]
    fn fetch_cache_tag_2m_changes_no_evidence_and_the_sidecar_names_it() {
        let mut code = vec![0xFA; 20];
        code.push(0xF4);
        let dir = std::env::temp_dir().join(format!("od-runner-fetch-tag-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut runs = Vec::new();
        for (name, no_cache, tag) in [
            ("tag4k", None, None),
            ("tag2m", None, Some("2m")),
            ("nocache", Some("1"), None),
        ] {
            let sidecar = dir.join(format!("{name}.json"));
            let env = DiagnosticEnv {
                no_fetch_cache: no_cache.map(Into::into),
                fetch_cache_stats: Some(sidecar.clone().into_os_string()),
                fetch_cache_sweep: None,
                fetch_cache_tag: tag.map(Into::into),
            };
            let mut evidence = run_case_with_env(name, &code, 100, &env);
            let object = evidence.as_object_mut().unwrap();
            object.remove("fetch_cache_stats_sidecar");
            object.remove("inputs");
            let written: Value = serde_json::from_slice(&std::fs::read(&sidecar).unwrap()).unwrap();
            runs.push((evidence, written));
        }
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(runs[0].1["fetch_cache_tag"], "4k");
        assert_eq!(runs[1].1["fetch_cache_tag"], "2m");
        assert_eq!(runs[2].1["fetch_cache_tag"], "4k");
        for run in &runs[1..] {
            assert_eq!(run.0, runs[0].0);
            assert_eq!(run.1["final_ram_sha256"], runs[0].1["final_ram_sha256"]);
        }
        let hash = runs[0].1["final_ram_sha256"].as_str().unwrap();
        assert_eq!(hash.len(), 64, "{hash}");
    }

    #[test]
    fn od_fetch_cache_sweep_is_an_opt_in_flag() {
        use std::ffi::OsStr;
        assert!(!fetch_cache_sweep_enabled(None));
        assert!(!fetch_cache_sweep_enabled(Some(OsStr::new(""))));
        assert!(!fetch_cache_sweep_enabled(Some(OsStr::new("0"))));
        assert!(fetch_cache_sweep_enabled(Some(OsStr::new("1"))));
    }

    /// S4-13c: with `OD_FETCH_CACHE_SWEEP` the stats sidecar gains one key, the sweep, whose
    /// reference shadow equals the real cache; the evidence of the run is the one of the run with
    /// the stats sidecar alone. Without the flag the sidecar has no such key, and the flag alone
    /// (no sidecar) does nothing.
    #[test]
    fn fetch_cache_sweep_adds_only_its_key_to_the_stats_sidecar() {
        let mut code = vec![0xFA; 20];
        code.push(0xF4);
        let dir =
            std::env::temp_dir().join(format!("od-runner-fetch-sweep-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let run = |name: &str, stats: bool, sweep: bool| {
            let sidecar = dir.join(format!("{name}.json"));
            let env = DiagnosticEnv {
                no_fetch_cache: None,
                fetch_cache_stats: stats.then(|| sidecar.clone().into_os_string()),
                fetch_cache_sweep: sweep.then(|| "1".into()),
                fetch_cache_tag: None,
            };
            let mut evidence = run_case_with_env(name, &code, 100, &env);
            if let Some(object) = evidence.as_object_mut() {
                object.remove("fetch_cache_stats_sidecar");
                object.remove("inputs");
            }
            let written = std::fs::read(&sidecar)
                .ok()
                .map(|bytes| serde_json::from_slice::<Value>(&bytes).unwrap());
            (evidence, written)
        };
        let (plain, none) = run("plain", false, false);
        assert!(none.is_none());
        let (flag_only, none) = run("flag-only", false, true);
        assert_eq!(flag_only, plain);
        assert!(none.is_none(), "the flag alone writes nothing");
        let (stats_only, stats_only_written) = run("stats", true, false);
        assert_eq!(stats_only, plain);
        let stats_only_written = stats_only_written.unwrap();
        assert!(stats_only_written.get("fetch_cache_sweep").is_none());
        let (swept, written) = run("sweep", true, true);
        assert_eq!(swept, plain, "the sweep changes nothing in the evidence");
        let mut written = written.unwrap();
        let sweep = written
            .as_object_mut()
            .unwrap()
            .remove("fetch_cache_sweep")
            .expect("the flag adds the key");
        // The rest of the sidecar is the one without the flag (the wall clock differs).
        let mut expected = stats_only_written;
        for value in [&mut written, &mut expected] {
            value.as_object_mut().unwrap().remove("run_wall_seconds");
        }
        assert_eq!(written, expected);
        assert_eq!(sweep["configs"].as_object().unwrap().len(), 112);
        let check = &sweep["self_check"];
        assert_eq!(check["reference"], "cap4-fifo-4k-per_call-write_watch");
        assert_eq!(check["total_equals_real"], true, "{sweep:#}");
        assert_eq!(check["phase1_equals_real"], true);
        assert_eq!(check["phase2_equals_real"], true);
        let reference = &sweep["configs"]["cap4-fifo-4k-per_call-write_watch"];
        assert_eq!(
            reference["total"]["hits"],
            written["total"]["cache"]["hits"]
        );
        assert!(reference["total"]["hits"].as_u64().unwrap() > 0);
        assert_eq!(
            sweep["configs"]["cap64-lru-2m-persistent-guest_only"]["capacity"],
            64
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Without `--set-cpl0` there is no report: the evidence says so and no empty sidecar stays.
    #[test]
    fn fetch_cache_stats_without_the_set_loop_leaves_no_empty_sidecar() {
        let dir =
            std::env::temp_dir().join(format!("od-runner-fetch-stats-base-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let sidecar = dir.join("stats.json");
        let mut case = Case::new(
            "fetch-stats-base",
            &bzimage_with_entry(&[0xF4]),
            &[0xAB; 4688],
            BLOB,
        );
        case.options.budget = Some(10);
        let env = DiagnosticEnv {
            no_fetch_cache: None,
            fetch_cache_stats: Some(sidecar.clone().into_os_string()),
            fetch_cache_sweep: None,
            fetch_cache_tag: None,
        };
        let (value, exit) = evaluate_with_env(&case.options, &env);
        assert_eq!(exit, 0, "{value:#}");
        let error = value["fetch_cache_stats_sidecar"]["write_error"]
            .as_str()
            .expect("the evidence says why there is no sidecar");
        assert!(error.contains("--set-cpl0"), "{error}");
        assert!(!sidecar.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// An existing file at the sidecar path is refused before the guest starts, and never
    /// overwritten.
    #[test]
    fn fetch_cache_stats_sidecar_refuses_an_existing_file() {
        let dir = std::env::temp_dir().join(format!(
            "od-runner-fetch-stats-existing-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let sidecar = dir.join("stats.json");
        std::fs::write(&sidecar, b"keep").unwrap();
        let mut case = Case::new(
            "fetch-stats-existing",
            &bzimage_with_entry(&[0xF4]),
            &[0xAB; 4688],
            BLOB,
        );
        case.options.budget = Some(10);
        let env = DiagnosticEnv {
            no_fetch_cache: None,
            fetch_cache_stats: Some(sidecar.clone().into_os_string()),
            fetch_cache_sweep: None,
            fetch_cache_tag: None,
        };
        let (value, exit) = evaluate_with_env(&case.options, &env);
        assert_eq!(exit, 4, "{value:#}");
        assert_eq!(value["guest_started"], false);
        assert_eq!(std::fs::read(&sidecar).unwrap(), b"keep");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn execution_observations_sidecar_serializes_bounded_counts_and_pf_metadata() {
        let path = std::env::temp_dir().join(format!(
            "od-runner-execution-observations-{}.json",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&path);
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .unwrap();
        let observations = LinuxExecutionObservations {
            rdtsc_cpl0_sites: vec![od_machine::RdtscSiteCount {
                rip: 0xffff_ffff_8100_1234,
                reads: 19,
            }],
            rdtsc_sample_rip: Some(0xffff_ffff_8271_b34d),
            rdtsc_sample_stride: 512,
            rdtsc_sample_window: Some(od_machine::RdtscSampleWindow {
                first_hit: 1024,
                last_hit: 1536,
            }),
            rdtsc_sample_site_hits: 19_000_000,
            rdtsc_site_samples: vec![od_machine::RdtscSiteSample {
                rip: 0xffff_ffff_8271_b34d,
                hit_ordinal: 1024,
                requested_cycles: 0x1234,
                start_tsc: 0x1000,
                observed_tsc: 0x1235,
                elapsed_cycles: 0x235,
                rbp: 0x3000,
                rsp: 0x2000,
                cr3: Some(0x4000),
            }],
            rdtsc_site_samples_omitted: 1,
            rdtsc_entry_sample_rip: Some(0xffff_ffff_8271_b31b),
            rdtsc_entry_sample_hits: 12,
            rdtsc_entry_samples: vec![od_machine::RdtscEntrySample {
                rip: 0xffff_ffff_8271_b31b,
                hit_ordinal: 12,
                requested_cycles: 0x1234,
                observed_tsc: 0x1000,
                rbp: 0x3000,
                rsp: 0x2000,
                cr3: Some(0x4000),
            }],
            rdtsc_entry_samples_omitted: 2,
            delay_tsc_call_summary: None,
            rdtsc_entry_clock_trace_samples: vec![
                od_machine::RdtscClockTraceSample {
                    rip: 0xffff_ffff_8271_b31b,
                    hit_ordinal: 1,
                    initial_now_ns: 10,
                    now_ns: 30,
                    callback_tsc: 0x1000,
                    tsc_hz: 2_400_000_000,
                    retired_instructions_charged: 5,
                    retired_ns_charged: Some(5),
                    quantum_instructions_executed: 3,
                    retired_instructions_through_rdtsc: 8,
                    pm_timer_ns_charged: 2,
                    pit_ns_charged: 3,
                    rdtsc_ns_charged: 4,
                    idle_ns_advanced: 6,
                    quantum_ordinal: 12,
                    irqs_delivered: 7,
                    unaccounted_elapsed_ns: Some(0),
                },
                od_machine::RdtscClockTraceSample {
                    rip: 0xffff_ffff_8271_b31b,
                    hit_ordinal: 2,
                    initial_now_ns: 10,
                    now_ns: 34,
                    callback_tsc: 0x1001,
                    tsc_hz: 2_400_000_000,
                    retired_instructions_charged: 8,
                    retired_ns_charged: Some(8),
                    quantum_instructions_executed: 4,
                    retired_instructions_through_rdtsc: 12,
                    pm_timer_ns_charged: 3,
                    pit_ns_charged: 3,
                    rdtsc_ns_charged: 4,
                    idle_ns_advanced: 6,
                    quantum_ordinal: 18,
                    irqs_delivered: 9,
                    unaccounted_elapsed_ns: Some(0),
                },
            ],
            rdtsc_entry_clock_trace_missing: 0,
            rdtsc_prefix_sentinel_rip: Some(od_machine::LINUX_RDTSC_PREFIX_SENTINEL_RIP),
            rdtsc_prefix_samples: vec![od_machine::RdtscPrefixSample {
                ordinal: 1,
                rip: od_machine::LINUX_RDTSC_PREFIX_SENTINEL_RIP,
                cpl: Some(0),
                cr3: Some(0x4000),
                rsp: 0x5000,
                rbp: 0x6000,
                clock: od_machine::RdtscClockTraceSample {
                    rip: od_machine::LINUX_RDTSC_PREFIX_SENTINEL_RIP,
                    hit_ordinal: 1,
                    initial_now_ns: 0,
                    now_ns: 20,
                    callback_tsc: 0x30,
                    tsc_hz: 2_400_000_000,
                    retired_instructions_charged: 10,
                    retired_ns_charged: Some(10),
                    quantum_instructions_executed: 3,
                    retired_instructions_through_rdtsc: 13,
                    pm_timer_ns_charged: 2,
                    pit_ns_charged: 3,
                    rdtsc_ns_charged: 1,
                    idle_ns_advanced: 4,
                    quantum_ordinal: 12,
                    irqs_delivered: 1,
                    unaccounted_elapsed_ns: Some(0),
                },
                page_faults_delivered_through: 4,
                general_protection_faults_delivered_through: 2,
            }],
            rdtsc_prefix_event_count: 1,
            rdtsc_prefix_missing: 0,
            rdtsc_prefix_status: od_machine::RdtscPrefixStatus::SentinelSeen,
            instruction_path_trace: None,
            rdtsc_cpl0_unattributed_reads: 2,
            rdtsc_cpl0_site_overflow_reads: 3,
            page_faults_delivered: 5,
            page_fault_events: vec![od_machine::DeliveredPageFaultObservation {
                rip: 0xffff_ffff_8100_5678,
                cr2: 0x0000_0000_0040_2000,
                error_code: 7,
                access: "Write".into(),
                cpl: 3,
                faulting_context: od_machine::FaultingPageFaultContext {
                    cr3: 0x1000,
                    rsp: 0x2000,
                    rbp: 0x3000,
                    rcx: 0x4000,
                    rsi: 0x5000,
                    rdi: 0x6000,
                    cs_selector: Some(0x23),
                    frame_chain: vec![od_machine::ConsoleContextFrame {
                        frame_pointer: 0x7000,
                        return_address: 0xffff_ffff_8100_7000,
                    }],
                    frame_chain_end: od_machine::FrameChainEnd::NullFramePointer,
                    page_walk: Some(od_machine::FaultingPageWalkContext {
                        entries: vec![od_machine::WalkEntry {
                            level: od_machine::PageTableLevel::Pd,
                            index: 4,
                            entry_address: od_machine::GuestPhysicalAddress::new(0x3020),
                            entry_value: 0x4007,
                        }],
                        reason: od_machine::PageFaultReason::NotPresent {
                            level: od_machine::PageTableLevel::Pt,
                        },
                    }),
                },
            }],
            page_fault_events_omitted: 4,
            post_iret_selected_ordinal: Some(3),
            post_iret_page_walk_status: Some("captured"),
            post_iret_page_walk: Some(od_machine::PostIretPageWalkSnapshot {
                event_ordinal: 3,
                fault_rip: 0xffff_ffff_8100_5678,
                fault_cr2: 0x0000_0000_0040_2000,
                fault_access: "Write".into(),
                fault_cpl: 3,
                post_iret_rip: 0xffff_ffff_8100_5678,
                post_iret_cs: Some(0x23),
                post_iret_cpl: 3,
                post_iret_rsp: 0x2000,
                post_iret_cr3: 0x1000,
                entries: vec![od_machine::WalkEntry {
                    level: od_machine::PageTableLevel::Pd,
                    index: 4,
                    entry_address: od_machine::GuestPhysicalAddress::new(0x3020),
                    entry_value: 0x4007,
                }],
                translation_status: "translated",
                translated_physical: Some(0x4_2000),
                translation_error: None,
            }),
            elf_stack_copy: None,
        };
        let (byte_count, sha256) =
            write_execution_observations_sidecar(&mut file, path.to_str().unwrap(), &observations)
                .unwrap();
        let metadata = execution_observations_sidecar_metadata(
            path.to_str().unwrap(),
            &observations,
            byte_count,
            sha256.clone(),
        );
        let written = std::fs::read(&path).unwrap();
        assert_eq!(byte_count, written.len());
        assert_eq!(sha256, od_testkit::sha256_hex(&written));
        assert_eq!(metadata["selected_rip"], "0xffffffff8271b34d");
        assert_eq!(metadata["stride"], 512);
        assert_eq!(metadata["selected_site_hits"], 19_000_000);
        assert_eq!(metadata["sample_count"], 1);
        assert_eq!(metadata["samples_omitted"], 1);
        assert_eq!(metadata["sample_window"]["first_hit"], 1024);
        assert_eq!(metadata["sample_window"]["last_hit"], 1536);
        assert_eq!(metadata["entry_clock_trace_sample_count"], 2);
        assert_eq!(metadata["entry_clock_trace_missing"], 0);
        assert_eq!(metadata["rdtsc_prefix_status"], "sentinel_seen");
        assert_eq!(metadata["rdtsc_prefix_sample_count"], 1);
        let value: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(value["schema_version"], 21);
        assert!(value["elf_stack_copy"].is_null());
        assert_eq!(value["rdtsc_prefix_status"], "sentinel_seen");
        assert_eq!(value["rdtsc_prefix_event_count"], 1);
        assert!(value["instruction_path_trace"].is_null());
        assert_eq!(
            value["limits"]["instruction_path_trace_records"],
            od_machine::LINUX_INSTRUCTION_PATH_TRACE_CAPACITY
        );
        assert_eq!(
            value["rdtsc_prefix_samples"][0]["clock"]["rip"],
            "0xffffffff8271b31b"
        );
        assert_eq!(value["post_iret_selected_ordinal"], 3);
        assert_eq!(value["post_iret_page_walk_status"], "captured");
        assert_eq!(
            value["post_iret_page_walk"]["translation_status"],
            "translated"
        );
        assert_eq!(
            value["post_iret_page_walk"]["entries"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            value["limits"]["rdtsc_cpl0_sites"],
            od_machine::LINUX_RDTSC_SITE_CAPACITY
        );
        assert_eq!(
            value["limits"]["page_fault_events"],
            od_machine::LINUX_PAGE_FAULT_EVENT_CAPACITY
        );
        assert_eq!(
            value["limits"]["frame_links_per_page_fault"],
            od_machine::LINUX_PAGE_FAULT_FRAME_LINK_CAPACITY
        );
        assert_eq!(
            value["limits"]["page_walk_entries_per_fault"],
            od_machine::LINUX_PAGE_FAULT_WALK_ENTRY_CAPACITY
        );
        assert_eq!(value["rdtsc_cpl0_sites"][0]["rip"], "0xffffffff81001234");
        assert_eq!(value["rdtsc_cpl0_sites"][0]["reads"], 19);
        assert_eq!(value["rdtsc_cpl0_site_overflow_reads"], 3);
        assert_eq!(
            value["limits"]["rdtsc_site_samples"],
            od_machine::LINUX_RDTSC_SAMPLE_CAPACITY
        );
        assert_eq!(value["rdtsc_sample_rip"], "0xffffffff8271b34d");
        assert_eq!(value["rdtsc_sample_stride"], 512);
        assert_eq!(value["rdtsc_sample_window"]["first_hit"], 1024);
        assert_eq!(value["rdtsc_sample_window"]["last_hit"], 1536);
        assert_eq!(value["rdtsc_sample_site_hits"], 19_000_000);
        assert_eq!(value["rdtsc_site_samples"][0]["hit_ordinal"], 1024);
        assert_eq!(value["rdtsc_site_samples"][0]["requested_cycles"], "0x1234");
        assert_eq!(value["rdtsc_site_samples"][0]["start_tsc"], "0x1000");
        assert_eq!(value["rdtsc_site_samples"][0]["observed_tsc"], "0x1235");
        assert_eq!(value["rdtsc_site_samples"][0]["elapsed_cycles"], "0x235");
        assert_eq!(value["rdtsc_site_samples"][0]["cr3"], "0x4000");
        assert_eq!(value["rdtsc_site_samples_omitted"], 1);
        assert_eq!(value["rdtsc_entry_sample_rip"], "0xffffffff8271b31b");
        assert_eq!(value["rdtsc_entry_sample_hits"], 12);
        assert_eq!(value["rdtsc_entry_samples"][0]["hit_ordinal"], 12);
        assert_eq!(
            value["rdtsc_entry_samples"][0]["requested_cycles"],
            "0x1234"
        );
        assert_eq!(value["rdtsc_entry_samples"][0]["observed_tsc"], "0x1000");
        assert_eq!(value["rdtsc_entry_samples_omitted"], 2);
        assert_eq!(
            value["rdtsc_entry_clock_trace_samples"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
        assert_eq!(value["rdtsc_entry_clock_trace_missing"], 0);
        assert_eq!(
            value["rdtsc_entry_clock_trace_samples"][0]["hit_ordinal"],
            1
        );
        assert_eq!(
            value["rdtsc_entry_clock_trace_samples"][1]["callback_tsc"],
            "0x1001"
        );
        assert_eq!(
            value["rdtsc_entry_clock_trace_samples"][0]["quantum_instructions_executed"],
            3
        );
        assert_eq!(
            value["rdtsc_entry_clock_trace_samples"][0]["retired_instructions_through_rdtsc"],
            8
        );
        assert_eq!(value["page_faults_delivered"], 5);
        assert_eq!(value["page_fault_events"][0]["rip"], "0xffffffff81005678");
        assert_eq!(value["page_fault_events"][0]["cr2"], "0x402000");
        assert_eq!(value["page_fault_events"][0]["error_code"], "0x00000007");
        assert_eq!(value["page_fault_events"][0]["access"], "Write");
        assert_eq!(value["page_fault_events"][0]["cpl"], 3);
        assert_eq!(
            value["page_fault_events"][0]["faulting_context"]["cr3"],
            "0x1000"
        );
        assert_eq!(
            value["page_fault_events"][0]["faulting_context"]["rsp"],
            "0x2000"
        );
        assert_eq!(
            value["page_fault_events"][0]["faulting_context"]["rbp"],
            "0x3000"
        );
        assert_eq!(
            value["page_fault_events"][0]["faulting_context"]["rcx"],
            "0x4000"
        );
        assert_eq!(
            value["page_fault_events"][0]["faulting_context"]["rsi"],
            "0x5000"
        );
        assert_eq!(
            value["page_fault_events"][0]["faulting_context"]["rdi"],
            "0x6000"
        );
        assert_eq!(
            value["page_fault_events"][0]["faulting_context"]["frame_chain"][0]["frame_pointer"],
            "0x7000"
        );
        assert_eq!(
            value["page_fault_events"][0]["faulting_context"]["frame_chain"][0]["return_address"],
            "0xffffffff81007000"
        );
        assert_eq!(
            value["page_fault_events"][0]["faulting_context"]["frame_chain_end"],
            "NullFramePointer"
        );
        assert_eq!(
            value["page_fault_events"][0]["faulting_context"]["page_walk"]["source"],
            "original_faulting_guest_access"
        );
        assert_eq!(
            value["page_fault_events"][0]["faulting_context"]["page_walk"]["entry_values"],
            "raw_before_accessed_dirty_updates"
        );
        assert_eq!(
            value["page_fault_events"][0]["faulting_context"]["page_walk"]["reason"],
            "NotPresent { level: Pt }"
        );
        assert_eq!(
            value["page_fault_events"][0]["faulting_context"]["page_walk"]["entries"][0]["level"],
            "Pd"
        );
        assert_eq!(
            value["page_fault_events"][0]["faulting_context"]["page_walk"]["entries"][0]["entry_address"],
            "0x3020"
        );
        assert_eq!(
            value["page_fault_events"][0]["faulting_context"]["page_walk"]["entries"][0]["entry_value"],
            "0x4007"
        );
        assert_eq!(value["page_fault_events_omitted"], 4);
        drop(file);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn stack_context_sidecar_serializes_bounded_uart_byte_provenance() {
        let path = std::env::temp_dir().join(format!(
            "od-runner-uart-context-{}-{}.json",
            std::process::id(),
            1
        ));
        let _ = std::fs::remove_file(&path);
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .unwrap();
        let capture = od_machine::UartOutputContextDiagnostic {
            marker_offset: Some(10),
            marker_prefix_before_trigger_unobserved: true,
            end: od_machine::UartOutputContextEnd::RunEnded,
            samples: vec![od_machine::UartOutputByteContextSnapshot {
                offset: 18,
                byte: b'=',
                retired: 99,
                rip: 0x1234,
                rsp: 0x8000,
                rbp: 0x8100,
                cr3: 0x6000,
                cs_selector: Some(0x1b),
                frames: vec![od_machine::ConsoleContextFrame {
                    frame_pointer: 0x8100,
                    return_address: 0xffff_ffff_8100_1234,
                }],
                ended: od_machine::FrameChainEnd::NullFramePointer,
            }],
        };
        write_stack_context_sidecar(&mut file, path.to_str().unwrap(), &[], Some(&capture))
            .unwrap();
        let value: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(value["schema_version"], 2);
        let provenance = &value["uart_provenance"];
        assert_eq!(provenance["marker_offset"], 10);
        assert_eq!(provenance["marker_prefix_before_trigger_unobserved"], true);
        assert_eq!(provenance["end"], "run_end");
        assert_eq!(provenance["samples"][0]["offset"], 18);
        assert_eq!(
            provenance["samples"][0]["rip_phase"],
            "post_out_instruction_boundary"
        );
        assert_eq!(provenance["samples"][0]["byte"], b'=' as u64);
        assert_eq!(provenance["samples"][0]["cpl"], 3);
        assert_eq!(
            provenance["samples"][0]["frames"].as_array().unwrap().len(),
            1
        );
        drop(file);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn stage4a_runs_leaf_1_without_the_c7_stop_and_marks_the_result_counted() {
        // CLI; MOV EAX, 1; CPUID; HLT, no --diagnostic-past-c7.
        let code = [0xFA, 0xB8, 0x01, 0x00, 0x00, 0x00, 0x0F, 0xA2, 0xF4];
        let mut case = Case::new(
            "stage4a-leaf1",
            &bzimage_with_entry(&code),
            &[0xAB; 4688],
            BLOB,
        );
        case.options.budget = Some(64);
        case.options.set_cpl0 = true;
        case.options.cpuid_profile = CpuidProfile::X8664V1Stage4A;
        let (value, exit) = evaluate(&case.options);
        assert_eq!(exit, 0, "{value:#}");
        let stop = &value["stop"];
        assert_ne!(stop["exit_kind"], "cpuid_divergence", "{value:#}");
        assert_eq!(stop["cpuid_divergence_leaf"], Value::Null);
        // Oracle from the contract table (SDM bit positions): EDX has FPU (bit 0), TSC (bit 4)
        // and FXSR (bit 24) since S4-38L9d, and not SSE/SSE2.
        assert_eq!(stop["gpr"]["rax"], "0x600");
        assert_eq!(stop["gpr"]["rbx"], "0x100");
        assert_eq!(stop["gpr"]["rcx"], "0x0");
        assert_eq!(stop["gpr"]["rdx"], "0x1000011");
        assert_eq!(
            stop["retired"], 3,
            "CLI, MOV, CPUID; HLT is outside the set"
        );
        // S4-39j: stage4a without --diagnostic-past-c7 is counted; the flag follows the request.
        assert_eq!(value["acceptance"], "counted", "{value:#}");
        assert_eq!(value["cpuid_profile"], "X8664V1Stage4A");
        assert_eq!(value["diagnostic_past_c7"], false);
        assert_eq!(stop["diagnostic_past_c7"], false);
        assert_eq!(
            stop["set_members"],
            json!([
                "cli",
                "cpu_owned_cr3",
                "cpuid:X8664V1Stage4A",
                "rdtsc",
                "msr:EferReadOnlyV1(3328)",
                "msr:fs_gs_base",
                "msr:star_write",
                "msr:lstar_write",
                "msr:cstar_write",
                "msr:sysenter_cs_write",
                "msr:sysenter_stack_write",
                "msr:syscall_mask_write",
                "msr:kernel_gs_base_write",
                "msr:tdx_keyid_partitioning_read",
                "fp_state",
                "int3",
                "pio_u8",
                "sti",
                "popf_cpl0",
                "pci_config",
                "irq"
            ])
        );
    }

    /// S4-38L8d: `--cpuid-profile stage4a-apic` is a profile of its own. S4-38L9-um: the user mode
    /// is accepted for it (and for `stage4a`), and still refused for the diagnostic profile.
    #[test]
    fn stage4a_apic_is_accepted_and_has_the_user_mode() {
        let with = |extra: &[&str]| {
            let mut args = vec!["--run", "--budget", "1", "--set-cpl0"];
            args.extend_from_slice(extra);
            parse_base(&args)
        };
        assert_eq!(
            with(&["--cpuid-profile", "stage4a-apic"])
                .unwrap()
                .cpuid_profile,
            CpuidProfile::X8664V1Stage4AApic
        );
        assert!(with(&["--cpuid-profile", "stage4a-apic", "--diagnostic-past-c7"]).is_ok());
        let user = with(&["--cpuid-profile", "stage4a-apic", "--user-mode"]).unwrap();
        assert!(user.user_mode);
        assert_eq!(user.cpuid_profile, CpuidProfile::X8664V1Stage4AApic);
        assert!(
            !with(&["--cpuid-profile", "stage4a-apic"])
                .unwrap()
                .user_mode
        );
        // Only the stage4a family has it: the diagnostic profile and the default are refused.
        for bad in [
            vec!["--cpuid-profile", "diagnostic", "--user-mode"],
            vec!["--user-mode"],
            vec![
                "--cpuid-profile",
                "stage4a-apic",
                "--user-mode",
                "--user-mode",
            ],
        ] {
            let error = with(&bad).unwrap_err();
            assert!(
                error.contains("--user-mode needs") || error.contains("duplicate"),
                "{error}"
            );
        }
        let error = with(&["--cpuid-profile", "diagnostic", "--user-mode"]).unwrap_err();
        assert!(error.contains("stage4a or stage4a-apic"), "{error}");
        // Needs --run and --set-cpl0 like the other profiles.
        assert!(parse_base(&["--cpuid-profile", "stage4a-apic"]).is_err());
    }

    #[test]
    fn stage4a_apic_evidence_names_the_profile_declares_apic_and_turns_on_irq_and_clock_owner() {
        // CLI; MOV EAX, 1; CPUID; HLT.
        let code = [0xFA, 0xB8, 0x01, 0x00, 0x00, 0x00, 0x0F, 0xA2, 0xF4];
        let run = |profile: CpuidProfile| {
            let mut case = Case::new(
                "stage4a-apic",
                &bzimage_with_entry(&code),
                &[0xAB; 4688],
                BLOB,
            );
            case.options.budget = Some(64);
            case.options.set_cpl0 = true;
            case.options.cpuid_profile = profile;
            let (value, exit) = evaluate(&case.options);
            assert_eq!(exit, 0, "{value:#}");
            value
        };
        let value = run(CpuidProfile::X8664V1Stage4AApic);
        let stop = &value["stop"];
        assert_eq!(value["cpuid_profile"], "X8664V1Stage4AApic");
        // Oracle by hand: FPU (bit 0) | TSC (bit 4) | APIC (bit 9) | FXSR (bit 24).
        assert_eq!(stop["gpr"]["rdx"], "0x1000211", "{value:#}");
        assert_eq!(stop["gpr"]["rcx"], "0x0");
        assert_eq!(stop["exit_kind"], "halted", "{value:#}");
        let members = stop["set_members"].as_array().unwrap();
        let has = |name: &str| members.iter().any(|member| member == name);
        for name in [
            "cpuid:X8664V1Stage4AApic",
            "halt",
            "irq",
            "clock_owner",
            "fp_state",
            "int3",
        ] {
            assert!(has(name), "{name}: {members:?}");
        }
        assert!(!has("user_mode"));
        // Not the counted profile of S4-39j: the PASS stays with stage4a.
        assert_eq!(value["acceptance"], "diagnostic_not_counted");
        let note = value["diagnostic_note"].as_str().unwrap();
        assert!(note.contains("X8664V1Stage4AApic"), "{note}");
        assert!(!note.contains("stage4a is the counted profile"), "{note}");
        // The same program under stage4a: no clock owner, no halt (HLT is outside its set), and
        // the unchanged evidence lines.
        let base = run(CpuidProfile::X8664V1Stage4A);
        assert_eq!(base["cpuid_profile"], "X8664V1Stage4A");
        assert_eq!(base["stop"]["gpr"]["rdx"], "0x1000011");
        assert_eq!(base["acceptance"], "counted");
        let base_members = base["stop"]["set_members"].as_array().unwrap();
        assert!(
            !base_members
                .iter()
                .any(|m| m == "clock_owner" || m == "halt")
        );
    }

    #[test]
    fn stage4a_runs_fninit_and_fxsave_and_the_default_profile_does_not() {
        // CLI; MOV RAX,CR0; AND RAX,-13; MOV RDI,RAX; MOV CR0,RDI; MOV EDI,0x620; MOV CR4,RDI;
        // FNINIT; HLT: the `fpu__init_cpu_generic` sequence of the kernel (HLT is outside the set).
        let code = [
            0xFA, 0x0F, 0x20, 0xC0, 0x48, 0x83, 0xE0, 0xF3, 0x48, 0x89, 0xC7, 0x0F, 0x22, 0xC7,
            0xBF, 0x20, 0x06, 0x00, 0x00, 0x0F, 0x22, 0xE7, 0xDB, 0xE3, 0xF4,
        ];
        let run = |profile: CpuidProfile| {
            let mut case = Case::new(
                "stage4a-fp",
                &bzimage_with_entry(&code),
                &[0xAB; 4688],
                BLOB,
            );
            case.options.budget = Some(64);
            case.options.set_cpl0 = true;
            case.options.cpuid_profile = profile;
            let (value, exit) = evaluate(&case.options);
            (value, exit)
        };
        let (on, exit) = run(CpuidProfile::X8664V1Stage4A);
        assert_eq!(exit, 0, "{on:#}");
        let stop = &on["stop"];
        assert_eq!(stop["exit_kind"], "unsupported", "{on:#}");
        assert_eq!(stop["unsupported_opcode"], "0xf4", "{on:#}");
        assert_eq!(stop["retired"], 8, "CLI .. FNINIT retire; HLT is the stop");
        assert!(
            stop["set_members"]
                .as_array()
                .expect("members")
                .contains(&json!("fp_state")),
            "{on:#}"
        );
        // CR4 got OSFXSR and OSXMMEXCPT (bits 9 and 10).
        let cr4 = u64::from_str_radix(
            stop["cr4"].as_str().expect("cr4").trim_start_matches("0x"),
            16,
        )
        .expect("hex");
        assert_eq!(cr4 & 0x600, 0x600, "{on:#}");
        // DiagnosticV1: no FP member, so the run stops earlier, at `MOV CR0, RDI` (opcode 0x22), after
        // CLI, MOV RAX,CR0, AND and MOV RDI,RAX; it never reaches FNINIT or HLT.
        let (off, exit) = run(CpuidProfile::DiagnosticV1);
        assert_eq!(exit, 0, "{off:#}");
        let stop = &off["stop"];
        assert_eq!(stop["exit_kind"], "unsupported", "{off:#}");
        assert_eq!(stop["unsupported_opcode"], "0x22", "{off:#}");
        assert!(
            !stop["set_members"]
                .as_array()
                .expect("members")
                .contains(&json!("fp_state")),
            "{off:#}"
        );
    }

    #[test]
    fn stage4a_delivers_int3_through_the_guest_idt_and_the_default_profile_does_not() {
        // CLI; LIDT [RIP+0x1f8]; INT3; HLT. Handler at +0x100 is IRETQ; the pseudo-descriptor is
        // at +0x200 (limit 0xff, base entry+0x300); gate 3 (+0x300+48) is an interrupt gate with
        // DPL 3 (0xEE, what Linux installs), selector 0x10, to the handler.
        const ENTRY: u64 = 0x0100_0200;
        let mut code = vec![0u8; 0x400];
        code[..9].copy_from_slice(&[0xFA, 0x0F, 0x01, 0x1D, 0xF8, 0x01, 0x00, 0x00, 0xCC]);
        code[9] = 0xF4;
        code[0x100..0x102].copy_from_slice(&[0x48, 0xCF]);
        code[0x200..0x202].copy_from_slice(&0xFFu16.to_le_bytes());
        code[0x202..0x20A].copy_from_slice(&(ENTRY + 0x300).to_le_bytes());
        let handler = ENTRY + 0x100;
        let gate = 0x300 + 3 * 16;
        code[gate..gate + 2].copy_from_slice(&(handler as u16).to_le_bytes());
        code[gate + 2..gate + 4].copy_from_slice(&0x10u16.to_le_bytes());
        code[gate + 5] = 0xEE;
        code[gate + 6..gate + 8].copy_from_slice(&((handler >> 16) as u16).to_le_bytes());
        code[gate + 8..gate + 12].copy_from_slice(&((handler >> 32) as u32).to_le_bytes());
        let run = |profile: CpuidProfile| {
            let mut case = Case::new(
                "stage4a-int3",
                &bzimage_with_entry(&code),
                &[0xAB; 4688],
                BLOB,
            );
            case.options.budget = Some(64);
            case.options.set_cpl0 = true;
            case.options.cpuid_profile = profile;
            let (value, exit) = evaluate(&case.options);
            assert_eq!(exit, 0, "{value:#}");
            value
        };
        let on = run(CpuidProfile::X8664V1Stage4A);
        let stop = &on["stop"];
        // CLI, LIDT, INT3, IRETQ retire; HLT (outside the set) is the stop after the INT3.
        assert_eq!(stop["exit_kind"], "unsupported", "{on:#}");
        assert_eq!(stop["unsupported_opcode"], "0xf4", "{on:#}");
        assert_eq!(stop["retired"], 4, "{on:#}");
        assert!(
            stop["set_members"]
                .as_array()
                .expect("members")
                .contains(&json!("int3")),
            "{on:#}"
        );
        // DiagnosticV1: no INT3 member, the stop is the INT3 itself after CLI and LIDT.
        let off = run(CpuidProfile::DiagnosticV1);
        let stop = &off["stop"];
        assert_eq!(stop["exit_kind"], "unsupported", "{off:#}");
        assert_eq!(stop["unsupported_opcode"], "0xcc", "{off:#}");
        assert_eq!(stop["retired"], 2, "{off:#}");
        assert!(
            !stop["set_members"]
                .as_array()
                .expect("members")
                .contains(&json!("int3")),
            "{off:#}"
        );
    }

    #[test]
    fn stage4a_answers_rdmsr_0x87_with_zero_and_the_default_profile_does_not() {
        // CLI; MOV ECX, 0x87; MOV EAX, 0x55; MOV EDX, 0x66; RDMSR; HLT.
        let code = [
            0xFA, 0xB9, 0x87, 0x00, 0x00, 0x00, 0xB8, 0x55, 0x00, 0x00, 0x00, 0xBA, 0x66, 0x00,
            0x00, 0x00, 0x0F, 0x32, 0xF4,
        ];
        let run = |profile: CpuidProfile| {
            let mut case = Case::new(
                "stage4a-rdmsr-87",
                &bzimage_with_entry(&code),
                &[0xAB; 4688],
                BLOB,
            );
            case.options.budget = Some(64);
            case.options.set_cpl0 = true;
            case.options.cpuid_profile = profile;
            let (value, exit) = evaluate(&case.options);
            assert_eq!(exit, 0, "{value:#}");
            value
        };
        let on = run(CpuidProfile::X8664V1Stage4A);
        let stop = &on["stop"];
        // Oracle: the kernel's tdx_init reads 0 as "no TDX KeyIDs" (arch/x86/virt/vmx/tdx/tdx.c).
        assert_eq!(stop["gpr"]["rax"], "0x0", "{on:#}");
        assert_eq!(stop["gpr"]["rdx"], "0x0", "{on:#}");
        assert_eq!(stop["gpr"]["rcx"], "0x87", "{on:#}");
        assert_eq!(
            stop["retired"], 5,
            "CLI, 3 MOV, RDMSR; HLT is outside the set"
        );
        let off = run(CpuidProfile::DiagnosticV1);
        let stop = &off["stop"];
        assert_eq!(stop["exit_kind"], "unsupported", "{off:#}");
        assert_eq!(stop["unsupported_opcode"], "0x32", "{off:#}");
        assert_eq!(stop["retired"], 4, "{off:#}");
        assert_eq!(stop["gpr"]["rax"], "0x55", "{off:#}");
    }

    #[test]
    fn diagnostic_note_has_both_explanations_with_both_flags() {
        let note = |past_c7: bool,
                    norandmaps: bool,
                    no_kstack_randomization: bool,
                    profile: CpuidProfile| {
            let mut evidence = json!({});
            mark_acceptance(
                &mut evidence,
                past_c7,
                norandmaps,
                no_kstack_randomization,
                profile,
            );
            evidence
                .get("diagnostic_note")
                .and_then(Value::as_str)
                .map(str::to_string)
        };
        let both = note(true, false, false, CpuidProfile::X8664V1Stage4A).expect("note");
        assert!(both.contains("X8664V1Stage4A declares a subset"), "{both}");
        assert!(both.contains("--diagnostic-past-c7"), "{both}");
        assert!(both.contains("NOT counted"), "{both}");
        let subset = note(false, false, false, CpuidProfile::X8664V1Stage4A).expect("note");
        assert!(subset.contains("subset") && !subset.contains("--diagnostic-past-c7"));
        assert!(
            subset.contains("is counted") && !subset.contains("NOT counted"),
            "{subset}"
        );
        let diag = note(true, false, false, CpuidProfile::DiagnosticV1).expect("note");
        assert!(diag.contains("DiagnosticV1 answers CPUID leaf 1"), "{diag}");
        let norandmaps = note(false, true, false, CpuidProfile::X8664V1Stage4A).expect("note");
        assert!(
            norandmaps.contains("address-space randomization"),
            "{norandmaps}"
        );
        assert!(norandmaps.contains("NOT counted"), "{norandmaps}");
        let no_kstack = note(false, false, true, CpuidProfile::X8664V1Stage4A).expect("note");
        assert!(
            no_kstack.contains("kernel stack offset randomization"),
            "{no_kstack}"
        );
        assert!(no_kstack.contains("NOT counted"), "{no_kstack}");
        assert_eq!(note(false, false, false, CpuidProfile::DiagnosticV1), None);
    }

    /// S4-38L8d: the notes of the accepted evidence stay byte for byte (the APIC profile has its
    /// own verdict). The strings are the `diagnostic_note` of the saved reports
    /// `s4-39g-pic-pit-hlt-stage4a-run.json` (stage4a + `--diagnostic-past-c7`),
    /// `s4-39j-gate-counted-cold1-stage4a-run.json` (stage4a alone); the diagnostic + flag note is the
    /// one of `main` (42dc902), the S4-38L7 report predates the `; ` joint.
    #[test]
    fn the_notes_of_the_accepted_evidence_are_unchanged() {
        let note = |past_c7: bool, profile: CpuidProfile| {
            let mut evidence = json!({});
            mark_acceptance(&mut evidence, past_c7, false, false, profile);
            evidence["diagnostic_note"].as_str().unwrap().to_string()
        };
        assert_eq!(
            note(true, CpuidProfile::X8664V1Stage4A),
            "CPUID profile X8664V1Stage4A declares a subset of the target (TSC, FPU and FXSR; \
             checkpoint C7 boundary off by the profile); --diagnostic-past-c7 also turns the \
             checkpoint C7 boundary off by itself, so this run does not show that the profile \
             alone avoids it; the result is NOT counted"
        );
        assert_eq!(
            note(true, CpuidProfile::DiagnosticV1),
            "DiagnosticV1 answers CPUID leaf 1 and 0x80000001 (checkpoint C7 boundary off); not a \
             profile of the target CPU; the result is NOT counted"
        );
        assert_eq!(
            note(false, CpuidProfile::X8664V1Stage4A),
            "CPUID profile X8664V1Stage4A declares a subset of the target (TSC, FPU and FXSR; \
             checkpoint C7 boundary off by the profile); the result is counted (stage4a is the \
             counted profile)"
        );
    }

    #[test]
    fn acceptance_is_counted_for_both_profiles_without_the_flag_and_not_with_it() {
        let acceptance = |past_c7: bool,
                          norandmaps: bool,
                          no_kstack_randomization: bool,
                          profile: CpuidProfile| {
            let mut evidence = json!({});
            mark_acceptance(
                &mut evidence,
                past_c7,
                norandmaps,
                no_kstack_randomization,
                profile,
            );
            evidence["acceptance"].as_str().unwrap().to_string()
        };
        assert_eq!(
            acceptance(false, false, false, CpuidProfile::X8664V1Stage4A),
            "counted"
        );
        assert_eq!(
            acceptance(true, false, false, CpuidProfile::X8664V1Stage4A),
            "diagnostic_not_counted"
        );
        assert_eq!(
            acceptance(false, true, false, CpuidProfile::X8664V1Stage4A),
            "diagnostic_not_counted"
        );
        // The default profile keeps its earlier marking: counted without the flag, not with it.
        assert_eq!(
            acceptance(false, false, false, CpuidProfile::DiagnosticV1),
            "counted"
        );
        assert_eq!(
            acceptance(true, false, false, CpuidProfile::DiagnosticV1),
            "diagnostic_not_counted"
        );
    }

    #[test]
    fn default_profile_evidence_has_no_cpuid_profile_key() {
        let (value, _) = cpuid_leaf_case("default-profile-evidence", 1, false);
        assert!(value.get("cpuid_profile").is_none(), "{value:#}");
        assert_eq!(value["acceptance"], "counted");
        assert_eq!(value["stop"]["exit_kind"], "cpuid_divergence");
    }

    /// Real artifacts of S4-35/S4-36/S4-31g (outside Git): the kernel is loaded and no guest
    /// instruction runs. Run with `-- --ignored` on a machine that has `work/stage4`.
    #[test]
    #[ignore = "needs work/stage4 artifacts (vmlinuz, initramfs, ACPI blob v2)"]
    fn real_artifacts_load_without_running_the_guest() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
        let path = |p: &str| root.join(p).to_string_lossy().into_owned();
        let options = Options {
            vmlinuz: path("work/stage4/linux-7.0.0-38.38/vmlinuz-7.0.0-38-generic"),
            initramfs: path("work/stage4/initramfs/initramfs.cpio"),
            acpi_blob: path("tests/fixtures/guest/stage4/acpi_blob/stage4_acpi_blob.bin"),
            nonce: "real-run-1".to_string(),
            expected_vmlinuz_sha256:
                "9f55ef7253055a9cf68f8a04323c93dead2fca617386d842fbde9ca7b178b27e".to_string(),
            expected_acpi_manifest_hash: board_manifest_hash(),
            expected_initramfs_sha256:
                "ee9183a4eded3c16a8c6a7a81c6d5bb79be8bb023f793f7cd61694ea2bd3e588".to_string(),
            ram_mib: 128,
            out: String::new(),
            budget: None,
            vcpu_count: 1,
            interleaved_seed: None,
            rootfs_raw: None,
            set_cpl0: false,
            diagnostic_past_c7: false,
            cpuid_profile: CpuidProfile::DiagnosticV1,
            user_mode: false,
            nonce_bootconfig: false,
            linux_norandmaps: false,
            linux_no_randomize_kstack_offset: false,
            checkpoint_table: None,
            stack_context_out: None,
            execution_observations_out: None,
            execution_observations_post_iret_index: None,
            execution_observations_rdtsc_rip: None,
            execution_observations_rdtsc_entry_rip: None,
            execution_observations_rdtsc_stride: None,
            execution_observations_rdtsc_window: None,
            execution_observations_rdtsc_prefix: false,
            execution_observations_delay_call_summary: false,
            execution_observations_quantum_trace: false,
            execution_observations_elf_stack_copy: false,
        };
        let (value, exit) = evaluate(&options);
        assert_eq!(exit, 0, "{value:#}");
        assert_eq!(value["retired"], 0);
        assert_eq!(value["acpi_rsdp_addr"], "0xe0000");
        assert!(value["entry"]["rip"].as_str().unwrap().ends_with("200"));
        assert_eq!(value["cmdline_in_memory_matches"], true);
        let ecam = value["e820"]
            .as_array()
            .unwrap()
            .iter()
            .find(|entry| entry["addr"] == "0xfc100000")
            .expect("ECAM entry");
        assert_eq!(
            (ecam["size"].as_str(), ecam["type"].as_u64()),
            (Some("0x100000"), Some(2))
        );
    }

    /// Real kernel, first stop (S4-38L1): `startup_64` begins with CLD (`fc fa 31 c0`), which the
    /// base paged profile does not admit. The evidence file of the slice comes from the CLI.
    #[test]
    #[ignore = "needs work/stage4 artifacts (vmlinuz, initramfs, ACPI blob v2)"]
    fn real_artifacts_stop_at_the_first_instruction_of_the_kernel() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
        let path = |p: &str| root.join(p).to_string_lossy().into_owned();
        let options = Options {
            vmlinuz: path("work/stage4/linux-7.0.0-38.38/vmlinuz-7.0.0-38-generic"),
            initramfs: path("work/stage4/initramfs/initramfs.cpio"),
            acpi_blob: path("tests/fixtures/guest/stage4/acpi_blob/stage4_acpi_blob.bin"),
            nonce: "real-run-1".to_string(),
            expected_vmlinuz_sha256:
                "9f55ef7253055a9cf68f8a04323c93dead2fca617386d842fbde9ca7b178b27e".to_string(),
            expected_acpi_manifest_hash: board_manifest_hash(),
            expected_initramfs_sha256:
                "ee9183a4eded3c16a8c6a7a81c6d5bb79be8bb023f793f7cd61694ea2bd3e588".to_string(),
            ram_mib: 128,
            out: String::new(),
            budget: Some(1000),
            vcpu_count: 1,
            interleaved_seed: None,
            rootfs_raw: None,
            set_cpl0: false,
            diagnostic_past_c7: false,
            cpuid_profile: CpuidProfile::DiagnosticV1,
            user_mode: false,
            nonce_bootconfig: false,
            linux_norandmaps: false,
            linux_no_randomize_kstack_offset: false,
            checkpoint_table: None,
            stack_context_out: None,
            execution_observations_out: None,
            execution_observations_post_iret_index: None,
            execution_observations_rdtsc_rip: None,
            execution_observations_rdtsc_entry_rip: None,
            execution_observations_rdtsc_stride: None,
            execution_observations_rdtsc_window: None,
            execution_observations_rdtsc_prefix: false,
            execution_observations_delay_call_summary: false,
            execution_observations_quantum_trace: false,
            execution_observations_elf_stack_copy: false,
        };
        let (value, exit) = evaluate(&options);
        assert_eq!(exit, 0, "{value:#}");
        println!("{value:#}");
        let stop = &value["stop"];
        assert_eq!(stop["exit_kind"], "unsupported", "{value:#}");
        assert_eq!(stop["unsupported_opcode"], "0xfc");
        assert_eq!(stop["rip"], "0x1000200");
        assert_eq!(stop["rip_gpa"], "0x1000200");
        assert!(
            stop["rip_bytes"]
                .as_str()
                .unwrap()
                .starts_with("fc fa 31 c0")
        );
        assert_eq!(value["retired"], 0);
        assert_eq!(value["guest_started"], true);
    }

    /// S4-38L8d checkpoint, real kernel under `--cpuid-profile stage4a-apic` (release build:
    /// `cargo test -p od-runner --release --offline -- --ignored stage4a_apic_reaches_check_timer`,
    /// about ten minutes). The kernel enumerates the IOAPIC, `check_timer` takes the pin 2 path
    /// without a fallback message, and the board delivers the timer IRQs. The stop after it is NOT
    /// asserted: it moves with the next slices (today the SCI on IOAPIC pin 9, L8e). Evidence file
    /// of the slice: `docs/reports/data/stage4/s4-38l8d-apic-check-timer.json` (from the CLI).
    #[test]
    #[ignore = "needs work/stage4 artifacts; release build, minutes"]
    fn real_artifacts_stage4a_apic_reaches_check_timer() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
        let path = |p: &str| root.join(p).to_string_lossy().into_owned();
        let options = Options {
            vmlinuz: path("work/stage4/linux-7.0.0-38.38/vmlinuz-7.0.0-38-generic"),
            initramfs: path("work/stage4/initramfs/initramfs.cpio"),
            acpi_blob: path("tests/fixtures/guest/stage4/acpi_blob/stage4_acpi_blob.bin"),
            nonce: "s4-38l4a".to_string(),
            expected_vmlinuz_sha256:
                "9f55ef7253055a9cf68f8a04323c93dead2fca617386d842fbde9ca7b178b27e".to_string(),
            expected_acpi_manifest_hash: board_manifest_hash(),
            expected_initramfs_sha256:
                "ee9183a4eded3c16a8c6a7a81c6d5bb79be8bb023f793f7cd61694ea2bd3e588".to_string(),
            ram_mib: 128,
            out: String::new(),
            budget: Some(2_600_000_000),
            vcpu_count: 1,
            interleaved_seed: None,
            rootfs_raw: None,
            set_cpl0: true,
            diagnostic_past_c7: false,
            cpuid_profile: CpuidProfile::X8664V1Stage4AApic,
            user_mode: false,
            nonce_bootconfig: false,
            linux_norandmaps: false,
            linux_no_randomize_kstack_offset: false,
            checkpoint_table: Some(path("docs/reports/data/stage4/s4-38l2-oracle.json")),
            stack_context_out: None,
            execution_observations_out: None,
            execution_observations_post_iret_index: None,
            execution_observations_rdtsc_rip: None,
            execution_observations_rdtsc_entry_rip: None,
            execution_observations_rdtsc_stride: None,
            execution_observations_rdtsc_window: None,
            execution_observations_rdtsc_prefix: false,
            execution_observations_delay_call_summary: false,
            execution_observations_quantum_trace: false,
            execution_observations_elf_stack_copy: false,
        };
        let (value, exit) = evaluate(&options);
        assert_eq!(exit, 0, "{value:#}");
        assert_eq!(value["cpuid_profile"], "X8664V1Stage4AApic");
        assert_eq!(value["acceptance"], "diagnostic_not_counted");
        let stop = &value["stop"];
        let com1 = stop["com1_text"].as_str().unwrap();
        let ioapic = com1
            .find("IOAPIC[0]: apic_id 1, version 32, address 0xfec00000, GSI 0-23")
            .unwrap_or_else(|| panic!("no IOAPIC line: {stop:#}"));
        let timer = com1
            .find("..TIMER: vector=0x30 apic1=0 pin1=2 apic2=-1 pin2=-1")
            .unwrap_or_else(|| panic!("no ..TIMER line: {stop:#}"));
        assert!(ioapic < timer, "the IOAPIC line comes first");
        for bad in [
            "MP-BIOS bug",
            "trying to set up timer",
            "IO-APIC + timer doesn't work",
            "No local APIC present",
        ] {
            assert!(!com1[timer..].contains(bad), "{bad}");
        }
        assert!(
            stop["irqs_delivered"].as_u64().unwrap() >= 5,
            "{}",
            stop["irqs_delivered"]
        );
        assert!(stop["irq0_edges"].as_u64().unwrap() >= 5);
        let c13 = value["checkpoints"]["records"]
            .as_array()
            .unwrap()
            .iter()
            .find(|record| record["id"] == "C13")
            .expect("C13");
        assert_eq!(c13["status"], "reached", "{c13:#}");
    }

    /// Real kernel through the CPL0 set (S4-38L3): the null segment loads (S2-46) and the dword
    /// `MOV EAX, [RSI+0x230]` retire, `LGDT [RAX]` (S2-47a) loads the GDTR, `LRETQ` (S2-47b)
    /// reloads CS and `ENDBR64` (S2-48) is a NOP; the adapter's byte, word and dword stores let
    /// the byte store `MOV [RAX], SIL` retire, `LIDT` (S2-49) retires and, with the `cpuid` member
    /// (S4-38L4), the `CPUID` of leaf 0x8000_0000 is answered; the first stop is past it (a
    /// further unsupported form or the CPUID boundary). The evidence file of the slice comes from
    /// the CLI. The nonce is the evidence's (`s4-38l4a`): the cmdline carries it, and its length
    /// changes the instruction count of the boot (`real-run-1` gives 1411 retired, `s4-38l4a`
    /// gives 1385 on this stop), so the expectations below are for this nonce only.
    #[test]
    #[ignore = "needs work/stage4 artifacts (vmlinuz, initramfs, ACPI blob v2)"]
    fn real_artifacts_checkpoint_at_budget_30m_past_the_qword_read() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
        let path = |p: &str| root.join(p).to_string_lossy().into_owned();
        let options = Options {
            vmlinuz: path("work/stage4/linux-7.0.0-38.38/vmlinuz-7.0.0-38-generic"),
            initramfs: path("work/stage4/initramfs/initramfs.cpio"),
            acpi_blob: path("tests/fixtures/guest/stage4/acpi_blob/stage4_acpi_blob.bin"),
            nonce: "s4-38l4a".to_string(),
            expected_vmlinuz_sha256:
                "9f55ef7253055a9cf68f8a04323c93dead2fca617386d842fbde9ca7b178b27e".to_string(),
            expected_acpi_manifest_hash: board_manifest_hash(),
            expected_initramfs_sha256:
                "ee9183a4eded3c16a8c6a7a81c6d5bb79be8bb023f793f7cd61694ea2bd3e588".to_string(),
            ram_mib: 128,
            out: String::new(),
            budget: Some(30_000_000),
            vcpu_count: 1,
            interleaved_seed: None,
            rootfs_raw: None,
            set_cpl0: true,
            diagnostic_past_c7: false,
            cpuid_profile: CpuidProfile::DiagnosticV1,
            user_mode: false,
            nonce_bootconfig: false,
            linux_norandmaps: false,
            linux_no_randomize_kstack_offset: false,
            checkpoint_table: None,
            stack_context_out: None,
            execution_observations_out: None,
            execution_observations_post_iret_index: None,
            execution_observations_rdtsc_rip: None,
            execution_observations_rdtsc_entry_rip: None,
            execution_observations_rdtsc_stride: None,
            execution_observations_rdtsc_window: None,
            execution_observations_rdtsc_prefix: false,
            execution_observations_delay_call_summary: false,
            execution_observations_quantum_trace: false,
            execution_observations_elf_stack_copy: false,
        };
        let (value, exit) = evaluate(&options);
        println!("{value:#}");
        // S4-38L6: the qword read across the 4 KiB boundary (`MOV RCX, [RAX]`, S4-38L5e) and the
        // `#PF` delivery (S4-38L5c) are passed. The budget checkpoint is deterministic: with budget
        // 30000000 the run is `budget_exhausted` at `MOV [RSP+0x18], RAX` (`48 89 44 24 18`) on
        // 0x543_4d78 (retired 30000000, no architectural stop). The 200M run stops at 0x543_4e32
        // (`budget_exhausted`, 2030 s user CPU: about 98k instructions/s, see STAGE_4 L6). Evidence:
        // `docs/reports/data/stage4/s4-38l5e-cross-page-read-run.json` (30M) and
        // `docs/reports/data/stage4/s4-38l6-budget-200m-run.json` (200M).
        assert_eq!(value["status"], "loaded", "{value:#}");
        assert_eq!(exit, 0, "{value:#}");
        let stop = &value["stop"];
        assert_eq!(value["retired"], 30_000_000, "{value:#}");
        assert_eq!(stop["quanta"], 2148, "{value:#}");
        assert_eq!(stop["exit_kind"], "budget_exhausted", "{value:#}");
        assert_eq!(stop["rip"], "0x5434d78");
        assert!(
            stop["rip_bytes"]
                .as_str()
                .unwrap()
                .starts_with("48 89 44 24 18"),
            "{value:#}"
        );
        assert_eq!(stop["set_exit_detail"], "Run(InstructionBudgetExhausted)");
        assert!(stop["memory_error"].is_null());
        assert!(stop["page_fault"].is_null());
        assert_eq!(
            stop["set_members"],
            json!([
                "cli",
                "cpu_owned_cr3",
                "cpuid:DiagnosticV1",
                "rdtsc",
                "msr:EferReadOnlyV1(3328)",
                "msr:fs_gs_base",
                "msr:star_write",
                "msr:lstar_write",
                "msr:cstar_write",
                "msr:sysenter_cs_write",
                "msr:sysenter_stack_write",
                "msr:syscall_mask_write",
                "msr:kernel_gs_base_write",
                "pio_u8",
                "sti",
                "popf_cpl0",
                "pci_config",
                "irq"
            ])
        );
        // S2-46: the null MOV DS/ES/SS/FS/GS and LEA RBP retire. S4-38L3: the set's adapter
        // reads the dword `MOV EAX, [RSI+0x230]` (RIP 0x100_0215) and 13 more instructions
        // retire (22 in all). S2-47a: `LGDT [RAX]` (`0f 01 10`) at 0x100_0257 retires and four
        // more instructions follow (26 in all). S2-47b: `LRETQ` (`48 cb`) at 0x100_0264 reloads
        // CS = 0x10 from the loaded GDT (descriptor `0x00af9a000000ffff`, accessed bit set by
        // the load), then `mov r15, rsi` and `call 0x2066410` retire (29 in all); the next stop
        // is the `ENDBR64` (`f3 0f 1e fa`, opcode 0x1e) at the call target. S2-48: the set
        // runs `ENDBR64` as a NOP (no CET_IBT) and 20 more instructions retire (50 in all); the
        // next stop is the byte store `MOV [RAX], SIL` (`40 88 30`) at 0x206_3a80, which the
        // board's paged adapter refuses (`paged MOV profile does not support non-qword writes`).
        // S4-38L3: the adapter's byte, word and dword stores let `MOV [RAX], SIL` retire and 67
        // more instructions follow (118 in all); the next stop was `LIDT [RIP+0x17bbd]`
        // (`0f 01 1d bd 7b 01 00`) at 0x206_647c. S2-49: `LIDT` retires; the next stop was
        // `CPUID` (`0f a2`) with EAX = 0x8000_0000 at 0x206_7b0d after 138 retired instructions.
        // S4-38L4: the set has the `cpuid` member (DiagnosticV1), which answers leaf 0x8000_0000.
        // S4-38L4a: the one-byte read of `CMP byte [RDI+0x1EF], 0` is admitted; S2-50: POPFQ on
        // CPL0 is admitted and the members list gains `popf_cpl0`. The stop-record checks of the
        // S2-49..S4-38L4a runs do not apply while the run is refused (see above).
    }

    // ---- S4-41: lifecycle of the headless runner for the linux-direct machine ----
    //
    // The lifecycle of this machine is "load, run to the first stop, report"; a stop is final for
    // the run. There is no pause/resume API on the Linux board (it exists only on the diagnostic
    // and paged vCPU sessions); a second `run_linux_direct_board*` call on the same board continues
    // from the stop, which is what the budget-slice test below exercises.

    /// Everything one load needs; the board borrows nothing from it after `load`.
    struct Session {
        vmlinuz: Vec<u8>,
        initramfs: Vec<u8>,
        cmdline: String,
    }

    impl Session {
        fn new(code: &[u8]) -> Self {
            Self {
                vmlinuz: bzimage_with_entry(code),
                initramfs: vec![0xAB; 4688],
                cmdline: cmdline("N0nce-1", false, false),
            }
        }

        fn input(&self) -> LinuxDirectInput<'_> {
            LinuxDirectInput {
                bzimage: &self.vmlinuz,
                initrd: &self.initramfs,
                cmdline: self.cmdline.as_bytes(),
                acpi_blob: BLOB,
                expected_acpi_manifest_hash: board_manifest_hash(),
                ram_bytes: 128 * MIB,
            }
        }
    }

    /// Loads a board and runs it once per budget; each call continues from the previous stop.
    fn run_slices(code: &[u8], budgets: &[u64], set_cpl0: bool) -> Vec<LinuxStopRecord> {
        let session = Session::new(code);
        let input = session.input();
        let mut boot = load_linux_direct_board(&input).expect("load");
        budgets
            .iter()
            .map(|&budget| {
                if set_cpl0 {
                    run_linux_direct_board_set(
                        &mut boot,
                        &input,
                        budget,
                        LinuxCpl0SetMembers::linux_default(),
                    )
                } else {
                    run_linux_direct_board(&mut boot, &input, budget)
                }
                .expect("run")
            })
            .collect()
    }

    /// MOV AL,'X'; MOV DX,0x3F8; OUT DX,AL; HLT (4 instructions, writes `X` to COM1).
    const COM1_X: [u8; 8] = [0xb0, b'X', 0x66, 0xba, 0xf8, 0x03, 0xee, 0xf4];
    /// MOV DX,0x604; MOV AL,0x34; OUT DX,AL (SLP_TYP=S5, SLP_EN: the guest's own STOP); HLT.
    const ACPI_S5: [u8; 8] = [0x66, 0xBA, 0x05, 0x06, 0xB0, 0x34, 0xEE, 0xF4];

    #[test]
    fn lifecycle_refusals_with_run_never_start_the_guest() {
        // Every invalid configuration is refused before the machine runs, also when the run was
        // asked for: no `stop` object, `guest_started` false, `retired` 0, and a stable exit code
        // (3 unreadable input, 4 refused).
        type Break = fn(&mut Case);
        let cases: [(&str, Break, u8, &str); 7] = [
            (
                "vmlinuz-missing",
                |c| c.options.vmlinuz = c.dir.join("nope").to_string_lossy().into_owned(),
                3,
                "invalid_input",
            ),
            (
                "initramfs-missing",
                |c| c.options.initramfs = c.dir.join("nope").to_string_lossy().into_owned(),
                3,
                "invalid_input",
            ),
            (
                "acpi-missing",
                |c| c.options.acpi_blob = c.dir.join("nope").to_string_lossy().into_owned(),
                3,
                "invalid_input",
            ),
            (
                "vmlinuz-sha",
                |c| c.options.expected_vmlinuz_sha256 = "0".repeat(64),
                4,
                "vmlinuz_sha256_mismatch",
            ),
            (
                "initramfs-sha",
                |c| c.options.expected_initramfs_sha256 = "0".repeat(64),
                4,
                "initramfs_sha256_mismatch",
            ),
            (
                "acpi-manifest",
                |c| c.options.expected_acpi_manifest_hash[0] ^= 1,
                4,
                "acpi_manifest_mismatch",
            ),
            (
                "initrd-too-large",
                |c| c.options.ram_mib = 1,
                4,
                "initrd_too_large",
            ),
        ];
        for (name, break_it, exit_code, code) in cases {
            // The initramfs is 2 MiB so that `ram_mib = 1` makes it too large.
            let mut case = Case::new(
                name,
                &bzimage_with_entry(&COM1_X),
                &vec![0xAB; 2 << 20],
                BLOB,
            );
            case.options.budget = Some(100);
            case.options.set_cpl0 = true;
            // The control: the same case, unbroken, does run the guest.
            if name == "vmlinuz-missing" {
                let (value, exit) = evaluate(&case.options);
                assert_eq!(
                    (exit, &value["guest_started"]),
                    (0, &json!(true)),
                    "{value:#}"
                );
            }
            break_it(&mut case);
            let (value, exit) = evaluate(&case.options);
            assert_eq!(exit, exit_code, "{name}: {value:#}");
            assert_eq!(value["error"]["code"], code, "{name}: {value:#}");
            assert_eq!(value["guest_started"], false, "{name}");
            assert_eq!(value["retired"], 0, "{name}");
            assert!(value.get("stop").is_none(), "{name}");
        }
    }

    #[test]
    fn lifecycle_argument_refusals_are_exit_3_and_never_reach_the_machine() {
        // `run` prints an `invalid_input` JSON and returns exit 3 before any file is read: an
        // unknown CPUID profile name, `--user-mode` with the diagnostic profile or without
        // `--set-cpl0`, a malformed nonce.
        let out = std::env::temp_dir().join(format!("od-runner-s441-{}.json", std::process::id()));
        let out = out.to_string_lossy().into_owned();
        let hash = "a".repeat(64);
        let base = |extra: &[&str]| -> Vec<String> {
            let mut args: Vec<String> = [
                "--vmlinuz",
                "/nonexistent/vmlinuz",
                "--initramfs",
                "/nonexistent/initramfs",
                "--acpi-blob",
                "/nonexistent/acpi",
                "--nonce",
                "n1",
                "--expected-vmlinuz-sha256",
                hash.as_str(),
                "--expected-acpi-manifest-hash",
                hash.as_str(),
                "--expected-initramfs-sha256",
                hash.as_str(),
                "--ram-mib",
                "128",
                "--out",
                out.as_str(),
            ]
            .iter()
            .map(|s| s.to_string())
            .collect();
            args.extend(extra.iter().map(|s| s.to_string()));
            args
        };
        // Positive control: the base arguments parse, and the pipeline then reaches the files
        // (the unreadable kernel is a stage after `parse`, so `parse` accepted everything else).
        let good = parse(&base(&["--run", "--budget", "1"])).unwrap();
        assert_eq!(good.budget, Some(1));
        let (value, exit) = evaluate(&good);
        assert_eq!(exit, 3, "{value:#}");
        assert_eq!(code_of(&value), ("invalid_input", "vmlinuz_file"));
        assert!(
            parse(&base(&[
                "--run",
                "--budget",
                "1",
                "--set-cpl0",
                "--cpuid-profile",
                "stage4a",
                "--user-mode"
            ]))
            .unwrap()
            .user_mode
        );

        let set_run = ["--run", "--budget", "1", "--set-cpl0"];
        let user_mode_needs = "--user-mode needs --run, --set-cpl0 and --cpuid-profile stage4a";
        let cases: [(Vec<&str>, &str); 7] = [
            // Unknown profile name.
            (
                [&set_run[..], &["--cpuid-profile", "nope"]].concat(),
                "invalid --cpuid-profile 'nope'",
            ),
            // `--user-mode` with the diagnostic profile: the `profile != stage4a` branch.
            (
                [
                    &set_run[..],
                    &["--cpuid-profile", "diagnostic", "--user-mode"],
                ]
                .concat(),
                user_mode_needs,
            ),
            // `--user-mode` without `--set-cpl0` and without a profile: the default profile is
            // diagnostic, so it is the same `profile != stage4a` branch.
            (
                vec!["--run", "--budget", "1", "--user-mode"],
                user_mode_needs,
            ),
            // A profile without `--set-cpl0` is refused first, by its own branch.
            (
                vec![
                    "--run",
                    "--budget",
                    "1",
                    "--cpuid-profile",
                    "stage4a",
                    "--user-mode",
                ],
                "--cpuid-profile needs --run and --set-cpl0",
            ),
            // A duplicated `--nonce` (the base already has one).
            (
                vec!["--run", "--budget", "1", "--nonce", "again"],
                "duplicate option --nonce",
            ),
            // `--run` without a budget.
            (vec!["--run"], "--run needs --budget"),
            // A trailing flag without a value.
            (
                vec!["--run", "--budget", "1", "--nonce"],
                "флаг --nonce требует значение",
            ),
        ];
        let exit_3 = format!("{:?}", ExitCode::from(3));
        for (extra, message) in cases {
            let args = base(&extra);
            let error = parse(&args).unwrap_err();
            assert!(error.contains(message), "{extra:?}: {error}");
            assert_eq!(format!("{:?}", run(&args)), exit_3, "{extra:?}");
        }
        // The malformed nonce `a b` is a case of its own: it replaces the base nonce.
        let mut args = base(&["--run", "--budget", "1"]);
        let at = args.iter().position(|a| a == "--nonce").unwrap() + 1;
        args[at] = "a b".to_string();
        assert!(parse(&args).unwrap_err().starts_with("invalid --nonce"));
        assert_eq!(format!("{:?}", run(&args)), exit_3);
        // Nothing was written to `--out` (there is no evidence file for a bad argument).
        assert!(!std::path::Path::new(&out).exists());
    }

    #[test]
    fn lifecycle_guest_fault_is_a_reported_stop_and_leaves_no_state_behind() {
        // CLI; then the REX filler after the code has no opcode: #GP(0) is the stop `fault`.
        let fault_case = |name: &str| {
            let mut case = Case::new(name, &bzimage_with_entry(&[0xFA]), &[0xAB; 4688], BLOB);
            case.options.budget = Some(64);
            case.options.set_cpl0 = true;
            case
        };
        let good = |name: &str| {
            let mut case = Case::new(name, &bzimage_with_entry(&COM1_X), &[0xAB; 4688], BLOB);
            case.options.budget = Some(64);
            case
        };
        let baseline = evaluate(&good("fault-baseline").options);
        assert_eq!(baseline.1, 0, "{:#}", baseline.0);
        assert_eq!(baseline.0["stop"]["exit_kind"], "halted");

        let (value, exit) = evaluate(&fault_case("fault-run").options);
        // A fault is a result, not a failure of the runner: exit 0, `loaded`, no panic.
        assert_eq!(exit, 0, "{value:#}");
        assert_eq!(value["status"], "loaded");
        assert_eq!(value["guest_started"], true);
        assert_eq!(value["stop"]["exit_kind"], "fault", "{value:#}");
        assert!(value["stop"]["retired"].as_u64().unwrap() >= 1, "{value:#}");
        assert!(
            value["stop"]["exit_detail"]
                .as_str()
                .unwrap()
                .starts_with("Fault(ArchitecturalFault { vector: 13")
        );

        // The next session in the same process is a fresh board: same result as the baseline.
        let again = evaluate(&good("fault-after").options);
        let normalize = |mut v: Value| {
            v["inputs"] = Value::Null;
            v
        };
        assert_eq!(normalize(again.0), normalize(baseline.0));
        assert_eq!(again.1, baseline.1);
    }

    #[test]
    fn lifecycle_repeated_session_in_one_process_gives_the_same_evidence() {
        // Two sessions in a row in the same process on the same inputs (a counted-profile user
        // mode run) must produce identical evidence: nothing persists between loads (no cache,
        // counter or device state carried over).
        let mut case = Case::new(
            "repeat",
            &bzimage_with_entry(&iretq_to_cpl3_program()),
            &[0xAB; 4688],
            BLOB,
        );
        case.options.budget = Some(100);
        case.options.set_cpl0 = true;
        case.options.cpuid_profile = CpuidProfile::X8664V1Stage4A;
        case.options.user_mode = true;
        let first = evaluate(&case.options);
        let second = evaluate(&case.options);
        assert_eq!(first.1, 0, "{:#}", first.0);
        // The run is a guest fault in CPL3 (the board page is supervisor-only), a stop like any other.
        assert_eq!(first.0["stop"]["exit_kind"], "user_mode_fault");
        assert_eq!(first.0["stop"]["retired"], 13);
        assert_eq!(first, second);
    }

    #[test]
    fn lifecycle_stop_is_prompt_final_and_idempotent() {
        // STOP by budget: 0 stops before the first fetch, nothing retired.
        let zero = run_slices(&COM1_X, &[0], false);
        assert_eq!(zero[0].exit_kind, "budget_exhausted");
        assert_eq!(zero[0].retired, 0);
        // STOP by the guest (PM1_CNT S5): the machine reports `stopped` with a shutdown reason.
        let stopped = run_slices(&ACPI_S5, &[100, 100], true);
        assert_eq!(stopped[0].exit_kind, "stopped");
        assert_eq!(stopped[0].stop_reason, Some(MachineStopReason::Shutdown));
        // A second call after the STOP does not run the guest further: same stop, same RIP.
        assert_eq!(
            stopped[1].exit_kind, "stopped",
            "{:?}",
            stopped[1].exit_detail
        );
        assert_eq!(stopped[1].rip, stopped[0].rip);
        assert_eq!(stopped[1].retired, 0, "the guest did not run after STOP");
        assert_eq!(stopped[1].com1, stopped[0].com1);
    }

    #[test]
    fn lifecycle_budget_slices_continue_the_same_board_like_one_run() {
        // The Linux board has no pause/resume API; a budget stop followed by another call is the
        // closest thing (there is no asynchronous pause from outside). The slices are compared with
        // the whole run by `rip`, `gpr`, `rflags` and COM1; guest RAM is not compared.
        let whole = run_slices(&COM1_X, &[100], false);
        let sliced = run_slices(&COM1_X, &[2, 100], false);
        assert_eq!(whole[0].exit_kind, "halted");
        assert_eq!(sliced[0].exit_kind, "budget_exhausted");
        // `retired` counts one call: 2 + 2 = the 4 of the whole run.
        assert_eq!(sliced[0].retired, 2);
        assert_eq!(sliced[1].retired, 2);
        assert_eq!(whole[0].retired, 4);
        assert!(sliced[0].com1.is_empty(), "OUT not yet executed");
        assert_eq!(sliced[1].exit_kind, "halted");
        assert_eq!(sliced[1].rip, whole[0].rip);
        assert_eq!(sliced[1].gpr, whole[0].gpr);
        assert_eq!(sliced[1].rflags, whole[0].rflags);
        assert_eq!(sliced[1].com1, b"X");
        assert_eq!(sliced[1].com1, whole[0].com1);
    }

    // ---- S4-38L4i: --checkpoint-table ----

    /// `CLI; MOV EAX, 0; CPUID; MOV EAX, 1; CPUID`: DiagnosticV1 answers leaf 0 (C6) and stops
    /// before leaf 1 (C7) at `ENTRY + 13`.
    const CPUID_PROGRAM: [u8; 15] = [
        0xFA, 0xB8, 0, 0, 0, 0, 0x0F, 0xA2, 0xB8, 1, 0, 0, 0, 0x0F, 0xA2,
    ];
    const ORACLE_ENTRY: u64 = 0x0100_0200;

    /// A synthetic oracle with the keys the runner reads: the vmlinuz hash, the relocated copy and
    /// the rows. `cpuid1` is at `cpuid1_pa` (the rest are addresses the program never reaches).
    fn synthetic_oracle(vmlinuz_sha: &str, cpuid1_pa: u64) -> Value {
        let row = |name: &str, pa: u64, bytes_16: &str| json!({"name": name, "va": 0xffff_ffff_8000_0000u64 + pa, "pa": pa, "bytes_16": bytes_16});
        let mut rows = vec![
            row("e_entry", 0x3f2_53e0, "4989f7488d255eebadffb9010100c031"),
            row("cpuid1", cpuid1_pa, "0fa2"),
        ];
        for (index, name) in [
            "x86_64_start_kernel",
            "start_kernel",
            "setup_arch",
            "tsc_init",
            "apic_intr_mode_init",
            "check_timer",
            "populate_rootfs",
            "run_init_process",
            "kernel_execve",
        ]
        .iter()
        .enumerate()
        {
            rows.push(row(name, 0x200_0000 + 0x1000 * index as u64, "00"));
        }
        json!({
            "inputs": {"vmlinuz": {"sha256": vmlinuz_sha}},
            "relocated_copy": {"relocated_rip_first_fetch": 0x541_b9c0},
            "checkpoints": {"rows": rows},
        })
    }

    fn write_oracle(case: &Case, oracle: &Value) -> String {
        let path = case.dir.join("oracle.json");
        std::fs::write(&path, serde_json::to_vec(oracle).unwrap()).unwrap();
        path.to_string_lossy().into_owned()
    }

    fn checkpoint_case(name: &str) -> Case {
        let mut case = Case::new(
            name,
            &bzimage_with_entry(&CPUID_PROGRAM),
            &[0xAB; 4688],
            BLOB,
        );
        case.options.budget = Some(64);
        case.options.set_cpl0 = true;
        case
    }

    fn checkpoint<'a>(value: &'a Value, id: &str) -> &'a Value {
        value["checkpoints"]["records"]
            .as_array()
            .unwrap()
            .iter()
            .find(|record| record["id"] == id)
            .unwrap_or_else(|| panic!("no {id} in {value:#}"))
    }

    #[test]
    fn without_the_flag_the_report_has_no_checkpoints_key() {
        let case = checkpoint_case("cp-none");
        let (value, exit) = evaluate(&case.options);
        assert_eq!(exit, 0, "{value:#}");
        assert!(value.get("checkpoints").is_none(), "{value:#}");
        assert!(value["stop"].get("checkpoints").is_none());
    }

    #[test]
    fn the_flag_records_the_checkpoints_and_changes_nothing_else() {
        let mut case = checkpoint_case("cp-table");
        let (plain, _) = evaluate(&case.options);
        let sha = case.options.expected_vmlinuz_sha256.clone();
        let oracle = synthetic_oracle(&sha, ORACLE_ENTRY + 13);
        case.options.checkpoint_table = Some(write_oracle(&case, &oracle));
        let (value, exit) = evaluate(&case.options);
        assert_eq!(exit, 0, "{value:#}");
        assert_eq!(value["stop"]["exit_kind"], "cpuid_divergence");
        let c0 = checkpoint(&value, "C0");
        assert_eq!(
            (&c0["status"], &c0["retired"], &c0["rip"]),
            (&json!("reached"), &json!(0), &json!("0x1000200"))
        );
        let c6 = checkpoint(&value, "C6");
        assert_eq!(
            (&c6["status"], &c6["retired"]),
            (&json!("reached"), &json!(2))
        );
        assert_eq!(c6["rip"], "0x1000206");
        let c7 = checkpoint(&value, "C7");
        assert_eq!(c7["status"], "reached");
        assert_eq!(c7["rip"], "0x100020d");
        // The boundary of the stop: the counters are the ones of the stop record.
        assert_eq!(c7["retired"], value["stop"]["retired"]);
        assert_eq!(c7["quanta"], value["stop"]["quanta"]);
        assert_eq!(c7["gpa"], "0x100020d");
        assert_eq!(c7["bytes_match_oracle"], true);
        // The others are listed, with a reason.
        for id in ["C1", "C2", "C3", "C4", "C5", "C8", "C13", "C15"] {
            let record = checkpoint(&value, id);
            assert_eq!(record["status"], "not_reached", "{id}");
            assert!(
                record["reason"]
                    .as_str()
                    .unwrap()
                    .contains("cpuid_divergence")
            );
        }
        let c16 = checkpoint(&value, "C16");
        assert!(
            c16["reason"].as_str().unwrap().contains("after C15"),
            "{c16}"
        );
        let c17 = checkpoint(&value, "C17");
        assert!(c17["reason"].as_str().unwrap().contains("/init"), "{c17}");
        assert_eq!(
            value["checkpoints"]["records"].as_array().unwrap().len(),
            18
        );
        assert_eq!(value["checkpoints"]["table"]["vmlinuz_sha256"], sha);
        assert_eq!(
            value["checkpoints"]["table"]["vmlinuz_sha256_matches_the_run"],
            true
        );
        // Observation only: without the key the report is the report of the run without the flag
        // (the table path is the only other thing that differs: it is not in the report).
        let mut stripped = value.clone();
        stripped.as_object_mut().unwrap().remove("checkpoints");
        assert_eq!(stripped, plain);
    }

    #[test]
    fn a_shifted_address_is_not_reached() {
        let mut case = checkpoint_case("cp-shifted");
        let sha = case.options.expected_vmlinuz_sha256.clone();
        let oracle = synthetic_oracle(&sha, ORACLE_ENTRY + 13 + 1);
        case.options.checkpoint_table = Some(write_oracle(&case, &oracle));
        let (value, exit) = evaluate(&case.options);
        assert_eq!(exit, 0, "{value:#}");
        assert_eq!(checkpoint(&value, "C7")["status"], "not_reached");
        assert_eq!(checkpoint(&value, "C6")["status"], "reached");
    }

    #[test]
    fn an_oracle_of_another_kernel_is_refused_before_the_guest_starts() {
        let mut case = checkpoint_case("cp-foreign");
        let oracle = synthetic_oracle(&"0".repeat(64), ORACLE_ENTRY + 13);
        case.options.checkpoint_table = Some(write_oracle(&case, &oracle));
        let (value, exit) = evaluate(&case.options);
        assert_refused(
            &value,
            exit,
            "checkpoint_table_vmlinuz_mismatch",
            "checkpoint_table",
        );
    }

    #[test]
    fn an_unreadable_or_incomplete_table_is_invalid_input() {
        let mut case = checkpoint_case("cp-invalid");
        case.options.checkpoint_table = Some("/nonexistent/oracle.json".to_string());
        let (value, exit) = evaluate(&case.options);
        assert_eq!(exit, 3, "{value:#}");
        assert_eq!(code_of(&value), ("invalid_input", "checkpoint_table"));
        let sha = case.options.expected_vmlinuz_sha256.clone();
        let mut oracle = synthetic_oracle(&sha, ORACLE_ENTRY + 13);
        oracle["checkpoints"]["rows"] = json!([]);
        case.options.checkpoint_table = Some(write_oracle(&case, &oracle));
        let (value, exit) = evaluate(&case.options);
        assert_eq!(exit, 3, "{value:#}");
        assert_eq!(code_of(&value), ("invalid_input", "checkpoint_table"));
    }

    #[test]
    fn the_flag_needs_run_and_set_cpl0() {
        let base = |extra: &[&str]| -> Vec<String> {
            let mut args: Vec<String> = [
                "--vmlinuz",
                "k",
                "--initramfs",
                "i",
                "--acpi-blob",
                "b",
                "--nonce",
                "n",
                "--expected-vmlinuz-sha256",
                &"a".repeat(64),
                "--expected-acpi-manifest-hash",
                &"b".repeat(64),
                "--expected-initramfs-sha256",
                &"c".repeat(64),
                "--ram-mib",
                "128",
                "--out",
                "o",
            ]
            .iter()
            .map(|a| a.to_string())
            .collect();
            args.extend(extra.iter().map(|a| a.to_string()));
            args
        };
        let ok = parse(&base(&[
            "--run",
            "--budget",
            "1",
            "--set-cpl0",
            "--checkpoint-table",
            "t.json",
        ]))
        .unwrap();
        assert_eq!(ok.checkpoint_table.as_deref(), Some("t.json"));
        assert_eq!(
            parse(&base(&["--run", "--budget", "1", "--set-cpl0"]))
                .unwrap()
                .checkpoint_table,
            None
        );
        for bad in [
            vec!["--checkpoint-table", "t.json"],
            vec!["--run", "--budget", "1", "--checkpoint-table", "t.json"],
            vec![
                "--run",
                "--budget",
                "1",
                "--set-cpl0",
                "--checkpoint-table",
                "a",
                "--checkpoint-table",
                "b",
            ],
            vec!["--run", "--budget", "1", "--set-cpl0", "--checkpoint-table"],
        ] {
            assert!(parse(&base(&bad)).is_err(), "{bad:?}");
        }
    }
}
