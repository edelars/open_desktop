//! `TPM2_ContextSave` and `TPM2_ContextLoad` (slice C2 of phase B, S7-23) for transient objects
//! and sessions. Windows TBS uses them as a resource manager (`docs/specs/WINDOWS_PLATFORM_
//! REQUIREMENTS.md`, K-07, a hypothesis until the S7-32 trace).
//!
//! **Context blob** (Part 1 v184, clauses 28.3.1 and 28.3.2, Equations 50-52; Part 2, clauses
//! 14.5-14.7). `TPMS_CONTEXT = sequence (UINT64) || savedHandle || hierarchy || contextBlob
//! (TPM2B)`, with `contextBlob = integrity (TPM2B) || encContext`:
//!
//! - `encRoot = KDFa(SHA-256, hProof, "ODTPM-CTXENC", resetValue, empty, 256)` binds the encryption
//!   root to this TPM Reset epoch (Part 3 v185, 9.3.1);
//! - `(symKey, symIv) = KDFa(SHA-256, encRoot, "CONTEXT_ENCRYPT", sequence, handle, 256)`: the first
//!   16 octets are the AES-128 key, the next 16 the CFB IV (the key and IV are made after the
//!   integrity check, as the text requires);
//! - `encContext = AES-128-CFB(symKey, symIv, TPM2B_CONTEXT_SENSITIVE)`, the sensitive buffer
//!   being `sequence || body` (the text: the sequence is stored inside before the encryption and
//!   compared after the decryption);
//! - `integrity = HMAC-SHA-256(hProof, resetValue [|| clearCount] || sequence || handle ||
//!   encContext)`; `clearCount` (a volatile counter of the TPM Restarts, not `restartCount`)
//!   only for `savedHandle` `0x80000002` (an object with `stClear`).
//!
//! `hProof` is the proof of the context's hierarchy (`object_cmd.rs`; for a session or a Null
//! hierarchy object `nullProof = KDFa(SHA-256, Null seed, "ODTPM-NULLPROOF", ...)`, which changes
//! on every TPM Reset, as the text asks). `resetValue` (Equation 52) is `KDFa(SHA-256, Null seed,
//! "ODTPM-CTXRESET", ...)`: the Null seed is new on every TPM Reset and kept by Restart and Resume
//! (`hierarchy.rs`), exactly the lifetime the text gives `resetValue` ("a random value that changes
//! on every TPM Reset"), so no value of its own is stored. Every saved context dies with a Reset,
//! an object context with the change of its hierarchy's seed (`TPM2_Clear`) and an endorsement
//! object context with a change of the storage seed too.
//!
//! **Objects.** The context holds the public area, the Qualified Name, the hierarchy and the
//! private part (`TPM2B_SENSITIVE`, the format of `object.rs`); the saved object stays loaded. A
//! context can be loaded any number of times (each load takes an object slot). `savedHandle` is
//! `0x80000000`, or `0x80000002` when the object has `stClear`.
//!
//! **Sessions.** `ContextSave` removes the session from its slot and records `(slot, sequence)`
//! in [`ContextState`]; the slot stays reserved (the handle of a session does not change, Part 1,
//! clause 28.5) and the session counts as active. A saved session loads once (the record is
//! removed), only with the sequence of the last save, and at its old handle; `FlushContext` of the
//! handle forgets it. `sequence` is the context counter, increased by every save and load.
//!
//! **Failure mode** (Part 2, clause 14.5): if the integrity is valid but the decrypted sequence
//! differs from the parameter, the TPM enters failure mode (`TPM_RC_FAILURE`).
//!
//! Not implemented: saving sequence objects (`savedHandle` `0x80000001`), `TPM_RC_CONTEXT_GAP`
//! (the counter is 64 bits and never rolls over), saved sessions in the snapshot (sessions are
//! volatile; only `Shutdown(STATE)` keeps the record).
//!
//! NOT VERIFIED against TPM 2.0 Library v185/v184 (Part 4 is not available; Part 1 and Part 2 were
//! read for the construction): the labels `ODTPM-NULLPROOF` and `ODTPM-CTXRESET` (both internal,
//! never visible outside) and the layout of the plaintext body (ours; the text leaves the vendor
//! structures to the implementation); AES-128-CFB and SHA-256 as "vendor" algorithms
//! (`TPM_PT_CONTEXT_HASH` etc. are not reported); `sequence` in the KDF and
//! HMAC as 8 octets and `handle` as 4 octets (the text says "sequence" and "handle"); the codes
//! `TPM_RC_VALUE` (handle 1) for a handle that cannot be saved, `TPM_RC_HANDLE` (parameter 1) for a
//! session that is not saved or whose sequence is not the last one, `TPM_RC_VALUE` (parameter 1)
//! for a sequence beyond the counters and for an unknown `savedHandle` or hierarchy,
//! `TPM_RC_HIERARCHY` (parameter 1) for a disabled hierarchy, `TPM_RC_SIZE` for a context blob
//! that does not split into integrity and ciphertext, `TPM_RC_OBJECT_MEMORY` without a free slot.

use zeroize::Zeroizing;

use crate::codes::{handle, object_attr};
use crate::crypto::{DIGEST, hmac_sha256, hmac_sha256_verify, kdfa_sha256};
use crate::da::DaClass;
use crate::dispatch::Invocation;
use crate::hash::AuthValue;
use crate::marshal::{Reader, TpmtPublic, Writer, limits};
use crate::object::{
    Object, Sensitive, aes128_cfb, name_of, sensitive_from_bytes, sensitive_to_bytes,
};
use crate::paramenc::SessionSym;
use crate::policy::{PolicyHashRestriction, PolicyState};
use crate::response::{Output, Rc};
use crate::session::{BoundEntity, Session, SessionKind, session_handle};
use crate::state::Tpm;

/// Largest `contextBlob` accepted by `TPM2_ContextLoad` (an RSA object context is under 1 KiB).
const MAX_CONTEXT_BLOB: usize = 2048;

/// `savedHandle` of an object with `stClear` (Part 2, Table 245).
const SAVED_ST_CLEAR: u32 = 0x8000_0002;
/// `savedHandle` of an ordinary transient object.
const SAVED_OBJECT: u32 = 0x8000_0000;

/// Volatile state of the context protection; it travels with `Shutdown(STATE)` (`state.rs`).
#[derive(Clone, Default)]
pub(crate) struct ContextState {
    /// Sequence counter of object contexts.
    object_seq: u64,
    /// Context counter of sessions (`contextID`).
    session_counter: u64,
    /// Saved sessions: `(slot, sequence of the saved context)`.
    saved_sessions: Vec<(usize, u64)>,
    /// `clearCount` (Part 1, clause 28.3): increased on each TPM Restart (`Startup(CLEAR)` after
    /// `Shutdown(STATE)`), unchanged by TPM Resume, zero after TPM Reset. It is not the
    /// `restartCount` of `TPMS_CLOCK_INFO`, which Resume also increases.
    clear_count: u32,
}

impl ContextState {
    /// TPM Restart: the contexts of `stClear` objects lose their validity.
    pub(crate) fn restart(&mut self) {
        self.clear_count = self.clear_count.wrapping_add(1);
    }

    /// Slots of the saved sessions, which stay reserved for them.
    pub(crate) fn reserved_slots(&self) -> Vec<usize> {
        self.saved_sessions.iter().map(|(s, _)| *s).collect()
    }

    /// Number of saved sessions (they count as active sessions).
    pub(crate) fn saved_count(&self) -> usize {
        self.saved_sessions.len()
    }

    /// Handles of the saved sessions for `TPM_CAP_HANDLES` (reported with the HMAC type: the TPM
    /// does not track the session type of a saved session, Part 3, clause 30.2).
    pub(crate) fn saved_handles(&self) -> Vec<u32> {
        self.saved_sessions
            .iter()
            .map(|(s, _)| session_handle(SessionKind::Hmac, *s))
            .collect()
    }

    /// Forgets the saved session in `slot` (`TPM2_FlushContext`); `false` if none is saved there.
    pub(crate) fn forget_saved_session(&mut self, slot: usize) -> bool {
        let before = self.saved_sessions.len();
        self.saved_sessions.retain(|(s, _)| *s != slot);
        self.saved_sessions.len() != before
    }
}

/// What the proof of a context's hierarchy is made of: see [`Tpm::context_proof`].
fn context_blob(
    proof: &[u8],
    reset: &[u8],
    clear_count: Option<u32>,
    sequence: u64,
    saved_handle: u32,
    body: &[u8],
) -> Vec<u8> {
    context_blob_inner(
        proof,
        reset,
        clear_count,
        sequence,
        sequence,
        saved_handle,
        body,
    )
}

/// [`context_blob`] with the sequence stored inside the encrypted part given separately (the
/// two differ only in the test of the failure-mode path).
fn context_blob_inner(
    proof: &[u8],
    reset: &[u8],
    clear_count: Option<u32>,
    sequence: u64,
    inner_sequence: u64,
    saved_handle: u32,
    body: &[u8],
) -> Vec<u8> {
    let (key, iv) = context_key_iv(proof, reset, sequence, saved_handle);
    let mut plain = Zeroizing::new(Vec::with_capacity(2 + 8 + body.len()));
    plain.extend_from_slice(&((8 + body.len()) as u16).to_be_bytes());
    plain.extend_from_slice(&inner_sequence.to_be_bytes());
    plain.extend_from_slice(body);
    let enc = aes128_cfb(&key, &iv, &plain, true);
    let tag = context_integrity(proof, reset, clear_count, sequence, saved_handle, &enc);
    let mut w = Writer::new();
    w.tpm2b(&tag);
    w.bytes(&enc);
    w.finish()
}

fn context_key_iv(
    proof: &[u8],
    reset: &[u8],
    sequence: u64,
    saved_handle: u32,
) -> (Zeroizing<Vec<u8>>, [u8; 16]) {
    let root = kdfa_sha256(proof, b"ODTPM-CTXENC", reset, &[], 256);
    let material = kdfa_sha256(
        &root,
        b"CONTEXT_ENCRYPT",
        &sequence.to_be_bytes(),
        &saved_handle.to_be_bytes(),
        256,
    );
    let key = Zeroizing::new(material[..16].to_vec());
    let mut iv = [0u8; 16];
    iv.copy_from_slice(&material[16..32]);
    (key, iv)
}

fn context_integrity(
    proof: &[u8],
    reset: &[u8],
    clear_count: Option<u32>,
    sequence: u64,
    saved_handle: u32,
    enc: &[u8],
) -> [u8; 32] {
    let clear = clear_count.map(u32::to_be_bytes);
    hmac_sha256(
        proof,
        &[
            reset,
            clear.as_ref().map_or(&[][..], |c| &c[..]),
            &sequence.to_be_bytes(),
            &saved_handle.to_be_bytes(),
            enc,
        ],
    )
}

/// Why a context did not open.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum OpenError {
    /// The blob does not split into integrity and ciphertext.
    Size,
    /// The integrity value does not match.
    Integrity,
    /// Valid integrity but a different sequence inside (failure mode) or a body that is not a
    /// well-formed `TPM2B_CONTEXT_SENSITIVE`.
    Corrupt,
}

/// Verifies the integrity of `blob` and decrypts it; returns the body (after the inner sequence).
fn open_context(
    proof: &[u8],
    reset: &[u8],
    clear_count: Option<u32>,
    sequence: u64,
    saved_handle: u32,
    blob: &[u8],
) -> Result<Zeroizing<Vec<u8>>, OpenError> {
    let mut r = Reader::new(blob);
    let tag = r
        .tpm2b(limits::MAX_DIGEST_SIZE)
        .map_err(|_| OpenError::Size)?;
    let enc = r.bytes(r.remaining()).map_err(|_| OpenError::Size)?;
    if enc.len() < 2 + 8 {
        return Err(OpenError::Size);
    }
    let clear = clear_count.map(u32::to_be_bytes);
    if !hmac_sha256_verify(
        proof,
        &[
            reset,
            clear.as_ref().map_or(&[][..], |c| &c[..]),
            &sequence.to_be_bytes(),
            &saved_handle.to_be_bytes(),
            enc,
        ],
        tag,
    ) {
        return Err(OpenError::Integrity);
    }
    // The key and the IV are made only now, after the integrity check (Part 1, 28.3.1).
    let (key, iv) = context_key_iv(proof, reset, sequence, saved_handle);
    let plain = aes128_cfb(&key, &iv, enc, false);
    let mut p = Reader::new(&plain);
    let n = usize::from(p.u16().map_err(|_| OpenError::Corrupt)?);
    let mut inner = p.sub(n).map_err(|_| OpenError::Corrupt)?;
    if p.remaining() != 0 || inner.u64().map_err(|_| OpenError::Corrupt)? != sequence {
        return Err(OpenError::Corrupt);
    }
    let body = inner
        .bytes(inner.remaining())
        .map_err(|_| OpenError::Corrupt)?;
    Ok(Zeroizing::new(body.to_vec()))
}

// ------------------------------------------------------------------------------------- bodies

fn encode_object(o: &Object) -> Result<Zeroizing<Vec<u8>>, Rc> {
    let mut w = Writer {
        buf: Vec::with_capacity(1024),
    };
    w.u8(u8::from(o.sensitive.is_some()));
    w.tpm2b(&o.public.to_bytes());
    w.tpm2b(&o.qualified_name);
    w.u32(o.hierarchy);
    if let Some(s) = &o.sensitive {
        let bytes = sensitive_to_bytes(s, o.public.type_alg())?;
        w.bytes(&bytes);
    }
    Ok(Zeroizing::new(w.buf))
}

fn decode_object(body: &[u8]) -> Result<Object, Rc> {
    let mut r = Reader::new(body);
    let has_sensitive = r.u8()?;
    let public_bytes = r.tpm2b(limits::MAX_COMMAND_SIZE)?;
    let mut pr = Reader::new(public_bytes);
    let public = TpmtPublic::unmarshal(&mut pr)?;
    pr.finish()?;
    let qualified_name = r.tpm2b(limits::MAX_DIGEST_SIZE + 2)?.to_vec();
    let hierarchy = r.u32()?;
    let sensitive: Option<Sensitive> = match has_sensitive {
        0 => None,
        1 => {
            let rest = r.bytes(r.remaining())?;
            Some(sensitive_from_bytes(rest, &public)?)
        }
        _ => return Err(Rc::VALUE),
    };
    if has_sensitive == 0 {
        r.finish()?;
    }
    Ok(Object {
        name: name_of(&public),
        public,
        qualified_name,
        hierarchy,
        sensitive,
    })
}

fn encode_session(s: &Session) -> Zeroizing<Vec<u8>> {
    let mut w = Writer {
        buf: Vec::with_capacity(256),
    };
    w.u8(match s.kind {
        SessionKind::Hmac => 0,
        SessionKind::Policy => 1,
        SessionKind::Trial => 2,
    });
    w.tpm2b(&s.nonce_tpm);
    w.tpm2b(s.key.as_slice());
    match &s.bound {
        Some(b) => {
            w.u8(1);
            w.u32(b.handle);
            w.tpm2b(b.auth.as_slice());
        }
        None => w.u8(0),
    }
    let p = &s.policy;
    w.bytes(&p.digest);
    for v in [p.command_code, p.pcr_update_counter] {
        match v {
            Some(v) => {
                w.u8(1);
                w.u32(v);
            }
            None => w.u8(0),
        }
    }
    if let Some(hash) = &p.hash_restriction {
        let (kind, digest) = match hash {
            PolicyHashRestriction::CpHash(digest) => (1, digest),
            PolicyHashRestriction::NameHash(digest) => (2, digest),
            PolicyHashRestriction::TemplateHash(digest) => (3, digest),
            PolicyHashRestriction::ParametersHash(digest) => (4, digest),
        };
        w.u8(kind);
        w.bytes(digest);
    } else {
        w.u8(0);
    }
    let bind_da = match s.bind_da {
        DaClass::Exempt => 0,
        DaClass::Protected => 1,
        DaClass::Lockout => 2,
    };
    w.u8(u8::from(p.auth_value_needed) | (u8::from(p.password_needed) << 1) | (bind_da << 2));
    w.u8(s.symmetric.to_octet());
    if let Some(digest) = s.audit_digest {
        w.u8(1); // Version of the optional audit tail.
        w.bytes(&digest);
    } else if p.command_locality != 0 {
        w.u8(2); // Optional locality tail for policy sessions.
        w.u8(p.command_locality);
    }
    Zeroizing::new(w.buf)
}

fn decode_session(body: &[u8]) -> Result<Session, Rc> {
    let mut r = Reader::new(body);
    let kind = match r.u8()? {
        0 => SessionKind::Hmac,
        1 => SessionKind::Policy,
        2 => SessionKind::Trial,
        _ => return Err(Rc::VALUE),
    };
    let nonce_tpm = r.tpm2b(limits::MAX_DIGEST_SIZE)?.to_vec();
    let key = AuthValue::from_slice(r.tpm2b(limits::MAX_DIGEST_SIZE)?).ok_or(Rc::SIZE)?;
    let bound = match r.u8()? {
        0 => None,
        1 => {
            let h = r.u32()?;
            let auth = AuthValue::from_slice(r.tpm2b(limits::MAX_DIGEST_SIZE)?).ok_or(Rc::SIZE)?;
            Some(BoundEntity { handle: h, auth })
        }
        _ => return Err(Rc::VALUE),
    };
    let mut policy = PolicyState::new();
    policy.digest.copy_from_slice(r.bytes(32)?);
    let opt_u32 = |r: &mut Reader<'_>| -> Result<Option<u32>, Rc> {
        match r.u8()? {
            0 => Ok(None),
            1 => Ok(Some(r.u32()?)),
            _ => Err(Rc::VALUE),
        }
    };
    policy.command_code = opt_u32(&mut r)?;
    policy.pcr_update_counter = opt_u32(&mut r)?;
    policy.hash_restriction = match r.u8()? {
        0 => None,
        tag @ (1..=4) => {
            let mut h = [0u8; 32];
            h.copy_from_slice(r.bytes(32)?);
            Some(match tag {
                1 => PolicyHashRestriction::CpHash(h),
                2 => PolicyHashRestriction::NameHash(h),
                3 => PolicyHashRestriction::TemplateHash(h),
                4 => PolicyHashRestriction::ParametersHash(h),
                _ => unreachable!(),
            })
        }
        _ => return Err(Rc::VALUE),
    };
    let flags = r.u8()?;
    let bind_da = match flags >> 2 {
        // Legacy HMAC bodies retain the bound handle. Policy and audit
        // bodies from that format do not record bind provenance.
        0 if bound
            .as_ref()
            .is_some_and(|b| b.handle == handle::RH_LOCKOUT) =>
        {
            DaClass::Lockout
        }
        0 => DaClass::Exempt,
        1 => DaClass::Protected,
        2 => DaClass::Lockout,
        _ => return Err(Rc::VALUE),
    };
    if bind_da != DaClass::Exempt && key.as_slice().len() != DIGEST {
        return Err(Rc::VALUE);
    }
    policy.auth_value_needed = flags & 1 != 0;
    policy.password_needed = flags & 2 != 0;
    let symmetric = SessionSym::from_octet(r.u8()?).ok_or(Rc::VALUE)?;
    let mut audit_digest = None;
    if r.remaining() != 0 {
        match r.u8()? {
            1 if kind == SessionKind::Hmac && bound.is_none() => {
                audit_digest = Some(r.bytes(DIGEST)?.try_into().map_err(|_| Rc::SIZE)?);
            }
            2 if kind == SessionKind::Policy => {
                policy.command_locality = r.u8()?;
                if policy.command_locality == 0 {
                    return Err(Rc::VALUE);
                }
            }
            _ => return Err(Rc::VALUE),
        }
    }
    r.finish()?;
    Ok(Session {
        nonce_tpm,
        key,
        bound,
        bind_da,
        kind,
        policy,
        symmetric,
        audit_digest,
    })
}

impl Tpm {
    /// Reset-bound material for the encryption root and the context integrity tag.
    fn context_reset_value(&self) -> Zeroizing<Vec<u8>> {
        kdfa_sha256(&self.hv.null_seed[..], b"ODTPM-CTXRESET", &[], &[], 256)
    }

    /// The proof of the context's hierarchy. Null objects and sessions use a `nullProof`
    /// derived from the Null seed, which a TPM Reset replaces.
    fn context_proof(&self, hierarchy: u32) -> Zeroizing<Vec<u8>> {
        self.proof(hierarchy).unwrap_or_else(|| self.null_proof())
    }

    /// `TPM2_ContextSave` (Part 3, clause 28.2).
    pub(crate) fn cmd_context_save(
        &mut self,
        inv: &Invocation,
        p: &mut Reader<'_>,
    ) -> Result<Output, Rc> {
        p.finish()?;
        let h = inv.handles[0];
        let (saved_handle, hierarchy, sequence, body, st_clear) = match handle::handle_type(h) {
            handle::HT_TRANSIENT => {
                let Some(o) = self.objects.get(h) else {
                    // A sequence object cannot be saved; an unloaded handle is not loaded.
                    return Err(if self.seq_slot(h, 0).is_ok() {
                        Rc::VALUE.in_handle(1)
                    } else {
                        Rc::reference_handle(0)
                    });
                };
                let st_clear = o.has(object_attr::ST_CLEAR);
                let saved = if st_clear {
                    SAVED_ST_CLEAR
                } else {
                    SAVED_OBJECT
                };
                let body = encode_object(o)?;
                let hierarchy = o.hierarchy;
                self.context.object_seq = self.context.object_seq.wrapping_add(1);
                (saved, hierarchy, self.context.object_seq, body, st_clear)
            }
            handle::HT_HMAC_SESSION | handle::HT_POLICY_SESSION => {
                let slot = self.sessions.slot_of(h).ok_or(Rc::HANDLE.in_handle(1))?;
                let body = encode_session(self.sessions.get(slot).ok_or(Rc::HANDLE.in_handle(1))?);
                self.context.session_counter = self.context.session_counter.wrapping_add(1);
                let sequence = self.context.session_counter;
                self.sessions.park(slot);
                self.context.saved_sessions.push((slot, sequence));
                (h, handle::RH_NULL, sequence, body, false)
            }
            _ => return Err(Rc::VALUE.in_handle(1)),
        };
        let proof = self.context_proof(hierarchy);
        let clear = st_clear.then_some(self.context.clear_count);
        let blob = context_blob(
            &proof,
            &self.context_reset_value(),
            clear,
            sequence,
            saved_handle,
            &body,
        );
        let mut w = Writer::new();
        w.u64(sequence);
        w.u32(saved_handle);
        w.u32(hierarchy);
        w.tpm2b(&blob);
        Ok(Output {
            handles: Vec::new(),
            params: w.finish(),
        })
    }

    /// `TPM2_ContextLoad` (Part 3, clause 28.3).
    pub(crate) fn cmd_context_load(&mut self, p: &mut Reader<'_>) -> Result<Output, Rc> {
        p.set_param(1);
        let sequence = p.u64()?;
        let saved_handle = p.u32()?;
        let hierarchy = p.u32()?;
        let blob = p.tpm2b(MAX_CONTEXT_BLOB)?;
        p.finish()?;
        let session = matches!(
            handle::handle_type(saved_handle),
            handle::HT_HMAC_SESSION | handle::HT_POLICY_SESSION
        );
        if !session && saved_handle != SAVED_OBJECT && saved_handle != SAVED_ST_CLEAR {
            return Err(Rc::VALUE.in_param(1));
        }
        if !crate::object::is_object_hierarchy(hierarchy)
            || (session && hierarchy != handle::RH_NULL)
        {
            return Err(Rc::VALUE.in_param(1));
        }
        if !self.permanent_enabled(hierarchy) {
            return Err(Rc::HIERARCHY.in_param(1));
        }
        let slot = (saved_handle & 0x00FF_FFFF) as usize;
        if session {
            let tracked = self
                .context
                .saved_sessions
                .iter()
                .find(|(s, _)| *s == slot)
                .map(|(_, q)| *q);
            if tracked != Some(sequence) {
                // Not saved, saved again later, or already loaded (a sequence beyond the
                // counter is among these: the record holds only sequences that were issued).
                return Err(Rc::HANDLE.in_param(1));
            }
        } else if sequence > self.context.object_seq {
            return Err(Rc::VALUE.in_param(1));
        } else if self.objects.free_slots() == 0 {
            return Err(Rc::OBJECT_MEMORY);
        }
        let proof = self.context_proof(hierarchy);
        let clear = (saved_handle == SAVED_ST_CLEAR).then_some(self.context.clear_count);
        let body = match open_context(
            &proof,
            &self.context_reset_value(),
            clear,
            sequence,
            saved_handle,
            blob,
        ) {
            Ok(b) => b,
            Err(OpenError::Size) => return Err(Rc::SIZE.in_param(1)),
            Err(OpenError::Integrity) => return Err(Rc::INTEGRITY.in_param(1)),
            Err(OpenError::Corrupt) => {
                self.failed = true;
                return Err(Rc::FAILURE);
            }
        };
        if session {
            let s = decode_session(&body).map_err(|_| {
                self.failed = true;
                Rc::FAILURE
            })?;
            if s.kind.handle_type() != handle::handle_type(saved_handle) {
                self.failed = true;
                return Err(Rc::FAILURE);
            }
            self.context.forget_saved_session(slot);
            self.context.session_counter = self.context.session_counter.wrapping_add(1);
            if !self.sessions.insert_at(slot, s) {
                self.failed = true;
                return Err(Rc::FAILURE);
            }
            let mut handles = Writer::new();
            handles.u32(saved_handle);
            return Ok(Output {
                handles: handles.finish(),
                params: Vec::new(),
            });
        }
        let o = decode_object(&body).map_err(|_| {
            self.failed = true;
            Rc::FAILURE
        })?;
        if o.hierarchy != hierarchy
            || o.has(object_attr::ST_CLEAR) != (saved_handle == SAVED_ST_CLEAR)
        {
            self.failed = true;
            return Err(Rc::FAILURE);
        }
        let new_handle = self.objects.insert(o).ok_or(Rc::OBJECT_MEMORY)?;
        let mut handles = Writer::new();
        handles.u32(new_handle);
        Ok(Output {
            handles: handles.finish(),
            params: Vec::new(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PROOF: [u8; 32] = [3; 32];
    const RESET: [u8; 32] = [4; 32];

    #[test]
    fn legacy_session_bodies_recover_da_only_when_the_bound_handle_survived() {
        for (kind, bound, expected) in [
            (0, Some(handle::RH_LOCKOUT), DaClass::Lockout),
            (0, Some(handle::RH_OWNER), DaClass::Exempt),
            (1, None, DaClass::Exempt),
        ] {
            let mut w = Writer::new();
            w.u8(kind);
            w.tpm2b(&[1; 16]);
            w.tpm2b(&[2; 32]);
            if let Some(bound) = bound {
                w.u8(1);
                w.u32(bound);
                w.tpm2b(b"old");
            } else {
                w.u8(0);
            }
            w.bytes(&[0; 32]);
            w.bytes(&[0, 0, 0, 0, SessionSym::Null.to_octet()]);
            let mut legacy = w.finish();
            let session = decode_session(&legacy).unwrap();
            assert_eq!(session.bind_da, expected);
            if expected == DaClass::Exempt {
                assert_eq!(&encode_session(&session)[..], legacy);
            }
            let flags = legacy.len() - 2;
            legacy[flags] = 0x0c;
            assert!(matches!(decode_session(&legacy), Err(Rc::VALUE)));
        }
    }

    #[test]
    fn a_context_opens_only_with_the_values_it_was_made_with() {
        let blob = context_blob(&PROOF, &RESET, None, 7, 0x8000_0000, b"body");
        let open = |proof: &[u8], reset: &[u8], clear: Option<u32>, seq: u64, h: u32, b: &[u8]| {
            open_context(proof, reset, clear, seq, h, b)
        };
        assert_eq!(
            &open(&PROOF, &RESET, None, 7, 0x8000_0000, &blob).unwrap()[..],
            b"body"
        );
        // Another proof, resetValue, clearCount, sequence or handle: integrity failure.
        for (proof, reset, clear, seq, h) in [
            ([5u8; 32], RESET, None, 7, 0x8000_0000),
            (PROOF, [5u8; 32], None, 7, 0x8000_0000),
            (PROOF, RESET, Some(0), 7, 0x8000_0000),
            (PROOF, RESET, None, 8, 0x8000_0000),
            (PROOF, RESET, None, 7, 0x8000_0002),
        ] {
            assert_eq!(
                open(&proof, &reset, clear, seq, h, &blob),
                Err(OpenError::Integrity)
            );
        }
        // clearCount is part of the integrity value of an stClear context.
        let st = context_blob(&PROOF, &RESET, Some(2), 7, 0x8000_0002, b"body");
        assert!(open(&PROOF, &RESET, Some(2), 7, 0x8000_0002, &st).is_ok());
        assert_eq!(
            open(&PROOF, &RESET, Some(3), 7, 0x8000_0002, &st),
            Err(OpenError::Integrity)
        );
        // The plaintext is not in the blob.
        assert!(!blob.windows(4).any(|w| w == b"body"));
        // Truncations: no integrity value, no ciphertext.
        assert_eq!(
            open(&PROOF, &RESET, None, 7, 0x8000_0000, &[]),
            Err(OpenError::Size)
        );
        assert_eq!(
            open(&PROOF, &RESET, None, 7, 0x8000_0000, &blob[..34]),
            Err(OpenError::Size)
        );
    }

    #[test]
    fn a_valid_integrity_value_with_another_inner_sequence_is_corruption() {
        // Part 2, clause 14.5: valid integrity but a decrypted sequence that differs from the
        // parameter means failure mode (the caller sets it on `OpenError::Corrupt`).
        let blob = context_blob_inner(&PROOF, &RESET, None, 7, 8, 0x8000_0000, b"body");
        assert_eq!(
            open_context(&PROOF, &RESET, None, 7, 0x8000_0000, &blob),
            Err(OpenError::Corrupt)
        );
    }
}
