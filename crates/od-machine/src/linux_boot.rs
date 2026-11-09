//! Pure parsing and validation of the Linux x86 `setup_header` of a bzImage.
//!
//! Field offsets follow `Documentation/arch/x86/boot.rst` (Linux 7.0, "The Real-Mode
//! Kernel Header"). This module neither places the kernel nor touches a guest.
//!
//! `acpi_rsdp_addr` is intentionally absent: it is a `boot_params` field (offset 0x70),
//! not part of `setup_header`, so it belongs to the boot_params serializer (L3).

/// Smallest file prefix that holds every parsed field (`handover_offset` ends at 0x268).
pub(crate) const LINUX_SETUP_HEADER_MIN_BYTES: usize = 0x268;
/// Minimum supported boot protocol version (2.14 is "burnt", 2.15 is what Linux 7.0 reports;
/// 0x020E is the floor chosen by the S4-37L plan).
pub(crate) const LINUX_MIN_BOOT_PROTOCOL: u16 = 0x020E;
/// Required `boot_flag` value at 0x1FE.
pub(crate) const LINUX_BOOT_FLAG: u16 = 0xAA55;
/// `header` magic at 0x202, little-endian "HdrS".
pub(crate) const LINUX_HDRS_MAGIC: [u8; 4] = *b"HdrS";
/// `loadflags` bit 0.
pub(crate) const LINUX_LOADFLAGS_LOADED_HIGH: u8 = 1;
/// `xloadflags` bit 0: legacy 64-bit entry point at offset 0x200 of the protected-mode code.
pub(crate) const LINUX_XLF_KERNEL_64: u16 = 1;
/// Smallest accepted `kernel_alignment`.
pub(crate) const LINUX_MIN_KERNEL_ALIGNMENT: u32 = 0x1000;
/// Sanity upper bound on `init_size`. The real kernel window (`pref_address + init_size`)
/// is checked against guest RAM by the placement planner (L2), not here.
pub(crate) const LINUX_MAX_INIT_SIZE: u32 = 0x7FFF_FFFF;
/// Bytes per setup sector.
pub(crate) const LINUX_SECTOR_BYTES: u64 = 512;

const OFF_SETUP_SECTS: usize = 0x1F1;
const OFF_SYSSIZE: usize = 0x1F4;
const OFF_BOOT_FLAG: usize = 0x1FE;
const OFF_HEADER: usize = 0x202;
const OFF_VERSION: usize = 0x206;
const OFF_REALMODE_SWTCH: usize = 0x208;
const OFF_START_SYS_SEG: usize = 0x20C;
const OFF_KERNEL_VERSION: usize = 0x20E;
const OFF_TYPE_OF_LOADER: usize = 0x210;
const OFF_LOADFLAGS: usize = 0x211;
const OFF_SETUP_MOVE_SIZE: usize = 0x212;
const OFF_CODE32_START: usize = 0x214;
const OFF_RAMDISK_IMAGE: usize = 0x218;
const OFF_RAMDISK_SIZE: usize = 0x21C;
const OFF_BOOTSECT_KLUDGE: usize = 0x220;
const OFF_HEAP_END_PTR: usize = 0x224;
const OFF_EXT_LOADER_VER: usize = 0x226;
const OFF_EXT_LOADER_TYPE: usize = 0x227;
const OFF_CMD_LINE_PTR: usize = 0x228;
const OFF_INITRD_ADDR_MAX: usize = 0x22C;
const OFF_KERNEL_ALIGNMENT: usize = 0x230;
const OFF_RELOCATABLE_KERNEL: usize = 0x234;
const OFF_MIN_ALIGNMENT: usize = 0x235;
const OFF_XLOADFLAGS: usize = 0x236;
const OFF_CMDLINE_SIZE: usize = 0x238;
const OFF_HARDWARE_SUBARCH: usize = 0x23C;
const OFF_PAYLOAD_OFFSET: usize = 0x248;
const OFF_PAYLOAD_LENGTH: usize = 0x24C;
const OFF_SETUP_DATA: usize = 0x250;
const OFF_PREF_ADDRESS: usize = 0x258;
const OFF_INIT_SIZE: usize = 0x260;
const OFF_HANDOVER_OFFSET: usize = 0x264;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LinuxHeaderError {
    /// File is shorter than `LINUX_SETUP_HEADER_MIN_BYTES`.
    TooShort {
        len: u64,
    },
    BadBootFlag {
        found: u16,
    },
    BadHdrSMagic {
        found: [u8; 4],
    },
    VersionTooOld {
        found: u16,
    },
    /// `loadflags` bit 0 (LOADED_HIGH) is clear.
    NotLoadedHigh {
        loadflags: u8,
    },
    /// `xloadflags` bit 0 (XLF_KERNEL_64) is clear.
    Kernel64NotSupported {
        xloadflags: u16,
    },
    /// Not a power of two or below `LINUX_MIN_KERNEL_ALIGNMENT`.
    InvalidKernelAlignment {
        found: u32,
    },
    /// The planner loads only at `pref_address` (no relocation search), so 0 is unusable.
    PrefAddressZero,
    /// `pref_address` is not a multiple of `kernel_alignment`.
    PrefAddressMisaligned {
        pref_address: u64,
        kernel_alignment: u32,
    },
    /// `(setup_sects + 1) * 512` lies beyond the end of the file.
    SetupOutOfFile {
        protected_mode_offset: u64,
        file_len: u64,
    },
    /// `protected_mode_offset + syssize * 16` lies beyond the end of the file.
    PmCodeOutOfFile {
        protected_mode_offset: u64,
        pm_code_size: u64,
        file_len: u64,
    },
    PayloadEmpty,
    /// Payload range overflows or does not fit inside the protected-mode code (`limit`).
    PayloadOutOfFile {
        payload_offset: u32,
        payload_length: u32,
        limit: u64,
    },
    /// `init_size` is smaller than the protected-mode code size `pm_code_size`.
    InitSizeTooSmall {
        init_size: u32,
        pm_code_size: u64,
    },
    /// `init_size` exceeds `LINUX_MAX_INIT_SIZE`.
    InitSizeTooLarge {
        init_size: u32,
    },
    /// `pref_address + init_size` overflows.
    KernelRangeOverflow {
        pref_address: u64,
        init_size: u32,
    },
    CmdlineSizeZero,
}

/// Validated fields of a bzImage `setup_header`. Only the fields needed by the placement
/// planner are public; the rest are kept for the boot_params serializer and diagnostics.
/// `allow(dead_code)`: the `pub(crate)` fields are consumed by L2-L5, not yet by this slice.
#[allow(dead_code)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LinuxBzImageHeader {
    pub version: u16,
    pub xloadflags: u16,
    pub relocatable: u8,
    pub kernel_alignment: u32,
    pub pref_address: u64,
    pub init_size: u32,
    /// Raw `payload_offset`: offset of the payload from the start of the protected-mode code.
    pub payload_offset: u32,
    pub payload_length: u32,
    /// File offset of the protected-mode code: `(setup_sects + 1) * 512`.
    pub protected_mode_offset: u64,
    /// Size of the protected-mode code in bytes: `syssize * 16`. Copy this many bytes from
    /// `protected_mode_offset` (the file tail may be a signature, not kernel data).
    pub pm_code_size: u64,
    /// Maximum command line length in bytes, without the terminating NUL. The buffer the
    /// loader writes needs `cmdline_size + 1` bytes.
    pub cmdline_size: u32,
    pub initrd_addr_max: u32,

    pub(crate) setup_sects: u8,
    pub(crate) realmode_swtch: u32,
    pub(crate) start_sys_seg: u16,
    pub(crate) kernel_version: u16,
    pub(crate) type_of_loader: u8,
    pub(crate) loadflags: u8,
    pub(crate) setup_move_size: u16,
    pub(crate) code32_start: u32,
    pub(crate) ramdisk_image: u32,
    pub(crate) ramdisk_size: u32,
    pub(crate) bootsect_kludge: u32,
    pub(crate) heap_end_ptr: u16,
    pub(crate) ext_loader_ver: u8,
    pub(crate) ext_loader_type: u8,
    pub(crate) cmd_line_ptr: u32,
    pub(crate) min_alignment: u8,
    pub(crate) hardware_subarch: u32,
    pub(crate) setup_data: u64,
    pub(crate) handover_offset: u32,
}

impl LinuxBzImageHeader {
    /// File offset of the payload: `protected_mode_offset + payload_offset` (checked).
    pub fn payload_file_offset(&self) -> Option<u64> {
        self.protected_mode_offset
            .checked_add(u64::from(self.payload_offset))
    }
}

fn u16_at(b: &[u8], off: usize) -> u16 {
    u16::from_le_bytes([b[off], b[off + 1]])
}

fn u32_at(b: &[u8], off: usize) -> u32 {
    u32::from_le_bytes([b[off], b[off + 1], b[off + 2], b[off + 3]])
}

fn u64_at(b: &[u8], off: usize) -> u64 {
    let mut raw = [0u8; 8];
    raw.copy_from_slice(&b[off..off + 8]);
    u64::from_le_bytes(raw)
}

/// Parses and validates the `setup_header` of a whole bzImage file.
///
/// `pref_address` is required to be a multiple of `kernel_alignment` (not merely 0x1000),
/// because boot.rst derives the runtime start from `pref_address` and `kernel_alignment`.
pub fn parse_linux_bzimage_header(file: &[u8]) -> Result<LinuxBzImageHeader, LinuxHeaderError> {
    if file.len() < LINUX_SETUP_HEADER_MIN_BYTES {
        return Err(LinuxHeaderError::TooShort {
            len: file.len() as u64,
        });
    }
    let file_len = file.len() as u64;

    let boot_flag = u16_at(file, OFF_BOOT_FLAG);
    if boot_flag != LINUX_BOOT_FLAG {
        return Err(LinuxHeaderError::BadBootFlag { found: boot_flag });
    }
    let mut magic = [0u8; 4];
    magic.copy_from_slice(&file[OFF_HEADER..OFF_HEADER + 4]);
    if magic != LINUX_HDRS_MAGIC {
        return Err(LinuxHeaderError::BadHdrSMagic { found: magic });
    }
    let version = u16_at(file, OFF_VERSION);
    if version < LINUX_MIN_BOOT_PROTOCOL {
        return Err(LinuxHeaderError::VersionTooOld { found: version });
    }

    let loadflags = file[OFF_LOADFLAGS];
    if loadflags & LINUX_LOADFLAGS_LOADED_HIGH == 0 {
        return Err(LinuxHeaderError::NotLoadedHigh { loadflags });
    }
    let xloadflags = u16_at(file, OFF_XLOADFLAGS);
    if xloadflags & LINUX_XLF_KERNEL_64 == 0 {
        return Err(LinuxHeaderError::Kernel64NotSupported { xloadflags });
    }

    let kernel_alignment = u32_at(file, OFF_KERNEL_ALIGNMENT);
    if !kernel_alignment.is_power_of_two() || kernel_alignment < LINUX_MIN_KERNEL_ALIGNMENT {
        return Err(LinuxHeaderError::InvalidKernelAlignment {
            found: kernel_alignment,
        });
    }
    let pref_address = u64_at(file, OFF_PREF_ADDRESS);
    if pref_address == 0 {
        return Err(LinuxHeaderError::PrefAddressZero);
    }
    if pref_address % u64::from(kernel_alignment) != 0 {
        return Err(LinuxHeaderError::PrefAddressMisaligned {
            pref_address,
            kernel_alignment,
        });
    }

    // setup_sects == 0 means 4 (boot.rst, footnote 1).
    let raw_setup_sects = file[OFF_SETUP_SECTS];
    let setup_sects = if raw_setup_sects == 0 {
        4
    } else {
        raw_setup_sects
    };
    let protected_mode_offset = (u64::from(setup_sects) + 1) * LINUX_SECTOR_BYTES;
    if protected_mode_offset > file_len {
        return Err(LinuxHeaderError::SetupOutOfFile {
            protected_mode_offset,
            file_len,
        });
    }
    let pm_code_size = u64::from(u32_at(file, OFF_SYSSIZE)) * 16;
    let pm_code_end = protected_mode_offset.checked_add(pm_code_size);
    match pm_code_end {
        Some(end) if end <= file_len => {}
        _ => {
            return Err(LinuxHeaderError::PmCodeOutOfFile {
                protected_mode_offset,
                pm_code_size,
                file_len,
            });
        }
    }

    let payload_offset = u32_at(file, OFF_PAYLOAD_OFFSET);
    let payload_length = u32_at(file, OFF_PAYLOAD_LENGTH);
    if payload_length == 0 {
        return Err(LinuxHeaderError::PayloadEmpty);
    }
    // payload_offset is relative to the start of the protected-mode code, and the payload
    // must lie inside that code (not in the signature tail after it).
    let payload_end = protected_mode_offset
        .checked_add(u64::from(payload_offset))
        .and_then(|start| start.checked_add(u64::from(payload_length)));
    match (payload_end, pm_code_end) {
        (Some(end), Some(limit)) if end <= limit => {}
        (_, limit) => {
            return Err(LinuxHeaderError::PayloadOutOfFile {
                payload_offset,
                payload_length,
                limit: limit.unwrap_or(file_len),
            });
        }
    }

    let init_size = u32_at(file, OFF_INIT_SIZE);
    if init_size > LINUX_MAX_INIT_SIZE {
        return Err(LinuxHeaderError::InitSizeTooLarge { init_size });
    }
    if u64::from(init_size) < pm_code_size {
        return Err(LinuxHeaderError::InitSizeTooSmall {
            init_size,
            pm_code_size,
        });
    }
    if pref_address.checked_add(u64::from(init_size)).is_none() {
        return Err(LinuxHeaderError::KernelRangeOverflow {
            pref_address,
            init_size,
        });
    }

    let cmdline_size = u32_at(file, OFF_CMDLINE_SIZE);
    if cmdline_size == 0 {
        return Err(LinuxHeaderError::CmdlineSizeZero);
    }

    Ok(LinuxBzImageHeader {
        version,
        xloadflags,
        relocatable: file[OFF_RELOCATABLE_KERNEL],
        kernel_alignment,
        pref_address,
        init_size,
        payload_offset,
        payload_length,
        protected_mode_offset,
        pm_code_size,
        cmdline_size,
        initrd_addr_max: u32_at(file, OFF_INITRD_ADDR_MAX),
        setup_sects,
        realmode_swtch: u32_at(file, OFF_REALMODE_SWTCH),
        start_sys_seg: u16_at(file, OFF_START_SYS_SEG),
        kernel_version: u16_at(file, OFF_KERNEL_VERSION),
        type_of_loader: file[OFF_TYPE_OF_LOADER],
        loadflags,
        setup_move_size: u16_at(file, OFF_SETUP_MOVE_SIZE),
        code32_start: u32_at(file, OFF_CODE32_START),
        ramdisk_image: u32_at(file, OFF_RAMDISK_IMAGE),
        ramdisk_size: u32_at(file, OFF_RAMDISK_SIZE),
        bootsect_kludge: u32_at(file, OFF_BOOTSECT_KLUDGE),
        heap_end_ptr: u16_at(file, OFF_HEAP_END_PTR),
        ext_loader_ver: file[OFF_EXT_LOADER_VER],
        ext_loader_type: file[OFF_EXT_LOADER_TYPE],
        cmd_line_ptr: u32_at(file, OFF_CMD_LINE_PTR),
        min_alignment: file[OFF_MIN_ALIGNMENT],
        hardware_subarch: u32_at(file, OFF_HARDWARE_SUBARCH),
        setup_data: u64_at(file, OFF_SETUP_DATA),
        handover_offset: u32_at(file, OFF_HANDOVER_OFFSET),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const SETUP_SECTS: u8 = 2;
    const PAYLOAD_OFFSET: u32 = 0x1000;
    const PAYLOAD_LENGTH: u32 = 0x2000;
    /// Protected-mode code size: payload fully inside it, no signature tail.
    const PM_CODE_BYTES: u32 = PAYLOAD_OFFSET + PAYLOAD_LENGTH;

    fn put16(b: &mut [u8], off: usize, v: u16) {
        b[off..off + 2].copy_from_slice(&v.to_le_bytes());
    }
    fn put32(b: &mut [u8], off: usize, v: u32) {
        b[off..off + 4].copy_from_slice(&v.to_le_bytes());
    }
    fn put64(b: &mut [u8], off: usize, v: u64) {
        b[off..off + 8].copy_from_slice(&v.to_le_bytes());
    }

    /// Builds a valid synthetic bzImage whose file ends exactly at the payload end.
    fn synthetic() -> Vec<u8> {
        let pm = (usize::from(SETUP_SECTS) + 1) * 512;
        let len = pm + PM_CODE_BYTES as usize;
        let mut b = vec![0u8; len];
        b[OFF_SETUP_SECTS] = SETUP_SECTS;
        put32(&mut b, OFF_SYSSIZE, PM_CODE_BYTES / 16);
        put16(&mut b, OFF_BOOT_FLAG, 0xAA55);
        b[OFF_HEADER..OFF_HEADER + 4].copy_from_slice(b"HdrS");
        put16(&mut b, OFF_VERSION, 0x020F);
        b[OFF_LOADFLAGS] = 1;
        put32(&mut b, OFF_INITRD_ADDR_MAX, 0x7FFF_FFFF);
        put32(&mut b, OFF_KERNEL_ALIGNMENT, 0x0020_0000);
        b[OFF_RELOCATABLE_KERNEL] = 1;
        put16(&mut b, OFF_XLOADFLAGS, 0x7F);
        put32(&mut b, OFF_CMDLINE_SIZE, 0x7FF);
        put32(&mut b, OFF_PAYLOAD_OFFSET, PAYLOAD_OFFSET);
        put32(&mut b, OFF_PAYLOAD_LENGTH, PAYLOAD_LENGTH);
        put64(&mut b, OFF_PREF_ADDRESS, 0x0100_0000);
        put32(&mut b, OFF_INIT_SIZE, 0x0010_0000);
        b
    }

    fn corrupt(f: impl FnOnce(&mut Vec<u8>)) -> Result<LinuxBzImageHeader, LinuxHeaderError> {
        let mut b = synthetic();
        f(&mut b);
        parse_linux_bzimage_header(&b)
    }

    #[test]
    fn synthetic_header_parses() {
        let h = parse_linux_bzimage_header(&synthetic()).expect("valid header");
        assert_eq!(h.version, 0x020F);
        assert_eq!(h.xloadflags, 0x7F);
        assert_eq!(h.relocatable, 1);
        assert_eq!(h.kernel_alignment, 0x0020_0000);
        assert_eq!(h.pref_address, 0x0100_0000);
        assert_eq!(h.init_size, 0x0010_0000);
        assert_eq!(h.payload_offset, PAYLOAD_OFFSET);
        assert_eq!(h.payload_length, PAYLOAD_LENGTH);
        assert_eq!(h.protected_mode_offset, 3 * 512);
        assert_eq!(h.pm_code_size, u64::from(PM_CODE_BYTES));
        assert_eq!(h.payload_file_offset(), Some(3 * 512 + 0x1000));
        assert_eq!(h.cmdline_size, 0x7FF);
        assert_eq!(h.initrd_addr_max, 0x7FFF_FFFF);
        assert_eq!(h.setup_sects, SETUP_SECTS);
    }

    #[test]
    fn setup_sects_zero_means_four() {
        // Rebuild with setup_sects = 0 and a file large enough for the implied value 4.
        let mut b = synthetic();
        let pm = 5 * 512;
        b.resize(pm + PAYLOAD_OFFSET as usize + PAYLOAD_LENGTH as usize, 0);
        b[OFF_SETUP_SECTS] = 0;
        let h = parse_linux_bzimage_header(&b).expect("valid header");
        assert_eq!(h.setup_sects, 4);
        assert_eq!(h.protected_mode_offset, 5 * 512);
    }

    #[test]
    fn short_file_is_rejected() {
        for len in [
            0usize,
            0x100,
            0x20F,
            0x210,
            LINUX_SETUP_HEADER_MIN_BYTES - 1,
        ] {
            let mut b = synthetic();
            b.truncate(len);
            match parse_linux_bzimage_header(&b) {
                Err(LinuxHeaderError::TooShort { len: l }) => assert_eq!(l, len as u64),
                other => panic!("len {len:#x}: {other:?}"),
            }
        }
    }

    #[test]
    fn bad_boot_flag() {
        match corrupt(|b| put16(b, OFF_BOOT_FLAG, 0x55AA)) {
            Err(LinuxHeaderError::BadBootFlag { found: 0x55AA }) => {}
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn bad_hdrs_magic() {
        match corrupt(|b| b[OFF_HEADER] = b'X') {
            Err(LinuxHeaderError::BadHdrSMagic { found }) => assert_eq!(&found, b"XdrS"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn old_version() {
        match corrupt(|b| put16(b, OFF_VERSION, 0x020D)) {
            Err(LinuxHeaderError::VersionTooOld { found: 0x020D }) => {}
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn kernel64_clear() {
        match corrupt(|b| put16(b, OFF_XLOADFLAGS, 0x7E)) {
            Err(LinuxHeaderError::Kernel64NotSupported { xloadflags: 0x7E }) => {}
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn loaded_high_clear() {
        match corrupt(|b| b[OFF_LOADFLAGS] = 0) {
            Err(LinuxHeaderError::NotLoadedHigh { loadflags: 0 }) => {}
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn kernel_alignment_not_power_of_two() {
        match corrupt(|b| put32(b, OFF_KERNEL_ALIGNMENT, 0x0030_0000)) {
            Err(LinuxHeaderError::InvalidKernelAlignment { found: 0x0030_0000 }) => {}
            other => panic!("{other:?}"),
        }
        match corrupt(|b| put32(b, OFF_KERNEL_ALIGNMENT, 0x800)) {
            Err(LinuxHeaderError::InvalidKernelAlignment { found: 0x800 }) => {}
            other => panic!("{other:?}"),
        }
        match corrupt(|b| put32(b, OFF_KERNEL_ALIGNMENT, 0)) {
            Err(LinuxHeaderError::InvalidKernelAlignment { found: 0 }) => {}
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn pref_address_misaligned_or_zero() {
        // Aligned to 0x1000 but not to kernel_alignment 0x20_0000.
        match corrupt(|b| put64(b, OFF_PREF_ADDRESS, 0x0100_1000)) {
            Err(LinuxHeaderError::PrefAddressMisaligned {
                pref_address: 0x0100_1000,
                kernel_alignment: 0x0020_0000,
            }) => {}
            other => panic!("{other:?}"),
        }
        match corrupt(|b| put64(b, OFF_PREF_ADDRESS, 0)) {
            Err(LinuxHeaderError::PrefAddressZero) => {}
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn setup_beyond_file() {
        // setup_sects = 255 needs 256 * 512 bytes; the synthetic file is smaller.
        match corrupt(|b| b[OFF_SETUP_SECTS] = 255) {
            Err(LinuxHeaderError::SetupOutOfFile {
                protected_mode_offset: 0x20000,
                ..
            }) => {}
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn payload_outside_file() {
        match corrupt(|b| put32(b, OFF_PAYLOAD_LENGTH, PAYLOAD_LENGTH + 1)) {
            Err(LinuxHeaderError::PayloadOutOfFile { .. }) => {}
            other => panic!("{other:?}"),
        }
        match corrupt(|b| put32(b, OFF_PAYLOAD_OFFSET, PAYLOAD_OFFSET + 1)) {
            Err(LinuxHeaderError::PayloadOutOfFile { .. }) => {}
            other => panic!("{other:?}"),
        }
        // u32 sum would overflow; the u64 sum is still beyond the file.
        match corrupt(|b| {
            put32(b, OFF_PAYLOAD_OFFSET, u32::MAX);
            put32(b, OFF_PAYLOAD_LENGTH, u32::MAX);
        }) {
            Err(LinuxHeaderError::PayloadOutOfFile { .. }) => {}
            other => panic!("{other:?}"),
        }
        match corrupt(|b| put32(b, OFF_PAYLOAD_LENGTH, 0)) {
            Err(LinuxHeaderError::PayloadEmpty) => {}
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn init_size_bounds() {
        match corrupt(|b| put32(b, OFF_INIT_SIZE, 0x8000_0000)) {
            Err(LinuxHeaderError::InitSizeTooLarge {
                init_size: 0x8000_0000,
            }) => {}
            other => panic!("{other:?}"),
        }
        match corrupt(|b| put32(b, OFF_INIT_SIZE, u32::MAX)) {
            Err(LinuxHeaderError::InitSizeTooLarge { .. }) => {}
            other => panic!("{other:?}"),
        }
        match corrupt(|b| put32(b, OFF_INIT_SIZE, PAYLOAD_LENGTH - 1)) {
            Err(LinuxHeaderError::InitSizeTooSmall { .. }) => {}
            other => panic!("{other:?}"),
        }
        match corrupt(|b| put32(b, OFF_INIT_SIZE, PM_CODE_BYTES - 1)) {
            Err(LinuxHeaderError::InitSizeTooSmall { .. }) => {}
            other => panic!("{other:?}"),
        }
        // Exactly at the limits is accepted.
        assert!(corrupt(|b| put32(b, OFF_INIT_SIZE, LINUX_MAX_INIT_SIZE)).is_ok());
        assert!(corrupt(|b| put32(b, OFF_INIT_SIZE, PM_CODE_BYTES)).is_ok());
    }

    #[test]
    fn pref_address_plus_init_size_overflow() {
        // 2^64 - 2 MiB is aligned to 0x20_0000; adding init_size 0x20_0001 overflows u64.
        match corrupt(|b| {
            put64(b, OFF_PREF_ADDRESS, u64::MAX - 0x1F_FFFF);
            put32(b, OFF_INIT_SIZE, 0x0020_0001);
        }) {
            Err(LinuxHeaderError::KernelRangeOverflow { .. }) => {}
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn pm_code_outside_file() {
        // syssize claims one paragraph more than the file holds after the protected-mode start.
        match corrupt(|b| put32(b, OFF_SYSSIZE, PM_CODE_BYTES / 16 + 1)) {
            Err(LinuxHeaderError::PmCodeOutOfFile { .. }) => {}
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn payload_in_signature_tail_is_rejected() {
        // The payload fits in the file but runs past the protected-mode code (syssize).
        match corrupt(|b| {
            b.extend_from_slice(&[0u8; 0x100]);
            put32(b, OFF_PAYLOAD_LENGTH, PAYLOAD_LENGTH + 0x100);
        }) {
            Err(LinuxHeaderError::PayloadOutOfFile { .. }) => {}
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn cmdline_size_zero() {
        match corrupt(|b| put32(b, OFF_CMDLINE_SIZE, 0)) {
            Err(LinuxHeaderError::CmdlineSizeZero) => {}
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn parsed_values_come_from_the_file_not_constants() {
        let h = corrupt(|b| {
            put32(b, OFF_INITRD_ADDR_MAX, 0x3FFF_FFFF);
            put64(b, OFF_PREF_ADDRESS, 0x0200_0000);
            put32(b, OFF_CMDLINE_SIZE, 0x1234);
            put32(b, OFF_KERNEL_ALIGNMENT, 0x0010_0000);
            put32(b, OFF_INIT_SIZE, 0x0040_0000);
        })
        .expect("valid header");
        assert_eq!(h.initrd_addr_max, 0x3FFF_FFFF);
        assert_eq!(h.pref_address, 0x0200_0000);
        assert_eq!(h.cmdline_size, 0x1234);
        assert_eq!(h.kernel_alignment, 0x0010_0000);
        assert_eq!(h.init_size, 0x0040_0000);
    }

    const REAL_VMLINUZ: &str = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../work/stage4/linux-7.0.0-38.38/vmlinuz-7.0.0-38-generic"
    );

    /// Ignored by default because `work/` is not part of the repository. It must be run
    /// explicitly (`--ignored`) and fails, instead of passing vacuously, if the file is absent.
    #[test]
    #[ignore = "needs work/stage4/linux-7.0.0-38.38/vmlinuz-7.0.0-38-generic; run with --ignored"]
    fn real_vmlinuz_header() {
        let file = std::fs::read(REAL_VMLINUZ)
            .unwrap_or_else(|e| panic!("vmlinuz absent, test NOT passed: {REAL_VMLINUZ}: {e}"));
        let h = parse_linux_bzimage_header(&file).expect("real vmlinuz header");
        assert_eq!(h.version, 0x020F);
        assert_eq!(h.setup_sects, 39);
        assert_eq!(h.kernel_alignment, 0x0020_0000);
        assert_eq!(h.relocatable, 1);
        assert_eq!(h.pref_address, 0x0100_0000);
        assert_eq!(h.init_size, 0x044C_1000);
        assert_eq!(h.xloadflags, 0x7F);
        assert_eq!(h.cmdline_size, 0x07FF);
        assert_eq!(h.initrd_addr_max, 0x7FFF_FFFF);
        assert_eq!(h.protected_mode_offset, 0x5000);
        assert_eq!(h.pm_code_size, 0x107F200);
        assert_eq!(h.payload_offset, 0x2CC);
        assert_eq!(h.payload_length, 0x10456EA);
        // The payload must start with the zstd magic and lie inside the protected-mode code.
        let start = h.payload_file_offset().expect("payload offset") as usize;
        assert_eq!(&file[start..start + 4], &[0x28, 0xB5, 0x2F, 0xFD]);
        assert!(h.protected_mode_offset + h.pm_code_size <= file.len() as u64);
    }

    /// Injection on the real file: changing the bytes must change the parse result.
    #[test]
    #[ignore = "needs work/stage4/linux-7.0.0-38.38/vmlinuz-7.0.0-38-generic; run with --ignored"]
    fn real_vmlinuz_injection_is_reflected() {
        let mut file = std::fs::read(REAL_VMLINUZ)
            .unwrap_or_else(|e| panic!("vmlinuz absent, test NOT passed: {REAL_VMLINUZ}: {e}"));
        put32(&mut file, OFF_INITRD_ADDR_MAX, 0x3FFF_FFFF);
        put64(&mut file, OFF_PREF_ADDRESS, 0x0200_0000);
        let h = parse_linux_bzimage_header(&file).expect("patched header");
        assert_eq!(h.initrd_addr_max, 0x3FFF_FFFF);
        assert_eq!(h.pref_address, 0x0200_0000);
    }
}
