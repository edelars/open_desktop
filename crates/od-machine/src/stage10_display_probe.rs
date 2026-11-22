//! Guest display probe loader (S10-08).
//!
//! The probe image is own freestanding x86-64 code in an ODFWIMG1 container, built by
//! `tools/build/build_stage10_display_probe.py` (see its manifest in
//! `tests/fixtures/guest/stage10/`). It runs on the firmware board: the image is
//! loaded with [`load_firmware_image`] and advanced with
//! [`LoadedFirmwareBoard::run_boot`] (the CPL0 set loop). That path executes dword MMIO
//! stores into the display registers and dword stores into the framebuffer region
//! (DISPLAY_DEVICE.md 13.1). Nothing here writes a display register or a framebuffer byte
//! on behalf of the guest: the frame is whatever the guest code stored.

use crate::acpi_blob::sha256;
use crate::{
    DisplayHandle, DisplayMode, DisplayReadError, FIRMWARE_IMAGE_BYTES, FirmwareBoardConfig,
    FirmwareBoardError, FrameInfo, LoadedFirmwareBoard, MachineBus, PagedProbeExit,
    load_firmware_image,
};
use od_isa::Register;
use std::fmt;

#[derive(Debug, Eq, PartialEq)]
pub enum Stage10DisplayProbeError {
    /// The stored image is longer than the 4 MiB firmware image.
    ImageTooLarge { actual: usize },
    /// The firmware board refused the padded image (header, CRC, configuration).
    Board(FirmwareBoardError),
}

impl fmt::Display for Stage10DisplayProbeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Stage10DisplayProbe load failed: {self:?}")
    }
}

impl std::error::Error for Stage10DisplayProbeError {}

/// How far the probe got in one [`LoadedStage10DisplayProbe::step`] call.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Stage10ProbeOutcome {
    /// The budget was used up; the guest is still running.
    BudgetExhausted,
    /// The guest executed HLT. Final: later steps execute nothing.
    Halted,
    /// Any other end (fault, unsupported opcode, backend or port error). Final.
    Failed,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Stage10ProbeStep {
    pub outcome: Stage10ProbeOutcome,
    /// Instructions retired by this call.
    pub retired: u64,
    /// For [`Stage10ProbeOutcome::Failed`]: the exit of the paged loop, as text.
    pub failure: Option<String>,
}

/// What the probe and the device show after the run. STATUS values are what the guest itself
/// read: EDX after `MODE_COMMIT`, R9D after the `FRAME_GENERATION` doorbell.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Stage10DisplayProbeReport {
    pub outcome: Option<Stage10ProbeOutcome>,
    pub failure: Option<String>,
    pub retired: u64,
    /// Set only when the guest halted: before HLT these registers hold nothing the probe wrote.
    pub status_after_commit: Option<u32>,
    pub status_after_frame: Option<u32>,
    pub mode: DisplayMode,
    pub generation: u64,
    pub mode_change_count: u64,
    /// SHA-256 of the frame copied by `DisplayHandle::read_frame_into`; `None` if no frame
    /// can be read (see `frame_error`).
    pub frame_sha256: Option<[u8; 32]>,
    pub frame_error: Option<String>,
}

impl Stage10DisplayProbeReport {
    pub fn frame_sha256_hex(&self) -> Option<String> {
        self.frame_sha256.map(|digest| hex(&digest))
    }
}

pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// A probe image loaded on the firmware board, not yet (or partly) run.
pub struct LoadedStage10DisplayProbe {
    board: LoadedFirmwareBoard,
    finished: Option<Stage10ProbeOutcome>,
    failure: Option<String>,
    retired: u64,
}

/// Loads a stored probe image: an ODFWIMG1 image, optionally with trailing zero bytes trimmed.
/// It is zero-padded to [`FIRMWARE_IMAGE_BYTES`] and loaded on the default firmware board
/// (the display device at its fixed addresses, no other optional device).
pub fn load_stage10_display_probe(
    image: &[u8],
) -> Result<LoadedStage10DisplayProbe, Stage10DisplayProbeError> {
    if image.len() > FIRMWARE_IMAGE_BYTES {
        return Err(Stage10DisplayProbeError::ImageTooLarge {
            actual: image.len(),
        });
    }
    let mut padded = vec![0u8; FIRMWARE_IMAGE_BYTES];
    padded[..image.len()].copy_from_slice(image);
    let board = load_firmware_image(&padded, &FirmwareBoardConfig::default())
        .map_err(Stage10DisplayProbeError::Board)?;
    Ok(LoadedStage10DisplayProbe {
        board,
        finished: None,
        failure: None,
        retired: 0,
    })
}

impl LoadedStage10DisplayProbe {
    /// Executes at most `budget` guest instructions. After a final outcome (HLT or a
    /// failure) it executes nothing and repeats that outcome with `retired = 0`.
    pub fn step(&mut self, budget: u64) -> Result<Stage10ProbeStep, FirmwareBoardError> {
        if let Some(outcome) = self.finished {
            return Ok(Stage10ProbeStep {
                outcome,
                retired: 0,
                failure: self.failure.clone(),
            });
        }
        if budget == 0 {
            return Ok(Stage10ProbeStep {
                outcome: Stage10ProbeOutcome::BudgetExhausted,
                retired: 0,
                failure: None,
            });
        }
        let report = self.board.run_boot(budget)?;
        self.retired += report.retired;
        let outcome = match report.exit {
            PagedProbeExit::Halted => Stage10ProbeOutcome::Halted,
            PagedProbeExit::BudgetExhausted => Stage10ProbeOutcome::BudgetExhausted,
            _ => Stage10ProbeOutcome::Failed,
        };
        let failure = (outcome == Stage10ProbeOutcome::Failed).then(|| {
            format!(
                "{:?}; page_fault={:?}; memory_error={:?}",
                report.exit, report.page_fault, report.memory_error
            )
        });
        if outcome != Stage10ProbeOutcome::BudgetExhausted {
            self.finished = Some(outcome);
            self.failure = failure.clone();
        }
        Ok(Stage10ProbeStep {
            outcome,
            retired: report.retired,
            failure,
        })
    }

    pub fn display(&self) -> DisplayHandle {
        self.board.display()
    }

    pub fn bus(&self) -> &MachineBus {
        self.board.bus()
    }

    /// Copies the current frame through `DisplayHandle::read_frame_into`.
    pub fn read_frame(&self) -> Result<(FrameInfo, Vec<u8>), DisplayReadError> {
        let display = self.board.display();
        let bytes = display.current_mode().frame_bytes();
        let mut frame = vec![0u8; usize::try_from(bytes).unwrap_or(0)];
        let info = display.read_frame_into(self.board.bus(), &mut frame)?;
        Ok((info, frame))
    }

    pub fn report(&self) -> Stage10DisplayProbeReport {
        let display = self.board.display();
        let gpr = &self.board.state().gpr;
        let halted = self.finished == Some(Stage10ProbeOutcome::Halted);
        let (frame_sha256, frame_error) = match self.read_frame() {
            Ok((_, frame)) => (Some(sha256(&frame)), None),
            Err(error) => (None, Some(format!("{error:?}"))),
        };
        Stage10DisplayProbeReport {
            outcome: self.finished,
            failure: self.failure.clone(),
            retired: self.retired,
            status_after_commit: halted.then(|| gpr[Register::Rdx as usize] as u32),
            status_after_frame: halted.then(|| gpr[Register::R9 as usize] as u32),
            mode: display.current_mode(),
            generation: display.frame_generation(),
            mode_change_count: display.mode_change_count(),
            frame_sha256,
            frame_error,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    const IMAGE: &[u8] =
        include_bytes!("../../../tests/fixtures/guest/stage10/display_probe_bars_64x48_seed0.img");
    const MANIFEST: &str =
        include_str!("../../../tests/fixtures/guest/stage10/display_probe_bars_64x48_seed0.json");
    const REFERENCE: &[u8] =
        include_bytes!("../../../tests/fixtures/guest/stage10/bars_64x48_seed0.raw");

    fn manifest() -> Value {
        serde_json::from_str(MANIFEST).unwrap()
    }

    fn number(manifest: &Value, key: &str) -> u64 {
        manifest[key].as_u64().unwrap()
    }

    #[test]
    fn stage10_display_probe_fixture_matches_its_manifest() {
        let manifest = manifest();
        assert_eq!(number(&manifest, "image_bytes"), IMAGE.len() as u64);
        assert_eq!(
            manifest["image_sha256"].as_str().unwrap(),
            hex(&sha256(IMAGE))
        );
        assert_eq!(
            manifest["expected_frame_sha256"].as_str().unwrap(),
            hex(&sha256(REFERENCE))
        );
        assert_eq!(number(&manifest, "frame_bytes"), REFERENCE.len() as u64);
    }

    #[test]
    fn stage10_display_probe_draws_the_manifest_frame() {
        let manifest = manifest();
        let mut probe = load_stage10_display_probe(IMAGE).unwrap();
        let step = probe.step(number(&manifest, "instruction_budget")).unwrap();
        assert_eq!(step.outcome, Stage10ProbeOutcome::Halted, "{step:?}");
        assert_eq!(step.retired, number(&manifest, "expected_retired"));
        let report = probe.report();
        assert_eq!(report.outcome, Some(Stage10ProbeOutcome::Halted));
        assert_eq!(
            u64::from(report.status_after_commit.unwrap()),
            number(&manifest, "expected_status_after_commit")
        );
        assert_eq!(
            u64::from(report.status_after_frame.unwrap()),
            number(&manifest, "expected_status_after_frame")
        );
        assert_eq!(report.generation, number(&manifest, "expected_generation"));
        assert_eq!(report.mode.width as u64, number(&manifest, "width"));
        assert_eq!(report.mode.height as u64, number(&manifest, "height"));
        assert_eq!(report.mode.stride as u64, number(&manifest, "stride"));
        assert_eq!(report.mode.format as u64, number(&manifest, "format"));
        assert_eq!(
            report.frame_sha256_hex().unwrap(),
            manifest["expected_frame_sha256"].as_str().unwrap()
        );
        let (info, frame) = probe.read_frame().unwrap();
        assert_eq!(info.generation, 1);
        assert_eq!(
            frame, REFERENCE,
            "the guest frame is the reference, byte for byte"
        );
    }

    #[test]
    fn stage10_display_probe_resumes_across_small_quanta() {
        let manifest = manifest();
        let mut probe = load_stage10_display_probe(IMAGE).unwrap();
        let mut total = 0;
        let mut steps = 0;
        loop {
            let step = probe.step(1000).unwrap();
            total += step.retired;
            steps += 1;
            assert!(steps < 1000, "the probe must reach HLT");
            if step.outcome == Stage10ProbeOutcome::Halted {
                break;
            }
            assert_eq!(step.outcome, Stage10ProbeOutcome::BudgetExhausted);
            assert!(step.retired <= 1000);
        }
        assert!(steps > 1);
        assert_eq!(total, number(&manifest, "expected_retired"));
        assert_eq!(
            probe.report().frame_sha256_hex().unwrap(),
            manifest["expected_frame_sha256"].as_str().unwrap()
        );
        // HLT is final: nothing more is executed.
        let again = probe.step(1000).unwrap();
        assert_eq!(again.outcome, Stage10ProbeOutcome::Halted);
        assert_eq!(again.retired, 0);
    }

    #[test]
    fn stage10_display_probe_budget_exhausted_has_no_frame_yet() {
        let mut probe = load_stage10_display_probe(IMAGE).unwrap();
        let step = probe.step(50).unwrap();
        assert_eq!(step.outcome, Stage10ProbeOutcome::BudgetExhausted);
        assert_eq!(step.retired, 50);
        let report = probe.report();
        assert_eq!(report.outcome, None);
        assert_eq!(report.generation, 0);
        assert_eq!(report.status_after_commit, None);
        assert_eq!(report.status_after_frame, None);
        assert_eq!(report.frame_sha256, None);
        assert!(report.frame_error.is_some());
        let zero = probe.step(0).unwrap();
        assert_eq!(zero.outcome, Stage10ProbeOutcome::BudgetExhausted);
        assert_eq!(zero.retired, 0);
    }

    #[test]
    fn stage10_display_probe_can_move_between_threads() {
        // od-host keeps the probe inside a session that is dropped on any thread.
        fn assert_send<T: Send>() {}
        assert_send::<LoadedStage10DisplayProbe>();
    }

    #[test]
    fn stage10_display_probe_refuses_bad_images() {
        let too_large = vec![0u8; FIRMWARE_IMAGE_BYTES + 1];
        assert!(matches!(
            load_stage10_display_probe(&too_large),
            Err(Stage10DisplayProbeError::ImageTooLarge { actual }) if actual == FIRMWARE_IMAGE_BYTES + 1
        ));
        let mut corrupt = IMAGE.to_vec();
        corrupt[0x41] ^= 0xFF;
        assert!(matches!(
            load_stage10_display_probe(&corrupt),
            Err(Stage10DisplayProbeError::Board(
                FirmwareBoardError::ImageCrc { .. }
            ))
        ));
    }

    #[test]
    fn stage10_display_probe_stops_on_an_unsupported_access() {
        // Break the display register base: the store goes to unmapped physical memory.
        let mut image = IMAGE.to_vec();
        image[0x44] = 0xFD; // mov esi, 0xFD000000
        // Recompute the CRC over the padded image.
        image[0x30..0x34].fill(0);
        let mut padded = image.clone();
        padded.resize(FIRMWARE_IMAGE_BYTES, 0);
        let crc = crate::crc32::crc32_ieee(&[&padded[..]]);
        image[0x30..0x34].copy_from_slice(&crc.to_le_bytes());
        let mut probe = load_stage10_display_probe(&image).unwrap();
        let step = probe.step(1000).unwrap();
        assert_eq!(step.outcome, Stage10ProbeOutcome::Failed, "{step:?}");
        assert!(step.failure.is_some());
        assert_eq!(probe.display().frame_generation(), 0);
        let again = probe.step(1000).unwrap();
        assert_eq!(again.outcome, Stage10ProbeOutcome::Failed);
        assert_eq!(again.retired, 0);
    }
}
