//! NV indices, base (Part 3 v184, clause 31; Part 2 clause 13): `TPM2_NV_DefineSpace`,
//! `TPM2_NV_UndefineSpace`, `TPM2_NV_ReadPublic`, `TPM2_NV_Write`, `TPM2_NV_Read`, and the two
//! lock commands `TPM2_NV_WriteLock` and `TPM2_NV_ReadLock` (not in the plan's list, added so that
//! `TPMA_NV_WRITEDEFINE`, `TPMA_NV_WRITE_STCLEAR` and `TPMA_NV_READ_STCLEAR` can take effect and
//! `TPMA_NV_WRITELOCKED`/`READLOCKED` are reachable).
//!
//! Supported index type: `TPM_NT_ORDINARY` (opaque data written by `TPM2_NV_Write`). Counters,
//! bit fields, extend indices and PIN indices need `TPM2_NV_Increment`, `SetBits`, `Extend` and
//! are slice B: defining one is `TPM_RC_ATTRIBUTES` (Part 3, clause 31.3.1: "if the implementation
//! does not support ... the TPM shall return TPM_RC_ATTRIBUTES"). Attributes that need machinery
//! the slice lacks are refused the same way: `TPMA_NV_POLICYREAD`/`POLICYWRITE` (policy sessions),
//! `POLICY_DELETE` (`TPM2_NV_UndefineSpaceSpecial`), `GLOBALLOCK` (`TPM2_NV_GlobalWriteLock`),
//! `ORDERLY`, `CLEAR_STCLEAR` (PTP 1.07, Table 12: not required). `TPMA_NV_NO_DA` is stored (there
//! is no dictionary-attack logic to bypass).
//!
//! Limits (this implementation; PTP 1.07 minimum: 68 indices, 11026 octets, Table 2 and clause
//! 4.6.2): [`MAX_NV_INDICES`] indices, [`NV_POOL_SIZE`] octets of data in total,
//! [`MAX_NV_INDEX_SIZE`] octets per index (`TPM_PT_NV_INDEX_MAX`, PTP minimum 8500) and
//! [`NV_BUFFER_MAX`] octets per read or write (`TPM_PT_NV_BUFFER_MAX`, PTP minimum 512).
//!
//! Persistence: the NV store is part of [`crate::state::Persistent`] and survives `_TPM_Init`,
//! but it is NOT part of the snapshot yet (slice C, "Persistent state v1"): `restore` returns a
//! TPM without indices.
//!
//! Access control (Part 3, clause 31.1; Part 2, Table 43): an index created with
//! `TPMA_NV_PLATFORMCREATE` is accessible only while `phEnableNV` is SET, one created without it
//! only while `shEnable` is SET (`TPM_RC_HANDLE` otherwise, Table 43); the platform cannot define
//! an index while `phEnableNV` is CLEAR (`TPM_RC_HIERARCHY`, Table 43). Authorization comes from
//! `TPM_RH_PLATFORM` (`PPREAD`/`PPWRITE`), `TPM_RH_OWNER` (`OWNERREAD`/`OWNERWRITE`) or the index
//! itself (`AUTHREAD`/`AUTHWRITE`); an index handle other than `nvIndex` is `TPM_RC_NV_AUTHORIZATION`.
//! The Name of an index is `nameAlg || H(TPMS_NV_PUBLIC)` and changes when `WRITTEN`/locks change
//! (Part 1, clause 14).
//!
//! Order of checks for a command with a session: handle validity and access control (including
//! `TPM_RC_NV_UNINITIALIZED` for a read of an unwritten index, which must precede the authValue
//! check) -> authorization -> lock state and range (Part 3: "authorization sessions are checked
//! before the lock state", clauses 31.7.1 and 31.13.1).
//!
//! NOT VERIFIED against TPM 2.0 Library v185/v184 (the open text states the rule, not the code):
//! `TPM_RC_AUTH_TYPE` when the authorization handle's role is not enabled by the index attributes;
//! `TPM_RC_RESERVED_BITS` for reserved `TPMA_NV` bits in `TPM2_NV_DefineSpace`; `TPM_RC_ATTRIBUTES`
//! for the unsupported attributes and index types listed above; `TPM_RC_SIZE` for a `publicInfo`
//! whose `size` differs from the marshaled structure and for a non-empty `authPolicy` that is not
//! a digest; `TPM_RC_VALUE` for a read `size` above [`NV_BUFFER_MAX`]; `TPM_RC_NV_RANGE` for an
//! `offset` beyond the data size; `TPM_RC_NV_LOCKED` for a locked read or write (the code is
//! named in clauses 31.7.1 and 31.13.1); `TPM_RC_HANDLE` with the handle number for an undefined
//! or inaccessible index (named in clause 31.4.1 and Table 43; the index modifier is ours).

use sha2::{Digest, Sha256};
use zeroize::Zeroizing;

use crate::codes::{alg, handle as h};
use crate::dispatch::Invocation;
use crate::hash::AuthValue;
use crate::marshal::{Reader, Writer, limits};
use crate::response::{Output, Rc};
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

/// `TPM_NT_ORDINARY` (Part 2, Table 231).
const NT_ORDINARY: u32 = 0x0;

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
    fn nv_index_at(&self, handle: u32, n: u8) -> Result<&NvIndex, Rc> {
        if !is_nv_handle(handle) {
            return Err(Rc::VALUE.in_handle(n));
        }
        match self.persistent.nv.get(handle) {
            Some(i) if self.nv_accessible(i) => Ok(i),
            _ => Err(Rc::HANDLE.in_handle(n)),
        }
    }

    /// The handle checks of the NV commands, before authorization (see the module comment).
    pub(crate) fn check_nv_handles(&self, code: u32, handles: &[u32; 2]) -> Result<(), Rc> {
        match code {
            crate::codes::cc::NV_DEFINE_SPACE => {
                provision_handle(handles[0])?;
                // The platform cannot define an index while `phEnableNV` is CLEAR (Table 43).
                if handles[0] == h::RH_PLATFORM && !self.hv.ph_enable_nv {
                    return Err(Rc::HIERARCHY.in_handle(1));
                }
            }
            crate::codes::cc::NV_UNDEFINE_SPACE => {
                provision_handle(handles[0])?;
                self.nv_index_at(handles[1], 2)?;
            }
            crate::codes::cc::NV_READ_PUBLIC => {
                self.nv_index_at(handles[0], 1)?;
            }
            crate::codes::cc::NV_WRITE | crate::codes::cc::NV_WRITE_LOCK => {
                self.check_nv_authorization(handles, Access::Write)?;
            }
            crate::codes::cc::NV_READ | crate::codes::cc::NV_READ_LOCK => {
                let index = self.check_nv_authorization(handles, Access::Read)?;
                // A read of an index that was never written fails before the authValue check
                // (clause 31.1); a read lock does not need data (clause 31.14.1).
                if code == crate::codes::cc::NV_READ && index.attributes & attr::WRITTEN == 0 {
                    return Err(Rc::NV_UNINITIALIZED);
                }
            }
            _ => {}
        }
        Ok(())
    }

    /// Validates `authHandle` (`TPMI_RH_NV_AUTH`), `nvIndex` and the role the attributes allow.
    fn check_nv_authorization(&self, handles: &[u32; 2], access: Access) -> Result<&NvIndex, Rc> {
        let auth = handles[0];
        if auth != h::RH_PLATFORM && auth != h::RH_OWNER && !is_nv_handle(auth) {
            return Err(Rc::VALUE.in_handle(1));
        }
        if is_nv_handle(auth) {
            // The authorizing index must exist before it can be compared with `nvIndex`.
            self.nv_index_at(auth, 1)?;
        }
        let index = self.nv_index_at(handles[1], 2)?;
        if is_nv_handle(auth) && auth != handles[1] {
            return Err(Rc::NV_AUTHORIZATION);
        }
        let (platform, owner, own) = match access {
            Access::Read => (attr::PPREAD, attr::OWNERREAD, attr::AUTHREAD),
            Access::Write => (attr::PPWRITE, attr::OWNERWRITE, attr::AUTHWRITE),
        };
        let needed = match auth {
            h::RH_PLATFORM => platform,
            h::RH_OWNER => owner,
            _ => own,
        };
        if index.attributes & needed == 0 {
            return Err(Rc::AUTH_TYPE);
        }
        Ok(index)
    }

    /// `authValue` of an NV index used as authorization handle.
    pub(crate) fn nv_auth(&self, handle: u32) -> Option<AuthValue> {
        self.persistent.nv.get(handle).map(|i| i.auth.clone())
    }

    /// Name of an NV index (`nameAlg || H(public)`), `None` if it is not defined.
    pub(crate) fn nv_name(&self, handle: u32) -> Option<Vec<u8>> {
        self.persistent.nv.get(handle).map(NvIndex::name)
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
        let unsupported = attr::POLICYREAD
            | attr::POLICYWRITE
            | attr::POLICY_DELETE
            | attr::GLOBALLOCK
            | attr::ORDERLY
            | attr::CLEAR_STCLEAR;
        let read = attr::PPREAD | attr::OWNERREAD | attr::AUTHREAD;
        let write = attr::PPWRITE | attr::OWNERWRITE | attr::AUTHWRITE;
        if (attributes & attr::NT_MASK) >> 4 != NT_ORDINARY
            || attributes & (attr::WRITTEN | attr::READLOCKED | attr::WRITELOCKED) != 0
            || attributes & unsupported != 0
            || attributes & read == 0
            || attributes & write == 0
            // `PLATFORMCREATE` must say who defined the index (clause 31.3.1).
            || (attributes & attr::PLATFORMCREATE != 0) != platform
        {
            return Err(Rc::ATTRIBUTES.in_param(2));
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

    /// `TPM2_NV_UndefineSpace` (clause 31.4): the platform deletes any index, the owner only an
    /// index without `TPMA_NV_PLATFORMCREATE` (`TPM_RC_NV_AUTHORIZATION` otherwise).
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
        self.persistent.nv.remove(index);
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
    /// authorization when the index is write-locked; `TPM_RC_NV_RANGE` beyond the data area or,
    /// with `TPMA_NV_WRITEALL`, for a size different from the data area. Sets `WRITTEN`.
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
