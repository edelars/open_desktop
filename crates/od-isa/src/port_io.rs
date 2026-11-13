//! Narrow CPU-to-machine boundary for bounded port-I/O instruction profiles.

/// Byte port-I/O operations used by explicitly selected interpreter profiles.
///
/// Implementations are called at most once for an admitted instruction. A
/// failed operation must report its error without retrying internally.
pub trait PortIo {
    type Error;

    fn read_u8(&mut self, port: u16) -> Result<u8, Self::Error>;
    fn write_u8(&mut self, port: u16, value: u8) -> Result<(), Self::Error>;

    /// S4-38L9r: one 16-bit access at `port` (`IN AX, DX` / `OUT DX, AX`). The CPU calls it once
    /// per admitted word instruction. A bus-backed implementation must forward it as one
    /// access of width 2, so a device error leaves no partial effect. The provided default is
    /// for byte-only test doubles: it is two little-endian byte accesses at `port` and
    /// `port + 1` (wrapping at 16 bits) and is not atomic.
    fn read_u16(&mut self, port: u16) -> Result<u16, Self::Error> {
        let low = self.read_u8(port)?;
        let high = self.read_u8(port.wrapping_add(1))?;
        Ok(u16::from_le_bytes([low, high]))
    }

    /// The 16-bit write counterpart of [`PortIo::read_u16`], with the same contract.
    fn write_u16(&mut self, port: u16, value: u16) -> Result<(), Self::Error> {
        let [low, high] = value.to_le_bytes();
        self.write_u8(port, low)?;
        self.write_u8(port.wrapping_add(1), high)
    }
}
