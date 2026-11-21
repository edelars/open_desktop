//! S7-26: counts boots in an NVRAM variable, prints the count and PCR0 on COM1 and resets the
//! machine. The host runs it again with `--reboot-cycles N`; the count rises by one per cycle
//! and PCR0 is the same on every cycle.

#![no_std]
#![no_main]

use core::ptr;
use od_firmware::tpm::{TCG2_PROTOCOL_GUID, Tcg2Protocol};
use od_firmware::uefi::{
    guid::Guid,
    runtime::RESET_COLD,
    status::Status,
    system_table::SystemTable,
    types::{Char16, Handle},
};
use od_firmware::var::services::NON_VOLATILE_RUNTIME;
use od_firmware_test_apps::{ascii_to_char16, output};

const VENDOR: Guid = Guid {
    data1: 0x6f1d_52a4,
    data2: 0x26b0,
    data3: 0x4f0c,
    data4: [0x9a, 0x37, 0x4e, 0x0b, 0x61, 0xc8, 0x2d, 0x15],
};
static NAME: [Char16; 12] = ascii_to_char16(b"ODBootCount\0");
const LINE_CHARS: usize = 96;
/// `TPM2_PCR_Read`, no sessions: one selection, SHA-256, PCR 0.
const PCR_READ_PCR0: [u8; 20] = [
    0x80, 0x01, 0, 0, 0, 0x14, 0, 0, 0x01, 0x7e, 0, 0, 0, 1, 0, 0x0b, 3, 1, 0, 0,
];
/// Header, update counter, the selection out (count and one 6-byte entry), the digest list
/// (count and the size of the digest): the digest starts here.
const DIGEST_AT: usize = 10 + 4 + 10 + 4 + 2;
const DIGEST_LEN: usize = 32;

#[unsafe(no_mangle)]
unsafe extern "efiapi" fn efi_main(handle: Handle, system: *mut SystemTable) -> Status {
    if handle.is_null() || system.is_null() {
        return Status::INVALID_PARAMETER;
    }
    unsafe { run(system) }.err().unwrap_or(Status::LOAD_ERROR)
}

/// Only returns on an error: a successful reset does not come back.
unsafe fn run(system: *mut SystemTable) -> Result<(), Status> {
    let runtime = unsafe { (*system).runtime_services.as_ref() }.ok_or(Status::NOT_FOUND)?;
    let mut bytes = [0u8; 4];
    let mut size = bytes.len();
    let mut attributes = 0;
    let status = unsafe {
        (runtime.get_variable)(
            NAME.as_ptr(),
            &VENDOR,
            &mut attributes,
            &mut size,
            bytes.as_mut_ptr().cast(),
        )
    };
    let count = match status {
        Status::NOT_FOUND => 0,
        Status::SUCCESS if size == bytes.len() && attributes == NON_VOLATILE_RUNTIME => {
            u32::from_le_bytes(bytes)
        }
        other => return Err(other),
    };
    let count = count.wrapping_add(1);
    let status = unsafe {
        (runtime.set_variable)(
            NAME.as_ptr(),
            &VENDOR,
            NON_VOLATILE_RUNTIME,
            bytes.len(),
            count.to_le_bytes().as_ptr().cast(),
        )
    };
    if status != Status::SUCCESS {
        return Err(status);
    }
    let mut line = Line::new(b"OD-UEFI reboot BootCount=");
    line.decimal(count);
    unsafe { line.print(system) }?;

    let boot = unsafe { &*(*system).boot_services };
    let mut interface = ptr::null_mut();
    let status = unsafe {
        (boot.locate_protocol)(&TCG2_PROTOCOL_GUID, ptr::null_mut(), &mut interface)
    };
    if status != Status::SUCCESS {
        return Err(status);
    }
    let tpm = unsafe { interface.cast::<Tcg2Protocol>().as_ref() }.ok_or(Status::NOT_FOUND)?;
    let mut reply = [0u8; 80];
    let status = unsafe {
        (tpm.submit_command)(
            tpm,
            PCR_READ_PCR0.len() as u32,
            PCR_READ_PCR0.as_ptr(),
            reply.len() as u32,
            reply.as_mut_ptr(),
        )
    };
    if status != Status::SUCCESS {
        return Err(status);
    }
    // Response size 62 and TPM_RC_SUCCESS; byte reads, no 16-bit byte swaps.
    if reply[..6] != [0x80, 0x01, 0, 0, 0, 62] || reply[6..10] != [0; 4] {
        return Err(Status::DEVICE_ERROR);
    }
    // 13 + 64 characters: the console wraps a line at column 80.
    let mut line = Line::new(b"OD-UEFI PCR0=");
    for byte in &reply[DIGEST_AT..DIGEST_AT + DIGEST_LEN] {
        line.hex(*byte);
    }
    unsafe { line.print(system) }?;

    unsafe {
        ((*(*system).runtime_services).reset_system)(RESET_COLD, Status::SUCCESS, 0, ptr::null())
    };
    Err(Status::DEVICE_ERROR)
}

/// One COM1 line built in a fixed buffer: a text, then numbers, then CR LF and the terminator.
struct Line {
    chars: [Char16; LINE_CHARS],
    len: usize,
}

impl Line {
    fn new(text: &[u8]) -> Self {
        let mut line = Self {
            chars: [0; LINE_CHARS],
            len: 0,
        };
        for byte in text {
            line.push(*byte);
        }
        line
    }

    fn push(&mut self, byte: u8) {
        self.chars[self.len] = Char16::from(byte);
        self.len += 1;
    }

    fn decimal(&mut self, value: u32) {
        let mut digits = [0u8; 10];
        let mut count = 0;
        let mut rest = value;
        loop {
            digits[count] = b'0' + (rest % 10) as u8;
            count += 1;
            rest /= 10;
            if rest == 0 {
                break;
            }
        }
        while count > 0 {
            count -= 1;
            self.push(digits[count]);
        }
    }

    fn hex(&mut self, byte: u8) {
        for nibble in [byte >> 4, byte & 15] {
            self.push(if nibble < 10 {
                b'0' + nibble
            } else {
                b'a' - 10 + nibble
            });
        }
    }

    unsafe fn print(mut self, system: *mut SystemTable) -> Result<(), Status> {
        self.push(b'\r');
        self.push(b'\n');
        self.chars[self.len] = 0;
        match unsafe { output(system, self.chars.as_ptr()) } {
            Status::SUCCESS => Ok(()),
            other => Err(other),
        }
    }
}
