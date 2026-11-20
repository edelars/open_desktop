//! Trusted computing for the firmware board.

mod crb_driver;
mod event_log;
pub mod measure;
#[cfg(target_os = "none")]
pub mod secure_boot;
mod tcg2;

pub use tcg2::{
    FINAL_EVENTS_GUID, InitializedTpm, TCG2_PROTOCOL_GUID, Tcg2Protocol, active, initialize,
    install, measure_bytes,
};
