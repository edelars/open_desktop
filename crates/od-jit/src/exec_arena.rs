//! Executable memory for a JIT that places code at addresses known before the code is encoded
//! (the turbo JIT of the unified loop, ADR-0022).
//!
//! `od-jit` does not map memory itself: the host layer (`od-host`) owns the process's executable
//! mapping and hands the JIT an implementation of [`ExecutableArena`].

/// A contiguous executable region owned by the host layer.
///
/// Contract for implementations:
/// - `[base(), base() + len())` stays mapped at the same address for the life of the value;
/// - [`ExecutableArena::write`] makes the written words executable for the calling thread before
///   it returns (write protection restored, instruction cache invalidated for the range);
/// - one thread writes and executes the region at a time (the owner of the value).
///
/// The JIT allocates inside the region itself (it needs the final address of a block before it
/// resolves the block's branches, and it patches words in place).
pub trait ExecutableArena: Send {
    /// The first address of the region.
    fn base(&self) -> usize;
    /// The length of the region in bytes.
    fn len(&self) -> usize;
    /// Copy `words` to `at..at + 4 * words.len()`, which must lie inside the region.
    ///
    /// # Panics
    /// When the range is not inside the region.
    fn write(&mut self, at: usize, words: &[u32]);
}
