//! Tests of the agent channel ABI. The guest side is driven without a CPU: a bus with the COM2
//! device bound at `0x2F8` takes port accesses (the unit-level harness of S11-21 until the session
//! of S10-09 exists).

use super::*;
use od_machine::{
    AGENT_CHANNEL_RX_QUEUE_BYTES, AccessWidth, BusDevice, GuestPhysicalAddress, GuestPhysicalRange,
    MachineBus, MachineMemoryMap, MemoryRegionKind, MemoryRegionSpec, PortAddress, PortRange,
    PortRegionBinding, RamLimits, UartPortSpec,
};

const HEADER: &str = include_str!("../../../../include/open_desktop/agent_abi_v1.h");

fn create() -> *mut OdAgentChannel {
    let mut handle = std::ptr::null_mut();
    assert_eq!(
        unsafe { od_agent_channel_test_create_v1(&mut handle) },
        STATUS_OK
    );
    assert!(!handle.is_null());
    handle
}

fn read(handle: *mut OdAgentChannel, capacity: usize) -> (u32, Vec<u8>) {
    let mut buf = vec![0u8; capacity];
    let mut count = usize::MAX;
    let status =
        unsafe { od_agent_channel_read_v1(handle, buf.as_mut_ptr(), buf.len(), &mut count) };
    if status == STATUS_OK {
        buf.truncate(count);
    }
    (status, buf)
}

fn write(handle: *mut OdAgentChannel, data: &[u8]) -> (u32, usize) {
    let mut count = usize::MAX;
    let status =
        unsafe { od_agent_channel_write_v1(handle, data.as_ptr(), data.len(), &mut count) };
    (status, count)
}

fn inject(handle: *mut OdAgentChannel, data: &[u8]) -> (u32, usize) {
    let mut count = usize::MAX;
    let status =
        unsafe { od_agent_channel_test_inject_v1(handle, data.as_ptr(), data.len(), &mut count) };
    (status, count)
}

fn stats(handle: *mut OdAgentChannel) -> OdAgentChannelStats {
    let mut out = OdAgentChannelStats {
        abi_version: ABI_VERSION,
        struct_size: STATS_SIZE as u32,
        ..OdAgentChannelStats::default()
    };
    assert_eq!(
        unsafe { od_agent_channel_stats_v1(handle, &mut out) },
        STATUS_OK
    );
    out
}

#[test]
fn the_header_and_the_rust_mirror_agree() {
    assert!(HEADER.contains("#define OD_AGENT_ABI_VERSION_V1 UINT32_C(0x00010000)"));
    assert_eq!(ABI_VERSION, 0x0001_0000);
    assert!(HEADER.contains(&format!(
        "#define OD_AGENT_CHANNEL_STATS_SIZE_V1 {STATS_SIZE}"
    )));
    for (name, value) in [
        ("OK", STATUS_OK),
        ("INVALID_ARGUMENT", STATUS_INVALID_ARGUMENT),
        ("ABI_MISMATCH", STATUS_ABI_MISMATCH),
        ("INVALID_HANDLE", STATUS_INVALID_HANDLE),
        ("INTERNAL_ERROR", STATUS_INTERNAL_ERROR),
    ] {
        assert!(
            HEADER.contains(&format!("OD_AGENT_STATUS_{name} = {value}")),
            "status {name}"
        );
    }
    for symbol in [
        "od_agent_channel_read_v1",
        "od_agent_channel_write_v1",
        "od_agent_channel_stats_v1",
        "od_agent_channel_release_v1",
        "od_agent_channel_test_create_v1",
        "od_agent_channel_test_inject_v1",
    ] {
        assert!(HEADER.contains(symbol), "{symbol}");
    }
    // Every field offset of the Rust mirror, and the same pair written in the header comment.
    macro_rules! offsets {
        ($($field:ident = $offset:expr),* $(,)?) => {$(
            assert_eq!(
                std::mem::offset_of!(OdAgentChannelStats, $field),
                $offset,
                stringify!($field)
            );
            assert!(
                HEADER.contains(&format!("{} {}", stringify!($field), $offset)),
                "header offset of {}",
                stringify!($field)
            );
        )*};
    }
    offsets!(
        abi_version = 0,
        struct_size = 4,
        rx_capacity = 8,
        tx_capacity = 16,
        rx_queued = 24,
        tx_queued = 32,
        rx_bytes_accepted = 40,
        rx_bytes_delivered = 48,
        rx_overflow_bytes = 56,
        tx_bytes_accepted = 64,
        tx_bytes_read = 72,
        tx_overflow_bytes = 80,
    );
}

#[test]
fn a_handle_is_released_once_and_a_stale_or_null_handle_is_rejected() {
    let handle = create();
    assert_eq!(read(handle, 4).0, STATUS_OK);
    assert_eq!(unsafe { od_agent_channel_release_v1(handle) }, STATUS_OK);
    assert_eq!(
        unsafe { od_agent_channel_release_v1(handle) },
        STATUS_INVALID_HANDLE
    );
    assert_eq!(read(handle, 4).0, STATUS_INVALID_HANDLE);
    assert_eq!(write(handle, b"x").0, STATUS_INVALID_HANDLE);
    assert_eq!(
        unsafe { od_agent_channel_release_v1(std::ptr::null_mut()) },
        STATUS_INVALID_ARGUMENT
    );
    assert_eq!(read(std::ptr::null_mut(), 4).0, STATUS_INVALID_ARGUMENT);
    // A forged identifier is not a pointer and is simply unknown.
    let forged = std::ptr::without_provenance_mut::<OdAgentChannel>(usize::MAX / 2);
    assert_eq!(read(forged, 4).0, STATUS_INVALID_HANDLE);
    // A second creation never reuses the identifier.
    let again = create();
    assert_ne!(again, handle);
    assert_eq!(unsafe { od_agent_channel_release_v1(again) }, STATUS_OK);
}

#[test]
fn create_refuses_a_bad_slot() {
    assert_eq!(
        unsafe { od_agent_channel_test_create_v1(std::ptr::null_mut()) },
        STATUS_INVALID_ARGUMENT
    );
    let handle = create();
    let mut occupied = handle;
    assert_eq!(
        unsafe { od_agent_channel_test_create_v1(&mut occupied) },
        STATUS_INVALID_ARGUMENT
    );
    assert_eq!(occupied, handle);
    assert_eq!(unsafe { od_agent_channel_release_v1(handle) }, STATUS_OK);
}

#[test]
fn injected_bytes_are_read_in_order_and_a_short_buffer_leaves_the_rest() {
    let handle = create();
    assert_eq!(read(handle, 8), (STATUS_OK, Vec::new()));
    assert_eq!(inject(handle, b"hello"), (STATUS_OK, 5));
    assert_eq!(read(handle, 2), (STATUS_OK, b"he".to_vec()));
    assert_eq!(read(handle, 0), (STATUS_OK, Vec::new()));
    assert_eq!(read(handle, 8), (STATUS_OK, b"llo".to_vec()));
    let s = stats(handle);
    assert_eq!(
        (s.tx_bytes_accepted, s.tx_bytes_read, s.tx_queued),
        (5, 5, 0)
    );
    assert_eq!(unsafe { od_agent_channel_release_v1(handle) }, STATUS_OK);
}

#[test]
fn zero_length_calls_accept_a_null_buffer_and_a_nonzero_one_does_not() {
    let handle = create();
    let mut count = 7usize;
    assert_eq!(
        unsafe { od_agent_channel_write_v1(handle, std::ptr::null(), 0, &mut count) },
        STATUS_OK
    );
    assert_eq!(count, 0);
    assert_eq!(
        unsafe { od_agent_channel_write_v1(handle, std::ptr::null(), 1, &mut count) },
        STATUS_INVALID_ARGUMENT
    );
    assert_eq!(
        unsafe { od_agent_channel_read_v1(handle, std::ptr::null_mut(), 1, &mut count) },
        STATUS_INVALID_ARGUMENT
    );
    assert_eq!(
        unsafe { od_agent_channel_write_v1(handle, b"a".as_ptr(), 1, std::ptr::null_mut()) },
        STATUS_INVALID_ARGUMENT
    );
    // A buffer that contains the out pointer is refused.
    let mut buf = [0u8; 16];
    let out = buf.as_mut_ptr().cast::<usize>();
    assert_eq!(
        unsafe { od_agent_channel_read_v1(handle, buf.as_mut_ptr(), 16, out) },
        STATUS_INVALID_ARGUMENT
    );
    assert_eq!(unsafe { od_agent_channel_release_v1(handle) }, STATUS_OK);
}

#[test]
fn the_host_write_reports_the_accepted_count_and_counts_the_overflow() {
    let handle = create();
    let big = vec![0xab_u8; AGENT_CHANNEL_RX_QUEUE_BYTES + 100];
    // The whole queue fits; one byte moves into the UART receive register at once.
    assert_eq!(
        write(handle, &big),
        (STATUS_OK, AGENT_CHANNEL_RX_QUEUE_BYTES)
    );
    let s = stats(handle);
    assert_eq!(s.rx_capacity, AGENT_CHANNEL_RX_QUEUE_BYTES as u64);
    assert_eq!(s.rx_queued, AGENT_CHANNEL_RX_QUEUE_BYTES as u64 - 1);
    assert_eq!(s.rx_bytes_delivered, 1);
    assert_eq!(s.rx_overflow_bytes, 100);
    // One slot is free again.
    assert_eq!(write(handle, &[1, 2, 3]), (STATUS_OK, 1));
    assert_eq!(stats(handle).rx_overflow_bytes, 102);
    assert_eq!(unsafe { od_agent_channel_release_v1(handle) }, STATUS_OK);
}

#[test]
fn the_tx_overflow_of_test_inject_is_counted() {
    let handle = create();
    let big = vec![7u8; od_machine::AGENT_CHANNEL_TX_QUEUE_BYTES + 5];
    assert_eq!(
        inject(handle, &big),
        (STATUS_OK, od_machine::AGENT_CHANNEL_TX_QUEUE_BYTES)
    );
    assert_eq!(stats(handle).tx_overflow_bytes, 5);
    assert_eq!(unsafe { od_agent_channel_release_v1(handle) }, STATUS_OK);
}

#[test]
fn stats_check_the_version_and_the_size_of_the_caller_struct() {
    let handle = create();
    let mut out = OdAgentChannelStats::default();
    assert_eq!(
        unsafe { od_agent_channel_stats_v1(handle, &mut out) },
        STATUS_ABI_MISMATCH
    );
    out.abi_version = ABI_VERSION;
    out.struct_size = STATS_SIZE as u32 + 8;
    assert_eq!(
        unsafe { od_agent_channel_stats_v1(handle, &mut out) },
        STATUS_ABI_MISMATCH
    );
    assert_eq!(
        unsafe { od_agent_channel_stats_v1(handle, std::ptr::null_mut()) },
        STATUS_INVALID_ARGUMENT
    );
    let ok = stats(handle);
    assert_eq!(
        (ok.abi_version, ok.struct_size),
        (ABI_VERSION, STATS_SIZE as u32)
    );
    assert_eq!(unsafe { od_agent_channel_release_v1(handle) }, STATUS_OK);
}

/// The unit-level harness: the device on a bus at `0x2F8`, no CPU. The guest side is port
/// accesses; the host side is the ABI.
#[test]
fn bytes_cross_between_port_accesses_of_the_guest_and_the_abi() {
    let (device, channel) = AgentChannel::new();
    let map = MachineMemoryMap::new(
        36,
        vec![MemoryRegionSpec::new(
            GuestPhysicalRange::new(GuestPhysicalAddress::new(0), 1 << 20).unwrap(),
            MemoryRegionKind::Ram,
        )],
    )
    .unwrap();
    let devices: Vec<Box<dyn BusDevice>> = vec![Box::new(device)];
    let ports = vec![PortRegionBinding {
        range: PortRange::new(
            PortAddress::new(UartPortSpec::COM2.base_port),
            UartPortSpec::PORT_BYTES,
        )
        .unwrap(),
        device_index: 0,
    }];
    let mut bus =
        MachineBus::new(map, RamLimits::new(1 << 20), devices, Vec::new(), ports).unwrap();
    let handle = register_agent_channel(channel);
    let port = |offset: u16| PortAddress::new(UartPortSpec::COM2.base_port + offset);

    // Guest: 8N1 with a divisor, then "OK" to the host.
    for (offset, value) in [(3u16, 0x83u32), (0, 1), (1, 0), (3, 3)] {
        bus.write_port(port(offset), AccessWidth::Byte, value)
            .unwrap();
    }
    for byte in *b"OK" {
        bus.write_port(port(0), AccessWidth::Byte, u32::from(byte))
            .unwrap();
        // The host call pumps the device, so a polling guest sees THRE again afterwards.
        let (status, bytes) = read(handle, 1);
        assert_eq!((status, bytes.as_slice()), (STATUS_OK, &[byte][..]));
        assert_eq!(
            bus.read_port(port(5), AccessWidth::Byte).unwrap() & 0x60,
            0x60
        );
    }
    // Host: "hi" to the guest, read through RBR with a pump between the bytes.
    assert_eq!(write(handle, b"hi"), (STATUS_OK, 2));
    assert_eq!(bus.read_port(port(5), AccessWidth::Byte).unwrap() & 1, 1);
    assert_eq!(
        bus.read_port(port(0), AccessWidth::Byte).unwrap(),
        u32::from(b'h')
    );
    assert_eq!(write(handle, b""), (STATUS_OK, 0));
    assert_eq!(
        bus.read_port(port(0), AccessWidth::Byte).unwrap(),
        u32::from(b'i')
    );
    let s = stats(handle);
    assert_eq!((s.rx_bytes_accepted, s.rx_bytes_delivered), (2, 2));
    assert_eq!((s.tx_bytes_accepted, s.tx_bytes_read), (2, 2));
    assert_eq!(unsafe { od_agent_channel_release_v1(handle) }, STATUS_OK);
}

#[test]
fn two_threads_exchange_bytes_through_one_handle() {
    let handle = create();
    let address = handle.addr();
    let producer = std::thread::spawn(move || {
        let handle = std::ptr::without_provenance_mut::<OdAgentChannel>(address);
        let mut sent = 0usize;
        while sent < 1000 {
            let chunk: Vec<u8> = (sent..sent + 10).map(|n| n as u8).collect();
            let (status, count) = inject(handle, &chunk);
            assert_eq!(status, STATUS_OK);
            sent += count;
        }
    });
    // The reader stops when it has everything, when the producer is done and a last read finds
    // nothing, or at the deadline: a lost byte fails the test instead of hanging it.
    let mut got = Vec::new();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while got.len() < 1000 && std::time::Instant::now() < deadline {
        let finished = producer.is_finished();
        let (status, bytes) = read(handle, 64);
        assert_eq!(status, STATUS_OK);
        let empty = bytes.is_empty();
        got.extend(bytes);
        if finished && empty {
            break;
        }
    }
    producer.join().unwrap();
    assert_eq!(got.len(), 1000);
    assert!(got.iter().enumerate().all(|(n, &byte)| byte == n as u8));
    assert_eq!(unsafe { od_agent_channel_release_v1(handle) }, STATUS_OK);
}
