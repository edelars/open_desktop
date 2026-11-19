//! Firmware implementations of UEFI Boot Services.

pub(crate) mod config_table;
pub mod event;
pub mod memory;
pub mod misc;
pub mod protocol;
mod state;

pub use state::{attach_system_table, initialize};
