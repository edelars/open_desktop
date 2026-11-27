//! Stop snapshot (S8-10): a JSON description of the stopped machine with the module and RVA of
//! the instruction pointer and of the return addresses on the stack.
//!
//! The snapshot is taken after the run ended, from the machine owner's thread, through the
//! read-only diagnostic API of `od-machine` (no guest write, no accessed/dirty bit, no
//! device access, no further execution). A module is identified from PE headers in guest memory
//! only (see `guest_module`).

use crate::guest_module::{GuestMemory, Lookup, ReadFault, find_module};
use od_machine::{DiagnosticReadError, LoadedFirmwareBoard, VcpuDiagnosticState};
use serde_json::{Value, json};

/// Bytes read at RIP.
const RIP_BYTES: usize = 16;
/// Stack slots (8-byte) from RSP examined for return addresses.
const STACK_SLOTS: usize = 8;
const GPR_NAMES: [&str; 16] = [
    "rax", "rcx", "rdx", "rbx", "rsp", "rbp", "rsi", "rdi", "r8", "r9", "r10", "r11", "r12", "r13",
    "r14", "r15",
];
/// Most `--peek` reads per run and bytes per read (S7-32).
pub const MAX_PEEKS: usize = 16;
pub const MAX_PEEK_BYTES: usize = 4096;
const DEFAULT_PEEK_BYTES: usize = 64;

/// Where a `--peek` read starts (S7-32): an absolute guest-virtual address, or an offset from the
/// base of the image that covers RIP at the stop (a kernel base moves with KASLR).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PeekBase {
    Absolute,
    RipModule,
}

/// One guest-virtual read written into the stop snapshot (`--peek`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Peek {
    pub base: PeekBase,
    pub offset: u64,
    pub len: usize,
}

/// Parses `<hex>[:<bytes>]` or `rip_module+<hex>[:<bytes>]`: the address or offset is hexadecimal
/// (an optional `0x`), `<bytes>` is decimal, 1 to [`MAX_PEEK_BYTES`], 64 when omitted.
pub fn parse_peek(value: &str) -> Result<Peek, String> {
    let invalid = || {
        format!(
            "invalid --peek '{value}'; expected (<hex>|rip_module+<hex>)[:<bytes>], a hexadecimal \
             address or offset and 1 to {MAX_PEEK_BYTES} bytes in decimal"
        )
    };
    let (address, len) = match value.split_once(':') {
        Some((address, len)) => (address, len.parse::<usize>().map_err(|_| invalid())?),
        None => (value, DEFAULT_PEEK_BYTES),
    };
    if len == 0 || len > MAX_PEEK_BYTES {
        return Err(invalid());
    }
    let (base, offset) = match address.strip_prefix("rip_module+") {
        Some(offset) => (PeekBase::RipModule, offset),
        None => (PeekBase::Absolute, address),
    };
    let offset = offset.strip_prefix("0x").unwrap_or(offset);
    let offset = u64::from_str_radix(offset, 16).map_err(|_| invalid())?;
    Ok(Peek { base, offset, len })
}

/// The guest-virtual space of one vCPU of a board.
pub struct BoardMemory<'a> {
    pub board: &'a LoadedFirmwareBoard,
    pub vcpu: usize,
}

impl GuestMemory for BoardMemory<'_> {
    fn read(&self, linear: u64, output: &mut [u8]) -> Result<(), ReadFault> {
        self.board
            .diagnostic_read_virtual(self.vcpu, linear, output)
            .map_err(|failure| ReadFault {
                read: failure.read,
                reason: match failure.error {
                    DiagnosticReadError::Translate(error) => format!(
                        "address 0x{:016x} does not translate: {error:?}",
                        failure.linear
                    ),
                    DiagnosticReadError::Physical { physical, error } => format!(
                        "address 0x{:016x} maps to physical 0x{physical:x}, which is not readable \
                         RAM, ROM or framebuffer: {error:?}",
                        failure.linear
                    ),
                },
            })
    }
}

fn hex(value: u64) -> String {
    format!("0x{value:016x}")
}

fn opt_hex(value: Option<u64>) -> Value {
    value.map_or(Value::Null, |value| json!(hex(value)))
}

/// The JSON of one address: `module` fields when an image covers it, else the reason.
pub(crate) fn module_json(memory: &impl GuestMemory, address: u64) -> Value {
    match find_module(memory, address) {
        Lookup::Found {
            module,
            pages_scanned,
        } => {
            let (pdb, pdb_error) = match &module.codeview {
                Ok(codeview) => (
                    json!({
                        "path": codeview.pdb_path,
                        "guid": codeview.guid_text(),
                        "age": codeview.age,
                        "symbol_key": codeview.symbol_key(),
                    }),
                    Value::Null,
                ),
                Err(reason) => (Value::Null, json!(reason)),
            };
            json!({
                "found": true,
                "base": hex(module.base),
                "rva": format!("0x{:x}", address - module.base),
                "size_of_image": module.size_of_image,
                "time_date_stamp": module.time_date_stamp,
                "machine": format!("0x{:04x}", module.machine),
                "pdb": pdb,
                "pdb_error": pdb_error,
                "pages_scanned": pages_scanned,
            })
        }
        Lookup::NotFound {
            reason,
            pages_scanned,
        } => json!({"found": false, "reason": reason, "pages_scanned": pages_scanned}),
    }
}

/// Bytes at `linear` as lowercase hex, with how many were read and why the read ended.
fn bytes_json(memory: &impl GuestMemory, linear: u64, len: usize) -> Value {
    let mut bytes = vec![0u8; len];
    let (read, error) = match memory.read(linear, &mut bytes) {
        Ok(()) => (len, Value::Null),
        Err(fault) => (fault.read, json!(fault.reason)),
    };
    let text: String = bytes[..read]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    json!({"hex": text, "read": read, "error": error})
}

/// The return-address search over the first slots of the stack. A slot that is not mapped, or not
/// inside any image, still appears with the reason.
fn stack_json(memory: &impl GuestMemory, rsp: u64) -> Value {
    let mut raw = [0u8; STACK_SLOTS * 8];
    let (read, error) = match memory.read(rsp, &mut raw) {
        Ok(()) => (raw.len(), Value::Null),
        Err(fault) => (fault.read, json!(fault.reason)),
    };
    let slots: Vec<Value> = raw[..read - read % 8]
        .chunks_exact(8)
        .enumerate()
        .map(|(slot, bytes)| {
            let value = u64::from_le_bytes(bytes.try_into().expect("8 bytes"));
            json!({
                "slot": slot,
                "address": hex(rsp.wrapping_add(slot as u64 * 8)),
                "value": hex(value),
                "module": module_json(memory, value),
            })
        })
        .collect();
    json!({"rsp": hex(rsp), "read_bytes": read, "error": error, "slots": slots})
}

/// The `--peek` reads: each names its address, the bytes read as lowercase hex and why a read
/// ended early. The image that covers RIP is looked up once; a read relative to it, with no image
/// at RIP, has a null address and the reason. Offsets wrap modulo 2^64 (a negative offset in two's
/// complement reads below the base); a non-canonical result fails as a translation error.
fn peeks_json(memory: &impl GuestMemory, rip: u64, peeks: &[Peek]) -> Value {
    let needs_module = peeks.iter().any(|peek| peek.base == PeekBase::RipModule);
    let rip_module = needs_module.then(|| match find_module(memory, rip) {
        Lookup::Found { module, .. } => Ok(module.base),
        Lookup::NotFound { reason, .. } => Err(reason),
    });
    let values: Vec<Value> = peeks
        .iter()
        .map(|peek| {
            let offset = json!(format!("0x{:x}", peek.offset));
            let address = match (peek.base, &rip_module) {
                (PeekBase::Absolute, _) => peek.offset,
                (PeekBase::RipModule, Some(Ok(base))) => base.wrapping_add(peek.offset),
                (PeekBase::RipModule, Some(Err(reason))) => {
                    return json!({
                        "address": Value::Null,
                        "rip_module_offset": offset,
                        "read": 0,
                        "hex": "",
                        "error": format!("no image covers RIP: {reason}"),
                    });
                }
                (PeekBase::RipModule, None) => unreachable!("the RIP image is looked up for these"),
            };
            let mut value = bytes_json(memory, address, peek.len);
            value["address"] = json!(hex(address));
            if peek.base == PeekBase::RipModule {
                value["rip_module_offset"] = offset;
            }
            value
        })
        .collect();
    Value::Array(values)
}

/// Builds the snapshot of `vcpu`. `reason` carries the stop reason and its fields as the run
/// report has them, `counters` the instruction and quantum counters and the virtual clock,
/// `inputs` the hashes of the run inputs.
pub fn snapshot(
    state: &VcpuDiagnosticState,
    memory: &impl GuestMemory,
    reason: Value,
    counters: Value,
    inputs: Value,
    peeks: &[Peek],
) -> Value {
    let gpr: serde_json::Map<String, Value> = GPR_NAMES
        .iter()
        .zip(state.gpr)
        .map(|(name, value)| ((*name).to_owned(), json!(hex(value))))
        .collect();
    let rsp = state.gpr[od_isa::Register::Rsp as usize];
    let mut snapshot = json!({
        "schema_version": 1,
        "command": "boot-firmware",
        "kind": "stop_snapshot",
        "reason": reason,
        "vcpu": state.vcpu,
        "mode": if state.long_mode_model { "long64" } else { "legacy_model" },
        "cs_selector": state.cs_selector.map(|selector| format!("0x{selector:04x}")),
        "cpl": state.cpl,
        "activity": state.activity.map(|activity| format!("{activity:?}")),
        "rip": hex(state.rip),
        "rip_bytes": bytes_json(memory, state.rip, RIP_BYTES),
        "module": module_json(memory, state.rip),
        "stack": stack_json(memory, rsp),
        "gpr": gpr,
        "rflags": hex(state.rflags),
        "cr0": opt_hex(state.cr0),
        "cr2": opt_hex(state.cr2),
        "cr3": opt_hex(state.cr3),
        "cr4": opt_hex(state.cr4),
        "efer": hex(state.efer),
        "counters": counters,
        "inputs": inputs,
    });
    // Only with `--peek`: a snapshot without it stays byte-identical to one written before the
    // option existed (golden comparisons hash the snapshot).
    if !peeks.is_empty() {
        snapshot["peeks"] = peeks_json(memory, state.rip, peeks);
    }
    snapshot
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::guest_module::tests::{FakeMemory, test_pe};
    use od_machine::{FirmwareBoardConfig, PagedProbeExit, load_firmware_image};

    const IMAGE_BYTES: usize = 0x40_0000;
    const LOAD_BASE: u64 = 0x20_0000;

    fn crc32(bytes: &[u8]) -> u32 {
        let mut crc = 0xFFFF_FFFFu32;
        for &byte in bytes {
            crc ^= u32::from(byte);
            for _ in 0..8 {
                crc = if crc & 1 != 0 {
                    (crc >> 1) ^ 0xEDB8_8320
                } else {
                    crc >> 1
                };
            }
        }
        !crc
    }

    /// An ODFWIMG1 image (FIRMWARE_BOOT.md 3.2) entered at image offset `entry`, with `patches`
    /// copied in. It is loaded at `LOAD_BASE` and identity-mapped by the boot page tables.
    pub(crate) fn rom(entry: u32, patches: &[(usize, &[u8])]) -> Vec<u8> {
        let mut image = vec![0u8; IMAGE_BYTES];
        image[..8].copy_from_slice(b"ODFWIMG1");
        image[0x08..0x0C].copy_from_slice(&1u32.to_le_bytes());
        image[0x0C..0x10].copy_from_slice(&0x40u32.to_le_bytes());
        image[0x10..0x18].copy_from_slice(&(IMAGE_BYTES as u64).to_le_bytes());
        image[0x18..0x20].copy_from_slice(&LOAD_BASE.to_le_bytes());
        image[0x20..0x28].copy_from_slice(&u64::from(entry).to_le_bytes());
        image[0x28..0x2A].copy_from_slice(&1u16.to_le_bytes());
        for (offset, bytes) in patches {
            image[*offset..*offset + bytes.len()].copy_from_slice(bytes);
        }
        let crc = crc32(&image);
        image[0x30..0x34].copy_from_slice(&crc.to_le_bytes());
        image
    }

    fn stop_snapshot_of(image: &[u8]) -> (PagedProbeExit, Value) {
        stop_snapshot_with_peeks(image, &[])
    }

    fn stop_snapshot_with_peeks(image: &[u8], peeks: &[Peek]) -> (PagedProbeExit, Value) {
        let mut board = load_firmware_image(image, &FirmwareBoardConfig::default()).unwrap();
        let report = board.run_boot(1000).unwrap();
        let state = board.diagnostic_vcpu_state(BOOT_VCPU).unwrap();
        let memory = BoardMemory {
            board: &board,
            vcpu: BOOT_VCPU,
        };
        let snapshot = snapshot(
            &state,
            &memory,
            json!({"exit": format!("{:?}", report.exit)}),
            json!({"retired": report.retired, "quanta": report.quanta}),
            json!({"image_sha256": "test"}),
            peeks,
        );
        (report.exit, snapshot)
    }

    const BOOT_VCPU: usize = 0;

    #[test]
    fn peek_specs_parse_absolute_and_rip_module_forms_and_refuse_bad_lengths() {
        assert_eq!(
            parse_peek("0xfffff805ab746be0").unwrap(),
            Peek {
                base: PeekBase::Absolute,
                offset: 0xffff_f805_ab74_6be0,
                len: 64,
            }
        );
        assert_eq!(
            parse_peek("rip_module+f46be0:40").unwrap(),
            Peek {
                base: PeekBase::RipModule,
                offset: 0xf4_6be0,
                len: 40,
            }
        );
        assert_eq!(parse_peek("1000:4096").unwrap().len, 4096);
        for bad in [
            "",
            "1000:0",
            "1000:4097",
            "xyz",
            "rip_module+",
            "1000:-1",
            "rip_module+g",
        ] {
            assert!(parse_peek(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn peeks_read_absolute_and_rip_module_addresses_and_name_a_failed_read() {
        // The same image as the PE test below: the machine stops on UD2 at RVA 0x200A.
        let pe = test_pe("od-test-image.pdb", 0x4000);
        let code = [0xe8, 0, 0, 0, 0, 0xe8, 0, 0, 0, 0, 0x0f, 0x0b];
        let image = rom(0x10_2000, &[(0x10_0000, &pe.bytes), (0x10_2000, &code)]);
        let base = LOAD_BASE + 0x10_0000;
        let peeks = [
            Peek {
                base: PeekBase::RipModule,
                offset: 0x2000,
                len: 12,
            },
            Peek {
                base: PeekBase::Absolute,
                offset: base + 0x2005,
                len: 5,
            },
            Peek {
                base: PeekBase::Absolute,
                offset: 0x8000_0000_0000,
                len: 8,
            },
        ];
        let (_, snapshot) = stop_snapshot_with_peeks(&image, &peeks);
        let reads = snapshot["peeks"].as_array().unwrap();
        assert_eq!(reads.len(), 3);
        assert_eq!(reads[0]["address"], hex(base + 0x2000));
        assert_eq!(reads[0]["rip_module_offset"], "0x2000");
        assert_eq!(reads[0]["hex"], "e800000000e8000000000f0b");
        assert_eq!(reads[0]["read"], 12);
        assert_eq!(reads[0]["error"], Value::Null);
        assert_eq!(reads[1]["address"], hex(base + 0x2005));
        assert_eq!(reads[1]["hex"], "e800000000");
        assert!(reads[1].get("rip_module_offset").is_none());
        assert_eq!(reads[2]["read"], 0);
        assert_eq!(reads[2]["hex"], "");
        assert!(
            reads[2]["error"]
                .as_str()
                .unwrap()
                .contains("does not translate")
        );
    }

    #[test]
    fn a_rip_module_peek_without_an_image_at_rip_says_so() {
        // UD2 at the ROM entry, no PE image around it.
        let image = rom(0x1000, &[(0x1000, &[0x0f, 0x0b])]);
        let peeks = [Peek {
            base: PeekBase::RipModule,
            offset: 0x10,
            len: 4,
        }];
        let (_, snapshot) = stop_snapshot_with_peeks(&image, &peeks);
        let read = &snapshot["peeks"][0];
        assert_eq!(read["read"], 0);
        assert_eq!(read["rip_module_offset"], "0x10");
        assert!(
            read["error"]
                .as_str()
                .unwrap()
                .starts_with("no image covers RIP")
        );
        assert_eq!(read["address"], Value::Null);
    }

    #[test]
    fn a_stop_inside_a_pe_image_names_the_module_rva_and_pdb_and_its_callers() {
        // The PE sits at image offset 0x10_0000 (guest-virtual 0x30_0000, SizeOfImage 0x4000);
        // its code is at RVA 0x2000: CALL +0 (pushes RVA 0x2005); CALL +0 (pushes RVA 0x200A);
        // UD2 at RVA 0x200A. The machine stops there.
        let pe = test_pe("od-test-image.pdb", 0x4000);
        let code = [0xe8, 0, 0, 0, 0, 0xe8, 0, 0, 0, 0, 0x0f, 0x0b];
        let image = rom(0x10_2000, &[(0x10_0000, &pe.bytes), (0x10_2000, &code)]);
        let (exit, snapshot) = stop_snapshot_of(&image);
        assert!(matches!(exit, PagedProbeExit::Fault(_)), "{exit:?}");
        // Without `--peek` the snapshot has no `peeks` key (golden snapshots keep their hash).
        assert!(snapshot.get("peeks").is_none());

        let base = LOAD_BASE + 0x10_0000;
        assert_eq!(snapshot["kind"], "stop_snapshot");
        assert_eq!(snapshot["vcpu"], 0);
        assert_eq!(snapshot["mode"], "long64");
        assert_eq!(snapshot["cpl"], serde_json::Value::Null);
        assert_eq!(snapshot["rip"], hex(base + 0x200a));
        assert_eq!(
            snapshot["rip_bytes"]["hex"],
            format!("0f0b{}", "00".repeat(14))
        );
        assert_eq!(snapshot["rip_bytes"]["read"], 16);
        let module = &snapshot["module"];
        assert_eq!(module["found"], true);
        assert_eq!(module["base"], hex(base));
        assert_eq!(module["rva"], "0x200a");
        assert_eq!(module["size_of_image"], 0x4000);
        assert_eq!(module["time_date_stamp"], pe.time_date_stamp);
        assert_eq!(module["pdb"]["path"], "od-test-image.pdb");
        assert_eq!(
            module["pdb"]["guid"],
            "12345678-9ABC-DEF0-0123-456789ABCDEF"
        );
        assert_eq!(module["pdb"]["age"], pe.age);
        assert_eq!(module["pdb_error"], Value::Null);
        // The two return addresses are the first two stack slots, in the same module.
        let slots = snapshot["stack"]["slots"].as_array().unwrap();
        assert_eq!(slots.len(), 8);
        assert_eq!(
            slots[0]["value"],
            hex(base + 0x2005 + 5),
            "the last CALL pushed 0x200A"
        );
        assert_eq!(slots[0]["module"]["rva"], "0x200a");
        assert_eq!(slots[1]["value"], hex(base + 0x2005));
        assert_eq!(slots[1]["module"]["rva"], "0x2005");
        assert_eq!(slots[1]["module"]["pdb"]["path"], "od-test-image.pdb");
        assert_eq!(snapshot["gpr"].as_object().unwrap().len(), 16);
        assert_eq!(snapshot["cr3"].is_string(), true);
        assert_eq!(snapshot["inputs"]["image_sha256"], "test");
    }

    #[test]
    fn a_stop_on_an_unmapped_page_writes_the_reason_and_does_not_fail() {
        // MOV RAX, 0x7FFF_0000_0000; JMP RAX: the next fetch is on a page no table maps.
        let target: u64 = 0x0000_7fff_0000_0000;
        let mut code = vec![0x48, 0xb8];
        code.extend_from_slice(&target.to_le_bytes());
        code.extend_from_slice(&[0xff, 0xe0]);
        let (exit, snapshot) = stop_snapshot_of(&rom(0x40, &[(0x40, &code)]));
        assert!(
            !matches!(exit, PagedProbeExit::Halted | PagedProbeExit::Shutdown),
            "{exit:?}"
        );
        assert_eq!(snapshot["rip"], hex(target));
        assert_eq!(snapshot["rip_bytes"]["read"], 0);
        assert_eq!(snapshot["rip_bytes"]["hex"], "");
        assert!(
            snapshot["rip_bytes"]["error"]
                .as_str()
                .unwrap()
                .contains("NotPresent"),
            "{}",
            snapshot["rip_bytes"]
        );
        assert_eq!(snapshot["module"]["found"], false);
        assert_eq!(snapshot["module"]["pages_scanned"], 0);
        assert!(
            snapshot["module"]["reason"]
                .as_str()
                .unwrap()
                .contains("not readable")
        );
        // The stack is still described (RSP is the boot stack).
        assert_eq!(snapshot["stack"]["read_bytes"], 64);
    }

    #[test]
    fn a_read_that_ends_inside_the_rip_page_pair_reports_the_bytes_it_got() {
        let mut memory = FakeMemory::default();
        memory.map(0x1000, &[0x90; 0x1000]);
        // RIP is 6 bytes before the end of the last mapped page.
        let bytes = bytes_json(&memory, 0x1ffa, RIP_BYTES);
        assert_eq!(bytes["read"], 6);
        assert_eq!(bytes["hex"], "909090909090");
        assert!(bytes["error"].as_str().unwrap().contains("no page"));
    }
}
