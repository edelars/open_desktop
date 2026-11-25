//! The executable region of the turbo JIT (ADR-0022): the process's single MAP_JIT mapping under
//! the same lease as every other publication of this crate, handed to the JIT as an
//! [`od_jit::exec_arena::ExecutableArena`].

use crate::{ExecutableMemoryError, MappedRegion};

/// One MAP_JIT region the turbo JIT allocates in. While it lives, no other MAP_JIT publication of
/// the process can be made ([`crate::map_jit_region_live`] is `true`).
pub struct JitArena {
    region: MappedRegion,
}

impl JitArena {
    /// Map `len` bytes. Fails with [`ExecutableMemoryError::MappingAlreadyLive`] while another
    /// MAP_JIT region of the process is live, and with
    /// [`ExecutableMemoryError::UnsupportedPlatform`] outside macOS on AArch64.
    pub fn new(len: usize) -> Result<Self, ExecutableMemoryError> {
        Ok(Self {
            region: MappedRegion::allocate(len)?,
        })
    }
}

// SAFETY: the value owns its mapping and lease outright (no shared state besides the process
// lease flag, which is atomic); moving it to another thread moves the whole region. Writes go
// through `&mut self` and the JIT write protection toggle is per thread.
unsafe impl Send for JitArena {}

impl od_jit::exec_arena::ExecutableArena for JitArena {
    fn base(&self) -> usize {
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        {
            self.region.ptr().as_ptr() as usize
        }
        #[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
        {
            unreachable!("no MAP_JIT region on this host")
        }
    }

    fn len(&self) -> usize {
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        {
            self.region.len
        }
        #[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
        {
            unreachable!("no MAP_JIT region on this host")
        }
    }

    fn write(&mut self, at: usize, words: &[u32]) {
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        {
            let bytes = words.len() * 4;
            let base = self.base();
            assert!(
                at >= base
                    && at
                        .checked_add(bytes)
                        .is_some_and(|end| end <= base + self.len()),
                "JitArena::write outside the region"
            );
            // SAFETY: host words are little-endian AArch64 instruction words; the byte view
            // covers exactly `words`.
            let source = unsafe { std::slice::from_raw_parts(words.as_ptr().cast::<u8>(), bytes) };
            // SAFETY: the range was checked to lie inside the live mapping owned by `self`, and the
            // source (a borrowed slice) cannot alias it.
            unsafe {
                crate::write_mapping_bytes(std::ptr::NonNull::new_unchecked(at as *mut u8), source)
            };
        }
        #[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
        {
            let _ = (at, words);
            unreachable!("no MAP_JIT region on this host")
        }
    }
}

#[cfg(all(test, target_os = "macos", target_arch = "aarch64"))]
mod tests {
    use super::JitArena;
    use od_jit::exec_arena::ExecutableArena;

    #[test]
    fn written_words_run_and_the_lease_is_held_while_the_arena_lives() {
        let _guard = crate::tests::jit_test_lock();
        let mut arena = JitArena::new(1 << 16).expect("map");
        assert!(crate::map_jit_region_live());
        assert_eq!(
            crate::JitArena::new(4096).err(),
            Some(crate::ExecutableMemoryError::MappingAlreadyLive)
        );
        // mov x0, #42 ; ret
        let at = arena.base() + 64;
        arena.write(at, &[0xd280_0540, 0xd65f_03c0]);
        // SAFETY: two valid AArch64 instructions just written and made executable.
        let f: extern "C" fn() -> u64 = unsafe { std::mem::transmute(at) };
        assert_eq!(f(), 42);
        // Patch in place: mov x0, #7.
        arena.write(at, &[0xd280_00e0]);
        assert_eq!(f(), 7);
        drop(arena);
        assert!(!crate::map_jit_region_live());
    }

    #[test]
    #[should_panic(expected = "outside the region")]
    fn a_write_outside_the_region_panics() {
        let _guard = crate::tests::jit_test_lock();
        let mut arena = JitArena::new(4096).expect("map");
        let end = arena.base() + arena.len();
        arena.write(end - 4, &[0, 0]);
    }
}
