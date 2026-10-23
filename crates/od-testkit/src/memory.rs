//! Bounded fixture RAM, not a production MMU or an x86 paging implementation.

use crate::case::MemoryPermissions;
use crate::result::MemoryRegionSnapshot;
use crate::v2::{Fixture, decode_bytes};
use crate::{FormatError, HexU64};
use od_isa::memory::{AccessContext, AccessKind, GuestMemory, MemoryError};

#[derive(Clone, Debug)]
struct Region {
    id: String,
    base: u64,
    permissions: MemoryPermissions,
    bytes: Vec<u8>,
}

#[derive(Clone, Debug)]
pub struct FlatMemory {
    regions: Vec<Region>,
    injected: Option<(u64, AccessKind, MemoryError)>,
}

impl FlatMemory {
    pub fn from_fixture(fixture: &Fixture) -> Result<Self, FormatError> {
        fixture.validate()?;
        let regions = fixture
            .memory
            .iter()
            .map(|region| {
                Ok(Region {
                    id: region.region_id.clone(),
                    base: region.guest_address.get(),
                    permissions: region.permissions,
                    bytes: decode_bytes(&region.initial_bytes_hex)?,
                })
            })
            .collect::<Result<Vec<_>, FormatError>>()?;
        Ok(Self {
            regions,
            injected: None,
        })
    }

    /// Consume this error on the next access of this kind covering `address`.
    /// Injection occurs after overflow checks and before mapping or RAM effects.
    pub fn inject_once(&mut self, address: u64, access: AccessKind, error: MemoryError) {
        self.injected = Some((address, access, error));
    }

    fn injected_error(
        &mut self,
        address: u64,
        length: usize,
        kind: AccessKind,
    ) -> Result<(), MemoryError> {
        let end = address
            .checked_add(length as u64)
            .ok_or(MemoryError::AddressOverflow { address, length })?;
        if self
            .injected
            .as_ref()
            .is_some_and(|(at, access, _)| *access == kind && address <= *at && *at < end)
        {
            let (_, _, error) = self.injected.take().expect("matched injection");
            return Err(error);
        }
        Ok(())
    }

    pub fn snapshot(&self) -> Vec<MemoryRegionSnapshot> {
        self.regions
            .iter()
            .map(|region| MemoryRegionSnapshot {
                region_id: region.id.clone(),
                guest_address: HexU64(region.base),
                bytes_hex: encode_bytes(&region.bytes),
            })
            .collect()
    }

    /// Resolve and check the full access before any RAM/destination mutation.
    fn locations(
        &self,
        address: u64,
        length: usize,
        access: AccessKind,
    ) -> Result<Vec<(usize, usize)>, MemoryError> {
        let length_u64 =
            u64::try_from(length).map_err(|_| MemoryError::AddressOverflow { address, length })?;
        address
            .checked_add(length_u64)
            .ok_or(MemoryError::AddressOverflow { address, length })?;
        let mut locations = Vec::new();
        for offset in 0..length_u64 {
            let current = address + offset;
            let (index, region) = self
                .regions
                .iter()
                .enumerate()
                .find(|(_, region)| {
                    current >= region.base && current - region.base < region.bytes.len() as u64
                })
                .ok_or(MemoryError::Unmapped {
                    address: current,
                    access,
                })?;
            let permitted = match access {
                AccessKind::Fetch => region.permissions.execute,
                AccessKind::Read => region.permissions.read,
                AccessKind::Write => region.permissions.write,
            };
            if !permitted {
                return Err(MemoryError::PermissionDenied {
                    address: current,
                    access,
                });
            }
            locations.push((index, (current - region.base) as usize));
        }
        Ok(locations)
    }
}

impl GuestMemory for FlatMemory {
    fn fetch(&mut self, address: u64, _context: AccessContext) -> Result<u8, MemoryError> {
        self.injected_error(address, 1, AccessKind::Fetch)?;
        let location = self.locations(address, 1, AccessKind::Fetch)?[0];
        Ok(self.regions[location.0].bytes[location.1])
    }

    fn read(
        &mut self,
        address: u64,
        _context: AccessContext,
        destination: &mut [u8],
    ) -> Result<(), MemoryError> {
        self.injected_error(address, destination.len(), AccessKind::Read)?;
        let locations = self.locations(address, destination.len(), AccessKind::Read)?;
        for (byte, (region, offset)) in destination.iter_mut().zip(locations) {
            *byte = self.regions[region].bytes[offset];
        }
        Ok(())
    }

    fn write(
        &mut self,
        address: u64,
        _context: AccessContext,
        source: &[u8],
    ) -> Result<(), MemoryError> {
        self.injected_error(address, source.len(), AccessKind::Write)?;
        let locations = self.locations(address, source.len(), AccessKind::Write)?;
        for (byte, (region, offset)) in source.iter().zip(locations) {
            self.regions[region].bytes[offset] = *byte;
        }
        Ok(())
    }

    fn atomic_exchange_integer(
        &mut self,
        address: u64,
        _context: AccessContext,
        size: od_isa::OperandSize,
        replacement: u64,
    ) -> Result<u64, MemoryError> {
        let length = size.bytes();
        self.injected_error(address, length, AccessKind::Read)?;
        self.injected_error(address, length, AccessKind::Write)?;
        // Validate both sides of the read-modify-write before changing bytes.
        // FlatMemory owns private RAM and exposes no aliases, so this one method
        // call is its serialization point for the bounded fixture backend.
        let read_locations = self.locations(address, length, AccessKind::Read)?;
        let write_locations = self.locations(address, length, AccessKind::Write)?;
        let mut old_bytes = [0u8; 8];
        for (byte, (region, offset)) in old_bytes[..length].iter_mut().zip(read_locations) {
            *byte = self.regions[region].bytes[offset];
        }
        for (byte, (region, offset)) in replacement.to_le_bytes()[..length]
            .iter()
            .zip(write_locations)
        {
            self.regions[region].bytes[offset] = *byte;
        }
        Ok(u64::from_le_bytes(old_bytes))
    }

    fn atomic_fetch_add_integer(
        &mut self,
        address: u64,
        _context: AccessContext,
        size: od_isa::OperandSize,
        addend: u64,
    ) -> Result<u64, MemoryError> {
        let length = size.bytes();
        self.injected_error(address, length, AccessKind::Read)?;
        self.injected_error(address, length, AccessKind::Write)?;
        // Validate the whole read and write ranges before any byte changes.
        // This private-RAM method is the serialization point for FlatMemory.
        let read_locations = self.locations(address, length, AccessKind::Read)?;
        let write_locations = self.locations(address, length, AccessKind::Write)?;
        let mut old_bytes = [0u8; 8];
        for (byte, (region, offset)) in old_bytes[..length].iter_mut().zip(read_locations) {
            *byte = self.regions[region].bytes[offset];
        }
        let old = u64::from_le_bytes(old_bytes);
        let sum = old.wrapping_add(addend) & size.mask();
        for (byte, (region, offset)) in sum.to_le_bytes()[..length].iter().zip(write_locations) {
            self.regions[region].bytes[offset] = *byte;
        }
        Ok(old)
    }

    fn atomic_compare_exchange_integer(
        &mut self,
        address: u64,
        _context: AccessContext,
        size: od_isa::OperandSize,
        expected: u64,
        replacement: u64,
    ) -> Result<u64, MemoryError> {
        let length = size.bytes();
        self.injected_error(address, length, AccessKind::Read)?;
        self.injected_error(address, length, AccessKind::Write)?;
        // CMPXCHG requires write access even when the comparison will fail.
        // Validate both ranges before reading or conditionally storing.
        let read_locations = self.locations(address, length, AccessKind::Read)?;
        let write_locations = self.locations(address, length, AccessKind::Write)?;
        let mut old_bytes = [0u8; 8];
        for (byte, (region, offset)) in old_bytes[..length].iter_mut().zip(read_locations) {
            *byte = self.regions[region].bytes[offset];
        }
        let old = u64::from_le_bytes(old_bytes);
        if old == (expected & size.mask()) {
            for (byte, (region, offset)) in replacement.to_le_bytes()[..length]
                .iter()
                .zip(write_locations)
            {
                self.regions[region].bytes[offset] = *byte;
            }
        }
        Ok(old)
    }

    fn mfence(&mut self, _context: AccessContext) -> Result<(), MemoryError> {
        // FlatMemory owns its bytes and every prior access commits synchronously;
        // no host atomic/thread fence is needed to complete this local RAM model.
        Ok(())
    }
}

pub fn encode_bytes(bytes: &[u8]) -> String {
    use std::fmt::Write;
    let mut result = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        write!(&mut result, "{byte:02x}").expect("writing a String cannot fail");
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::case::MemoryRegionSpec;

    fn fixture() -> Fixture {
        let old = crate::case::Fixture::from_json(include_bytes!(
            "../../../tests/fixtures/x86/s0_09_mov_imm64.case.json"
        ))
        .unwrap();
        Fixture::convert_v1(&old).unwrap()
    }

    #[test]
    fn fetch_read_write_permissions_are_independent() {
        for permissions in 0u8..8 {
            let mut input = fixture();
            input.memory[0].permissions = MemoryPermissions {
                read: permissions & 1 != 0,
                write: permissions & 2 != 0,
                execute: permissions & 4 != 0,
            };
            let mut memory = FlatMemory::from_fixture(&input).unwrap();
            let ctx = AccessContext::default();
            assert_eq!(memory.fetch(0, ctx).is_ok(), permissions & 4 != 0);
            let mut byte = [0xcc];
            let before = memory.snapshot();
            assert_eq!(memory.read(0, ctx, &mut byte).is_ok(), permissions & 1 != 0);
            if permissions & 1 == 0 {
                assert_eq!(byte, [0xcc]);
            }
            assert_eq!(memory.write(0, ctx, &[0x12]).is_ok(), permissions & 2 != 0);
            if permissions & 2 == 0 {
                assert_eq!(memory.snapshot(), before);
            }
        }
    }

    #[test]
    fn atomic_qword_exchange_validates_adjacent_ranges_before_commit() {
        let mut input = fixture();
        input.memory = vec![
            MemoryRegionSpec {
                region_id: "left".into(),
                guest_address: HexU64(0x0020_0ff8),
                length: 8,
                permissions: MemoryPermissions {
                    read: true,
                    write: true,
                    execute: false,
                },
                initial_bytes_hex: "a5a5a5a5a5a58877".into(),
                comparison_mask_hex: "ffffffffffffffff".into(),
            },
            MemoryRegionSpec {
                region_id: "right".into(),
                guest_address: HexU64(0x0020_1000),
                length: 8,
                permissions: MemoryPermissions {
                    read: true,
                    write: true,
                    execute: false,
                },
                initial_bytes_hex: "665544332211a5a5".into(),
                comparison_mask_hex: "ffffffffffffffff".into(),
            },
        ];
        let mut memory = FlatMemory::from_fixture(&input).unwrap();
        assert_eq!(
            memory.atomic_exchange_integer(
                0x0020_0ffe,
                AccessContext::default(),
                od_isa::OperandSize::Qword,
                0x0000_0000_0020_0ffe,
            ),
            Ok(0x1122_3344_5566_7788)
        );
        let after = memory.snapshot();
        assert_eq!(after[0].bytes_hex, "a5a5a5a5a5a5fe0f");
        assert_eq!(after[1].bytes_hex, "200000000000a5a5");

        let mut denied = FlatMemory::from_fixture(&input).unwrap();
        denied.regions[1].permissions.write = false;
        let before = denied.snapshot();
        assert!(matches!(
            denied.atomic_exchange_integer(
                0x0020_0ffe,
                AccessContext::default(),
                od_isa::OperandSize::Qword,
                0x0000_0000_0020_0ffe,
            ),
            Err(MemoryError::PermissionDenied {
                address: 0x0020_1000,
                access: AccessKind::Write,
            })
        ));
        assert_eq!(denied.snapshot(), before);

        let mut injected = FlatMemory::from_fixture(&input).unwrap();
        injected.inject_once(
            0x0020_1003,
            AccessKind::Write,
            MemoryError::Backend("injected atomic failure".into()),
        );
        let before = injected.snapshot();
        assert_eq!(
            injected.atomic_exchange_integer(
                0x0020_0ffe,
                AccessContext::default(),
                od_isa::OperandSize::Qword,
                0x0000_0000_0020_0ffe,
            ),
            Err(MemoryError::Backend("injected atomic failure".into()))
        );
        assert_eq!(injected.snapshot(), before);
    }

    #[test]
    fn atomic_qword_fetch_add_validates_adjacent_ranges_before_commit() {
        let mut input = fixture();
        input.memory = vec![
            MemoryRegionSpec {
                region_id: "left".into(),
                guest_address: HexU64(0x0020_0ff8),
                length: 8,
                permissions: MemoryPermissions {
                    read: true,
                    write: true,
                    execute: false,
                },
                initial_bytes_hex: "a5a5a5a5a5a58877".into(),
                comparison_mask_hex: "ffffffffffffffff".into(),
            },
            MemoryRegionSpec {
                region_id: "right".into(),
                guest_address: HexU64(0x0020_1000),
                length: 8,
                permissions: MemoryPermissions {
                    read: true,
                    write: true,
                    execute: false,
                },
                initial_bytes_hex: "665544332211a5a5".into(),
                comparison_mask_hex: "ffffffffffffffff".into(),
            },
        ];
        let mut memory = FlatMemory::from_fixture(&input).unwrap();
        assert_eq!(
            memory.atomic_fetch_add_integer(
                0x0020_0ffe,
                AccessContext::default(),
                od_isa::OperandSize::Qword,
                1,
            ),
            Ok(0x1122_3344_5566_7788)
        );
        let after = memory.snapshot();
        assert_eq!(after[0].bytes_hex, "a5a5a5a5a5a58977");
        assert_eq!(after[1].bytes_hex, "665544332211a5a5");

        let mut denied = FlatMemory::from_fixture(&input).unwrap();
        denied.regions[1].permissions.write = false;
        let before = denied.snapshot();
        assert!(matches!(
            denied.atomic_fetch_add_integer(
                0x0020_0ffe,
                AccessContext::default(),
                od_isa::OperandSize::Qword,
                1,
            ),
            Err(MemoryError::PermissionDenied {
                address: 0x0020_1000,
                access: AccessKind::Write,
            })
        ));
        assert_eq!(denied.snapshot(), before);

        let mut injected = FlatMemory::from_fixture(&input).unwrap();
        injected.inject_once(
            0x0020_1003,
            AccessKind::Write,
            MemoryError::Backend("injected atomic add failure".into()),
        );
        let before = injected.snapshot();
        assert_eq!(
            injected.atomic_fetch_add_integer(
                0x0020_0ffe,
                AccessContext::default(),
                od_isa::OperandSize::Qword,
                1,
            ),
            Err(MemoryError::Backend("injected atomic add failure".into()))
        );
        assert_eq!(injected.snapshot(), before);
    }

    #[test]
    fn atomic_qword_compare_exchange_validates_write_access_on_mismatch() {
        let mut input = fixture();
        input.memory = vec![
            MemoryRegionSpec {
                region_id: "left".into(),
                guest_address: HexU64(0x2000),
                length: 7,
                permissions: MemoryPermissions {
                    read: true,
                    write: true,
                    execute: false,
                },
                initial_bytes_hex: "a5a5a503200000".into(),
                comparison_mask_hex: "ffffffffffffff".into(),
            },
            MemoryRegionSpec {
                region_id: "right".into(),
                guest_address: HexU64(0x2007),
                length: 8,
                permissions: MemoryPermissions {
                    read: true,
                    write: true,
                    execute: false,
                },
                initial_bytes_hex: "00000000a5a5a5a5".into(),
                comparison_mask_hex: "ffffffffffffffff".into(),
            },
        ];
        let mut memory = FlatMemory::from_fixture(&input).unwrap();
        assert_eq!(
            memory.atomic_compare_exchange_integer(
                0x2003,
                AccessContext::default(),
                od_isa::OperandSize::Qword,
                0x2003,
                0x8877_6655_4433_2211,
            ),
            Ok(0x2003)
        );
        let after = memory.snapshot();
        assert_eq!(after[0].bytes_hex, "a5a5a511223344");
        assert_eq!(after[1].bytes_hex, "55667788a5a5a5a5");

        let mut mismatch = FlatMemory::from_fixture(&input).unwrap();
        let before = mismatch.snapshot();
        assert_eq!(
            mismatch.atomic_compare_exchange_integer(
                0x2003,
                AccessContext::default(),
                od_isa::OperandSize::Qword,
                0x9999,
                0x8877_6655_4433_2211,
            ),
            Ok(0x2003)
        );
        assert_eq!(mismatch.snapshot(), before);

        let mut denied = FlatMemory::from_fixture(&input).unwrap();
        denied.regions[1].permissions.write = false;
        let before = denied.snapshot();
        assert!(matches!(
            denied.atomic_compare_exchange_integer(
                0x2003,
                AccessContext::default(),
                od_isa::OperandSize::Qword,
                0x9999,
                0x8877_6655_4433_2211,
            ),
            Err(MemoryError::PermissionDenied {
                address: 0x2007,
                access: AccessKind::Write,
            })
        ));
        assert_eq!(denied.snapshot(), before);

        let mut injected = FlatMemory::from_fixture(&input).unwrap();
        injected.inject_once(
            0x2008,
            AccessKind::Write,
            MemoryError::Backend("injected compare-exchange write failure".into()),
        );
        let before = injected.snapshot();
        assert_eq!(
            injected.atomic_compare_exchange_integer(
                0x2003,
                AccessContext::default(),
                od_isa::OperandSize::Qword,
                0x9999,
                0x8877_6655_4433_2211,
            ),
            Err(MemoryError::Backend(
                "injected compare-exchange write failure".into()
            ))
        );
        assert_eq!(injected.snapshot(), before);
    }

    #[test]
    fn ram_crosses_guest_and_host_page_boundaries_and_adjacent_regions() {
        let mut input = fixture();
        input.memory = vec![
            MemoryRegionSpec {
                region_id: "ram".into(),
                guest_address: HexU64(0),
                length: 16384,
                permissions: MemoryPermissions {
                    read: true,
                    write: true,
                    execute: false,
                },
                initial_bytes_hex: "00".repeat(16384),
                comparison_mask_hex: "ff".repeat(16384),
            },
            MemoryRegionSpec {
                region_id: "next".into(),
                guest_address: HexU64(16384),
                length: 8,
                permissions: MemoryPermissions {
                    read: true,
                    write: true,
                    execute: true,
                },
                initial_bytes_hex: "00".repeat(8),
                comparison_mask_hex: "ff".repeat(8),
            },
        ];
        // The full 64 KiB mapping limit must be expressible in the v2 loader.
        let input = Fixture::from_json(&input.to_json().unwrap())
            .unwrap()
            .fixture;
        let mut memory = FlatMemory::from_fixture(&input).unwrap();
        for address in [4094, 8190, 12286, 16382] {
            let bytes = [1, 2, 3, 4, 5, 6, 7, 8];
            memory
                .write(address, AccessContext::default(), &bytes)
                .unwrap();
            let mut actual = [0; 8];
            memory
                .read(address, AccessContext::default(), &mut actual)
                .unwrap();
            assert_eq!(actual, bytes);
        }
        assert_eq!(memory.fetch(16384, AccessContext::default()).unwrap(), 3);
    }

    #[test]
    fn crossing_failures_leave_ram_and_read_destinations_unchanged() {
        let mut input = fixture();
        input.memory[0].permissions.write = true;
        let mut memory = FlatMemory::from_fixture(&input).unwrap();
        let end = u64::from(input.memory[0].length);
        let before = memory.snapshot();
        assert_eq!(
            memory.write(end - 1, AccessContext::default(), &[1, 2]),
            Err(MemoryError::Unmapped {
                address: end,
                access: AccessKind::Write
            })
        );
        assert_eq!(memory.snapshot(), before);
        let mut actual = [0xcc; 2];
        assert!(
            memory
                .read(end - 1, AccessContext::default(), &mut actual)
                .is_err()
        );
        assert_eq!(actual, [0xcc; 2]);
        assert!(matches!(
            memory.read(u64::MAX, AccessContext::default(), &mut actual),
            Err(MemoryError::AddressOverflow { .. })
        ));
        assert_eq!(actual, [0xcc; 2]);
        assert_eq!(memory.snapshot(), before);
    }

    #[test]
    fn adjacent_region_permission_failure_checks_all_bytes_before_commit() {
        let mut input = fixture();
        input.memory[0].permissions.write = true;
        let end = u64::from(input.memory[0].length);
        let mut second = input.memory[0].clone();
        second.region_id = "second".into();
        second.guest_address = HexU64(end);
        second.permissions.read = false;
        second.permissions.write = false;
        input.memory.push(second);
        let mut memory = FlatMemory::from_fixture(&input).unwrap();
        let before = memory.snapshot();
        let ctx = AccessContext::default();
        assert_eq!(
            memory.write(end - 1, ctx, &[1, 2]),
            Err(MemoryError::PermissionDenied {
                address: end,
                access: AccessKind::Write,
            })
        );
        assert_eq!(memory.snapshot(), before);
        let mut destination = [0xcc; 2];
        assert_eq!(
            memory.read(end - 1, ctx, &mut destination),
            Err(MemoryError::PermissionDenied {
                address: end,
                access: AccessKind::Read,
            })
        );
        assert_eq!(destination, [0xcc; 2]);
    }

    #[test]
    fn injection_is_consumed_only_by_matching_nonoverflowing_access() {
        let mut input = fixture();
        input.memory[0].permissions.write = true;
        let mut memory = FlatMemory::from_fixture(&input).unwrap();
        let ctx = AccessContext::default();
        let error = MemoryError::Backend("once".into());
        memory.inject_once(1, AccessKind::Read, error.clone());
        let expected = memory.snapshot();
        memory.fetch(1, ctx).unwrap();
        memory.write(1, ctx, &[0x12]).unwrap();
        let mut destination = [0xcc; 2];
        assert!(matches!(
            memory.read(u64::MAX, ctx, &mut destination),
            Err(MemoryError::AddressOverflow { .. })
        ));
        assert_eq!(memory.read(0, ctx, &mut destination), Err(error));
        assert_eq!(destination, [0xcc; 2]);
        memory.read(0, ctx, &mut destination).unwrap();
        assert_eq!(destination[1], 0x12);
        assert_eq!(
            &memory.snapshot()[0].bytes_hex[..2],
            &expected[0].bytes_hex[..2]
        );
    }

    #[test]
    fn little_endian_widths_and_injected_architectural_vs_backend_errors() {
        use od_isa::OperandSize;
        use od_isa::memory::ArchitecturalFault;
        let mut input = fixture();
        input.memory[0].permissions.write = true;
        let mut memory = FlatMemory::from_fixture(&input).unwrap();
        let ctx = AccessContext::default();
        for size in [
            OperandSize::Byte,
            OperandSize::Word,
            OperandSize::Dword,
            OperandSize::Qword,
        ] {
            memory
                .write_integer(1, ctx, size, 0x8877_6655_4433_2211)
                .unwrap();
            assert_eq!(
                memory.read_integer(1, ctx, size).unwrap(),
                0x8877_6655_4433_2211 & size.mask()
            );
            assert!(
                memory.snapshot()[0].bytes_hex[2..]
                    .starts_with(&"1122334455667788"[..size.bytes() * 2])
            );
        }
        let fault = MemoryError::Architectural(ArchitecturalFault {
            vector: 13,
            error_code: Some(0),
            linear_address: None,
        });
        for kind in [AccessKind::Fetch, AccessKind::Read, AccessKind::Write] {
            let before = memory.snapshot();
            for error in [
                fault.clone(),
                MemoryError::Backend("injected mapping failure".into()),
            ] {
                memory.inject_once(1, kind, error.clone());
                let mut bytes = [0xcc; 8];
                let result = match kind {
                    AccessKind::Fetch => memory.fetch(1, ctx).map(|_| ()),
                    AccessKind::Read => memory.read(1, ctx, &mut bytes),
                    AccessKind::Write => memory.write(1, ctx, &bytes),
                };
                assert_eq!(result, Err(error));
                assert_eq!(bytes, [0xcc; 8]);
                assert_eq!(memory.snapshot(), before);
            }
        }
    }
}
