//! I/O command set (`Read`, `Write`, `Flush`): NOT IMPLEMENTED in slice A.
//!
//! Slice B implements the three NVM commands over a disk backend. Until then
//! every I/O command completes with `Invalid Command Opcode`, which is what a
//! controller without the command returns. This is a recorded gap against the
//! mandatory NVM command set, not a stub that reports success
//! (`docs/specs/NVME_DEVICE.md`, "Gaps").

use super::queues::{Command, Completion, Outcome, Status};

pub(super) fn execute(_command: &Command) -> Outcome {
    Outcome::Done(Completion::error(Status::INVALID_OPCODE))
}
