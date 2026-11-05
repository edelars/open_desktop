use od_isa::{CpuActivity, INTEGER_FLAGS, Register};
use od_machine::{
    AccessWidth, GuestPhysicalAddress, PagedProbeExit, STAGE4_FLAT_PROBE_IMAGE_BYTES,
    STAGE4_FLAT_PROBE_IMAGE_GPA, Stage4FlatProbeLayoutError, Stage4FlatProbeLoadError,
    load_stage4_store_load_probe,
};

const CODE: [u8; 8] = [0x48, 0x89, 0x0b, 0x48, 0x8b, 0x03, 0xee, 0xf4];

fn range(address: u64) -> od_machine::GuestPhysicalRange {
    od_machine::GuestPhysicalRange::new(od_machine::GuestPhysicalAddress::new(address), 8).unwrap()
}

fn image(initial: u64, value: u64) -> Vec<u8> {
    let mut bytes = vec![0; STAGE4_FLAT_PROBE_IMAGE_BYTES];
    for (gpa, pte) in [
        (0x1000, 0x2003),
        (0x2000, 0x3003),
        (0x3010, 0x4003),
        (0x3020, 0x5003),
        (0x3028, 0x6003),
        (0x4000, 0x7001),
        (0x5000, 0x9003 | (1 << 63)),
        (0x6000, 0xa003 | (1 << 63)),
    ] {
        put(
            &mut bytes,
            (gpa - STAGE4_FLAT_PROBE_IMAGE_GPA) as usize,
            pte,
        );
    }
    bytes[0x6000..0x6008].copy_from_slice(&CODE);
    put(&mut bytes, 0x8000, initial);
    put(&mut bytes, 0x8008, value);
    bytes
}

fn put(bytes: &mut [u8], offset: usize, value: u64) {
    bytes[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
}

#[test]
fn artifact_value_is_handed_off_then_guest_store_load_uart_hlt_executes() {
    let initial = 0x0102_0304_0506_0708;
    let value = 0x1122_3344_5566_7788;
    let mut machine = load_stage4_store_load_probe(&image(initial, value)).unwrap();
    assert_eq!(machine.state().gpr[Register::Rcx as usize], value);
    assert_eq!(
        machine
            .bus_mut()
            .read_physical(GuestPhysicalAddress::new(0x9000), AccessWidth::Qword)
            .unwrap(),
        initial
    );
    assert_eq!(machine.rflags_defined(), 0x0cd7);
    let mut callbacks = Vec::new();
    let report = machine.run(4, || false, |range| callbacks.push(range));
    assert_eq!(report.exit, PagedProbeExit::Halted);
    assert_eq!(report.instructions_executed, 4);
    assert_eq!(report.rflags_defined, INTEGER_FLAGS);
    assert_eq!(machine.state().rip, 0x0040_0008);
    assert_eq!(machine.state().activity, Some(CpuActivity::Halted));
    assert_eq!(machine.state().gpr[Register::Rax as usize], value);
    assert_eq!(
        callbacks,
        [
            range(0x1000),
            range(0x2000),
            range(0x3010),
            range(0x4000),
            range(0x3020),
            range(0x5000),
            range(0x5000),
            range(0x9000),
        ]
    );
    assert_eq!(
        machine
            .bus_mut()
            .read_physical(GuestPhysicalAddress::new(0x9000), AccessWidth::Qword)
            .unwrap(),
        value
    );
    let uart = machine.uart();
    assert!(uart.transfer_holding_to_shift());
    assert_eq!(uart.complete_transmission(), Some(value as u8));
}

#[test]
fn rejects_equal_data_values_and_wrong_profile_code() {
    assert!(matches!(
        load_stage4_store_load_probe(&image(7, 7)),
        Err(Stage4FlatProbeLoadError::Layout(
            Stage4FlatProbeLayoutError::DataQwordsEqual
        ))
    ));
    let mut wrong = image(1, 2);
    wrong[0x6000] ^= 1;
    assert!(matches!(
        load_stage4_store_load_probe(&wrong),
        Err(Stage4FlatProbeLoadError::Layout(
            Stage4FlatProbeLayoutError::CodeBytes { .. }
        ))
    ));
}
