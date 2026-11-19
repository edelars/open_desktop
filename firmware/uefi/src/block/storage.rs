//! Publish the fixed NVMe controller, its namespace and valid GPT partitions.

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
    nvme::driver::{BLOCK_BYTES, Controller, NAMESPACE_ID, REGISTERS_BASE, REGISTERS_BYTES},
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

const CONTROLLER_PATH: usize = 24 + 4;
const NAMESPACE_PATH: usize = 24 + 16 + 4;
const PARTITION_PATH: usize = 24 + 16 + 42 + 4;

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

fn device_paths(eui: u64) -> ([u8; CONTROLLER_PATH], [u8; NAMESPACE_PATH]) {
    let mmio = memory_mapped_node(REGISTERS_BASE, REGISTERS_BASE + REGISTERS_BYTES - 1);
    let mut controller = [0; CONTROLLER_PATH];
    controller[..24].copy_from_slice(&mmio);
    controller[24..].copy_from_slice(&END_PATH_BYTES);
    let mut namespace = [0; NAMESPACE_PATH];
    namespace[..24].copy_from_slice(&mmio);
    namespace[24..28].copy_from_slice(&[3, 23, 16, 0]);
    namespace[28..32].copy_from_slice(&NAMESPACE_ID.to_le_bytes());
    namespace[32..40].copy_from_slice(&eui.to_le_bytes());
    namespace[40..].copy_from_slice(&END_PATH_BYTES);
    (controller, namespace)
}

fn partition_path(
    parent: &[u8; NAMESPACE_PATH],
    slot: u32,
    entry: Partition,
) -> [u8; PARTITION_PATH] {
    let mut path = [0; PARTITION_PATH];
    path[..40].copy_from_slice(&parent[..40]);
    path[40..44].copy_from_slice(&[4, 1, 42, 0]);
    path[44..48].copy_from_slice(&slot.to_le_bytes());
    path[48..56].copy_from_slice(&entry.start.to_le_bytes());
    path[56..64].copy_from_slice(&(entry.end - entry.start + 1).to_le_bytes());
    let guid = entry.unique_id;
    path[64..68].copy_from_slice(&guid.data1.to_le_bytes());
    path[68..70].copy_from_slice(&guid.data2.to_le_bytes());
    path[70..72].copy_from_slice(&guid.data3.to_le_bytes());
    path[72..80].copy_from_slice(&guid.data4);
    path[80..82].copy_from_slice(&[2, 2]); // GPT and GUID signature.
    path[82..].copy_from_slice(&END_PATH_BYTES);
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
                path: partition_path(&storage.namespace_path, slot, partition),
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
        let (controller_path, namespace_path) = device_paths(namespace.eui);
        unsafe {
            storage.write(NamespaceStorage {
                controller,
                endpoint: MaybeUninit::uninit(),
                controller_path,
                namespace_path,
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
