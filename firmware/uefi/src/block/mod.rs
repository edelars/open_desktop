//! UEFI media access and partition discovery.

pub mod block_io;
pub mod disk_io;
pub(crate) mod partition_gpt;
pub mod storage;
