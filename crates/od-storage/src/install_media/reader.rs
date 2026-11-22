//! Reads back the GPT and FAT32 layout that this module writes (ESP or Microsoft
//! Basic Data partition), to extend a converted installation disk by files and
//! to check a written disk. It accepts only that layout: one partition at the fixed start, 512-byte
//! sectors, FAT32 with two FATs and no deleted-entry recovery.

use std::collections::HashSet;

use crate::crc32::crc32;
use crate::io::{CheckedStorage, StorageBackend};

use super::ImportError;
use super::fat32::MAX_FAT_BYTES;
use super::gpt::{self, PARTITION_START};
use super::tree::{Contents, Entry, Extent, File, FileData, Name};

const SECTOR: u64 = 512;
const MAX_DIRECTORY_BYTES: u64 = 16 * 1024 * 1024;
const MAX_DEPTH: usize = 16;
const MAX_ENTRIES: usize = 1 << 20;
const FAT_END: u32 = 0x0fff_fff8;

pub(super) struct Disk {
    pub(super) label: [u8; 11],
    pub(super) partition_type: [u8; 16],
    pub(super) entries: Vec<Entry>,
}

fn corrupt(reason: &'static str) -> ImportError {
    ImportError::Corrupt(reason)
}

fn le16(bytes: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([bytes[at], bytes[at + 1]])
}

fn le32(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]])
}

fn le64(bytes: &[u8], at: usize) -> u64 {
    let mut raw = [0; 8];
    raw.copy_from_slice(&bytes[at..at + 8]);
    u64::from_le_bytes(raw)
}

struct Volume<'a, B: StorageBackend> {
    source: &'a CheckedStorage<B>,
    data_base: u64,
    cluster_bytes: u64,
    cluster_count: u32,
    fat: Vec<u32>,
}

/// The first `disk_len` bytes of `source` must be the disk.
pub(super) fn read<B: StorageBackend>(
    source: &CheckedStorage<B>,
    disk_len: u64,
) -> Result<Disk, ImportError> {
    if disk_len < (u64::from(PARTITION_START) + 1) * SECTOR || !disk_len.is_multiple_of(SECTOR) {
        return Err(corrupt("media_disk_size"));
    }
    let mut header = [0; 512];
    source.read_exact_at(SECTOR, &mut header)?;
    if &header[..8] != b"EFI PART" || le32(&header, 12) != 92 {
        return Err(corrupt("media_gpt_header"));
    }
    let stored_crc = le32(&header, 16);
    let mut checked = header;
    checked[16..20].fill(0);
    if crc32(&checked[..92]) != stored_crc {
        return Err(corrupt("media_gpt_crc"));
    }
    if le64(&header, 72) != 2 || le32(&header, 80) == 0 || le32(&header, 84) != 128 {
        return Err(corrupt("media_gpt_table"));
    }
    if le32(&header, 80) > 128 {
        return Err(ImportError::Unsupported("media_gpt_table"));
    }
    let mut table = vec![0; (le32(&header, 80) * 128) as usize];
    source.read_exact_at(2 * SECTOR, &mut table)?;
    if crc32(&table) != le32(&header, 88) {
        return Err(corrupt("media_gpt_crc"));
    }
    let entry = &table[..128];
    let first = le64(entry, 32);
    let last = le64(entry, 40);
    // Both values come from the disk: a hostile last LBA must not overflow.
    let partition_end = last
        .checked_add(1)
        .and_then(|sectors| sectors.checked_mul(SECTOR));
    if first != u64::from(PARTITION_START)
        || last < first
        || partition_end.is_none_or(|end| end > disk_len)
    {
        return Err(ImportError::Unsupported("media_partition_layout"));
    }
    // Now first <= last < disk_len / SECTOR, so these cannot overflow.
    let partition_sectors = last - first + 1;
    if table[128..].iter().any(|&byte| byte != 0) {
        return Err(ImportError::Unsupported("media_partition_layout"));
    }
    let mut partition_type = [0; 16];
    partition_type.copy_from_slice(&entry[..16]);
    if partition_type != gpt::ESP.type_guid() && partition_type != gpt::BASIC_DATA.type_guid() {
        return Err(ImportError::Unsupported("media_partition_type"));
    }

    let base = first * SECTOR;
    let mut boot = [0; 512];
    source.read_exact_at(base, &mut boot)?;
    if boot[510..] != [0x55, 0xaa] || &boot[82..90] != b"FAT32   " {
        return Err(corrupt("media_fat_boot"));
    }
    let cluster_sectors = u64::from(boot[13]);
    let reserved = u64::from(le16(&boot, 14));
    let fats = u64::from(boot[16]);
    let total = u64::from(le32(&boot, 32));
    let fat_sectors = u64::from(le32(&boot, 36));
    if le16(&boot, 11) != 512
        || cluster_sectors == 0
        || !cluster_sectors.is_power_of_two()
        || fats != 2
        || reserved == 0
        || fat_sectors == 0
        || le32(&boot, 44) != 2
        || le32(&boot, 28) != PARTITION_START
        || le16(&boot, 17) != 0
        || le16(&boot, 19) != 0
        || total > partition_sectors
    {
        return Err(ImportError::Unsupported("media_fat_layout"));
    }
    let overhead = fats
        .checked_mul(fat_sectors)
        .and_then(|fat_area| fat_area.checked_add(reserved))
        .ok_or(ImportError::Unsupported("media_fat_layout"))?;
    let fat_bytes_len = fat_sectors
        .checked_mul(SECTOR)
        .ok_or(ImportError::Unsupported("media_fat_layout"))?;
    if total <= overhead || fat_bytes_len > MAX_FAT_BYTES {
        return Err(ImportError::Unsupported("media_fat_layout"));
    }
    let cluster_count = u32::try_from((total - overhead) / cluster_sectors)
        .map_err(|_| ImportError::Unsupported("media_fat_layout"))?;
    if u64::from(cluster_count) + 2 > fat_bytes_len / 4 {
        return Err(corrupt("media_fat_size"));
    }
    // overhead < total <= partition_sectors, so the FATs and the data area lie
    // inside the partition that was checked against disk_len above.
    let mut fat_bytes = vec![0; fat_bytes_len as usize];
    source.read_exact_at(base + reserved * SECTOR, &mut fat_bytes)?;
    let fat = fat_bytes
        .chunks_exact(4)
        .map(|word| le32(word, 0) & 0x0fff_ffff)
        .collect();
    let volume = Volume {
        source,
        data_base: base + overhead * SECTOR,
        cluster_bytes: cluster_sectors * SECTOR,
        cluster_count,
        fat,
    };
    let mut label = [0; 11];
    label.copy_from_slice(&boot[71..82]);
    let mut seen = 0;
    let mut visited = HashSet::new();
    let entries = volume.directory(2, 0, &mut seen, &mut visited)?;
    Ok(Disk {
        label,
        partition_type,
        entries,
    })
}

impl<B: StorageBackend> Volume<'_, B> {
    /// Extents of the cluster chain starting at `first` and the bytes it holds.
    fn chain(&self, first: u32) -> Result<(Vec<Extent>, u64), ImportError> {
        let mut extents: Vec<Extent> = Vec::new();
        let mut cluster = first;
        let mut capacity = 0;
        for _ in 0..self.cluster_count {
            if cluster < 2 || cluster >= self.cluster_count + 2 {
                return Err(corrupt("media_fat_chain"));
            }
            let offset = u64::from(cluster - 2)
                .checked_mul(self.cluster_bytes)
                .and_then(|inside| inside.checked_add(self.data_base))
                .ok_or(corrupt("media_fat_chain"))?;
            match extents.last_mut() {
                Some(last) if last.offset.checked_add(last.length) == Some(offset) => {
                    last.length += self.cluster_bytes;
                }
                _ => extents.push(Extent {
                    offset,
                    length: self.cluster_bytes,
                }),
            }
            capacity += self.cluster_bytes;
            let next = self.fat[cluster as usize];
            if next >= FAT_END {
                return Ok((extents, capacity));
            }
            cluster = next;
        }
        Err(corrupt("media_fat_loop"))
    }

    fn file(&self, first: u32, length: u32) -> Result<File, ImportError> {
        if length == 0 {
            return File::new(0, Vec::new());
        }
        if first == 0 {
            return Err(corrupt("media_fat_chain"));
        }
        let (mut extents, capacity) = self.chain(first)?;
        let length = u64::from(length);
        if capacity < length {
            return Err(corrupt("media_fat_chain"));
        }
        let mut excess = capacity - length;
        while excess != 0 {
            let last = extents.last_mut().ok_or(corrupt("media_fat_chain"))?;
            let cut = excess.min(last.length);
            last.length -= cut;
            excess -= cut;
            if last.length == 0 {
                extents.pop();
            }
        }
        File::new(length, extents)
    }

    fn directory(
        &self,
        first: u32,
        depth: usize,
        seen: &mut usize,
        visited: &mut HashSet<u32>,
    ) -> Result<Vec<Entry>, ImportError> {
        if depth > MAX_DEPTH {
            return Err(ImportError::Unsupported("media_directory_depth"));
        }
        // A directory entry that points at an ancestor (or at a directory read
        // before) would be followed again and again.
        if !visited.insert(first) {
            return Err(corrupt("media_directory_cycle"));
        }
        let (extents, capacity) = self.chain(first)?;
        if capacity > MAX_DIRECTORY_BYTES {
            return Err(ImportError::Unsupported("media_directory_size"));
        }
        let mut bytes = Vec::with_capacity(capacity as usize);
        for extent in &extents {
            let start = bytes.len();
            bytes.resize(start + extent.length as usize, 0);
            self.source
                .read_exact_at(extent.offset, &mut bytes[start..])?;
        }
        let mut entries = Vec::new();
        let mut long: Option<LongName> = None;
        for row in bytes.chunks_exact(32) {
            if row[0] == 0 {
                break;
            }
            if row[0] == 0xe5 {
                long = None;
                continue;
            }
            let attributes = row[11];
            if attributes & 0x3f == 0x0f {
                long.get_or_insert_with(LongName::default).push(row)?;
                continue;
            }
            let pending = long.take();
            if attributes & 0x08 != 0 || row[..2] == *b". " || row[..3] == *b".. " {
                continue;
            }
            *seen += 1;
            if *seen > MAX_ENTRIES {
                return Err(ImportError::Unsupported("media_entry_count"));
            }
            let mut short = [0; 11];
            short.copy_from_slice(&row[..11]);
            let name = match pending {
                Some(long) => Name::Long(long.finish(&short)?),
                None => Name::Short(short),
            };
            let cluster = (u32::from(le16(row, 20)) << 16) | u32::from(le16(row, 26));
            let contents = if attributes & 0x10 != 0 {
                if cluster < 2 {
                    return Err(corrupt("media_fat_chain"));
                }
                Contents::Directory(self.directory(cluster, depth + 1, seen, visited)?)
            } else {
                Contents::File(FileData::Source(self.file(cluster, le32(row, 28))?))
            };
            entries.push(Entry { name, contents });
        }
        Ok(entries)
    }
}

/// Long-name slots in the order they appear on disk (last part first).
#[derive(Default)]
struct LongName {
    slots: Vec<(u8, [u16; 13])>,
    checksum: u8,
    total: usize,
}

impl LongName {
    fn push(&mut self, row: &[u8]) -> Result<(), ImportError> {
        const OFFSETS: [usize; 13] = [1, 3, 5, 7, 9, 14, 16, 18, 20, 22, 24, 28, 30];
        let order = row[0] & 0x3f;
        if row[0] & 0x40 != 0 {
            if !self.slots.is_empty() || order == 0 || order > 20 {
                return Err(corrupt("media_fat_lfn"));
            }
            self.total = usize::from(order);
            self.checksum = row[13];
        } else if self.slots.is_empty()
            || self.checksum != row[13]
            || order + 1 != self.slots[self.slots.len() - 1].0
        {
            return Err(corrupt("media_fat_lfn"));
        }
        let mut units = [0; 13];
        for (unit, offset) in units.iter_mut().zip(OFFSETS) {
            *unit = le16(row, offset);
        }
        self.slots.push((order, units));
        Ok(())
    }

    fn finish(self, short: &[u8; 11]) -> Result<String, ImportError> {
        let sum = short
            .iter()
            .fold(0u8, |sum, &byte| sum.rotate_right(1).wrapping_add(byte));
        if self.slots.len() != self.total
            || self.slots[self.slots.len() - 1].0 != 1
            || sum != self.checksum
        {
            return Err(corrupt("media_fat_lfn"));
        }
        let mut name = String::new();
        'slots: for (_, units) in self.slots.iter().rev() {
            for &unit in units {
                match unit {
                    0 => break 'slots,
                    0x20..=0x7e => name.push(char::from(unit as u8)),
                    _ => return Err(ImportError::Unsupported("media_name_charset")),
                }
            }
        }
        if name.is_empty() {
            return Err(corrupt("media_fat_lfn"));
        }
        Ok(name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::install_media::{AddedFile, InstallMedia, list_disk};
    use crate::io::StorageCapabilities;
    use crate::testing::memory::MemoryStorage;

    const DISK_START: usize = PARTITION_START as usize * SECTOR as usize;

    /// A written payload disk with the file `sub/a.txt`.
    fn sample_disk() -> Vec<u8> {
        let source = CheckedStorage::new(MemoryStorage::new_read_only(b"hello")).unwrap();
        let files = [AddedFile {
            path: "sub/a.txt",
            source_offset: 0,
            length: 5,
        }];
        let media = InstallMedia::payload_disk(&source, "DATA", 1 << 20, &files).unwrap();
        let mut capabilities = StorageCapabilities::read_write(64 << 20);
        capabilities.file_durability = true;
        let mut backend = MemoryStorage::with_capabilities(&[], capabilities);
        backend.set_flush_file_ok();
        let mut target = CheckedStorage::new(backend).unwrap();
        media.write_and_verify(&source, &mut target, 7).unwrap();
        target.into_backend().ok().unwrap().bytes().to_vec()
    }

    fn list(bytes: &[u8]) -> Result<crate::install_media::DiskListing, ImportError> {
        let source = CheckedStorage::new(MemoryStorage::new_read_only(bytes)).unwrap();
        list_disk(&source, bytes.len() as u64)
    }

    /// Recomputes the table and header CRCs after the test changed them.
    fn fix_crcs(disk: &mut [u8]) {
        let count = le32(&disk[512..], 80) as usize;
        let table = crc32(&disk[1024..1024 + count * 128]);
        disk[512 + 88..512 + 92].copy_from_slice(&table.to_le_bytes());
        disk[512 + 16..512 + 20].fill(0);
        let header = crc32(&disk[512..512 + 92]);
        disk[512 + 16..512 + 20].copy_from_slice(&header.to_le_bytes());
    }

    fn set_partition_last(disk: &mut [u8], last: u64) {
        disk[1024 + 40..1024 + 48].copy_from_slice(&last.to_le_bytes());
        fix_crcs(disk);
    }

    fn data_base(disk: &[u8]) -> usize {
        let boot = &disk[DISK_START..DISK_START + 512];
        let reserved = le16(boot, 14) as usize;
        let fat_sectors = le32(boot, 36) as usize;
        DISK_START + (reserved + 2 * fat_sectors) * 512
    }

    fn cluster_offset(disk: &[u8], cluster: u32) -> usize {
        let cluster_bytes = disk[DISK_START + 13] as usize * 512;
        data_base(disk) + (cluster as usize - 2) * cluster_bytes
    }

    /// Offset of the first short entry with these attributes in a directory.
    fn find_row(disk: &[u8], directory: usize, attributes: u8) -> usize {
        (0..16)
            .map(|row| directory + row * 32)
            .find(|&at| disk[at] != 0 && disk[at + 11] != 0x0f && disk[at + 11] == attributes)
            .unwrap()
    }

    fn point_to(disk: &mut [u8], row: usize, cluster: u32) {
        disk[row + 11] = 0x10;
        disk[row + 20..row + 22].copy_from_slice(&((cluster >> 16) as u16).to_le_bytes());
        disk[row + 26..row + 28].copy_from_slice(&(cluster as u16).to_le_bytes());
    }

    #[test]
    fn sample_disk_is_listed() {
        let listing = list(&sample_disk()).unwrap();
        assert_eq!(listing.directories, ["sub"]);
        assert_eq!(listing.files.len(), 1);
        assert_eq!(listing.files[0].path, "sub/a.txt");
    }

    #[test]
    fn partition_last_lba_at_the_u64_limit_is_an_error_not_a_panic() {
        for last in [u64::MAX, u64::MAX - 1, u64::MAX / SECTOR, 1 << 55] {
            let mut disk = sample_disk();
            set_partition_last(&mut disk, last);
            assert!(
                matches!(
                    list(&disk),
                    Err(ImportError::Unsupported("media_partition_layout"))
                ),
                "last = {last:#x}"
            );
        }
    }

    #[test]
    fn partition_that_ends_past_the_disk_is_refused() {
        let mut disk = sample_disk();
        let sectors = disk.len() as u64 / SECTOR;
        set_partition_last(&mut disk, sectors);
        assert!(matches!(
            list(&disk),
            Err(ImportError::Unsupported("media_partition_layout"))
        ));
        set_partition_last(&mut disk, sectors - 1);
        assert!(list(&disk).is_ok());
    }

    #[test]
    fn partition_that_ends_before_it_starts_is_refused() {
        let mut disk = sample_disk();
        set_partition_last(&mut disk, u64::from(PARTITION_START) - 1);
        assert!(matches!(
            list(&disk),
            Err(ImportError::Unsupported("media_partition_layout"))
        ));
    }

    #[test]
    fn second_overlapping_partition_entry_is_refused() {
        let mut disk = sample_disk();
        let (head, tail) = disk[1024..].split_at_mut(128);
        tail[..128].copy_from_slice(head);
        fix_crcs(&mut disk);
        assert!(matches!(
            list(&disk),
            Err(ImportError::Unsupported("media_partition_layout"))
        ));
    }

    #[test]
    fn wrong_gpt_crc_is_refused() {
        let mut table = sample_disk();
        table[1024 + 40] ^= 1;
        assert!(matches!(
            list(&table),
            Err(ImportError::Corrupt("media_gpt_crc"))
        ));
        let mut header = sample_disk();
        header[512 + 40] ^= 1;
        assert!(matches!(
            list(&header),
            Err(ImportError::Corrupt("media_gpt_crc"))
        ));
    }

    #[test]
    fn fat_volume_larger_than_the_partition_is_refused() {
        let mut disk = sample_disk();
        disk[DISK_START + 32..DISK_START + 36].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(matches!(
            list(&disk),
            Err(ImportError::Unsupported("media_fat_layout"))
        ));
    }

    #[test]
    fn oversized_fat_is_refused_before_it_is_read() {
        let mut disk = sample_disk();
        disk[DISK_START + 36..DISK_START + 40].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(matches!(
            list(&disk),
            Err(ImportError::Unsupported("media_fat_layout"))
        ));
    }

    /// A disk of `total` sectors in the partition whose FAT has `fat_sectors`
    /// sectors; the length, the partition and the volume all fit together, so
    /// only the size of the FAT can be the reason for a refusal.
    fn disk_with_fat_sectors(fat_sectors: u32, total: u32) -> Vec<u8> {
        let mut disk = sample_disk();
        disk.resize(
            (PARTITION_START as usize + total as usize + 1) * SECTOR as usize,
            0,
        );
        set_partition_last(&mut disk, u64::from(PARTITION_START) + u64::from(total) - 1);
        disk[DISK_START + 32..DISK_START + 36].copy_from_slice(&total.to_le_bytes());
        disk[DISK_START + 36..DISK_START + 40].copy_from_slice(&fat_sectors.to_le_bytes());
        disk
    }

    #[test]
    fn fat_above_the_limit_is_refused_by_its_size_alone() {
        let limit = (MAX_FAT_BYTES / SECTOR) as u32;
        let total = 2 * limit + 100_000;
        // One sector over the limit: the volume is larger than its FAT area
        // and fits the partition, so only `fat_bytes_len > MAX_FAT_BYTES`
        // refuses it, and the FAT (64 MiB) is not read to find out.
        let over = disk_with_fat_sectors(limit + 1, total);
        assert!(u64::from(total) > u64::from(32 + 2 * (limit + 1)));
        assert!(matches!(
            list(&over),
            Err(ImportError::Unsupported("media_fat_layout"))
        ));
        // Exactly at the limit the same disk passes that check, and the 64 MiB
        // FAT is read (the root directory it finds there is empty).
        let at = disk_with_fat_sectors(limit, total);
        assert!(list(&at).is_ok());
    }

    #[test]
    fn directory_that_points_at_itself_is_a_cycle() {
        let mut disk = sample_disk();
        let root = cluster_offset(&disk, 2);
        let sub_row = find_row(&disk, root, 0x10);
        let sub =
            u32::from(le16(&disk[sub_row..], 26)) | u32::from(le16(&disk[sub_row..], 20)) << 16;
        let sub_dir = cluster_offset(&disk, sub);
        let file_row = find_row(&disk, sub_dir, 0x20);
        point_to(&mut disk, file_row, sub);
        assert!(matches!(
            list(&disk),
            Err(ImportError::Corrupt("media_directory_cycle"))
        ));
    }

    #[test]
    fn directory_that_points_at_its_ancestor_is_a_cycle() {
        let mut disk = sample_disk();
        let root = cluster_offset(&disk, 2);
        let sub_row = find_row(&disk, root, 0x10);
        let sub =
            u32::from(le16(&disk[sub_row..], 26)) | u32::from(le16(&disk[sub_row..], 20)) << 16;
        let sub_dir = cluster_offset(&disk, sub);
        let file_row = find_row(&disk, sub_dir, 0x20);
        point_to(&mut disk, file_row, 2);
        assert!(matches!(
            list(&disk),
            Err(ImportError::Corrupt("media_directory_cycle"))
        ));
    }
}
