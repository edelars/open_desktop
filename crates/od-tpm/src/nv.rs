//! NV indices, base (Part 3 v184, clause 31; Part 2 clause 13): `TPM2_NV_DefineSpace`,
//! `TPM2_NV_UndefineSpace`, `TPM2_NV_ReadPublic`, `TPM2_NV_Write`, `TPM2_NV_Read`, and the two
//! lock commands `TPM2_NV_WriteLock` and `TPM2_NV_ReadLock` (not in the plan's list, added so that
//! `TPMA_NV_WRITEDEFINE`, `TPMA_NV_WRITE_STCLEAR` and `TPMA_NV_READ_STCLEAR` can take effect and
//! `TPMA_NV_WRITELOCKED`/`READLOCKED` are reachable).
//!
//! Supported index types: `TPM_NT_ORDINARY` (opaque data written by `TPM2_NV_Write`) and, since
//! slice C3, `TPM_NT_COUNTER` (8 octets, `TPM2_NV_Increment`), `TPM_NT_BITS` (8 octets,
//! `TPM2_NV_SetBits`) and `TPM_NT_EXTEND` (a SHA-256 digest, `TPM2_NV_Extend`). PIN indices
//! (`TPM_NT_PIN_FAIL`, `TPM_NT_PIN_PASS`) and reserved types are `TPM_RC_ATTRIBUTES` (Part 3,
//! clause 31.3.1: "if the implementation does not support ... the TPM shall return
//! TPM_RC_ATTRIBUTES"; Part 2, clause 13.2: reserved `TPM_NT` is `TPM_RC_ATTRIBUTES`).
//! Attributes that need machinery the implementation lacks are refused the same way:
//! `GLOBALLOCK` (`TPM2_NV_GlobalWriteLock`), `ORDERLY` and `CLEAR_STCLEAR` (PTP 1.07, Table 12).
//! `TPMA_NV_NO_DA` exempts the index's
//! authorization value from dictionary-attack protection.
//!
//! **Sizes by type** (Part 3, clause 31.3.1): a counter or bit field has `dataSize` 8, an extend
//! index the digest size of its `nameAlg` (only SHA-256 is supported: 32); any other size is
//! `TPM_RC_SIZE`. `TPM2_NV_Write` of a counter, bit field or extend index is `TPM_RC_ATTRIBUTES`
//! (clause 31.7.1); `TPM2_NV_Read` reads any type.
//!
//! **Counters** (clause 31.2): the first increment of a counter starts from the largest value
//! any counter has had (kept in [`NvStore`], persistent: snapshot v3) and adds one, so a counter
//! that is deleted and defined again never goes back ("greater than any value a counter Index ...
//! has had", clause 31.2; the text allows tracking the largest count of deleted and defined
//! counters). `TPM2_NV_Increment` on a counter that is at `u64::MAX` is `TPM_RC_NV_RANGE` and
//! changes nothing.
//!
//! Limits (this implementation; PTP 1.07 minimum: 68 indices, 11026 octets, Table 2 and clause
//! 4.6.2): [`MAX_NV_INDICES`] indices, [`NV_POOL_SIZE`] octets of data in total,
//! [`MAX_NV_INDEX_SIZE`] octets per index (`TPM_PT_NV_INDEX_MAX`, PTP minimum 8500) and
//! [`NV_BUFFER_MAX`] octets per read or write (`TPM_PT_NV_BUFFER_MAX`, PTP minimum 512).
//!
//! Persistence: the NV store (indices and the largest counter value seen) is part of
//! [`crate::state::Persistent`], survives `_TPM_Init` and is in the snapshot (the type of an index
//! is in its `TPMA_NV` attributes; `snapshot.rs` checks that attributes, size and data of every
//! restored index are consistent with its type).
//!
//! Access control (Part 3, clause 31.1; Part 2, Table 43): an index created with
//! `TPMA_NV_PLATFORMCREATE` is accessible only while `phEnableNV` is SET, one created without it
//! only while `shEnable` is SET (`TPM_RC_HANDLE` otherwise, Table 43); the platform cannot define
//! an index while `phEnableNV` is CLEAR (`TPM_RC_HIERARCHY`, Table 43). Authorization comes from
//! `TPM_RH_PLATFORM` (`PPREAD`/`PPWRITE`), `TPM_RH_OWNER` (`OWNERREAD`/`OWNERWRITE`) or the index
//! itself (`AUTHREAD`/`AUTHWRITE` for password/HMAC and `POLICYREAD`/`POLICYWRITE` for policy).
//! An index handle other than `nvIndex` is `TPM_RC_NV_AUTHORIZATION`.
//! The Name of an index is `nameAlg || H(TPMS_NV_PUBLIC)` and changes when `WRITTEN`/locks change
//! (Part 1, clause 14).
//!
//! Order of checks for a command with a session: handle validity and access control (including
//! `TPM_RC_NV_UNINITIALIZED` for a read of an unwritten index, which must precede the authValue
//! check) -> authorization -> lock state and range (Part 3: "authorization sessions are checked
//! before the lock state", clauses 31.7.1 and 31.13.1).
//!
//! NOT VERIFIED against TPM 2.0 Library v185/v184 (the open text states the rule, not the code):
//! `TPM_RC_NV_RANGE` for the overflow of a counter (the text does not say what a counter at the
//! maximum does); the order of the type check and the lock check in `TPM2_NV_Write` (lock first,
//! as the clause lists them) versus `Increment`/`Extend`/`SetBits` (type first); the strict
//! restore-time consistency rules of the snapshot (no index with data while `WRITTEN` is CLEAR);
//! `TPM_RC_AUTH_TYPE` when Owner or Platform access is not enabled by the index attributes;
//! `TPM_RC_RESERVED_BITS` for reserved `TPMA_NV` bits in `TPM2_NV_DefineSpace`; `TPM_RC_ATTRIBUTES`
//! for the unsupported attributes and index types listed above; `TPM_RC_SIZE` for a `publicInfo`
//! whose `size` differs from the marshaled structure and for a non-empty `authPolicy` that is not
//! a digest; `TPM_RC_VALUE` for a read `size` above [`NV_BUFFER_MAX`]; `TPM_RC_NV_RANGE` for an
//! `offset` beyond the data size; `TPM_RC_NV_LOCKED` for a locked read or write (the code is
//! named in clauses 31.7.1 and 31.13.1); `TPM_RC_HANDLE` with the handle number for an undefined
//! or inaccessible index (named in clause 31.4.1 and Table 43; the index modifier is ours).

use sha2::{Digest, Sha256};
use zeroize::Zeroizing;

use crate::codes::{alg, cc, handle as h};
use crate::dispatch::{CommandHandles, Invocation};
use crate::hash::AuthValue;
use crate::marshal::{Reader, Writer, limits};
use crate::response::{Output, Rc};
use crate::session::SessionKind;
use crate::state::Tpm;

/// Defined NV indices at once.
pub const MAX_NV_INDICES: usize = 128;
/// Total NV data size of all indices, octets.
pub const NV_POOL_SIZE: usize = 16384;
/// Largest data area of one index (`TPM_PT_NV_INDEX_MAX`).
pub const MAX_NV_INDEX_SIZE: usize = 8500;
/// Largest data of one `TPM2_NV_Read`/`TPM2_NV_Write` (`TPM_PT_NV_BUFFER_MAX`).
pub const NV_BUFFER_MAX: usize = 512;

/// `TPMA_NV` bits (Part 2 v184, Table 233).
pub mod attr {
    pub const PPWRITE: u32 = 1 << 0;
    pub const OWNERWRITE: u32 = 1 << 1;
    pub const AUTHWRITE: u32 = 1 << 2;
    pub const POLICYWRITE: u32 = 1 << 3;
    /// Bits 7:4, `TPM_NT`.
    pub const NT_MASK: u32 = 0xF0;
    /// Bits 9:8, reserved.
    pub const RESERVED_LOW: u32 = 0x300;
    pub const POLICY_DELETE: u32 = 1 << 10;
    pub const WRITELOCKED: u32 = 1 << 11;
    pub const WRITEALL: u32 = 1 << 12;
    pub const WRITEDEFINE: u32 = 1 << 13;
    pub const WRITE_STCLEAR: u32 = 1 << 14;
    pub const GLOBALLOCK: u32 = 1 << 15;
    pub const PPREAD: u32 = 1 << 16;
    pub const OWNERREAD: u32 = 1 << 17;
    pub const AUTHREAD: u32 = 1 << 18;
    pub const POLICYREAD: u32 = 1 << 19;
    /// Bits 24:20, reserved.
    pub const RESERVED_HIGH: u32 = 0x01F0_0000;
    pub const NO_DA: u32 = 1 << 25;
    pub const ORDERLY: u32 = 1 << 26;
    pub const CLEAR_STCLEAR: u32 = 1 << 27;
    pub const READLOCKED: u32 = 1 << 28;
    pub const WRITTEN: u32 = 1 << 29;
    pub const PLATFORMCREATE: u32 = 1 << 30;
    pub const READ_STCLEAR: u32 = 1 << 31;
}

/// `TPM_NT` values (Part 2, Table 231).
pub(crate) mod nt {
    pub const ORDINARY: u32 = 0x0;
    pub const COUNTER: u32 = 0x1;
    pub const BITS: u32 = 0x2;
    pub const EXTEND: u32 = 0x4;
}

/// Size of a counter and of a bit field (Part 3, clause 31.3.1).
pub(crate) const COUNTER_SIZE: usize = 8;
/// Size of an extend index with `nameAlg` SHA-256.
pub(crate) const EXTEND_SIZE: usize = 32;

/// Highest `TPMA_NV` attribute bits that this implementation can honour, as a mask of those it
/// refuses.
const UNSUPPORTED: u32 = attr::GLOBALLOCK | attr::ORDERLY | attr::CLEAR_STCLEAR;
const READ_ACCESS: u32 = attr::PPREAD | attr::OWNERREAD | attr::AUTHREAD | attr::POLICYREAD;
const WRITE_ACCESS: u32 = attr::PPWRITE | attr::OWNERWRITE | attr::AUTHWRITE | attr::POLICYWRITE;

/// One defined index.
pub(crate) struct NvIndex {
    pub handle: u32,
    pub name_alg: u16,
    pub attributes: u32,
    pub auth_policy: Vec<u8>,
    pub auth: AuthValue,
    pub data: Zeroizing<Vec<u8>>,
}

impl NvIndex {
    /// `TPM_NT` of the index (bits 7:4 of the attributes).
    pub(crate) fn nt(&self) -> u32 {
        (self.attributes & attr::NT_MASK) >> 4
    }

    /// The data size an index of this type must have (`None`: any size, an ordinary index).
    fn required_size(attributes: u32) -> Option<usize> {
        match (attributes & attr::NT_MASK) >> 4 {
            nt::COUNTER | nt::BITS => Some(COUNTER_SIZE),
            nt::EXTEND => Some(EXTEND_SIZE),
            _ => None,
        }
    }

    /// `true` if a restored index is consistent: no reserved bit, a supported type, no feature
    /// the implementation refuses at definition, the size of its type, the data zero while
    /// `WRITTEN` is CLEAR, and (for the commands that need it) both read and write access.
    pub(crate) fn is_consistent(&self) -> bool {
        let a = self.attributes;
        let size_ok = match Self::required_size(a) {
            Some(n) => self.data.len() == n,
            None => true,
        };
        a & (attr::RESERVED_LOW | attr::RESERVED_HIGH | UNSUPPORTED) == 0
            && matches!(
                self.nt(),
                nt::ORDINARY | nt::COUNTER | nt::BITS | nt::EXTEND
            )
            && size_ok
            && a & READ_ACCESS != 0
            && a & WRITE_ACCESS != 0
            && (a & attr::POLICY_DELETE == 0 || a & attr::PLATFORMCREATE != 0)
            && (a & attr::WRITTEN != 0 || self.data.iter().all(|b| *b == 0))
    }

    /// The counter value (an 8-octet counter or bit field).
    fn value_u64(&self) -> Option<u64> {
        let b: [u8; 8] = self.data.as_slice().try_into().ok()?;
        Some(u64::from_be_bytes(b))
    }

    /// `TPMS_NV_PUBLIC` (Part 2, Table 235) with the current attributes.
    pub(crate) fn public(&self) -> Vec<u8> {
        let mut w = Writer::new();
        w.u32(self.handle);
        w.u16(self.name_alg);
        w.u32(self.attributes);
        w.tpm2b(&self.auth_policy);
        w.u16(self.data.len() as u16);
        w.finish()
    }

    /// `nameAlg || H(TPMS_NV_PUBLIC)` (Part 1, Table 6).
    pub(crate) fn name(&self) -> Vec<u8> {
        let mut n = self.name_alg.to_be_bytes().to_vec();
        n.extend_from_slice(&Sha256::digest(self.public()));
        n
    }
}

/// The NV index store.
#[derive(Default)]
pub(crate) struct NvStore {
    indices: Vec<NvIndex>,
    /// The largest value any counter has had: a counter's first increment starts above it
    /// (Part 3, clause 31.2). Persistent.
    counter_max: u64,
}

impl NvStore {
    pub(crate) fn get(&self, handle: u32) -> Option<&NvIndex> {
        self.indices.iter().find(|i| i.handle == handle)
    }

    fn get_mut(&mut self, handle: u32) -> Option<&mut NvIndex> {
        self.indices.iter_mut().find(|i| i.handle == handle)
    }

    pub(crate) fn count(&self) -> usize {
        self.indices.len()
    }

    /// The indices in ascending handle order, for the snapshot.
    pub(crate) fn iter(&self) -> impl Iterator<Item = &NvIndex> {
        self.indices.iter()
    }

    /// The largest counter value seen, for the snapshot.
    pub(crate) fn counter_max(&self) -> u64 {
        self.counter_max
    }

    /// Rebuilds the store from restored indices (any order) and the largest counter value; `None`
    /// for a duplicate handle, a handle outside the NV index range, more than [`MAX_NV_INDICES`]
    /// indices, more than [`NV_POOL_SIZE`] octets of data, an index that is not consistent with
    /// its type (`NvIndex::is_consistent`) or a written counter above `counter_max` (a counter
    /// can never exceed the largest value seen: restoring such a state would let a counter that
    /// was deleted and defined again start below its old value).
    pub(crate) fn from_indices(mut indices: Vec<NvIndex>, counter_max: u64) -> Option<NvStore> {
        if indices.len() > MAX_NV_INDICES
            || indices.iter().map(|i| i.data.len()).sum::<usize>() > NV_POOL_SIZE
            || indices
                .iter()
                .any(|i| !is_nv_handle(i.handle) || !i.is_consistent())
        {
            return None;
        }
        let counter_over_max = indices.iter().any(|i| {
            i.nt() == nt::COUNTER
                && i.attributes & attr::WRITTEN != 0
                && i.value_u64().is_none_or(|v| v > counter_max)
        });
        if counter_over_max {
            return None;
        }
        indices.sort_unstable_by_key(|i| i.handle);
        if indices.windows(2).any(|w| w[0].handle == w[1].handle) {
            return None;
        }
        Some(NvStore {
            indices,
            counter_max,
        })
    }

    /// Data octets in use.
    pub(crate) fn used(&self) -> usize {
        self.indices.iter().map(|i| i.data.len()).sum()
    }

    /// Defined handles in ascending order.
    pub(crate) fn handles(&self) -> Vec<u32> {
        self.indices.iter().map(|i| i.handle).collect()
    }

    fn insert(&mut self, index: NvIndex) {
        let at = self
            .indices
            .iter()
            .position(|i| i.handle > index.handle)
            .unwrap_or(self.indices.len());
        self.indices.insert(at, index);
    }

    fn remove(&mut self, handle: u32) {
        self.indices.retain(|i| i.handle != handle);
    }

    /// `TPM2_Clear`: deletes every index without `TPMA_NV_PLATFORMCREATE` (Part 3, clause 24.6).
    pub(crate) fn remove_owner_created(&mut self) {
        self.indices
            .retain(|i| i.attributes & attr::PLATFORMCREATE != 0);
    }

    /// `Startup(CLEAR)` (Reset and Restart, Part 3, clause 9.3.1): `READLOCKED` is CLEAR again
    /// (Part 2, Table 233), `WRITELOCKED` is CLEAR for an index with `WRITEDEFINE` CLEAR or
    /// `WRITTEN` CLEAR.
    pub(crate) fn startup_clear(&mut self) {
        for i in &mut self.indices {
            i.attributes &= !attr::READLOCKED;
            if i.attributes & attr::WRITEDEFINE == 0 || i.attributes & attr::WRITTEN == 0 {
                i.attributes &= !attr::WRITELOCKED;
            }
        }
    }
}

/// `true` if `handle` is in the range of `TPMI_RH_NV_INDEX` that slice A serves
/// (`NV_INDEX_FIRST..=NV_INDEX_LAST`, `TPM_HT_NV_INDEX`).
pub(crate) fn is_nv_handle(handle: u32) -> bool {
    h::handle_type(handle) == h::HT_NV_INDEX
}

/// What an NV command wants from the authorization handle.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Access {
    Read,
    Write,
}

impl Access {
    fn for_command(code: u32) -> Option<Self> {
        match code {
            cc::NV_READ
            | cc::NV_READ_LOCK
            | cc::NV_CERTIFY
            | cc::POLICY_NV
            | cc::POLICY_AUTHORIZE_NV => Some(Self::Read),
            cc::NV_WRITE
            | cc::NV_WRITE_LOCK
            | cc::NV_INCREMENT
            | cc::NV_EXTEND
            | cc::NV_SET_BITS => Some(Self::Write),
            _ => None,
        }
    }

    fn index_attribute(self, policy: bool) -> u32 {
        match (self, policy) {
            (Self::Read, false) => attr::AUTHREAD,
            (Self::Read, true) => attr::POLICYREAD,
            (Self::Write, false) => attr::AUTHWRITE,
            (Self::Write, true) => attr::POLICYWRITE,
        }
    }
}

impl Tpm {
    /// Whether the index may be accessed with the hierarchy enables as they are now (Part 2,
    /// Table 43).
    fn nv_accessible(&self, index: &NvIndex) -> bool {
        if index.attributes & attr::PLATFORMCREATE != 0 {
            self.hv.ph_enable_nv
        } else {
            self.hv.sh_enable
        }
    }

    /// An existing, accessible index at handle position `n` (one-based) of the command.
    pub(crate) fn nv_index_at(&self, handle: u32, n: u8) -> Result<&NvIndex, Rc> {
        if !is_nv_handle(handle) {
            return Err(Rc::VALUE.in_handle(n));
        }
        match self.persistent.nv.get(handle) {
            Some(i) if self.nv_accessible(i) => Ok(i),
            _ => Err(Rc::HANDLE.in_handle(n)),
        }
    }

    /// The handle checks of the NV commands, before authorization (see the module comment).
    pub(crate) fn check_nv_handles(&self, code: u32, handles: &CommandHandles) -> Result<(), Rc> {
        match code {
            cc::NV_DEFINE_SPACE => {
                provision_handle(handles[0])?;
                // The platform cannot define an index while `phEnableNV` is CLEAR (Table 43).
                if handles[0] == h::RH_PLATFORM && !self.hv.ph_enable_nv {
                    return Err(Rc::HIERARCHY.in_handle(1));
                }
            }
            cc::NV_UNDEFINE_SPACE => {
                provision_handle(handles[0])?;
                self.nv_index_at(handles[1], 2)?;
            }
            cc::NV_UNDEFINE_SPACE_SPECIAL => {
                self.nv_index_at(handles[0], 1)?;
                if handles[1] != h::RH_PLATFORM {
                    return Err(Rc::VALUE.in_handle(2));
                }
            }
            cc::NV_READ_PUBLIC | cc::NV_CHANGE_AUTH => {
                self.nv_index_at(handles[0], 1)?;
            }
            _ => {
                if let Some(access) = Access::for_command(code) {
                    let index = self.check_nv_authorization(handles[0], handles[1], 1, access)?;
                    // A read of an index that was never written fails before the authValue check
                    // (clause 31.1); a read lock does not need data (clause 31.14.1).
                    if code == cc::NV_READ && index.attributes & attr::WRITTEN == 0 {
                        return Err(Rc::NV_UNINITIALIZED);
                    }
                }
            }
        }
        Ok(())
    }

    /// `NV_Certify` authorizes the NV read with handles 2 and 3 after the signing key.
    pub(crate) fn check_nv_certify_handles(&self, handles: &CommandHandles) -> Result<(), Rc> {
        let index = self.check_nv_authorization(handles[1], handles[2], 2, Access::Read)?;
        if index.attributes & attr::WRITTEN == 0 {
            return Err(Rc::NV_UNINITIALIZED);
        }
        Ok(())
    }
    /// PolicyNV and PolicyAuthorizeNV use an independent target session. A trial still needs
    /// authorization to read the index.
    pub(crate) fn check_policy_nv_handles(&self, handles: &CommandHandles) -> Result<(), Rc> {
        let index = self.check_nv_authorization(handles[0], handles[1], 1, Access::Read)?;
        let (_, kind) = self.check_policy_session(handles[2], 2)?;
        // The real assertion checks WRITTEN before comparing the NV index authValue (clause 31.1).
        if kind != SessionKind::Trial && index.attributes & attr::WRITTEN == 0 {
            return Err(Rc::NV_UNINITIALIZED);
        }
        Ok(())
    }

    /// Validates both handles and hierarchy access. The index's own access depends
    /// on the authorization session and is checked later by `nv_index_auth`.
    fn check_nv_authorization(
        &self,
        auth: u32,
        index_handle: u32,
        auth_position: u8,
        access: Access,
    ) -> Result<&NvIndex, Rc> {
        if auth != h::RH_PLATFORM && auth != h::RH_OWNER && !is_nv_handle(auth) {
            return Err(Rc::VALUE.in_handle(auth_position));
        }
        if is_nv_handle(auth) {
            // The authorizing index must exist before it can be compared with `nvIndex`.
            self.nv_index_at(auth, auth_position)?;
        }
        let index = self.nv_index_at(index_handle, auth_position + 1)?;
        if is_nv_handle(auth) && auth != index_handle {
            return Err(Rc::NV_AUTHORIZATION);
        }
        let needed = match (auth, access) {
            (h::RH_PLATFORM, Access::Read) => attr::PPREAD,
            (h::RH_OWNER, Access::Read) => attr::OWNERREAD,
            (h::RH_PLATFORM, Access::Write) => attr::PPWRITE,
            (h::RH_OWNER, Access::Write) => attr::OWNERWRITE,
            _ => return Ok(index),
        };
        if index.attributes & needed == 0 {
            return Err(Rc::AUTH_TYPE);
        }
        Ok(index)
    }
    /// Authorization value for an index acting as its own authorizer. The selected
    /// mode must be enabled for the command's access (Part 3 v185, §5.6 step 7).
    pub(crate) fn nv_index_auth(
        &self,
        code: u32,
        handle: u32,
        position: u8,
        admin: bool,
        policy: bool,
    ) -> Result<AuthValue, Rc> {
        let index = self.nv_index_at(handle, position)?;
        if admin {
            // NV ADMIN requires policy and has no USER read/write attribute check (§5.6).
            return if policy {
                Ok(index.auth.clone())
            } else {
                Err(Rc::AUTH_TYPE)
            };
        }
        let needed = if code == cc::POLICY_SECRET {
            // PolicySecret proves the index's authValue even with a policy session (§23.4).
            attr::AUTHREAD
        } else {
            Access::for_command(code)
                .ok_or(Rc::AUTH_UNAVAILABLE)?
                .index_attribute(policy)
        };
        if index.attributes & needed == 0 {
            return Err(Rc::AUTH_UNAVAILABLE);
        }
        Ok(index.auth.clone())
    }

    /// `authValue` of an NV index used as authorization handle.
    pub(crate) fn nv_auth(&self, handle: u32) -> Option<AuthValue> {
        self.persistent.nv.get(handle).map(|i| i.auth.clone())
    }

    /// Name of an NV index (`nameAlg || H(public)`), `None` if it is not defined.
    pub(crate) fn nv_name(&self, handle: u32) -> Option<Vec<u8>> {
        self.persistent.nv.get(handle).map(NvIndex::name)
    }
    /// `TPM2_NV_ChangeAuth` (Part 3 v185, §31.15). The response uses the new authValue.
    pub(crate) fn cmd_nv_change_auth(
        &mut self,
        inv: &Invocation,
        p: &mut Reader<'_>,
    ) -> Result<Output, Rc> {
        p.set_param(1);
        let new_auth = p.tpm2b(limits::MAX_DIGEST_SIZE)?;
        p.finish()?;
        let auth = AuthValue::from_slice_trimmed(new_auth).ok_or(Rc::SIZE.in_param(1))?;
        let index = self
            .persistent
            .nv
            .get_mut(inv.handles[0])
            .ok_or(Rc::HANDLE.in_handle(1))?;
        index.auth = auth;
        Ok(Output::default())
    }

    /// `TPM2_NV_DefineSpace` (clause 31.3).
    pub(crate) fn cmd_nv_define_space(
        &mut self,
        inv: &Invocation,
        p: &mut Reader<'_>,
    ) -> Result<Output, Rc> {
        p.set_param(1);
        let auth_bytes = p.tpm2b(limits::MAX_DIGEST_SIZE)?;
        let auth = AuthValue::from_slice_trimmed(auth_bytes).ok_or(Rc::SIZE.in_param(1))?;
        p.set_param(2);
        let public_size = usize::from(p.u16()?);
        let before = p.remaining();
        let nv_index = p.u32()?;
        let name_alg = p.u16()?;
        let attributes = p.u32()?;
        let auth_policy = p.tpm2b(limits::MAX_DIGEST_SIZE)?;
        let data_size = usize::from(p.u16()?);
        if before - p.remaining() != public_size {
            return Err(p.err(Rc::SIZE));
        }
        p.finish()?;
        if !is_nv_handle(nv_index) {
            return Err(Rc::VALUE.in_param(2));
        }
        if name_alg != alg::SHA256 {
            return Err(Rc::HASH.in_param(2));
        }
        if attributes & (attr::RESERVED_LOW | attr::RESERVED_HIGH) != 0 {
            return Err(Rc::RESERVED_BITS.in_param(2));
        }
        if !auth_policy.is_empty() && auth_policy.len() != limits::MAX_DIGEST_SIZE {
            return Err(Rc::SIZE.in_param(2));
        }
        if self.persistent.nv.get(nv_index).is_some() {
            return Err(Rc::NV_DEFINED);
        }
        let platform = inv.handles[0] == h::RH_PLATFORM;
        if !matches!(
            (attributes & attr::NT_MASK) >> 4,
            nt::ORDINARY | nt::COUNTER | nt::BITS | nt::EXTEND
        ) || attributes & (attr::WRITTEN | attr::READLOCKED | attr::WRITELOCKED) != 0
            || attributes & UNSUPPORTED != 0
            || attributes & READ_ACCESS == 0
            || attributes & WRITE_ACCESS == 0
            // `PLATFORMCREATE` must say who defined the index (clause 31.3.1).
            || (attributes & attr::PLATFORMCREATE != 0) != platform
            || (attributes & attr::POLICY_DELETE != 0 && !platform)
        {
            return Err(Rc::ATTRIBUTES.in_param(2));
        }
        // A counter and a bit field are 8 octets, an extend index a digest of its `nameAlg`
        // (clause 31.3.1: `TPM_RC_SIZE`).
        if let Some(n) = NvIndex::required_size(attributes)
            && data_size != n
        {
            return Err(Rc::SIZE.in_param(2));
        }
        if data_size > MAX_NV_INDEX_SIZE
            || (attributes & attr::WRITEALL != 0 && data_size > NV_BUFFER_MAX)
        {
            return Err(Rc::SIZE.in_param(2));
        }
        let nv = &self.persistent.nv;
        if nv.count() >= MAX_NV_INDICES || nv.used() + data_size > NV_POOL_SIZE {
            return Err(Rc::NV_SPACE);
        }
        self.persistent.nv.insert(NvIndex {
            handle: nv_index,
            name_alg,
            attributes,
            auth_policy: auth_policy.to_vec(),
            auth,
            data: Zeroizing::new(vec![0u8; data_size]),
        });
        Ok(Output::default())
    }

    /// `TPM2_NV_UndefineSpace` (clause 31.4). Platform-created indices need Platform authorization,
    /// and policy-delete indices require the dedicated ADMIN command.
    pub(crate) fn cmd_nv_undefine_space(
        &mut self,
        inv: &Invocation,
        p: &mut Reader<'_>,
    ) -> Result<Output, Rc> {
        p.finish()?;
        let index = inv.handles[1];
        let attributes = self
            .persistent
            .nv
            .get(index)
            .ok_or(Rc::HANDLE.in_handle(2))?
            .attributes;
        if attributes & attr::PLATFORMCREATE != 0 && inv.handles[0] != h::RH_PLATFORM {
            return Err(Rc::NV_AUTHORIZATION);
        }
        if attributes & attr::POLICY_DELETE != 0 {
            return Err(Rc::ATTRIBUTES);
        }
        self.persistent.nv.remove(index);
        Ok(Output::default())
    }
    /// `TPM2_NV_UndefineSpaceSpecial` (Part 3 v185, §31.5), authorized by the index and Platform.
    pub(crate) fn cmd_nv_undefine_space_special(
        &mut self,
        inv: &Invocation,
        p: &mut Reader<'_>,
    ) -> Result<Output, Rc> {
        p.finish()?;
        let handle = inv.handles[0];
        let attributes = self.nv_index_at(handle, 1)?.attributes;
        let required = attr::PLATFORMCREATE | attr::POLICY_DELETE;
        if attributes & required != required {
            return Err(Rc::ATTRIBUTES);
        }
        self.persistent.nv.remove(handle);
        Ok(Output::default())
    }

    /// `TPM2_NV_ReadPublic` (clause 31.6): `TPM2B_NV_PUBLIC` and `TPM2B_NAME`, no authorization.
    pub(crate) fn cmd_nv_read_public(
        &mut self,
        inv: &Invocation,
        p: &mut Reader<'_>,
    ) -> Result<Output, Rc> {
        p.finish()?;
        let index = self.nv_index_at(inv.handles[0], 1)?;
        let mut w = Writer::new();
        w.tpm2b(&index.public());
        w.tpm2b(&index.name());
        Ok(Output {
            handles: Vec::new(),
            params: w.finish(),
        })
    }

    /// `TPM2_NV_Write` (clause 31.7): `data` at `offset`; `TPM_RC_NV_LOCKED` after the
    /// authorization when the index is write-locked; `TPM_RC_ATTRIBUTES` for an index that is not
    /// ordinary; `TPM_RC_NV_RANGE` beyond the data area or, with `TPMA_NV_WRITEALL`, for a size
    /// different from the data area. Sets `WRITTEN`.
    pub(crate) fn cmd_nv_write(
        &mut self,
        inv: &Invocation,
        p: &mut Reader<'_>,
    ) -> Result<Output, Rc> {
        p.set_param(1);
        let data = p.tpm2b(NV_BUFFER_MAX)?;
        p.set_param(2);
        let offset = usize::from(p.u16()?);
        p.finish()?;
        let index = self
            .persistent
            .nv
            .get_mut(inv.handles[1])
            .ok_or(Rc::HANDLE.in_handle(2))?;
        if index.attributes & attr::WRITELOCKED != 0 {
            return Err(Rc::NV_LOCKED);
        }
        // A counter, a bit field and an extend index change only by their own commands.
        if index.nt() != nt::ORDINARY {
            return Err(Rc::ATTRIBUTES);
        }
        if index.attributes & attr::WRITEALL != 0 && data.len() != index.data.len() {
            return Err(Rc::NV_RANGE);
        }
        if offset + data.len() > index.data.len() {
            return Err(Rc::NV_RANGE);
        }
        index.data[offset..offset + data.len()].copy_from_slice(data);
        index.attributes |= attr::WRITTEN;
        Ok(Output::default())
    }

    /// The index of a counter, extend or bit-field command (`nvIndex` is handle 2): the type
    /// check comes before the lock check (clauses 31.8.1, 31.9.1, 31.10.1).
    fn nv_typed_index(&mut self, handle: u32, expected: u32) -> Result<&mut NvIndex, Rc> {
        let index = self
            .persistent
            .nv
            .get_mut(handle)
            .ok_or(Rc::HANDLE.in_handle(2))?;
        if index.nt() != expected {
            return Err(Rc::ATTRIBUTES);
        }
        if index.attributes & attr::WRITELOCKED != 0 {
            return Err(Rc::NV_LOCKED);
        }
        Ok(index)
    }

    /// `TPM2_NV_Increment` (clause 31.8): the 8-octet counter grows by one; the first increment
    /// starts from the largest value any counter has had (clause 31.2). `TPM_RC_ATTRIBUTES` for
    /// an index that is not a counter, `TPM_RC_NV_LOCKED` when write-locked, `TPM_RC_NV_RANGE`
    /// when the counter cannot grow (NOT VERIFIED: the text does not name this case). Sets
    /// `WRITTEN`.
    pub(crate) fn cmd_nv_increment(
        &mut self,
        inv: &Invocation,
        p: &mut Reader<'_>,
    ) -> Result<Output, Rc> {
        p.finish()?;
        let max = self.persistent.nv.counter_max;
        let index = self.nv_typed_index(inv.handles[1], nt::COUNTER)?;
        let current = if index.attributes & attr::WRITTEN != 0 {
            index.value_u64().ok_or(Rc::FAILURE)?
        } else {
            max
        };
        let next = current.checked_add(1).ok_or(Rc::NV_RANGE)?;
        index.data.copy_from_slice(&next.to_be_bytes());
        index.attributes |= attr::WRITTEN;
        let store = &mut self.persistent.nv;
        store.counter_max = store.counter_max.max(next);
        Ok(Output::default())
    }

    /// `TPM2_NV_Extend` (clause 31.9): `data_new = SHA-256(data_old || data)`, the old value
    /// being the zero digest while `WRITTEN` is CLEAR (the data area starts as zeros). `data`
    /// may be any size up to `TPM2B_MAX_NV_BUFFER`. Sets `WRITTEN`.
    pub(crate) fn cmd_nv_extend(
        &mut self,
        inv: &Invocation,
        p: &mut Reader<'_>,
    ) -> Result<Output, Rc> {
        p.set_param(1);
        let data = p.tpm2b(NV_BUFFER_MAX)?;
        p.finish()?;
        let index = self.nv_typed_index(inv.handles[1], nt::EXTEND)?;
        let digest = Sha256::new()
            .chain_update(&index.data[..])
            .chain_update(data)
            .finalize();
        index.data.copy_from_slice(&digest);
        index.attributes |= attr::WRITTEN;
        Ok(Output::default())
    }

    /// `TPM2_NV_SetBits` (clause 31.10): the 64-bit field is ORed with `bits` (an unwritten
    /// index counts as all zero, which its data area already is). Sets `WRITTEN`, even when no
    /// bit changes.
    pub(crate) fn cmd_nv_set_bits(
        &mut self,
        inv: &Invocation,
        p: &mut Reader<'_>,
    ) -> Result<Output, Rc> {
        p.set_param(1);
        let bits = p.u64()?;
        p.finish()?;
        let index = self.nv_typed_index(inv.handles[1], nt::BITS)?;
        let now = index.value_u64().ok_or(Rc::FAILURE)?;
        index.data.copy_from_slice(&(now | bits).to_be_bytes());
        index.attributes |= attr::WRITTEN;
        Ok(Output::default())
    }

    /// `TPM2_NV_Read` (clause 31.13): `size` octets at `offset`.
    pub(crate) fn cmd_nv_read(
        &mut self,
        inv: &Invocation,
        p: &mut Reader<'_>,
    ) -> Result<Output, Rc> {
        p.set_param(1);
        let size = usize::from(p.u16()?);
        p.set_param(2);
        let offset = usize::from(p.u16()?);
        p.finish()?;
        let index = self
            .persistent
            .nv
            .get(inv.handles[1])
            .ok_or(Rc::HANDLE.in_handle(2))?;
        if index.attributes & attr::READLOCKED != 0 {
            return Err(Rc::NV_LOCKED);
        }
        if size > NV_BUFFER_MAX {
            return Err(Rc::VALUE.in_param(1));
        }
        if offset + size > index.data.len() {
            return Err(Rc::NV_RANGE);
        }
        let mut w = Writer::new();
        w.tpm2b(&index.data[offset..offset + size]);
        Ok(Output {
            handles: Vec::new(),
            params: w.finish(),
        })
    }

    /// `TPM2_NV_WriteLock` (clause 31.11): needs `WRITEDEFINE` or `WRITE_STCLEAR`, sets
    /// `WRITELOCKED`; already locked with proper authorization is a success.
    pub(crate) fn cmd_nv_write_lock(
        &mut self,
        inv: &Invocation,
        p: &mut Reader<'_>,
    ) -> Result<Output, Rc> {
        p.finish()?;
        let index = self
            .persistent
            .nv
            .get_mut(inv.handles[1])
            .ok_or(Rc::HANDLE.in_handle(2))?;
        if index.attributes & (attr::WRITEDEFINE | attr::WRITE_STCLEAR) == 0 {
            return Err(Rc::ATTRIBUTES);
        }
        index.attributes |= attr::WRITELOCKED;
        Ok(Output::default())
    }

    /// `TPM2_NV_ReadLock` (clause 31.14): needs `READ_STCLEAR`, sets `READLOCKED` until the next
    /// `Startup(CLEAR)`; an unwritten index may be read-locked.
    pub(crate) fn cmd_nv_read_lock(
        &mut self,
        inv: &Invocation,
        p: &mut Reader<'_>,
    ) -> Result<Output, Rc> {
        p.finish()?;
        let index = self
            .persistent
            .nv
            .get_mut(inv.handles[1])
            .ok_or(Rc::HANDLE.in_handle(2))?;
        if index.attributes & attr::READ_STCLEAR == 0 {
            return Err(Rc::ATTRIBUTES);
        }
        index.attributes |= attr::READLOCKED;
        Ok(Output::default())
    }
}

/// `TPMI_RH_PROVISION` (Part 2, Table 68): owner or platform, otherwise `TPM_RC_VALUE`.
fn provision_handle(handle: u32) -> Result<(), Rc> {
    if handle == h::RH_OWNER || handle == h::RH_PLATFORM {
        Ok(())
    } else {
        Err(Rc::VALUE.in_handle(1))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn index(handle: u32, attributes: u32, size: usize) -> NvIndex {
        NvIndex {
            handle,
            name_alg: alg::SHA256,
            attributes,
            auth_policy: Vec::new(),
            auth: AuthValue::empty(),
            data: Zeroizing::new(vec![0u8; size]),
        }
    }

    #[test]
    fn public_area_and_name_follow_tables_235_and_6() {
        let i = index(0x0100_0001, attr::OWNERWRITE | attr::OWNERREAD, 4);
        // nvIndex, nameAlg SHA-256, attributes, authPolicy (empty), dataSize.
        let mut expect = Vec::new();
        expect.extend_from_slice(&0x0100_0001u32.to_be_bytes());
        expect.extend_from_slice(&0x000Bu16.to_be_bytes());
        expect.extend_from_slice(&(attr::OWNERWRITE | attr::OWNERREAD).to_be_bytes());
        expect.extend_from_slice(&0u16.to_be_bytes());
        expect.extend_from_slice(&4u16.to_be_bytes());
        assert_eq!(i.public(), expect);
        let name = i.name();
        assert_eq!(name.len(), 34);
        assert_eq!(&name[..2], &[0x00, 0x0B]);
        assert_eq!(&name[2..], &Sha256::digest(&expect)[..]);
    }

    #[test]
    fn name_changes_with_the_written_attribute() {
        let mut i = index(0x0100_0001, attr::OWNERWRITE | attr::OWNERREAD, 4);
        let before = i.name();
        i.attributes |= attr::WRITTEN;
        assert_ne!(before, i.name());
    }

    #[test]
    fn store_keeps_handles_sorted_and_counts_octets() {
        let mut s = NvStore::default();
        s.insert(index(0x0100_0003, 0, 3));
        s.insert(index(0x0100_0001, 0, 5));
        s.insert(index(0x0100_0002, 0, 7));
        assert_eq!(s.handles(), [0x0100_0001, 0x0100_0002, 0x0100_0003]);
        assert_eq!(s.used(), 15);
        s.remove(0x0100_0002);
        assert_eq!(s.count(), 2);
    }

    #[test]
    fn startup_clear_unlocks_as_part_3_clause_9_3_1_says() {
        let mut s = NvStore::default();
        // Locked, WRITEDEFINE and WRITTEN: stays write-locked; read lock is dropped.
        s.insert(index(
            0x0100_0001,
            attr::WRITEDEFINE | attr::WRITTEN | attr::WRITELOCKED | attr::READLOCKED,
            1,
        ));
        // Locked, WRITEDEFINE but not WRITTEN: unlocked.
        s.insert(index(0x0100_0002, attr::WRITEDEFINE | attr::WRITELOCKED, 1));
        // Locked, WRITE_STCLEAR only: unlocked.
        s.insert(index(
            0x0100_0003,
            attr::WRITE_STCLEAR | attr::WRITTEN | attr::WRITELOCKED,
            1,
        ));
        s.startup_clear();
        let a = |h: u32| s.get(h).unwrap().attributes;
        assert_ne!(a(0x0100_0001) & attr::WRITELOCKED, 0);
        assert_eq!(a(0x0100_0001) & attr::READLOCKED, 0);
        assert_eq!(a(0x0100_0002) & attr::WRITELOCKED, 0);
        assert_eq!(a(0x0100_0003) & attr::WRITELOCKED, 0);
    }

    #[test]
    fn clear_removes_only_owner_created_indices() {
        let mut s = NvStore::default();
        s.insert(index(0x0100_0001, attr::PLATFORMCREATE, 1));
        s.insert(index(0x0100_0002, 0, 1));
        s.remove_owner_created();
        assert_eq!(s.handles(), [0x0100_0001]);
    }
}
