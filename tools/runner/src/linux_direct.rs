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
//! counted and `cr3` is its CR3). Since S4-38L5c a guest `#PF` is delivered through the guest IDT
//! when one is loaded; `exceptions_delivered` counts these (a delivery is not a retired
//! instruction), and a `#PF` that could not be delivered is still the `fault` stop, with the reason
//! in `set_exit_detail`. The `paged_probe` path carries the same fields with empty
//! values; `schema_version` stays 1 (the fields are additive).
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

use od_machine::{
    LinuxCpl0SetMembers, LinuxDirectBoot, LinuxDirectError, LinuxDirectInput, LinuxStopRecord,
    load_linux_direct_board, run_linux_direct_board, run_linux_direct_board_set,
};
use od_testkit::sha256_hex;
use serde_json::{Value, json};
use std::fs::File;
use std::io::Read;
use std::process::ExitCode;

const USAGE: &str = "linux-direct --vmlinuz <file> --initramfs <file> --acpi-blob <file> \
--nonce <token> --expected-vmlinuz-sha256 <hex> --expected-acpi-manifest-hash <hex> \
--ram-mib <n> --out <json> --expected-initramfs-sha256 <hex> [--run --budget <n> [--set-cpl0 [--diagnostic-past-c7]]]";

/// Kernel command line of the S4-29g audit (`docs/reports/STAGE_4_S4-29G_2026-11-09.md`); the
/// nonce is appended as `od.nonce=<nonce>`.
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
    nonce: String,
    expected_vmlinuz_sha256: String,
    expected_acpi_manifest_hash: [u8; 32],
    expected_initramfs_sha256: String,
    ram_mib: u64,
    out: String,
    /// `--run --budget N`: run the guest for at most N instructions after loading.
    budget: Option<u64>,
    /// `--set-cpl0`: run through the CPL0 profile set instead of the base probe profile.
    set_cpl0: bool,
    /// `--diagnostic-past-c7` (S4-38L7): CPUID boundary off, result diagnostic and not counted.
    diagnostic_past_c7: bool,
}

pub fn run(args: &[String]) -> ExitCode {
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
    let (value, code) = evaluate(&options);
    let text = format!("{value:#}\n");
    if let Err(error) = std::fs::write(&options.out, &text) {
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

/// Runs the whole pipeline and returns the JSON evidence and the exit code (0 loaded, 4 refused,
/// 3 unreadable input). Without `--run` the guest is not started; with `--run` it runs until the
/// budget or its first stop, after the load has succeeded.
fn evaluate(options: &Options) -> (Value, u8) {
    let mut evidence = json!({
        "retired": 0,
        "guest_started": false,
        "nonce": options.nonce,
        "ram_mib": options.ram_mib,
    });
    mark_acceptance(&mut evidence, options.diagnostic_past_c7);
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
    let initramfs = match read_capped(&options.initramfs, ram_bytes) {
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
    let cmdline = cmdline(&options.nonce);
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
    match load_linux_direct_board(&machine_input) {
        Ok(mut boot) => {
            describe(&mut evidence, &boot, &cmdline, &vmlinuz, &initramfs_sha);
            if let Some(budget) = options.budget {
                let result = if options.set_cpl0 {
                    run_linux_direct_board_set(
                        &mut boot,
                        &machine_input,
                        budget,
                        LinuxCpl0SetMembers {
                            // S4-13 diagnostics: `OD_NO_FETCH_CACHE=1` runs without the fetch
                            // translation cache (same results, more page walks).
                            fetch_cache: std::env::var_os("OD_NO_FETCH_CACHE").is_none(),
                            diagnostic_past_c7: options.diagnostic_past_c7,
                            ..LinuxCpl0SetMembers::linux_default()
                        },
                    )
                } else {
                    run_linux_direct_board(&mut boot, &machine_input, budget)
                };
                match result {
                    Ok(record) => {
                        evidence["guest_started"] = json!(true);
                        evidence["retired"] = json!(record.retired);
                        evidence["stop"] = describe_stop(&record, budget);
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
/// present: `acceptance` is `counted` without `--diagnostic-past-c7` and `diagnostic_not_counted`
/// with it (CPUID boundary off, DiagnosticV1 answers leaf 1 and 0x80000001, result NOT counted).
/// The marker follows the request of the flag, not its effect.
fn mark_acceptance(evidence: &mut Value, diagnostic_past_c7: bool) {
    evidence["diagnostic_past_c7"] = json!(diagnostic_past_c7);
    evidence["acceptance"] = json!(if diagnostic_past_c7 {
        "diagnostic_not_counted"
    } else {
        "counted"
    });
    if diagnostic_past_c7 {
        evidence["diagnostic_note"] = json!(
            "DiagnosticV1 answers CPUID leaf 1 and 0x80000001 (checkpoint C7 boundary off); \
             not a profile of the target CPU, the result is NOT counted"
        );
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

/// The kernel command line: the S4-29g template with the operator's nonce, then `nokaslr`.
fn cmdline(nonce: &str) -> String {
    format!("{CMDLINE_PREFIX} od.nonce={nonce} {CMDLINE_SUFFIX}")
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
    initramfs_sha: &str,
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
    evidence["memory_sha256"] = json!({
        "kernel_pm_code": kernel_memory,
        "kernel_pm_code_matches_file": kernel_input.as_deref() == Some(kernel_memory.as_str()),
        "initrd": initrd_memory,
        "initrd_matches_file": initrd_memory.as_deref() == Some(initramfs_sha),
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
    // S4-38L7f: the guest-visible IA32_EFER (`efer` is the walker's); set apart from the
    // literal above to stay under the `json!` macro recursion limit.
    stop["guest_efer"] = json!(hex64(record.guest_efer));
    // S4-38L7w: the guest PCI CONFIG_ADDRESS (port 0xCF8).
    stop["pci_config_address"] = json!(hex64(u64::from(record.pci_config_address)));
    stop
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
    let mut nonce = None;
    let mut expected_vmlinuz = None;
    let mut expected_manifest = None;
    let mut expected_initramfs = None;
    let mut ram_mib = None;
    let mut out = None;
    let mut run = false;
    let mut set_cpl0 = false;
    let mut diagnostic_past_c7 = false;
    let mut budget = None;
    let mut index = 0;
    while index < args.len() {
        let name = args[index].as_str();
        // `--run` is the one flag without a value.
        if name == "--run" {
            if std::mem::replace(&mut run, true) {
                return Err(format!("duplicate option --run; usage: {USAGE}"));
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
        nonce,
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
        set_cpl0,
        diagnostic_past_c7,
    })
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
                set_cpl0: false,
                diagnostic_past_c7: false,
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
        // CLD: outside the base profile, stops before retirement.
        let value = run_case("run-cld", &[0xFC, 0xFA, 0x31, 0xC0], 100);
        assert_eq!(value["guest_started"], true);
        assert_eq!(value["retired"], 0);
        let stop = &value["stop"];
        assert_eq!(stop["exit_kind"], "unsupported");
        assert_eq!(stop["unsupported_opcode"], "0xfc");
        assert_eq!(stop["rip"], "0x1000200");
        assert_eq!(stop["rip_gpa"], "0x1000200");
        assert!(
            stop["rip_bytes"]
                .as_str()
                .unwrap()
                .starts_with("fc fa 31 c0")
        );
        assert_eq!(stop["retired"], 0);
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
        // CLD; CLI; XOR EAX,EAX; MOV DS,AX: with the set the stop moves from CLD past MOV DS.
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
                "msr:EferReadOnlyV1(3328)",
                "msr:fs_gs_base",
                "pio_u8",
                "popf_cpl0",
                "pci_config"
            ])
        );
        assert_eq!(stop["cpuid_divergence_leaf"], Value::Null);
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
        // The same bytes through the default path stop at CLD, with the old schema values.
        let value = run_case("set-cpl0-control", &code, 64);
        assert_eq!(value["stop"]["path"], "paged_probe");
        assert_eq!(value["stop"]["unsupported_opcode"], "0xfc");
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
                "msr:EferReadOnlyV1(3328)",
                "msr:fs_gs_base",
                "pio_u8",
                "popf_cpl0",
                "pci_config"
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
            mark_acceptance(&mut base, flag);
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
            set_cpl0: false,
            diagnostic_past_c7: false,
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
            set_cpl0: false,
            diagnostic_past_c7: false,
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
            set_cpl0: true,
            diagnostic_past_c7: false,
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
                "msr:EferReadOnlyV1(3328)",
                "msr:fs_gs_base",
                "pio_u8",
                "popf_cpl0",
                "pci_config"
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
}
