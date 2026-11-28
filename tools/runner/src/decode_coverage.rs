//! S7-32: `od-runner decode-coverage` — which instruction forms of a PE32+ x64 image the decoder
//! of `od-isa` does not admit.
//!
//! The image is mapped at its `ImageBase` from the section table; every function range of the
//! exception directory (`.pdata`, `RUNTIME_FUNCTION`: begin, end, unwind RVAs) is walked linearly
//! from its first byte to its end. Each instruction is decoded by `od_isa::long_decode::decode` in a
//! 64-bit context of the image's privilege (CPL0 for native, EFI and boot-application subsystems, CPL3 otherwise, or
//! as `--cpl` says) and measured by `od_isa::length::long64_length`, which also steps over a form the
//! decoder refuses. The default CPL is a heuristic on the subsystem: native-subsystem user programs
//! (`smss.exe`, `csrss.exe`, `autochk.exe`) are scanned at CPL0 unless `--cpl 3` is given; the CPL
//! changes only the admission of privileged forms. A refused form is counted under its signature (prefix class, opcode map and
//! byte, the ModRM group or register form); a decoded instruction whose length differs from the
//! measured one is reported separately (a defect of one of the two).
//!
//! This is a static diagnostic: it does not run guest code, and a function range may hold data the
//! sweep reads as code. Without `--exec` it does not decide what executes (the profile set of a run
//! admits a subset of the decoded forms). With `--exec windows` (S7-32 slice S) every
//! decoded form is also executed one step in a sandbox by the CPL0 set of the unified loop for the
//! Windows guest ([`exec`]): at CPL0 for the CPL0 images, or, with `--cpl 3` (ADR-0023 slice C2a),
//! through the CPL3 entry of the same set for every image; the sweep then also follows the targets of `call rel32` outside
//! `.pdata`, and what it finds there is reported only in the `exec` section (the `images`, `gaps`
//! and `length_mismatches` fields stay those of the `.pdata` sweep). It writes one JSON report to
//! stdout.

mod exec;
mod seh;

use od_isa::length::long64_length;
use od_isa::long_decode::{DecodeError, Opcode, OpcodeMap, Operation, decode};
use od_isa::memory::{
    AccessContext, AccessOrigin, GuestMemory, InstructionMode, MemoryError, PrivilegeLevel,
};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::process::ExitCode;

const USAGE: &str =
    "usage: decode-coverage [--cpl 0|3] [--exec windows] [--top <n>] <pe32+ image> ...";

struct Image {
    base: u64,
    bytes: Vec<u8>,
    subsystem: u16,
    functions: Vec<(u32, u32)>,
    /// `RUNTIME_FUNCTION` entries (begin, end, unwind info RVA), sorted by begin (S7-32 slice T:
    /// the `__try` scopes of `--exec`).
    runtime_functions: Vec<(u32, u32, u32)>,
    /// Export (index 0) and import (index 1) directories: (RVA, size).
    export_directory: (usize, usize),
    import_directory: (usize, usize),
}

struct ImageMemory<'a> {
    base: u64,
    bytes: &'a [u8],
}

impl GuestMemory for ImageMemory<'_> {
    fn fetch(&mut self, address: u64, _context: AccessContext) -> Result<u8, MemoryError> {
        address
            .checked_sub(self.base)
            .and_then(|offset| self.bytes.get(usize::try_from(offset).ok()?))
            .copied()
            .ok_or_else(|| MemoryError::Backend("fetch outside the image".into()))
    }

    fn read(&mut self, _: u64, _: AccessContext, _: &mut [u8]) -> Result<(), MemoryError> {
        Err(MemoryError::Backend("decode-coverage reads no data".into()))
    }

    fn write(&mut self, _: u64, _: AccessContext, _: &[u8]) -> Result<(), MemoryError> {
        Err(MemoryError::Backend(
            "decode-coverage writes no data".into(),
        ))
    }

    fn mfence(&mut self, _: AccessContext) -> Result<(), MemoryError> {
        Err(MemoryError::Backend(
            "decode-coverage executes nothing".into(),
        ))
    }
}

fn u16_at(bytes: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(bytes.get(at..at + 2)?.try_into().ok()?))
}

fn u32_at(bytes: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(bytes.get(at..at + 4)?.try_into().ok()?))
}

fn u64_at(bytes: &[u8], at: usize) -> Option<u64> {
    Some(u64::from_le_bytes(bytes.get(at..at + 8)?.try_into().ok()?))
}

/// Map a PE32+ file at its image base and read its function ranges.
fn load_image(file: &[u8]) -> Result<Image, String> {
    let pe = u32_at(file, 0x3c).ok_or("no DOS header")? as usize;
    if file.get(pe..pe + 4) != Some(b"PE\0\0") {
        return Err("no PE signature".into());
    }
    let coff = pe + 4;
    if u16_at(file, coff) != Some(0x8664) {
        return Err("not an x64 image".into());
    }
    let sections = u16_at(file, coff + 2).ok_or("short COFF header")? as usize;
    let optional_size = u16_at(file, coff + 16).ok_or("short COFF header")? as usize;
    let optional = coff + 20;
    if u16_at(file, optional) != Some(0x20b) {
        return Err("not PE32+".into());
    }
    let base = u64_at(file, optional + 24).ok_or("short optional header")?;
    let size_of_image = u32_at(file, optional + 56).ok_or("short optional header")? as usize;
    let size_of_headers = u32_at(file, optional + 60).ok_or("short optional header")? as usize;
    let subsystem = u16_at(file, optional + 68).ok_or("short optional header")?;
    let directories = u32_at(file, optional + 108).ok_or("short optional header")?;
    let directory = |index: usize| -> (usize, usize) {
        if directories as usize > index {
            (
                u32_at(file, optional + 112 + index * 8).unwrap_or(0) as usize,
                u32_at(file, optional + 112 + index * 8 + 4).unwrap_or(0) as usize,
            )
        } else {
            (0, 0)
        }
    };
    let (export_directory, import_directory) = (directory(0), directory(1));
    let (pdata_rva, pdata_size) = if directories > 3 {
        (
            u32_at(file, optional + 112 + 3 * 8).ok_or("short directories")? as usize,
            u32_at(file, optional + 112 + 3 * 8 + 4).ok_or("short directories")? as usize,
        )
    } else {
        (0, 0)
    };
    if size_of_image > 1 << 30 {
        return Err("SizeOfImage above 1 GiB".into());
    }
    let mut bytes = vec![0; size_of_image];
    let headers = size_of_headers.min(file.len()).min(size_of_image);
    bytes[..headers].copy_from_slice(&file[..headers]);
    let table = optional + optional_size;
    for index in 0..sections {
        let header = table + index * 40;
        let virtual_size = u32_at(file, header + 8).ok_or("short section table")? as usize;
        let rva = u32_at(file, header + 12).ok_or("short section table")? as usize;
        let raw_size = u32_at(file, header + 16).ok_or("short section table")? as usize;
        let raw = u32_at(file, header + 20).ok_or("short section table")? as usize;
        // A truncated file or a section outside SizeOfImage maps nothing (never a panic).
        if raw >= file.len() || rva >= size_of_image {
            continue;
        }
        let length = virtual_size
            .min(raw_size)
            .min(file.len() - raw)
            .min(size_of_image - rva);
        bytes[rva..rva + length].copy_from_slice(&file[raw..raw + length]);
    }
    let mut functions = Vec::new();
    let mut runtime_functions = Vec::new();
    for entry in (pdata_rva..pdata_rva + pdata_size).step_by(12) {
        let (Some(begin), Some(end)) = (u32_at(&bytes, entry), u32_at(&bytes, entry + 4)) else {
            break;
        };
        if begin < end && (end as usize) <= size_of_image {
            functions.push((begin, end));
            runtime_functions.push((begin, end, u32_at(&bytes, entry + 8).unwrap_or(0)));
        }
    }
    runtime_functions.sort_unstable();
    functions.sort_unstable();
    functions.dedup();
    Ok(Image {
        base,
        bytes,
        subsystem,
        functions,
        runtime_functions,
        export_directory,
        import_directory,
    })
}

/// Opcodes whose ModRM.reg selects the instruction (SDM opcode extensions, Table A-6).
fn is_group(opcode: Opcode) -> bool {
    match opcode.map {
        OpcodeMap::Primary => matches!(
            opcode.byte,
            0x80..=0x83 | 0x8f | 0xc0 | 0xc1 | 0xc6 | 0xc7 | 0xd0..=0xd3 | 0xd8..=0xdf | 0xf6
                | 0xf7 | 0xfe | 0xff
        ),
        OpcodeMap::Escape0f => matches!(
            opcode.byte,
            0x00 | 0x01 | 0x0d | 0x18 | 0x71..=0x73 | 0xae | 0xb9 | 0xba | 0xc7
        ),
        _ => false,
    }
}

/// The signature of the form at the start of `bytes`: the prefix class (`LOCK`, `66`, `F2`, `F3`,
/// `REX.W`, a segment override), the opcode and, for ModRM forms, the group and register/memory form.
fn signature(bytes: &[u8]) -> String {
    let mut at = 0;
    let mut prefixes = Vec::new();
    let mut rex_w = false;
    while let Some(&byte) = bytes.get(at) {
        match byte {
            0xf0 => prefixes.push("LOCK"),
            0xf2 => prefixes.push("F2"),
            0xf3 => prefixes.push("F3"),
            0x66 => prefixes.push("66"),
            0x67 => prefixes.push("67"),
            0x64 => prefixes.push("FS"),
            0x65 => prefixes.push("GS"),
            0x2e | 0x36 | 0x3e | 0x26 => prefixes.push("SEG"),
            0x40..=0x4f => rex_w = byte & 8 != 0,
            _ => break,
        }
        at += 1;
    }
    if rex_w {
        prefixes.push("REX.W");
    }
    prefixes.sort_unstable();
    prefixes.dedup();
    let mut text = prefixes.join(" ");
    let push = |text: &mut String, part: &str| {
        if !text.is_empty() {
            text.push(' ');
        }
        text.push_str(part);
    };
    let Some(&first) = bytes.get(at) else {
        return text;
    };
    let (opcode, modrm_at) = match first {
        0xc4 | 0xc5 | 0x62 => {
            push(&mut text, &format!("{first:02X} (VEX/EVEX)"));
            return text;
        }
        0x0f => match bytes.get(at + 1) {
            Some(&0x38) | Some(&0x3a) => {
                let map = bytes[at + 1];
                let byte = bytes.get(at + 2).copied().unwrap_or(0);
                push(&mut text, &format!("0F {map:02X} {byte:02X}"));
                (
                    Opcode {
                        map: if map == 0x38 {
                            OpcodeMap::Escape0f38
                        } else {
                            OpcodeMap::Escape0f3a
                        },
                        byte,
                    },
                    at + 3,
                )
            }
            Some(&byte) => {
                push(&mut text, &format!("0F {byte:02X}"));
                (
                    Opcode {
                        map: OpcodeMap::Escape0f,
                        byte,
                    },
                    at + 2,
                )
            }
            None => return text,
        },
        byte => {
            push(&mut text, &format!("{byte:02X}"));
            (
                Opcode {
                    map: OpcodeMap::Primary,
                    byte,
                },
                at + 1,
            )
        }
    };
    let modrm = od_isa::length::long64_layout(bytes)
        .ok()
        .and_then(|layout| layout.modrm)
        .filter(|&at| at == modrm_at)
        .and_then(|at| bytes.get(at).copied());
    if let Some(modrm) = modrm {
        let register = modrm >> 6 == 3;
        if is_group(opcode) {
            if register && opcode.map == OpcodeMap::Escape0f && matches!(opcode.byte, 0x01 | 0xae) {
                push(&mut text, &format!("{modrm:02X}"));
            } else {
                push(&mut text, &format!("/{}", (modrm >> 3) & 7));
                push(&mut text, if register { "reg" } else { "mem" });
            }
        } else {
            push(&mut text, if register { "reg" } else { "mem" });
        }
    }
    text
}

#[derive(Default)]
struct Gap {
    count: u64,
    files: BTreeMap<String, u64>,
    example: Option<(String, u64, String)>,
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

pub fn run(args: &[String]) -> ExitCode {
    let mut cpl = None;
    let mut exec_set = false;
    let mut top = 200usize;
    let mut files = Vec::new();
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--cpl" => match iter.next().map(String::as_str) {
                Some("0") => cpl = Some(PrivilegeLevel::Ring0),
                Some("3") => cpl = Some(PrivilegeLevel::Ring3),
                _ => {
                    eprintln!("{USAGE}");
                    return ExitCode::from(3);
                }
            },
            "--exec" => match iter.next().map(String::as_str) {
                Some("windows") => exec_set = true,
                _ => {
                    eprintln!("{USAGE}");
                    return ExitCode::from(3);
                }
            },
            "--top" => match iter.next().and_then(|value| value.parse().ok()) {
                Some(value) => top = value,
                None => {
                    eprintln!("{USAGE}");
                    return ExitCode::from(3);
                }
            },
            _ => files.push(arg.clone()),
        }
    }
    // ADR-0023 slice C2a: `--cpl 3 --exec windows` runs the sandbox through the CPL3 entry.
    if files.is_empty() {
        eprintln!("{USAGE}");
        return ExitCode::from(3);
    }
    match coverage(&files, cpl, exec_set, top) {
        Ok(report) => {
            println!("{}", serde_json::to_string_pretty(&report).expect("JSON"));
            ExitCode::SUCCESS
        }
        Err(code) => ExitCode::from(code),
    }
}

/// The report of the command for `files` (the arguments already checked). `Err` is the exit code:
/// 4 when the sandbox state of `--exec` is refused by the set (a defect of the tool).
fn coverage(
    files: &[String],
    cpl: Option<PrivilegeLevel>,
    exec_set: bool,
    top: usize,
) -> Result<Value, u8> {
    let mut exec =
        exec_set.then(|| exec::ExecCoverage::windows(cpl == Some(PrivilegeLevel::Ring3)));
    let mut gaps: HashMap<String, Gap> = HashMap::new();
    let mut reports = Vec::new();
    let mut mismatches = Vec::new();
    for path in files {
        let name = std::path::Path::new(path)
            .file_name()
            .map_or_else(|| path.clone(), |name| name.to_string_lossy().into_owned());
        let image = match std::fs::read(path)
            .map_err(|error| error.to_string())
            .and_then(|file| load_image(&file))
        {
            Ok(image) => image,
            Err(error) => {
                reports.push(json!({"file": name, "error": error}));
                continue;
            }
        };
        // Native (1), EFI (10..=13) and Windows boot application (16) images run at CPL0.
        let privilege = cpl.unwrap_or(if matches!(image.subsystem, 1 | 10..=13 | 16) {
            PrivilegeLevel::Ring0
        } else {
            PrivilegeLevel::Ring3
        });
        let context = AccessContext {
            mode: InstructionMode::Long64,
            privilege,
            origin: AccessOrigin::Explicit,
        };
        let mut memory = ImageMemory {
            base: image.base,
            bytes: &image.bytes,
        };
        let (mut decoded, mut refused, mut invalid, mut unmeasured) = (0u64, 0u64, 0u64, 0u64);
        // The exec sandbox runs the images of its privilege only (CPL0, or CPL3 with `--cpl 3`).
        let mut image_exec = exec.as_mut().filter(|exec| privilege == exec.privilege());
        let inside_pdata = |target: u32| {
            let index = image
                .functions
                .partition_point(|function| function.0 <= target);
            index > 0 && image.functions[index - 1].1 > target
        };
        // `.pdata` ranges first; with `--exec`, then sweeps from the targets of `call rel32` outside
        // `.pdata` (to the first RET, JMP or INT3), which count only in the exec section.
        let seh_scopes = seh::SehScopes::new(&image);
        let mut queue: VecDeque<(u32, u32, bool)> = image
            .functions
            .iter()
            .map(|&(begin, end)| (begin, end, false))
            .collect();
        let mut call_targets = BTreeSet::new();
        let mut followed = BTreeSet::new();
        while let Some((begin, end, extra)) = queue.pop_front() {
            let mut rva = begin;
            let mut last_ecx = None;
            while rva < end {
                let at = rva as usize;
                let window = &image.bytes[at..image.bytes.len().min(at + 15)];
                let measured = long64_length(window);
                let rip = image.base.wrapping_add(u64::from(rva));
                let mut stop = false;
                let step = match decode(&mut memory, rip, context) {
                    Ok(instruction) => {
                        let length = usize::from(instruction.length);
                        if !extra {
                            decoded += 1;
                            if measured != Ok(length) && mismatches.len() < 50 {
                                mismatches.push(json!({
                                    "file": name, "rva": format!("{rva:#x}"),
                                    "bytes": hex(&window[..window.len().min(length.max(1))]),
                                    "decoder": length, "measured": format!("{measured:?}"),
                                }));
                            }
                        }
                        if let Some(exec) = image_exec.as_deref_mut() {
                            let bytes = &window[..length.min(window.len())];
                            // S7-32 slice T: whether an `RDMSR`/`WRMSR` lies inside a `__try` scope
                            // (only for those: the scope walk is not free).
                            let in_try = matches!(
                                instruction.operation,
                                Operation::ReadMsr | Operation::WriteMsr
                            ) && seh_scopes.inside_try(rva);
                            if let Err(error) = exec.observe(
                                &name,
                                rva,
                                window,
                                bytes,
                                &instruction,
                                last_ecx,
                                extra,
                                in_try,
                            ) {
                                eprintln!(
                                    "decode-coverage --exec: the sandbox state is refused: {error:?}"
                                );
                                return Err(4);
                            }
                            if bytes.len() == 5 && bytes[0] == 0xb9 {
                                last_ecx = Some(u32::from_le_bytes([
                                    bytes[1], bytes[2], bytes[3], bytes[4],
                                ]));
                            }
                            if let Operation::Call { displacement } = instruction.operation {
                                let target =
                                    (i64::from(rva) + length as i64 + i64::from(displacement))
                                        as u32;
                                call_targets.insert(target);
                                if (target as usize) < image.bytes.len()
                                    && !inside_pdata(target)
                                    && followed.insert(target)
                                {
                                    queue.push_back((target, image.bytes.len() as u32, true));
                                }
                            }
                            stop = extra
                                && matches!(
                                    instruction.operation,
                                    Operation::Return { .. }
                                        | Operation::Jump { .. }
                                        | Operation::IndirectJump { .. }
                                        | Operation::Int3
                                );
                        }
                        length
                    }
                    Err(error) => {
                        let Ok(length) = measured else {
                            if !extra {
                                unmeasured += 1;
                            }
                            break;
                        };
                        let kind = match error {
                            DecodeError::Unsupported { .. } => "",
                            DecodeError::Memory(_) => {
                                if !extra {
                                    unmeasured += 1;
                                }
                                break;
                            }
                            _ => "#UD ",
                        };
                        let key = format!("{kind}{}", signature(&window[..length]));
                        if extra {
                            if let Some(exec) = image_exec.as_deref_mut() {
                                exec.extra_refusal(key);
                            }
                        } else {
                            if kind.is_empty() {
                                refused += 1;
                            } else {
                                invalid += 1;
                            }
                            let gap = gaps.entry(key).or_default();
                            gap.count += 1;
                            *gap.files.entry(name.clone()).or_default() += 1;
                            gap.example.get_or_insert_with(|| {
                                (name.clone(), u64::from(rva), hex(&window[..length]))
                            });
                        }
                        length
                    }
                };
                rva += step.max(1) as u32;
                if stop {
                    break;
                }
            }
        }
        if let Some(exec) = image_exec {
            exec.call_targets_outside_pdata(
                call_targets
                    .iter()
                    .filter(|target| !inside_pdata(**target))
                    .count() as u64,
            );
        }
        reports.push(json!({
            "file": name,
            "cpl": if privilege == PrivilegeLevel::Ring0 { 0 } else { 3 },
            "functions": image.functions.len(),
            "decoded": decoded,
            "refused": refused,
            "decoded_invalid": invalid,
            "sweeps_stopped_unmeasured": unmeasured,
        }));
    }
    let mut ranked: Vec<(String, Gap)> = gaps.into_iter().collect();
    ranked.sort_by(|a, b| b.1.count.cmp(&a.1.count).then_with(|| a.0.cmp(&b.0)));
    let total_gaps = ranked.len();
    let gaps: Vec<Value> = ranked
        .into_iter()
        .take(top)
        .map(|(key, gap)| {
            let mut files: Vec<(String, u64)> = gap.files.into_iter().collect();
            files.sort_by(|a, b| b.1.cmp(&a.1));
            let (file, rva, bytes) = gap.example.unwrap_or_default();
            json!({
                "form": key,
                "count": gap.count,
                "files": files.len(),
                "top_files": files.iter().take(6).map(|(name, count)| format!("{name}:{count}")).collect::<Vec<_>>(),
                "example": format!("{file}+{rva:#x} {bytes}"),
            })
        })
        .collect();
    let mut report = json!({
        "command": "decode-coverage",
        "images": reports,
        "distinct_forms": total_gaps,
        "gaps": gaps,
        "length_mismatches": mismatches,
    });
    if let Some(exec) = &exec {
        report["exec"] = exec.report(top);
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signatures_name_prefix_class_opcode_and_form() {
        assert_eq!(signature(&[0x0f, 0x01, 0x49, 0x0a]), "0F 01 /1 mem");
        assert_eq!(signature(&[0x0f, 0x01, 0xd0]), "0F 01 D0");
        assert_eq!(signature(&[0x8c, 0x49, 0x16]), "8C mem");
        assert_eq!(
            signature(&[0xf0, 0x48, 0x0f, 0xc7, 0x09]),
            "LOCK REX.W 0F C7 /1 mem"
        );
        assert_eq!(
            signature(&[0x66, 0x0f, 0x38, 0x00, 0xc1]),
            "66 0F 38 00 reg"
        );
        assert_eq!(signature(&[0xc5, 0xf8, 0x77]), "C5 (VEX/EVEX)");
        assert_eq!(signature(&[0xe8, 0, 0, 0, 0]), "E8");
        assert_eq!(signature(&[0x0f, 0xa2]), "0F A2");
    }

    #[test]
    fn a_built_image_maps_sections_and_reads_function_ranges() {
        // A minimal PE32+: headers in the first 0x200 bytes, one section at RVA 0x1000 holding
        // `90 C3` and a .pdata entry (0x1000, 0x1002).
        let mut file = vec![0u8; 0x400];
        file[0x3c] = 0x80;
        file[0x80..0x84].copy_from_slice(b"PE\0\0");
        file[0x84..0x86].copy_from_slice(&0x8664u16.to_le_bytes());
        file[0x86..0x88].copy_from_slice(&1u16.to_le_bytes());
        file[0x94..0x96].copy_from_slice(&240u16.to_le_bytes());
        let optional = 0x98;
        file[optional..optional + 2].copy_from_slice(&0x20bu16.to_le_bytes());
        file[optional + 24..optional + 32].copy_from_slice(&0x1_4000_0000u64.to_le_bytes());
        file[optional + 56..optional + 60].copy_from_slice(&0x2000u32.to_le_bytes());
        file[optional + 60..optional + 64].copy_from_slice(&0x200u32.to_le_bytes());
        file[optional + 68..optional + 70].copy_from_slice(&1u16.to_le_bytes());
        file[optional + 108..optional + 112].copy_from_slice(&16u32.to_le_bytes());
        file[optional + 112 + 24..optional + 112 + 28].copy_from_slice(&0x1010u32.to_le_bytes());
        file[optional + 112 + 28..optional + 112 + 32].copy_from_slice(&12u32.to_le_bytes());
        let section = optional + 240;
        file[section + 8..section + 12].copy_from_slice(&0x100u32.to_le_bytes());
        file[section + 12..section + 16].copy_from_slice(&0x1000u32.to_le_bytes());
        file[section + 16..section + 20].copy_from_slice(&0x200u32.to_le_bytes());
        file[section + 20..section + 24].copy_from_slice(&0x200u32.to_le_bytes());
        file[0x200] = 0x90;
        file[0x201] = 0xc3;
        file[0x210..0x214].copy_from_slice(&0x1000u32.to_le_bytes());
        file[0x214..0x218].copy_from_slice(&0x1002u32.to_le_bytes());
        let image = load_image(&file).unwrap();
        assert_eq!(image.base, 0x1_4000_0000);
        assert_eq!(image.subsystem, 1);
        assert_eq!(image.functions, vec![(0x1000, 0x1002)]);
        assert_eq!(&image.bytes[0x1000..0x1002], &[0x90, 0xc3]);

        // A section whose raw data lies past the end of the file, or whose RVA lies past
        // SizeOfImage, maps nothing instead of panicking.
        let mut truncated = file.clone();
        truncated[section + 20..section + 24].copy_from_slice(&0x1_0000u32.to_le_bytes());
        assert_eq!(load_image(&truncated).unwrap().bytes[0x1000], 0);
        let mut outside = file.clone();
        outside[section + 12..section + 16].copy_from_slice(&0x3000u32.to_le_bytes());
        assert!(load_image(&outside).is_ok());
    }

    /// A PE32+ with one section at RVA 0x1000 (0x1000 bytes) holding `code` at its start and a
    /// `.pdata` table at RVA 0x1800 with `functions`; subsystem native (CPL0).
    fn pe(code: &[(u32, &[u8])], functions: &[(u32, u32)]) -> Vec<u8> {
        let mut file = vec![0u8; 0x1200];
        file[0x3c] = 0x80;
        file[0x80..0x84].copy_from_slice(b"PE\0\0");
        file[0x84..0x86].copy_from_slice(&0x8664u16.to_le_bytes());
        file[0x86..0x88].copy_from_slice(&1u16.to_le_bytes());
        file[0x94..0x96].copy_from_slice(&240u16.to_le_bytes());
        let optional = 0x98;
        file[optional..optional + 2].copy_from_slice(&0x20bu16.to_le_bytes());
        file[optional + 24..optional + 32].copy_from_slice(&0x1_4000_0000u64.to_le_bytes());
        file[optional + 56..optional + 60].copy_from_slice(&0x2000u32.to_le_bytes());
        file[optional + 60..optional + 64].copy_from_slice(&0x200u32.to_le_bytes());
        file[optional + 68..optional + 70].copy_from_slice(&1u16.to_le_bytes());
        file[optional + 108..optional + 112].copy_from_slice(&16u32.to_le_bytes());
        file[optional + 112 + 24..optional + 112 + 28].copy_from_slice(&0x1800u32.to_le_bytes());
        let pdata_size = (functions.len() * 12) as u32;
        file[optional + 112 + 28..optional + 112 + 32].copy_from_slice(&pdata_size.to_le_bytes());
        let section = optional + 240;
        file[section + 8..section + 12].copy_from_slice(&0x1000u32.to_le_bytes());
        file[section + 12..section + 16].copy_from_slice(&0x1000u32.to_le_bytes());
        file[section + 16..section + 20].copy_from_slice(&0x1000u32.to_le_bytes());
        file[section + 20..section + 24].copy_from_slice(&0x200u32.to_le_bytes());
        for (rva, bytes) in code {
            let at = 0x200 + (*rva as usize - 0x1000);
            file[at..at + bytes.len()].copy_from_slice(bytes);
        }
        for (index, (begin, end)) in functions.iter().enumerate() {
            let at = 0x200 + 0x800 + index * 12;
            file[at..at + 4].copy_from_slice(&begin.to_le_bytes());
            file[at + 4..at + 8].copy_from_slice(&end.to_le_bytes());
        }
        file
    }

    fn exec_report(name: &str, file: &[u8]) -> Value {
        exec_report_at(name, file, None)
    }

    /// The report of `--exec windows` for one image, at the CPL `cpl` gives (`None`: the default).
    fn exec_report_at(name: &str, file: &[u8], cpl: Option<PrivilegeLevel>) -> Value {
        let path = std::env::temp_dir().join(format!(
            "od-decode-coverage-{}-{name}.sys",
            std::process::id()
        ));
        std::fs::write(&path, file).unwrap();
        let report = coverage(&[path.to_string_lossy().into_owned()], cpl, true, 50);
        std::fs::remove_file(&path).unwrap();
        report.unwrap()
    }

    fn rows<'a>(report: &'a Value, list: &str) -> &'a Vec<Value> {
        report["exec"][list].as_array().unwrap()
    }

    #[test]
    fn exec_reports_a_form_the_set_refuses_and_the_forms_it_runs() {
        // S7-32 slice S: `67 F2 A6` (REPNE CMPSB, address size 32) decodes and the CPL0 set never
        // runs it; NOP and RET run; `F3 6C` (REP INSB) is a decoder refusal and stays in `gaps`.
        let code: &[u8] = &[0x67, 0xf2, 0xa6, 0x90, 0xf3, 0x6c, 0xc3];
        let report = exec_report("refuse", &pe(&[(0x1000, code)], &[(0x1000, 0x1007)]));
        assert_eq!(report["exec"]["verdicts"]["never"], 1, "{report}");
        assert_eq!(report["exec"]["verdicts"]["exec"], 2, "{report}");
        let never = &rows(&report, "families")[0];
        assert_eq!(never["verdict"], "never");
        assert_eq!(never["operation"], "RepneCmpsbAddr32");
        assert_eq!(rows(&report, "forms")[0]["form"], "67 F2 A6");
        assert_eq!(report["gaps"][0]["form"], "F3 6C");
        assert_eq!(report["images"][0]["decoded"], 3);
        assert_eq!(report["images"][0]["refused"], 1);
    }

    #[test]
    fn exec_follows_a_call_target_outside_pdata_only_in_the_exec_section() {
        // `call 0x1100; ret` in `.pdata`; at 0x1100, outside it, `67 F2 A6; F3 6C; ret`.
        let call: &[u8] = &[0xe8, 0xfb, 0x00, 0x00, 0x00, 0xc3];
        let target: &[u8] = &[0x67, 0xf2, 0xa6, 0xf3, 0x6c, 0xc3];
        let file = pe(&[(0x1000, call), (0x1100, target)], &[(0x1000, 0x1006)]);
        let report = exec_report("follow", &file);
        assert_eq!(report["images"][0]["decoded"], 2, "{report}");
        assert_eq!(report["images"][0]["refused"], 0);
        assert!(report["gaps"].as_array().unwrap().is_empty());
        assert_eq!(report["exec"]["extra_instructions"], 2);
        assert_eq!(report["exec"]["call_targets_outside_pdata"], 1);
        assert_eq!(report["exec"]["verdicts"]["never"], 1);
        assert_eq!(rows(&report, "extra_decoder_refusals")[0]["form"], "F3 6C");
    }

    /// S7-32 slice T: `msr_indexes` counts the `RDMSR`/`WRMSR` inside a `__try` scope only under
    /// `__C_specific_handler` (named through the `jmp [IAT]` stub of a driver), not under
    /// `__GSHandlerCheck` (no scope table) and not in a `__finally` scope (`JumpTarget` 0); and the
    /// decision of each (index, direction) is the set's own: `60DH` is modelled, `3FAH` is #GP(0).
    #[test]
    fn exec_counts_msr_accesses_inside_try_only_under_the_c_specific_handler() {
        let le = |value: u32| value.to_le_bytes();
        let unwind = |handler: u32, scopes: &[(u32, u32, u32, u32)]| -> Vec<u8> {
            let mut info = vec![0x09, 0, 0, 0]; // version 1, UNW_FLAG_EHANDLER, no codes
            info.extend(le(handler));
            info.extend(le(scopes.len() as u32));
            for &(begin, end, filter, target) in scopes {
                for value in [begin, end, filter, target] {
                    info.extend(le(value));
                }
            }
            info
        };
        let mut pdata = Vec::new();
        for (begin, end, info) in [
            (0x1000u32, 0x1008u32, 0x1100u32),
            (0x1010, 0x1018, 0x1140),
            (0x1020, 0x1028, 0x1180),
            (0x1030, 0x1038, 0x11c0),
        ] {
            for value in [begin, end, info] {
                pdata.extend(le(value));
            }
        }
        let mut import = Vec::new();
        for value in [0x1300u32, 0, 0, 0x1380, 0x1340, 0, 0, 0, 0, 0] {
            import.extend(le(value));
        }
        let thunks: Vec<u8> = [0x1400u64, 0x1420, 0]
            .iter()
            .flat_map(|v| v.to_le_bytes())
            .collect();
        let f1: &[u8] = &[0xb9, 0x0d, 0x06, 0x00, 0x00, 0x0f, 0x32, 0xc3]; // mov ecx, 0x60d; rdmsr; ret
        let f2: &[u8] = &[0xb9, 0xfa, 0x03, 0x00, 0x00, 0x0f, 0x32, 0xc3]; // mov ecx, 0x3fa; rdmsr; ret
        let f3: &[u8] = &[0xb9, 0x11, 0x06, 0x00, 0x00, 0x0f, 0x32, 0xc3]; // mov ecx, 0x611; rdmsr; ret
        let stub_c = [0xff, 0x25, 0xfa, 0x02, 0x00, 0x00]; // jmp [0x1340]
        let stub_gs = [0xff, 0x25, 0xf2, 0x02, 0x00, 0x00]; // jmp [0x1348]
        // A static `__GSHandlerCheck_SEH` without a name: it calls the `__C_specific_handler` stub.
        let static_seh = [0xe8, 0xdb, 0xff, 0xff, 0xff, 0xc3]; // call 0x1040; ret
        let f4: &[u8] = &[0xb9, 0x06, 0x06, 0x00, 0x00, 0x0f, 0x32, 0xc3]; // mov ecx, 0x606; rdmsr; ret
        let u4 = unwind(0x1060, &[(0x1035, 0x1037, 1, 0x1037)]);
        let u1 = unwind(0x1040, &[(0x1005, 0x1007, 1, 0x1007)]);
        // Under `__GSHandlerCheck` the bytes after the handler are GS data, not a scope table, even
        // when they would read as a scope that covers the `rdmsr`.
        let u2 = unwind(0x1050, &[(0x1015, 0x1017, 1, 0x1017)]);
        let u3 = unwind(0x1040, &[(0x1025, 0x1027, 0x1060, 0)]);
        let c_name = b"\0\0__C_specific_handler\0";
        let gs_name = b"\0\0__GSHandlerCheck\0";
        let code: Vec<(u32, &[u8])> = vec![
            (0x1000, f1),
            (0x1010, f2),
            (0x1020, f3),
            (0x1030, f4),
            (0x1040, &stub_c),
            (0x1050, &stub_gs),
            (0x1100, &u1),
            (0x1140, &u2),
            (0x1180, &u3),
            (0x11c0, &u4),
            (0x1060, &static_seh),
            (0x1200, &import),
            (0x1300, &thunks),
            (0x1340, &thunks),
            (0x1380, b"ntoskrnl.exe\0"),
            (0x1400, c_name),
            (0x1420, gs_name),
        ];
        let mut file = pe(&code, &[]);
        // The .pdata table at 0x1800 (four entries) and the import directory (index 1).
        let optional = 0x98;
        file[0x200 + 0x800..0x200 + 0x800 + pdata.len()].copy_from_slice(&pdata);
        file[optional + 112 + 28..optional + 112 + 32]
            .copy_from_slice(&(pdata.len() as u32).to_le_bytes());
        file[optional + 112 + 8..optional + 112 + 12].copy_from_slice(&0x1200u32.to_le_bytes());
        file[optional + 112 + 12..optional + 112 + 16].copy_from_slice(&40u32.to_le_bytes());
        let report = exec_report("seh", &file);
        let msr = |index: &str| -> Value {
            rows(&report, "msr_indexes")
                .iter()
                .find(|row| row["index"] == index)
                .cloned()
                .unwrap_or_else(|| panic!("{index}: {report}"))
        };
        assert_eq!(msr("0x60d")["in_try"], 1, "{report}");
        assert_eq!(msr("0x60d")["decision"], "model");
        assert_eq!(
            msr("0x3fa")["in_try"],
            0,
            "__GSHandlerCheck has no scope table"
        );
        assert_eq!(msr("0x3fa")["decision"], "gp");
        assert_eq!(
            msr("0x611")["in_try"],
            0,
            "a __finally scope is not a __try"
        );
        assert_eq!(msr("0x606")["in_try"], 1, "a static __GSHandlerCheck_SEH");
    }

    /// S7-32 slice T: the scope walk follows chained unwind info to the primary entry, places the
    /// language-specific data after an odd number of unwind codes rounded up to even, and names a
    /// static handler only by what its straight-line body reaches: a `call`/tail `jmp` to
    /// `__C_specific_handler` (direct or through the import) counts; a call after `ret`, `ret n`,
    /// `rep ret`, `int3` or an unconditional `jmp` belongs to another function and does not.
    #[test]
    fn exec_in_try_follows_chains_odd_codes_and_the_static_handler_body() {
        // Layout: functions 0x1000.., the stub 0x1180, handlers 0x1200.., .pdata 0x1800,
        // unwind info 0x1900.. (0x40 apart: a chained record takes 0x34), imports 0x1e00..
        const C_STUB: u32 = 0x1180;
        const C_SLOT: u32 = 0x1e40;
        let le = |value: u32| value.to_le_bytes();
        let function = |k: u32| 0x1000 + 0x10 * k;
        let handler_at = |k: u32| 0x1200 + 0x20 * k;
        let unwind_at = |k: u32| 0x1900 + 0x40 * k;
        let rel32 = |from_next: u32, to: u32| (to as i64 - from_next as i64) as i32;
        // EHANDLER unwind info with `codes` unwind codes, then the handler and one scope over the
        // `rdmsr` of function `k`.
        let unwind = |k: u32, codes: u8, handler: u32| -> Vec<u8> {
            let mut info = vec![0x09, 2 * codes, codes, 0];
            info.extend(vec![0x11; 2 * usize::from(codes)]);
            if codes % 2 == 1 {
                info.extend([0, 0]); // the padding slot
            }
            info.extend(le(handler));
            info.extend(le(1));
            for value in [function(k) + 5, function(k) + 7, 1, function(k) + 7] {
                info.extend(le(value));
            }
            info
        };
        let call_c = |at: u32| -> Vec<u8> {
            let mut code = vec![0xe8];
            code.extend(rel32(at + 5, C_STUB).to_le_bytes());
            code
        };
        let mut code: Vec<(u32, Vec<u8>)> = Vec::new();
        let mut pdata = Vec::new();
        let mut expected = Vec::new();
        let mut stub = vec![0xff, 0x25];
        stub.extend(rel32(C_STUB + 6, C_SLOT).to_le_bytes());
        code.push((C_STUB, stub));
        // (static handler body or None for the named stub, unwind codes, chained, expected)
        let mut handlers: Vec<(Option<Vec<u8>>, u8, bool, u64)> = vec![
            (None, 0, true, 1), // chained to a primary entry under __C_specific_handler
            (None, 1, false, 1),
            (None, 3, false, 1),
        ];
        for (prefix, counts) in [
            (vec![0xc3], 0u64),          // ret, then a call of the next function
            (vec![0xc2, 0x00, 0x00], 0), // ret 0
            (vec![0xf3, 0xc3], 0),       // rep ret
            (vec![0xeb, 0x05], 0),       // jmp over the call
            (vec![0xcc], 0),             // int3
            (vec![0xff, 0xe0], 0),       // jmp rax
            (vec![0x48, 0x89, 0xc8], 1), // mov rax, rcx; call __C_specific_handler
            (vec![0x48, 0xff, 0xe0], 0), // rex.w jmp rax
            (vec![0x3e, 0xff, 0xe0], 0), // notrack jmp rax
            (vec![0xff, 0x28], 0),       // jmp far [rax]
        ] {
            handlers.push((Some(prefix), 0, false, counts));
        }
        // Tail jumps and an indirect call through the import.
        handlers.push((Some(vec![0xe9]), 0, false, 1)); // jmp rel32 __C_specific_handler
        handlers.push((Some(vec![0x48, 0xff, 0x25]), 0, false, 1)); // jmp [rip+slot]
        handlers.push((Some(vec![0xff, 0x15]), 0, false, 1)); // call [rip+slot]
        // mov rax, rcx; rex.w jmp [rip+slot]: the tail jump after a prologue.
        handlers.push((Some(vec![0x48, 0x89, 0xc8, 0x48, 0xff, 0x25]), 0, false, 1));
        // call [rbp+disp32] with the displacement of the slot: not RIP-relative, not the import.
        handlers.push((Some(vec![0xff, 0x95]), 0, false, 0));
        for (k, (body, codes, chained, counts)) in handlers.into_iter().enumerate() {
            let k = k as u32;
            let index = 0x701 + k;
            let mut f = vec![0xb9];
            f.extend(le(index));
            f.extend([0x0f, 0x32, 0xc3]);
            code.push((function(k), f));
            let handler = match body {
                None => C_STUB,
                Some(prefix) => {
                    let at = handler_at(k);
                    let mut h = prefix.clone();
                    let len = h.len() as u32;
                    match prefix.as_slice() {
                        [0xe9] => h.extend(rel32(at + 5, C_STUB).to_le_bytes()),
                        [.., 0xff, 0x25] => h.extend(rel32(at + len + 4, C_SLOT).to_le_bytes()),
                        [0xff, 0x15] | [0xff, 0x95] => {
                            h.extend(rel32(at + 6, C_SLOT).to_le_bytes());
                            h.push(0xc3);
                        }
                        _ => {
                            h.extend(call_c(at + len));
                            h.push(0xc3);
                        }
                    }
                    code.push((at, h));
                    at
                }
            };
            let info = if chained {
                // CHAININFO (flags 4), no codes, then the primary RUNTIME_FUNCTION.
                let primary = unwind_at(k) + 0x18;
                let mut info = vec![0x21, 0, 0, 0];
                for value in [function(k), function(k) + 8, primary] {
                    info.extend(le(value));
                }
                info.resize(0x18, 0);
                info.extend(unwind(k, 0, handler));
                info
            } else {
                unwind(k, codes, handler)
            };
            code.push((unwind_at(k), info));
            for value in [function(k), function(k) + 8, unwind_at(k)] {
                pdata.extend(le(value));
            }
            expected.push((format!("{index:#x}"), counts));
        }
        let mut import = Vec::new();
        for value in [C_SLOT, 0, 0, 0x1e80, C_SLOT, 0, 0, 0, 0, 0] {
            import.extend(le(value));
        }
        let thunks: Vec<u8> = [0x1ea0u64, 0]
            .iter()
            .flat_map(|v| v.to_le_bytes())
            .collect();
        code.push((0x1e00, import));
        code.push((C_SLOT, thunks));
        code.push((0x1e80, b"ntoskrnl.exe\0".to_vec()));
        code.push((0x1ea0, b"\0\0__C_specific_handler\0".to_vec()));
        let slices: Vec<(u32, &[u8])> = code.iter().map(|(at, b)| (*at, b.as_slice())).collect();
        let mut file = pe(&slices, &[]);
        let optional = 0x98;
        file[0x200 + 0x800..0x200 + 0x800 + pdata.len()].copy_from_slice(&pdata);
        file[optional + 112 + 28..optional + 112 + 32]
            .copy_from_slice(&(pdata.len() as u32).to_le_bytes());
        file[optional + 112 + 8..optional + 112 + 12].copy_from_slice(&0x1e00u32.to_le_bytes());
        file[optional + 112 + 12..optional + 112 + 16].copy_from_slice(&40u32.to_le_bytes());
        // No two pieces overlap (a record that overwrote its neighbour would pass unnoticed).
        let mut spans: Vec<(u32, u32)> = code
            .iter()
            .map(|(at, b)| (*at, *at + b.len() as u32))
            .collect();
        spans.push((0x1800, 0x1800 + pdata.len() as u32));
        spans.sort_unstable();
        assert!(
            spans.windows(2).all(|pair| pair[0].1 <= pair[1].0),
            "{spans:x?}"
        );
        let report = exec_report("seh-chain", &file);
        for (index, counts) in expected {
            let row = rows(&report, "msr_indexes")
                .iter()
                .find(|row| row["index"] == index.as_str())
                .cloned()
                .unwrap_or_else(|| panic!("{index}: {report}"));
            assert_eq!(row["in_try"], counts, "{index}");
        }
    }

    /// S7-32 slice T: `decision` is the set's own outcome: `stop` for an index the set refuses
    /// (`WRMSR 0x10`, a write of the TSC, rule M1), `model` for a modelled read, `gp` for an absent
    /// MSR.
    #[test]
    fn exec_msr_decision_is_stop_model_or_gp() {
        // mov ecx, 0x10; wrmsr; mov ecx, 0x60d; rdmsr; mov ecx, 0x3fa; rdmsr; ret
        let code: &[u8] = &[
            0xb9, 0x10, 0, 0, 0, 0x0f, 0x30, 0xb9, 0x0d, 0x06, 0, 0, 0x0f, 0x32, 0xb9, 0xfa, 0x03,
            0, 0, 0x0f, 0x32, 0xc3,
        ];
        let report = exec_report("decision", &pe(&[(0x1000, code)], &[(0x1000, 0x1016)]));
        let decision = |op: &str, index: &str| -> Value {
            rows(&report, "msr_indexes")
                .iter()
                .find(|row| row["op"] == op && row["index"] == index)
                .unwrap_or_else(|| panic!("{op} {index}: {report}"))["decision"]
                .clone()
        };
        assert_eq!(decision("wrmsr", "0x10"), "stop");
        assert_eq!(decision("rdmsr", "0x60d"), "model");
        assert_eq!(decision("rdmsr", "0x3fa"), "gp");
    }

    #[test]
    fn exec_takes_only_the_windows_set_at_cpl0_or_cpl3() {
        let args = |list: &[&str]| list.iter().map(|arg| (*arg).to_owned()).collect::<Vec<_>>();
        let code = |list: &[&str]| format!("{:?}", run(&args(list)));
        let usage = format!("{:?}", ExitCode::from(3));
        assert_eq!(code(&["--exec", "linux", "x.sys"]), usage);
        assert_eq!(code(&["--exec"]), usage);
        assert_eq!(code(&["--cpl", "2", "--exec", "windows", "x.sys"]), usage);
        // ADR-0023 slice C2a: CPL3 runs the sandbox through the CPL3 entry (a missing file is a
        // report row, not a usage error).
        let missing = std::env::temp_dir().join("od-decode-coverage-missing-cpl3.dll");
        let missing = missing.to_string_lossy().into_owned();
        assert_eq!(
            code(&["--cpl", "3", "--exec", "windows", &missing]),
            format!("{:?}", ExitCode::SUCCESS)
        );
    }

    #[test]
    fn exec_cpl3_reports_the_verdict_of_the_cpl3_entry() {
        // ADR-0023 slice C2a: `mov rbx, rcx` (`48 8b d9`, the stop of ISSUE-719 at
        // `ntdll!LdrInitializeThunk`) and `mov rax, cr0` (`0f 20 c0`, class `Privileged`: #GP(0)
        // is an architectural outcome, `exec`) through the CPL3 entry; `ret` ends the range.
        let code: &[u8] = &[0x48, 0x8b, 0xd9, 0x0f, 0x20, 0xc0, 0xc3];
        let report = exec_report_at(
            "cpl3",
            &pe(&[(0x1000, code)], &[(0x1000, 0x1007)]),
            Some(PrivilegeLevel::Ring3),
        );
        assert_eq!(report["exec"]["cpl"], 3, "{report}");
        assert_eq!(report["images"][0]["cpl"], 3, "{report}");
        assert_eq!(report["exec"]["decoded"], 3, "{report}");
        let family = |operation: &str| {
            rows(&report, "families")
                .iter()
                .find(|row| row["operation"] == operation)
                .map(|row| row["verdict"].as_str().unwrap().to_owned())
        };
        // Slice C1 refused `48 8b d9` (the allow-list, ISSUE-719: `never`); since C2a-4 the class
        // `Neutral` runs every form the set runs at CPL0.
        assert_eq!(family("Mov"), None, "{report}");
        assert_eq!(report["exec"]["verdicts"]["exec"], 3, "{report}");
        let gp = rows(&report, "exec_single_fault")
            .iter()
            .find(|row| row["vector"] == 13)
            .unwrap_or_else(|| panic!("no #GP(0) row: {report}"));
        assert_eq!(gp["form"], signature(&[0x0f, 0x20, 0xc0]), "{report}");
    }
}
