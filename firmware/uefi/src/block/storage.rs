//! Publish the NVMe controller (the fixed window or the PCI function, S7-32), its namespace and
//! valid GPT partitions.

use core::{
    ffi::c_void,
    mem::{MaybeUninit, size_of},
    ptr, slice,
};

use crate::{
    bs::{
        event::{EVT_SIGNAL_EXIT_BOOT_SERVICES, Event, TPL_NOTIFY},
        memory::{allocate_pool, free_pool},
    },
    device_path::{DEVICE_PATH_PROTOCOL_GUID, END_PATH_BYTES, memory_mapped_node},
    handoff::Handoff,
    mem::map::MemoryType,
    nvme::driver::{
        BLOCK_BYTES, Controller, Location, NAMESPACE_ID, REGISTERS_BASE, REGISTERS_BYTES,
    },
    uefi::{
        guid::Guid,
        status::Status,
        system_table::{BootServices, SystemTable},
        types::Handle,
    },
};

use super::{
    block_io::{BLOCK_IO_PROTOCOL_GUID, Endpoint},
    disk_io::DISK_IO_PROTOCOL_GUID,
    partition_gpt::{Header, SYSTEM_PARTITION, Table, protective_mbr},
};

pub use super::partition_gpt::Partition;

pub const PARTITION_INFO_PROTOCOL_GUID: Guid = Guid {
    data1: 0x8cf2_f62c,
    data2: 0xbc9b,
    data3: 0x4821,
    data4: [0x80, 0x8d, 0xec, 0x9e, 0xc4, 0x21, 0xa1, 0xa0],
};

#[repr(C)]
pub struct PartitionInfo {
    pub revision: u32,
    pub partition_type: u32,
    pub system: u8,
    reserved: [u8; 7],
    pub partition: Partition,
}

/// The longest controller prefix: the memory-mapped node of the fixed window (24 bytes). The PCI
/// prefix `PciRoot(0)/Pci(device,0)` is 12 + 6 bytes; a shorter path leaves zeros after its end
/// node.
const PREFIX_MAX: usize = 24;
const CONTROLLER_PATH: usize = PREFIX_MAX + 4;
const NAMESPACE_PATH: usize = PREFIX_MAX + 16 + 4;
const PARTITION_PATH: usize = PREFIX_MAX + 16 + 42 + 4;
/// ACPI device path node (UEFI 2.11 §10.3.3): HID = EISA ID PNP0A03 (PCI host bridge, the
/// compressed `0x0A0341D0`), UID 0.
const PCI_ROOT_NODE: [u8; 12] = [2, 1, 12, 0, 0xd0, 0x41, 0x03, 0x0a, 0, 0, 0, 0];

struct Pool(*mut c_void);

impl Pool {
    fn allocate(bytes: usize) -> Result<Self, Status> {
        let mut pointer = ptr::null_mut();
        check(unsafe { allocate_pool(MemoryType::BOOT_SERVICES_DATA, bytes, &mut pointer) })?;
        Ok(Self(pointer))
    }
}

impl Drop for Pool {
    fn drop(&mut self) {
        unsafe { free_pool(self.0) };
    }
}

struct NamespaceStorage {
    controller: Controller,
    endpoint: MaybeUninit<Endpoint>,
    controller_path: [u8; CONTROLLER_PATH],
    namespace_path: [u8; NAMESPACE_PATH],
    /// Bytes of `namespace_path` before its end node.
    namespace_nodes: usize,
    controller_handle: Handle,
    namespace_handle: Handle,
}

struct PartitionStorage {
    endpoint: MaybeUninit<Endpoint>,
    info: PartitionInfo,
    path: [u8; PARTITION_PATH],
    handle: Handle,
}

struct Partitions {
    pool: Pool,
    count: usize,
}

impl Partitions {
    fn records(&self) -> impl DoubleEndedIterator<Item = *mut PartitionStorage> + '_ {
        (0..self.count).map(|index| unsafe { self.pool.0.cast::<PartitionStorage>().add(index) })
    }
}

fn check(status: Status) -> Result<(), Status> {
    if status == Status::SUCCESS {
        Ok(())
    } else {
        Err(status)
    }
}

/// The device path nodes of the controller (UEFI 2.11 §10.3): the memory-mapped node of the fixed
/// window, or `PciRoot(0)/Pci(device,0)` (§10.3.3 ACPI node, §10.3.2.1 PCI node: function, then
/// device) for the PCI function. Returns the nodes and their length.
fn controller_prefix(location: Location) -> ([u8; PREFIX_MAX], usize) {
    let mut prefix = [0; PREFIX_MAX];
    match location {
        Location::Fixed => {
            prefix.copy_from_slice(&memory_mapped_node(
                REGISTERS_BASE,
                REGISTERS_BASE + REGISTERS_BYTES - 1,
            ));
            (prefix, PREFIX_MAX)
        }
        Location::Pci { device } => {
            prefix[..12].copy_from_slice(&PCI_ROOT_NODE);
            prefix[12..18].copy_from_slice(&[1, 1, 6, 0, 0, device]);
            (prefix, 18)
        }
    }
}

/// The controller path, the namespace path (with the NVMe namespace node, §10.3.4.21) and the
/// length of the namespace path's nodes before its end node.
fn device_paths(
    location: Location,
    eui: u64,
) -> ([u8; CONTROLLER_PATH], [u8; NAMESPACE_PATH], usize) {
    let (prefix, length) = controller_prefix(location);
    let mut controller = [0; CONTROLLER_PATH];
    controller[..length].copy_from_slice(&prefix[..length]);
    controller[length..length + 4].copy_from_slice(&END_PATH_BYTES);
    let mut namespace = [0; NAMESPACE_PATH];
    namespace[..length].copy_from_slice(&prefix[..length]);
    namespace[length..length + 4].copy_from_slice(&[3, 23, 16, 0]);
    namespace[length + 4..length + 8].copy_from_slice(&NAMESPACE_ID.to_le_bytes());
    namespace[length + 8..length + 16].copy_from_slice(&eui.to_le_bytes());
    let nodes = length + 16;
    namespace[nodes..nodes + 4].copy_from_slice(&END_PATH_BYTES);
    (controller, namespace, nodes)
}

fn partition_path(
    parent: &[u8; NAMESPACE_PATH],
    parent_nodes: usize,
    slot: u32,
    entry: Partition,
) -> [u8; PARTITION_PATH] {
    let mut path = [0; PARTITION_PATH];
    let at = parent_nodes;
    path[..at].copy_from_slice(&parent[..at]);
    path[at..at + 4].copy_from_slice(&[4, 1, 42, 0]);
    path[at + 4..at + 8].copy_from_slice(&slot.to_le_bytes());
    path[at + 8..at + 16].copy_from_slice(&entry.start.to_le_bytes());
    path[at + 16..at + 24].copy_from_slice(&(entry.end - entry.start + 1).to_le_bytes());
    let guid = entry.unique_id;
    path[at + 24..at + 28].copy_from_slice(&guid.data1.to_le_bytes());
    path[at + 28..at + 30].copy_from_slice(&guid.data2.to_le_bytes());
    path[at + 30..at + 32].copy_from_slice(&guid.data3.to_le_bytes());
    path[at + 32..at + 40].copy_from_slice(&guid.data4);
    path[at + 40..at + 42].copy_from_slice(&[2, 2]); // GPT and GUID signature.
    path[at + 42..at + 46].copy_from_slice(&END_PATH_BYTES);
    path
}

fn collect_partitions(
    storage: &NamespaceStorage,
    table: &Table<'_>,
) -> Result<Option<Partitions>, Status> {
    let visible = || {
        table
            .entries()
            .filter(|(_, partition)| partition.exposes_block_io())
    };
    let count = visible().count();
    if count == 0 {
        return Ok(None);
    }
    let bytes = count
        .checked_mul(size_of::<PartitionStorage>())
        .ok_or(Status::OUT_OF_RESOURCES)?;
    let parts = Partitions {
        pool: Pool::allocate(bytes)?,
        count,
    };
    let items = parts.pool.0.cast::<PartitionStorage>();
    for (index, (slot, partition)) in visible().enumerate() {
        let item = unsafe { items.add(index) };
        unsafe {
            item.write(PartitionStorage {
                endpoint: MaybeUninit::uninit(),
                info: PartitionInfo {
                    revision: 0x0001_0000,
                    partition_type: 2,
                    system: u8::from(partition.partition_type == SYSTEM_PARTITION),
                    reserved: [0; 7],
                    partition,
                },
                path: partition_path(
                    &storage.namespace_path,
                    storage.namespace_nodes,
                    slot,
                    partition,
                ),
                handle: ptr::null_mut(),
            });
            Endpoint::init(
                ptr::addr_of_mut!((*item).endpoint).cast(),
                &storage.controller,
                partition.start,
                partition.end - partition.start + 1,
                true,
            );
        }
    }
    Ok(Some(parts))
}

unsafe fn discover_partitions(
    storage: *mut NamespaceStorage,
) -> Result<Option<Partitions>, Status> {
    let media = unsafe { (*storage).endpoint.assume_init_ref() };
    let last = media.media.last_block;
    let mut block = [0; BLOCK_BYTES];
    match unsafe { media.access_bytes(false, 0, block.len(), block.as_mut_ptr()) } {
        Ok(()) => (),
        Err(Status::NO_MEDIA) => {
            // The endpoint has yet to be published, so there are no external references.
            unsafe {
                (*ptr::addr_of_mut!((*storage).endpoint).cast::<Endpoint>())
                    .media
                    .media_present = 0
            };
            return Ok(None);
        }
        Err(error) => return Err(error),
    }
    if !protective_mbr(&block, last) || last < 2 {
        return Ok(None);
    }
    select_gpt(last, |lba| {
        unsafe {
            media.access_bytes(
                false,
                lba * BLOCK_BYTES as u64,
                block.len(),
                block.as_mut_ptr(),
            )
        }?;
        let Some(header) = Header::parse(&block, lba, last) else {
            return Ok(None);
        };
        let table_pool = Pool::allocate(header.table_bytes)?;
        unsafe {
            media.access_bytes(
                false,
                header.table_lba * BLOCK_BYTES as u64,
                header.table_bytes,
                table_pool.0.cast(),
            )
        }?;
        let array = unsafe { slice::from_raw_parts(table_pool.0.cast(), header.table_bytes) };
        if let Some(table) = header.table(array) {
            let partitions = collect_partitions(unsafe { &*storage }, &table)?;
            #[cfg(target_os = "none")]
            crate::tpm::measure::gpt(&block, array, header.entry_bytes())?;
            return Ok(Some(partitions));
        }
        Ok(None)
    })
    .map(Option::flatten)
}

fn select_gpt<T>(
    last: u64,
    mut decode: impl FnMut(u64) -> Result<Option<T>, Status>,
) -> Result<Option<T>, Status> {
    // The backup location comes from Identify Namespace, independent of the primary header.
    for lba in [1, last] {
        match decode(lba) {
            Ok(Some(table)) => return Ok(Some(table)),
            Ok(None) | Err(Status::DEVICE_ERROR) => (),
            Err(error) => return Err(error),
        }
    }
    Ok(None)
}

struct EndpointBindings {
    path: *const u8,
    endpoint: *mut Endpoint,
    info: *const PartitionInfo,
}

impl EndpointBindings {
    unsafe fn type_marker(&self) -> *const Guid {
        let kind = unsafe { &(*self.info).partition.partition_type };
        if [
            DEVICE_PATH_PROTOCOL_GUID,
            BLOCK_IO_PROTOCOL_GUID,
            DISK_IO_PROTOCOL_GUID,
            PARTITION_INFO_PROTOCOL_GUID,
        ]
        .contains(kind)
        {
            // That GUID is already present as a live interface on this handle.
            ptr::null()
        } else {
            kind
        }
    }

    unsafe fn install(&self, boot: &BootServices, handle: *mut Handle) -> Result<(), Status> {
        let install = boot
            .install_multiple_protocol_interfaces
            .ok_or(Status::UNSUPPORTED)?;
        let path = self.path.cast::<c_void>();
        let block = unsafe { ptr::addr_of!((*self.endpoint).block).cast::<c_void>() };
        let disk = unsafe { ptr::addr_of!((*self.endpoint).disk).cast::<c_void>() };
        let end = ptr::null::<Guid>();
        check(unsafe {
            if self.info.is_null() {
                install(
                    handle,
                    &DEVICE_PATH_PROTOCOL_GUID as *const Guid,
                    path,
                    &BLOCK_IO_PROTOCOL_GUID as *const Guid,
                    block,
                    &DISK_IO_PROTOCOL_GUID as *const Guid,
                    disk,
                    end,
                )
            } else {
                install(
                    handle,
                    &DEVICE_PATH_PROTOCOL_GUID as *const Guid,
                    path,
                    &BLOCK_IO_PROTOCOL_GUID as *const Guid,
                    block,
                    &DISK_IO_PROTOCOL_GUID as *const Guid,
                    disk,
                    &PARTITION_INFO_PROTOCOL_GUID as *const Guid,
                    self.info.cast::<c_void>(),
                    self.type_marker(),
                    ptr::null::<c_void>(),
                    end,
                )
            }
        })
    }

    unsafe fn uninstall(&self, boot: &BootServices, handle: Handle) -> bool {
        if handle.is_null() {
            return true;
        }
        let Some(uninstall) = boot.uninstall_multiple_protocol_interfaces else {
            return false;
        };
        let path = self.path.cast::<c_void>();
        let block = unsafe { ptr::addr_of!((*self.endpoint).block).cast::<c_void>() };
        let disk = unsafe { ptr::addr_of!((*self.endpoint).disk).cast::<c_void>() };
        let end = ptr::null::<Guid>();
        let status = unsafe {
            if self.info.is_null() {
                uninstall(
                    handle,
                    &DEVICE_PATH_PROTOCOL_GUID as *const Guid,
                    path,
                    &BLOCK_IO_PROTOCOL_GUID as *const Guid,
                    block,
                    &DISK_IO_PROTOCOL_GUID as *const Guid,
                    disk,
                    end,
                )
            } else {
                uninstall(
                    handle,
                    &DEVICE_PATH_PROTOCOL_GUID as *const Guid,
                    path,
                    &BLOCK_IO_PROTOCOL_GUID as *const Guid,
                    block,
                    &DISK_IO_PROTOCOL_GUID as *const Guid,
                    disk,
                    &PARTITION_INFO_PROTOCOL_GUID as *const Guid,
                    self.info.cast::<c_void>(),
                    self.type_marker(),
                    ptr::null::<c_void>(),
                    end,
                )
            }
        };
        status == Status::SUCCESS
    }
}

unsafe fn partition_bindings(partition: *mut PartitionStorage) -> EndpointBindings {
    unsafe {
        EndpointBindings {
            path: ptr::addr_of!((*partition).path).cast(),
            endpoint: ptr::addr_of_mut!((*partition).endpoint).cast(),
            info: ptr::addr_of!((*partition).info),
        }
    }
}

unsafe extern "efiapi" fn exit_boot_services(_: Event, controller: *mut c_void) {
    // The controller has a single BSP owner and submits only synchronous commands.
    unsafe { &*controller.cast::<Controller>() }.quiesce();
}

/// # Safety
/// Called once during BSP startup with live Boot Services and the board's identity map.
pub unsafe fn install(handoff: &Handoff<'_>, system: *mut SystemTable) -> Status {
    let result = (|| {
        let Some(controller) = (unsafe { Controller::discover(handoff) })? else {
            return Ok(());
        };
        let pool = match Pool::allocate(size_of::<NamespaceStorage>()) {
            Ok(pool) => pool,
            Err(error) => {
                unsafe { controller.release() };
                return Err(error);
            }
        };
        let storage = pool.0.cast::<NamespaceStorage>();
        let namespace = controller.namespace();
        let (controller_path, namespace_path, namespace_nodes) =
            device_paths(controller.location(), namespace.eui);
        unsafe {
            storage.write(NamespaceStorage {
                controller,
                endpoint: MaybeUninit::uninit(),
                controller_path,
                namespace_path,
                namespace_nodes,
                controller_handle: ptr::null_mut(),
                namespace_handle: ptr::null_mut(),
            });
        }
        let controller = unsafe { ptr::addr_of!((*storage).controller) };
        let endpoint = unsafe { ptr::addr_of_mut!((*storage).endpoint).cast::<Endpoint>() };
        unsafe { Endpoint::init(endpoint, controller, 0, namespace.sectors, false) };
        let partitions = match unsafe { discover_partitions(storage) } {
            Ok(parts) => parts,
            Err(error) => {
                unsafe { controller.read().release() };
                return Err(error);
            }
        };
        let boot = unsafe { &*(*system).boot_services };
        let mut event = ptr::null_mut();
        let namespace = EndpointBindings {
            path: unsafe { ptr::addr_of!((*storage).namespace_path).cast() },
            endpoint,
            info: ptr::null(),
        };
        let path = unsafe { ptr::addr_of!((*storage).controller_path).cast::<c_void>() };
        let end = ptr::null::<Guid>();
        let publish = (|| {
            let multiple = boot
                .install_multiple_protocol_interfaces
                .ok_or(Status::UNSUPPORTED)?;
            check(unsafe {
                (boot.create_event)(
                    EVT_SIGNAL_EXIT_BOOT_SERVICES,
                    TPL_NOTIFY,
                    Some(exit_boot_services),
                    controller.cast_mut().cast(),
                    &mut event,
                )
            })?;
            check(unsafe {
                multiple(
                    ptr::addr_of_mut!((*storage).controller_handle),
                    &DEVICE_PATH_PROTOCOL_GUID as *const Guid,
                    path,
                    end,
                )
            })?;
            unsafe { namespace.install(boot, ptr::addr_of_mut!((*storage).namespace_handle)) }?;
            if let Some(parts) = &partitions {
                for item in parts.records() {
                    unsafe {
                        partition_bindings(item).install(boot, ptr::addr_of_mut!((*item).handle))
                    }?;
                }
            }
            Ok(())
        })();
        if publish.is_err() {
            let mut unbound =
                event.is_null() || unsafe { (boot.close_event)(event) } == Status::SUCCESS;
            if let Some(parts) = &partitions {
                for item in parts.records().rev() {
                    unbound &= unsafe { partition_bindings(item).uninstall(boot, (*item).handle) };
                }
            }
            unbound &= unsafe { namespace.uninstall(boot, (*storage).namespace_handle) };
            let handle = unsafe { (*storage).controller_handle };
            if !handle.is_null() {
                unbound &=
                    boot.uninstall_multiple_protocol_interfaces
                        .is_some_and(|uninstall| unsafe {
                            uninstall(handle, &DEVICE_PATH_PROTOCOL_GUID as *const Guid, path, end)
                                == Status::SUCCESS
                        });
            }
            if unbound {
                unsafe { controller.read().release() };
                return publish;
            }
            // A callback may still hold a published interface. The driver is stopped and its
            // allocations retain their BootServicesData lifetime when rollback cannot withdraw it.
            unsafe { &*controller }.quiesce();
        }
        core::mem::forget(pool);
        core::mem::forget(partitions);
        publish
    })();
    result.err().unwrap_or(Status::SUCCESS)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// S7-32: the PCI controller's paths, byte by byte from UEFI 2.11 §10.3: ACPI node (type 2,
    /// subtype 1, length 12, HID 0x0A0341D0 = PNP0A03, UID 0), PCI node (type 1, subtype 1, length
    /// 6, function 0, device 4), NVMe namespace node (type 3, subtype 23, length 16, NSID 1, EUI-64),
    /// end node (type 0x7F, subtype 0xFF, length 4).
    #[test]
    fn the_pci_controller_paths_are_pci_root_pci_and_the_namespace_node() {
        let eui = 0x0102_0304_0506_0708u64;
        let (controller, namespace, nodes) = device_paths(Location::Pci { device: 4 }, eui);
        let root_and_pci = [
            0x02, 0x01, 0x0c, 0x00, 0xd0, 0x41, 0x03, 0x0a, 0x00, 0x00, 0x00,
            0x00, // PciRoot(0)
            0x01, 0x01, 0x06, 0x00, 0x00, 0x04, // Pci(4,0)
        ];
        let end = [0x7f, 0xff, 0x04, 0x00];
        assert_eq!(&controller[..18], &root_and_pci);
        assert_eq!(&controller[18..22], &end);
        assert!(controller[22..].iter().all(|byte| *byte == 0));
        assert_eq!(nodes, 34);
        assert_eq!(&namespace[..18], &root_and_pci);
        assert_eq!(
            &namespace[18..34],
            &[0x03, 0x17, 0x10, 0x00, 1, 0, 0, 0, 8, 7, 6, 5, 4, 3, 2, 1]
        );
        assert_eq!(&namespace[34..38], &end);
    }

    /// A partition with a known GUID for the path tests.
    fn partition() -> Partition {
        let zero = Guid {
            data1: 0,
            data2: 0,
            data3: 0,
            data4: [0; 8],
        };
        Partition {
            partition_type: zero,
            unique_id: Guid {
                data1: 0x0011_2233,
                data2: 0x4455,
                data3: 0x6677,
                data4: [0x88, 0x99, 0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff],
            },
            start: 2048,
            end: 4095,
            attributes: 0,
            name: [0; 36],
        }
    }

    /// The Hard Drive node (UEFI 2.11 §10.3.5.1): type 4, subtype 1, length 42, partition number,
    /// start LBA, size in LBAs, the GUID in its mixed-endian byte order, format 2 (GPT), signature
    /// type 2 (GUID).
    fn hard_drive_node(slot: u32) -> Vec<u8> {
        let mut node = vec![4, 1, 42, 0];
        node.extend_from_slice(&slot.to_le_bytes());
        node.extend_from_slice(&2048u64.to_le_bytes());
        node.extend_from_slice(&2048u64.to_le_bytes());
        node.extend_from_slice(&[
            0x33, 0x22, 0x11, 0x00, 0x55, 0x44, 0x77, 0x66, 0x88, 0x99, 0xaa, 0xbb, 0xcc, 0xdd,
            0xee, 0xff,
        ]);
        node.extend_from_slice(&[2, 2]);
        node
    }

    /// The fixed window keeps its memory-mapped node (type 1, subtype 3, length 24, memory type
    /// 11, start 0xFC001000, end 0xFC002FFF) and the old layout of the partition path.
    #[test]
    fn the_fixed_controller_paths_keep_the_memory_mapped_node() {
        let (controller, namespace, nodes) = device_paths(Location::Fixed, 0);
        let mut mmio = vec![0x01, 0x03, 0x18, 0x00, 11, 0, 0, 0];
        mmio.extend_from_slice(&0xfc00_1000u64.to_le_bytes());
        mmio.extend_from_slice(&0xfc00_2fffu64.to_le_bytes());
        assert_eq!(&controller[..24], &mmio[..]);
        assert_eq!(&controller[24..], &[0x7f, 0xff, 0x04, 0x00]);
        assert_eq!(nodes, 40);
        assert_eq!(&namespace[..24], &mmio[..]);
        assert_eq!(&namespace[40..], &[0x7f, 0xff, 0x04, 0x00]);
        let path = partition_path(&namespace, nodes, 3, partition());
        assert_eq!(&path[..40], &namespace[..40]);
        assert_eq!(&path[40..82], &hard_drive_node(3)[..]);
        assert_eq!(&path[82..], &[0x7f, 0xff, 0x04, 0x00]);
    }

    /// Behind the PCI prefix (18 bytes) and the namespace node the Hard Drive node starts at 34
    /// and the end node at 76; the rest of the fixed-size buffer stays zero.
    #[test]
    fn the_pci_partition_path_follows_the_shorter_prefix() {
        let (_, namespace, nodes) = device_paths(Location::Pci { device: 4 }, 7);
        let path = partition_path(&namespace, nodes, 1, partition());
        assert_eq!(&path[..34], &namespace[..34]);
        assert_eq!(&path[34..76], &hard_drive_node(1)[..]);
        assert_eq!(&path[76..80], &[0x7f, 0xff, 0x04, 0x00]);
        assert!(path[80..].iter().all(|byte| *byte == 0));
    }

    #[test]
    fn failed_primary_sector_reads_try_the_backup_but_transport_timeouts_are_reported() {
        assert_eq!(
            select_gpt(255, |lba| match lba {
                1 => Err(Status::DEVICE_ERROR),
                255 => Ok(Some(255)),
                _ => unreachable!(),
            }),
            Ok(Some(255))
        );
        assert_eq!(
            select_gpt(255, |lba| {
                assert_eq!(lba, 1);
                Err::<Option<()>, _>(Status::TIMEOUT)
            }),
            Err(Status::TIMEOUT)
        );
        assert_eq!(
            select_gpt(255, |lba| {
                assert_eq!(lba, 1);
                Ok(Some(None::<()>))
            }),
            Ok(Some(None))
        );
    }
}
