//! SHA-256, SHA-384 and SHA-512 hashing, SHA-256 HMAC, and their sequences.
//!
//! Sequence objects are transient objects (handle type `TPM_HT_TRANSIENT`), at most
//! [`MAX_SEQUENCES`] at a time; `TPM_RC_OBJECT_MEMORY` when no slot is free.
//!
//! **`TPM2_HMAC`** (Part 3 v184, clause 15.5) computes HMAC-SHA-256 with the key of an unrestricted
//! keyed-hash object that has `sign` SET (`TPM_RC_KEY` without `sign`, `TPM_RC_TYPE` for another
//! object type, `TPM_RC_ATTRIBUTES` for a restricted key). The key is created by `TPM2_Create` or
//! `TPM2_CreatePrimary` from a `KEYEDHASH` template with `sign` and scheme HMAC (SHA-256) or NULL
//! (`object.rs`): the key bytes come from `inSensitive.data` (at most 128 octets; `sensitiveDataOrigin`
//! CLEAR) or the TPM generates 32 octets (data empty, `sensitiveDataOrigin` SET). `hashAlg` is the
//! hash of the key's scheme or NULL, or SHA-256 for a key with a NULL scheme (`TPM_RC_VALUE`, hash
//! selection matrix of Part 3, Table 75); another algorithm is `TPM_RC_HASH`. The TPM may
//! implement `TPM2_HMAC` or `TPM2_MAC`, not both (same command code): this TPM implements
//! `TPM2_HMAC`. The Windows requirement for `TPM2_HMAC` is T-03 of
//! `docs/specs/WINDOWS_PLATFORM_REQUIREMENTS.md` (by Microsoft documentation; use is a hypothesis
//! until the S7-32 trace).
//!
//! **ML-DSA sequences (slice 7).** Pure ML-DSA signs the message, not a digest, so the sign and verify
//! sequences of an ML-DSA key buffer the message (at most [`MAX_MLDSA_MESSAGE`] octets, then
//! `TPM_RC_SIZE`) and keep the context string of the start command (up to 255 octets); the key is
//! asked to sign or verify at completion.
//!
//! Limitations recorded in `docs/specs/VTPM.md`:
//! - NOT VERIFIED against TPM 2.0 Library v185: `TPM_RC_MODE` for a sequence of the wrong kind
//!   carries the handle number here; whether sequence objects are exempt from dictionary attack
//!   protection (a wrong password is answered `TPM_RC_BAD_AUTH`, there is no DA in phase A).

use hmac::{Hmac, Mac};
use sha2::digest::common::hazmat::{SerializableState, SerializedState};
use sha2::{Digest, Sha256, Sha384, Sha512};
use zeroize::{Zeroize, Zeroizing};

use crate::codes::{TPM_GENERATED_VALUE, alg, handle, object_attr, st};
use crate::context::MAX_CONTEXT_BLOB;
use crate::crypto::{
    DIGEST, HashAlg, HmacSha256Stream, hash, hmac_sha256, hmac_sha256_state, hmac_sha256_verify,
};
use crate::dispatch::Invocation;
use crate::marshal::{
    PublicParams, Reader, Scheme, TpmtSignature, TpmtTkHashcheck, Writer, digest_size, limits,
};
use crate::mldsa;
use crate::object::{Secret, find_object};
use crate::object_cmd::{
    VerifiedTicket, check_signature_scheme, fixed_digest_scheme, mldsa_message_signature,
    sign_digest_output, signature_output,
};
use crate::pcr::{EventHash, resolve_pcr_handle};
use crate::response::{Output, Rc};
use crate::state::Tpm;

/// The longest message of an ML-DSA sign or verify sequence, in octets: pure ML-DSA signs the whole
/// message and the primitive takes it in one piece, so the sequence buffers it (the largest command
/// the TPM accepts, `limits::MAX_COMMAND_SIZE`, is also the largest message). Choice of this
/// implementation, NOT VERIFIED against the profile: a longer message is `TPM_RC_SIZE`.
pub const MAX_MLDSA_MESSAGE: usize = limits::MAX_COMMAND_SIZE;

/// Sequence objects held at once (`TPM_PT_HR_TRANSIENT_MIN` is 3 in PTP 1.07, Table 2).
pub const MAX_SEQUENCES: usize = 3;

/// `TPMI_RH_HIERARCHY+` (Part 2, Table 2x): owner, platform, endorsement or null.
fn check_hierarchy(r: &Reader<'_>, h: u32) -> Result<u32, Rc> {
    match h {
        handle::RH_OWNER | handle::RH_PLATFORM | handle::RH_ENDORSEMENT | handle::RH_NULL => Ok(h),
        _ => Err(r.err(Rc::VALUE)),
    }
}

const GENERATED_PREFIX: [u8; 4] = TPM_GENERATED_VALUE.to_be_bytes();

fn safe_hashcheck_prefix(data: &[u8]) -> bool {
    data.len() >= GENERATED_PREFIX.len() && !data.starts_with(&GENERATED_PREFIX)
}

fn hashcheck_parts(digest: &[u8]) -> [&[u8]; 2] {
    const TAG: [u8; 2] = st::HASHCHECK.to_be_bytes();
    [&TAG, digest]
}

/// Authorization value (at most one SHA-256 digest long, [`DIGEST`]; a hash sequence of SHA-384 or
/// SHA-512 may carry one digest of its own hash, see [`AuthValue::from_slice_up_to`]); wiped on
/// drop. The storage fits the largest digest ([`limits::MAX_HASH_SIZE`]).
#[derive(Clone)]
pub(crate) struct AuthValue {
    len: usize,
    bytes: [u8; limits::MAX_HASH_SIZE],
}

impl AuthValue {
    pub(crate) fn empty() -> AuthValue {
        AuthValue {
            len: 0,
            bytes: [0; limits::MAX_HASH_SIZE],
        }
    }

    pub(crate) fn from_slice(v: &[u8]) -> Option<AuthValue> {
        AuthValue::from_slice_up_to(v, DIGEST)
    }

    /// [`AuthValue::from_slice`] with the size limit `max` (at most [`limits::MAX_HASH_SIZE`]).
    pub(crate) fn from_slice_up_to(v: &[u8], max: usize) -> Option<AuthValue> {
        if v.len() > max.min(limits::MAX_HASH_SIZE) {
            return None;
        }
        let mut a = AuthValue::empty();
        a.bytes[..v.len()].copy_from_slice(v);
        a.len = v.len();
        Some(a)
    }

    /// Like [`AuthValue::from_slice`] but without trailing zero octets: "trailing zeros are always
    /// removed from an authValue before it is used in an authorization computation" (Part 1 v184,
    /// clause 17.6.5). The size limit applies to the value as given.
    pub(crate) fn from_slice_trimmed(v: &[u8]) -> Option<AuthValue> {
        AuthValue::from_slice_trimmed_up_to(v, DIGEST)
    }

    /// [`AuthValue::from_slice_trimmed`] with the size limit `max` (the digest size of the
    /// `nameAlg` of an object; at most [`limits::MAX_HASH_SIZE`]).
    pub(crate) fn from_slice_trimmed_up_to(v: &[u8], max: usize) -> Option<AuthValue> {
        if v.len() > max {
            return None;
        }
        AuthValue::from_slice_up_to(trim(v), max)
    }

    pub(crate) fn as_slice(&self) -> &[u8] {
        &self.bytes[..self.len]
    }
}

impl Drop for AuthValue {
    fn drop(&mut self) {
        self.bytes.zeroize();
        self.len = 0;
    }
}

/// Octets of a password without trailing zero octets (Part 1, clause 17.4: the TPM truncates
/// trailing zeros of both values before comparing).
pub(crate) fn trim(v: &[u8]) -> &[u8] {
    let end = v.iter().rposition(|b| *b != 0).map_or(0, |i| i + 1);
    &v[..end]
}

/// Password check without an early exit on the first differing octet.
pub(crate) fn password_matches(given: &[u8], auth: &[u8]) -> bool {
    let (g, a) = (trim(given), trim(auth));
    let mut diff = (g.len() ^ a.len()) as u32;
    for i in 0..limits::MAX_HASH_SIZE {
        let x = g.get(i).copied().unwrap_or(0);
        let y = a.get(i).copied().unwrap_or(0);
        diff |= u32::from(x ^ y);
    }
    diff == 0
}

#[derive(Clone, Copy, Default)]
struct MessagePrefix {
    bytes: [u8; GENERATED_PREFIX.len()],
    len: usize,
}

impl MessagePrefix {
    fn append(&mut self, data: &[u8]) {
        let count = (self.bytes.len() - self.len).min(data.len());
        self.bytes[self.len..self.len + count].copy_from_slice(&data[..count]);
        self.len += count;
    }

    fn is_generated(self) -> bool {
        self.len == GENERATED_PREFIX.len() && self.bytes == GENERATED_PREFIX
    }
}

/// The running hash of a sign or verify sequence: the hash of the signing scheme of the key
/// (SHA-256, SHA-384 or SHA-512), or the buffered message of an ML-DSA key (which signs the
/// message, not a digest).
#[derive(Clone)]
enum SchemeDigest {
    Sha256(Sha256),
    Sha384(Sha384),
    Sha512(Sha512),
    Message(Zeroizing<Vec<u8>>),
}

impl SchemeDigest {
    /// `None` for a hash that is not a signing hash.
    fn new(hash: u16) -> Option<SchemeDigest> {
        match hash {
            alg::SHA256 => Some(Self::Sha256(Sha256::new())),
            alg::SHA384 => Some(Self::Sha384(Sha384::new())),
            alg::SHA512 => Some(Self::Sha512(Sha512::new())),
            _ => None,
        }
    }

    /// Appends `data`; `Err` (nothing appended) when a buffered message would pass
    /// [`MAX_MLDSA_MESSAGE`].
    fn update(&mut self, data: &[u8]) -> Result<(), ()> {
        match self {
            Self::Sha256(hash) => hash.update(data),
            Self::Sha384(hash) => hash.update(data),
            Self::Sha512(hash) => hash.update(data),
            Self::Message(message) => {
                if message.len() + data.len() > MAX_MLDSA_MESSAGE {
                    return Err(());
                }
                message.extend_from_slice(data);
            }
        }
        Ok(())
    }

    /// The buffered message and `data` (`None` for a hash, or past [`MAX_MLDSA_MESSAGE`]).
    fn message_with(&self, data: &[u8]) -> Option<Zeroizing<Vec<u8>>> {
        let Self::Message(message) = self else {
            return None;
        };
        if message.len() + data.len() > MAX_MLDSA_MESSAGE {
            return None;
        }
        let mut whole = Zeroizing::new(Vec::with_capacity(message.len() + data.len()));
        whole.extend_from_slice(message);
        whole.extend_from_slice(data);
        Some(whole)
    }

    /// The digest of everything so far and `data`, leaving the running state as it is.
    fn finalize_with(&self, data: &[u8]) -> Vec<u8> {
        let mut state = self.clone();
        let _ = state.update(data);
        match state {
            Self::Sha256(hash) => hash.finalize().to_vec(),
            Self::Sha384(hash) => hash.finalize().to_vec(),
            Self::Sha512(hash) => hash.finalize().to_vec(),
            // Never a digest: an ML-DSA sequence is completed by `message_with`.
            Self::Message(message) => message.to_vec(),
        }
    }
}

pub(crate) struct SignSequence {
    key_name: Vec<u8>,
    scheme: Scheme,
    digest: SchemeDigest,
    prefix: MessagePrefix,
    /// The context string of an ML-DSA key (empty for every other key).
    context: Vec<u8>,
}

// A public key with the same Name may be reloaded under any of these hierarchies.
const VERIFIED_HIERARCHIES: [u32; 3] = [
    handle::RH_PLATFORM,
    handle::RH_OWNER,
    handle::RH_ENDORSEMENT,
];

struct MessageTicketState {
    proof_id: [u8; DIGEST],
    mac: HmacSha256Stream,
}

pub(crate) struct VerifySequence {
    key_name: Vec<u8>,
    scheme: Scheme,
    digest: SchemeDigest,
    context: Vec<u8>,
    tickets: [MessageTicketState; VERIFIED_HIERARCHIES.len()],
}

pub(crate) enum SequenceKind {
    /// Hash sequence of one algorithm (SHA-256).
    Hash(Sha256),
    /// Hash sequence of one algorithm (SHA-384).
    Hash384(Sha384),
    /// Hash sequence of one algorithm (SHA-512).
    Hash512(Sha512),
    /// Incremental HMAC-SHA-256 state, independent of the original key handle.
    Hmac(Hmac<Sha256>),
    /// Event sequence with both PCR bank algorithms.
    Event(EventHash),
    Sign(SignSequence),
    Verify(Box<VerifySequence>),
    /// Reserved for platform indications. It has no guest handle.
    Hcrtm(EventHash),
}

pub(crate) struct Sequence {
    pub kind: SequenceKind,
    pub auth: AuthValue,
    /// `None` until the first hash buffer has been accepted (Part 3 v185 §17.8.1).
    first_buffer_safe: Option<bool>,
}

/// Transient sequence slots.
#[derive(Default)]
pub(crate) struct Sequences {
    slots: [Option<Sequence>; MAX_SEQUENCES],
}

impl Sequences {
    pub(crate) fn clear(&mut self) {
        self.slots = Default::default();
    }

    pub(crate) fn loaded_handles(&self) -> Vec<u32> {
        (0..MAX_SEQUENCES)
            .filter(|i| self.is_guest_sequence(*i))
            .map(seq_handle)
            .collect()
    }

    pub(crate) fn free_slots(&self) -> usize {
        self.slots.iter().filter(|s| s.is_none()).count()
    }

    fn is_guest_sequence(&self, slot: usize) -> bool {
        self.slots.get(slot).is_some_and(|sequence| {
            sequence
                .as_ref()
                .is_some_and(|s| !matches!(s.kind, SequenceKind::Hcrtm(_)))
        })
    }

    fn hcrtm_slot(&self) -> Option<usize> {
        self.slots.iter().position(|s| {
            s.as_ref()
                .is_some_and(|s| matches!(s.kind, SequenceKind::Hcrtm(_)))
        })
    }

    pub(crate) fn hcrtm_active(&self) -> bool {
        self.hcrtm_slot().is_some()
    }

    pub(crate) fn start_hcrtm(&mut self) {
        // The platform indication must succeed. At exhaustion replace the last sequence slot.
        let slot = self
            .hcrtm_slot()
            .or_else(|| self.slots.iter().position(Option::is_none))
            .unwrap_or(MAX_SEQUENCES - 1);
        self.slots[slot] = Some(Sequence {
            kind: SequenceKind::Hcrtm(EventHash::default()),
            auth: AuthValue::empty(),
            first_buffer_safe: None,
        });
    }

    pub(crate) fn update_hcrtm(&mut self, data: &[u8]) {
        if let Some(slot) = self.hcrtm_slot()
            && let Some(Sequence {
                kind: SequenceKind::Hcrtm(hash),
                ..
            }) = &mut self.slots[slot]
        {
            hash.update(data);
        }
    }

    pub(crate) fn take_hcrtm(&mut self) -> Option<EventHash> {
        let slot = self.hcrtm_slot()?;
        match self.slots[slot].take()?.kind {
            SequenceKind::Hcrtm(hash) => Some(hash),
            _ => None,
        }
    }

    /// The saveable sequence in `slot` (a guest sequence, not the platform one).
    pub(crate) fn get(&self, slot: usize) -> Option<&Sequence> {
        if self.is_guest_sequence(slot) {
            self.slots[slot].as_ref()
        } else {
            None
        }
    }

    /// Stores a sequence of a loaded context in the lowest free slot; `None` without a free slot.
    pub(crate) fn insert(&mut self, sequence: Sequence) -> Option<usize> {
        let slot = self.slots.iter().position(Option::is_none)?;
        self.slots[slot] = Some(sequence);
        Some(slot)
    }

    /// Flushes the sequence in `slot` (`TPM2_FlushContext`).
    pub(crate) fn release(&mut self, slot: usize) {
        if let Some(s) = self.slots.get_mut(slot) {
            *s = None;
        }
    }

    fn start(&mut self, kind: SequenceKind, auth: AuthValue) -> Result<Output, Rc> {
        let slot = self
            .slots
            .iter()
            .position(|s| s.is_none())
            .ok_or(Rc::OBJECT_MEMORY)?;
        self.slots[slot] = Some(Sequence {
            kind,
            auth,
            first_buffer_safe: None,
        });
        Ok(Output {
            handles: seq_handle(slot).to_be_bytes().to_vec(),
            params: Vec::new(),
        })
    }
}

pub(crate) fn seq_handle(slot: usize) -> u32 {
    (u32::from(handle::HT_TRANSIENT) << 24) | slot as u32
}

// ------------------------------------------------------------------- saved contexts

/// Tags of the body of a saved sequence (`context.rs`; the layout is ours: the text leaves the body
/// to the implementation). A hash sequence of SHA-256, SHA-384 and SHA-512, a sign and a verify
/// sequence; the HMAC, event and platform sequences are not saved.
const BODY_HASH256: u8 = 1;
const BODY_HASH384: u8 = 2;
const BODY_HASH512: u8 = 3;
const BODY_SIGN: u8 = 4;
const BODY_VERIFY: u8 = 5;

/// The largest serialized hash state (SHA-512: 8 words, the block buffer and the counters).
const MAX_HASH_STATE: usize = 256;

/// A hash state as a `TPM2B`; the serialized copy is wiped (it holds the buffered message octets).
fn put_state<D: SerializableState>(w: &mut Writer, state: &D) {
    let mut bytes = state.serialize();
    w.tpm2b(bytes.as_slice());
    bytes.as_mut_slice().zeroize();
}

fn get_state<D: SerializableState>(r: &mut Reader<'_>) -> Result<D, Rc> {
    let state = SerializedState::<D>::try_from(r.tpm2b(MAX_HASH_STATE)?).map_err(|_| Rc::SIZE)?;
    D::deserialize(&state).map_err(|_| Rc::VALUE)
}

impl SchemeDigest {
    fn write(&self, w: &mut Writer) {
        match self {
            Self::Sha256(hash) => {
                w.u8(0);
                put_state(w, hash);
            }
            Self::Sha384(hash) => {
                w.u8(1);
                put_state(w, hash);
            }
            Self::Sha512(hash) => {
                w.u8(2);
                put_state(w, hash);
            }
            Self::Message(message) => {
                w.u8(3);
                w.u32(message.len() as u32);
                w.bytes(message);
            }
        }
    }

    fn read(r: &mut Reader<'_>) -> Result<Self, Rc> {
        Ok(match r.u8()? {
            0 => Self::Sha256(get_state(r)?),
            1 => Self::Sha384(get_state(r)?),
            2 => Self::Sha512(get_state(r)?),
            3 => {
                let n = r.u32()? as usize;
                if n > MAX_MLDSA_MESSAGE {
                    return Err(Rc::SIZE);
                }
                Self::Message(Zeroizing::new(r.bytes(n)?.to_vec()))
            }
            _ => return Err(Rc::VALUE),
        })
    }

    /// Whether this running hash is the one `scheme` names (a message buffer for the NULL scheme of
    /// an ML-DSA key).
    fn matches(&self, scheme: Scheme) -> bool {
        match self {
            Self::Sha256(_) => scheme.alg != alg::NULL && scheme.hash == alg::SHA256,
            Self::Sha384(_) => scheme.alg != alg::NULL && scheme.hash == alg::SHA384,
            Self::Sha512(_) => scheme.alg != alg::NULL && scheme.hash == alg::SHA512,
            Self::Message(_) => scheme.alg == alg::NULL,
        }
    }
}

/// The common head of a sign and a verify sequence: the Name of the key, the scheme, the running
/// hash and the ML-DSA context string.
fn write_signing_head(
    w: &mut Writer,
    key_name: &[u8],
    scheme: Scheme,
    digest: &SchemeDigest,
    context: &[u8],
) {
    w.tpm2b(key_name);
    w.u16(scheme.alg);
    w.u16(scheme.hash);
    digest.write(w);
    w.tpm2b(context);
}

type SigningHead = (Vec<u8>, Scheme, SchemeDigest, Vec<u8>);

fn read_signing_head(r: &mut Reader<'_>) -> Result<SigningHead, Rc> {
    let key_name = r.tpm2b(2 + limits::MAX_HASH_SIZE)?.to_vec();
    let scheme = Scheme {
        alg: r.u16()?,
        hash: r.u16()?,
    };
    let digest = SchemeDigest::read(r)?;
    let context = r.tpm2b(mldsa::MAX_CONTEXT_BYTES)?.to_vec();
    // The hash must be the one of the scheme, and only an ML-DSA sequence has a context string.
    if !digest.matches(scheme)
        || (!context.is_empty() && !matches!(digest, SchemeDigest::Message(_)))
    {
        return Err(Rc::VALUE);
    }
    Ok((key_name, scheme, digest, context))
}

impl Sequence {
    /// The body of the context of this sequence, or `None` for a kind that is not saved (HMAC, event
    /// and platform sequences).
    pub(crate) fn context_body(&self) -> Option<Zeroizing<Vec<u8>>> {
        let mut w = Writer {
            // Room for the largest context body at once: no reallocation leaves an unwiped copy.
            buf: Vec::with_capacity(MAX_CONTEXT_BLOB),
        };
        w.u8(match &self.kind {
            SequenceKind::Hash(_) => BODY_HASH256,
            SequenceKind::Hash384(_) => BODY_HASH384,
            SequenceKind::Hash512(_) => BODY_HASH512,
            SequenceKind::Sign(_) => BODY_SIGN,
            SequenceKind::Verify(_) => BODY_VERIFY,
            SequenceKind::Hmac(_) | SequenceKind::Event(_) | SequenceKind::Hcrtm(_) => return None,
        });
        w.tpm2b(self.auth.as_slice());
        w.u8(match self.first_buffer_safe {
            None => 0,
            Some(false) => 1,
            Some(true) => 2,
        });
        match &self.kind {
            SequenceKind::Hash(hash) => put_state(&mut w, hash),
            SequenceKind::Hash384(hash) => put_state(&mut w, hash),
            SequenceKind::Hash512(hash) => put_state(&mut w, hash),
            SequenceKind::Sign(sign) => {
                write_signing_head(
                    &mut w,
                    &sign.key_name,
                    sign.scheme,
                    &sign.digest,
                    &sign.context,
                );
                w.u8(sign.prefix.len as u8);
                w.bytes(&sign.prefix.bytes[..sign.prefix.len]);
            }
            SequenceKind::Verify(verify) => {
                write_signing_head(
                    &mut w,
                    &verify.key_name,
                    verify.scheme,
                    &verify.digest,
                    &verify.context,
                );
                for ticket in &verify.tickets {
                    // The proof of the hierarchy never leaves the TPM, not even encrypted: only
                    // its identifier is saved and the key block is derived from the current proof
                    // at load.
                    w.bytes(&ticket.proof_id);
                    put_state(&mut w, ticket.mac.inner());
                }
            }
            SequenceKind::Hmac(_) | SequenceKind::Event(_) | SequenceKind::Hcrtm(_) => return None,
        }
        Some(Zeroizing::new(w.buf))
    }

    /// The sequence of a context body (`Rc::VALUE` for an unknown kind or a body that does not fit
    /// its kind). `proof_of` gives the current proof of a hierarchy; a verify sequence whose saved
    /// proof identifier is not that of the current proof (the proof was rotated) is
    /// `Rc::INTEGRITY`: the key block of its ticket accumulators is derived from the proof and the
    /// prefix accumulated before the rotation cannot be authenticated.
    pub(crate) fn from_context_body(
        body: &[u8],
        proof_of: impl Fn(u32) -> Option<Zeroizing<Vec<u8>>>,
    ) -> Result<Sequence, Rc> {
        let mut r = Reader::new(body);
        let tag = r.u8()?;
        let auth =
            AuthValue::from_slice_up_to(r.tpm2b(limits::MAX_HASH_SIZE)?, limits::MAX_HASH_SIZE)
                .ok_or(Rc::SIZE)?;
        let first_buffer_safe = match r.u8()? {
            0 => None,
            1 => Some(false),
            2 => Some(true),
            _ => return Err(Rc::VALUE),
        };
        let kind = match tag {
            BODY_HASH256 => SequenceKind::Hash(get_state(&mut r)?),
            BODY_HASH384 => SequenceKind::Hash384(get_state(&mut r)?),
            BODY_HASH512 => SequenceKind::Hash512(get_state(&mut r)?),
            BODY_SIGN => {
                let (key_name, scheme, digest, context) = read_signing_head(&mut r)?;
                let len = usize::from(r.u8()?);
                if len > GENERATED_PREFIX.len() {
                    return Err(Rc::VALUE);
                }
                let mut prefix = MessagePrefix::default();
                prefix.append(r.bytes(len)?);
                SequenceKind::Sign(SignSequence {
                    key_name,
                    scheme,
                    digest,
                    prefix,
                    context,
                })
            }
            BODY_VERIFY => {
                let (key_name, scheme, digest, context) = read_signing_head(&mut r)?;
                let mut tickets = Vec::with_capacity(VERIFIED_HIERARCHIES.len());
                for hierarchy in VERIFIED_HIERARCHIES {
                    let proof_id: [u8; DIGEST] =
                        r.bytes(DIGEST)?.try_into().map_err(|_| Rc::SIZE)?;
                    let inner = get_state(&mut r)?;
                    let proof = proof_of(hierarchy).ok_or(Rc::INTEGRITY)?;
                    if proof_id != <[u8; DIGEST]>::from(Sha256::digest(&proof[..])) {
                        return Err(Rc::INTEGRITY);
                    }
                    tickets.push(MessageTicketState {
                        proof_id,
                        mac: HmacSha256Stream::restored(&proof, inner),
                    });
                }
                let Ok(tickets) =
                    <[MessageTicketState; VERIFIED_HIERARCHIES.len()]>::try_from(tickets)
                else {
                    return Err(Rc::SIZE);
                };
                SequenceKind::Verify(Box::new(VerifySequence {
                    key_name,
                    scheme,
                    digest,
                    context,
                    tickets,
                }))
            }
            _ => return Err(Rc::VALUE),
        };
        r.finish()?;
        Ok(Sequence {
            kind,
            auth,
            first_buffer_safe,
        })
    }
}

impl Tpm {
    /// Slot of the loaded sequence a handle names. `handle_idx` is the zero-based position in the
    /// handle area. Not a transient/persistent object handle: `TPM_RC_VALUE`; a persistent handle
    /// (nothing is persistent in phase A): `TPM_RC_HANDLE`; unloaded transient:
    /// `TPM_RC_REFERENCE_Hn`.
    pub(crate) fn seq_slot(&self, h: u32, handle_idx: usize) -> Result<usize, Rc> {
        let n = (handle_idx + 1) as u8;
        match handle::handle_type(h) {
            handle::HT_TRANSIENT => {
                let slot = (h & 0x00FF_FFFF) as usize;
                if self.sequences.is_guest_sequence(slot) {
                    Ok(slot)
                } else {
                    Err(Rc::reference_handle(handle_idx))
                }
            }
            handle::HT_PERSISTENT => Err(Rc::HANDLE.in_handle(n)),
            _ => Err(Rc::VALUE.in_handle(n)),
        }
    }

    /// Authorization value of the sequence in `slot`.
    pub(crate) fn seq_auth(&self, slot: usize) -> AuthValue {
        self.sequences.slots[slot]
            .as_ref()
            .map_or_else(AuthValue::empty, |s| s.auth.clone())
    }

    /// `TPM2_Hash` (Part 3, clause 15.4).
    pub(crate) fn cmd_hash(&mut self, p: &mut Reader<'_>) -> Result<Output, Rc> {
        p.set_param(1);
        let data = p.tpm2b(limits::MAX_2B_BUFFER_SIZE)?;
        p.set_param(2);
        let hash_alg = p.u16()?;
        let Some(algorithm) = HashAlg::from_alg(hash_alg) else {
            // NULL is not allowed for TPM2_Hash and is answered like any unimplemented hash.
            return Err(p.err(Rc::HASH));
        };
        p.set_param(3);
        let hierarchy = p.u32()?;
        check_hierarchy(p, hierarchy)?;
        p.finish()?;
        let hierarchy = if safe_hashcheck_prefix(data) {
            hierarchy
        } else {
            handle::RH_NULL
        };
        Ok(self.hash_result(hash(algorithm, &[data]).as_slice(), hierarchy))
    }

    /// The digest and its optional HASHCHECK in the common Hash/SequenceComplete wire format.
    fn hash_result(&self, digest: &[u8], hierarchy: u32) -> Output {
        let ticket = match self.proof(hierarchy) {
            Some(proof) => TpmtTkHashcheck {
                hierarchy,
                digest: hmac_sha256(&proof, &hashcheck_parts(digest)).to_vec(),
            },
            None => TpmtTkHashcheck {
                hierarchy: handle::RH_NULL,
                digest: Vec::new(),
            },
        };
        let mut w = Writer::new();
        w.tpm2b(digest);
        ticket.marshal(&mut w);
        Output {
            handles: Vec::new(),
            params: w.finish(),
        }
    }

    pub(crate) fn valid_hashcheck_ticket(&self, digest: &[u8], ticket: &TpmtTkHashcheck) -> bool {
        self.permanent_enabled(ticket.hierarchy)
            && self.proof(ticket.hierarchy).is_some_and(|proof| {
                hmac_sha256_verify(&proof, &hashcheck_parts(digest), &ticket.digest)
            })
    }

    /// `TPM2_HashSequenceStart` (Part 3, clause 17.4): `hashAlg` NULL starts an event sequence.
    pub(crate) fn cmd_hash_sequence_start(&mut self, p: &mut Reader<'_>) -> Result<Output, Rc> {
        p.set_param(1);
        let auth_bytes = p.tpm2b(limits::MAX_HASH_SIZE)?;
        p.set_param(2);
        let hash_alg = p.u16()?;
        // A hash sequence takes an authValue of the size of its hash; the event sequence (NULL)
        // and an unimplemented algorithm one SHA-256 digest.
        let auth_limit = digest_size(hash_alg).unwrap_or(DIGEST);
        let auth =
            AuthValue::from_slice_up_to(auth_bytes, auth_limit).ok_or(Rc::SIZE.in_param(1))?;
        let kind = match hash_alg {
            alg::SHA256 => SequenceKind::Hash(Sha256::new()),
            alg::SHA384 => SequenceKind::Hash384(Sha384::new()),
            alg::SHA512 => SequenceKind::Hash512(Sha512::new()),
            alg::NULL => SequenceKind::Event(EventHash::default()),
            _ => return Err(p.err(Rc::HASH)),
        };
        p.finish()?;
        self.sequences.start(kind, auth)
    }

    /// `TPM2_SignSequenceStart` without key authorization (Part 3 v185 §17.5).
    pub(crate) fn cmd_sign_sequence_start(
        &mut self,
        inv: &Invocation,
        p: &mut Reader<'_>,
    ) -> Result<Output, Rc> {
        p.set_param(1);
        let auth = p.tpm2b(limits::MAX_DIGEST_SIZE)?;
        let auth = AuthValue::from_slice(auth).ok_or_else(|| p.err(Rc::SIZE))?;
        p.set_param(2);
        // Only an ML-DSA key takes a context (Part 2 v185, Table 220): up to 255 octets.
        let context = p.tpm2b(mldsa::MAX_CONTEXT_BYTES)?;
        p.finish()?;
        let key = self.object(inv.handles[0]).ok_or(Rc::reference_handle(0))?;
        if key.has(object_attr::X509_SIGN) {
            return Err(Rc::ATTRIBUTES.in_handle(1));
        }
        let (scheme, digest) = if matches!(key.public.params, PublicParams::MlDsa { .. }) {
            if !key.has(object_attr::SIGN) {
                return Err(Rc::KEY.in_handle(1));
            }
            (Scheme::NULL, SchemeDigest::Message(Zeroizing::default()))
        } else {
            if !context.is_empty() {
                return Err(Rc::SIZE.in_param(2));
            }
            let scheme = fixed_digest_scheme(key, 1)?;
            let digest = SchemeDigest::new(scheme.hash).ok_or(Rc::SCHEME.in_handle(1))?;
            (scheme, digest)
        };
        let kind = SequenceKind::Sign(SignSequence {
            key_name: key.name.clone(),
            scheme,
            digest,
            prefix: MessagePrefix::default(),
            context: context.to_vec(),
        });
        self.sequences.start(kind, auth)
    }

    /// `TPM2_VerifySequenceStart` with the public key's fixed scheme (Part 3 v185 §17.6).
    pub(crate) fn cmd_verify_sequence_start(
        &mut self,
        inv: &Invocation,
        p: &mut Reader<'_>,
    ) -> Result<Output, Rc> {
        p.set_param(1);
        let auth = p.tpm2b(limits::MAX_DIGEST_SIZE)?;
        let auth = AuthValue::from_slice(auth).ok_or_else(|| p.err(Rc::SIZE))?;
        p.set_param(2);
        p.tpm2b(0)?; // Only EdDSA signing schemes take a hint.
        p.set_param(3);
        let context = p.tpm2b(mldsa::MAX_CONTEXT_BYTES)?;
        p.finish()?;
        let key = self.object(inv.handles[0]).ok_or(Rc::reference_handle(0))?;
        let (scheme, digest) = if matches!(key.public.params, PublicParams::MlDsa { .. }) {
            if !key.has(object_attr::SIGN) {
                return Err(Rc::KEY.in_handle(1));
            }
            (Scheme::NULL, SchemeDigest::Message(Zeroizing::default()))
        } else {
            if !context.is_empty() {
                return Err(Rc::SIZE.in_param(3));
            }
            let scheme = fixed_digest_scheme(key, 1)?;
            let digest = SchemeDigest::new(scheme.hash).ok_or(Rc::SCHEME.in_handle(1))?;
            (scheme, digest)
        };
        // Parallel accumulators preserve the raw-message transcript across key reloads. Their
        // size is independent of the number of SequenceUpdate calls and message length.
        let tickets = VERIFIED_HIERARCHIES.map(|hierarchy| {
            let proof = self
                .proof(hierarchy)
                .expect("a ticket hierarchy always has a proof");
            let mut mac = HmacSha256Stream::new(&proof);
            mac.update(&st::MESSAGE_VERIFIED.to_be_bytes());
            MessageTicketState {
                proof_id: Sha256::digest(&proof[..]).into(),
                mac,
            }
        });
        let kind = SequenceKind::Verify(Box::new(VerifySequence {
            key_name: key.name.clone(),
            scheme,
            digest,
            context: context.to_vec(),
            tickets,
        }));
        self.sequences.start(kind, auth)
    }

    /// The key and hash selection rules shared by HMAC and HMAC_Start (Part 3, Tables 75 and 79).
    fn hmac_key_for_hash(&self, handle: u32, hash_alg: u16) -> Result<&[u8], Rc> {
        let object = self.object(handle).ok_or(Rc::reference_handle(0))?;
        if !object.has(object_attr::SIGN) {
            return Err(Rc::KEY.in_handle(1));
        }
        let PublicParams::KeyedHash { scheme } = &object.public.params else {
            return Err(Rc::TYPE.in_handle(1));
        };
        if object.has(object_attr::RESTRICTED) {
            return Err(Rc::ATTRIBUTES.in_handle(1));
        }
        if !matches!(hash_alg, alg::SHA256 | alg::NULL) {
            return Err(Rc::HASH.in_param(2));
        }
        let valid_hash = if scheme.alg != alg::NULL {
            hash_alg == alg::NULL || hash_alg == scheme.hash
        } else {
            hash_alg == alg::SHA256
        };
        if !valid_hash {
            return Err(Rc::VALUE.in_param(2));
        }
        let Some(Secret::Data(key)) = object.sensitive.as_ref().map(|s| &s.secret) else {
            return Err(Rc::AUTH_UNAVAILABLE);
        };
        Ok(key)
    }

    /// `TPM2_HMAC` (Part 3, clause 15.5).
    pub(crate) fn cmd_hmac(&mut self, inv: &Invocation, p: &mut Reader<'_>) -> Result<Output, Rc> {
        p.set_param(1);
        let buffer = p.tpm2b(limits::MAX_2B_BUFFER_SIZE)?;
        p.set_param(2);
        let hash_alg = p.u16()?;
        p.finish()?;
        let key = self.hmac_key_for_hash(inv.handles[0], hash_alg)?;
        let mut w = Writer::new();
        w.tpm2b(&hmac_sha256(key, &[buffer]));
        Ok(Output {
            handles: Vec::new(),
            params: w.finish(),
        })
    }

    /// `TPM2_HMAC_Start` (Part 3, clause 17.2).
    pub(crate) fn cmd_hmac_start(
        &mut self,
        inv: &Invocation,
        p: &mut Reader<'_>,
    ) -> Result<Output, Rc> {
        p.set_param(1);
        let auth = AuthValue::from_slice(p.tpm2b(limits::MAX_DIGEST_SIZE)?)
            .ok_or_else(|| p.err(Rc::SIZE))?;
        p.set_param(2);
        let hash_alg = p.u16()?;
        p.finish()?;
        let key = self.hmac_key_for_hash(inv.handles[0], hash_alg)?;
        let kind = SequenceKind::Hmac(hmac_sha256_state(key));
        self.sequences.start(kind, auth)
    }

    /// `TPM2_SequenceUpdate` (Part 3, clause 17.5).
    pub(crate) fn cmd_sequence_update(
        &mut self,
        inv: &Invocation,
        p: &mut Reader<'_>,
    ) -> Result<Output, Rc> {
        let slot = self.seq_slot(inv.handles[0], 0)?;
        p.set_param(1);
        let data = p.tpm2b(limits::MAX_2B_BUFFER_SIZE)?;
        p.finish()?;
        if let Some(sequence) = self.sequences.slots[slot].as_mut() {
            if matches!(
                sequence.kind,
                SequenceKind::Hash(_) | SequenceKind::Hash384(_) | SequenceKind::Hash512(_)
            ) {
                sequence
                    .first_buffer_safe
                    .get_or_insert_with(|| safe_hashcheck_prefix(data));
            }
            match &mut sequence.kind {
                SequenceKind::Hash(hash) => hash.update(data),
                SequenceKind::Hash384(hash) => hash.update(data),
                SequenceKind::Hash512(hash) => hash.update(data),
                SequenceKind::Event(hash) => hash.update(data),
                SequenceKind::Hmac(mac) => mac.update(data),
                SequenceKind::Sign(sign) => {
                    // A message over the ML-DSA limit changes nothing.
                    sign.digest
                        .update(data)
                        .map_err(|()| Rc::SIZE.in_param(1))?;
                    sign.prefix.append(data);
                }
                SequenceKind::Verify(verify) => {
                    verify
                        .digest
                        .update(data)
                        .map_err(|()| Rc::SIZE.in_param(1))?;
                    for ticket in &mut verify.tickets {
                        ticket.mac.update(data);
                    }
                }
                SequenceKind::Hcrtm(_) => return Err(Rc::reference_handle(0)),
            }
        }
        Ok(Output::default())
    }

    /// `TPM2_SequenceComplete` (Part 3, clause 17.6): hash and HMAC sequences (`TPM_RC_MODE` for an
    /// event sequence); flushes the sequence on success.
    pub(crate) fn cmd_sequence_complete(
        &mut self,
        inv: &Invocation,
        p: &mut Reader<'_>,
    ) -> Result<Output, Rc> {
        let slot = self.seq_slot(inv.handles[0], 0)?;
        p.set_param(1);
        let data = p.tpm2b(limits::MAX_2B_BUFFER_SIZE)?;
        p.set_param(2);
        let hierarchy = p.u32()?;
        check_hierarchy(p, hierarchy)?;
        p.finish()?;
        let sequence = self.sequences.slots[slot]
            .take()
            .expect("the sequence slot was validated");
        let first_buffer_safe = sequence
            .first_buffer_safe
            .unwrap_or_else(|| safe_hashcheck_prefix(data));
        let hierarchy = if first_buffer_safe {
            hierarchy
        } else {
            handle::RH_NULL
        };
        let result = match sequence.kind {
            SequenceKind::Hash(mut hash) => {
                hash.update(data);
                self.hash_result(&hash.finalize(), hierarchy)
            }
            SequenceKind::Hash384(mut hash) => {
                hash.update(data);
                self.hash_result(&hash.finalize(), hierarchy)
            }
            SequenceKind::Hash512(mut hash) => {
                hash.update(data);
                self.hash_result(&hash.finalize(), hierarchy)
            }
            SequenceKind::Hmac(mut mac) => {
                mac.update(data);
                self.hash_result(&mac.finalize().into_bytes(), handle::RH_NULL)
            }
            SequenceKind::Event(_)
            | SequenceKind::Sign(_)
            | SequenceKind::Verify(_)
            | SequenceKind::Hcrtm(_) => {
                self.sequences.slots[slot] = Some(sequence);
                return Err(Rc::MODE.in_handle(1));
            }
        };
        Ok(result)
    }

    /// `TPM2_SignSequenceComplete` adds the final message buffer (Part 3 v185 §20.6).
    pub(crate) fn cmd_sign_sequence_complete(
        &mut self,
        inv: &Invocation,
        p: &mut Reader<'_>,
    ) -> Result<Output, Rc> {
        let slot = self.seq_slot(inv.handles[0], 0)?;
        p.set_param(1);
        let data = p.tpm2b(limits::MAX_2B_BUFFER_SIZE)?;
        p.finish()?;
        let Some(Sequence {
            kind: SequenceKind::Sign(sign),
            ..
        }) = self.sequences.slots[slot].as_ref()
        else {
            return Err(Rc::MODE.in_handle(1));
        };
        let key = find_object(&self.objects, &self.persistent.objects, inv.handles[1])
            .ok_or(Rc::reference_handle(1))?;
        if key.name != sign.key_name {
            return Err(Rc::SIGN_CONTEXT_KEY.in_handle(2));
        }

        let mut prefix = sign.prefix;
        prefix.append(data);
        if key.has(object_attr::RESTRICTED) && prefix.is_generated() {
            return Err(Rc::VALUE.in_param(1));
        }
        let result = if matches!(key.public.params, PublicParams::MlDsa { .. }) {
            // ML-DSA signs the whole message under the context of the sequence.
            let message = sign.digest.message_with(data).ok_or(Rc::SIZE.in_param(1))?;
            signature_output(&mldsa_message_signature(key, &message, &sign.context)?)
        } else {
            let digest = sign.digest.finalize_with(data);
            sign_digest_output(key, &digest, sign.scheme, &mut self.drbg)?
        };
        self.sequences.release(slot);
        Ok(result)
    }

    /// `TPM2_VerifySequenceComplete` verifies the streamed message (Part 3 v185 §20.3).
    pub(crate) fn cmd_verify_sequence_complete(
        &mut self,
        inv: &Invocation,
        p: &mut Reader<'_>,
    ) -> Result<Output, Rc> {
        let slot = self.seq_slot(inv.handles[0], 0)?;
        p.set_param(1);
        // A hash outside SHA-256, SHA-384 and SHA-512 is `TPM_RC_HASH` for the signature here.
        let signature = TpmtSignature::unmarshal(p)?;
        p.finish()?;
        let Some(Sequence {
            kind: SequenceKind::Verify(verify),
            ..
        }) = self.sequences.slots[slot].as_ref()
        else {
            return Err(Rc::MODE.in_handle(1));
        };
        let key = self.object(inv.handles[1]).ok_or(Rc::reference_handle(1))?;
        if key.name != verify.key_name {
            return Err(Rc::SIGN_CONTEXT_KEY.in_handle(2));
        }
        if matches!(key.public.params, PublicParams::MlDsa { .. }) {
            if !matches!(signature, TpmtSignature::MlDsa { .. }) {
                return Err(Rc::SCHEME.in_param(1));
            }
            let message = verify.digest.message_with(&[]).ok_or(Rc::SIZE)?;
            if !key.verify_mldsa_message(&message, &verify.context, &signature) {
                return Err(Rc::SIGNATURE);
            }
        } else {
            check_signature_scheme(&signature, verify.scheme, 1)?;
            let digest = verify.digest.finalize_with(&[]);
            if !key.verify_signature(&digest, &signature) {
                return Err(Rc::SIGNATURE);
            }
        }
        let result = if key.hierarchy == handle::RH_NULL {
            VerifiedTicket::message_output(handle::RH_NULL, &[])
        } else {
            let mismatch = Rc::SIGN_CONTEXT_KEY.in_handle(2);
            let index = VERIFIED_HIERARCHIES
                .iter()
                .position(|h| *h == key.hierarchy)
                .ok_or(mismatch)?;
            let proof = self.proof(key.hierarchy).ok_or(mismatch)?;
            let current_id: [u8; DIGEST] = Sha256::digest(&proof[..]).into();
            let ticket = &verify.tickets[index];
            if ticket.proof_id != current_id {
                // A changed proof cannot authenticate the prefix accumulated before rotation.
                return Err(mismatch);
            }
            let mut mac = ticket.mac.clone();
            mac.update(&key.name);
            VerifiedTicket::message_output(key.hierarchy, &mac.finalize())
        };
        self.sequences.release(slot);
        Ok(result)
    }

    /// `TPM2_EventSequenceComplete` (Part 3, clause 17.7): handle 1 is the PCR (or `TPM_RH_NULL`),
    /// handle 2 the event sequence (`TPM_RC_MODE` for a hash sequence). The sequence is flushed
    /// only if the extend succeeded.
    pub(crate) fn cmd_event_sequence_complete(
        &mut self,
        inv: &Invocation,
        p: &mut Reader<'_>,
    ) -> Result<Output, Rc> {
        let pcr = resolve_pcr_handle(inv.handles[0], true, 1)?;
        let slot = self.seq_slot(inv.handles[1], 1)?;
        p.set_param(1);
        let data = p.tpm2b(limits::MAX_2B_BUFFER_SIZE)?;
        p.finish()?;
        let Some(Sequence {
            kind: SequenceKind::Event(h),
            ..
        }) = self.sequences.slots[slot].as_ref()
        else {
            return Err(Rc::MODE.in_handle(2));
        };
        let mut h = h.clone();
        h.update(data);
        let response = self.complete_pcr_event(pcr, h.finish())?;
        self.sequences.slots[slot] = None;
        Ok(response)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn password_comparison_truncates_trailing_zeros() {
        assert!(password_matches(b"", b""));
        assert!(password_matches(&[0, 0], b""));
        assert!(password_matches(b"abc\0\0", b"abc"));
        assert!(password_matches(b"abc", b"abc\0"));
        assert!(!password_matches(b"abd", b"abc"));
        assert!(!password_matches(b"abc", b""));
        assert!(!password_matches(b"", b"\x01"));
        // A leading zero is significant.
        assert!(!password_matches(b"\0a", b"a"));
    }

    #[test]
    fn auth_value_limits_and_wipe() {
        assert!(AuthValue::from_slice(&[1; 33]).is_none());
        let a = AuthValue::from_slice(&[1, 2, 3]).unwrap();
        assert_eq!(a.as_slice(), [1, 2, 3]);
        assert!(AuthValue::empty().as_slice().is_empty());
    }

    #[test]
    fn hierarchy_values() {
        let r = Reader::new(&[]);
        for h in [
            handle::RH_OWNER,
            handle::RH_PLATFORM,
            handle::RH_ENDORSEMENT,
            handle::RH_NULL,
        ] {
            assert_eq!(check_hierarchy(&r, h), Ok(h));
        }
        assert_eq!(check_hierarchy(&r, 0x4000_000A), Err(Rc::VALUE));
    }
}
