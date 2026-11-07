//! Open Desktop vTPM 2.0 command processor, phase A (S7-21) and slice A of phase B (S7-23).
//!
//! Pure logic without bus, MMIO, threads, files, OS clock or OS entropy (ADR-0009, decision 1):
//! bytes of a command and a locality in, bytes of a response out. Time and entropy are arguments
//! from the owner. The CRB device (S7-22) lives in `od-machine`.
//!
//! Phase A: startup/shutdown, self-test, capabilities, random, PCR (SHA-256 bank), hash and
//! hash/event sequences, clock, password sessions. Phase B slice A: hierarchies (Platform, Owner,
//! Endorsement, Null, Lockout authorization), HMAC sessions, flushing, and the base of NV
//! indices. Everything else answers `TPM_RC_COMMAND_CODE`.
//! The contract, the command table with the limits and the list of values not verified against
//! the TPM 2.0 Library v185 text are in `docs/specs/VTPM.md`. TCG conformance or certification
//! is not claimed.
//!
//! Secrets (hierarchy seeds, DRBG state, passwords) are wiped on drop and never printed by
//! `Debug`; no function of this crate logs.

pub mod capability;
pub mod codes;
pub mod command;
pub mod crypto;
pub mod dispatch;
pub mod hash;
pub mod hierarchy;
pub mod marshal;
pub mod nv;
pub mod pcr;
pub mod random;
pub mod response;
pub mod session;
pub mod startup;
pub mod state;

pub use response::Rc;
pub use state::{SNAPSHOT_VERSION, StateError, Tpm};

#[cfg(test)]
mod tests;
