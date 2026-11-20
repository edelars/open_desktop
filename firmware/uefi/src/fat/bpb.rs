//! Checked FAT volume geometry (Microsoft EFI FAT Specification 1.03).

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum FatType {
    Fat12,
    Fat16,
    Fat32,
}

impl FatType {
    fn entry_bits(self) -> u64 {
        match self {
            Self::Fat12 => 12,
            Self::Fat16 => 16,
            Self::Fat32 => 32,
        }
    }

    pub(super) fn reserved_cluster(self) -> u32 {
        match self {
            Self::Fat12 => 0x0ff0,
            Self::Fat16 => 0xfff0,
            Self::Fat32 => 0x0fff_fff0,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Root {
    Fixed { offset: u64, entries: u16 },
    Cluster(u32),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct Geometry {
    pub(super) fat_type: FatType,
    pub(super) total_bytes: u64,
    pub(super) bytes_per_sector: u32,
    pub(super) cluster_bytes: u32,
    pub(super) cluster_count: u32,
    pub(super) fat_base: u64,
    pub(super) fat_bytes: u64,
    pub(super) root: Root,
    pub(super) bpb_label: [u8; 11],
    data_base: u64,
}

impl Geometry {
    pub(super) fn parse(boot: &[u8; 512], device_bytes: u64) -> Option<Self> {
        let jump = boot[0] == 0xe9 || (boot[0] == 0xeb && boot[2] == 0x90);
        if !jump || boot[510..] != [0x55, 0xaa] {
            return None;
        }
        let sector_bytes = u32::from(le16(boot, 11));
        let cluster_sectors = u32::from(boot[13]);
        if !matches!(sector_bytes, 512 | 1024 | 2048 | 4096) || !cluster_sectors.is_power_of_two() {
            return None;
        }
        let cluster_bytes = cluster_sectors * sector_bytes;
        if cluster_bytes > 32 * 1024 {
            return None;
        }

        let reserved = u64::from(le16(boot, 14));
        let fats = u64::from(boot[16]);
        let root_entries = le16(boot, 17);
        let short_total = le16(boot, 19);
        let short_fat = le16(boot, 22);
        let total = if short_total == 0 {
            u64::from(le32(boot, 32))
        } else {
            u64::from(short_total)
        };
        let fat_sectors = if short_fat == 0 {
            u64::from(le32(boot, 36))
        } else {
            u64::from(short_fat)
        };
        let sector_bytes64 = u64::from(sector_bytes);
        let total_bytes = total * sector_bytes64;
        if reserved == 0 || fats == 0 || fat_sectors == 0 || total_bytes > device_bytes {
            return None;
        }

        let root_sector = reserved + fats * fat_sectors;
        let root_sectors = (u64::from(root_entries) * 32).div_ceil(sector_bytes64);
        let data_sector = root_sector + root_sectors;
        let count =
            u32::try_from(total.checked_sub(data_sector)? / u64::from(cluster_sectors)).ok()?;
        let fat_type = match count {
            0 => return None,
            1..=4084 => FatType::Fat12,
            4085..=65524 => FatType::Fat16,
            65525..=0x0fff_ffee => FatType::Fat32,
            _ => return None,
        };
        let fat_bytes = fat_sectors * sector_bytes64;
        if fat_bytes < ((u64::from(count) + 2) * fat_type.entry_bits()).div_ceil(8) {
            return None;
        }

        let (root, active_fat, label_at) = if fat_type == FatType::Fat32 {
            if short_fat != 0 || short_total != 0 || root_entries != 0 || le16(boot, 42) != 0 {
                return None;
            }
            let flags = le16(boot, 40);
            let index = if flags & 0x80 != 0 {
                u64::from(flags & 0x0f)
            } else {
                0
            };
            let cluster = le32(boot, 44);
            if index >= fats || !valid_cluster(count, cluster) {
                return None;
            }
            (Root::Cluster(cluster), index, 71)
        } else {
            if short_fat == 0 || root_entries == 0 {
                return None;
            }
            (
                Root::Fixed {
                    offset: root_sector * sector_bytes64,
                    entries: root_entries,
                },
                0,
                43,
            )
        };
        let bpb_label = if boot[label_at - 5] == 0x29 {
            boot[label_at..label_at + 11].try_into().unwrap()
        } else {
            *b"NO NAME    "
        };
        Some(Self {
            fat_type,
            total_bytes,
            bytes_per_sector: sector_bytes,
            cluster_bytes,
            cluster_count: count,
            fat_base: (reserved + active_fat * fat_sectors) * sector_bytes64,
            fat_bytes,
            root,
            bpb_label,
            data_base: data_sector * sector_bytes64,
        })
    }

    pub(super) fn cluster_offset(self, cluster: u32) -> Option<u64> {
        if !valid_cluster(self.cluster_count, cluster) {
            return None;
        }
        Some(self.data_base + u64::from(cluster - 2) * u64::from(self.cluster_bytes))
    }
}

fn valid_cluster(count: u32, value: u32) -> bool {
    value >= 2 && value - 2 < count
}

fn le16(bytes: &[u8], at: usize) -> u16 {
    u16::from_le_bytes(bytes[at..at + 2].try_into().unwrap())
}

fn le32(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap())
}
