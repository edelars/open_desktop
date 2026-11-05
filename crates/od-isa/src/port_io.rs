//! Narrow CPU-to-machine boundary for bounded port-I/O instruction profiles.

/// Byte port-I/O operations used by explicitly selected interpreter profiles.
///
/// Implementations are called at most once for an admitted instruction. A
/// failed operation must report its error without retrying internally.
pub trait PortIo {
    type Error;

    fn read_u8(&mut self, port: u16) -> Result<u8, Self::Error>;
    fn write_u8(&mut self, port: u16, value: u8) -> Result<(), Self::Error>;
}
