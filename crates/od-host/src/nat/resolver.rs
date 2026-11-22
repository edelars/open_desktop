//! Name resolution for the DNS forwarder (S11-31b, NETWORK.md section 8).
//!
//! [`Resolver`] is the narrow interface the forwarder needs: the IPv4 addresses of a name. The product
//! implementation, [`SystemResolver`], calls the system resolver (`getaddrinfo`), which honours
//! `/etc/hosts`, the network settings and the scoped resolvers of macOS. The call blocks and cannot be
//! interrupted: it runs only on the worker threads of [`super::dns::DnsService`]. Tests use their own
//! implementations, so no test needs a network.

use std::net::Ipv4Addr;

/// Result of looking up the A records of one name.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ResolveOutcome {
    /// The addresses `getaddrinfo` returned for `AF_INET`, in its order (may repeat, may be empty).
    Addrs(Vec<Ipv4Addr>),
    /// The name exists but has no IPv4 address (`EAI_NODATA`, where the system reports it).
    NoData,
    /// The name does not exist (`EAI_NONAME`).
    NotFound,
    /// The resolver failed: `EAI_AGAIN`, `EAI_FAIL`, any other error.
    Failed,
}

pub trait Resolver: Send + Sync + 'static {
    /// Looks up the IPv4 addresses of `name`: an ASCII host name without a trailing dot. Blocks.
    fn resolve_a(&self, name: &str) -> ResolveOutcome;
}

/// The system resolver of the host.
pub struct SystemResolver;

impl Resolver for SystemResolver {
    fn resolve_a(&self, name: &str) -> ResolveOutcome {
        system_resolve_a(name)
    }
}

#[cfg(unix)]
fn system_resolve_a(name: &str) -> ResolveOutcome {
    use std::ffi::CString;
    use std::ptr;

    // A name with a NUL byte cannot be passed to the C interface (the forwarder never offers one).
    let Ok(node) = CString::new(name) else {
        return ResolveOutcome::NotFound;
    };
    // SAFETY: an all-zero `addrinfo` is a valid "no hints" value (null pointers, zero numbers); the
    // fields the resolver reads are set below.
    let mut hints: libc::addrinfo = unsafe { std::mem::zeroed() };
    hints.ai_family = libc::AF_INET;
    // One socket type, so that an address is not repeated for stream, datagram and raw.
    hints.ai_socktype = libc::SOCK_STREAM;
    let mut list: *mut libc::addrinfo = ptr::null_mut();
    // SAFETY: `node` is a NUL-terminated string that outlives the call, `hints` is initialised, `list`
    // is a valid out-pointer; on success the list is released by `freeaddrinfo` below.
    let status = unsafe { libc::getaddrinfo(node.as_ptr(), ptr::null(), &hints, &mut list) };
    if status != 0 {
        return classify_error(status);
    }
    let mut addrs = Vec::new();
    let mut entry = list;
    while !entry.is_null() {
        // SAFETY: `entry` is a node of the list `getaddrinfo` returned and not yet released.
        let node = unsafe { &*entry };
        if node.ai_family == libc::AF_INET
            && !node.ai_addr.is_null()
            && node.ai_addrlen as usize >= std::mem::size_of::<libc::sockaddr_in>()
        {
            // SAFETY: the family is AF_INET and the length covers a `sockaddr_in`; `read_unaligned`
            // because the C allocation is only promised to be suitably aligned for `sockaddr`.
            let sin = unsafe { ptr::read_unaligned(node.ai_addr.cast::<libc::sockaddr_in>()) };
            // `s_addr` is in network byte order: its in-memory bytes are the address octets.
            addrs.push(Ipv4Addr::from(sin.sin_addr.s_addr.to_ne_bytes()));
        }
        entry = node.ai_next;
    }
    // SAFETY: `list` came from a successful `getaddrinfo` and is released exactly once.
    unsafe { libc::freeaddrinfo(list) };
    ResolveOutcome::Addrs(addrs)
}

#[cfg(unix)]
fn classify_error(status: libc::c_int) -> ResolveOutcome {
    #[cfg(target_vendor = "apple")]
    if status == libc::EAI_NODATA {
        return ResolveOutcome::NoData;
    }
    if status == libc::EAI_NONAME {
        ResolveOutcome::NotFound
    } else {
        ResolveOutcome::Failed
    }
}

/// Hosts without a system resolver interface: every lookup fails (the product host is macOS).
#[cfg(not(unix))]
fn system_resolve_a(_name: &str) -> ResolveOutcome {
    ResolveOutcome::Failed
}
