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
//! This is a static diagnostic: it does not run code, does not decide what executes (the profile
//! set of a run admits a subset of the decoded forms), and a function range may hold data the
//! sweep reads as code. It writes one JSON report to stdout.

use od_isa::length::long64_length;
use od_isa::long_decode::{DecodeError, Opcode, OpcodeMap, decode};
use od_isa::memory::{
    AccessContext, AccessOrigin, GuestMemory, InstructionMode, MemoryError, PrivilegeLevel,
};
use serde_json::{Value, json};
use std::collections::{BTreeMap, HashMap};
use std::process::ExitCode;

const USAGE: &str = "usage: decode-coverage [--cpl 0|3] [--top <n>] <pe32+ image> ...";

struct Image {
    base: u64,
    bytes: Vec<u8>,
    subsystem: u16,
    functions: Vec<(u32, u32)>,
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
    for entry in (pdata_rva..pdata_rva + pdata_size).step_by(12) {
        let (Some(begin), Some(end)) = (u32_at(&bytes, entry), u32_at(&bytes, entry + 4)) else {
            break;
        };
        if begin < end && (end as usize) <= size_of_image {
            functions.push((begin, end));
        }
    }
    functions.sort_unstable();
    functions.dedup();
    Ok(Image {
        base,
        bytes,
        subsystem,
        functions,
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
    if files.is_empty() {
        eprintln!("{USAGE}");
        return ExitCode::from(3);
    }
    let mut gaps: HashMap<String, Gap> = HashMap::new();
    let mut reports = Vec::new();
    let mut mismatches = Vec::new();
    for path in &files {
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
        for &(begin, end) in &image.functions {
            let mut rva = begin;
            while rva < end {
                let at = rva as usize;
                let window = &image.bytes[at..image.bytes.len().min(at + 15)];
                let measured = long64_length(window);
                let rip = image.base.wrapping_add(u64::from(rva));
                let step = match decode(&mut memory, rip, context) {
                    Ok(instruction) => {
                        decoded += 1;
                        let length = usize::from(instruction.length);
                        if measured != Ok(length) && mismatches.len() < 50 {
                            mismatches.push(json!({
                                "file": name, "rva": format!("{rva:#x}"),
                                "bytes": hex(&window[..window.len().min(length.max(1))]),
                                "decoder": length, "measured": format!("{measured:?}"),
                            }));
                        }
                        length
                    }
                    Err(error) => {
                        let Ok(length) = measured else {
                            unmeasured += 1;
                            break;
                        };
                        let kind = match error {
                            DecodeError::Unsupported { .. } => {
                                refused += 1;
                                ""
                            }
                            DecodeError::Memory(_) => {
                                unmeasured += 1;
                                break;
                            }
                            _ => {
                                invalid += 1;
                                "#UD "
                            }
                        };
                        let key = format!("{kind}{}", signature(&window[..length]));
                        let gap = gaps.entry(key).or_default();
                        gap.count += 1;
                        *gap.files.entry(name.clone()).or_default() += 1;
                        gap.example.get_or_insert_with(|| {
                            (name.clone(), u64::from(rva), hex(&window[..length]))
                        });
                        length
                    }
                };
                rva += step.max(1) as u32;
            }
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
    let report = json!({
        "command": "decode-coverage",
        "images": reports,
        "distinct_forms": total_gaps,
        "gaps": gaps,
        "length_mismatches": mismatches,
    });
    println!("{}", serde_json::to_string_pretty(&report).expect("JSON"));
    ExitCode::SUCCESS
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
}
