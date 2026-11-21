use super::*;
use crate::pe::test_image::{PE_START, RELOCATIONS, fixture, put_u16, put_u32};

const IMAGE_BYTES: usize = 0x4000;
const PREFERRED: u64 = 0x1_4000_0000;
const LOAD_BASE: u64 = 0x20_0000;
const RAW_TEXT: usize = 0x200;
const RAW_RELOC: usize = 0x400;
const DIR64: u16 = 0xa000;

fn block(file: &mut [u8], page_rva: u32, entries: &[u16]) {
    let size = 8 + 2 * entries.len();
    put_u32(file, RELOCATIONS + 4, size as u32);
    put_u32(file, RAW_RELOC, page_rva);
    put_u32(file, RAW_RELOC + 4, size as u32);
    for (index, &entry) in entries.iter().enumerate() {
        put_u16(file, RAW_RELOC + 8 + 2 * index, entry);
    }
}

fn get_u64(bytes: &[u8], offset: usize) -> u64 {
    u64::from_le_bytes(bytes[offset..offset + 8].try_into().unwrap())
}

fn run(file: &[u8], base: u64, target: &mut [u8]) -> Result<MappedImage, LoadError> {
    load_into(&PeImage::parse(file).unwrap(), base, target)
}

#[test]
fn maps_headers_raw_padding_and_zero_fill_with_wrapping_relocations() {
    let mut file = fixture(12);
    let address = PREFERRED + 0x420;
    file[RAW_TEXT + 0x20..RAW_TEXT + 0x28].copy_from_slice(&address.to_le_bytes());
    file[RAW_TEXT + 0x1f0] = 0x7e;
    block(&mut file, 0x1000, &[DIR64 | 0x20, 0x0fff]);

    for base in [PREFERRED, LOAD_BASE, 0xc000_0000_0000_0000] {
        let mut destination = [0xa5; IMAGE_BYTES + 17];
        let mapped = run(&file, base, &mut destination).unwrap();
        assert_eq!(mapped.base, base);
        assert_eq!(mapped.size, 0x4000);
        assert_eq!(mapped.entry_address, base + 0x1000);
        assert_eq!(mapped.subsystem, Subsystem::RuntimeDriver);
        assert_eq!(&destination[..0x200], &file[..0x200]);
        assert_eq!(destination[0x1000], 0xc3);
        assert_eq!(destination[0x11f0], 0x7e);
        assert_eq!(get_u64(&destination, 0x1020), base + 0x420);
        assert!(destination[0x200..0x1000].iter().all(|&byte| byte == 0));
        assert!(
            destination[0x3000..IMAGE_BYTES]
                .iter()
                .all(|&byte| byte == 0)
        );
        assert_eq!(&destination[IMAGE_BYTES..], &[0xa5; 17]);
    }
}

#[test]
fn unaligned_and_duplicate_targets_follow_original_directory_stream() {
    let mut file = fixture(10);
    // The first target crosses into the mapped relocation table. The next
    // entry must still be fetched from the pristine file.
    block(
        &mut file,
        0x1000,
        &[DIR64 | 0xffc, DIR64 | 0x19, DIR64 | 0x19],
    );
    let original = 7u64;
    file[RAW_TEXT + 0x19..RAW_TEXT + 0x21].copy_from_slice(&original.to_le_bytes());
    let mut destination = [0; IMAGE_BYTES];
    run(&file, LOAD_BASE, &mut destination).unwrap();
    let delta = LOAD_BASE.wrapping_sub(PREFERRED);
    assert_eq!(
        get_u64(&destination, 0x1ffc),
        (0x1000u64 << 32).wrapping_add(delta)
    );
    assert_eq!(
        get_u64(&destination, 0x1019),
        original.wrapping_add(delta).wrapping_add(delta)
    );
    assert_eq!(get_u64(&file, RAW_TEXT + 0x19), original);
}

#[test]
fn accepts_last_qword_and_final_ten_byte_block() {
    let mut file = fixture(11);
    block(&mut file, 0x3000, &[DIR64 | 0xff8]);
    let mut destination = [0xa5; IMAGE_BYTES];
    run(&file, LOAD_BASE, &mut destination).unwrap();
    assert_eq!(
        get_u64(&destination, IMAGE_BYTES - 8),
        LOAD_BASE.wrapping_sub(PREFERRED)
    );
}

#[test]
fn preflights_every_entry_before_touching_the_destination() {
    let mut file = fixture(10);
    let mut destination = [0xa5; IMAGE_BYTES + 8];
    for (last, expected) in [
        (0x3020, RelocationError::UnsupportedType(3)),
        (DIR64 | 0xff9, RelocationError::TargetOutsideImage),
    ] {
        block(&mut file, 0x3000, &[DIR64 | 0x10, last]);
        assert_eq!(
            run(&file, LOAD_BASE, &mut destination),
            Err(LoadError::Relocation(expected))
        );
        assert!(destination.iter().all(|&byte| byte == 0xa5));
    }
}

#[test]
fn rejects_bad_block_sizes_page_rvas_and_alignment() {
    let mut file = fixture(10);
    let mut destination = [0xa5; IMAGE_BYTES];
    for size in [0, 6, 9, 0x180] {
        block(&mut file, 0x1000, &[DIR64]);
        put_u32(&mut file, RAW_RELOC + 4, size);
        assert_eq!(
            run(&file, LOAD_BASE, &mut destination),
            Err(LoadError::Relocation(RelocationError::InvalidBlockSize))
        );
    }
    block(&mut file, 0x1002, &[DIR64]);
    assert_eq!(
        run(&file, LOAD_BASE, &mut destination),
        Err(LoadError::Relocation(RelocationError::InvalidPageRva))
    );
    block(&mut file, 0x1000, &[DIR64]);
    put_u32(&mut file, RELOCATIONS + 4, 12);
    assert_eq!(
        run(&file, LOAD_BASE, &mut destination),
        Err(LoadError::Relocation(RelocationError::MisalignedBlock))
    );
    assert!(destination.iter().all(|&byte| byte == 0xa5));
}

#[test]
fn handles_missing_and_stripped_tables_and_checks_caller_span() {
    let mut file = fixture(10);
    let mut destination = [0xa5; IMAGE_BYTES];
    block(&mut file, 0x1000, &[DIR64]);
    put_u16(&mut file, PE_START + 22, 0x0003);
    assert_eq!(
        run(&file, LOAD_BASE, &mut destination),
        Err(LoadError::ConflictingRelocationFlag)
    );
    put_u32(&mut file, RELOCATIONS, 0);
    put_u32(&mut file, RELOCATIONS + 4, 0);
    assert_eq!(
        run(&file, LOAD_BASE, &mut destination),
        Err(LoadError::RelocationsRequired)
    );
    assert_eq!(
        run(&file, PREFERRED, &mut destination[1..]),
        Err(LoadError::DestinationTooSmall)
    );
    assert_eq!(
        run(&file, u64::MAX & !0xfff, &mut destination),
        Err(LoadError::InvalidAddress)
    );
    assert_eq!(
        run(&file, PREFERRED + 1, &mut destination),
        Err(LoadError::InvalidAddress)
    );
    assert!(destination.iter().all(|&byte| byte == 0xa5));
    assert!(run(&file, PREFERRED, &mut destination).is_ok());
}

const PIC_BASE_OFFSET: usize = crate::pe::test_image::OPTIONAL_START + 24;
const UBUNTU_VMLINUZ: &str = "work/stage4/linux-7.0.0-38.38/vmlinuz-7.0.0-38-generic";

fn position_independent_fixture() -> Vec<u8> {
    let mut file = fixture(10);
    file[PIC_BASE_OFFSET..PIC_BASE_OFFSET + 8].copy_from_slice(&0u64.to_le_bytes());
    put_u32(&mut file, RELOCATIONS, 0);
    put_u32(&mut file, RELOCATIONS + 4, 0);
    file
}

#[test]
fn zero_base_image_without_relocations_loads_at_any_aligned_address() {
    let mut file = position_independent_fixture();
    file[RAW_TEXT + 0x20..RAW_TEXT + 0x28].copy_from_slice(&0x1234u64.to_le_bytes());
    let image = PeImage::parse(&file).unwrap();
    assert!(is_relocatable(&image));
    for base in [0, LOAD_BASE, 0xc000_0000_0000_0000] {
        let mut destination = [0xa5; IMAGE_BYTES];
        let mapped = run(&file, base, &mut destination).unwrap();
        assert_eq!(mapped.base, base);
        assert_eq!(mapped.entry_address, base + 0x1000);
        // No fixups exist: the bytes are copied verbatim at every base.
        assert_eq!(get_u64(&destination, 0x1020), 0x1234);
        assert_eq!(destination[0x1000], 0xc3);
    }
    let mut destination = [0xa5; IMAGE_BYTES];
    assert_eq!(
        run(&file, LOAD_BASE + 1, &mut destination),
        Err(LoadError::InvalidAddress)
    );
    assert!(destination.iter().all(|&byte| byte == 0xa5));
}

#[test]
fn stripped_or_nonzero_base_image_without_relocations_stays_pinned() {
    let mut destination = [0xa5; IMAGE_BYTES];

    let mut stripped = position_independent_fixture();
    put_u16(&mut stripped, PE_START + 22, 0x0003);
    assert!(!is_relocatable(&PeImage::parse(&stripped).unwrap()));
    assert!(run(&stripped, 0, &mut destination).is_ok());
    assert_eq!(
        run(&stripped, LOAD_BASE, &mut destination),
        Err(LoadError::RelocationsRequired)
    );

    let mut fixed = fixture(10);
    put_u32(&mut fixed, RELOCATIONS, 0);
    put_u32(&mut fixed, RELOCATIONS + 4, 0);
    assert!(!is_relocatable(&PeImage::parse(&fixed).unwrap()));
    assert!(run(&fixed, PREFERRED, &mut destination).is_ok());
    assert_eq!(
        run(&fixed, LOAD_BASE, &mut destination),
        Err(LoadError::RelocationsRequired)
    );
}

#[test]
fn zero_base_image_with_a_directory_applies_fixups() {
    let mut file = fixture(10);
    file[PIC_BASE_OFFSET..PIC_BASE_OFFSET + 8].copy_from_slice(&0u64.to_le_bytes());
    file[RAW_TEXT + 0x20..RAW_TEXT + 0x28].copy_from_slice(&0x420u64.to_le_bytes());
    block(&mut file, 0x1000, &[DIR64 | 0x20]);
    assert!(is_relocatable(&PeImage::parse(&file).unwrap()));
    let mut destination = [0; IMAGE_BYTES];
    run(&file, LOAD_BASE, &mut destination).unwrap();
    assert_eq!(get_u64(&destination, 0x1020), LOAD_BASE + 0x420);
}

fn ubuntu_vmlinuz() -> Option<Vec<u8>> {
    let relative = std::path::Path::new(UBUNTU_VMLINUZ);
    let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let path = std::env::var_os("OD_UBUNTU_VMLINUZ")
        .map(std::path::PathBuf::from)
        .or_else(|| {
            // The kernel lives in the untracked work/ tree of the main checkout;
            // a linked worktree sits below it in .claude/worktrees/.
            manifest
                .ancestors()
                .map(|root| root.join(relative))
                .find(|candidate| candidate.is_file())
        })?;
    Some(std::fs::read(path).unwrap())
}

#[test]
fn ubuntu_kernel_efi_stub_is_position_independent_and_loads_without_fixups() {
    let Some(file) = ubuntu_vmlinuz() else {
        eprintln!("skipped: {UBUNTU_VMLINUZ} is not staged (set OD_UBUNTU_VMLINUZ)");
        return;
    };
    eprintln!("ubuntu vmlinuz: {} bytes", file.len());
    let image = PeImage::parse(&file).unwrap();
    // Facts read from the real headers (S7-31b): ImageBase 0, no base
    // relocation directory, RELOCS_STRIPPED clear.
    assert_eq!(image.image_base(), 0);
    assert!(image.relocation_data().is_none());
    assert!(!image.relocations_stripped());
    assert!(is_relocatable(&image));
    // LoadImage derives its page runs from the same headers.
    assert!(crate::pe::PageLayout::for_image(&image).is_some());
    let size = image.image_size() as usize;
    let text = image
        .sections()
        .find(|section| section.file_data.len() > 0x1000 && section.virtual_address == 0x5000)
        .expect(".text section");
    for base in [0x4000_0000u64, 0x1_0000_0000] {
        let mut destination = vec![0xa5; size];
        let mapped = run(&file, base, &mut destination).unwrap();
        assert_eq!(mapped.base, base);
        assert_eq!(mapped.size as usize, size);
        assert_eq!(mapped.entry_address, base + u64::from(image.entry_rva()));
        assert_eq!(&destination[..0x40], &file[..0x40]);
        assert_eq!(
            &destination[0x5000..0x5000 + 0x1000],
            &text.file_data[..0x1000]
        );
    }
}
