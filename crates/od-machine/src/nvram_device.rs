//! Two committed generations of firmware variables, with an optional protected policy.

use crate::bus::{AccessWidth, BusDevice, DeviceAccess, DeviceError, IoSpace};
use crate::crc32::crc32_ieee;
use od_uefi_security::protected::{IMAGE_CAPACITY, Image};
use std::collections::HashSet;
use std::fmt;
use std::sync::{Arc, Mutex, MutexGuard};

mod authenticated;
mod image;
mod provision;

pub const NVRAM_MMIO_BASE: u64 = 0xff00_0000;
pub const NVRAM_STORE_BYTES: usize = IMAGE_CAPACITY;
pub const NVRAM_MMIO_BYTES: u64 = NVRAM_STORE_BYTES as u64 + 0x1000;
const REGISTERS: u64 = NVRAM_STORE_BYTES as u64;
const ADDR: u64 = REGISTERS;
const LEN: u64 = REGISTERS + 4;
const CMD: u64 = REGISTERS + 8;
const STATUS: u64 = REGISTERS + 12;
const ACTIVE_LEN: u64 = REGISTERS + 16;
const POLICY_EPOCH_LOW: u64 = REGISTERS + 20;
const POLICY_EPOCH_HIGH: u64 = REGISTERS + 24;

const READ: u32 = 1;
const WRITE: u32 = 2;
const FLUSH: u32 = 3;
const AUTH_UPDATE: u32 = 4;
const IMAGE_BEGIN: u32 = 5;
const IMAGE_CHUNK: u32 = 6;
const IMAGE_VERIFY: u32 = 7;

const SNAPSHOT_MAGIC: &[u8; 8] = b"ODNVBANK";
const SNAPSHOT_HEADER_BYTES: usize = 16;
const BANK_HEADER_BYTES: usize = 16;
const BANK_BYTES: usize = BANK_HEADER_BYTES + NVRAM_STORE_BYTES;
pub const NVRAM_SNAPSHOT_BYTES: usize = SNAPSHOT_HEADER_BYTES + 2 * BANK_BYTES;
// Empty ODVARSTR v1 from docs/specs/NVRAM_DEVICE.md. The machine treats the
// bytes as opaque and does not parse the firmware's variable format.
const EMPTY_STORE: &[u8] = &[
    0x4f, 0x44, 0x56, 0x41, 0x52, 0x53, 0x54, 0x52, 0x01, 0x00, 0x18, 0x00, 0x18, 0x00, 0x00, 0x00,
    0x5b, 0xed, 0x3b, 0x0b, 0x00, 0x00, 0x00, 0x00,
];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NvramContentsError {
    InvalidHeader,
    NoValidBank,
    AmbiguousGeneration,
    InvalidProtectedStore,
    ProtectionRequiresFirmware,
    InvalidPlatformKey,
    ReusedPlatformKey,
}

/// Policy applied by the host while creating or resetting a protected variable store.
#[derive(Clone, Copy, Debug)]
pub enum SecureBootKeys<'a> {
    Defaults { public_platform_key: &'a [u8] },
    SetupMode,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u32)]
enum SnapshotFormat {
    Legacy = 1,
    Protected = 2,
}

/// Initial bytes for a new board instance. Every board gets its own banks.
#[derive(Clone, Eq, PartialEq)]
pub enum NvramConfig {
    Empty,
    Snapshot(Vec<u8>),
}

impl NvramConfig {
    pub(crate) fn is_protected(&self) -> bool {
        matches!(self, Self::Snapshot(bytes) if bytes.starts_with(SNAPSHOT_MAGIC)
            && bytes.get(8..12) == Some(&(SnapshotFormat::Protected as u32).to_le_bytes()))
    }
}

impl fmt::Debug for NvramConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => f.write_str("Empty"),
            Self::Snapshot(bytes) => f
                .debug_struct("Snapshot")
                .field("bytes", &bytes.len())
                .finish(),
        }
    }
}

impl fmt::Display for NvramContentsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "invalid NVRAM snapshot: {self:?}")
    }
}

impl std::error::Error for NvramContentsError {}

/// Device for the 256 KiB data aperture and its command register page.
pub struct NvramDevice {
    state: Arc<Mutex<State>>,
}

/// Read-only host view of both committed banks, used between guest quanta.
pub(crate) struct NvramHandle {
    state: Arc<Mutex<State>>,
}

impl NvramDevice {
    pub fn new() -> Self {
        Self::from_banks(Banks::new(EMPTY_STORE, SnapshotFormat::Legacy))
    }
    /// Initializes a new protected device with an image provided by the trusted host.
    pub fn with_protected_store(image: &[u8]) -> Result<Self, NvramContentsError> {
        validate_protected(image)?;
        Ok(Self::from_banks(Banks::new(
            image,
            SnapshotFormat::Protected,
        )))
    }

    /// Creates a protected store from the applicable host provisioning policy.
    pub fn with_secure_boot_keys(keys: SecureBootKeys<'_>) -> Result<Self, NvramContentsError> {
        Self::with_protected_store(&provision::new_image(keys, None)?)
    }

    /// Applies a cold host reset to the supplied protected snapshot. Ordinary variables persist.
    pub fn reset_secure_boot_keys(
        bytes: &[u8],
        keys: SecureBootKeys<'_>,
    ) -> Result<Self, NvramContentsError> {
        let banks = Banks::with_contents(bytes)?;
        if banks.format != SnapshotFormat::Protected {
            return Err(NvramContentsError::InvalidProtectedStore);
        }
        Self::with_protected_store(&provision::new_image(keys, Some(banks.current().data()))?)
    }

    /// Reopen a snapshot, recovering the highest valid generation.
    pub fn with_contents(bytes: &[u8]) -> Result<Self, NvramContentsError> {
        Ok(Self::from_banks(Banks::with_contents(bytes)?))
    }

    pub fn nvram_snapshot(&self) -> Vec<u8> {
        lock(&self.state).banks.snapshot()
    }
    pub(crate) fn has_protected_store(&self) -> bool {
        lock(&self.state).banks.format == SnapshotFormat::Protected
    }
    pub(crate) fn secure_boot_disabled(&self) -> bool {
        let state = lock(&self.state);
        state.banks.format == SnapshotFormat::Protected
            && Image::new(state.banks.current().data())
                .is_ok_and(|image| image.flags.secure_boot_disabled)
    }

    pub(crate) fn handle(&self) -> NvramHandle {
        NvramHandle {
            state: Arc::clone(&self.state),
        }
    }

    fn from_banks(banks: Banks) -> Self {
        let staging = (banks.format == SnapshotFormat::Protected)
            .then(|| vec![0; NVRAM_STORE_BYTES].into_boxed_slice());
        Self {
            state: Arc::new(Mutex::new(State {
                banks,
                staging,
                address: 0,
                length: 0,
                status: Status::Idle,
                transfer: Transfer::Idle,
                policy_epoch: 0,
            })),
        }
    }
}

impl Default for NvramDevice {
    fn default() -> Self {
        Self::new()
    }
}

impl NvramHandle {
    pub(crate) fn nvram_snapshot(&self) -> Vec<u8> {
        lock(&self.state).banks.snapshot()
    }

    pub(crate) fn read(&self, access: DeviceAccess) -> Result<u64, DeviceError> {
        lock(&self.state).read(access)
    }

    pub(crate) fn write(&self, access: DeviceAccess, value: u64) -> Result<(), DeviceError> {
        lock(&self.state).write(access, value)
    }
}

impl BusDevice for NvramDevice {
    fn read(&mut self, access: DeviceAccess) -> Result<u64, DeviceError> {
        lock(&self.state).read(access)
    }

    fn write(&mut self, access: DeviceAccess, value: u64) -> Result<(), DeviceError> {
        lock(&self.state).write(access, value)
    }
}

fn lock(state: &Mutex<State>) -> MutexGuard<'_, State> {
    state
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[derive(Clone, Copy, Eq, PartialEq)]
#[repr(u32)]
enum Status {
    Idle = 0,
    ReadReady = 1,
    WriteReady = 2,
    Committed = 3,
    ImageCollecting = 4,
    ImageAuthorized = 5,
    ImagePolicyDisabled = 6,
    InvalidRange = 0x8000_0001,
    Incomplete = 0x8000_0002,
    InvalidCommand = 0x8000_0003,
    Exhausted = 0x8000_0004,
    PolicyRejected = 0x8000_0005,
    VariableNotFound = 0x8000_0006,
    OutOfResources = 0x8000_0007,
}

enum Transfer {
    Idle,
    Read { start: usize, end: usize },
    Write { length: usize, written: usize },
    ImageUpload(image::Upload),
    ImageAuthority(Vec<u8>),
}

struct State {
    banks: Banks,
    staging: Option<Box<[u8]>>,
    address: u32,
    length: u32,
    status: Status,
    transfer: Transfer,
    policy_epoch: u64,
}

impl State {
    fn read(&self, access: DeviceAccess) -> Result<u64, DeviceError> {
        validate_access(access)?;
        let offset = access.offset;
        if offset < REGISTERS {
            let (data, start, end) = match &self.transfer {
                Transfer::Read { start, end } => (self.banks.current().data(), *start, *end),
                Transfer::ImageAuthority(bytes) => (&bytes[..], 0, bytes.len()),
                _ => return Err(DeviceError::Busy),
            };
            let offset = offset as usize;
            let length = access.width.bytes() as usize;
            if offset < start || offset + length > end {
                return Err(DeviceError::UnknownOffset);
            }
            let mut value = [0; 8];
            value[..length].copy_from_slice(&data[offset..offset + length]);
            return Ok(u64::from_le_bytes(value));
        }
        match offset {
            ADDR => Ok(u64::from(self.address)),
            LEN => Ok(u64::from(self.length)),
            CMD => Err(DeviceError::WriteOnly),
            STATUS => Ok(u64::from(self.status as u32)),
            ACTIVE_LEN => Ok(self.banks.current().length as u64),
            POLICY_EPOCH_LOW if self.staging.is_some() => Ok(u64::from(self.policy_epoch as u32)),
            POLICY_EPOCH_HIGH if self.staging.is_some() => Ok(self.policy_epoch >> 32),
            _ => Err(DeviceError::UnknownOffset),
        }
    }

    fn write(&mut self, access: DeviceAccess, value: u64) -> Result<(), DeviceError> {
        validate_access(access)?;
        let offset = access.offset;
        if offset < REGISTERS {
            if let Transfer::ImageUpload(upload) = &mut self.transfer {
                let width = access.width.bytes() as usize;
                return upload.write(offset as usize, &value.to_le_bytes()[..width]);
            }
            let Transfer::Write { length, written } = self.transfer else {
                return Err(DeviceError::ReadOnly);
            };
            let width = access.width.bytes() as usize;
            if offset as usize != written || written + width > length {
                return Err(DeviceError::UnknownOffset);
            }
            let destination = match self.staging.as_deref_mut() {
                Some(staging) => staging,
                None => &mut self.banks.inactive().data,
            };
            destination[written..written + width].copy_from_slice(&value.to_le_bytes()[..width]);
            self.transfer = Transfer::Write {
                length,
                written: written + width,
            };
            return Ok(());
        }
        match offset {
            ADDR => self.address = value as u32,
            LEN => self.length = value as u32,
            CMD => self.command(value as u32),
            STATUS | ACTIVE_LEN => return Err(DeviceError::ReadOnly),
            POLICY_EPOCH_LOW | POLICY_EPOCH_HIGH if self.staging.is_some() => {
                return Err(DeviceError::ReadOnly);
            }
            _ => return Err(DeviceError::UnknownOffset),
        }
        Ok(())
    }

    fn command(&mut self, code: u32) {
        let previous = std::mem::replace(&mut self.transfer, Transfer::Idle);
        let start = self.address as usize;
        let length = self.length as usize;
        self.status = match code {
            READ if length != 0 && start.saturating_add(length) <= self.banks.current().length => {
                self.transfer = Transfer::Read {
                    start,
                    end: start + length,
                };
                Status::ReadReady
            }
            WRITE if start == 0 && (1..=NVRAM_STORE_BYTES).contains(&length) => {
                if self.banks.current().generation == u64::MAX {
                    Status::Exhausted
                } else {
                    if self.staging.is_none() {
                        self.banks.inactive().invalidate();
                    }
                    self.transfer = Transfer::Write { length, written: 0 };
                    Status::WriteReady
                }
            }
            READ | WRITE => Status::InvalidRange,
            FLUSH => match previous {
                Transfer::Write { length, written } if written == length => self.flush(length),
                _ => Status::Incomplete,
            },
            AUTH_UPDATE if self.staging.is_some() => match previous {
                Transfer::Write { length, written } if written == length => {
                    self.authenticated_update(length)
                }
                _ => Status::Incomplete,
            },
            IMAGE_BEGIN | IMAGE_CHUNK | IMAGE_VERIFY if self.staging.is_some() => {
                self.image_command(code, previous)
            }
            _ => Status::InvalidCommand,
        };
    }

    fn flush(&mut self, length: usize) -> Status {
        if let Some(staging) = &self.staging {
            let candidate = &staging[..length];
            if !preserves_protected_policy(self.banks.current().data(), candidate) {
                return Status::PolicyRejected;
            }
            self.banks.inactive().data[..length].copy_from_slice(candidate);
        }
        if self.banks.flush(length) {
            Status::Committed
        } else {
            Status::Exhausted
        }
    }

    fn authenticated_update(&mut self, length: usize) -> Status {
        let Some(staged) = &self.staging else {
            return Status::InvalidCommand;
        };
        match authenticated::prepare(self.banks.current().data(), &staged[..length]) {
            Ok(Some(image)) if self.banks.current().generation != u64::MAX => {
                self.banks.inactive().data[..image.len()].copy_from_slice(&image);
                let committed = self.banks.flush(image.len());
                debug_assert!(committed);
                // Each epoch also advances the generation, so it cannot wrap before it.
                self.policy_epoch += 1;
                Status::Committed
            }
            Ok(Some(_)) => Status::Exhausted,
            Ok(None) => Status::Committed,
            Err(authenticated::Error::InvalidRequest) => Status::InvalidRange,
            Err(authenticated::Error::PolicyRejected) => Status::PolicyRejected,
            Err(authenticated::Error::NotFound) => Status::VariableNotFound,
            Err(authenticated::Error::OutOfResources) => Status::OutOfResources,
        }
    }
}

fn validate_protected(bytes: &[u8]) -> Result<(), NvramContentsError> {
    let image = Image::new(bytes).map_err(|_| NvramContentsError::InvalidProtectedStore)?;
    if image.policy_bytes().is_err() || !has_unique_ordinary_names(&image) {
        return Err(NvramContentsError::InvalidProtectedStore);
    }
    Ok(())
}

fn preserves_protected_policy(current: &[u8], candidate: &[u8]) -> bool {
    let (Ok(current), Ok(candidate)) = (Image::new(current), Image::new(candidate)) else {
        return false;
    };
    current.flags == candidate.flags
        && has_unique_ordinary_names(&candidate)
        && matches!((current.policy_bytes(), candidate.policy_bytes()), (Ok(old), Ok(new)) if old == new)
}

fn has_unique_ordinary_names(image: &Image<'_>) -> bool {
    let mut keys = HashSet::new();
    image.records().all(|decoded| {
        decoded.is_ok_and(|(record, _)| {
            record.policy_variable().is_some() || keys.insert((record.guid, record.name))
        })
    })
}

fn validate_access(access: DeviceAccess) -> Result<(), DeviceError> {
    if access.space != IoSpace::Mmio {
        return Err(DeviceError::UnsupportedSpace);
    }
    if access.offset >= NVRAM_MMIO_BYTES {
        return Err(DeviceError::UnknownOffset);
    }
    if access.offset < REGISTERS {
        if access.offset + u64::from(access.width.bytes()) > REGISTERS {
            return Err(DeviceError::CrossesRegister);
        }
    } else if access.width != AccessWidth::Dword {
        return Err(DeviceError::UnsupportedWidth);
    } else if access.offset % 4 != 0 {
        return Err(DeviceError::Misaligned);
    }
    Ok(())
}

struct Bank {
    generation: u64,
    length: usize,
    crc: u32,
    data: Box<[u8]>,
}

impl Bank {
    fn data(&self) -> &[u8] {
        &self.data[..self.length]
    }

    fn invalid() -> Self {
        Self {
            generation: 0,
            length: 0,
            crc: 0,
            data: vec![0; NVRAM_STORE_BYTES].into_boxed_slice(),
        }
    }

    fn invalidate(&mut self) {
        self.generation = 0;
        self.length = 0;
        self.crc = 0;
    }

    fn seal(&mut self, generation: u64, length: usize) {
        self.crc = bank_checksum(generation, &self.data[..length]);
        self.length = length;
        self.generation = generation;
    }

    fn decode(bytes: &[u8]) -> Self {
        let generation = u64::from_le_bytes(bytes[..8].try_into().unwrap());
        let length = u32::from_le_bytes(bytes[8..12].try_into().unwrap()) as usize;
        let crc = u32::from_le_bytes(bytes[12..16].try_into().unwrap());
        let mut bank = Self::invalid();
        if generation != 0 && (1..=NVRAM_STORE_BYTES).contains(&length) {
            let bytes = &bytes[BANK_HEADER_BYTES..BANK_HEADER_BYTES + length];
            if crc == bank_checksum(generation, bytes) {
                bank.generation = generation;
                bank.length = length;
                bank.crc = crc;
                bank.data[..length].copy_from_slice(bytes);
            }
        }
        bank
    }

    fn encode(&self, out: &mut [u8]) {
        out[..8].copy_from_slice(&self.generation.to_le_bytes());
        out[8..12].copy_from_slice(&(self.length as u32).to_le_bytes());
        out[12..16].copy_from_slice(&self.crc.to_le_bytes());
        out[BANK_HEADER_BYTES..BANK_HEADER_BYTES + self.length]
            .copy_from_slice(&self.data[..self.length]);
    }
}

fn bank_checksum(generation: u64, bytes: &[u8]) -> u32 {
    crc32_ieee(&[
        &generation.to_le_bytes(),
        &(bytes.len() as u32).to_le_bytes(),
        bytes,
    ])
}

struct Banks {
    format: SnapshotFormat,
    banks: [Bank; 2],
    active: usize,
}

impl Banks {
    fn new(image: &[u8], format: SnapshotFormat) -> Self {
        let mut first = Bank::invalid();
        first.data[..image.len()].copy_from_slice(image);
        first.seal(1, image.len());
        Self {
            format,
            banks: [first, Bank::invalid()],
            active: 0,
        }
    }

    fn with_contents(bytes: &[u8]) -> Result<Self, NvramContentsError> {
        if bytes.len() != NVRAM_SNAPSHOT_BYTES
            || &bytes[..8] != SNAPSHOT_MAGIC
            || bytes[12..16] != [0; 4]
        {
            return Err(NvramContentsError::InvalidHeader);
        }
        let format = match u32::from_le_bytes(bytes[8..12].try_into().unwrap()) {
            1 => SnapshotFormat::Legacy,
            2 => SnapshotFormat::Protected,
            _ => return Err(NvramContentsError::InvalidHeader),
        };
        let middle = SNAPSHOT_HEADER_BYTES + BANK_BYTES;
        let first_slot = &bytes[SNAPSHOT_HEADER_BYTES..middle];
        let second_slot = &bytes[middle..];
        let first = Bank::decode(first_slot);
        let second = Bank::decode(second_slot);
        if format == SnapshotFormat::Protected {
            for (slot, bank) in [(first_slot, &first), (second_slot, &second)] {
                let padding = if bank.generation != 0 {
                    validate_protected(bank.data())?;
                    &slot[BANK_HEADER_BYTES + bank.length..]
                } else {
                    slot
                };
                if padding.iter().any(|&byte| byte != 0) {
                    return Err(NvramContentsError::InvalidProtectedStore);
                }
            }
        }
        let active = match (first.generation, second.generation) {
            (0, 0) => return Err(NvramContentsError::NoValidBank),
            (a, b) if a == b => return Err(NvramContentsError::AmbiguousGeneration),
            (a, b) => usize::from(b > a),
        };
        Ok(Self {
            format,
            banks: [first, second],
            active,
        })
    }

    fn snapshot(&self) -> Vec<u8> {
        let mut bytes = vec![0; NVRAM_SNAPSHOT_BYTES];
        bytes[..8].copy_from_slice(SNAPSHOT_MAGIC);
        bytes[8..12].copy_from_slice(&(self.format as u32).to_le_bytes());
        for (slot, bank) in bytes[SNAPSHOT_HEADER_BYTES..]
            .chunks_exact_mut(BANK_BYTES)
            .zip(&self.banks)
        {
            bank.encode(slot);
        }
        bytes
    }

    fn current(&self) -> &Bank {
        &self.banks[self.active]
    }

    fn inactive(&mut self) -> &mut Bank {
        &mut self.banks[1 - self.active]
    }

    fn flush(&mut self, length: usize) -> bool {
        let Some(next) = self.current().generation.checked_add(1) else {
            return false;
        };
        self.inactive().seal(next, length);
        self.active = 1 - self.active;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use od_uefi_security::{
        Guid, Name,
        authvar::Timestamp,
        protected::{
            AUTHENTICATED_ATTRIBUTES, Flags, GLOBAL_VARIABLE, HEADER_BYTES,
            IMAGE_SECURITY_DATABASE, Record,
        },
    };

    fn access(offset: u64, width: AccessWidth) -> DeviceAccess {
        DeviceAccess {
            space: IoSpace::Mmio,
            offset,
            width,
        }
    }

    fn reg(device: &mut NvramDevice, offset: u64) -> u64 {
        device.read(access(offset, AccessWidth::Dword)).unwrap()
    }

    fn write_reg(device: &mut NvramDevice, offset: u64, value: u32) {
        device
            .write(access(offset, AccessWidth::Dword), u64::from(value))
            .unwrap();
    }

    fn start(device: &mut NvramDevice, code: u32, address: u32, length: usize) {
        write_reg(device, ADDR, address);
        write_reg(device, LEN, length as u32);
        write_reg(device, CMD, code);
    }

    fn status(device: &mut NvramDevice, expected: Status) {
        assert_eq!(reg(device, STATUS), expected as u64);
    }

    fn read_active(device: &mut NvramDevice) -> Vec<u8> {
        let length = reg(device, ACTIVE_LEN) as usize;
        start(device, READ, 0, length);
        status(device, Status::ReadReady);
        (0..length as u64)
            .map(|offset| device.read(access(offset, AccessWidth::Byte)).unwrap() as u8)
            .collect()
    }

    fn stage(device: &mut NvramDevice, bytes: &[u8]) {
        start(device, WRITE, 0, bytes.len());
        status(device, Status::WriteReady);
        for (offset, &byte) in bytes.iter().enumerate() {
            device
                .write(access(offset as u64, AccessWidth::Byte), u64::from(byte))
                .unwrap();
        }
    }
    fn replace(device: &mut NvramDevice, bytes: &[u8]) {
        stage(device, bytes);
        write_reg(device, CMD, FLUSH);
        status(device, Status::Committed);
    }

    const DEFAULT_FLAGS: Flags = Flags {
        secure_boot_disabled: false,
        vendor_keys_modified: false,
    };

    fn protected_record<'a>(guid: Guid, name: &'a [u8], payload: &'a [u8]) -> Record<'a> {
        Record {
            guid,
            name: Name::new(name).unwrap(),
            value: payload,
            attributes: AUTHENTICATED_ATTRIBUTES,
            watermark: Timestamp::from_bytes(&[
                0xea, 7, 11, 19, 22, 12, 8, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            ]),
        }
    }

    fn protected_image(flags: Flags, records: &[Record<'_>]) -> Vec<u8> {
        let mut image = vec![0; NVRAM_STORE_BYTES];
        let mut length = HEADER_BYTES;
        for record in records {
            length += record.write(&mut image[length..]).unwrap();
        }
        image.truncate(length);
        Image::write_header(&mut image, flags).unwrap();
        image
    }

    #[test]
    fn image_upload_requires_complete_consecutive_chunks_without_writing_banks() {
        let mut device =
            NvramDevice::with_protected_store(&protected_image(DEFAULT_FLAGS, &[])).unwrap();
        let snapshot = device.nvram_snapshot();
        let total = NVRAM_STORE_BYTES + 3;
        start(&mut device, IMAGE_BEGIN, 0, total);
        status(&mut device, Status::ImageCollecting);
        start(&mut device, IMAGE_CHUNK, 0, NVRAM_STORE_BYTES);
        status(&mut device, Status::WriteReady);
        assert!(matches!(
            device.write(access(8, AccessWidth::Qword), 0),
            Err(DeviceError::UnknownOffset)
        ));
        for offset in (0..NVRAM_STORE_BYTES).step_by(8) {
            device
                .write(access(offset as u64, AccessWidth::Qword), offset as u64)
                .unwrap();
        }
        start(&mut device, IMAGE_CHUNK, NVRAM_STORE_BYTES as u32, 3);
        status(&mut device, Status::WriteReady);
        device.write(access(0, AccessWidth::Word), 0x1234).unwrap();
        device.write(access(2, AccessWidth::Byte), 0x56).unwrap();
        start(&mut device, IMAGE_VERIFY, 0, 0);
        status(&mut device, Status::ImagePolicyDisabled);
        assert!(matches!(
            device.read(access(0, AccessWidth::Byte)),
            Err(DeviceError::Busy)
        ));

        for (offset, expected) in [(0, Status::Incomplete), (1, Status::InvalidRange)] {
            start(&mut device, IMAGE_BEGIN, 0, 4);
            status(&mut device, Status::ImageCollecting);
            start(&mut device, IMAGE_CHUNK, offset, 4);
            if offset == 0 {
                status(&mut device, Status::WriteReady);
                device.write(access(0, AccessWidth::Word), 0).unwrap();
                start(&mut device, IMAGE_CHUNK, 2, 2);
            }
            status(&mut device, expected);
            start(&mut device, IMAGE_VERIFY, 0, 0);
            status(&mut device, Status::Incomplete);
        }
        start(&mut device, IMAGE_BEGIN, 0, 64 * 1024 * 1024 + 1);
        status(&mut device, Status::InvalidRange);
        assert_eq!(device.nvram_snapshot(), snapshot);
    }

    #[test]
    fn image_verification_uses_protected_mode_and_drops_inflight_state_on_other_commands() {
        let pk = esl(
            od_uefi_security::siglist::SignatureType::X509,
            include_bytes!("../../../tests/fixtures/firmware/certs/windows-uefi-ca-2023.der"),
        );
        let records = [protected_record(GLOBAL_VARIABLE, b"P\0K\0", &pk)];
        for (disabled, expected) in [
            (false, Status::PolicyRejected),
            (true, Status::ImagePolicyDisabled),
        ] {
            let flags = Flags {
                secure_boot_disabled: disabled,
                ..DEFAULT_FLAGS
            };
            let mut device =
                NvramDevice::with_protected_store(&protected_image(flags, &records)).unwrap();
            let snapshot = device.nvram_snapshot();
            for interrupt in [false, true] {
                start(&mut device, IMAGE_BEGIN, 0, 1);
                status(&mut device, Status::ImageCollecting);
                start(&mut device, IMAGE_CHUNK, 0, 1);
                status(&mut device, Status::WriteReady);
                device.write(access(0, AccessWidth::Byte), 0).unwrap();
                if interrupt {
                    start(&mut device, READ, 0, 1);
                    status(&mut device, Status::ReadReady);
                }
                start(&mut device, IMAGE_VERIFY, 0, 0);
                status(
                    &mut device,
                    if interrupt {
                        Status::Incomplete
                    } else {
                        expected
                    },
                );
                assert_eq!(
                    device.read(access(LEN, AccessWidth::Dword)).unwrap(),
                    if !interrupt && !disabled {
                        od_uefi_security::image_execution::RejectedImageAction::PolicyFailed as u64
                    } else {
                        0
                    }
                );
                assert!(device.read(access(0, AccessWidth::Byte)).is_err());
                assert_eq!(device.nvram_snapshot(), snapshot);
            }
        }
        let mut legacy = NvramDevice::new();
        start(&mut legacy, IMAGE_BEGIN, 0, 1);
        status(&mut legacy, Status::InvalidCommand);
    }

    #[test]
    fn protected_raw_flush_preserves_policy_and_timestamp_in_both_banks() {
        let pk = protected_record(GLOBAL_VARIABLE, b"P\0K\0", b"platform key");
        let dbx = protected_record(IMAGE_SECURITY_DATABASE, b"d\0b\0x\0", b"revocations");
        let initial = protected_image(DEFAULT_FLAGS, &[pk, dbx]);
        let mut device = NvramDevice::with_protected_store(&initial).unwrap();
        assert!(device.has_protected_store());

        let timeout = Record {
            guid: GLOBAL_VARIABLE,
            name: Name::new(b"T\0i\0m\0e\0o\0u\0t\0").unwrap(),
            value: &[7, 0],
            attributes: 7,
            watermark: None,
        };
        let updated = protected_image(DEFAULT_FLAGS, &[dbx, timeout, pk]);
        replace(&mut device, &updated);
        assert_eq!(read_active(&mut device), updated);
        let snapshot = device.nvram_snapshot();
        assert_eq!(&snapshot[8..12], &2_u32.to_le_bytes());

        let attempts = [
            protected_image(DEFAULT_FLAGS, &[pk]),
            protected_image(
                DEFAULT_FLAGS,
                &[
                    Record {
                        value: b"new platform key",
                        ..pk
                    },
                    dbx,
                ],
            ),
            protected_image(
                DEFAULT_FLAGS,
                &[
                    pk,
                    Record {
                        watermark: None,
                        ..dbx
                    },
                ],
            ),
            protected_image(DEFAULT_FLAGS, &[pk, Record { value: b"", ..dbx }]),
            protected_image(DEFAULT_FLAGS, &[pk, pk, dbx]),
            protected_image(DEFAULT_FLAGS, &[pk, dbx, timeout, timeout]),
            protected_image(
                Flags {
                    secure_boot_disabled: true,
                    ..DEFAULT_FLAGS
                },
                &[pk, dbx],
            ),
            protected_image(
                Flags {
                    vendor_keys_modified: true,
                    ..DEFAULT_FLAGS
                },
                &[pk, dbx],
            ),
            protected_image(DEFAULT_FLAGS, &[]),
            EMPTY_STORE.to_vec(),
        ];
        for candidate in attempts {
            stage(&mut device, &candidate);
            assert!(device.nvram_snapshot() == snapshot);
            write_reg(&mut device, CMD, FLUSH);
            status(&mut device, Status::PolicyRejected);
            assert!(device.nvram_snapshot() == snapshot);
        }
        start(&mut device, WRITE, 0, updated.len());
        status(&mut device, Status::WriteReady);
        write_reg(&mut device, CMD, FLUSH);
        status(&mut device, Status::Incomplete);
        assert!(device.nvram_snapshot() == snapshot);

        let mut reopened = NvramDevice::with_contents(&snapshot).unwrap();
        assert!(reopened.has_protected_store());
        assert_eq!(read_active(&mut reopened), updated);
        assert!(reopened.nvram_snapshot() == snapshot);
    }

    #[test]
    fn protected_reopen_rejects_corruption_instead_of_loading_an_older_policy() {
        let pk = protected_record(GLOBAL_VARIABLE, b"P\0K\0", b"platform key");
        let initial = protected_image(DEFAULT_FLAGS, &[pk]);
        let mut device = NvramDevice::with_protected_store(&initial).unwrap();
        replace(&mut device, &initial);
        let snapshot = device.nvram_snapshot();
        let slot = SNAPSHOT_HEADER_BYTES + BANK_BYTES;
        for flip in [
            slot + BANK_HEADER_BYTES + initial.len() - 1,
            slot + BANK_HEADER_BYTES + initial.len(),
        ] {
            let mut corrupted = snapshot.clone();
            corrupted[flip] ^= 1;
            assert_eq!(
                NvramDevice::with_contents(&corrupted).err(),
                Some(NvramContentsError::InvalidProtectedStore)
            );
        }
        let mut erased_header = snapshot;
        erased_header[slot..slot + BANK_HEADER_BYTES].fill(0);
        assert_eq!(
            NvramDevice::with_contents(&erased_header).err(),
            Some(NvramContentsError::InvalidProtectedStore)
        );

        // Even a valid bank envelope cannot turn a non-protected image into protected state.
        let v1_in_v2 = Banks::new(EMPTY_STORE, SnapshotFormat::Protected).snapshot();
        assert_eq!(
            NvramDevice::with_contents(&v1_in_v2).err(),
            Some(NvramContentsError::InvalidProtectedStore)
        );
        assert_eq!(
            NvramDevice::with_protected_store(EMPTY_STORE).err(),
            Some(NvramContentsError::InvalidProtectedStore)
        );
    }

    #[test]
    fn raw_writes_cannot_enroll_platform_key_in_a_fresh_protected_store() {
        let initial = protected_image(DEFAULT_FLAGS, &[]);
        let mut device = NvramDevice::with_protected_store(&initial).unwrap();
        let snapshot = device.nvram_snapshot();
        let staged = protected_image(
            DEFAULT_FLAGS,
            &[protected_record(GLOBAL_VARIABLE, b"P\0K\0", b"key")],
        );
        stage(&mut device, &staged);
        write_reg(&mut device, CMD, FLUSH);
        status(&mut device, Status::PolicyRejected);
        assert!(device.nvram_snapshot() == snapshot);
        assert_eq!(read_active(&mut device), initial);
    }

    fn esl(kind: od_uefi_security::siglist::SignatureType, value: &[u8]) -> Vec<u8> {
        let mut list = kind.guid_bytes().to_vec();
        list.extend(((28 + 16 + value.len()) as u32).to_le_bytes());
        list.extend([0; 4]);
        list.extend(((16 + value.len()) as u32).to_le_bytes());
        list.extend([0xf0; 16]);
        list.extend(value);
        list
    }

    fn auth_request(guid: Guid, name: &[u8], append: bool, descriptor: &[u8]) -> Vec<u8> {
        let attributes = AUTHENTICATED_ATTRIBUTES | if append { 0x40 } else { 0 };
        let mut wire = attributes.to_le_bytes().to_vec();
        wire.extend(((name.len() / 2) as u16).to_le_bytes());
        wire.extend([0; 2]);
        wire.extend(guid.data1.to_le_bytes());
        wire.extend(guid.data2.to_le_bytes());
        wire.extend(guid.data3.to_le_bytes());
        wire.extend(guid.data4);
        wire.extend(name);
        wire.extend(descriptor);
        wire
    }

    fn unsigned_auth2(second: u8, value: &[u8]) -> Vec<u8> {
        let mut bytes = vec![0xea, 7, 11, 19, 22, 40, second, 0, 0, 0, 0, 0, 0, 0, 0, 0];
        bytes.extend([24, 0, 0, 0, 0, 2, 0xf1, 0x0e]);
        bytes.extend([
            0x9d, 0xd2, 0xaf, 0x4a, 0xdf, 0x68, 0xee, 0x49, 0x8a, 0xa9, 0x34, 0x7d, 0x37, 0x56,
            0x65, 0xa7,
        ]);
        bytes.extend(value);
        bytes
    }

    fn submit_auth(device: &mut NvramDevice, request: &[u8], expected: Status) {
        stage(device, request);
        write_reg(device, CMD, AUTH_UPDATE);
        status(device, expected);
    }

    #[test]
    fn policy_epoch_detects_direct_auth2_commits_even_when_the_value_returns() {
        use od_uefi_security::siglist::SignatureType;

        let start_image = protected_image(DEFAULT_FLAGS, &[]);
        let mut device = NvramDevice::with_protected_store(&start_image).unwrap();
        assert_eq!(reg(&mut device, POLICY_EPOCH_LOW), 0);
        assert_eq!(reg(&mut device, POLICY_EPOCH_HIGH), 0);
        assert!(matches!(
            device.write(access(POLICY_EPOCH_LOW, AccessWidth::Dword), 1),
            Err(DeviceError::ReadOnly)
        ));
        let digest_a = esl(SignatureType::Sha256, &[0xa1; 32]);
        let digest_b = esl(SignatureType::Sha256, &[0xb1; 32]);
        for (index, value) in [&digest_a, &digest_b, &digest_a].into_iter().enumerate() {
            let auth = unsigned_auth2(index as u8 + 1, value);
            let request = auth_request(IMAGE_SECURITY_DATABASE, b"d\0b\0x\0", false, &auth);
            submit_auth(&mut device, &request, Status::Committed);
            assert_eq!(reg(&mut device, POLICY_EPOCH_LOW), index as u64 + 1);
        }
        submit_auth(&mut device, &[0], Status::InvalidRange);
        let active = read_active(&mut device);
        replace(&mut device, &active);
        assert_eq!(reg(&mut device, POLICY_EPOCH_LOW), 3);
        let mut reopened = NvramDevice::with_contents(&device.nvram_snapshot()).unwrap();
        assert_eq!(reg(&mut reopened, POLICY_EPOCH_LOW), 0);
        assert_eq!(read_active(&mut reopened), active);
        assert!(matches!(
            NvramDevice::new().read(access(POLICY_EPOCH_LOW, AccessWidth::Dword)),
            Err(DeviceError::UnknownOffset)
        ));
    }

    #[test]
    fn authenticated_mmio_applies_microsoft_dbx_only_with_an_enrolled_kek() {
        use od_uefi_security::{
            authvar::Descriptor, protected::PolicyVariable, siglist::SignatureType,
        };

        let pk_pem =
            include_bytes!("../../../tests/fixtures/firmware/certs/windows-uefi-ca-2023.der");
        let kek_2023 =
            include_bytes!("../../../tests/fixtures/firmware/certs/microsoft-kek-ca-2023.der");
        let kek_2011 =
            include_bytes!("../../../tests/fixtures/firmware/certs/microsoft-kek-ca-2011.der");
        let update = include_bytes!(
            "../../../tests/fixtures/firmware/certs/dbx-update-x64-v1.7.2-signed.efiauth2"
        );
        let pk_list = esl(SignatureType::X509, pk_pem);
        let request = auth_request(IMAGE_SECURITY_DATABASE, b"d\0b\0x\0", true, update);
        let mut tampered = request.clone();
        *tampered.last_mut().unwrap() ^= 1;
        for (kek, expected) in [
            (kek_2011.as_slice(), Status::PolicyRejected),
            (kek_2023.as_slice(), Status::Committed),
        ] {
            let kek_list = esl(SignatureType::X509, kek);
            let initial = protected_image(
                DEFAULT_FLAGS,
                &[
                    protected_record(GLOBAL_VARIABLE, b"P\0K\0", &pk_list),
                    protected_record(GLOBAL_VARIABLE, b"K\0E\0K\0", &kek_list),
                ],
            );
            let mut device = NvramDevice::with_protected_store(&initial).unwrap();
            let original = device.nvram_snapshot();
            submit_auth(&mut device, &tampered, Status::PolicyRejected);
            assert!(device.nvram_snapshot() == original);
            submit_auth(&mut device, &request, expected);
            let snapshot = device.nvram_snapshot();
            if expected == Status::PolicyRejected {
                assert!(snapshot == original);
                continue;
            }
            let mut device = NvramDevice::with_contents(&snapshot).unwrap();
            let current = read_active(&mut device);
            let image = Image::new(&current).unwrap();
            let dbx = image
                .records()
                .map(Result::unwrap)
                .map(|(record, _)| record)
                .find(|record| record.policy_variable() == Some(PolicyVariable::Dbx))
                .unwrap();
            let descriptor = Descriptor::decode(update, true).unwrap();
            assert_eq!(dbx.value, descriptor.value);
            assert_eq!(dbx.watermark, descriptor.timestamp);
            assert_eq!(image.flags, DEFAULT_FLAGS);
            // An identical, valid APPEND performs no rewrite or generation bump.
            submit_auth(&mut device, &request, Status::Committed);
            assert!(device.nvram_snapshot() == snapshot);
        }
    }

    #[test]
    fn setup_mode_checks_layout_and_watermark_and_user_mode_requires_signatures() {
        use od_uefi_security::{
            authvar::Descriptor, protected::PolicyVariable, siglist::SignatureType,
        };

        let initial = protected_image(DEFAULT_FLAGS, &[]);
        let mut device = NvramDevice::with_protected_store(&initial).unwrap();
        let add = |second, append, value: &[u8]| {
            auth_request(
                IMAGE_SECURITY_DATABASE,
                b"d\0b\0x\0",
                append,
                &unsigned_auth2(second, value),
            )
        };
        let first = esl(SignatureType::Sha256, &[0x31; 32]);
        let extra = esl(SignatureType::Sha256, &[0x42; 32]);
        let mut malformed_signed_data = unsigned_auth2(10, &first);
        malformed_signed_data[16] = 25;
        malformed_signed_data.insert(40, 0xff);
        let before = device.nvram_snapshot();
        let malformed = auth_request(
            IMAGE_SECURITY_DATABASE,
            b"d\0b\0x\0",
            false,
            &malformed_signed_data,
        );
        submit_auth(&mut device, &malformed, Status::PolicyRejected);
        assert!(device.nvram_snapshot() == before);
        submit_auth(&mut device, &add(10, false, &first), Status::Committed);
        let committed = device.nvram_snapshot();
        submit_auth(&mut device, &add(9, false, &extra), Status::PolicyRejected);
        assert!(device.nvram_snapshot() == committed);
        submit_auth(&mut device, &add(9, true, &extra), Status::Committed);
        let current = read_active(&mut device);
        let entry = Image::new(&current)
            .unwrap()
            .records()
            .next()
            .unwrap()
            .unwrap()
            .0;
        assert_eq!(entry.value, [first.as_slice(), extra.as_slice()].concat());
        assert_eq!(
            entry.watermark,
            Descriptor::decode(&unsigned_auth2(10, &first), false)
                .unwrap()
                .timestamp
        );

        let before_append = device.nvram_snapshot();
        submit_auth(&mut device, &add(11, true, &first), Status::Committed);
        assert!(device.nvram_snapshot() != before_append);
        let current = read_active(&mut device);
        let entry = Image::new(&current)
            .unwrap()
            .records()
            .next()
            .unwrap()
            .unwrap()
            .0;
        assert_eq!(entry.value, [first.as_slice(), extra.as_slice()].concat());
        assert_eq!(
            entry.watermark,
            Descriptor::decode(&unsigned_auth2(11, &first), true)
                .unwrap()
                .timestamp
        );

        submit_auth(&mut device, &add(12, false, &[]), Status::Committed);
        let tombstoned = device.nvram_snapshot();
        let mut device = NvramDevice::with_contents(&tombstoned).unwrap();
        submit_auth(&mut device, &add(10, false, &first), Status::PolicyRejected);
        assert!(device.nvram_snapshot() == tombstoned);
        let current = read_active(&mut device);
        let image = Image::new(&current).unwrap();
        let entry = image.records().next().unwrap().unwrap().0;
        assert_eq!(entry.policy_variable(), Some(PolicyVariable::Dbx));
        assert!(entry.value.is_empty());
        assert!(image.flags.vendor_keys_modified);

        let bad_pk = esl(SignatureType::X509, b"not a certificate");
        let set_pk = |certificate: &[u8]| {
            auth_request(
                GLOBAL_VARIABLE,
                b"P\0K\0",
                false,
                &unsigned_auth2(12, certificate),
            )
        };
        submit_auth(&mut device, &set_pk(&bad_pk), Status::InvalidRange);
        assert!(device.nvram_snapshot() == tombstoned);
        let valid_pk =
            include_bytes!("../../../tests/fixtures/firmware/certs/windows-uefi-ca-2023.der");
        submit_auth(
            &mut device,
            &set_pk(&esl(SignatureType::X509, valid_pk)),
            Status::Committed,
        );
        let user_mode = device.nvram_snapshot();
        submit_auth(&mut device, &add(13, false, &first), Status::PolicyRejected);
        assert!(device.nvram_snapshot() == user_mode);
    }

    #[test]
    fn auth_update_capacity_and_legacy_refusals_leave_committed_banks_intact() {
        use od_uefi_security::siglist::SignatureType;

        let large_esl = |entries: usize| {
            let entry = esl(SignatureType::Sha256, &[8; 32]);
            let mut value = entry[..28].to_vec();
            value[16..20].copy_from_slice(&((28 + 48 * entries) as u32).to_le_bytes());
            for _ in 0..entries {
                value.extend(&entry[28..]);
            }
            value
        };
        let existing = large_esl(1250);
        let initial = protected_image(
            DEFAULT_FLAGS,
            &[
                protected_record(IMAGE_SECURITY_DATABASE, b"d\0b\0", &existing),
                protected_record(IMAGE_SECURITY_DATABASE, b"d\0b\0r\0", &existing),
            ],
        );
        let update = auth_request(
            IMAGE_SECURITY_DATABASE,
            b"d\0b\0x\0",
            false,
            &unsigned_auth2(20, &large_esl(250)),
        );
        let mut protected = NvramDevice::with_protected_store(&initial).unwrap();
        let snapshot = protected.nvram_snapshot();
        submit_auth(&mut protected, &update, Status::OutOfResources);
        assert!(protected.nvram_snapshot() == snapshot);

        let mut legacy = NvramDevice::new();
        let snapshot = legacy.nvram_snapshot();
        submit_auth(&mut legacy, &update, Status::InvalidCommand);
        assert!(legacy.nvram_snapshot() == snapshot);
    }

    #[test]
    fn cold_secure_boot_provisioning_and_reset_preserve_ordinary_variables() {
        use od_uefi_security::{protected::PolicyVariable, siglist::signature_lists};

        let first_pk =
            include_bytes!("../../../tests/fixtures/firmware/certs/windows-uefi-ca-2023.der");
        let next_pk =
            include_bytes!("../../../tests/fixtures/firmware/certs/microsoft-uefi-ca-2023.der");
        let keys = SecureBootKeys::Defaults {
            public_platform_key: first_pk,
        };
        let mut device = NvramDevice::with_secure_boot_keys(keys).unwrap();
        let initial = read_active(&mut device);
        let image = Image::new(&initial).unwrap();
        let records: Vec<_> = image
            .records()
            .map(Result::unwrap)
            .map(|(record, _)| record)
            .collect();
        assert_eq!(image.flags, DEFAULT_FLAGS);
        assert_eq!(records.len(), 4);
        assert!(records.iter().all(|record| record.watermark.is_none()));
        let expected = [
            (PolicyVariable::Pk, 1),
            (PolicyVariable::Kek, 2),
            (PolicyVariable::Db, 4),
            (PolicyVariable::Dbx, 445),
        ];
        for (record, (kind, count)) in records.iter().zip(expected) {
            assert_eq!(record.policy_variable(), Some(kind));
            assert_eq!(
                signature_lists(record.value)
                    .map(Result::unwrap)
                    .flat_map(|list| list.signatures())
                    .count(),
                count
            );
        }
        assert_eq!(
            signature_lists(records[0].value)
                .next()
                .unwrap()
                .unwrap()
                .signatures()
                .next()
                .unwrap()
                .data(),
            first_pk
        );
        assert_eq!(
            records[3].value,
            include_bytes!("../../../tests/fixtures/firmware/certs/dbx-x64-v1.7.2.bin")
        );

        let ordinary = Record {
            guid: GLOBAL_VARIABLE,
            name: Name::new(b"T\0i\0m\0e\0o\0u\0t\0").unwrap(),
            attributes: 7,
            value: &[12, 0],
            watermark: None,
        };
        let mut entries = records;
        entries.push(ordinary);
        replace(&mut device, &protected_image(DEFAULT_FLAGS, &entries));
        let snapshot = device.nvram_snapshot();
        assert_eq!(
            NvramDevice::reset_secure_boot_keys(&snapshot, keys).err(),
            Some(NvramContentsError::ReusedPlatformKey)
        );
        let new_keys = SecureBootKeys::Defaults {
            public_platform_key: next_pk,
        };
        let mut replaced = NvramDevice::reset_secure_boot_keys(&snapshot, new_keys).unwrap();
        let replacement = read_active(&mut replaced);
        let new_image = Image::new(&replacement).unwrap();
        let mut records = new_image
            .records()
            .map(Result::unwrap)
            .map(|(record, _)| record);
        assert_eq!(records.next().unwrap().value, ordinary.value);
        let new_platform_key = records.next().unwrap();
        assert_eq!(new_platform_key.policy_variable(), Some(PolicyVariable::Pk));
        assert_eq!(
            signature_lists(new_platform_key.value)
                .next()
                .unwrap()
                .unwrap()
                .signatures()
                .next()
                .unwrap()
                .data(),
            next_pk
        );

        let mut setup =
            NvramDevice::reset_secure_boot_keys(&snapshot, SecureBootKeys::SetupMode).unwrap();
        let setup_image = read_active(&mut setup);
        let image = Image::new(&setup_image).unwrap();
        assert_eq!(image.policy_bytes().unwrap().iter().flatten().count(), 0);
        assert_eq!(
            image.records().next().unwrap().unwrap().0.value,
            ordinary.value
        );
        assert_eq!(image.flags, DEFAULT_FLAGS);
        assert!(device.nvram_snapshot() == snapshot);

        let legacy = NvramDevice::new().nvram_snapshot();
        assert_eq!(
            NvramDevice::reset_secure_boot_keys(&legacy, new_keys).err(),
            Some(NvramContentsError::InvalidProtectedStore)
        );
        let invalid = SecureBootKeys::Defaults {
            public_platform_key: b"not a DER certificate",
        };
        assert_eq!(
            NvramDevice::with_secure_boot_keys(invalid).err(),
            Some(NvramContentsError::InvalidPlatformKey)
        );
    }

    #[test]
    fn starts_with_the_canonical_empty_store_and_idle_registers() {
        let mut device = NvramDevice::new();
        status(&mut device, Status::Idle);
        assert_eq!(reg(&mut device, ADDR), 0);
        assert_eq!(reg(&mut device, LEN), 0);
        assert_eq!(read_active(&mut device), EMPTY_STORE);
        let mut crc_input = EMPTY_STORE.to_vec();
        crc_input[16..20].fill(0);
        assert_eq!(crc32_ieee(&[&crc_input]).to_le_bytes(), EMPTY_STORE[16..20]);
    }

    #[test]
    fn complete_flush_switches_generation_and_obeys_little_endian_width() {
        let mut device = NvramDevice::new();
        start(&mut device, WRITE, 0, 8);
        device
            .write(access(0, AccessWidth::Qword), 0x8877_6655_4433_2211)
            .unwrap();
        assert_eq!(reg(&mut device, ACTIVE_LEN), EMPTY_STORE.len() as u64);
        write_reg(&mut device, CMD, FLUSH);
        status(&mut device, Status::Committed);
        start(&mut device, READ, 2, 4);
        let value = device.read(access(2, AccessWidth::Dword)).unwrap();
        assert_eq!(value, 0x6655_4433);
        assert_eq!(
            device.read(access(1, AccessWidth::Dword)),
            Err(DeviceError::UnknownOffset)
        );
        let envelope = device.nvram_snapshot();
        let mut restored = NvramDevice::with_contents(&envelope).unwrap();
        assert_eq!(
            read_active(&mut restored),
            0x8877_6655_4433_2211u64.to_le_bytes()
        );
        assert_eq!(restored.nvram_snapshot(), envelope);
    }

    #[test]
    fn interrupted_or_incomplete_write_restores_the_last_committed_bank() {
        let mut device = NvramDevice::new();
        replace(&mut device, b"previous committed bytes");
        start(&mut device, WRITE, 0, 8);
        device
            .write(access(0, AccessWidth::Byte), b'x'.into())
            .unwrap();
        let mut recovered = NvramDevice::with_contents(&device.nvram_snapshot()).unwrap();
        status(&mut recovered, Status::Idle);
        assert_eq!(read_active(&mut recovered), b"previous committed bytes");
        write_reg(&mut device, CMD, FLUSH);
        status(&mut device, Status::Incomplete);
        assert_eq!(read_active(&mut device), b"previous committed bytes");
    }

    #[test]
    fn bad_newer_bank_crc_falls_back_to_the_previous_generation() {
        let mut device = NvramDevice::new();
        replace(&mut device, b"first");
        replace(&mut device, b"second");
        let mut bytes = device.nvram_snapshot();
        bytes[SNAPSHOT_HEADER_BYTES + BANK_HEADER_BYTES + 2] ^= 1;
        let mut recovered = NvramDevice::with_contents(&bytes).unwrap();
        assert_eq!(read_active(&mut recovered), b"first");
        bytes[SNAPSHOT_HEADER_BYTES + BANK_BYTES + BANK_HEADER_BYTES] ^= 1;
        assert!(matches!(
            NvramDevice::with_contents(&bytes),
            Err(NvramContentsError::NoValidBank)
        ));
    }

    #[test]
    fn invalid_guest_accesses_preserve_latched_state_and_staged_progress() {
        let mut device = NvramDevice::new();
        start(&mut device, WRITE, 0, 4);
        assert_eq!(
            device.write(access(1, AccessWidth::Byte), 0),
            Err(DeviceError::UnknownOffset)
        );
        assert_eq!(
            device.write(access(CMD, AccessWidth::Word), FLUSH.into()),
            Err(DeviceError::UnsupportedWidth)
        );
        assert_eq!(
            device.read(access(STATUS + 1, AccessWidth::Dword)),
            Err(DeviceError::Misaligned)
        );
        assert_eq!(
            device.read(access(REGISTERS - 1, AccessWidth::Word)),
            Err(DeviceError::CrossesRegister)
        );
        assert_eq!(
            device.write(access(STATUS, AccessWidth::Dword), 0),
            Err(DeviceError::ReadOnly)
        );
        assert_eq!(
            device.read(access(CMD, AccessWidth::Dword)),
            Err(DeviceError::WriteOnly)
        );
        assert_eq!(
            device.read(access(NVRAM_MMIO_BYTES, AccessWidth::Dword)),
            Err(DeviceError::UnknownOffset)
        );
        status(&mut device, Status::WriteReady);
        device
            .write(access(0, AccessWidth::Dword), 0x0403_0201)
            .unwrap();
        write_reg(&mut device, CMD, FLUSH);
        status(&mut device, Status::Committed);
        assert_eq!(read_active(&mut device), [1, 2, 3, 4]);
    }

    #[test]
    fn range_and_generation_errors_leave_active_data_intact() {
        let mut banks = Banks::new(EMPTY_STORE, SnapshotFormat::Legacy);
        banks.banks[0].seal(u64::MAX, EMPTY_STORE.len());
        let mut device = NvramDevice::with_contents(&banks.snapshot()).unwrap();
        start(&mut device, READ, u32::MAX, 8);
        status(&mut device, Status::InvalidRange);
        start(&mut device, WRITE, 1, 8);
        status(&mut device, Status::InvalidRange);
        start(&mut device, WRITE, 0, 8);
        status(&mut device, Status::Exhausted);
        assert_eq!(read_active(&mut device), EMPTY_STORE);
    }

    #[test]
    fn rejects_truncated_unversioned_and_ambiguous_host_snapshots() {
        let mut banks = Banks::new(EMPTY_STORE, SnapshotFormat::Legacy);
        let mut snapshot = banks.snapshot();
        assert!(matches!(
            NvramDevice::with_contents(&snapshot[..15]),
            Err(NvramContentsError::InvalidHeader)
        ));
        snapshot[8] ^= 1;
        assert!(matches!(
            NvramDevice::with_contents(&snapshot),
            Err(NvramContentsError::InvalidHeader)
        ));
        banks.banks[1].data[0] = 1;
        banks.banks[1].seal(1, 1);
        assert!(matches!(
            NvramDevice::with_contents(&banks.snapshot()),
            Err(NvramContentsError::AmbiguousGeneration)
        ));
    }
}
