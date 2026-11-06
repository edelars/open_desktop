//! Disk backends, storage metadata, and snapshot containers.
//!
//! Implemented so far: the narrow storage I/O contract in [`io`] and the
//! headerless RAW image in [`raw`]. CoW disk format, metadata, and snapshot
//! containers have not started. This crate never imports `od-host`; a host
//! layer implements [`io::StorageBackend`].

pub mod io;
pub mod raw;

/// Prototype of a minimal CoW commit protocol (S9-08). Not a normative format.
#[cfg(any(test, feature = "test-support"))]
pub mod cow_proto;

/// Strict read-only parser for the prototype layout (S9-09). Not normative.
#[cfg(any(test, feature = "test-support"))]
pub mod cow_parse;

/// Byte-addressed write transaction over the prototype commit (S9-11). Not
/// normative.
#[cfg(any(test, feature = "test-support"))]
pub mod cow_tx;

/// Immutable snapshots, bounded parent chains and writable children over the
/// prototype commit (S9-14). Not normative.
#[cfg(any(test, feature = "test-support"))]
pub mod cow_snapshot;

/// In-memory backend and the shared backend contract suite. Enabled by tests
/// and by the `test-support` feature for host crates that run the same suite.
#[cfg(any(test, feature = "test-support"))]
pub mod testing;

#[cfg(test)]
mod cow_open_tests;
#[cfg(test)]
mod cow_parse_tests;
#[cfg(test)]
mod cow_proto_tests;
#[cfg(test)]
mod cow_recovery_tests;
#[cfg(test)]
mod cow_snapshot_tests;
#[cfg(test)]
mod cow_tx_tests;
#[cfg(test)]
mod io_tests;
