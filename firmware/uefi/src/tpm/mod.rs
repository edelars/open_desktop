//! Trusted computing for the firmware board.

mod crb_driver;
mod event_log;
pub mod measure;
mod tcg2;

pub use tcg2::{
    FINAL_EVENTS_GUID, TCG2_PROTOCOL_GUID, Tcg2Protocol, active, install, measure_bytes,
};
