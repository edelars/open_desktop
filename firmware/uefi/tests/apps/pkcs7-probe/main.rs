#![no_std]
#![no_main]

use core::{arch::asm, ptr};
use od_firmware::uefi::{
    pkcs7::{PKCS7_VERIFY_PROTOCOL_GUID, Protocol, SignatureListArray},
    status::Status,
    system_table::SystemTable,
    types::Handle,
};

macro_rules! fixture {
    ($name:literal) => {
        include_bytes!(concat!(env!("OD_PKCS7_FIXTURE_DIR"), "/", $name))
    };
}

const MESSAGE: &[u8] = fixture!("message");
const DIGEST: &[u8] = fixture!("digest");
const EMBEDDED: &[u8] = fixture!("embedded.der");
const DETACHED: &[u8] = fixture!("detached.der");
const TIMESTAMPED: &[u8] = fixture!("timestamped.der");
const FOREIGN_TIMESTAMP: &[u8] = fixture!("foreign-timestamp.der");
const ALLOWED: &[u8] = fixture!("allowed.esl");
const TIMESTAMPS: &[u8] = fixture!("tsa.esl");
const REVOKED_HASH: &[u8] = fixture!("message-revoked.esl");
const REVOKED_BEFORE: &[u8] = fixture!("revoked-before.esl");
const REVOKED_AFTER: &[u8] = fixture!("revoked-after.esl");
const REVOKED_ALWAYS: &[u8] = fixture!("revoked-always.esl");

#[unsafe(no_mangle)]
unsafe extern "efiapi" fn efi_main(_image: Handle, system: *mut SystemTable) -> Status {
    let Some(boot) =
        (unsafe { system.as_ref() }).and_then(|table| unsafe { table.boot_services.as_ref() })
    else {
        return Status::LOAD_ERROR;
    };
    let mut interface = ptr::null_mut();
    let status = unsafe {
        (boot.locate_protocol)(&PKCS7_VERIFY_PROTOCOL_GUID, ptr::null_mut(), &mut interface)
    };
    if status != Status::SUCCESS || interface.is_null() {
        write_com1(b"OD-UEFI PKCS7 LocateProtocol failed\r\n");
        return Status::LOAD_ERROR;
    }
    let protocol = unsafe { &*interface.cast::<Protocol>() };
    match unsafe { exercise(protocol) } {
        Ok(()) => {
            write_com1(
                b"OD-UEFI PKCS7 VerifyBuffer, VerifySignature and timed revocation verified\r\n",
            );
            Status::SUCCESS
        }
        Err(case) => {
            write_com1(b"OD-UEFI PKCS7 check failed: ");
            write_com1(case);
            write_com1(b"\r\n");
            Status::LOAD_ERROR
        }
    }
}

unsafe fn exercise(protocol: &Protocol) -> Result<(), &'static [u8]> {
    let allowed = [ALLOWED.as_ptr().cast(), ptr::null()];
    let timestamps = [TIMESTAMPS.as_ptr().cast(), ptr::null()];
    let revoked_hash = [REVOKED_HASH.as_ptr().cast(), ptr::null()];
    let revoked_before = [REVOKED_BEFORE.as_ptr().cast(), ptr::null()];
    let revoked_after = [REVOKED_AFTER.as_ptr().cast(), ptr::null()];
    let revoked_always = [REVOKED_ALWAYS.as_ptr().cast(), ptr::null()];
    let empty = [ptr::null()];
    let malformed = [0u8; 28];
    let invalid = [malformed.as_ptr().cast(), ptr::null()];

    let mut output = [0xaa; MESSAGE.len()];
    let mut size = output.len() - 1;
    let status = unsafe {
        (protocol.verify_buffer)(
            protocol,
            EMBEDDED.as_ptr().cast(),
            EMBEDDED.len(),
            ptr::null(),
            0,
            allowed.as_ptr(),
            ptr::null(),
            ptr::null(),
            output.as_mut_ptr().cast(),
            &mut size,
        )
    };
    if status != Status::BUFFER_TOO_SMALL
        || size != MESSAGE.len()
        || output != [0xaa; MESSAGE.len()]
    {
        return Err(b"short output buffer");
    }
    for (signed, input) in [(EMBEDDED, None), (DETACHED, Some(MESSAGE))] {
        let (in_data, in_size) = input
            .map(|bytes| (bytes.as_ptr().cast(), bytes.len()))
            .unwrap_or((ptr::null(), 0));
        let status = unsafe {
            (protocol.verify_buffer)(
                protocol,
                signed.as_ptr().cast(),
                signed.len(),
                in_data,
                in_size,
                allowed.as_ptr(),
                ptr::null(),
                ptr::null(),
                output.as_mut_ptr().cast(),
                &mut size,
            )
        };
        if status != Status::SUCCESS || size != MESSAGE.len() || output.as_slice() != MESSAGE {
            return Err(b"verified content copy");
        }
        output.fill(0xaa);
    }
    let verify = |signed: &[u8], hash: &[u8]| unsafe {
        (protocol.verify_signature)(
            protocol,
            signed.as_ptr().cast(),
            signed.len(),
            hash.as_ptr().cast(),
            hash.len(),
            allowed.as_ptr(),
            ptr::null(),
            ptr::null(),
        )
    };
    if verify(EMBEDDED, DIGEST) != Status::SUCCESS || verify(DETACHED, DIGEST) != Status::SUCCESS {
        return Err(b"prehashed embedded and detached");
    }
    if verify(DETACHED, &[0; 32]) != Status::COMPROMISED_DATA
        || verify(DETACHED, &DIGEST[..31]) != Status::COMPROMISED_DATA
    {
        return Err(b"prehashed mismatch");
    }
    let verify = |sign: &[u8], input: Option<&[u8]>, trust: SignatureListArray, revoked, tsa| unsafe {
        let (in_data, in_size) = input
            .map(|data| (data.as_ptr().cast(), data.len()))
            .unwrap_or((ptr::null(), 0));
        (protocol.verify_buffer)(
            protocol,
            sign.as_ptr().cast(),
            sign.len(),
            in_data,
            in_size,
            trust,
            revoked,
            tsa,
            ptr::null_mut(),
            ptr::null_mut(),
        )
    };
    if verify(DETACHED, None, allowed.as_ptr(), ptr::null(), ptr::null()) != Status::NOT_FOUND
        || verify(
            EMBEDDED,
            Some(MESSAGE),
            allowed.as_ptr(),
            ptr::null(),
            ptr::null(),
        ) != Status::UNSUPPORTED
        || verify(&[0x30, 0], None, allowed.as_ptr(), ptr::null(), ptr::null())
            != Status::UNSUPPORTED
    {
        return Err(b"missing, conflicting or malformed content");
    }
    if verify(EMBEDDED, None, ptr::null(), ptr::null(), ptr::null()) != Status::INVALID_PARAMETER
        || verify(EMBEDDED, None, empty.as_ptr(), ptr::null(), ptr::null())
            != Status::SECURITY_VIOLATION
        || verify(EMBEDDED, None, invalid.as_ptr(), ptr::null(), ptr::null()) != Status::ABORTED
    {
        return Err(b"caller-only trust and list errors");
    }
    if verify(
        EMBEDDED,
        None,
        allowed.as_ptr(),
        revoked_hash.as_ptr(),
        ptr::null(),
    ) != Status::SECURITY_VIOLATION
    {
        return Err(b"message hash revocation");
    }
    if verify(
        TIMESTAMPED,
        None,
        allowed.as_ptr(),
        revoked_after.as_ptr(),
        timestamps.as_ptr(),
    ) != Status::SUCCESS
    {
        return Err(b"trusted timestamp before cutoff");
    }
    for (signature, revoked, tsa) in [
        (TIMESTAMPED, revoked_before.as_ptr(), timestamps.as_ptr()),
        (TIMESTAMPED, revoked_always.as_ptr(), timestamps.as_ptr()),
        (TIMESTAMPED, revoked_after.as_ptr(), ptr::null()),
        (
            FOREIGN_TIMESTAMP,
            revoked_after.as_ptr(),
            timestamps.as_ptr(),
        ),
    ] {
        if verify(signature, None, allowed.as_ptr(), revoked, tsa) != Status::SECURITY_VIOLATION {
            return Err(b"cutoff or invalid timestamp proof");
        }
    }
    Ok(())
}

fn write_com1(bytes: &[u8]) {
    for &byte in bytes {
        unsafe { asm!("out dx, al", in("dx") 0x3f8u16, in("al") byte, options(nostack, nomem)) };
    }
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo<'_>) -> ! {
    loop {
        unsafe { asm!("cli", "hlt", options(nostack, nomem)) };
    }
}
