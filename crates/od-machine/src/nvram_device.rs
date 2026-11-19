//! Two committed generations of opaque firmware variable bytes.

use crate::bus::{AccessWidth, BusDevice, DeviceAccess, DeviceError, IoSpace};
use crate::crc32::crc32_ieee;
use std::fmt;
use std::sync::{Arc, Mutex, MutexGuard};

pub const NVRAM_MMIO_BASE: u64 = 0xff00_0000;
pub const NVRAM_STORE_BYTES: usize = 256 * 1024;
pub const NVRAM_MMIO_BYTES: u64 = NVRAM_STORE_BYTES as u64 + 0x1000;
const REGISTERS: u64 = NVRAM_STORE_BYTES as u64;
const ADDR: u64 = REGISTERS;
const LEN: u64 = REGISTERS + 4;
const CMD: u64 = REGISTERS + 8;
const STATUS: u64 = REGISTERS + 12;
const ACTIVE_LEN: u64 = REGISTERS + 16;

const READ: u32 = 1;
const WRITE: u32 = 2;
const FLUSH: u32 = 3;

const SNAPSHOT_MAGIC: &[u8; 8] = b"ODNVBANK";
const SNAPSHOT_VERSION: u32 = 1;
const SNAPSHOT_HEADER_BYTES: usize = 16;
const BANK_HEADER_BYTES: usize = 16;
const BANK_BYTES: usize = BANK_HEADER_BYTES + NVRAM_STORE_BYTES;
const SNAPSHOT_BYTES: usize = SNAPSHOT_HEADER_BYTES + 2 * BANK_BYTES;
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
}

/// Initial bytes for a new board instance. Every board gets its own banks.
#[derive(Clone, Eq, PartialEq)]
pub enum NvramConfig {
    Empty,
    Snapshot(Vec<u8>),
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
        Self::from_banks(Banks::new())
    }

    /// Reopen a snapshot, recovering the highest valid generation.
    pub fn with_contents(bytes: &[u8]) -> Result<Self, NvramContentsError> {
        Ok(Self::from_banks(Banks::with_contents(bytes)?))
    }

    pub fn nvram_snapshot(&self) -> Vec<u8> {
        lock(&self.state).banks.snapshot()
    }

    pub(crate) fn handle(&self) -> NvramHandle {
        NvramHandle {
            state: Arc::clone(&self.state),
        }
    }

    fn from_banks(banks: Banks) -> Self {
        Self {
            state: Arc::new(Mutex::new(State {
                banks,
                address: 0,
                length: 0,
                status: Status::Idle,
                transfer: Transfer::Idle,
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
    InvalidRange = 0x8000_0001,
    Incomplete = 0x8000_0002,
    InvalidCommand = 0x8000_0003,
    Exhausted = 0x8000_0004,
}

#[derive(Clone, Copy)]
enum Transfer {
    Idle,
    Read { start: usize, end: usize },
    Write { length: usize, written: usize },
}

struct State {
    banks: Banks,
    address: u32,
    length: u32,
    status: Status,
    transfer: Transfer,
}

impl State {
    fn read(&self, access: DeviceAccess) -> Result<u64, DeviceError> {
        validate_access(access)?;
        let offset = access.offset;
        if offset < REGISTERS {
            let Transfer::Read { start, end } = self.transfer else {
                return Err(DeviceError::Busy);
            };
            let offset = offset as usize;
            let length = access.width.bytes() as usize;
            if offset < start || offset + length > end {
                return Err(DeviceError::UnknownOffset);
            }
            let mut value = [0; 8];
            value[..length].copy_from_slice(&self.banks.current().data[offset..offset + length]);
            return Ok(u64::from_le_bytes(value));
        }
        match offset {
            ADDR => Ok(u64::from(self.address)),
            LEN => Ok(u64::from(self.length)),
            CMD => Err(DeviceError::WriteOnly),
            STATUS => Ok(u64::from(self.status as u32)),
            ACTIVE_LEN => Ok(self.banks.current().length as u64),
            _ => Err(DeviceError::UnknownOffset),
        }
    }

    fn write(&mut self, access: DeviceAccess, value: u64) -> Result<(), DeviceError> {
        validate_access(access)?;
        let offset = access.offset;
        if offset < REGISTERS {
            let Transfer::Write { length, written } = self.transfer else {
                return Err(DeviceError::ReadOnly);
            };
            let width = access.width.bytes() as usize;
            if offset as usize != written || written + width > length {
                return Err(DeviceError::UnknownOffset);
            }
            self.banks.inactive().data[written..written + width]
                .copy_from_slice(&value.to_le_bytes()[..width]);
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
                    self.banks.inactive().invalidate();
                    self.transfer = Transfer::Write { length, written: 0 };
                    Status::WriteReady
                }
            }
            READ | WRITE => Status::InvalidRange,
            FLUSH => match previous {
                Transfer::Write { length, written } if written == length => {
                    if self.banks.flush(length) {
                        Status::Committed
                    } else {
                        Status::Exhausted
                    }
                }
                _ => Status::Incomplete,
            },
            _ => Status::InvalidCommand,
        };
    }
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
    banks: [Bank; 2],
    active: usize,
}

impl Banks {
    fn new() -> Self {
        let mut first = Bank::invalid();
        first.data[..EMPTY_STORE.len()].copy_from_slice(EMPTY_STORE);
        first.seal(1, EMPTY_STORE.len());
        Self {
            banks: [first, Bank::invalid()],
            active: 0,
        }
    }

    fn with_contents(bytes: &[u8]) -> Result<Self, NvramContentsError> {
        if bytes.len() != SNAPSHOT_BYTES
            || &bytes[..8] != SNAPSHOT_MAGIC
            || bytes[8..12] != SNAPSHOT_VERSION.to_le_bytes()
            || bytes[12..16] != [0; 4]
        {
            return Err(NvramContentsError::InvalidHeader);
        }
        let middle = SNAPSHOT_HEADER_BYTES + BANK_BYTES;
        let first = Bank::decode(&bytes[SNAPSHOT_HEADER_BYTES..middle]);
        let second = Bank::decode(&bytes[middle..]);
        let active = match (first.generation, second.generation) {
            (0, 0) => return Err(NvramContentsError::NoValidBank),
            (a, b) if a == b => return Err(NvramContentsError::AmbiguousGeneration),
            (a, b) => usize::from(b > a),
        };
        Ok(Self {
            banks: [first, second],
            active,
        })
    }

    fn snapshot(&self) -> Vec<u8> {
        let mut bytes = vec![0; SNAPSHOT_BYTES];
        bytes[..8].copy_from_slice(SNAPSHOT_MAGIC);
        bytes[8..12].copy_from_slice(&SNAPSHOT_VERSION.to_le_bytes());
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

    fn replace(device: &mut NvramDevice, bytes: &[u8]) {
        start(device, WRITE, 0, bytes.len());
        status(device, Status::WriteReady);
        for (offset, &byte) in bytes.iter().enumerate() {
            device
                .write(access(offset as u64, AccessWidth::Byte), u64::from(byte))
                .unwrap();
        }
        write_reg(device, CMD, FLUSH);
        status(device, Status::Committed);
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
        let mut banks = Banks::new();
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
        let mut banks = Banks::new();
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
