use od_isa::{CpuActivity, Register};
use od_machine::{
    AccessWidth, GuestPhysicalAddress, PagedProbeExit, STAGE4_FLAT_PROBE_IMAGE_BYTES,
    STAGE4_FLAT_PROBE_IMAGE_GPA, Stage4FlatProbeLayoutError, Stage4FlatProbeLoadError,
    load_stage4_alias_probe,
};

const INITIAL: u64 = 0x1122334455667788;
const STORE: u64 = 0x8877665544332211;
fn image(initial: u64, store: u64) -> Vec<u8> {
    let mut b = vec![0; STAGE4_FLAT_PROBE_IMAGE_BYTES];
    for (g, v) in [
        (0x1000, 0x2003),
        (0x2000, 0x3003),
        (0x3010, 0x4003),
        (0x3020, 0x5003),
        (0x3028, 0x6003),
        (0x4000, 0x7001),
        (0x5000, 0x9003 | (1 << 63)),
        (0x6000, 0x9003 | (1 << 63)),
    ] {
        put(&mut b, (g - STAGE4_FLAT_PROBE_IMAGE_GPA) as usize, v)
    }
    b[0x6000..0x6006].copy_from_slice(&[0x48, 0x89, 0x0b, 0x58, 0xee, 0xf4]);
    put(&mut b, 0x8000, initial);
    put(&mut b, 0x8008, store);
    b
}
fn put(b: &mut [u8], o: usize, v: u64) {
    b[o..o + 8].copy_from_slice(&v.to_le_bytes())
}
fn qword(m: &mut od_machine::LoadedStage4FlatProbe, g: u64) -> u64 {
    m.bus_mut()
        .read_physical(GuestPhysicalAddress::new(g), AccessWidth::Qword)
        .unwrap()
}

#[test]
fn guest_store_and_pop_observe_same_ram_page_through_two_gvas() {
    let mut m = load_stage4_alias_probe(&image(INITIAL, STORE)).unwrap();
    assert_eq!(m.state().gpr[Register::Rbx as usize], 0x800000);
    assert_eq!(m.state().gpr[Register::Rsp as usize], 0xa00000);
    let mut all_writes = Vec::new();
    for step in 0..4 {
        let mut writes = Vec::new();
        let r = m.run_stack(1, || false, |x| writes.push((x.start().get(), x.len())));
        all_writes.push(writes.clone());
        assert_eq!(r.instructions_executed, 1);
        assert_eq!(
            r.exit,
            if step == 3 {
                PagedProbeExit::Halted
            } else {
                PagedProbeExit::BudgetExhausted
            }
        );
        if step == 0 {
            assert_eq!(qword(&mut m, 0x9000), STORE);
            assert_eq!(writes.iter().filter(|&&(g, _)| g == 0x9000).count(), 1)
        }
    }
    assert_eq!(m.state().gpr[Register::Rax as usize], STORE);
    assert_eq!(m.state().gpr[Register::Rsp as usize], 0xa00008);
    assert_eq!(m.state().rip, 0x400006);
    assert_eq!(m.state().activity, Some(CpuActivity::Halted));
    assert_eq!(qword(&mut m, 0x5000) & 0x60, 0x60);
    assert_eq!(qword(&mut m, 0x6000) & 0x60, 0x20);
    assert_eq!(qword(&mut m, 0x9000), STORE);
    let u = m.uart();
    assert!(u.transfer_holding_to_shift());
    assert_eq!(u.complete_transmission(), Some(STORE as u8));
    assert_eq!(
        all_writes
            .iter()
            .flatten()
            .filter(|&&(g, _)| g == 0x9000)
            .count(),
        1
    );
    assert_eq!(
        all_writes[0],
        vec![
            (0x1000, 8),
            (0x2000, 8),
            (0x3010, 8),
            (0x4000, 8),
            (0x3020, 8),
            (0x5000, 8),
            (0x5000, 8),
            (0x9000, 8)
        ]
    );
    assert_eq!(all_writes[1], vec![(0x3028, 8), (0x6000, 8)]);
    assert!(all_writes[2].is_empty());
    assert!(all_writes[3].is_empty());
}

#[test]
fn rejects_split_alias_and_legacy_profile_remains_separate() {
    let mut b = image(INITIAL, STORE);
    put(&mut b, 0x5000, 0xa003 | (1 << 63));
    assert!(matches!(
        load_stage4_alias_probe(&b),
        Err(Stage4FlatProbeLoadError::Layout(
            Stage4FlatProbeLayoutError::PageTableEntry { gpa: 0x6000, .. }
        ))
    ));
}
