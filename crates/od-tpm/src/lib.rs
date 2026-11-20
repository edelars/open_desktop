//! Open Desktop vTPM 2.0 command processor, phase A (S7-21) and slices A, B, C1, C2 and C3 of
//! phase B (S7-23).
//!
//! Pure logic without bus, MMIO, threads, files, OS clock or OS entropy (ADR-0009, decision 1):
//! bytes of a command and a locality in, bytes of a response out. Time and entropy are arguments
//! from the owner. The CRB device (S7-22) lives in `od-machine`.
//!
//! Phase A: startup/shutdown, self-test, capabilities, random, PCR (SHA-256 bank), hash and
//! hash/event sequences, clock, password sessions. Phase B slice A: hierarchies (Platform, Owner,
//! Endorsement, Null, Lockout authorization), HMAC sessions, flushing, and the base of NV
//! indices. Slice B: RSA-2048 and ECC P-256 objects (primary, child, external, sealed data):
//! `CreatePrimary`, `Create`, `Load`, `LoadExternal`, `ReadPublic`, `Unseal`, `EvictControl`,
//! `Sign`, `VerifySignature`. Slice C1: `RSA_Encrypt`, `RSA_Decrypt`, `ECDH_ZGen`, dictionary-attack
//! protection (`DictionaryAttackLockReset`, `DictionaryAttackParameters`) and the full persistent
//! state (snapshot `ODTPMSTA`, now v4). Slice C2: policy sessions, `ContextSave/Load`, `TPM2_HMAC`,
//! `Quote`, `Certify`. Slice C3: NV counters, bit fields and extend indices, `TPM2_GetTime`,
//! `TPM_RH_NULL` as signing handle, parameter encryption (AES-128-CFB and XOR). Everything else
//! answers `TPM_RC_COMMAND_CODE`.
//! The contract, the command table with the limits and the list of values not verified against
//! the TPM 2.0 Library v185 text are in `docs/specs/VTPM.md`. TCG conformance or certification
//! is not claimed.
//!
//! Secrets (hierarchy seeds, DRBG state, passwords) are wiped on drop and never printed by
//! `Debug`; no function of this crate logs.

mod asym;
mod attest;
pub mod capability;
pub mod codes;
pub mod command;
mod context;
mod credential;
pub mod crypto;
mod da;
pub mod dispatch;
mod duplicate;
pub mod ek;
pub mod hash;
pub mod hierarchy;
pub mod marshal;
pub mod nv;
pub mod object;
mod object_cmd;
mod paramenc;
pub mod pcr;
mod policy;
pub mod random;
pub mod response;
mod self_test;
pub mod session;
mod snapshot;
pub mod startup;
pub mod state;

pub use response::Rc;
pub use state::{SNAPSHOT_VERSION, StateError, Tpm};

#[cfg(test)]
mod tests;
