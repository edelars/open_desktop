//! S7-32 slice T: whether an RVA of a PE32+ x64 image lies inside a `__try` scope of its function
//! (the `__try`/`__except` the MSVC compiler records in the language-specific data of the
//! function's UNWIND_INFO).
//!
//! The function is found in `.pdata` (`RUNTIME_FUNCTION`); chained unwind info
//! (`UNW_FLAG_CHAININFO`) is followed to the primary entry. A handler (`UNW_FLAG_EHANDLER` or
//! `UNW_FLAG_UHANDLER`) counts only when it is `__C_specific_handler` or `__GSHandlerCheck_SEH`:
//! their language-specific data begins with a C_SCOPE_TABLE (a count, then `{BeginAddress,
//! EndAddress, HandlerAddress, JumpTarget}`; `__GSHandlerCheck_SEH` puts the GS data after the
//! table). `__GSHandlerCheck` alone has no scope table (the GS data follows the handler RVA), so a
//! function with it has no `__try`. A scope with `JumpTarget` 0 is a `__finally`, not a `__try`.
//! The handler is named by the export directory (the kernel exports `__C_specific_handler`) or, in
//! a driver, by the import its `jmp [rip+disp32]` stub jumps through; a handler linked into the
//! image without a name counts as `__GSHandlerCheck_SEH` when its straight-line body (up to its
//! `ret` or unconditional `jmp`) calls or tail-jumps to `__C_specific_handler`. A handler that
//! cannot be named counts as no `__try` (the count is a lower bound).

use super::{Image, u16_at, u32_at};
use std::collections::HashMap;

const UNW_FLAG_EHANDLER: u8 = 1;
const UNW_FLAG_UHANDLER: u8 = 2;
const UNW_FLAG_CHAININFO: u8 = 4;

pub(super) struct SehScopes<'a> {
    image: &'a Image,
    exports: HashMap<u32, String>,
    imports: HashMap<u32, String>,
}

fn c_string(bytes: &[u8], at: usize) -> Option<String> {
    let tail = bytes.get(at..)?;
    let end = tail.iter().position(|&byte| byte == 0)?;
    Some(String::from_utf8_lossy(&tail[..end]).into_owned())
}

impl<'a> SehScopes<'a> {
    pub(super) fn new(image: &'a Image) -> Self {
        let bytes = &image.bytes;
        let mut exports = HashMap::new();
        let (export_rva, export_size) = image.export_directory;
        if export_size != 0 {
            let names = u32_at(bytes, export_rva + 24).unwrap_or(0) as usize;
            let functions = u32_at(bytes, export_rva + 28).unwrap_or(0) as usize;
            let name_table = u32_at(bytes, export_rva + 32).unwrap_or(0) as usize;
            let ordinals = u32_at(bytes, export_rva + 36).unwrap_or(0) as usize;
            for index in 0..names.min(1 << 16) {
                let (Some(name), Some(ordinal)) = (
                    u32_at(bytes, name_table + 4 * index),
                    u16_at(bytes, ordinals + 2 * index),
                ) else {
                    break;
                };
                let (Some(function), Some(name)) = (
                    u32_at(bytes, functions + 4 * usize::from(ordinal)),
                    c_string(bytes, name as usize),
                ) else {
                    continue;
                };
                exports.insert(function, name);
            }
        }
        let mut imports = HashMap::new();
        let (import_rva, import_size) = image.import_directory;
        if import_size != 0 {
            for descriptor in (import_rva..).step_by(20).take(4096) {
                let (Some(lookup), Some(name), Some(first_thunk)) = (
                    u32_at(bytes, descriptor),
                    u32_at(bytes, descriptor + 12),
                    u32_at(bytes, descriptor + 16),
                ) else {
                    break;
                };
                if name == 0 && first_thunk == 0 {
                    break;
                }
                let table = if lookup != 0 { lookup } else { first_thunk } as usize;
                for slot in 0..4096usize {
                    let Some(entry) = super::u64_at(bytes, table + 8 * slot) else {
                        break;
                    };
                    if entry == 0 {
                        break;
                    }
                    if entry >> 63 != 0 {
                        continue; // by ordinal: no name
                    }
                    if let Some(symbol) = c_string(bytes, (entry & 0x7fff_ffff) as usize + 2) {
                        imports.insert(first_thunk + 8 * slot as u32, symbol);
                    }
                }
            }
        }
        Self {
            image,
            exports,
            imports,
        }
    }

    /// The name of the function at `rva`: an export, or the import a `jmp [rip+disp32]` stub at
    /// `rva` goes through.
    fn handler_name(&self, rva: u32) -> Option<&str> {
        if let Some(name) = self.exports.get(&rva) {
            return Some(name);
        }
        let bytes = &self.image.bytes;
        let at = rva as usize;
        let (opcode_at, length) = match bytes.get(at..at + 3)? {
            [0xff, 0x25, _] => (at, 6),
            [0x48, 0xff, 0x25] => (at + 1, 7),
            _ => return None,
        };
        let displacement =
            i32::from_le_bytes(bytes.get(opcode_at + 2..opcode_at + 6)?.try_into().ok()?);
        let slot = (i64::from(rva) + length + i64::from(displacement)) as u32;
        self.imports.get(&slot).map(String::as_str)
    }

    /// The import a `[rip+disp32]` operand names: `next` is the RVA after the instruction.
    fn import_at(&self, next: usize, displacement: &[u8]) -> Option<&str> {
        let displacement = i32::from_le_bytes(displacement.get(..4)?.try_into().ok()?);
        let slot = (next as i64 + i64::from(displacement)) as u32;
        self.imports.get(&slot).map(String::as_str)
    }

    /// The handler named as the scope walk needs it: its export or import name, or, for an unnamed
    /// handler linked into the image (a driver's static `__GSHandlerCheck_SEH`), the name
    /// `__GSHandlerCheck_SEH` when its body reaches `__C_specific_handler` — what
    /// `__GSHandlerCheck_SEH` does after the GS check, and `__GSHandlerCheck` does not. The body is
    /// scanned linearly (at most 64 instructions) for `call rel32` to a function named
    /// `__C_specific_handler`, `call [rip+disp32]` through its import, or the same as a tail jump
    /// (`jmp rel32`, `jmp [rip+disp32]`); the scan ends at `ret` (`C3`, `C2 iw`, `F3 C3`), `int3`
    /// and any other unconditional `jmp` (`EB`, `E9`, `FF /4` also with REX or `notrack`, `FF /5`),
    /// so it never runs into the next
    /// function. Branches are not followed: a call behind a conditional jump counts only if it lies
    /// on the straight line (a lower bound otherwise).
    fn handler_kind(&self, rva: u32) -> Option<&str> {
        if let Some(name) = self.handler_name(rva) {
            return Some(name);
        }
        const SEH: &str = "__GSHandlerCheck_SEH";
        let is_c = |name: Option<&str>| name == Some("__C_specific_handler");
        let bytes = &self.image.bytes;
        let mut at = rva as usize;
        for _ in 0..64 {
            let window = bytes.get(at..bytes.len().min(at + 15))?;
            let length = od_isa::length::long64_length(window).ok()?;
            let next = at + length;
            // A REX or `notrack` (`3E`) prefix before `FF` (`48 FF 25`, `3E FF E0`).
            let opcode_at = window
                .iter()
                .take(3)
                .take_while(|&&byte| matches!(byte, 0x3e | 0x40..=0x4f))
                .count();
            match (window.first()?, window.get(1)) {
                (0xc3 | 0xc2 | 0xcc, _) | (0xf3, Some(0xc3)) => return None,
                (0xe8 | 0xe9, _) if length == 5 => {
                    let displacement = i32::from_le_bytes(window.get(1..5)?.try_into().ok()?);
                    let target = (next as i64 + i64::from(displacement)) as u32;
                    if is_c(self.handler_name(target)) {
                        return Some(SEH);
                    }
                    if window[0] == 0xe9 {
                        return None;
                    }
                }
                (0xeb, _) => return None,
                _ if window.get(opcode_at) == Some(&0xff) => {
                    let modrm = *window.get(opcode_at + 1)?;
                    let reg = (modrm >> 3) & 7;
                    let rip_relative = modrm & 0xc7 == 0x05;
                    if matches!(reg, 2 | 4)
                        && rip_relative
                        && is_c(self.import_at(next, window.get(opcode_at + 2..)?))
                    {
                        return Some(SEH);
                    }
                    // `jmp r/m` (`/4`) and `jmp far m` (`/5`) end the straight line.
                    if matches!(reg, 4 | 5) {
                        return None;
                    }
                }
                _ => {}
            }
            at = next;
        }
        None
    }

    /// Whether `rva` lies inside a `__try` scope (see the module comment).
    pub(super) fn inside_try(&self, rva: u32) -> bool {
        let functions = &self.image.runtime_functions;
        let index = functions.partition_point(|function| function.0 <= rva);
        let Some(&(_, end, mut unwind)) = index.checked_sub(1).map(|i| &functions[i]) else {
            return false;
        };
        if rva >= end {
            return false;
        }
        let bytes = &self.image.bytes;
        for _ in 0..32 {
            let at = unwind as usize;
            let (Some(&version_flags), Some(&codes)) = (bytes.get(at), bytes.get(at + 2)) else {
                return false;
            };
            let flags = version_flags >> 3;
            let after = at + 4 + 2 * ((usize::from(codes) + 1) & !1);
            if flags & UNW_FLAG_CHAININFO != 0 {
                let Some(primary) = u32_at(bytes, after + 8) else {
                    return false;
                };
                unwind = primary;
                continue;
            }
            if flags & (UNW_FLAG_EHANDLER | UNW_FLAG_UHANDLER) == 0 {
                return false;
            }
            let Some(handler) = u32_at(bytes, after) else {
                return false;
            };
            if !matches!(
                self.handler_kind(handler),
                Some("__C_specific_handler" | "__GSHandlerCheck_SEH")
            ) {
                return false;
            }
            let Some(count) = u32_at(bytes, after + 4) else {
                return false;
            };
            return (0..count.min(256) as usize).any(|scope| {
                let entry = after + 8 + 16 * scope;
                matches!(
                    (u32_at(bytes, entry), u32_at(bytes, entry + 4), u32_at(bytes, entry + 12)),
                    (Some(begin), Some(end), Some(target)) if begin <= rva && rva < end && target != 0
                )
            });
        }
        false
    }
}
