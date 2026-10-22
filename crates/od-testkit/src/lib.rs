//! Fixture validation and machine-readable results for independent correctness checks.
//!
//! This crate is test infrastructure. Production runtime crates must not depend on it.

pub mod case;
pub mod compare;
mod error;
pub mod generate;
mod hex_u64;
pub mod minimize;
pub mod result;

pub use error::FormatError;
pub use hex_u64::HexU64;

/// Maximum encoded fixture size accepted by the v1 JSON loader.
pub const MAX_JSON_BYTES: usize = 64 * 1024;
/// Maximum encoded result size accepted by the v1 JSON loader.
pub const MAX_RESULT_JSON_BYTES: usize = 256 * 1024;
/// Maximum program size accepted by the initial S0-09 register-only profile.
pub const MAX_PROGRAM_BYTES: usize = 4096;
/// Maximum sum of memory-region bytes supported by the v1 result format.
pub const MAX_MEMORY_BYTES: usize = 64 * 1024;
/// Maximum number of memory regions supported by the v1 result format.
pub const MAX_MEMORY_REGIONS: usize = 16;

/// Return the lowercase hexadecimal SHA-256 digest of the exact supplied bytes.
pub fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};

    let digest = Sha256::digest(bytes);
    let mut output = String::with_capacity(64);
    for byte in digest {
        use std::fmt::Write as _;
        write!(&mut output, "{byte:02x}").expect("writing to String cannot fail");
    }
    output
}

#[cfg(test)]
mod tests {
    use super::sha256_hex;

    #[test]
    fn sha256_uses_the_standard_lowercase_encoding() {
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }
}
