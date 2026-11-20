//! PE32+ layout and Authenticode hashing shared by the loader and image policy.

mod authenticode;
mod parser;
mod win_certificate;

pub use authenticode::{DigestAlgorithm, DigestError, authenticode_digest};
pub use parser::{Error, MAX_SECTIONS, PeImage, Section, Subsystem};
pub use win_certificate::{CertificateError, WinCertificate, WinCertificates};
