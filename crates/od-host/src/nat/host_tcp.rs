//! Host sockets for the TCP handler: non-blocking BSD sockets through `libc` (S11-31d).
//!
//! A connection starts with `socket`, `O_NONBLOCK`, `connect` (which answers `EINPROGRESS`); its
//! completion is read with `poll` and `SO_ERROR`. Nothing here blocks, and no listening socket is
//! ever created. The descriptor is owned by an [`OwnedFd`], so dropping the stream closes it.

use std::net::SocketAddrV4;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};

use super::tcp::{HostError, HostStream, IoResult, TcpHost};

fn last_errno() -> i32 {
    std::io::Error::last_os_error().raw_os_error().unwrap_or(0)
}

/// The class of an error of `socket`, `connect` or `SO_ERROR` (NETWORK.md section 11).
fn connect_error(errno: i32) -> HostError {
    match errno {
        libc::ECONNREFUSED => HostError::Refused,
        libc::ENETUNREACH | libc::ENETDOWN => HostError::NetUnreachable,
        libc::EHOSTUNREACH | libc::EHOSTDOWN => HostError::HostUnreachable,
        libc::EMFILE | libc::ENFILE | libc::ENOBUFS | libc::ENOMEM => HostError::Resource,
        libc::ETIMEDOUT => HostError::TimedOut,
        _ => HostError::Other,
    }
}

fn set_flags(fd: &OwnedFd) -> Result<(), HostError> {
    let raw = fd.as_raw_fd();
    // SAFETY: `raw` is an open descriptor owned by `fd`; these calls only change its flags.
    unsafe {
        if libc::fcntl(raw, libc::F_SETFD, libc::FD_CLOEXEC) < 0 {
            return Err(connect_error(last_errno()));
        }
        let flags = libc::fcntl(raw, libc::F_GETFL);
        if flags < 0 || libc::fcntl(raw, libc::F_SETFL, flags | libc::O_NONBLOCK) < 0 {
            return Err(connect_error(last_errno()));
        }
        // A write to a closed connection must give EPIPE, not kill the process.
        #[cfg(target_vendor = "apple")]
        {
            let on: libc::c_int = 1;
            if libc::setsockopt(
                raw,
                libc::SOL_SOCKET,
                libc::SO_NOSIGPIPE,
                (&raw const on).cast(),
                std::mem::size_of::<libc::c_int>() as libc::socklen_t,
            ) < 0
            {
                return Err(connect_error(last_errno()));
            }
        }
    }
    Ok(())
}

/// Opens real sockets (the product implementation of [`TcpHost`]).
pub struct SystemTcpHost;

impl TcpHost for SystemTcpHost {
    fn connect(&mut self, dst: SocketAddrV4) -> Result<Box<dyn HostStream>, HostError> {
        // SAFETY: `socket` has no memory arguments; a non-negative result is a new descriptor that
        // nothing else owns, so `OwnedFd` may take it over.
        let fd = unsafe {
            let raw = libc::socket(libc::AF_INET, libc::SOCK_STREAM, 0);
            if raw < 0 {
                return Err(connect_error(last_errno()));
            }
            OwnedFd::from_raw_fd(raw)
        };
        set_flags(&fd)?;
        // SAFETY: an all-zero `sockaddr_in` is a valid value; the fields set below complete it.
        let mut addr: libc::sockaddr_in = unsafe { std::mem::zeroed() };
        #[cfg(target_vendor = "apple")]
        {
            addr.sin_len = std::mem::size_of::<libc::sockaddr_in>() as u8;
        }
        addr.sin_family = libc::AF_INET as libc::sa_family_t;
        addr.sin_port = dst.port().to_be();
        addr.sin_addr.s_addr = u32::from_ne_bytes(dst.ip().octets());
        // SAFETY: `addr` is a live `sockaddr_in` and the length passed is its size.
        let rc = unsafe {
            libc::connect(
                fd.as_raw_fd(),
                (&raw const addr).cast(),
                std::mem::size_of::<libc::sockaddr_in>() as libc::socklen_t,
            )
        };
        if rc < 0 {
            let errno = last_errno();
            if errno != libc::EINPROGRESS {
                return Err(connect_error(errno));
            }
        }
        Ok(Box::new(SystemStream { fd }))
    }
}

struct SystemStream {
    fd: OwnedFd,
}

impl HostStream for SystemStream {
    fn poll_connected(&mut self) -> Result<bool, HostError> {
        let mut pfd = libc::pollfd {
            fd: self.fd.as_raw_fd(),
            events: libc::POLLOUT,
            revents: 0,
        };
        // SAFETY: `pfd` is one live `pollfd`; the timeout 0 makes the call return at once.
        let ready = unsafe { libc::poll(&raw mut pfd, 1, 0) };
        if ready < 0 {
            return if last_errno() == libc::EINTR {
                Ok(false)
            } else {
                Err(HostError::Other)
            };
        }
        if ready == 0 {
            return Ok(false);
        }
        let mut error: libc::c_int = 0;
        let mut len = std::mem::size_of::<libc::c_int>() as libc::socklen_t;
        // SAFETY: `error` and `len` are live and `len` is the size of `error`.
        let rc = unsafe {
            libc::getsockopt(
                self.fd.as_raw_fd(),
                libc::SOL_SOCKET,
                libc::SO_ERROR,
                (&raw mut error).cast(),
                &raw mut len,
            )
        };
        if rc < 0 {
            return Err(connect_error(last_errno()));
        }
        if error != 0 {
            return Err(connect_error(error));
        }
        Ok(true)
    }

    fn read(&mut self, buf: &mut [u8]) -> IoResult {
        if buf.is_empty() {
            return IoResult::WouldBlock;
        }
        // SAFETY: `buf` is a live writable slice and its length is passed.
        let n = unsafe { libc::recv(self.fd.as_raw_fd(), buf.as_mut_ptr().cast(), buf.len(), 0) };
        match n {
            0 => IoResult::Eof,
            n if n > 0 => IoResult::Ready(n as usize),
            _ => classify_io(last_errno()),
        }
    }

    fn write(&mut self, buf: &[u8]) -> IoResult {
        if buf.is_empty() {
            return IoResult::WouldBlock;
        }
        #[cfg(target_vendor = "apple")]
        let flags = 0;
        #[cfg(not(target_vendor = "apple"))]
        let flags = libc::MSG_NOSIGNAL;
        // SAFETY: `buf` is a live readable slice and its length is passed.
        let n = unsafe { libc::send(self.fd.as_raw_fd(), buf.as_ptr().cast(), buf.len(), flags) };
        if n >= 0 {
            IoResult::Ready(n as usize)
        } else {
            classify_io(last_errno())
        }
    }

    fn shutdown_write(&mut self) {
        // SAFETY: shutting down an open descriptor touches no memory; a failure shows up in the
        // next read or write, which is where the handler looks for host errors.
        unsafe {
            libc::shutdown(self.fd.as_raw_fd(), libc::SHUT_WR);
        }
    }
}

fn classify_io(errno: i32) -> IoResult {
    if errno == libc::EAGAIN || errno == libc::EWOULDBLOCK || errno == libc::EINTR {
        IoResult::WouldBlock
    } else {
        IoResult::Failed
    }
}
