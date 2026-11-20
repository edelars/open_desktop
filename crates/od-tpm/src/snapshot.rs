//! The payload codec of `ODTPMSTA` versions 3 through 8. Version 4 adds the
//! primary policies of Owner, Endorsement and Lockout. Version 5 adds `totalResetCount`.
//! Version 6 adds the desired PCR allocation mask. Version 7 adds D-RTM establishment.
//! Version 8 appends the remaining 32 octets of each persistent Primary Seed.
//! The header, the SHA-256 check and the
//! public entry points are in `state.rs`; the layout of the whole snapshot is documented there.
//!
//! **Sensitive data.** The payload holds the hierarchy seeds, the authorization values of the
//! owner, the endorsement and the lockout hierarchies, the authorization values and the data of the
//! NV indices, and the private parts of the persistent objects, all in clear text (ADR-0009,
//! decision 5: the protection of this blob is S9-27, host side). The blob is sensitive data: it must
//! not be logged or stored unprotected; this module builds it in a buffer of exactly the final size
//! (two passes: length, then writing), so that no earlier, smaller copy is freed unwiped, wipes the
//! buffer on drop and never prints it.
//!
//! **Not in the payload** (volatile by Part 3, clause 9.3.1 or not state): `platformPolicy`, `platformAuth` (EmptyAuth
//! on every `Startup(CLEAR)`; a restored TPM is in the state after `_TPM_Init`, so `Startup` follows),
//! the Null seed, the enables, PCR, sessions, transient objects, sequences, the state saved by
//! `Shutdown(STATE)`, the DRBG, `Time`, the volatile DA timers.
//!
//! **Payload layout** (big-endian; `lv8` is a one-octet length and the octets, `lv16` a two-octet
//! length and the octets):
//!
//! | field | size |
//! |---|---|
//! | seeds platform, owner, endorsement | 3 x 32 |
//! | `Clock` ms (u64), `resetCount` (u32), `restartCount` (u32), orderly (u8: 0/1) | 17 |
//! | `ownerAuth`, `endorsementAuth`, `lockoutAuth` (`lv8`, at most 32 octets each) | variable |
//! | hierarchy flags (u8: bit 0 `ownerAuthSet`, 1 `endorsementAuthSet`, 2 `lockoutAuthSet`, 3 `disableClear`; others zero) | 1 |
//! | DA: `failedTries`, `maxTries`, `recoveryTime`, `lockoutRecovery` (u32 each), `lockoutAuth` enabled (u8: 0/1) | 17 |
//! | NV: count (u16, at most 128), then per index: handle (u32), `nameAlg` (u16), attributes (u32; bits 7:4 are `TPM_NT`: ordinary, counter, bit field or extend), `authPolicy` (`lv8`), `authValue` (`lv8`), data (`lv16`) | variable |
//! | NV counter maximum: the largest value any counter has had (u64, slice C3) | 8 |
//! | persistent objects: count (u8, at most 16), then per object: handle (u32), hierarchy (u32), `TPMT_PUBLIC` (`lv16`), Qualified Name (`lv8`), marshaled `TPM2B_SENSITIVE` (`lv16`) | variable |
//! | v4 onward: Owner, Endorsement, Lockout policy. Each is `hashAlg` (u16), then 32 octets for SHA-256. NULL has no digest. | 6–102 |
//! | v5 onward: `totalResetCount` | 8 |
//! | v6 onward: desired PCR bank mask (SHA-256 bit 0, SHA-384 bit 1) | 1 |
//!
//! `decode` rejects, with `StateError::BadField`, a field that is out of range or inconsistent
//! (a length above its limit, a duplicate or foreign handle, a handle outside the range of its
//! hierarchy, a public area that does not unmarshal, a private part that does not belong to the
//! public area, trailing octets); a truncated payload is `StateError::BadLength`. An NV index must
//! be consistent with its type (`TPM_NT` supported, size 8 for a counter or bit field and 32 for an
//! extend index, no reserved or unsupported attribute, data zero while `WRITTEN` is CLEAR) and a
//! written counter may not exceed the stored maximum (`NvStore::from_indices`).
//!
//! **Version history.** Version 2 (slice C1) had no counter maximum and only ordinary indices; it
//! is refused as `UnsupportedVersion(2)` and never restored with silently empty state. The refusal
//! is a choice, not a necessity: version 2 held no counters, so `counter_max` = 0 would have been
//! exact; refusing was chosen so that an old format is not read implicitly.

use zeroize::Zeroizing;

use crate::codes::{alg, handle as h};
use crate::crypto::DIGEST;
use crate::da::DaPersistent;
use crate::hash::AuthValue;
use crate::hierarchy::{HierarchyPersistent, LEGACY_SEED_BYTES, PRIMARY_SEED_BYTES, PrimarySeed};
use crate::marshal::{Reader, TpmtPublic, limits};
use crate::nv::{MAX_NV_INDEX_SIZE, NvIndex, NvStore};
use crate::object::{
    MAX_PERSISTENT, Object, PersistentObjects, name_of, sensitive_from_bytes, sensitive_to_bytes,
};
use crate::pcr::PcrAllocation;
use crate::state::{Persistent, StateError};

/// The most octets of a Qualified Name (`nameAlg` and a SHA-256 digest).
const MAX_QN: usize = 2 + 32;

/// A payload writer. It runs twice: a counting pass that only sums the length, then a writing pass
/// into a buffer of exactly that capacity, so the buffer never grows and no earlier, smaller copy of
/// the secrets is left freed but not wiped; the buffer is wiped on drop.
struct Enc {
    buf: Zeroizing<Vec<u8>>,
    len: usize,
    count_only: bool,
}

impl Enc {
    fn counting() -> Enc {
        Enc {
            buf: Zeroizing::new(Vec::new()),
            len: 0,
            count_only: true,
        }
    }
    fn writing(capacity: usize) -> Enc {
        Enc {
            buf: Zeroizing::new(Vec::with_capacity(capacity)),
            len: 0,
            count_only: false,
        }
    }
    fn raw(&mut self, v: &[u8]) {
        self.len += v.len();
        if !self.count_only {
            self.buf.extend_from_slice(v);
        }
    }
    fn u8(&mut self, v: u8) {
        self.raw(&[v]);
    }
    fn u16(&mut self, v: u16) {
        self.raw(&v.to_be_bytes());
    }
    fn u32(&mut self, v: u32) {
        self.raw(&v.to_be_bytes());
    }
    fn u64(&mut self, v: u64) {
        self.raw(&v.to_be_bytes());
    }
    /// `lv8`; the caller guarantees at most 255 octets.
    fn lv8(&mut self, v: &[u8]) {
        self.u8(v.len() as u8);
        self.raw(v);
    }
    /// `lv16`; the caller guarantees at most 65535 octets.
    fn lv16(&mut self, v: &[u8]) {
        self.u16(v.len() as u16);
        self.raw(v);
    }
}

/// A payload reader; `None` is a truncated payload.
struct Cur<'a> {
    buf: &'a [u8],
}

impl<'a> Cur<'a> {
    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        if self.buf.len() < n {
            return None;
        }
        let (a, b) = self.buf.split_at(n);
        self.buf = b;
        Some(a)
    }
    fn u8(&mut self) -> Option<u8> {
        self.take(1).map(|b| b[0])
    }
    fn u16(&mut self) -> Option<u16> {
        self.take(2).map(|b| u16::from_be_bytes([b[0], b[1]]))
    }
    fn u32(&mut self) -> Option<u32> {
        self.take(4)
            .map(|b| u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
    }
    fn u64(&mut self) -> Option<u64> {
        self.take(8).map(|b| {
            let mut a = [0u8; 8];
            a.copy_from_slice(b);
            u64::from_be_bytes(a)
        })
    }
    fn lv8(&mut self) -> Option<&'a [u8]> {
        let n = usize::from(self.u8()?);
        self.take(n)
    }
    fn lv16(&mut self) -> Option<&'a [u8]> {
        let n = usize::from(self.u16()?);
        self.take(n)
    }
}

/// Why a payload field was refused: truncated, or out of range or inconsistent.
enum Bad {
    Short,
    Field,
}

impl From<Bad> for StateError {
    fn from(b: Bad) -> StateError {
        match b {
            Bad::Short => StateError::BadLength,
            Bad::Field => StateError::BadField,
        }
    }
}

/// The payload of the persistent state; `clock_ms` is the current `Clock`. `None` if a private part
/// cannot be marshaled (not reachable for objects built by this crate); the caller fails closed.
pub(crate) fn encode(p: &Persistent, clock_ms: u64) -> Option<Zeroizing<Vec<u8>>> {
    let mut counter = Enc::counting();
    write_payload(p, clock_ms, &mut counter)?;
    let mut e = Enc::writing(counter.len);
    write_payload(p, clock_ms, &mut e)?;
    // The two passes write the same octets; a mismatch means the buffer would have grown.
    if e.buf.len() != counter.len || e.buf.capacity() != counter.len {
        return None;
    }
    Some(e.buf)
}

fn write_payload(p: &Persistent, clock_ms: u64, e: &mut Enc) -> Option<()> {
    let seeds = [&p.platform_seed, &p.owner_seed, &p.endorsement_seed];
    for seed in seeds {
        e.raw(&seed[..LEGACY_SEED_BYTES]);
    }
    e.u64(clock_ms);
    e.u32(p.reset_count);
    e.u32(p.restart_count);
    e.u8(u8::from(p.orderly));

    let hp = &p.hier;
    e.lv8(hp.owner_auth.as_slice());
    e.lv8(hp.endorsement_auth.as_slice());
    e.lv8(hp.lockout_auth.as_slice());
    e.u8(u8::from(hp.owner_auth_set)
        | (u8::from(hp.endorsement_auth_set) << 1)
        | (u8::from(hp.lockout_auth_set) << 2)
        | (u8::from(hp.disable_clear) << 3));

    let da = &p.da;
    e.u32(da.failed_tries);
    e.u32(da.max_tries);
    e.u32(da.recovery_time);
    e.u32(da.lockout_recovery);
    e.u8(u8::from(da.lockout_auth_enabled));

    e.u16(p.nv.count() as u16);
    for i in p.nv.iter() {
        e.u32(i.handle);
        e.u16(i.name_alg);
        e.u32(i.attributes);
        e.lv8(&i.auth_policy);
        e.lv8(i.auth.as_slice());
        e.lv16(&i.data);
    }
    e.u64(p.nv.counter_max());

    let objects = p.objects.sorted();
    e.u8(objects.len() as u8);
    for (handle, o) in objects {
        e.u32(handle);
        e.u32(o.hierarchy);
        e.lv16(&o.public.to_bytes());
        e.lv8(&o.qualified_name);
        let sensitive = o.sensitive.as_ref()?;
        let bytes = sensitive_to_bytes(sensitive, o.public.type_alg()).ok()?;
        e.lv16(&bytes);
    }
    for policy in [&hp.owner_policy, &hp.endorsement_policy, &hp.lockout_policy] {
        if let Some(digest) = policy {
            e.u16(alg::SHA256);
            e.raw(digest);
        } else {
            e.u16(alg::NULL);
        }
    }
    e.u64(p.total_reset_count);
    e.u8(p.pcr_allocation.bits());
    e.u8(u8::from(p.established));
    for seed in seeds {
        e.raw(&seed[LEGACY_SEED_BYTES..]);
    }
    Some(())
}

/// Parses a payload into persistent state; the payload has been checked against its digest by the
/// caller.
pub(crate) fn decode(payload: &[u8], version: u16) -> Result<Persistent, StateError> {
    decode_inner(payload, version).map_err(StateError::from)
}

fn decode_inner(payload: &[u8], version: u16) -> Result<Persistent, Bad> {
    let mut c = Cur { buf: payload };
    let seed = |c: &mut Cur<'_>| -> Result<PrimarySeed, Bad> {
        let b = c.take(LEGACY_SEED_BYTES).ok_or(Bad::Short)?;
        let mut s = Zeroizing::new([0u8; PRIMARY_SEED_BYTES]);
        s[..LEGACY_SEED_BYTES].copy_from_slice(b);
        Ok(s)
    };
    let mut platform_seed = seed(&mut c)?;
    let mut owner_seed = seed(&mut c)?;
    let mut endorsement_seed = seed(&mut c)?;
    let clock_base_ms = c.u64().ok_or(Bad::Short)?;
    let reset_count = c.u32().ok_or(Bad::Short)?;
    let restart_count = c.u32().ok_or(Bad::Short)?;
    let orderly = flag(c.u8().ok_or(Bad::Short)?)?;

    let auth = |c: &mut Cur<'_>| -> Result<AuthValue, Bad> {
        let b = c.lv8().ok_or(Bad::Short)?;
        // Stored without trailing zeros; normalized again so that a hand-made payload cannot
        // break the invariant the authorization checks rely on.
        AuthValue::from_slice_trimmed(b).ok_or(Bad::Field)
    };
    let owner_auth = auth(&mut c)?;
    let endorsement_auth = auth(&mut c)?;
    let lockout_auth = auth(&mut c)?;
    let hflags = c.u8().ok_or(Bad::Short)?;
    if hflags & 0xF0 != 0 {
        return Err(Bad::Field);
    }
    let mut hier = HierarchyPersistent {
        owner_auth,
        endorsement_auth,
        lockout_auth,
        owner_policy: None,
        endorsement_policy: None,
        lockout_policy: None,
        owner_auth_set: hflags & 1 != 0,
        endorsement_auth_set: hflags & 2 != 0,
        lockout_auth_set: hflags & 4 != 0,
        disable_clear: hflags & 8 != 0,
    };

    let da = DaPersistent {
        failed_tries: c.u32().ok_or(Bad::Short)?,
        max_tries: c.u32().ok_or(Bad::Short)?,
        recovery_time: c.u32().ok_or(Bad::Short)?,
        lockout_recovery: c.u32().ok_or(Bad::Short)?,
        lockout_auth_enabled: flag(c.u8().ok_or(Bad::Short)?)?,
    };

    let nv_count = usize::from(c.u16().ok_or(Bad::Short)?);
    if nv_count > crate::nv::MAX_NV_INDICES {
        return Err(Bad::Field);
    }
    let mut indices = Vec::with_capacity(nv_count);
    for _ in 0..nv_count {
        let handle = c.u32().ok_or(Bad::Short)?;
        let name_alg = c.u16().ok_or(Bad::Short)?;
        let attributes = c.u32().ok_or(Bad::Short)?;
        let policy = c.lv8().ok_or(Bad::Short)?;
        let index_auth = auth(&mut c)?;
        let data = c.lv16().ok_or(Bad::Short)?;
        if name_alg != alg::SHA256
            || !(policy.is_empty() || policy.len() == limits::MAX_DIGEST_SIZE)
            || data.len() > MAX_NV_INDEX_SIZE
        {
            return Err(Bad::Field);
        }
        indices.push(NvIndex {
            handle,
            name_alg,
            attributes,
            auth_policy: policy.to_vec(),
            auth: index_auth,
            data: Zeroizing::new(data.to_vec()),
        });
    }
    let counter_max = c.u64().ok_or(Bad::Short)?;
    let nv = NvStore::from_indices(indices, counter_max).ok_or(Bad::Field)?;

    let object_count = usize::from(c.u8().ok_or(Bad::Short)?);
    if object_count > MAX_PERSISTENT {
        return Err(Bad::Field);
    }
    let mut items = Vec::with_capacity(object_count);
    for _ in 0..object_count {
        let handle = c.u32().ok_or(Bad::Short)?;
        let hierarchy = c.u32().ok_or(Bad::Short)?;
        let public_bytes = c.lv16().ok_or(Bad::Short)?;
        let qn = c.lv8().ok_or(Bad::Short)?;
        let sensitive_bytes = Zeroizing::new(c.lv16().ok_or(Bad::Short)?.to_vec());
        if !persistent_handle_fits(handle, hierarchy) || qn.len() > MAX_QN {
            return Err(Bad::Field);
        }
        let mut r = Reader::new(public_bytes);
        let public = TpmtPublic::unmarshal(&mut r).map_err(|_| Bad::Field)?;
        r.finish().map_err(|_| Bad::Field)?;
        if public.name_alg != alg::SHA256 {
            return Err(Bad::Field);
        }
        let sensitive = sensitive_from_bytes(&sensitive_bytes, &public).map_err(|_| Bad::Field)?;
        items.push((
            handle,
            Object {
                name: name_of(&public),
                qualified_name: qn.to_vec(),
                public,
                hierarchy,
                sensitive: Some(sensitive),
            },
        ));
    }
    let objects = PersistentObjects::from_items(items).ok_or(Bad::Field)?;
    if version >= 4 {
        for policy in [
            &mut hier.owner_policy,
            &mut hier.endorsement_policy,
            &mut hier.lockout_policy,
        ] {
            *policy = match c.u16().ok_or(Bad::Short)? {
                alg::NULL => None,
                alg::SHA256 => Some(
                    c.take(DIGEST)
                        .ok_or(Bad::Short)?
                        .try_into()
                        .map_err(|_| Bad::Short)?,
                ),
                _ => return Err(Bad::Field),
            };
        }
    }
    let total_reset_count = if version >= 5 {
        c.u64().ok_or(Bad::Short)?
    } else {
        u64::from(reset_count)
    };
    if total_reset_count < u64::from(reset_count) {
        return Err(Bad::Field);
    }
    let pcr_allocation = if version >= 6 {
        PcrAllocation::from_bits(c.u8().ok_or(Bad::Short)?).ok_or(Bad::Field)?
    } else {
        PcrAllocation::default()
    };
    let established = version >= 7 && flag(c.u8().ok_or(Bad::Short)?)?;
    if version >= 8 {
        for seed in [&mut platform_seed, &mut owner_seed, &mut endorsement_seed] {
            let tail = &mut seed[LEGACY_SEED_BYTES..];
            tail.copy_from_slice(c.take(tail.len()).ok_or(Bad::Short)?);
        }
    }
    if !c.buf.is_empty() {
        return Err(Bad::Field);
    }
    Ok(Persistent {
        platform_seed,
        owner_seed,
        endorsement_seed,
        clock_base_ms,
        reset_count,
        restart_count,
        total_reset_count,
        pcr_allocation,
        established,
        orderly,
        hier,
        da,
        nv,
        objects,
    })
}

/// 0 or 1.
fn flag(b: u8) -> Result<bool, Bad> {
    match b {
        0 => Ok(false),
        1 => Ok(true),
        _ => Err(Bad::Field),
    }
}

/// The persistent handle belongs to the range of the object's hierarchy (Part 3, clause 28.5.1:
/// owner and endorsement objects below `PLATFORM_PERSISTENT`, platform objects from it on).
fn persistent_handle_fits(handle: u32, hierarchy: u32) -> bool {
    match hierarchy {
        h::RH_OWNER | h::RH_ENDORSEMENT => {
            (h::PERSISTENT_FIRST..h::PLATFORM_PERSISTENT).contains(&handle)
        }
        h::RH_PLATFORM => (h::PLATFORM_PERSISTENT..=h::PERSISTENT_LAST).contains(&handle),
        _ => false,
    }
}
