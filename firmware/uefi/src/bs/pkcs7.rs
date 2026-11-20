//! UEFI 2.11 §37.4 PKCS7 Verify Protocol on the protected NVRAM transport.

use core::{ffi::c_void, mem::size_of, ptr, slice};
use od_uefi_security::pkcs7::{MAX_DATABASE_BYTES, MAX_MESSAGE_SIGNATURE_BYTES, transport as wire};

use crate::{
    bs,
    uefi::{
        pkcs7::{PKCS7_VERIFY_PROTOCOL_GUID, Protocol, SignatureListArray},
        status::Status,
    },
    var::nvram::Nvram,
};

const LIST_HEADER_BYTES: usize = 28;
const MAX_LISTS_PER_DATABASE: usize = 64;

static PROTOCOL: Protocol = Protocol {
    verify_buffer,
    verify_signature,
};
static mut NVRAM: Option<Nvram> = None;

/// Publishes PKCS7 Verify Protocol for a board advertising the request/reply capability.
///
/// # Safety
/// Called once on the BSP during Boot Services, with the mapped NVRAM aperture from handoff.
pub unsafe fn install(base: u64) -> Status {
    if unsafe { ptr::addr_of!(NVRAM).read() }.is_some() {
        return Status::ALREADY_STARTED;
    }
    let nvram = unsafe { Nvram::at(base as usize) };
    unsafe { ptr::addr_of_mut!(NVRAM).write(Some(nvram)) };
    let mut handle = ptr::null_mut();
    let status = unsafe {
        bs::protocol::install_protocol_interface(
            &mut handle,
            &PKCS7_VERIFY_PROTOCOL_GUID,
            0,
            ptr::from_ref(&PROTOCOL).cast_mut().cast(),
        )
    };
    if status != Status::SUCCESS {
        unsafe { ptr::addr_of_mut!(NVRAM).write(None) };
    }
    status
}

struct Database<'a> {
    lists: [&'a [u8]; MAX_LISTS_PER_DATABASE],
    count: usize,
    total: usize,
}

impl<'a> Database<'a> {
    /// The caller supplies valid EFI memory for the terminated pointer array and each list.
    unsafe fn read(array: SignatureListArray) -> Result<Self, Status> {
        let mut db = Self {
            lists: [&[]; MAX_LISTS_PER_DATABASE],
            count: 0,
            total: 0,
        };
        if array.is_null() {
            return Ok(db);
        }
        while db.count < MAX_LISTS_PER_DATABASE {
            let list = unsafe { read_list_pointer(array, db.count)? };
            if list.is_null() {
                return Ok(db);
            }
            let header = unsafe { bytes(list, LIST_HEADER_BYTES) }.ok_or(Status::ABORTED)?;
            let size =
                u32::from_le_bytes([header[16], header[17], header[18], header[19]]) as usize;
            if size < LIST_HEADER_BYTES || size > MAX_DATABASE_BYTES - db.total {
                return Err(Status::ABORTED);
            }
            db.lists[db.count] = unsafe { bytes(list, size) }.ok_or(Status::ABORTED)?;
            db.total += size;
            db.count += 1;
        }
        if !unsafe { read_list_pointer(array, db.count)? }.is_null() {
            return Err(Status::ABORTED);
        }
        Ok(db)
    }

    fn iter(&self) -> impl Iterator<Item = &'a [u8]> + '_ {
        self.lists[..self.count].iter().copied()
    }
}

unsafe fn read_list_pointer(
    array: SignatureListArray,
    index: usize,
) -> Result<*const c_void, Status> {
    let address = (array as usize)
        .checked_add(index * size_of::<*const c_void>())
        .filter(|address| address.checked_add(size_of::<*const c_void>()).is_some())
        .ok_or(Status::ABORTED)?;
    Ok(unsafe { ptr::with_exposed_provenance::<*const c_void>(address).read_unaligned() })
}

unsafe fn active(this: *const Protocol) -> Option<Nvram> {
    if !ptr::eq(this, &PROTOCOL) || !unsafe { bs::boot_services_active() } {
        return None;
    }
    unsafe { ptr::addr_of!(NVRAM).read() }
}

/// The UEFI caller owns the backing allocation for the entire input range.
unsafe fn bytes<'a>(pointer: *const c_void, size: usize) -> Option<&'a [u8]> {
    if pointer.is_null() || size > isize::MAX as usize {
        return None;
    }
    (pointer as usize).checked_add(size)?;
    Some(unsafe { slice::from_raw_parts(pointer.cast(), size) })
}

unsafe fn request(
    nvram: Nvram,
    mode: wire::Mode,
    signed: &[u8],
    input: &[u8],
    allowed: SignatureListArray,
    revoked: SignatureListArray,
    timestamp: SignatureListArray,
) -> Result<wire::Reply, Status> {
    let allowed = unsafe { Database::read(allowed)? };
    let revoked = unsafe { Database::read(revoked)? };
    let timestamp = unsafe { Database::read(timestamp)? };
    let (header, total) = wire::header(
        mode,
        [
            signed.len(),
            input.len(),
            allowed.total,
            revoked.total,
            timestamp.total,
        ],
    )
    .map_err(|_| Status::UNSUPPORTED)?;
    nvram.verify_pkcs7(
        &header,
        total,
        [signed, input]
            .into_iter()
            .chain(allowed.iter())
            .chain(revoked.iter())
            .chain(timestamp.iter()),
    )
}

fn status(outcome: wire::Outcome) -> Status {
    match outcome {
        wire::Outcome::Success => Status::SUCCESS,
        wire::Outcome::Unsupported => Status::UNSUPPORTED,
        wire::Outcome::NotFound => Status::NOT_FOUND,
        wire::Outcome::Aborted => Status::ABORTED,
        wire::Outcome::SecurityViolation => Status::SECURITY_VIOLATION,
        wire::Outcome::CompromisedData => Status::COMPROMISED_DATA,
    }
}

unsafe extern "efiapi" fn verify_buffer(
    this: *const Protocol,
    signed_data: *const c_void,
    signed_size: usize,
    in_data: *const c_void,
    in_size: usize,
    allowed: SignatureListArray,
    revoked: SignatureListArray,
    timestamp: SignatureListArray,
    content: *mut c_void,
    content_size: *mut usize,
) -> Status {
    let Some(nvram) = (unsafe { active(this) }) else {
        return Status::INVALID_PARAMETER;
    };
    if signed_data.is_null()
        || signed_size == 0
        || allowed.is_null()
        || (in_data.is_null() && in_size != 0)
        || (!content.is_null() && content_size.is_null())
    {
        return Status::INVALID_PARAMETER;
    }
    if signed_size > MAX_MESSAGE_SIGNATURE_BYTES || in_size > wire::REQUEST_LIMIT {
        return Status::UNSUPPORTED;
    }
    if !content_size.is_null()
        && (content_size as usize)
            .checked_add(size_of::<usize>())
            .is_none()
    {
        return Status::INVALID_PARAMETER;
    }
    if content.is_null() && !content_size.is_null() && unsafe { content_size.read_unaligned() } != 0
    {
        return Status::INVALID_PARAMETER;
    }

    let response = {
        let Some(signed) = (unsafe { bytes(signed_data, signed_size) }) else {
            return Status::INVALID_PARAMETER;
        };
        let (mode, input) = if in_data.is_null() {
            (wire::Mode::Embedded, &[][..])
        } else if let Some(input) = unsafe { bytes(in_data, in_size) } {
            (wire::Mode::Detached, input)
        } else {
            return Status::INVALID_PARAMETER;
        };
        unsafe { request(nvram, mode, signed, input, allowed, revoked, timestamp) }
    };
    let reply = match response {
        Ok(reply) => reply,
        Err(error) => return error,
    };
    let result = status(reply.outcome);
    if result != Status::SUCCESS || content.is_null() {
        return result;
    }

    // The staging operation is synchronous. All borrowed request slices have ended before
    // copying to a caller buffer, which may overlap the original message.
    let (source, size) = if in_data.is_null() {
        let offset = reply.content_offset as usize;
        let size = reply.content_length as usize;
        if offset.checked_add(size).is_none_or(|end| end > signed_size) {
            return Status::DEVICE_ERROR;
        }
        (unsafe { signed_data.cast::<u8>().add(offset) }, size)
    } else {
        if reply.content_offset != 0 || reply.content_length != 0 {
            return Status::DEVICE_ERROR;
        }
        (in_data.cast::<u8>(), in_size)
    };
    let capacity = unsafe { content_size.read_unaligned() };
    if size > capacity {
        unsafe { content_size.write_unaligned(size) };
        return Status::BUFFER_TOO_SMALL;
    }
    if (content as usize).checked_add(size).is_none() {
        return Status::INVALID_PARAMETER;
    }
    unsafe {
        ptr::copy(source, content.cast::<u8>(), size);
        content_size.write_unaligned(size);
    }
    Status::SUCCESS
}

unsafe extern "efiapi" fn verify_signature(
    this: *const Protocol,
    signature: *const c_void,
    signature_size: usize,
    hash: *const c_void,
    hash_size: usize,
    allowed: SignatureListArray,
    revoked: SignatureListArray,
    timestamp: SignatureListArray,
) -> Status {
    let Some(nvram) = (unsafe { active(this) }) else {
        return Status::INVALID_PARAMETER;
    };
    if signature.is_null()
        || signature_size == 0
        || hash.is_null()
        || hash_size == 0
        || allowed.is_null()
    {
        return Status::INVALID_PARAMETER;
    }
    if signature_size > MAX_MESSAGE_SIGNATURE_BYTES {
        return Status::UNSUPPORTED;
    }
    if hash_size != 32 {
        return Status::COMPROMISED_DATA;
    }
    let (Some(signed), Some(hash)) = (unsafe { bytes(signature, signature_size) }, unsafe {
        bytes(hash, hash_size)
    }) else {
        return Status::INVALID_PARAMETER;
    };
    match unsafe {
        request(
            nvram,
            wire::Mode::Digest,
            signed,
            hash,
            allowed,
            revoked,
            timestamp,
        )
    } {
        Ok(reply) => status(reply.outcome),
        Err(error) => error,
    }
}
