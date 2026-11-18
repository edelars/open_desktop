//! Disk backends, storage metadata, and snapshot containers.
//!
//! Always built: the narrow storage I/O contract in [`io`] and the headerless
//! RAW image in [`raw`]. Behind `cfg(any(test, feature = "test-support"))`:
//! the CoW v1 commit protocol, parser and transaction (`cow_proto`,
//! `cow_parse`, `cow_tx`; normative since 2026-11-06, see
//! `docs/specs/STORAGE_FORMAT.md` §9), and the non-normative snapshot layer
//! with inspect, export/import and salvage on top of it. This crate never
//! imports `od-host`; a host layer implements [`io::StorageBackend`].

pub mod io;
pub mod raw;

/// CoW v1 commit protocol (S9-08); normative layout since 2026-11-06
/// (`STORAGE_FORMAT.md` §9).
#[cfg(any(test, feature = "test-support"))]
pub mod cow_proto;

/// Strict read-only parser for the CoW v1 layout (S9-09).
#[cfg(any(test, feature = "test-support"))]
pub mod cow_parse;

/// Byte-addressed write transaction over the CoW v1 commit (S9-11).
#[cfg(any(test, feature = "test-support"))]
pub mod cow_tx;

/// Immutable snapshots, bounded parent chains and writable children over the
/// CoW v1 commit (S9-14). The snapshot layer itself is not normative.
#[cfg(any(test, feature = "test-support"))]
pub mod cow_snapshot;

/// Read-only inspection and check of an image (S9-19).
#[cfg(any(test, feature = "test-support"))]
pub mod cow_inspect;

/// RAW import/export with a verified result (S9-20) and salvage into a new
/// copy (S9-21). Not normative.
#[cfg(any(test, feature = "test-support"))]
pub mod cow_export;

#[cfg(any(test, feature = "test-support"))]
pub mod cow_salvage;

/// In-memory backend and the shared backend contract suite. Enabled by tests
/// and by the `test-support` feature for host crates that run the same suite.
#[cfg(any(test, feature = "test-support"))]
pub mod testing;

#[cfg(test)]
mod cow_export_tests;
#[cfg(test)]
mod cow_fixture_tests;
#[cfg(test)]
mod cow_inspect_tests;
#[cfg(test)]
mod cow_open_tests;
#[cfg(test)]
mod cow_parse_tests;
#[cfg(test)]
mod cow_proto_tests;
#[cfg(test)]
mod cow_recovery_tests;
#[cfg(test)]
mod cow_salvage_tests;
#[cfg(test)]
mod cow_snapshot_tests;
#[cfg(test)]
mod cow_soak_tests;
#[cfg(test)]
mod cow_tx_tests;
#[cfg(test)]
mod io_tests;
