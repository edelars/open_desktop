//! `TPM2_Startup`, `TPM2_Shutdown`, `TPM2_SelfTest`, `TPM2_GetTestResult`.
//!
//! Sequences (Part 3 v184, clause 9.3.1): *Reset* is `Startup(CLEAR)` after `Shutdown(CLEAR)` or
//! after no shutdown (`resetCount` + 1, `restartCount` = 0); *Restart* is `Startup(CLEAR)` after
//! `Shutdown(STATE)` (`restartCount` + 1, PCR back to their initial values); *Resume* is
//! `Startup(STATE)` after `Shutdown(STATE)` (`restartCount` + 1, PCR with `TPM_PT_PCR_SAVE`
//! restored). `Startup(STATE)` without a preceding `Shutdown(STATE)` is `TPM_RC_VALUE`.
//!
//! The current algorithm profile has synchronous known-answer tests at `Startup` and
//! `TPM2_SelfTest(YES)`. A failed test latches failure mode until `_TPM_Init`.
//!
//! Hierarchy and NV effects (Part 3, clause 9.3.1; text of v184): on any `Startup` `phEnable` is
//! SET and sessions and sequences are flushed. Reset: `platformAuth` Empty, a new Null seed,
//! `phEnableNV`/`shEnable`/`ehEnable` SET, NV locks released. Restart: `platformAuth` Empty, the
//! three enables SET, NV locks released, the Null seed kept. Resume: the saved platform controls
//! and enables are restored (`phEnable` SET).
//! NOT VERIFIED against TPM 2.0 Library v185: the manufacturer-specific `outData` of
//! `GetTestResult` is empty here.

use crate::capability::ALGORITHMS;
use crate::codes::su;
use crate::context::ContextState;
use crate::hierarchy::HierarchyVolatile;
use crate::marshal::{Reader, Writer};
use crate::response::{Output, Rc};
use crate::state::{SavedState, Tpm};

impl Tpm {
    /// Runs the known-answer tests; failure mode on a failed test.
    fn self_test_now(&mut self) -> Result<(), Rc> {
        if crate::self_test::known_answers() {
            Ok(())
        } else {
            self.failed = true;
            Err(Rc::FAILURE)
        }
    }

    /// Enters failure mode deterministically for command-dispatch tests.
    #[cfg(test)]
    pub(crate) fn force_failure_mode(&mut self) {
        self.failed = true;
    }

    /// `TPM2_Startup` (Part 3, clause 9.3). `TPM_RC_INITIALIZE` if already started (checked by the
    /// dispatcher), `TPM_RC_VALUE` for a bad type or `STATE` without saved state.
    pub(crate) fn cmd_startup(&mut self, p: &mut Reader<'_>) -> Result<Output, Rc> {
        p.set_param(1);
        let startup_type = p.u16()?;
        p.finish()?;
        let startup_hcrtm = self.pre_startup_hcrtm.is_some();
        let restart = match startup_type {
            su::CLEAR => self.saved.is_some(),
            su::STATE => {
                let saved = self.saved.as_ref().ok_or(Rc::VALUE.in_param(1))?;
                // Part 1 v185 §31.3: Resume must match the preceding CLEAR's H-CRTM provenance.
                // The currently implemented command locality is always zero.
                if saved.startup_hcrtm != startup_hcrtm {
                    return Err(Rc::LOCALITY);
                }
                false
            }
            _ => return Err(Rc::VALUE.in_param(1)),
        };
        self.self_test_now()?;
        // Generate all startup nonces before installing the new state.
        let null_seed = if startup_type == su::CLEAR && !restart {
            Some(self.fresh_primary_seed()?)
        } else {
            None
        };
        let mut time_epoch = [0; 16];
        self.drbg.generate(&mut time_epoch).ok_or(Rc::FAILURE)?;
        self.time_epoch = time_epoch;
        let saved = self.saved.take();
        self.sequences.clear();
        self.sessions.clear();
        // Transient objects are flushed by every `Startup` (Part 3, clause 12.2.1).
        self.objects.clear();
        match (startup_type, saved) {
            (su::STATE, Some(s)) => {
                // TPM Resume.
                self.pcrs.startup_from_saved(&s.pcrs, true);
                self.hv = HierarchyVolatile::after_resume(s.hier);
                self.context = s.ctx;
                self.persistent.restart_count = self.persistent.restart_count.wrapping_add(1);
            }
            (_, saved) => {
                self.persistent.nv.startup_clear();
                match (saved, null_seed) {
                    (Some(s), _) => {
                        // TPM Restart.
                        self.pcrs.startup_from_saved(&s.pcrs, false);
                        self.hv = HierarchyVolatile::after_restart(s.hier);
                        self.context = s.ctx;
                        self.context.restart();
                        self.persistent.restart_count =
                            self.persistent.restart_count.wrapping_add(1);
                    }
                    (None, Some(seed)) => {
                        // TPM Reset.
                        self.pcrs.set_initial();
                        self.pcrs.clear_counter();
                        self.hv = HierarchyVolatile::after_reset(seed);
                        self.context = ContextState::default();
                        self.persistent.reset_count = self.persistent.reset_count.wrapping_add(1);
                        self.persistent.total_reset_count =
                            self.persistent.total_reset_count.saturating_add(1);
                        self.persistent.restart_count = 0;
                    }
                    (None, None) => unreachable!("TPM Reset always generates a Null seed"),
                }
            }
        }
        if let Some(digests) = self.pre_startup_hcrtm.take()
            && startup_type == su::CLEAR
        {
            self.pcrs.measure_static_hcrtm(&digests);
        }
        self.startup_hcrtm = startup_hcrtm;
        // `safe`: no Clock value above the current one was reported earlier, which holds only
        // after an orderly shutdown (or for a fresh TPM). The flag is cleared until the next
        // `Shutdown`.
        let was_orderly = self.persistent.orderly;
        self.clock_safe = was_orderly;
        self.persistent.orderly = false;
        self.da_startup(was_orderly);
        self.persistent.nv.orderly_startup(was_orderly);
        self.started = true;
        Ok(Output::default())
    }

    /// `TPM2_Shutdown` (Part 3, clause 9.4): `STATE` saves the PCR with `TPM_PT_PCR_SAVE` and the
    /// update counter; `CLEAR` discards any saved state. Both save the clock (the owner takes it
    /// with `persistent_snapshot`), write the ORDERLY NV counters (`nv.rs`) and mark the shutdown
    /// orderly.
    pub(crate) fn cmd_shutdown(&mut self, p: &mut Reader<'_>) -> Result<Output, Rc> {
        p.set_param(1);
        let shutdown_type = p.u16()?;
        p.finish()?;
        match shutdown_type {
            su::CLEAR => self.saved = None,
            su::STATE => {
                if self.pcr_allocation_pending {
                    return Err(Rc::TYPE.in_param(1));
                }
                self.saved = Some(SavedState {
                    pcrs: self.pcrs.save(),
                    hier: self.hv.clone(),
                    ctx: self.context.clone(),
                    startup_hcrtm: self.startup_hcrtm,
                })
            }
            _ => return Err(Rc::VALUE.in_param(1)),
        }
        self.persistent.nv.orderly_shutdown();
        self.persistent.orderly = true;
        Ok(Output::default())
    }

    /// `TPM2_SelfTest` (Part 3, clause 10.2): `fullTest` YES re-runs the tests, NO has nothing
    /// left to test because `Startup` completed the current algorithm tests.
    pub(crate) fn cmd_self_test(&mut self, p: &mut Reader<'_>) -> Result<Output, Rc> {
        p.set_param(1);
        let full = match p.u8()? {
            0 => false,
            1 => true,
            _ => return Err(Rc::VALUE.in_param(1)),
        };
        p.finish()?;
        if full {
            self.self_test_now()?;
        }
        Ok(Output::default())
    }

    /// `TPM2_IncrementalSelfTest` (Part 3 v185, §10.3). Startup finishes the tests for the
    /// advertised algorithms, so no supported request requires another test.
    pub(crate) fn cmd_incremental_self_test(&self, p: &mut Reader<'_>) -> Result<Output, Rc> {
        const MAX_ALG_LIST_SIZE: u32 = 64;
        p.set_param(1);
        let count = p.u32()?;
        if count > MAX_ALG_LIST_SIZE {
            return Err(Rc::SIZE.in_param(1));
        }
        let algorithms = p.bytes(count as usize * 2)?;
        p.finish()?;
        for entry in algorithms.as_chunks::<2>().0 {
            let algorithm = u16::from_be_bytes([entry[0], entry[1]]);
            if !ALGORITHMS
                .iter()
                .any(|(supported, _)| *supported == algorithm)
            {
                return Err(Rc::VALUE.in_param(1));
            }
        }
        let mut w = Writer::new();
        w.u32(0);
        Ok(Output {
            handles: Vec::new(),
            params: w.finish(),
        })
    }

    /// `TPM2_GetTestResult` (Part 3, clause 10.4): empty `outData` and the test result.
    pub(crate) fn cmd_get_test_result(&mut self, p: &mut Reader<'_>) -> Result<Output, Rc> {
        p.finish()?;
        let mut w = Writer::new();
        w.tpm2b(&[]);
        w.u32(if self.failed {
            Rc::FAILURE.0
        } else {
            Rc::SUCCESS.0
        });
        Ok(Output {
            handles: Vec::new(),
            params: w.finish(),
        })
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn known_answer_tests_pass() {
        assert!(crate::self_test::known_answers());
    }
}
