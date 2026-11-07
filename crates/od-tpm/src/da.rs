//! Dictionary attack (DA) protection (slice C1 of phase B, S7-23): Part 1 v184, clause 17.8, and
//! Part 3 v184, clauses 5.6 (authorization checks) and 25 (`TPM2_DictionaryAttackLockReset`,
//! `TPM2_DictionaryAttackParameters`).
//!
//! **What is DA protected** (Part 1, 17.8.1; Part 3, 5.6 and 25.1): the authValue of an object
//! whose `noDA` is CLEAR, of an NV index whose `TPMA_NV_NO_DA` is CLEAR, and `lockoutAuth`. The
//! authValue of any other permanent entity (platform, owner, endorsement, Null), of a PCR and of a
//! sequence object is not. Objects of the platform hierarchy and NV indices created with
//! `TPMA_NV_PLATFORMCREATE` are never locked out and their failures are not counted (Part 3, 25.1
//! "never locked out"; Part 1, 11.7 "generally not applied"); the same treatment of the failures
//! is NOT VERIFIED. This follows the text and is narrower than "all hierarchy authorizations".
//!
//! **State.** Persistent (in the snapshot): `failedTries`, `maxTries`, `recoveryTime` (seconds),
//! `lockoutRecovery` (seconds) and the flag "`lockoutAuth` enabled". Volatile: the time of the
//! last counted failure (self-healing) and of the last `lockoutAuth` failure; both use the
//! `Time` of the TPM (continuous power), so a power cycle restarts them.
//!
//! **Behaviour.**
//! - Use of a DA-protected authValue (password, HMAC, or the key of a bound session) while
//!   `failedTries >= maxTries` is `TPM_RC_LOCKOUT`, before the authValue is looked at.
//! - A wrong authValue of a DA-protected entity is `TPM_RC_AUTH_FAIL` and increments `failedTries`;
//!   of an exempt entity `TPM_RC_BAD_AUTH`, with no change. `recoveryTime` 0 disables DA: failures
//!   are `TPM_RC_BAD_AUTH`, nothing counts and there is no lockout. `maxTries` 0 locks out all
//!   DA-protected entities (`failedTries >= 0` always holds).
//! - A wrong `lockoutAuth` is `TPM_RC_AUTH_FAIL` and disables `lockoutAuth` regardless of
//!   `failedTries` (clause 17.8.5): further use is `TPM_RC_LOCKOUT` until `lockoutRecovery`
//!   seconds of `Time` have passed, or, when `lockoutRecovery` is 0, until the next `Startup`;
//!   `TPM2_DictionaryAttackLockReset` (when `lockoutAuth` is enabled) enables it again. A
//!   `lockoutAuth` failure does not increment `failedTries` (NOT VERIFIED; the text says the
//!   special state arises "regardless of" it).
//! - Self-healing: every `recoveryTime` seconds without a counted failure, `failedTries` drops by
//!   one, not below zero (clause 17.8.2). `TPM2_DictionaryAttackLockReset` sets it to zero.
//!   `TPM2_Clear` sets it to zero, restores the default parameters (clause 17.8.2: when a new
//!   owner is installed; NOT VERIFIED that `Clear` counts as installing one) and enables
//!   `lockoutAuth`.
//!
//! **Defaults** (implementation choice, NOT VERIFIED: "the default value specified in the relevant
//! platform-specific specification"; PTP 1.07 states none): `maxTries` 32, `recoveryTime` 600 s,
//! `lockoutRecovery` 86400 s. The guest sets its own values with
//! `TPM2_DictionaryAttackParameters`.
//!
//! Policy authorization (Part 3, 5.6, `TPM_RC_POLICY`) does not exist yet (slice C2).
//! NOT VERIFIED against TPM 2.0 Library v185/v184: the session index modifier on `TPM_RC_AUTH_FAIL`
//! and `TPM_RC_LOCKOUT` (a warning code takes none), `TPM_RC_BAD_AUTH` for a failure while DA is
//! disabled, the order "lockout check before the authValue check" relative to other handle checks
//! (the text lists the lockout check as step 3 of the authorization checks, before the HMAC and
//! password checks), and the counting of a failure of a bound session.

use crate::codes::{handle as h, object_attr};
use crate::marshal::Reader;
use crate::nv::attr as nv_attr;
use crate::response::{Output, Rc};
use crate::state::Tpm;

/// Default `maxTries`.
pub(crate) const DEFAULT_MAX_TRIES: u32 = 32;
/// Default `recoveryTime`, seconds.
pub(crate) const DEFAULT_RECOVERY_TIME: u32 = 600;
/// Default `lockoutRecovery`, seconds.
pub(crate) const DEFAULT_LOCKOUT_RECOVERY: u32 = 86_400;

/// Persistent DA state (goes into the snapshot).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct DaPersistent {
    pub failed_tries: u32,
    pub max_tries: u32,
    pub recovery_time: u32,
    pub lockout_recovery: u32,
    /// `lockoutAuth` may be used.
    pub lockout_auth_enabled: bool,
}

impl Default for DaPersistent {
    fn default() -> Self {
        DaPersistent {
            failed_tries: 0,
            max_tries: DEFAULT_MAX_TRIES,
            recovery_time: DEFAULT_RECOVERY_TIME,
            lockout_recovery: DEFAULT_LOCKOUT_RECOVERY,
            lockout_auth_enabled: true,
        }
    }
}

/// Volatile DA state: `Time` (ms) of the last counted failure or decrement, and of the last
/// `lockoutAuth` failure.
#[derive(Clone, Copy, Default)]
pub(crate) struct DaVolatile {
    pub heal_epoch_ms: u64,
    pub lockout_fail_ms: u64,
}

/// Whether and how an authorization is DA protected.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum DaClass {
    /// Not DA protected.
    Exempt,
    /// An object or an NV index with DA protection.
    Protected,
    /// `lockoutAuth`.
    Lockout,
}

impl Tpm {
    /// DA is on while `recoveryTime` is not zero (Part 3, 25.3.1).
    fn da_active(&self) -> bool {
        self.persistent.da.recovery_time != 0
    }

    /// `TPMA_PERMANENT.inLockout`: DA is on and `failedTries` has reached `maxTries`.
    pub(crate) fn da_in_lockout(&self) -> bool {
        let da = &self.persistent.da;
        self.da_active() && da.failed_tries >= da.max_tries
    }

    /// Time-driven DA state, applied at the start of every command: self-healing of `failedTries`
    /// and the end of the `lockoutAuth` lockout. The text lets the TPM lower `failedTries` at any
    /// time (Part 3, clause 5.6).
    pub(crate) fn da_tick(&mut self) {
        let now = self.power_ms;
        let da = &mut self.persistent.da;
        if da.recovery_time != 0 && da.failed_tries > 0 {
            let step = u64::from(da.recovery_time) * 1000;
            let steps = now.saturating_sub(self.da_v.heal_epoch_ms) / step;
            if steps > 0 {
                da.failed_tries = da
                    .failed_tries
                    .saturating_sub(u32::try_from(steps).unwrap_or(u32::MAX));
                self.da_v.heal_epoch_ms = if da.failed_tries == 0 {
                    now
                } else {
                    self.da_v.heal_epoch_ms.saturating_add(steps * step)
                };
            }
        }
        if !da.lockout_auth_enabled && da.lockout_recovery != 0 {
            let until = self
                .da_v
                .lockout_fail_ms
                .saturating_add(u64::from(da.lockout_recovery) * 1000);
            if now >= until {
                da.lockout_auth_enabled = true;
            }
        }
    }

    /// `Startup`: `lockoutAuth` is enabled again when `lockoutRecovery` is zero (Part 3, clause
    /// 9.3.1); otherwise a disabled `lockoutAuth` waits `lockoutRecovery` seconds of the new power
    /// cycle (implementation choice: the time spent powered off is not credited).
    ///
    /// After a disorderly shutdown (`was_orderly` false: no `Shutdown` before the state was saved)
    /// `failedTries` grows by one unless it is already `maxTries`, so that a power loss cannot be
    /// used to undo a failed attempt.
    // NOT VERIFIED against TPM 2.0 Library v184 17.8.6 (Part 1: the increment after a disorderly
    // shutdown); see docs/specs/VTPM.md, DA.
    pub(crate) fn da_startup(&mut self, was_orderly: bool) {
        let da = &mut self.persistent.da;
        if !was_orderly && da.failed_tries < da.max_tries {
            da.failed_tries += 1;
        }
        if da.lockout_recovery == 0 {
            da.lockout_auth_enabled = true;
        }
        self.da_v.lockout_fail_ms = self.power_ms;
        self.da_v.heal_epoch_ms = self.power_ms;
    }

    /// `TPM2_Clear`: no failures, default parameters, `lockoutAuth` enabled (module comment).
    pub(crate) fn da_clear(&mut self) {
        self.persistent.da = DaPersistent::default();
        self.da_v.heal_epoch_ms = self.power_ms;
    }

    /// DA class of the authorization of the entity behind a handle (not a PCR, not `TPM_RH_NULL`:
    /// the dispatcher decides those).
    pub(crate) fn da_class_of(&self, handle: u32) -> DaClass {
        match h::handle_type(handle) {
            h::HT_PERMANENT => {
                if handle == h::RH_LOCKOUT {
                    DaClass::Lockout
                } else {
                    DaClass::Exempt
                }
            }
            h::HT_NV_INDEX => match self.persistent.nv.get(handle) {
                Some(i) if i.attributes & (nv_attr::NO_DA | nv_attr::PLATFORMCREATE) == 0 => {
                    DaClass::Protected
                }
                _ => DaClass::Exempt,
            },
            h::HT_TRANSIENT | h::HT_PERSISTENT => match self.object(handle) {
                Some(o) if !o.has(object_attr::NO_DA) && o.hierarchy != h::RH_PLATFORM => {
                    DaClass::Protected
                }
                _ => DaClass::Exempt,
            },
            _ => DaClass::Exempt,
        }
    }

    /// The lockout check that precedes an authorization (Part 3, clause 5.6, step 3): a
    /// DA-protected authValue is `TPM_RC_LOCKOUT` in lockout; `lockoutAuth` while disabled too.
    pub(crate) fn da_check(&self, class: DaClass) -> Result<(), Rc> {
        match class {
            DaClass::Exempt => Ok(()),
            DaClass::Protected if self.da_in_lockout() => Err(Rc::LOCKOUT),
            DaClass::Protected => Ok(()),
            DaClass::Lockout if !self.persistent.da.lockout_auth_enabled => Err(Rc::LOCKOUT),
            DaClass::Lockout => Ok(()),
        }
    }

    /// Records a wrong authValue of an entity of `class` and returns the response code for
    /// session number `session` (one-based).
    pub(crate) fn da_auth_failed(&mut self, class: DaClass, session: u8) -> Rc {
        let now = self.power_ms;
        match class {
            DaClass::Exempt => Rc::BAD_AUTH.in_session(session),
            DaClass::Lockout => {
                self.persistent.da.lockout_auth_enabled = false;
                self.da_v.lockout_fail_ms = now;
                Rc::AUTH_FAIL.in_session(session)
            }
            DaClass::Protected => {
                if !self.da_active() {
                    return Rc::BAD_AUTH.in_session(session);
                }
                let da = &mut self.persistent.da;
                da.failed_tries = da.failed_tries.saturating_add(1);
                self.da_v.heal_epoch_ms = now;
                Rc::AUTH_FAIL.in_session(session)
            }
        }
    }

    /// `TPM2_DictionaryAttackLockReset` (Part 3, clause 25.2): `failedTries` to zero (and
    /// `lockoutAuth` enabled: the command ran, so it was). Authorized by `lockoutAuth`.
    pub(crate) fn cmd_dictionary_attack_lock_reset(
        &mut self,
        p: &mut Reader<'_>,
    ) -> Result<Output, Rc> {
        p.finish()?;
        self.persistent.da.failed_tries = 0;
        self.persistent.da.lockout_auth_enabled = true;
        self.da_v.heal_epoch_ms = self.power_ms;
        Ok(Output::default())
    }

    /// `TPM2_DictionaryAttackParameters` (Part 3, clause 25.3): `newMaxTries`, `newRecoveryTime`,
    /// `lockoutRecovery`. Authorized by `lockoutAuth`; `failedTries` is not changed.
    pub(crate) fn cmd_dictionary_attack_parameters(
        &mut self,
        p: &mut Reader<'_>,
    ) -> Result<Output, Rc> {
        p.set_param(1);
        let max_tries = p.u32()?;
        p.set_param(2);
        let recovery_time = p.u32()?;
        p.set_param(3);
        let lockout_recovery = p.u32()?;
        p.finish()?;
        let da = &mut self.persistent.da;
        da.max_tries = max_tries;
        da.recovery_time = recovery_time;
        da.lockout_recovery = lockout_recovery;
        self.da_v.heal_epoch_ms = self.power_ms;
        Ok(Output::default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_the_recorded_choice() {
        let d = DaPersistent::default();
        assert_eq!(
            (
                d.failed_tries,
                d.max_tries,
                d.recovery_time,
                d.lockout_recovery
            ),
            (0, 32, 600, 86_400)
        );
        assert!(d.lockout_auth_enabled);
    }
}
