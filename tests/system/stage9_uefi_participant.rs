//! S9-26, local slice: the UEFI variable store as a separate participant of the
//! VM container (inline section), without a VMHost and without other
//! participants. See "Персистентность в контейнере VM v1" in
//! docs/specs/NVRAM_DEVICE.md.
//!
//! What is checked, with the real `od_storage::vm_container` codec and the real
//! `od_machine::NvramDevice`:
//!
//! 1. a v1 and a v2 snapshot make a round trip through the section;
//! 2. a flipped byte of the payload is caught by the section CRC (and one of the
//!    header by the header CRC);
//! 3. non-zero corruption written together with fresh container CRCs is caught
//!    only by the strict `ODNVBANK` check; v2 refuses a damaged newer bank
//!    instead of falling back to the older policy (an all-zero slot is another
//!    matter, see below), legacy v1 falls back (allowed);
//! 4. the identity and the epoch of the container, with fresh CRCs;
//! 5. a `schema_version` that disagrees with the snapshot header;
//! 6. a missing section is an error, never an empty store;
//! 7. a v1 snapshot where v2 is expected is refused (no downgrade);
//! 8. a snapshot taken during an unfinished WRITE restores the last committed bank;
//! 9. an older valid container of the same VM is accepted, and a cold
//!    secure-boot reset restarts the generation at 1 (an observation: no
//!    monotonicity is claimed).
//!
//! Also recorded: zeroing the whole newest v2 slot is accepted as an older bank
//! (a known, undetected policy rollback until MAC, S9-27).
//!
//! Not covered and not claimed: capture of the generation under a common cut
//! (S9-28), rollback protection without an external anchor (S9-27, S9-29),
//! authenticity (MAC, S9-27), live restore (POLICY_EPOCH is not saved, ISSUE-600),
//! a production writer or reader of the container (the glue below exists only in
//! this test) and any binding to a VM inside `ODNVBANK` itself (the VM is named by
//! the container header only).

use od_machine::{
    AccessWidth, BusDevice, DeviceAccess, IoSpace, NVRAM_STORE_BYTES, NvramContentsError,
    NvramDevice, NvramSnapshotFormat, NvramSnapshotInfo, SecureBootKeys, UEFI_SCHEMA_ID,
    UEFI_VARIABLES_INSTANCE, UEFI_VARIABLES_KIND, UefiRestoreError, UefiSection,
    cold_restore_uefi_section, verify_nvram_snapshot,
};
use od_storage::vm_container::{
    Container, ContainerError, Expect, Header, IdentityField, Section, SectionBody, SectionData,
    SectionInput, WriteSpec, read_container, write_container,
};

const VM: [u8; 16] = *b"s9-26-vm-ident-1";
const OTHER_VM: [u8; 16] = *b"s9-26-vm-ident-2";
/// Test-only number; profile numbers are not assigned yet.
const FULL_WINDOWS_VM: u32 = 3;
const CHECKPOINT: u64 = 7;
const EPOCH: u64 = 5;
/// An opaque participant that is not UEFI (CPU, RAM, ... in a real checkpoint).
const OTHER_KIND: u32 = u32::from_le_bytes(*b"OTHR");
const OTHER_BYTES: &[u8] = b"opaque state of another participant";
const REQUIRED: &[(u32, u32)] = &[(UEFI_VARIABLES_KIND, UEFI_VARIABLES_INSTANCE)];

#[derive(Debug, PartialEq, Eq)]
enum Failure {
    Container(ContainerError),
    Uefi(UefiRestoreError),
}

// ---------- container glue (test-only) ----------

fn spec(checkpoint: u64, epoch: u64, required: &'static [(u32, u32)]) -> WriteSpec<'static> {
    WriteSpec {
        header: Header {
            vm_identity: VM,
            checkpoint_id: checkpoint,
            epoch,
            profile: FULL_WINDOWS_VM,
        },
        required,
    }
}

fn uefi_input(snapshot: &[u8], schema_version: u32) -> SectionInput<'_> {
    SectionInput {
        kind: UEFI_VARIABLES_KIND,
        instance: UEFI_VARIABLES_INSTANCE,
        schema_id: UEFI_SCHEMA_ID,
        schema_version,
        mandatory: true,
        body: SectionBody::Inline(snapshot),
    }
}

fn other_input() -> SectionInput<'static> {
    SectionInput {
        kind: OTHER_KIND,
        instance: 0,
        schema_id: 1,
        schema_version: 1,
        mandatory: true,
        body: SectionBody::Inline(OTHER_BYTES),
    }
}

/// Writer: verifies the snapshot strictly first and takes the section version
/// from its header, so a writer cannot label v2 bytes as v1.
fn pack(snapshot: &[u8], checkpoint: u64, epoch: u64) -> Vec<u8> {
    let info = verify_nvram_snapshot(snapshot).expect("snapshot to pack must verify");
    pack_with_version(snapshot, info.format.schema_version(), checkpoint, epoch)
}

fn pack_with_version(snapshot: &[u8], version: u32, checkpoint: u64, epoch: u64) -> Vec<u8> {
    write_container(
        &spec(checkpoint, epoch, REQUIRED),
        &[other_input(), uefi_input(snapshot, version)],
    )
    .expect("container must be written")
}

fn expect(checkpoint: u64, required: &'static [(u32, u32)]) -> Expect<'static> {
    Expect {
        vm_identity: VM,
        checkpoint_id: checkpoint,
        profile: FULL_WINDOWS_VM,
        required,
        known_kinds: &[OTHER_KIND],
    }
}

fn uefi_view<'a>(container: &Container<'a>) -> UefiSection<'a> {
    match container.section(UEFI_VARIABLES_KIND, UEFI_VARIABLES_INSTANCE) {
        None => UefiSection::Missing,
        Some(Section {
            data: SectionData::Reference(_),
            ..
        }) => UefiSection::Reference,
        Some(Section {
            schema_id,
            schema_version,
            data: SectionData::Inline(bytes),
            ..
        }) => UefiSection::Inline {
            schema_id: *schema_id,
            schema_version: *schema_version,
            bytes,
        },
    }
}

/// Cold restore: container checks first, then the participant's checks. The
/// profile requires the protected format, so a missing store is never replaced.
fn restore_with(
    bytes: &[u8],
    expect: &Expect<'_>,
    format: NvramSnapshotFormat,
) -> Result<(NvramDevice, NvramSnapshotInfo), Failure> {
    let container = read_container(bytes, expect).map_err(Failure::Container)?;
    cold_restore_uefi_section(uefi_view(&container), format).map_err(Failure::Uefi)
}

fn restore(bytes: &[u8]) -> Result<(NvramDevice, NvramSnapshotInfo), Failure> {
    restore_with(
        bytes,
        &expect(CHECKPOINT, REQUIRED),
        NvramSnapshotFormat::Protected,
    )
}

fn restore_legacy(bytes: &[u8]) -> Result<(NvramDevice, NvramSnapshotInfo), Failure> {
    restore_with(
        bytes,
        &expect(CHECKPOINT, REQUIRED),
        NvramSnapshotFormat::Legacy,
    )
}

fn container_reads(bytes: &[u8]) -> bool {
    read_container(bytes, &expect(CHECKPOINT, REQUIRED)).is_ok()
}

// ---------- CRC helpers: the test recomputes container checksums itself ----------

fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = !0u32;
    for &byte in bytes {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            crc = if crc & 1 != 0 {
                (crc >> 1) ^ 0xEDB8_8320
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

fn u32_at(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap())
}

fn u64_at(bytes: &[u8], at: usize) -> u64 {
    u64::from_le_bytes(bytes[at..at + 8].try_into().unwrap())
}

/// Recomputes every payload CRC, the table CRC and the header CRC, so a change
/// made before it looks like a container written that way.
fn reseal(bytes: &mut [u8]) {
    let count = u32_at(bytes, 68) as usize;
    let table_end = 128 + 64 * count;
    for i in 0..count {
        let entry = 128 + 64 * i;
        let offset = u64_at(bytes, entry + 20) as usize;
        let len = u64_at(bytes, entry + 28) as usize;
        let crc = crc32(&bytes[offset..offset + len]);
        bytes[entry + 44..entry + 48].copy_from_slice(&crc.to_le_bytes());
    }
    let table = crc32(&bytes[128..table_end]);
    bytes[80..84].copy_from_slice(&table.to_le_bytes());
    let header = crc32(&bytes[0..124]);
    bytes[124..128].copy_from_slice(&header.to_le_bytes());
}

/// (table entry offset, payload offset, payload len) of the UEFI section.
fn uefi_entry(bytes: &[u8]) -> (usize, usize, usize) {
    let count = u32_at(bytes, 68) as usize;
    (0..count)
        .map(|i| 128 + 64 * i)
        .find(|&entry| u32_at(bytes, entry) == UEFI_VARIABLES_KIND)
        .map(|entry| {
            (
                entry,
                u64_at(bytes, entry + 20) as usize,
                u64_at(bytes, entry + 28) as usize,
            )
        })
        .expect("UEFI entry")
}

// ---------- device helpers (public MMIO contract, NVRAM_DEVICE.md) ----------

const REGISTERS: u64 = NVRAM_STORE_BYTES as u64;
const ADDR: u64 = 0;
const LEN: u64 = 4;
const CMD: u64 = 8;
const STATUS: u64 = 12;
const ACTIVE_LEN: u64 = 16;
const READ: u32 = 1;
const WRITE: u32 = 2;
const FLUSH: u32 = 3;
const STATUS_READ_READY: u32 = 1;
const STATUS_WRITE_READY: u32 = 2;
const STATUS_COMMITTED: u32 = 3;
const STATUS_INCOMPLETE: u32 = 0x8000_0002;

const SLOT_BYTES: usize = 0x40010;
const SNAPSHOT_HEADER: usize = 16;

fn access(offset: u64, width: AccessWidth) -> DeviceAccess {
    DeviceAccess {
        space: IoSpace::Mmio,
        offset,
        width,
    }
}

fn set_reg(device: &mut NvramDevice, register: u64, value: u32) {
    device
        .write(
            access(REGISTERS + register, AccessWidth::Dword),
            u64::from(value),
        )
        .unwrap();
}

fn get_reg(device: &mut NvramDevice, register: u64) -> u32 {
    device
        .read(access(REGISTERS + register, AccessWidth::Dword))
        .unwrap() as u32
}

fn begin_write(device: &mut NvramDevice, total: usize) {
    set_reg(device, ADDR, 0);
    set_reg(device, LEN, total as u32);
    set_reg(device, CMD, WRITE);
    assert_eq!(get_reg(device, STATUS), STATUS_WRITE_READY);
}

fn put(device: &mut NvramDevice, bytes: &[u8]) {
    for (offset, &byte) in bytes.iter().enumerate() {
        device
            .write(access(offset as u64, AccessWidth::Byte), u64::from(byte))
            .unwrap();
    }
}

fn commit(device: &mut NvramDevice, bytes: &[u8]) {
    begin_write(device, bytes.len());
    put(device, bytes);
    set_reg(device, CMD, FLUSH);
    assert_eq!(get_reg(device, STATUS), STATUS_COMMITTED);
}

fn active_image(device: &mut NvramDevice) -> Vec<u8> {
    let length = get_reg(device, ACTIVE_LEN) as usize;
    set_reg(device, ADDR, 0);
    set_reg(device, LEN, length as u32);
    set_reg(device, CMD, READ);
    assert_eq!(get_reg(device, STATUS), STATUS_READ_READY);
    (0..length as u64)
        .map(|offset| device.read(access(offset, AccessWidth::Byte)).unwrap() as u8)
        .collect()
}

// ---------- fixtures ----------

/// Legacy v1 with two committed banks: generation 1 (empty store) and 2 (A).
fn legacy_snapshot() -> (Vec<u8>, Vec<u8>) {
    let mut device = NvramDevice::new();
    let committed: Vec<u8> = (0..64u8).map(|i| i ^ 0x5a).collect();
    commit(&mut device, &committed);
    (device.nvram_snapshot(), committed)
}

/// Protected v2 in setup mode with two committed banks (generation 1 and 2).
fn protected_device() -> (NvramDevice, Vec<u8>) {
    let mut device = NvramDevice::with_secure_boot_keys(SecureBootKeys::SetupMode).unwrap();
    let image = active_image(&mut device);
    commit(&mut device, &image);
    (device, image)
}

fn flip(bytes: &mut [u8], at: usize) {
    bytes[at] ^= 0x01;
}

fn generation(snapshot: &[u8]) -> u64 {
    verify_nvram_snapshot(snapshot).unwrap().active_generation
}

// ---------- tests ----------

#[test]
fn crc_helper_matches_the_ieee_check_value() {
    assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
}

#[test]
fn v1_and_v2_snapshots_round_trip_through_the_inline_section() {
    let (legacy, committed) = legacy_snapshot();
    let (protected_device, image) = protected_device();
    let protected = protected_device.nvram_snapshot();

    for (snapshot, format, expected_generation) in [
        (&legacy, NvramSnapshotFormat::Legacy, 2),
        (&protected, NvramSnapshotFormat::Protected, 2),
    ] {
        let bytes = pack(snapshot, CHECKPOINT, EPOCH);
        assert_eq!(pack(snapshot, CHECKPOINT, EPOCH), bytes, "deterministic");

        let container = read_container(&bytes, &expect(CHECKPOINT, REQUIRED)).unwrap();
        let section = container
            .section(UEFI_VARIABLES_KIND, UEFI_VARIABLES_INSTANCE)
            .unwrap();
        assert_eq!(section.schema_id, UEFI_SCHEMA_ID);
        assert_eq!(section.schema_version, format.schema_version());
        assert!(section.mandatory);
        assert_eq!(section.data, SectionData::Inline(snapshot.as_slice()));

        let (restored, info) = restore_with(&bytes, &expect(CHECKPOINT, REQUIRED), format).unwrap();
        assert_eq!(info.format, format);
        assert_eq!(info.active_generation, expected_generation);
        assert_eq!(restored.nvram_snapshot(), *snapshot);
    }

    let (mut restored, _) = restore_legacy(&pack(&legacy, CHECKPOINT, EPOCH)).unwrap();
    assert_eq!(active_image(&mut restored), committed);
    let (mut restored, _) = restore(&pack(&protected, CHECKPOINT, EPOCH)).unwrap();
    assert_eq!(active_image(&mut restored), image);
}

#[test]
fn a_flipped_byte_is_caught_by_the_section_crc() {
    let (protected_device, _) = protected_device();
    let bytes = pack(&protected_device.nvram_snapshot(), CHECKPOINT, EPOCH);
    let (_, payload, len) = uefi_entry(&bytes);

    for at in [
        payload,
        payload + 20,
        payload + SLOT_BYTES,
        payload + len - 1,
    ] {
        let mut damaged = bytes.clone();
        flip(&mut damaged, at);
        assert_eq!(
            restore(&damaged).err(),
            Some(Failure::Container(ContainerError::SectionCrc {
                kind: UEFI_VARIABLES_KIND,
                instance: UEFI_VARIABLES_INSTANCE,
            })),
            "payload byte {at}"
        );
    }
    let mut damaged = bytes.clone();
    flip(&mut damaged, 40);
    assert_eq!(
        restore(&damaged).err(),
        Some(Failure::Container(ContainerError::HeaderCrc))
    );
}

#[test]
fn damage_with_fresh_container_crcs_is_caught_by_the_strict_odnvbank_check() {
    let (legacy, _) = legacy_snapshot();
    let (device, _) = protected_device();
    let protected = device.nvram_snapshot();
    let second_slot = SNAPSHOT_HEADER + SLOT_BYTES;

    // Same effect from a container that was repacked or patched with fresh CRCs.
    let (_, payload, _) = uefi_entry(&pack(&protected, CHECKPOINT, EPOCH));
    let mut patched = pack(&protected, CHECKPOINT, EPOCH);
    flip(&mut patched, payload + second_slot + 16);
    reseal(&mut patched);
    assert!(container_reads(&patched), "the container CRCs hold");
    assert_eq!(
        restore(&patched).err(),
        Some(Failure::Uefi(UefiRestoreError::Corrupt(
            NvramContentsError::InvalidProtectedStore
        )))
    );

    // v2: a damaged newer bank is refused, not replaced by the older policy.
    let mut newer_damaged = protected.clone();
    flip(&mut newer_damaged, second_slot + 16);
    let bytes = pack_with_version(&newer_damaged, 2, CHECKPOINT, EPOCH);
    assert!(container_reads(&bytes));
    assert_eq!(
        restore(&bytes).err(),
        Some(Failure::Uefi(UefiRestoreError::Corrupt(
            NvramContentsError::InvalidProtectedStore
        )))
    );
    // The same for the older bank.
    let mut older_damaged = protected.clone();
    flip(&mut older_damaged, SNAPSHOT_HEADER + 16);
    assert_eq!(
        restore(&pack_with_version(&older_damaged, 2, CHECKPOINT, EPOCH)).err(),
        Some(Failure::Uefi(UefiRestoreError::Corrupt(
            NvramContentsError::InvalidProtectedStore
        )))
    );

    // v1: the allowed fallback to the older committed bank.
    let mut legacy_newer_damaged = legacy.clone();
    flip(&mut legacy_newer_damaged, second_slot + 16);
    let (_, info) = restore_legacy(&pack_with_version(
        &legacy_newer_damaged,
        1,
        CHECKPOINT,
        EPOCH,
    ))
    .unwrap();
    assert_eq!(info.active_generation, 1);
    // v1 with no valid bank, a bad header and a truncated snapshot are refused.
    let mut no_bank = legacy.clone();
    flip(&mut no_bank, second_slot + 16);
    flip(&mut no_bank, SNAPSHOT_HEADER + 16);
    let mut bad_magic = legacy.clone();
    flip(&mut bad_magic, 0);
    let truncated = &legacy[..legacy.len() - 1];
    for (snapshot, error) in [
        (&no_bank[..], NvramContentsError::NoValidBank),
        (&bad_magic[..], NvramContentsError::InvalidHeader),
        (truncated, NvramContentsError::InvalidHeader),
    ] {
        let bytes = pack_with_version(snapshot, 1, CHECKPOINT, EPOCH);
        assert!(container_reads(&bytes));
        assert_eq!(
            restore_legacy(&bytes).err(),
            Some(Failure::Uefi(UefiRestoreError::Corrupt(error)))
        );
    }
}

#[test]
fn zeroing_the_whole_newest_v2_slot_with_fresh_crcs_is_accepted_as_an_older_bank() {
    // Documented gap (NVRAM_DEVICE.md, rollback table): an all-zero slot is a
    // legal unwritten bank, so this policy rollback is not detected before MAC.
    let (device, _) = protected_device();
    let mut zeroed = device.nvram_snapshot();
    let slot = SNAPSHOT_HEADER + SLOT_BYTES..SNAPSHOT_HEADER + 2 * SLOT_BYTES;
    zeroed[slot].fill(0);
    let bytes = pack_with_version(&zeroed, 2, CHECKPOINT, EPOCH);
    assert!(container_reads(&bytes));
    let (_, info) = restore(&bytes).unwrap();
    assert_eq!(info.format, NvramSnapshotFormat::Protected);
    assert_eq!(info.active_generation, 1);
}

#[test]
fn container_identity_and_epoch_are_checked_before_the_participant() {
    let (device, _) = protected_device();
    let bytes = pack(&device.nvram_snapshot(), CHECKPOINT, EPOCH);

    let mut other_vm = expect(CHECKPOINT, REQUIRED);
    other_vm.vm_identity = OTHER_VM;
    assert_eq!(
        restore_with(&bytes, &other_vm, NvramSnapshotFormat::Protected).err(),
        Some(Failure::Container(ContainerError::IdentityMismatch {
            field: IdentityField::VmIdentity
        }))
    );
    assert_eq!(
        restore_with(
            &bytes,
            &expect(CHECKPOINT + 1, REQUIRED),
            NvramSnapshotFormat::Protected
        )
        .err(),
        Some(Failure::Container(ContainerError::IdentityMismatch {
            field: IdentityField::CheckpointId
        }))
    );
    let mut other_profile = expect(CHECKPOINT, REQUIRED);
    other_profile.profile = FULL_WINDOWS_VM + 1;
    assert_eq!(
        restore_with(&bytes, &other_profile, NvramSnapshotFormat::Protected).err(),
        Some(Failure::Container(ContainerError::IdentityMismatch {
            field: IdentityField::Profile
        }))
    );

    // The section carries an epoch of another checkpoint (fresh CRCs).
    let (entry, _, _) = uefi_entry(&bytes);
    let mut foreign_epoch = bytes.clone();
    foreign_epoch[entry + 36..entry + 44].copy_from_slice(&(EPOCH + 1).to_le_bytes());
    reseal(&mut foreign_epoch);
    assert_eq!(
        restore(&foreign_epoch).err(),
        Some(Failure::Container(ContainerError::EpochMismatch {
            kind: UEFI_VARIABLES_KIND,
            instance: UEFI_VARIABLES_INSTANCE,
            found: EPOCH + 1,
            expected: EPOCH,
        }))
    );
    // The header epoch moved instead: every section now disagrees with it.
    let mut header_epoch = bytes.clone();
    header_epoch[56..64].copy_from_slice(&(EPOCH + 1).to_le_bytes());
    reseal(&mut header_epoch);
    assert!(matches!(
        restore(&header_epoch).err(),
        Some(Failure::Container(ContainerError::EpochMismatch { .. }))
    ));
    let mut zero_epoch = bytes.clone();
    zero_epoch[56..64].copy_from_slice(&0u64.to_le_bytes());
    reseal(&mut zero_epoch);
    assert_eq!(
        restore(&zero_epoch).err(),
        Some(Failure::Container(ContainerError::BadEpoch { epoch: 0 }))
    );
}

#[test]
fn schema_version_must_equal_the_snapshot_header_version() {
    let (legacy, _) = legacy_snapshot();
    let (device, _) = protected_device();
    let protected = device.nvram_snapshot();

    for (snapshot, header_version, wrong) in
        [(&protected, 2, 1), (&protected, 2, 3), (&legacy, 1, 2)]
    {
        let bytes = pack_with_version(snapshot, wrong, CHECKPOINT, EPOCH);
        assert!(container_reads(&bytes), "the container itself is valid");
        let format = if header_version == 2 {
            NvramSnapshotFormat::Protected
        } else {
            NvramSnapshotFormat::Legacy
        };
        assert_eq!(
            restore_with(&bytes, &expect(CHECKPOINT, REQUIRED), format).err(),
            Some(Failure::Uefi(UefiRestoreError::VersionMismatch {
                section: wrong,
                snapshot: header_version,
            }))
        );
    }
    // A wrong schema_id is a different error.
    let bytes = write_container(
        &spec(CHECKPOINT, EPOCH, REQUIRED),
        &[SectionInput {
            schema_id: UEFI_SCHEMA_ID + 1,
            ..uefi_input(&protected, 2)
        }],
    )
    .unwrap();
    assert_eq!(
        restore(&bytes).err(),
        Some(Failure::Uefi(UefiRestoreError::UnknownSchemaId {
            found: UEFI_SCHEMA_ID + 1
        }))
    );
}

#[test]
fn a_missing_section_is_an_error_and_is_never_replaced_by_an_empty_store() {
    // The writer refuses to produce a container without the required UEFI section.
    assert_eq!(
        write_container(&spec(CHECKPOINT, EPOCH, REQUIRED), &[other_input()]).err(),
        Some(ContainerError::MissingRequiredSection {
            kind: UEFI_VARIABLES_KIND,
            instance: UEFI_VARIABLES_INSTANCE,
        })
    );
    // A container written without any requirement still has no UEFI section.
    let without = write_container(&spec(CHECKPOINT, EPOCH, &[]), &[other_input()]).unwrap();
    assert_eq!(
        restore(&without).err(),
        Some(Failure::Container(ContainerError::MissingRequiredSection {
            kind: UEFI_VARIABLES_KIND,
            instance: UEFI_VARIABLES_INSTANCE,
        }))
    );
    // Even a reader that forgot to require the section gets no device.
    let lenient = Expect {
        known_kinds: &[OTHER_KIND, UEFI_VARIABLES_KIND],
        ..expect(CHECKPOINT, &[])
    };
    assert_eq!(
        restore_with(&without, &lenient, NvramSnapshotFormat::Protected).err(),
        Some(Failure::Uefi(UefiRestoreError::MissingSection))
    );
    assert_eq!(
        restore_with(&without, &lenient, NvramSnapshotFormat::Legacy).err(),
        Some(Failure::Uefi(UefiRestoreError::MissingSection))
    );
    // A reference is not accepted for this participant either.
    let reference = write_container(
        &spec(CHECKPOINT, EPOCH, REQUIRED),
        &[SectionInput {
            body: SectionBody::Reference(od_storage::vm_container::ReferenceId {
                file_uid: [9; 16],
                generation: 1,
                byte_len: 10,
                crc32: 1,
            }),
            ..uefi_input(&[], 2)
        }],
    )
    .unwrap();
    assert_eq!(
        restore(&reference).err(),
        Some(Failure::Uefi(UefiRestoreError::ReferenceSection))
    );
}

#[test]
fn a_v1_snapshot_is_refused_when_the_profile_expects_v2() {
    let (legacy, _) = legacy_snapshot();
    let bytes = pack(&legacy, CHECKPOINT, EPOCH);
    assert!(container_reads(&bytes));
    assert_eq!(
        restore(&bytes).err(),
        Some(Failure::Uefi(
            UefiRestoreError::LegacyWhereProtectedExpected
        ))
    );
    // The fresh store of NvramConfig::Empty is exactly such a v1 snapshot.
    let empty = NvramDevice::new().nvram_snapshot();
    assert_eq!(
        restore(&pack(&empty, CHECKPOINT, EPOCH)).err(),
        Some(Failure::Uefi(
            UefiRestoreError::LegacyWhereProtectedExpected
        ))
    );
    // And the reverse is refused as well.
    let (device, _) = protected_device();
    assert_eq!(
        restore_legacy(&pack(&device.nvram_snapshot(), CHECKPOINT, EPOCH)).err(),
        Some(Failure::Uefi(
            UefiRestoreError::ProtectedWhereLegacyExpected
        ))
    );
}

/// Not a concurrency test: there are no threads and no `NvramHandle`. Device
/// operations are interleaved with snapshots under one `&mut NvramDevice`, so
/// the snapshot is taken at the quantum boundary in the middle of a WRITE.
#[test]
fn a_snapshot_during_an_unfinished_write_restores_the_last_committed_bank() {
    // Legacy: the unfinished WRITE has invalidated the spare bank, the active one stays.
    let mut legacy = NvramDevice::new();
    let committed: Vec<u8> = (0..64u8).map(|i| i.wrapping_mul(3)).collect();
    commit(&mut legacy, &committed);
    let next: Vec<u8> = (0..64u8).map(|i| !i).collect();
    begin_write(&mut legacy, next.len());
    put(&mut legacy, &next[..10]);
    let during = legacy.nvram_snapshot();
    assert_eq!(generation(&during), 2);
    let (mut restored, info) = restore_legacy(&pack(&during, CHECKPOINT, EPOCH)).unwrap();
    assert_eq!(info.active_generation, 2);
    assert_eq!(active_image(&mut restored), committed);
    set_reg(&mut legacy, CMD, FLUSH);
    assert_eq!(get_reg(&mut legacy, STATUS), STATUS_INCOMPLETE);
    assert_eq!(generation(&legacy.nvram_snapshot()), 2);

    // Protected: the candidate sits in staging and never reaches the banks.
    let (mut device, image) = protected_device();
    let before = device.nvram_snapshot();
    begin_write(&mut device, image.len());
    put(&mut device, &image[..image.len() / 2]);
    let during = device.nvram_snapshot();
    assert_eq!(during, before, "staging is not part of the snapshot");
    let (mut restored, info) = restore(&pack(&during, CHECKPOINT, EPOCH)).unwrap();
    assert_eq!(info.active_generation, 2);
    assert_eq!(active_image(&mut restored), image);
    set_reg(&mut device, CMD, FLUSH);
    assert_eq!(get_reg(&mut device, STATUS), STATUS_INCOMPLETE);
    assert_eq!(device.nvram_snapshot(), before);
}

#[test]
fn an_older_valid_container_of_the_same_vm_is_accepted_and_cold_reset_restarts_generation() {
    let (device, _) = protected_device();
    let old_snapshot = device.nvram_snapshot();
    assert_eq!(generation(&old_snapshot), 2);
    let old_container = pack(&old_snapshot, CHECKPOINT, EPOCH);

    // A later checkpoint of the same VM after a cold secure-boot reset.
    let reset =
        NvramDevice::reset_secure_boot_keys(&old_snapshot, SecureBootKeys::SetupMode).unwrap();
    let new_snapshot = reset.nvram_snapshot();
    let new_container = pack(&new_snapshot, CHECKPOINT + 1, EPOCH + 1);
    let new_expect = expect(CHECKPOINT + 1, REQUIRED);

    let (_, new_info) =
        restore_with(&new_container, &new_expect, NvramSnapshotFormat::Protected).unwrap();
    // Observation, not a rule: the generation of the new store starts again at 1.
    assert_eq!(new_info.active_generation, 1);

    // The old, valid and untouched container of the same VM is accepted as well:
    // nothing in the container or in ODNVBANK says that it is outdated.
    let (_, old_info) = restore(&old_container).unwrap();
    assert_eq!(old_info.active_generation, 2);

    // No rollback detector exists in code: acceptance of the old container is
    // all this test records.
}
