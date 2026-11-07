//! Hierarchies (Part 1 v184, clause 11 and 12.4): Platform, Owner (Storage), Endorsement and Null
//! with their seeds, authorization values and enables, plus the Lockout authorization.
//! Commands: `TPM2_HierarchyControl`, `TPM2_HierarchyChangeAuth`, `TPM2_Clear`,
//! `TPM2_ClearControl`.
//!
//! State split (Part 1 clauses 11.3-11.5, Part 3 clause 9.3.1 "Startup", Part 2 Tables 42, 43):
//! - persistent: `ownerAuth`, `endorsementAuth`, `lockoutAuth`, `disableClear`, the three
//!   "AuthSet" flags of `TPMA_PERMANENT`; the seeds (platform, owner/storage, endorsement) live
//!   in the phase A persistent block;
//! - volatile: `platformAuth` (EmptyAuth on every `Startup(CLEAR)`), the enables `phEnable`
//!   (SET on any `Startup`), `shEnable`, `ehEnable`, `phEnableNV` (SET on `Startup(CLEAR)`,
//!   preserved by `Startup(STATE)`), and the Null seed (new on every TPM Reset). `Shutdown(STATE)`
//!   saves this block, `Startup(STATE)` restores it with `phEnable` SET.
//!
//! `authPolicy` values (`platformPolicy`, `ownerPolicy`, ...) and `TPM2_SetPrimaryPolicy` are not
//! implemented: policy sessions are not (slice A), so every policy is the Empty Buffer. The
//! hierarchy proof values (`shProof`, `ehProof`, `phProof`, `nullProof`) belong to tickets and
//! primary keys; slice B derives them from the seeds (`object_cmd.rs`), so `TPM2_Clear` changes
//! the storage seed and with it `shProof` and `ehProof`, but not the endorsement seed (EPS): the
//! endorsement primary key does not change (`TPM2_ChangeEPS` is not implemented).
//!
//! Lockout authorization exists as an authorization value only: there is no dictionary-attack
//! logic yet (slice C), so a wrong `lockoutAuth` is `TPM_RC_BAD_AUTH`, never a lockout.
//!
//! NOT VERIFIED against TPM 2.0 Library v185/v184 (the open text names the rule but not the
//! response code): `TPM_RC_AUTH_TYPE` when an authorization handle may not change the requested
//! enable (`TPM2_HierarchyControl`: owner/endorsement SET an enable, owner/endorsement/platform
//! change another hierarchy's enable, platform SET `phEnable`); `TPM_RC_AUTH_FAIL` when
//! `lockoutAuth` tries to CLEAR `disableClear`; `TPM_RC_VALUE` with the handle number for a handle
//! outside the types of the command (the `#TPM_RC_VALUE` note of Part 2, Tables 61-69 names this
//! code for the type check); `TPM_RC_HIERARCHY` for an authorization with a disabled hierarchy
//! (Part 3, clause 24.3 names it for `TPM2_SetPrimaryPolicy`; the same rule is stated for every
//! use in Part 1, clause 11.2) carries the handle number here.

use zeroize::Zeroizing;

use crate::codes::handle as h;
use crate::dispatch::Invocation;
use crate::hash::AuthValue;
use crate::marshal::Reader;
use crate::response::{Output, Rc};
use crate::state::Tpm;

/// Persistent part of the hierarchy state (survives power cycles).
pub(crate) struct HierarchyPersistent {
    pub owner_auth: AuthValue,
    pub endorsement_auth: AuthValue,
    pub lockout_auth: AuthValue,
    /// `TPMA_PERMANENT.ownerAuthSet`: `TPM2_HierarchyChangeAuth` ran since the last `TPM2_Clear`.
    pub owner_auth_set: bool,
    pub endorsement_auth_set: bool,
    pub lockout_auth_set: bool,
    /// `TPMA_PERMANENT.disableClear`.
    pub disable_clear: bool,
}

impl Default for HierarchyPersistent {
    fn default() -> Self {
        HierarchyPersistent {
            owner_auth: AuthValue::empty(),
            endorsement_auth: AuthValue::empty(),
            lockout_auth: AuthValue::empty(),
            owner_auth_set: false,
            endorsement_auth_set: false,
            lockout_auth_set: false,
            disable_clear: false,
        }
    }
}

/// Volatile part: reset by `_TPM_Init`, partly preserved by `Shutdown(STATE)`.
#[derive(Clone)]
pub(crate) struct HierarchyVolatile {
    pub platform_auth: AuthValue,
    pub ph_enable: bool,
    pub ph_enable_nv: bool,
    pub sh_enable: bool,
    pub eh_enable: bool,
    /// Null seed: random on every TPM Reset (Part 1, clause 12.4.5); all zero before the first
    /// `Startup`, when no command can use it yet.
    pub null_seed: Zeroizing<[u8; 32]>,
}

impl Default for HierarchyVolatile {
    fn default() -> Self {
        HierarchyVolatile {
            platform_auth: AuthValue::empty(),
            ph_enable: true,
            ph_enable_nv: true,
            sh_enable: true,
            eh_enable: true,
            null_seed: Zeroizing::new([0u8; 32]),
        }
    }
}

impl HierarchyVolatile {
    /// State after a TPM Reset: everything at its default, a fresh Null seed.
    pub(crate) fn after_reset(null_seed: [u8; 32]) -> HierarchyVolatile {
        HierarchyVolatile {
            null_seed: Zeroizing::new(null_seed),
            ..HierarchyVolatile::default()
        }
    }

    /// State after a TPM Restart from the state saved by `Shutdown(STATE)`: the platform controls
    /// and the enables return to their defaults, the Null seed is kept (Part 3, clause 9.3.1).
    pub(crate) fn after_restart(saved: HierarchyVolatile) -> HierarchyVolatile {
        HierarchyVolatile {
            null_seed: saved.null_seed.clone(),
            ..HierarchyVolatile::default()
        }
    }

    /// State after a TPM Resume: everything preserved except `phEnable`, which is SET.
    pub(crate) fn after_resume(mut saved: HierarchyVolatile) -> HierarchyVolatile {
        saved.ph_enable = true;
        saved
    }
}

/// `true` for the handles that stand for a hierarchy authorization (`TPM_RH_OWNER`,
/// `TPM_RH_PLATFORM`, `TPM_RH_ENDORSEMENT`, `TPM_RH_LOCKOUT`).
pub(crate) fn is_authorization_handle(handle: u32) -> bool {
    matches!(
        handle,
        h::RH_OWNER | h::RH_PLATFORM | h::RH_ENDORSEMENT | h::RH_LOCKOUT
    )
}

impl Tpm {
    /// Authorization value behind a permanent handle regardless of the hierarchy enables (used
    /// for response HMACs, which are computed after the command may have disabled the hierarchy).
    /// `None` for a handle that is not a hierarchy authorization or `TPM_RH_NULL`.
    pub(crate) fn permanent_auth(&self, handle: u32) -> Option<AuthValue> {
        match handle {
            h::RH_PLATFORM => Some(self.hv.platform_auth.clone()),
            h::RH_OWNER => Some(self.persistent.hier.owner_auth.clone()),
            h::RH_ENDORSEMENT => Some(self.persistent.hier.endorsement_auth.clone()),
            h::RH_LOCKOUT => Some(self.persistent.hier.lockout_auth.clone()),
            h::RH_NULL => Some(AuthValue::empty()),
            _ => None,
        }
    }

    /// Whether the authorization of a permanent handle may be used: the enable of its hierarchy
    /// is SET (Part 1, clause 11.2: with the enable CLEAR neither `authValue` nor `authPolicy` can
    /// authorize anything). The Null hierarchy and the Lockout authorization have no enable.
    pub(crate) fn permanent_enabled(&self, handle: u32) -> bool {
        match handle {
            h::RH_PLATFORM => self.hv.ph_enable,
            h::RH_OWNER => self.hv.sh_enable,
            h::RH_ENDORSEMENT => self.hv.eh_enable,
            _ => true,
        }
    }

    /// `TPMA_STARTUP_CLEAR` (Part 2, Table 43); `readOnly` is never SET.
    pub(crate) fn startup_clear_attributes(&self) -> u32 {
        u32::from(self.hv.ph_enable)
            | (u32::from(self.hv.sh_enable) << 1)
            | (u32::from(self.hv.eh_enable) << 2)
            | (u32::from(self.hv.ph_enable_nv) << 3)
    }

    /// `TPMA_PERMANENT` (Part 2, Table 42): `inLockout` is never SET (no DA yet), `tpmGeneratedEPS`
    /// always (the endorsement seed comes from the TPM's DRBG).
    pub(crate) fn permanent_attributes(&self) -> u32 {
        let hp = &self.persistent.hier;
        u32::from(hp.owner_auth_set)
            | (u32::from(hp.endorsement_auth_set) << 1)
            | (u32::from(hp.lockout_auth_set) << 2)
            | (u32::from(hp.disable_clear) << 8)
            | (1 << 10)
    }

    /// `TPM2_HierarchyControl` (Part 3, clause 24.2). `enable` selects the flag, `state` its new
    /// value. Platform authorization may CLEAR `phEnable` (never SET it), SET or CLEAR
    /// `phEnableNV`, `shEnable` and `ehEnable`; owner (endorsement) authorization may only CLEAR
    /// `shEnable` (`ehEnable`). Disabling a hierarchy would flush its transient objects and
    /// disable its persistent ones: none exist in slice A; NV indices follow the flags
    /// dynamically (`nv.rs`).
    pub(crate) fn cmd_hierarchy_control(
        &mut self,
        inv: &Invocation,
        p: &mut Reader<'_>,
    ) -> Result<Output, Rc> {
        p.set_param(1);
        let enable = p.u32()?;
        p.set_param(2);
        let state = yes_no(p)?;
        p.finish()?;
        let auth = inv.handles[0];
        match (auth, enable) {
            (h::RH_PLATFORM, h::RH_PLATFORM) => {
                // `phEnable` may not be SET with this command.
                if state {
                    // NOT VERIFIED: the parameter number (`enable`) of this `TPM_RC_AUTH_TYPE` against the reference
                    // implementation is not checked (VTPM.md, item 12); the code carries no parameter number.
                    return Err(Rc::AUTH_TYPE);
                }
                self.hv.ph_enable = false;
            }
            (h::RH_PLATFORM, h::RH_PLATFORM_NV) => self.hv.ph_enable_nv = state,
            (h::RH_PLATFORM | h::RH_OWNER, h::RH_OWNER) => {
                if auth == h::RH_OWNER && state {
                    // NOT VERIFIED: the parameter number (`enable`) of this `TPM_RC_AUTH_TYPE` against the reference
                    // implementation is not checked (VTPM.md, item 12); the code carries no parameter number.
                    return Err(Rc::AUTH_TYPE);
                }
                self.hv.sh_enable = state;
            }
            (h::RH_PLATFORM | h::RH_ENDORSEMENT, h::RH_ENDORSEMENT) => {
                if auth == h::RH_ENDORSEMENT && state {
                    // NOT VERIFIED: the parameter number (`enable`) of this `TPM_RC_AUTH_TYPE` against the reference
                    // implementation is not checked (VTPM.md, item 12); the code carries no parameter number.
                    return Err(Rc::AUTH_TYPE);
                }
                self.hv.eh_enable = state;
            }
            // `TPM_RH_NULL` is accepted by the `TPMI_RH_ENABLES` type but is no enable.
            (_, h::RH_OWNER | h::RH_PLATFORM | h::RH_ENDORSEMENT | h::RH_PLATFORM_NV) => {
                // NOT VERIFIED: the parameter number (`enable`) of this `TPM_RC_AUTH_TYPE` against the reference
                // implementation is not checked (VTPM.md, item 12); the code carries no parameter number.
                return Err(Rc::AUTH_TYPE);
            }
            _ => return Err(Rc::VALUE.in_param(1)),
        }
        // Disabling a hierarchy flushes its transient objects (Part 3, clause 24.2.1); its
        // persistent objects stay but cannot be used while it is disabled (`object_cmd.rs`).
        if !state && matches!(enable, h::RH_OWNER | h::RH_ENDORSEMENT | h::RH_PLATFORM) {
            self.objects.flush_hierarchy(enable);
        }
        Ok(Output::default())
    }

    /// `TPM2_HierarchyChangeAuth` (Part 3, clause 24.8): the new value is at most one digest
    /// (32 octets, SHA-256 is the context-integrity hash) and is stored without trailing zeros.
    /// The response HMAC uses the new value (the dispatcher recomputes it after the command).
    pub(crate) fn cmd_hierarchy_change_auth(
        &mut self,
        inv: &Invocation,
        p: &mut Reader<'_>,
    ) -> Result<Output, Rc> {
        p.set_param(1);
        let new_auth = p.tpm2b(crate::marshal::limits::MAX_DIGEST_SIZE)?;
        p.finish()?;
        let value = AuthValue::from_slice_trimmed(new_auth).ok_or(Rc::SIZE.in_param(1))?;
        let hp = &mut self.persistent.hier;
        match inv.handles[0] {
            h::RH_PLATFORM => self.hv.platform_auth = value,
            h::RH_OWNER => {
                hp.owner_auth = value;
                hp.owner_auth_set = true;
            }
            h::RH_ENDORSEMENT => {
                hp.endorsement_auth = value;
                hp.endorsement_auth_set = true;
            }
            h::RH_LOCKOUT => {
                hp.lockout_auth = value;
                hp.lockout_auth_set = true;
            }
            // Excluded by the handle check of the dispatcher.
            _ => return Err(Rc::VALUE.in_handle(1)),
        }
        Ok(Output::default())
    }

    /// `TPM2_Clear` (Part 3, clause 24.6): removes the Owner's context. `TPM_RC_DISABLED` after
    /// `TPM2_ClearControl`. Effects implemented: new storage seed, `shEnable` and `ehEnable` SET,
    /// owner, endorsement and lockout authorization values Empty, NV indices without
    /// `TPMA_NV_PLATFORMCREATE` deleted, `Clock` zero, `resetCount` and `restartCount` zero,
    /// `safe` YES, `pcrUpdateCounter` incremented, objects of the Storage and Endorsement
    /// hierarchies flushed, `shProof` and `ehProof` changed (derived from the new seed). Not
    /// applicable yet: empty policies (no policies).
    pub(crate) fn cmd_clear(&mut self, p: &mut Reader<'_>) -> Result<Output, Rc> {
        p.finish()?;
        if self.persistent.hier.disable_clear {
            return Err(Rc::DISABLED);
        }
        // The only fallible step goes first so that a failure leaves the TPM unchanged.
        let new_seed = self.drbg.generate_array().ok_or(Rc::FAILURE)?;
        self.persistent.owner_seed = Zeroizing::new(new_seed);
        self.hv.sh_enable = true;
        self.hv.eh_enable = true;
        let hp = &mut self.persistent.hier;
        hp.owner_auth = AuthValue::empty();
        hp.endorsement_auth = AuthValue::empty();
        hp.lockout_auth = AuthValue::empty();
        hp.owner_auth_set = false;
        hp.endorsement_auth_set = false;
        hp.lockout_auth_set = false;
        self.persistent.nv.remove_owner_created();
        // Flush the resident objects (transient and persistent) of the Storage and Endorsement
        // hierarchies; the proofs follow the new storage seed (`object_cmd.rs`, `proof`).
        for hierarchy in [h::RH_OWNER, h::RH_ENDORSEMENT] {
            self.objects.flush_hierarchy(hierarchy);
            self.persistent.objects.flush_hierarchy(hierarchy);
        }
        self.clock_epoch_ms = self.power_ms;
        self.persistent.clock_base_ms = 0;
        self.persistent.reset_count = 0;
        self.persistent.restart_count = 0;
        self.clock_safe = true;
        self.pcrs.increment_update_counter();
        Ok(Output::default())
    }

    /// `TPM2_ClearControl` (Part 3, clause 24.7): sets or clears `disableClear`. Platform
    /// authorization may do both, lockout authorization only SET it.
    pub(crate) fn cmd_clear_control(
        &mut self,
        inv: &Invocation,
        p: &mut Reader<'_>,
    ) -> Result<Output, Rc> {
        p.set_param(1);
        let disable = yes_no(p)?;
        p.finish()?;
        if inv.handles[0] == h::RH_LOCKOUT && !disable {
            return Err(Rc::AUTH_FAIL.in_handle(1));
        }
        self.persistent.hier.disable_clear = disable;
        Ok(Output::default())
    }
}

/// `TPMI_YES_NO`: 0 or 1, otherwise `TPM_RC_VALUE` for the current parameter.
fn yes_no(p: &mut Reader<'_>) -> Result<bool, Rc> {
    match p.u8()? {
        0 => Ok(false),
        1 => Ok(true),
        _ => Err(p.err(Rc::VALUE)),
    }
}
