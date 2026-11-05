use od_isa::{CpuActivity, INTEGER_FLAGS, LongModeState, Register};
use od_machine::{
    AccessWidth, GuestPhysicalAddress, PagedProbeExit, STAGE4_FLAT_PROBE_CR3,
    STAGE4_FLAT_PROBE_DATA_GVA, STAGE4_FLAT_PROBE_EFER, STAGE4_FLAT_PROBE_ENTRY_GVA,
    STAGE4_FLAT_PROBE_IMAGE_BYTES, STAGE4_FLAT_PROBE_IMAGE_GPA, STAGE4_FLAT_PROBE_STACK_TOP_GVA,
    STAGE4_FLAT_PROBE_UART_BASE, Stage4FlatProbeLayoutError, Stage4FlatProbeLoadError,
    load_stage4_flat_probe,
};

const CODE: [u8; 5] = [0x48, 0x8b, 0x03, 0xee, 0xf4];
const CODE_PTE_GPA: u64 = 0x4000;
const DATA_PTE_GPA: u64 = 0x5000;
const STACK_PTE_GPA: u64 = 0x6000;

fn valid_image(payload: u64) -> Vec<u8> {
    let mut image = vec![0; STAGE4_FLAT_PROBE_IMAGE_BYTES];
    entry(&mut image, 0x1000, 0x2003);
    entry(&mut image, 0x2000, 0x3003);
    entry(&mut image, 0x3010, 0x4003);
    entry(&mut image, 0x3020, 0x5003);
    entry(&mut image, 0x3028, 0x6003);
    entry(&mut image, CODE_PTE_GPA, 0x7001);
    entry(&mut image, DATA_PTE_GPA, 0x9003 | (1 << 63));
    entry(&mut image, STACK_PTE_GPA, 0xa003 | (1 << 63));
    image[0x7000 - STAGE4_FLAT_PROBE_IMAGE_GPA as usize
        ..0x7000 - STAGE4_FLAT_PROBE_IMAGE_GPA as usize + CODE.len()]
        .copy_from_slice(&CODE);
    let data_offset = (0x9000 - STAGE4_FLAT_PROBE_IMAGE_GPA) as usize;
    image[data_offset..data_offset + 8].copy_from_slice(&payload.to_le_bytes());
    image
}

fn entry(image: &mut [u8], gpa: u64, value: u64) {
    let offset = (gpa - STAGE4_FLAT_PROBE_IMAGE_GPA) as usize;
    image[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
}

#[test]
fn rejects_truncated_and_oversized_images() {
    let mut truncated = valid_image(0x1234);
    truncated.pop();
    assert!(matches!(
        load_stage4_flat_probe(&truncated),
        Err(Stage4FlatProbeLoadError::ImageSize {
            expected: STAGE4_FLAT_PROBE_IMAGE_BYTES,
            actual,
        }) if actual == STAGE4_FLAT_PROBE_IMAGE_BYTES - 1
    ));

    let mut oversized = valid_image(0x1234);
    oversized.push(0);
    assert!(matches!(
        load_stage4_flat_probe(&oversized),
        Err(Stage4FlatProbeLoadError::ImageSize {
            expected: STAGE4_FLAT_PROBE_IMAGE_BYTES,
            actual,
        }) if actual == STAGE4_FLAT_PROBE_IMAGE_BYTES + 1
    ));
}

#[test]
fn rejects_aliases_and_non_executable_entry_mapping() {
    let mut alias = valid_image(0x1234);
    entry(&mut alias, DATA_PTE_GPA, 0x7003 | (1 << 63));
    assert!(matches!(
        load_stage4_flat_probe(&alias),
        Err(Stage4FlatProbeLoadError::Layout(
            Stage4FlatProbeLayoutError::PageTableEntry {
                gpa: DATA_PTE_GPA,
                ..
            }
        ))
    ));

    let mut nx_code = valid_image(0x1234);
    entry(&mut nx_code, CODE_PTE_GPA, 0x7001 | (1 << 63));
    assert!(matches!(
        load_stage4_flat_probe(&nx_code),
        Err(Stage4FlatProbeLoadError::Layout(
            Stage4FlatProbeLayoutError::PageTableEntry {
                gpa: CODE_PTE_GPA,
                ..
            }
        ))
    ));
}

#[test]
fn rejects_invalid_stack_mapping_and_unexpected_present_table_entries() {
    let mut read_only_stack = valid_image(0x1234);
    entry(&mut read_only_stack, STACK_PTE_GPA, 0xa001 | (1 << 63));
    assert!(matches!(
        load_stage4_flat_probe(&read_only_stack),
        Err(Stage4FlatProbeLoadError::Layout(
            Stage4FlatProbeLayoutError::PageTableEntry {
                gpa: STACK_PTE_GPA,
                ..
            }
        ))
    ));

    let mut unexpected_mapping = valid_image(0x1234);
    entry(&mut unexpected_mapping, 0x3020 + 8, 0x8003);
    assert!(matches!(
        load_stage4_flat_probe(&unexpected_mapping),
        Err(Stage4FlatProbeLoadError::Layout(
            Stage4FlatProbeLayoutError::PageTableEntry { gpa: 0x3028, .. }
        ))
    ));
}

#[test]
fn loads_verbatim_image_and_hands_off_paged_probe_state_and_uart() {
    let payload = 0xfeed_face_cafe_be41;
    let image = valid_image(payload);
    let mut loaded = load_stage4_flat_probe(&image).unwrap();

    let state: &LongModeState = loaded.state();
    assert_eq!(state.rip, STAGE4_FLAT_PROBE_ENTRY_GVA);
    assert_eq!(
        state.gpr[Register::Rbx as usize],
        STAGE4_FLAT_PROBE_DATA_GVA
    );
    assert_eq!(
        state.gpr[Register::Rsp as usize],
        STAGE4_FLAT_PROBE_STACK_TOP_GVA
    );
    assert_eq!(
        state.gpr[Register::Rdx as usize],
        u64::from(STAGE4_FLAT_PROBE_UART_BASE)
    );
    assert_eq!(state.cr0, Some(0x8001_0033));
    assert_eq!(state.cr4, Some(0x20));
    assert_eq!(state.activity, Some(CpuActivity::Running));
    assert_eq!(loaded.cr3(), STAGE4_FLAT_PROBE_CR3);
    assert_eq!(loaded.efer(), STAGE4_FLAT_PROBE_EFER);
    assert_eq!(loaded.rflags_defined(), INTEGER_FLAGS);

    let physical = loaded
        .bus_mut()
        .read_physical(GuestPhysicalAddress::new(0x9000), AccessWidth::Qword)
        .unwrap();
    assert_eq!(physical, payload);
    let mut loaded_code = [0; CODE.len()];
    for (index, byte) in loaded_code.iter_mut().enumerate() {
        *byte = loaded
            .bus_mut()
            .read_physical(
                GuestPhysicalAddress::new(0x7000 + index as u64),
                AccessWidth::Byte,
            )
            .unwrap() as u8;
    }
    assert_eq!(loaded_code, CODE);

    let report = loaded.run(3, || false, |_| {});
    assert_eq!(report.exit, PagedProbeExit::Halted);
    assert_eq!(report.instructions_executed, 3);
    assert_eq!(report.rflags_defined, INTEGER_FLAGS);
    assert_eq!(loaded.rflags_defined(), INTEGER_FLAGS);
    assert_eq!(
        loaded.state().rip,
        STAGE4_FLAT_PROBE_ENTRY_GVA + CODE.len() as u64
    );
    assert_eq!(loaded.state().activity, Some(CpuActivity::Halted));
    assert_eq!(loaded.state().gpr[Register::Rax as usize], payload);

    let uart = loaded.uart();
    assert!(uart.transfer_holding_to_shift());
    assert_eq!(uart.complete_transmission(), Some(payload as u8));
}
