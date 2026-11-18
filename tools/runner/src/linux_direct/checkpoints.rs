//! `--checkpoint-table` (S4-38L4i): the table of checkpoint addresses from the S4-38L2 oracle and
//! the `checkpoints` key of the evidence.
//!
//! The runner reads `docs/reports/data/stage4/s4-38l2-oracle.json`, checks that its kernel hash is
//! the hash of the vmlinuz that runs, hands the addresses to `od-machine` (which records C0..C17 on
//! the instruction boundaries of the CPL0-set loop, observation only) and writes the result as
//! `checkpoints`. The key exists only with the flag; a checkpoint the run did not reach is listed
//! as `not_reached` with the reason the runner can state, never left out and never filled in.

use od_machine::{
    LINUX_CHECKPOINT_COUNT, LinuxCheckpointRecord, LinuxCheckpointTable, LinuxCheckpointTarget,
    LinuxStopRecord,
};
use od_testkit::sha256_hex;
use serde_json::{Value, json};

use super::{hex, hex64};

/// Stable names of C0..C17 (the symbols are the ones of the oracle table).
pub(super) const CHECKPOINT_NAMES: [&str; LINUX_CHECKPOINT_COUNT] = [
    "entry",
    "relocated_copy_first_fetch",
    "first_lidt",
    "first_cr3_write",
    "first_page_fault",
    "e_entry",
    "cpuid_leaf_0",
    "cpuid1",
    "x86_64_start_kernel",
    "start_kernel",
    "setup_arch",
    "tsc_init",
    "apic_intr_mode_init",
    "check_timer",
    "populate_rootfs",
    "run_init_process",
    "kernel_execve",
    "init_entry",
];

/// The oracle row (or key) behind each address checkpoint: `(id, row name in the oracle)`.
const ORACLE_ROWS: [(u8, &str); 10] = [
    (5, "e_entry"),
    (7, "cpuid1"),
    (8, "x86_64_start_kernel"),
    (9, "start_kernel"),
    (10, "setup_arch"),
    (11, "tsc_init"),
    (12, "apic_intr_mode_init"),
    (13, "check_timer"),
    (14, "populate_rootfs"),
    (15, "run_init_process"),
];
/// `kernel_execve` is C16, counted only after C15 (S4-38L4i: apart from the plan, which keeps it
/// under C17; a usermode helper may call it before `run_init_process`).
const C16: u8 = 16;
const C15: u8 = 15;
const C17: u8 = 17;

/// What the oracle says about one address checkpoint.
#[derive(Clone, Debug)]
struct OracleAddress {
    va: Option<u64>,
    pa: Option<u64>,
    bytes: Option<Vec<u8>>,
}

pub(super) struct CheckpointSetup {
    pub(super) table: LinuxCheckpointTable,
    path: String,
    oracle_sha256: String,
    oracle_vmlinuz_sha256: String,
    /// The `e_entry` of `/init` read from the initramfs (C17), when there is one.
    init_entry: Option<u64>,
    oracle: Vec<Option<OracleAddress>>,
}

/// Why the table is refused: `invalid` (unreadable or malformed, exit 3) or `mismatch` (the oracle
/// is not the one of the vmlinuz that runs, exit 4).
pub(super) enum TableError {
    Invalid(String),
    VmlinuzMismatch(String),
}

/// Reads and checks the oracle. `vmlinuz_sha256` is the hash of the kernel file that runs and
/// `initramfs` the archive whose `/init` entry is C17.
pub(super) fn load(
    path: &str,
    vmlinuz_sha256: &str,
    initramfs: &[u8],
) -> Result<CheckpointSetup, TableError> {
    let bytes = std::fs::read(path)
        .map_err(|error| TableError::Invalid(format!("cannot read {path}: {error}")))?;
    let oracle_sha256 = sha256_hex(&bytes);
    let oracle: Value = serde_json::from_slice(&bytes)
        .map_err(|error| TableError::Invalid(format!("{path} is not JSON: {error}")))?;
    let oracle_vmlinuz = oracle["inputs"]["vmlinuz"]["sha256"]
        .as_str()
        .ok_or_else(|| TableError::Invalid(format!("{path}: inputs.vmlinuz.sha256 is missing")))?
        .to_string();
    if oracle_vmlinuz != vmlinuz_sha256 {
        return Err(TableError::VmlinuzMismatch(format!(
            "the oracle {path} is for the vmlinuz {oracle_vmlinuz}, the run uses {vmlinuz_sha256}"
        )));
    }
    let rows = oracle["checkpoints"]["rows"]
        .as_array()
        .ok_or_else(|| TableError::Invalid(format!("{path}: checkpoints.rows is missing")))?;
    let mut addresses: Vec<Option<OracleAddress>> = vec![None; LINUX_CHECKPOINT_COUNT];
    let mut targets = Vec::new();
    let number = |value: &Value, key: &str| -> Result<u64, TableError> {
        value[key]
            .as_u64()
            .ok_or_else(|| TableError::Invalid(format!("{path}: {key} is missing or not a u64")))
    };
    // C1: the first fetch of the relocated copy (physical, identity-mapped).
    let relocated = oracle["relocated_copy"]["relocated_rip_first_fetch"]
        .as_u64()
        .ok_or_else(|| {
            TableError::Invalid(format!(
                "{path}: relocated_copy.relocated_rip_first_fetch is missing"
            ))
        })?;
    addresses[1] = Some(OracleAddress {
        va: None,
        pa: Some(relocated),
        bytes: None,
    });
    targets.push(target(1, None, Some(relocated), false, None));
    for (id, name) in ORACLE_ROWS {
        let row = rows
            .iter()
            .find(|row| row["name"].as_str() == Some(name))
            .ok_or_else(|| TableError::Invalid(format!("{path}: row {name} is missing")))?;
        let (va, pa) = (number(row, "va")?, number(row, "pa")?);
        addresses[usize::from(id)] = Some(OracleAddress {
            va: Some(va),
            pa: Some(pa),
            bytes: row["bytes_16"].as_str().and_then(parse_hex),
        });
        targets.push(target(id, Some(va), Some(pa), false, None));
    }
    let row = rows
        .iter()
        .find(|row| row["name"].as_str() == Some("kernel_execve"))
        .ok_or_else(|| TableError::Invalid(format!("{path}: row kernel_execve is missing")))?;
    let (va, pa) = (number(row, "va")?, number(row, "pa")?);
    addresses[usize::from(C16)] = Some(OracleAddress {
        va: Some(va),
        pa: Some(pa),
        bytes: row["bytes_16"].as_str().and_then(parse_hex),
    });
    targets.push(target(C16, Some(va), Some(pa), false, Some(C15)));
    let init_entry = init_entry_from_cpio(initramfs);
    if let Some(entry) = init_entry {
        addresses[usize::from(C17)] = Some(OracleAddress {
            va: Some(entry),
            pa: None,
            bytes: None,
        });
        // After C16: a usermode helper with the same e_entry (before `kernel_execve` of init)
        // is not C17.
        targets.push(target(C17, Some(entry), None, true, Some(C16)));
    }
    Ok(CheckpointSetup {
        table: LinuxCheckpointTable { targets },
        path: path.to_string(),
        oracle_sha256,
        oracle_vmlinuz_sha256: oracle_vmlinuz,
        init_entry,
        oracle: addresses,
    })
}

fn target(
    id: u8,
    va: Option<u64>,
    pa: Option<u64>,
    cpl3: bool,
    after: Option<u8>,
) -> LinuxCheckpointTarget {
    LinuxCheckpointTarget {
        id,
        name: CHECKPOINT_NAMES[usize::from(id)].to_string(),
        va,
        pa,
        cpl3,
        after,
    }
}

fn parse_hex(text: &str) -> Option<Vec<u8>> {
    if text.len() % 2 != 0 {
        return None;
    }
    (0..text.len() / 2)
        .map(|index| u8::from_str_radix(text.get(index * 2..index * 2 + 2)?, 16).ok())
        .collect()
}

/// The `e_entry` of the `/init` ELF in a `newc` cpio archive (`070701`), `None` when there is no
/// such file, it is not a little-endian ELF64 of type `ET_EXEC` (a static executable: its
/// `e_entry` is the address of the first user fetch; for `ET_DYN` it would be an offset and the
/// interpreter would run first), or the archive is malformed. Bounded by the archive; nothing is
/// executed.
pub(super) fn init_entry_from_cpio(archive: &[u8]) -> Option<u64> {
    let field = |header: &[u8], index: usize| -> Option<usize> {
        let text = std::str::from_utf8(header.get(6 + 8 * index..14 + 8 * index)?).ok()?;
        usize::from_str_radix(text, 16).ok()
    };
    let mut at = 0usize;
    loop {
        let header = archive.get(at..at.checked_add(110)?)?;
        if &header[..6] != b"070701" {
            return None;
        }
        let file_size = field(header, 6)?;
        let name_size = field(header, 11)?;
        let name_start = at + 110;
        let name = archive.get(name_start..name_start.checked_add(name_size)?)?;
        let name = name.strip_suffix(&[0]).unwrap_or(name);
        let data_start = name_start.checked_add(name_size)?.checked_add(3)? & !3;
        if name == b"TRAILER!!!" {
            return None;
        }
        let data = archive.get(data_start..data_start.checked_add(file_size)?)?;
        if matches!(name, b"init" | b"/init" | b"./init") {
            let elf64_little =
                data.get(..4)? == b"\x7fELF" && *data.get(4)? == 2 && *data.get(5)? == 1;
            const ET_EXEC: u16 = 2;
            let e_type = u16::from_le_bytes(data.get(0x10..0x12)?.try_into().ok()?);
            if !elf64_little || e_type != ET_EXEC {
                return None;
            }
            return Some(u64::from_le_bytes(data.get(0x18..0x20)?.try_into().ok()?));
        }
        at = data_start.checked_add(file_size)?.checked_add(3)? & !3;
    }
}

/// The `checkpoints` key of the evidence: the table, and all 18 checkpoints in id order, each
/// `reached` (with the record) or `not_reached` (with the reason).
pub(super) fn evidence(setup: &CheckpointSetup, stop: &LinuxStopRecord) -> Value {
    let records = stop.checkpoints.as_deref().unwrap_or(&[]);
    let mut entries = Vec::with_capacity(LINUX_CHECKPOINT_COUNT);
    let mut not_reached = Vec::new();
    for (index, name) in CHECKPOINT_NAMES.iter().enumerate() {
        let id = u8::try_from(index).expect("18 checkpoints");
        let label = format!("C{id}");
        let oracle = setup.oracle[index].as_ref().map(|address| {
            json!({
                "va": address.va.map(hex64),
                "pa": address.pa.map(hex64),
                "bytes_16": address.bytes.as_deref().map(hex),
            })
        });
        match records.iter().find(|record| record.id == id) {
            Some(record) => {
                let mut entry = reached(&label, name, record);
                // The first bytes at the physical address against the bytes the oracle read from
                // the same symbol of the vmlinux (only where the oracle has them).
                if let Some(expected) = setup.oracle[index]
                    .as_ref()
                    .and_then(|address| address.bytes.as_deref())
                {
                    entry["bytes_match_oracle"] =
                        json!(record.bytes.get(..expected.len()) == Some(expected));
                }
                entry["oracle"] = oracle.unwrap_or(Value::Null);
                entries.push(entry);
            }
            None => {
                not_reached.push(label.clone());
                entries.push(json!({
                    "id": label,
                    "name": name,
                    "status": "not_reached",
                    "reason": reason(id, setup, records, stop),
                    "oracle": oracle,
                }));
            }
        }
    }
    json!({
        "schema": "s4-38l4i-v1",
        "table": {
            "path": setup.path,
            "sha256": setup.oracle_sha256,
            "vmlinuz_sha256": setup.oracle_vmlinuz_sha256,
            "vmlinuz_sha256_matches_the_run": true,
            "slice": "S4-38L2",
        },
        "init_entry": setup.init_entry.map(hex64),
        "observation_only": true,
        "reached": records.len(),
        "not_reached": not_reached,
        "records": entries,
    })
}

fn reached(label: &str, name: &str, record: &LinuxCheckpointRecord) -> Value {
    json!({
        "id": label,
        "name": name,
        "status": "reached",
        "kind": record.kind,
        "rip": hex64(record.rip),
        "gpa": record.gpa.map(hex64),
        "retired": record.retired,
        "quanta": record.quanta,
        "virtual_ns": record.virtual_ns,
        "cr3": hex64(record.cr3),
        "cpl": record.cpl,
        "bytes": super::hex_spaced(&record.bytes),
    })
}

/// The reason a checkpoint was not reached, as far as the runner can state it from the stop
/// record; never a guess about the kernel.
fn reason(
    id: u8,
    setup: &CheckpointSetup,
    records: &[LinuxCheckpointRecord],
    stop: &LinuxStopRecord,
) -> String {
    let reached = |other: u8| records.iter().any(|record| record.id == other);
    if id == C16 && !reached(C15) {
        return "counted only after C15 (run_init_process): C15 was not reached".to_string();
    }
    if id == C17 {
        if setup.init_entry.is_none() {
            return "the initramfs has no /init that is a static ELF64 (ET_EXEC) whose e_entry \
                    could be read"
                .to_string();
        }
        if stop.exit_kind == "user_entry" {
            return "the run stopped at the user_entry IRETQ (no --user-mode): the CPL3 entry \
                    did not execute"
                .to_string();
        }
        if !reached(C16) {
            return "counted only after C16 (kernel_execve): C16 was not reached".to_string();
        }
    }
    format!(
        "the predicate did not hold before the run ended (exit_kind {}, rip {}, retired {})",
        stop.exit_kind,
        hex64(stop.rip),
        stop.retired
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn newc(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut out = Vec::new();
        let mut push = |name: &str, data: &[u8]| {
            let header = format!(
                "070701{:08x}{:08x}{:08x}{:08x}{:08x}{:08x}{:08x}{:08x}{:08x}{:08x}{:08x}{:08x}{:08x}",
                0,
                0o100755,
                0,
                0,
                1,
                0,
                data.len(),
                0,
                0,
                0,
                0,
                name.len() + 1,
                0
            );
            out.extend_from_slice(header.as_bytes());
            out.extend_from_slice(name.as_bytes());
            out.push(0);
            while out.len() % 4 != 0 {
                out.push(0);
            }
            out.extend_from_slice(data);
            while out.len() % 4 != 0 {
                out.push(0);
            }
        };
        for (name, data) in entries {
            push(name, data);
        }
        push("TRAILER!!!", &[]);
        out
    }

    fn elf(entry: u64) -> Vec<u8> {
        let mut bytes = vec![0u8; 64];
        bytes[..6].copy_from_slice(&[0x7f, b'E', b'L', b'F', 2, 1]);
        bytes[0x10..0x12].copy_from_slice(&2u16.to_le_bytes());
        bytes[0x18..0x20].copy_from_slice(&entry.to_le_bytes());
        bytes
    }

    #[test]
    fn the_init_entry_is_read_from_the_cpio_and_a_foreign_one_is_not_confused() {
        let archive = newc(&[("other", &elf(0x111)), ("init", &elf(0x40_00b0))]);
        assert_eq!(init_entry_from_cpio(&archive), Some(0x40_00b0));
        let archive = newc(&[("init", &elf(0x40_00b1))]);
        assert_eq!(init_entry_from_cpio(&archive), Some(0x40_00b1));
        // No init, not an ELF, a 32-bit ELF, a truncated archive and garbage give nothing.
        assert_eq!(init_entry_from_cpio(&newc(&[("other", &elf(1))])), None);
        assert_eq!(
            init_entry_from_cpio(&newc(&[("init", b"#!/bin/sh\n")])),
            None
        );
        let mut elf32 = elf(5);
        elf32[4] = 1;
        assert_eq!(init_entry_from_cpio(&newc(&[("init", &elf32)])), None);
        // A shared object (ET_DYN) is not a static init: its e_entry is an offset.
        let mut dynamic = elf(0x1000);
        dynamic[0x10..0x12].copy_from_slice(&3u16.to_le_bytes());
        assert_eq!(init_entry_from_cpio(&newc(&[("init", &dynamic)])), None);
        let full = newc(&[("init", &elf(0x40_00b0))]);
        assert_eq!(init_entry_from_cpio(&full[..150]), None);
        assert_eq!(
            init_entry_from_cpio(b"not a cpio archive at all, really....."),
            None
        );
        assert_eq!(init_entry_from_cpio(&[]), None);
    }
}
