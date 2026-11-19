//! Firmware implementations of UEFI Boot Services.

pub(crate) mod config_table;
pub mod event;
pub mod exit;
pub mod memory;
pub mod misc;
pub mod protocol;
mod state;

pub use state::{attach_system_tables, attach_watchdog, initialize};
