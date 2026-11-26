use super::{
    Entry, ReadAt, Volume,
    bpb::{FatType, Geometry, Root},
    lfn::LongName,
    table::Chain,
};
use crate::uefi::status::Status;

fn fixture(count: u32, bits: u32) -> ([u8; 512], u64) {
    let fat_sectors = ((count + 2) * bits).div_ceil(8).div_ceil(512);
    let reserved = if bits == 32 { 32 } else { 1 };
    let root_sectors = if bits == 32 { 0 } else { 32 };
    let sectors = reserved + 2 * fat_sectors + root_sectors + count;
    let mut bpb = [0; 512];
    bpb[..3].copy_from_slice(&[0xeb, 0x58, 0x90]);
    bpb[11..13].copy_from_slice(&512u16.to_le_bytes());
    bpb[13] = 1;
    bpb[14..16].copy_from_slice(&(reserved as u16).to_le_bytes());
    bpb[16] = 2;
    bpb[21] = 0xf8;
    bpb[510..].copy_from_slice(&[0x55, 0xaa]);
    if bits == 32 {
        bpb[32..36].copy_from_slice(&sectors.to_le_bytes());
        bpb[36..40].copy_from_slice(&fat_sectors.to_le_bytes());
        bpb[44..48].copy_from_slice(&2u32.to_le_bytes());
    } else {
        if sectors <= u16::MAX as u32 {
            bpb[19..21].copy_from_slice(&(sectors as u16).to_le_bytes());
        } else {
            bpb[32..36].copy_from_slice(&sectors.to_le_bytes());
        }
        bpb[17..19].copy_from_slice(&512u16.to_le_bytes());
        bpb[22..24].copy_from_slice(&(fat_sectors as u16).to_le_bytes());
    }
    (bpb, u64::from(sectors) * 512)
}

#[test]
fn fat_type_counts_data_clusters_and_rejects_a_short_device() {
    for (count, bits, kind) in [
        (4084, 12, FatType::Fat12),
        (4085, 16, FatType::Fat16),
        (65524, 16, FatType::Fat16),
        (65525, 32, FatType::Fat32),
    ] {
        let (mut boot, capacity) = fixture(count, bits);
        if bits == 32 {
            // FAT mirroring disabled, active FAT index one.
            boot[40] = 0x81;
        }
        let layout = Geometry::parse(&boot, capacity).unwrap();
        assert_eq!((layout.cluster_count, layout.fat_type), (count, kind));
        assert!(Geometry::parse(&boot, capacity - 1).is_none());
        if bits == 32 {
            assert_eq!(layout.root, Root::Cluster(2));
            assert_eq!(layout.fat_base, 32 * 512 + layout.fat_bytes);
            boot[40] = 0x82;
            assert!(Geometry::parse(&boot, capacity).is_none());
        } else {
            assert!(matches!(layout.root, Root::Fixed { entries: 512, .. }));
        }
    }
}

struct Bytes(std::vec::Vec<u8>);
impl ReadAt for Bytes {
    fn read_at(&mut self, offset: u64, out: &mut [u8]) -> Result<(), Status> {
        let at = offset as usize;
        out.copy_from_slice(
            self.0
                .get(at..at + out.len())
                .ok_or(Status::VOLUME_CORRUPTED)?,
        );
        Ok(())
    }
}

fn set_fat12(source: &mut Bytes, base: usize, cluster: u32, value: u16) {
    let index = (cluster + cluster / 2) as usize + base;
    let word = u16::from_le_bytes(source.0[index..index + 2].try_into().unwrap());
    let word = if cluster & 1 == 0 {
        (word & 0xf000) | value
    } else {
        (word & 0xf) | (value << 4)
    };
    source.0[index..index + 2].copy_from_slice(&word.to_le_bytes());
}

#[test]
fn fat12_link_crossing_a_sector_and_a_cycle() {
    let (boot, capacity) = fixture(500, 12);
    let layout = Geometry::parse(&boot, capacity).unwrap();
    let base = layout.fat_base as usize;
    let mut bytes = Bytes(std::vec![0; capacity as usize]);
    // Entry 341 begins at byte 511 of the FAT.
    set_fat12(&mut bytes, base, 341, 342);
    set_fat12(&mut bytes, base, 342, 0xff8);
    let mut chain = Chain::new(layout, 341).unwrap();
    assert!(chain.advance(layout, &mut bytes).unwrap());
    assert_eq!(chain.current(), 342);
    assert!(!chain.advance(layout, &mut bytes).unwrap());
    set_fat12(&mut bytes, base, 342, 341);
    let mut cyclic = Chain::new(layout, 341).unwrap();
    assert!(cyclic.advance(layout, &mut bytes).unwrap());
    assert!(cyclic.advance(layout, &mut bytes).unwrap());
    assert_eq!(
        cyclic.advance(layout, &mut bytes),
        Err(Status::VOLUME_CORRUPTED)
    );
}

#[test]
fn lfn_needs_complete_order_and_a_matching_short_entry() {
    let short = b"MYDOCU~1TXT";
    let checksum = short
        .iter()
        .fold(0u8, |c, &b| c.rotate_right(1).wrapping_add(b));
    let name: std::vec::Vec<_> = "My Document.txt".encode_utf16().collect();
    let mut units = [0xffff; 26];
    units[..name.len()].copy_from_slice(&name);
    units[name.len()] = 0;
    let record = |slot: usize| {
        let mut r = [0; 32];
        r[0] = if slot == 2 { 0x42 } else { 1 };
        r[11] = 0x0f;
        r[13] = checksum;
        for (i, at) in [1, 3, 5, 7, 9, 14, 16, 18, 20, 22, 24, 28, 30]
            .into_iter()
            .enumerate()
        {
            r[at..at + 2].copy_from_slice(&units[(slot - 1) * 13 + i].to_le_bytes());
        }
        r
    };
    let mut lfn = LongName::new();
    lfn.push(&record(1));
    assert_eq!(lfn.take(short), None);
    lfn.push(&record(2));
    lfn.push(&record(1));
    assert_eq!(lfn.take(b"OTHER   TXT"), None);
    lfn.push(&record(2));
    lfn.push(&record(1));
    assert_eq!(lfn.take(short), Some(name.as_slice()));
    assert_eq!(lfn.take(short), None);
}

struct SparseDisk {
    len: u64,
    regions: std::vec::Vec<(u64, std::vec::Vec<u8>)>,
}

impl SparseDisk {
    fn put(&mut self, at: u64, bytes: &[u8]) {
        assert!(at + bytes.len() as u64 <= self.len);
        self.regions.push((at, bytes.to_vec()));
    }

    fn set_link(&mut self, layout: Geometry, cluster: u32, next: u32) {
        let index = u64::from(cluster);
        match layout.fat_type {
            FatType::Fat12 => {
                let offset = layout.fat_base + index + index / 2;
                let mut existing = [0; 2];
                self.read_at(offset, &mut existing).unwrap();
                let word = u16::from_le_bytes(existing);
                let word = if cluster & 1 == 0 {
                    (word & 0xf000) | next as u16
                } else {
                    (word & 0xf) | ((next as u16) << 4)
                };
                self.put(offset, &word.to_le_bytes());
            }
            FatType::Fat16 => self.put(layout.fat_base + index * 2, &(next as u16).to_le_bytes()),
            FatType::Fat32 => self.put(layout.fat_base + index * 4, &next.to_le_bytes()),
        }
    }
}

impl ReadAt for SparseDisk {
    fn read_at(&mut self, offset: u64, out: &mut [u8]) -> Result<(), Status> {
        let end = offset
            .checked_add(out.len() as u64)
            .filter(|&end| end <= self.len)
            .ok_or(Status::VOLUME_CORRUPTED)?;
        out.fill(0);
        for (start, bytes) in &self.regions {
            let left = offset.max(*start);
            let right = end.min(*start + bytes.len() as u64);
            if left < right {
                out[(left - offset) as usize..(right - offset) as usize]
                    .copy_from_slice(&bytes[(left - start) as usize..(right - start) as usize]);
            }
        }
        Ok(())
    }
}

fn sfn(name: &[u8; 11], attributes: u8, cluster: u16, length: u32) -> [u8; 32] {
    let mut record = [0; 32];
    record[..11].copy_from_slice(name);
    record[11] = attributes;
    record[26..28].copy_from_slice(&cluster.to_le_bytes());
    record[28..].copy_from_slice(&length.to_le_bytes());
    record
}

#[test]
fn mixed_case_lfn_and_short_alias_read_fragmented_files_on_all_fat_types() {
    for (count, bits) in [(500, 12), (5000, 16), (70_000, 32)] {
        let (bpb, len) = fixture(count, bits);
        let layout = Geometry::parse(&bpb, len).unwrap();
        let mut disk = SparseDisk {
            len,
            regions: std::vec::Vec::new(),
        };
        disk.put(0, &bpb);
        let end = layout.fat_type.reserved_cluster() + 8;
        for (cluster, next) in [(2, end), (3, end), (4, end), (5, 9), (9, 6), (6, end)] {
            disk.set_link(layout, cluster, next);
        }
        let root = match layout.root {
            Root::Fixed { offset, .. } => offset,
            Root::Cluster(cluster) => layout.cluster_offset(cluster).unwrap(),
        };
        disk.put(root, &sfn(b"EFI        ", 0x10, 3, 0));
        disk.put(
            layout.cluster_offset(3).unwrap(),
            &sfn(b"BOOT       ", 0x10, 4, 0),
        );
        let alias = b"DOCUME~1TXT";
        let checksum = alias
            .iter()
            .fold(0u8, |c, &b| c.rotate_right(1).wrapping_add(b));
        let mut long = [0xff; 32];
        long[0] = 0x41;
        long[11..14].copy_from_slice(&[0x0f, 0, checksum]);
        long[26..28].copy_from_slice(&[0, 0]);
        for (unit, at) in "Δοκιμή.txt"
            .encode_utf16()
            .chain([0])
            .zip([1, 3, 5, 7, 9, 14, 16, 18, 20, 22, 24, 28, 30])
        {
            long[at..at + 2].copy_from_slice(&unit.to_le_bytes());
        }
        let boot_dir = layout.cluster_offset(4).unwrap();
        disk.put(boot_dir, &long);
        disk.put(boot_dir + 32, &sfn(alias, 0x20, 5, 1300));
        for (cluster, byte) in [(5, 0x11), (9, 0x22), (6, 0x33)] {
            disk.put(layout.cluster_offset(cluster).unwrap(), &[byte; 512]);
        }
        let volume = Volume::mount(&mut disk, len).unwrap();
        assert_eq!(volume.free_bytes(&mut disk), Ok(u64::from(count - 6) * 512));
        for path in ["\\EFI\\.\\boot\\δΟΚΙΜΉ.TXT", "\\efi\\BOOT\\docume~1.txt"] {
            let units: std::vec::Vec<_> = path.encode_utf16().collect();
            let file = volume.open_path(&mut disk, &Entry::root(), &units).unwrap();
            assert_eq!(file.len, 1300);
            assert_eq!(volume.allocation_bytes(&mut disk, &file), Ok(1536));
            let mut data = [0x5a; 832];
            assert_eq!(
                volume.read_file(&mut disk, &file, 480, &mut data, &mut None),
                Ok(820)
            );
            assert_eq!(&data[..32], &[0x11; 32]);
            assert_eq!(&data[32..544], &[0x22; 512]);
            assert_eq!(&data[544..820], &[0x33; 276]);
            assert_eq!(&data[820..], &[0x5a; 12]);
        }
        let units: std::vec::Vec<_> = "\\EFI\\BOOT\\absent.txt".encode_utf16().collect();
        assert_eq!(
            volume.open_path(&mut disk, &Entry::root(), &units).err(),
            Some(Status::NOT_FOUND)
        );
        disk.set_link(layout, 6, 5);
        let units: std::vec::Vec<_> = "\\EFI\\BOOT\\DOCUME~1.TXT".encode_utf16().collect();
        let file = volume.open_path(&mut disk, &Entry::root(), &units).unwrap();
        assert_eq!(
            volume.allocation_bytes(&mut disk, &file),
            Err(Status::VOLUME_CORRUPTED)
        );
        disk.set_link(layout, 5, end);
        assert_eq!(
            volume.allocation_bytes(&mut disk, &file),
            Err(Status::VOLUME_CORRUPTED)
        );
        assert_eq!(
            volume.read_file(&mut disk, &file, 512, &mut [0], &mut None),
            Err(Status::VOLUME_CORRUPTED)
        );
    }
}

#[test]
fn last_addressable_clusters_and_free_space_at_the_fat12_and_fat16_limits() {
    for (count, bits) in [(4084, 12), (65524, 16)] {
        let (bpb, len) = fixture(count, bits);
        let layout = Geometry::parse(&bpb, len).unwrap();
        let mut disk = SparseDisk {
            len,
            regions: std::vec::Vec::new(),
        };
        disk.put(0, &bpb);
        let Root::Fixed { offset, .. } = layout.root else {
            unreachable!()
        };
        let last = count + 1;
        disk.set_link(layout, last, layout.fat_type.reserved_cluster() + 8);
        disk.put(offset, &sfn(b"LAST    BIN", 0x20, last as u16, 3));
        disk.put(layout.cluster_offset(last).unwrap(), b"end");
        let volume = Volume::mount(&mut disk, len).unwrap();
        let path: std::vec::Vec<_> = "LAST.BIN".encode_utf16().collect();
        let file = volume.open_path(&mut disk, &Entry::root(), &path).unwrap();
        let mut content = [0; 3];
        assert_eq!(volume.allocation_bytes(&mut disk, &file), Ok(512));
        assert_eq!(
            volume.read_file(&mut disk, &file, 0, &mut content, &mut None),
            Ok(3)
        );
        assert_eq!(&content, b"end");
        assert_eq!(volume.free_bytes(&mut disk), Ok(u64::from(count - 1) * 512));
    }
}

#[test]
fn case_matching_uses_simple_uppercase_without_expanding_names() {
    assert!(super::name::equivalent(&[0x1f80], &[0x1f88]));
    assert!(!super::name::equivalent(
        &[0x00df],
        &[b'S' as u16, b'S' as u16]
    ));
}

/// Counts the reads that reach the device under a FAT volume.
struct Counting<'a> {
    disk: &'a mut SparseDisk,
    reads: usize,
}

impl ReadAt for Counting<'_> {
    fn read_at(&mut self, offset: u64, out: &mut [u8]) -> Result<(), Status> {
        self.reads += 1;
        self.disk.read_at(offset, out)
    }
}

/// S7-32: sequential reads of a fragmented file through a chain cursor return the same bytes as
/// one read, and the device reads grow linearly with the file (not with the square of its cluster
/// count): runs of physically consecutive clusters are one read, FAT entries come from a
/// 512-byte window, and each read continues the chain where the previous one stopped.
#[test]
fn sequential_reads_continue_the_chain_and_read_consecutive_clusters_at_once() {
    let (bpb, len) = fixture(70_000, 32);
    let layout = Geometry::parse(&bpb, len).unwrap();
    let mut disk = SparseDisk {
        len,
        regions: std::vec::Vec::new(),
    };
    disk.put(0, &bpb);
    let end = layout.fat_type.reserved_cluster() + 8;
    disk.set_link(layout, 2, end);
    // 600 clusters in runs of 3 consecutive clusters with a gap after each run:
    // 10, 11, 12, 14, 15, 16, 18, ... (the root directory is cluster 2).
    let clusters: std::vec::Vec<u32> = (0..600u32).map(|i| 10 + i / 3 * 4 + i % 3).collect();
    for pair in clusters.windows(2) {
        disk.set_link(layout, pair[0], pair[1]);
    }
    disk.set_link(layout, *clusters.last().unwrap(), end);
    let size = 600 * 512;
    for (i, &cluster) in clusters.iter().enumerate() {
        let fill: std::vec::Vec<u8> = (0..512).map(|b| (i * 7 + b) as u8).collect();
        disk.put(layout.cluster_offset(cluster).unwrap(), &fill);
    }
    let root = layout.cluster_offset(2).unwrap();
    disk.put(root, &sfn(b"BIG     BIN", 0x20, 10, size));
    let mut counting = Counting {
        disk: &mut disk,
        reads: 0,
    };
    let volume = Volume::mount(&mut counting, len).unwrap();
    let path: std::vec::Vec<_> = "BIG.BIN".encode_utf16().collect();
    let file = volume
        .open_path(&mut counting, &Entry::root(), &path)
        .unwrap();
    let mut whole = std::vec![0u8; size as usize];
    assert_eq!(
        volume.read_file(&mut counting, &file, 0, &mut whole, &mut None),
        Ok(size as usize)
    );
    for (i, chunk) in whole.chunks(512).enumerate() {
        assert_eq!(chunk[0], (i * 7) as u8, "cluster {i}");
        assert_eq!(chunk[511], (i * 7 + 511) as u8, "cluster {i}");
    }
    // 200 runs; the entries of clusters 10..=808 span 3.2 KB of FAT, 7 windows: about 207 reads,
    // far below the 600 links and the ~1 200 reads of a per-link FAT read.
    counting.reads = 0;
    let mut again = std::vec![0u8; size as usize];
    volume
        .read_file(&mut counting, &file, 0, &mut again, &mut None)
        .unwrap();
    assert_eq!(again, whole);
    assert!(counting.reads <= 220, "one read: {}", counting.reads);
    // In 700-byte pieces with the cursor: the same bytes; without the cursor the chain walk from
    // the first cluster would make the FAT reads quadratic.
    counting.reads = 0;
    let mut cursor = None;
    let mut pieces = std::vec::Vec::new();
    let mut offset = 0u64;
    while offset < u64::from(size) {
        let mut piece = [0u8; 700];
        let read = volume
            .read_file(&mut counting, &file, offset, &mut piece, &mut cursor)
            .unwrap();
        pieces.extend_from_slice(&piece[..read]);
        offset += read as u64;
    }
    assert_eq!(pieces, whole);
    let calls = (size as usize).div_ceil(700);
    assert!(
        counting.reads <= 3 * calls,
        "{} reads for {calls} pieces",
        counting.reads
    );
    // A cursor past the target is not used: reading from the start again works.
    let mut head = [0u8; 16];
    volume
        .read_file(&mut counting, &file, 0, &mut head, &mut cursor)
        .unwrap();
    assert_eq!(&head, &whole[..16]);
}

/// Records the offset and length of every read that reaches the device.
struct Recording<'a> {
    disk: &'a mut SparseDisk,
    reads: std::vec::Vec<(u64, usize)>,
}

impl ReadAt for Recording<'_> {
    fn read_at(&mut self, offset: u64, out: &mut [u8]) -> Result<(), Status> {
        self.reads.push((offset, out.len()));
        self.disk.read_at(offset, out)
    }
}

/// ISSUE-697: free space of FAT16 and FAT32 is counted a whole 512-byte window at a time. The
/// result equals an independent decode of every data-cluster entry (FAT32 with the reserved high
/// nibble masked off), the reserved entries 0 and 1 and the zero bytes after the last data
/// cluster are not counted, and the FAT is read once, in aligned windows, the last one ending at
/// the last entry (a full window when the entries end on a window boundary).
#[test]
fn free_space_of_fat16_and_fat32_is_counted_by_whole_windows() {
    // (data clusters, bits): partial last windows (5000, 70 000) and entries that end exactly on
    // a window boundary (4094 * 2 + 4 = 16 * 512, (65 534 + 2) * 4 = 512 * 512).
    for (count, bits) in [
        (4085, 16),
        (4094, 16),
        (5000, 16),
        (65534, 32),
        (70_000, 32),
    ] {
        let (bpb, len) = fixture(count, bits);
        let layout = Geometry::parse(&bpb, len).unwrap();
        let size = (bits / 8) as usize;
        let entries = (count + 2) as usize;
        let mut fat = std::vec![0u8; layout.fat_bytes as usize];
        let mut state = 0x2545_f491u32 ^ count;
        let mut expected = 0u64;
        for cluster in 2..entries {
            state = state.wrapping_mul(1_103_515_245).wrapping_add(12_345);
            let value = match state >> 28 {
                0..=3 => 0,
                4 => 0x1000_0000, // FAT32: free, only the reserved nibble is set.
                5 => 0xf000_0000,
                6 => 1,
                7 => 0x0fff_fff7,
                8 => 0x0fff_ffff,
                // Used entries whose low bits are zero: a mask narrower than the type's (FAT32
                // 0x0000_ffff or 0x00ff_ffff, FAT16 0x0fff or 0x00ff) would count them as free.
                // In a FAT16 table 0x0001_0000..0x0800_0000 are free (their low word is zero).
                9 => 0x0001_0000,
                10 => 0x0080_0000,
                11 => 0x0800_0000,
                12 => 0x1000,
                13 => 0x8000,
                _ => state & 0x0fff_ffff | 2,
            };
            let at = cluster * size;
            fat[at..at + size].copy_from_slice(&value.to_le_bytes()[..size]);
            let decoded = if bits == 32 {
                value & 0x0fff_ffff
            } else {
                value & 0xffff
            };
            if decoded == 0 {
                expected += 1;
            }
        }
        // Entries 0 and 1 stay zero and the FAT sector tail after the last entry is zero: neither
        // may be counted.
        assert!(fat[..2 * size].iter().all(|&b| b == 0));
        assert!(fat[entries * size..].iter().all(|&b| b == 0));
        let mut disk = SparseDisk {
            len,
            regions: std::vec::Vec::new(),
        };
        disk.put(0, &bpb);
        disk.put(layout.fat_base, &fat);
        let volume = Volume::mount(&mut disk, len).unwrap();
        let mut recording = Recording {
            disk: &mut disk,
            reads: std::vec::Vec::new(),
        };
        assert_eq!(
            volume.free_bytes(&mut recording),
            Ok(expected * 512),
            "{bits}-bit FAT with {count} clusters"
        );
        let end = (entries * size) as u64;
        let windows = end.div_ceil(512);
        assert_eq!(recording.reads.len() as u64, windows, "{count}");
        for (index, &(offset, length)) in recording.reads.iter().enumerate() {
            let at = index as u64 * 512;
            assert_eq!(offset, layout.fat_base + at);
            assert_eq!(
                length as u64,
                512.min(end - at),
                "window {index} of {count}"
            );
        }
    }
}

/// Fails the `fail_at`-th read (counted from 1) with `DEVICE_ERROR`.
struct FailingRead<'a> {
    disk: &'a mut SparseDisk,
    reads: usize,
    fail_at: usize,
}

impl ReadAt for FailingRead<'_> {
    fn read_at(&mut self, offset: u64, out: &mut [u8]) -> Result<(), Status> {
        self.reads += 1;
        if self.reads == self.fail_at {
            return Err(Status::DEVICE_ERROR);
        }
        self.disk.read_at(offset, out)
    }
}

/// ISSUE-697: a failed read of any FAT window (the first, a middle and the last one) ends the
/// free-space count with that error instead of counting a stale or empty window.
#[test]
fn free_space_count_returns_the_error_of_a_failed_window_read() {
    for (count, bits) in [(5000, 16), (70_000, 32)] {
        let (bpb, len) = fixture(count, bits);
        let mut disk = SparseDisk {
            len,
            regions: std::vec::Vec::new(),
        };
        disk.put(0, &bpb);
        let volume = Volume::mount(&mut disk, len).unwrap();
        let windows = (u64::from(count + 2) * u64::from(bits / 8)).div_ceil(512) as usize;
        for fail_at in [1, 2, windows / 2, windows] {
            let mut failing = FailingRead {
                disk: &mut disk,
                reads: 0,
                fail_at,
            };
            assert_eq!(
                volume.free_bytes(&mut failing),
                Err(Status::DEVICE_ERROR),
                "{bits}-bit FAT, read {fail_at} of {windows}"
            );
            assert_eq!(failing.reads, fail_at, "no read after the failed one");
        }
    }
}
