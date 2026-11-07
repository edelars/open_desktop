//! Big-endian marshaling of the TPM 2.0 Part 2 structures that phase A uses.
//!
//! Every structure has `marshal` and `unmarshal`; unmarshaling checks sizes
//! (`TPM_RC_SIZE`, `TPM_RC_INSUFFICIENT`) and, through [`Reader::set_param`], attaches the
//! one-based parameter number to format-one codes as Part 2, clause 6.6.2, requires.
//! Structure layouts: Part 2 v184, Tables 91, 92, 97, 98, 105, 106, 112-114, 120-128, 136,
//! 139, 140, 153, 154 (checked 2026-11-07, see `codes.rs`).

use zeroize::Zeroize;

use crate::codes::{alg, curve, object_attr, st};
use crate::response::Rc;

/// Implementation limits of phase A (Part 2 names in parentheses).
pub mod limits {
    /// `IMPLEMENTATION_PCR`: PCR 0-23 (PTP 1.07, clause 4.7: minimum 24).
    pub const PCR_COUNT: usize = 24;
    /// `PCR_SELECT_MIN` = `PCR_SELECT_MAX` = (24 + 7) / 8 (Part 2, formula 2; PTP Table 2: min 3).
    pub const PCR_SELECT_SIZE: usize = 3;
    /// `HASH_COUNT`: implemented hash algorithms (SHA-256 only in phase A).
    pub const HASH_COUNT: usize = 1;
    /// Digest size of the largest implemented hash (SHA-256).
    pub const MAX_DIGEST_SIZE: usize = 32;
    /// `MAX_2B_BUFFER_SIZE` for `TPM2B_MAX_BUFFER` (Part 2, clause 10.4.8: at least 1024).
    pub const MAX_2B_BUFFER_SIZE: usize = 1024;
    /// `TPM2B_EVENT` limit (Part 2, Table 97).
    pub const MAX_EVENT_SIZE: usize = 1024;
    /// `TPM2B_SENSITIVE_DATA` limit for `TPM2_StirRandom` (Part 3, clause 16.2: 128 octets).
    pub const MAX_STIR_SIZE: usize = 128;
    /// `TPML_DIGEST` limit (Part 2, Table 123).
    pub const MAX_DIGEST_LIST: usize = 8;
    /// Largest command and response (ADR-0009, decision 4: CRB buffer of 0xF80 bytes).
    pub const MAX_COMMAND_SIZE: usize = 3968;
    /// Largest response (see [`MAX_COMMAND_SIZE`]).
    pub const MAX_RESPONSE_SIZE: usize = 3968;
    /// Sessions in one command (`TPM_RC_AUTHSIZE` above it).
    pub const MAX_AUTH_SESSIONS: usize = 3;
}

/// Reading cursor over a byte slice.
pub struct Reader<'a> {
    buf: &'a [u8],
    pos: usize,
    param: u8,
}

impl<'a> Reader<'a> {
    pub fn new(buf: &'a [u8]) -> Self {
        Reader {
            buf,
            pos: 0,
            param: 0,
        }
    }

    /// Sets the one-based number of the parameter being unmarshaled (0 = none: handle or header).
    pub fn set_param(&mut self, n: u8) {
        self.param = n;
    }

    /// Annotates `rc` with the current parameter number.
    pub fn err(&self, rc: Rc) -> Rc {
        if self.param == 0 {
            rc
        } else {
            rc.in_param(self.param)
        }
    }

    pub fn remaining(&self) -> usize {
        self.buf.len() - self.pos
    }

    pub fn bytes(&mut self, n: usize) -> Result<&'a [u8], Rc> {
        if self.remaining() < n {
            return Err(self.err(Rc::INSUFFICIENT));
        }
        let s = &self.buf[self.pos..self.pos + n];
        self.pos += n;
        Ok(s)
    }

    pub fn u8(&mut self) -> Result<u8, Rc> {
        Ok(self.bytes(1)?[0])
    }

    pub fn u16(&mut self) -> Result<u16, Rc> {
        let b = self.bytes(2)?;
        Ok(u16::from_be_bytes([b[0], b[1]]))
    }

    pub fn u32(&mut self) -> Result<u32, Rc> {
        let b = self.bytes(4)?;
        Ok(u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
    }

    pub fn u64(&mut self) -> Result<u64, Rc> {
        let b = self.bytes(8)?;
        let mut a = [0u8; 8];
        a.copy_from_slice(b);
        Ok(u64::from_be_bytes(a))
    }

    /// `TPM2B_*`: `size` (UINT16) and `size` octets; `TPM_RC_SIZE` above `max`.
    pub fn tpm2b(&mut self, max: usize) -> Result<&'a [u8], Rc> {
        let n = usize::from(self.u16()?);
        if n > max {
            return Err(self.err(Rc::SIZE));
        }
        self.bytes(n)
    }

    /// A parameter area must be consumed completely; trailing octets are `TPM_RC_SIZE`
    /// (Part 3, clause 6: the unmarshaled size must match the command size).
    pub fn finish(&self) -> Result<(), Rc> {
        if self.remaining() != 0 {
            Err(Rc::SIZE)
        } else {
            Ok(())
        }
    }
}

/// Writing helper.
#[derive(Default)]
pub struct Writer {
    pub buf: Vec<u8>,
}

impl Writer {
    pub fn new() -> Self {
        Writer { buf: Vec::new() }
    }

    pub fn u8(&mut self, v: u8) {
        self.buf.push(v);
    }

    pub fn u16(&mut self, v: u16) {
        self.buf.extend_from_slice(&v.to_be_bytes());
    }

    pub fn u32(&mut self, v: u32) {
        self.buf.extend_from_slice(&v.to_be_bytes());
    }

    pub fn u64(&mut self, v: u64) {
        self.buf.extend_from_slice(&v.to_be_bytes());
    }

    pub fn bytes(&mut self, v: &[u8]) {
        self.buf.extend_from_slice(v);
    }

    /// `TPM2B_*`; callers pass buffers already limited by the protocol, a longer slice is
    /// clamped to `u16::MAX` so the size field never lies about the data that follows.
    pub fn tpm2b(&mut self, v: &[u8]) {
        let v = &v[..v.len().min(usize::from(u16::MAX))];
        self.u16(v.len() as u16);
        self.bytes(v);
    }

    pub fn finish(self) -> Vec<u8> {
        self.buf
    }
}

/// Digest size implied by a hash algorithm identifier; `None` if the algorithm is not implemented.
pub fn digest_size(hash: u16) -> Option<usize> {
    (hash == alg::SHA256).then_some(32)
}

/// `TPMS_PCR_SELECTION` (Table 106).
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct TpmsPcrSelection {
    pub hash: u16,
    pub select: [u8; limits::PCR_SELECT_SIZE],
}

impl TpmsPcrSelection {
    pub fn marshal(&self, w: &mut Writer) {
        w.u16(self.hash);
        w.u8(limits::PCR_SELECT_SIZE as u8);
        w.bytes(&self.select);
    }

    /// `TPM_RC_HASH` for an unimplemented algorithm, `TPM_RC_VALUE` if `sizeofSelect` is not 3.
    pub fn unmarshal(r: &mut Reader<'_>) -> Result<Self, Rc> {
        let hash = r.u16()?;
        if digest_size(hash).is_none() {
            return Err(r.err(Rc::HASH));
        }
        let n = usize::from(r.u8()?);
        if n != limits::PCR_SELECT_SIZE {
            return Err(r.err(Rc::VALUE));
        }
        let b = r.bytes(n)?;
        Ok(TpmsPcrSelection {
            hash,
            select: [b[0], b[1], b[2]],
        })
    }

    /// True if PCR `index` is selected.
    pub fn selects(&self, index: usize) -> bool {
        index < limits::PCR_COUNT && self.select[index / 8] & (1 << (index % 8)) != 0
    }
}

/// `TPML_PCR_SELECTION` (Table 125).
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct TpmlPcrSelection {
    pub selections: Vec<TpmsPcrSelection>,
}

impl TpmlPcrSelection {
    pub fn marshal(&self, w: &mut Writer) {
        w.u32(self.selections.len() as u32);
        for s in &self.selections {
            s.marshal(w);
        }
    }

    /// `TPM_RC_SIZE` if `count` exceeds `HASH_COUNT`.
    pub fn unmarshal(r: &mut Reader<'_>) -> Result<Self, Rc> {
        let count = r.u32()? as usize;
        if count > limits::HASH_COUNT {
            return Err(r.err(Rc::SIZE));
        }
        let mut selections = Vec::with_capacity(count);
        for _ in 0..count {
            selections.push(TpmsPcrSelection::unmarshal(r)?);
        }
        Ok(TpmlPcrSelection { selections })
    }
}

/// `TPMT_HA` (Table 91): hash algorithm and a digest of the implied size.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct TpmtHa {
    pub hash: u16,
    pub digest: Vec<u8>,
}

impl TpmtHa {
    pub fn marshal(&self, w: &mut Writer) {
        w.u16(self.hash);
        w.bytes(&self.digest);
    }

    /// `TPM_RC_HASH` if the algorithm is not implemented (Part 3, clause 22.2).
    pub fn unmarshal(r: &mut Reader<'_>) -> Result<Self, Rc> {
        let hash = r.u16()?;
        let Some(n) = digest_size(hash) else {
            return Err(r.err(Rc::HASH));
        };
        Ok(TpmtHa {
            hash,
            digest: r.bytes(n)?.to_vec(),
        })
    }
}

/// `TPML_DIGEST_VALUES` (Table 124).
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct TpmlDigestValues {
    pub digests: Vec<TpmtHa>,
}

impl TpmlDigestValues {
    pub fn marshal(&self, w: &mut Writer) {
        w.u32(self.digests.len() as u32);
        for d in &self.digests {
            d.marshal(w);
        }
    }

    /// `TPM_RC_SIZE` if `count` exceeds `HASH_COUNT`.
    pub fn unmarshal(r: &mut Reader<'_>) -> Result<Self, Rc> {
        let count = r.u32()? as usize;
        if count > limits::HASH_COUNT {
            return Err(r.err(Rc::SIZE));
        }
        let mut digests = Vec::with_capacity(count);
        for _ in 0..count {
            digests.push(TpmtHa::unmarshal(r)?);
        }
        Ok(TpmlDigestValues { digests })
    }
}

/// `TPML_DIGEST` (Table 123), as returned by `TPM2_PCR_Read`.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct TpmlDigest {
    pub digests: Vec<Vec<u8>>,
}

impl TpmlDigest {
    pub fn marshal(&self, w: &mut Writer) {
        w.u32(self.digests.len() as u32);
        for d in &self.digests {
            w.tpm2b(d);
        }
    }

    pub fn unmarshal(r: &mut Reader<'_>) -> Result<Self, Rc> {
        let count = r.u32()? as usize;
        if count > limits::MAX_DIGEST_LIST {
            return Err(r.err(Rc::SIZE));
        }
        let mut digests = Vec::with_capacity(count);
        for _ in 0..count {
            digests.push(r.tpm2b(limits::MAX_DIGEST_SIZE)?.to_vec());
        }
        Ok(TpmlDigest { digests })
    }
}

/// `TPMS_CLOCK_INFO` (Table 139).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct TpmsClockInfo {
    pub clock: u64,
    pub reset_count: u32,
    pub restart_count: u32,
    pub safe: bool,
}

impl TpmsClockInfo {
    pub fn marshal(&self, w: &mut Writer) {
        w.u64(self.clock);
        w.u32(self.reset_count);
        w.u32(self.restart_count);
        w.u8(u8::from(self.safe));
    }

    pub fn unmarshal(r: &mut Reader<'_>) -> Result<Self, Rc> {
        let clock = r.u64()?;
        let reset_count = r.u32()?;
        let restart_count = r.u32()?;
        let safe = match r.u8()? {
            0 => false,
            1 => true,
            _ => return Err(r.err(Rc::VALUE)),
        };
        Ok(TpmsClockInfo {
            clock,
            reset_count,
            restart_count,
            safe,
        })
    }
}

/// `TPMS_TIME_INFO` (Table 140).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct TpmsTimeInfo {
    pub time: u64,
    pub clock_info: TpmsClockInfo,
}

impl TpmsTimeInfo {
    pub fn marshal(&self, w: &mut Writer) {
        w.u64(self.time);
        self.clock_info.marshal(w);
    }

    pub fn unmarshal(r: &mut Reader<'_>) -> Result<Self, Rc> {
        Ok(TpmsTimeInfo {
            time: r.u64()?,
            clock_info: TpmsClockInfo::unmarshal(r)?,
        })
    }
}

/// `TPMT_TK_HASHCHECK` (Table 112). Phase A only produces the NULL ticket
/// (`hierarchy` = `TPM_RH_NULL`, empty digest).
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct TpmtTkHashcheck {
    pub hierarchy: u32,
    pub digest: Vec<u8>,
}

impl TpmtTkHashcheck {
    pub fn marshal(&self, w: &mut Writer) {
        w.u16(st::HASHCHECK);
        w.u32(self.hierarchy);
        w.tpm2b(&self.digest);
    }

    /// `TPM_RC_TAG` if the tag is not `TPM_ST_HASHCHECK`.
    pub fn unmarshal(r: &mut Reader<'_>) -> Result<Self, Rc> {
        if r.u16()? != st::HASHCHECK {
            return Err(r.err(Rc::TAG));
        }
        let hierarchy = r.u32()?;
        let digest = r.tpm2b(limits::MAX_DIGEST_SIZE)?.to_vec();
        Ok(TpmtTkHashcheck { hierarchy, digest })
    }
}

/// `TPMS_AUTH_COMMAND` (Table 153). Phase A accepts only password sessions
/// (`sessionHandle` = `TPM_RS_PW`, empty nonce, `hmac` = password).
#[derive(Clone, PartialEq, Eq)]
pub struct TpmsAuthCommand {
    pub session_handle: u32,
    pub nonce: Vec<u8>,
    /// `TPMA_SESSION` octet (Table 40).
    pub attributes: u8,
    /// Password; secret, never printed.
    pub hmac: Vec<u8>,
}

impl core::fmt::Debug for TpmsAuthCommand {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("TpmsAuthCommand")
            .field("session_handle", &self.session_handle)
            .field("hmac_len", &self.hmac.len())
            .finish()
    }
}

impl Drop for TpmsAuthCommand {
    fn drop(&mut self) {
        self.hmac.as_mut_slice().zeroize();
    }
}

impl TpmsAuthCommand {
    pub fn marshal(&self, w: &mut Writer) {
        w.u32(self.session_handle);
        w.tpm2b(&self.nonce);
        w.u8(self.attributes);
        w.tpm2b(&self.hmac);
    }

    pub fn unmarshal(r: &mut Reader<'_>) -> Result<Self, Rc> {
        let session_handle = r.u32()?;
        let nonce = r.tpm2b(limits::MAX_DIGEST_SIZE)?.to_vec();
        let attributes = r.u8()?;
        let hmac = r.tpm2b(limits::MAX_DIGEST_SIZE)?.to_vec();
        Ok(TpmsAuthCommand {
            session_handle,
            nonce,
            attributes,
            hmac,
        })
    }
}

/// `TPMS_AUTH_RESPONSE` (Table 154).
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct TpmsAuthResponse {
    pub nonce: Vec<u8>,
    pub attributes: u8,
    pub hmac: Vec<u8>,
}

impl TpmsAuthResponse {
    pub fn marshal(&self, w: &mut Writer) {
        w.tpm2b(&self.nonce);
        w.u8(self.attributes);
        w.tpm2b(&self.hmac);
    }

    pub fn unmarshal(r: &mut Reader<'_>) -> Result<Self, Rc> {
        let nonce = r.tpm2b(limits::MAX_DIGEST_SIZE)?.to_vec();
        let attributes = r.u8()?;
        let hmac = r.tpm2b(limits::MAX_DIGEST_SIZE)?.to_vec();
        Ok(TpmsAuthResponse {
            nonce,
            attributes,
            hmac,
        })
    }
}

/// `TPMS_ALG_PROPERTY` (Table 113).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct TpmsAlgProperty {
    pub alg: u16,
    /// `TPMA_ALGORITHM` (Table 38).
    pub properties: u32,
}

impl TpmsAlgProperty {
    pub fn marshal(&self, w: &mut Writer) {
        w.u16(self.alg);
        w.u32(self.properties);
    }

    pub fn unmarshal(r: &mut Reader<'_>) -> Result<Self, Rc> {
        Ok(TpmsAlgProperty {
            alg: r.u16()?,
            properties: r.u32()?,
        })
    }
}

/// `TPMA_ALGORITHM` bit 0: asymmetric algorithm (Table 38).
pub const TPMA_ALGORITHM_ASYMMETRIC: u32 = 1 << 0;
/// `TPMA_ALGORITHM` bit 1: symmetric algorithm (Table 38).
pub const TPMA_ALGORITHM_SYMMETRIC: u32 = 1 << 1;
/// `TPMA_ALGORITHM` bit 2: hash algorithm (Table 38).
pub const TPMA_ALGORITHM_HASH: u32 = 1 << 2;
/// `TPMA_ALGORITHM` bit 3: an object type.
pub const TPMA_ALGORITHM_OBJECT: u32 = 1 << 3;
/// `TPMA_ALGORITHM` bit 8: a signing algorithm.
pub const TPMA_ALGORITHM_SIGNING: u32 = 1 << 8;
/// `TPMA_ALGORITHM` bit 9: an encrypting algorithm (mode).
pub const TPMA_ALGORITHM_ENCRYPTING: u32 = 1 << 9;

/// `TPMS_TAGGED_PROPERTY` (Table 114).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct TpmsTaggedProperty {
    pub property: u32,
    pub value: u32,
}

impl TpmsTaggedProperty {
    pub fn marshal(&self, w: &mut Writer) {
        w.u32(self.property);
        w.u32(self.value);
    }

    pub fn unmarshal(r: &mut Reader<'_>) -> Result<Self, Rc> {
        Ok(TpmsTaggedProperty {
            property: r.u32()?,
            value: r.u32()?,
        })
    }
}

/// `TPMA_CC` (Table 45): command index plus the attributes the resource manager uses.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct TpmaCc(pub u32);

impl TpmaCc {
    pub fn new(index: u16, nv: bool, flushed: bool, c_handles: u8, r_handle: bool) -> Self {
        TpmaCc(
            u32::from(index)
                | (u32::from(nv) << 22)
                | (u32::from(flushed) << 24)
                | ((u32::from(c_handles) & 0x7) << 25)
                | (u32::from(r_handle) << 28),
        )
    }

    /// Sets `extensive` (bit 23): the command may flush any number of loaded contexts (`{E}` in
    /// Part 3).
    pub const fn with_extensive(self) -> Self {
        TpmaCc(self.0 | (1 << 23))
    }

    pub fn extensive(self) -> bool {
        self.0 & (1 << 23) != 0
    }

    pub fn command_index(self) -> u16 {
        (self.0 & 0xFFFF) as u16
    }

    pub fn nv(self) -> bool {
        self.0 & (1 << 22) != 0
    }

    pub fn flushed(self) -> bool {
        self.0 & (1 << 24) != 0
    }

    pub fn c_handles(self) -> u8 {
        ((self.0 >> 25) & 0x7) as u8
    }

    pub fn r_handle(self) -> bool {
        self.0 & (1 << 28) != 0
    }
}

// ------------------------------------------------------------------------------------------
// Slice B of phase B (S7-23): public areas, schemes, signatures, tickets and creation data.
// Layouts: Part 2 v184, Tables 109, 110, 160, 168, 169, 176, 180, 190, 193, 196, 197, 201, 203,
// 205, 207, 208, 212, 215, 216, 219, 220, 246, 247 (checked 2026-11-07).
// ------------------------------------------------------------------------------------------

/// Largest `TPM2B_PUBLIC_KEY_RSA` (RSA-2048 only, `MAX_RSA_KEY_BYTES`).
pub const MAX_RSA_BYTES: usize = 256;
/// Largest `TPM2B_ECC_PARAMETER` (P-256 only, `MAX_ECC_KEY_BYTES`).
pub const MAX_ECC_BYTES: usize = 32;
/// Largest `TPM2B_SENSITIVE_DATA` payload of a sealed object (`MAX_SYM_DATA`; Part 3, 12.7.2 limits
/// `outData` of `TPM2_Unseal` to 128 octets).
pub const MAX_SYM_DATA: usize = 128;
/// Largest `TPM2B_DATA` (`sizeof(TPMT_HA)` for SHA-256 = 2 + 32).
pub const MAX_DATA: usize = 34;

impl<'a> Reader<'a> {
    /// A reader over the next `n` octets, with the same parameter number; `TPM_RC_INSUFFICIENT`
    /// when fewer remain.
    pub fn sub(&mut self, n: usize) -> Result<Reader<'a>, Rc> {
        let b = self.bytes(n)?;
        let mut sub = Reader::new(b);
        sub.param = self.param;
        Ok(sub)
    }
}

/// `TPMT_SYM_DEF_OBJECT` (Table 160), restricted to what slice B can state: NULL, or AES-128 in
/// CFB mode (the template of a storage key); `object.rs` protects `outPrivate` with it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SymDef {
    Null,
    Aes128Cfb,
}

impl SymDef {
    pub fn marshal(&self, w: &mut Writer) {
        match self {
            SymDef::Null => w.u16(alg::NULL),
            SymDef::Aes128Cfb => {
                w.u16(alg::AES);
                w.u16(128);
                w.u16(alg::CFB);
            }
        }
    }

    /// `TPM_RC_SYMMETRIC` for another algorithm or key size (Table 81), `TPM_RC_MODE` for a mode
    /// other than CFB (Table 82).
    pub fn unmarshal(r: &mut Reader<'_>) -> Result<Self, Rc> {
        match r.u16()? {
            alg::NULL => Ok(SymDef::Null),
            alg::AES => {
                if r.u16()? != 128 {
                    return Err(r.err(Rc::SYMMETRIC));
                }
                if r.u16()? != alg::CFB {
                    return Err(r.err(Rc::MODE));
                }
                Ok(SymDef::Aes128Cfb)
            }
            _ => Err(r.err(Rc::SYMMETRIC)),
        }
    }
}

/// A signing or asymmetric scheme with its hash (`TPMT_SIG_SCHEME`, `TPMT_RSA_SCHEME`,
/// `TPMT_ECC_SCHEME`): `alg` is `TPM_ALG_NULL` (hash then `TPM_ALG_NULL`), `TPM_ALG_RSASSA` or
/// `TPM_ALG_ECDSA`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Scheme {
    pub alg: u16,
    pub hash: u16,
}

impl Scheme {
    pub const NULL: Scheme = Scheme {
        alg: alg::NULL,
        hash: alg::NULL,
    };

    pub fn marshal(&self, w: &mut Writer) {
        w.u16(self.alg);
        if self.alg != alg::NULL {
            w.u16(self.hash);
        }
    }

    /// `allowed` lists the non-NULL schemes the caller accepts; any other selector is `bad`
    /// (Table 84: `TPM_RC_SCHEME` for a signature scheme, Table 189: `TPM_RC_VALUE` for an RSA
    /// scheme, Table 199: `TPM_RC_SCHEME` for an ECC scheme). The hash must be SHA-256
    /// (`TPM_RC_HASH`, Table 78).
    pub fn unmarshal(r: &mut Reader<'_>, allowed: &[u16], bad: Rc) -> Result<Self, Rc> {
        let a = r.u16()?;
        if a == alg::NULL {
            return Ok(Scheme::NULL);
        }
        if !allowed.contains(&a) {
            return Err(r.err(bad));
        }
        let hash = r.u16()?;
        if hash != alg::SHA256 {
            return Err(r.err(Rc::HASH));
        }
        Ok(Scheme { alg: a, hash })
    }
}

/// `TPMU_PUBLIC_PARMS` (Table 217) of the supported object types.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum PublicParams {
    /// `TPMS_RSA_PARMS` (Table 215); `exponent` 0 stands for 65537 (as in the template).
    Rsa {
        symmetric: SymDef,
        scheme: Scheme,
        key_bits: u16,
        exponent: u32,
    },
    /// `TPMS_ECC_PARMS` (Table 216); the KDF is always `TPM_ALG_NULL`.
    Ecc {
        symmetric: SymDef,
        scheme: Scheme,
        curve: u16,
    },
    /// `TPMS_KEYEDHASH_PARMS` (Table 213): the scheme is always `TPM_ALG_NULL` (sealed data).
    KeyedHash,
}

/// `TPMU_PUBLIC_ID` (Table 212).
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Unique {
    Rsa(Vec<u8>),
    Ecc { x: Vec<u8>, y: Vec<u8> },
    Digest(Vec<u8>),
}

/// `TPMT_PUBLIC` (Table 219) of an RSA-2048, ECC P-256 or keyed-hash object with nameAlg SHA-256
/// (or `TPM_ALG_NULL`, only at unmarshal: callers refuse it where a Name is needed).
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct TpmtPublic {
    pub name_alg: u16,
    pub attributes: u32,
    pub auth_policy: Vec<u8>,
    pub params: PublicParams,
    pub unique: Unique,
}

impl TpmtPublic {
    /// `TPMI_ALG_PUBLIC` of the object.
    pub fn type_alg(&self) -> u16 {
        match self.params {
            PublicParams::Rsa { .. } => alg::RSA,
            PublicParams::Ecc { .. } => alg::ECC,
            PublicParams::KeyedHash => alg::KEYEDHASH,
        }
    }

    pub fn marshal(&self, w: &mut Writer) {
        w.u16(self.type_alg());
        w.u16(self.name_alg);
        w.u32(self.attributes);
        w.tpm2b(&self.auth_policy);
        match &self.params {
            PublicParams::Rsa {
                symmetric,
                scheme,
                key_bits,
                exponent,
            } => {
                symmetric.marshal(w);
                scheme.marshal(w);
                w.u16(*key_bits);
                w.u32(*exponent);
            }
            PublicParams::Ecc {
                symmetric,
                scheme,
                curve,
            } => {
                symmetric.marshal(w);
                scheme.marshal(w);
                w.u16(*curve);
                // `kdf`: TPMT_KDF_SCHEME with TPM_ALG_NULL.
                w.u16(alg::NULL);
            }
            PublicParams::KeyedHash => w.u16(alg::NULL),
        }
        match &self.unique {
            Unique::Rsa(n) => w.tpm2b(n),
            Unique::Ecc { x, y } => {
                w.tpm2b(x);
                w.tpm2b(y);
            }
            Unique::Digest(d) => w.tpm2b(d),
        }
    }

    /// The marshaled form (`TPMT_PUBLIC` without a size).
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut w = Writer::new();
        self.marshal(&mut w);
        w.finish()
    }

    /// Unmarshals one `TPMT_PUBLIC`. Codes: `TPM_RC_TYPE` for an object type other than RSA, ECC
    /// and keyed hash (Table 211), `TPM_RC_HASH` for a `nameAlg` other than SHA-256/NULL
    /// (Table 78), `TPM_RC_RESERVED_BITS` for reserved `TPMA_OBJECT` bits, `TPM_RC_SIZE` for an
    /// oversized `authPolicy` or `unique`, `TPM_RC_VALUE` for an RSA key size other than 2048 and
    /// an RSA or keyed-hash scheme the TPM does not implement (Tables 172, 189, 194),
    /// `TPM_RC_SCHEME` for an ECC scheme (Table 199), `TPM_RC_CURVE` for a curve other than NIST
    /// P-256 (Table 200), `TPM_RC_KDF` for a KDF other than NULL (Table 83), `TPM_RC_SYMMETRIC`
    /// and `TPM_RC_MODE` as in [`SymDef::unmarshal`].
    pub fn unmarshal(r: &mut Reader<'_>) -> Result<Self, Rc> {
        let type_alg = r.u16()?;
        if !matches!(type_alg, alg::RSA | alg::ECC | alg::KEYEDHASH) {
            return Err(r.err(Rc::TYPE));
        }
        let name_alg = r.u16()?;
        if name_alg != alg::SHA256 && name_alg != alg::NULL {
            return Err(r.err(Rc::HASH));
        }
        let attributes = r.u32()?;
        if attributes & object_attr::RESERVED != 0 {
            return Err(r.err(Rc::RESERVED_BITS));
        }
        let auth_policy = r.tpm2b(limits::MAX_DIGEST_SIZE)?.to_vec();
        let (params, unique) = match type_alg {
            alg::RSA => {
                let symmetric = SymDef::unmarshal(r)?;
                let scheme = Scheme::unmarshal(r, &[alg::RSASSA], Rc::VALUE)?;
                let key_bits = r.u16()?;
                if key_bits != 2048 {
                    return Err(r.err(Rc::VALUE));
                }
                let exponent = r.u32()?;
                let n = r.tpm2b(MAX_RSA_BYTES)?.to_vec();
                (
                    PublicParams::Rsa {
                        symmetric,
                        scheme,
                        key_bits,
                        exponent,
                    },
                    Unique::Rsa(n),
                )
            }
            alg::ECC => {
                let symmetric = SymDef::unmarshal(r)?;
                let scheme = Scheme::unmarshal(r, &[alg::ECDSA], Rc::SCHEME)?;
                let curve_id = r.u16()?;
                if curve_id != curve::NIST_P256 {
                    return Err(r.err(Rc::CURVE));
                }
                if r.u16()? != alg::NULL {
                    return Err(r.err(Rc::KDF));
                }
                let x = r.tpm2b(MAX_ECC_BYTES)?.to_vec();
                let y = r.tpm2b(MAX_ECC_BYTES)?.to_vec();
                (
                    PublicParams::Ecc {
                        symmetric,
                        scheme,
                        curve: curve_id,
                    },
                    Unique::Ecc { x, y },
                )
            }
            _ => {
                if r.u16()? != alg::NULL {
                    return Err(r.err(Rc::VALUE));
                }
                let d = r.tpm2b(limits::MAX_DIGEST_SIZE)?.to_vec();
                (PublicParams::KeyedHash, Unique::Digest(d))
            }
        };
        Ok(TpmtPublic {
            name_alg,
            attributes,
            auth_policy,
            params,
            unique,
        })
    }

    /// `TPM2B_PUBLIC` (Table 220): the size must equal the unmarshaled structure (`TPM_RC_SIZE`).
    pub fn unmarshal_sized(r: &mut Reader<'_>) -> Result<Self, Rc> {
        let n = usize::from(r.u16()?);
        let mut sub = r.sub(n)?;
        let t = TpmtPublic::unmarshal(&mut sub)?;
        if sub.remaining() != 0 {
            return Err(r.err(Rc::SIZE));
        }
        Ok(t)
    }

    /// `TPM2B_PUBLIC`.
    pub fn marshal_sized(&self, w: &mut Writer) {
        w.tpm2b(&self.to_bytes());
    }
}

/// `TPMS_SENSITIVE_CREATE` inside `TPM2B_SENSITIVE_CREATE` (Tables 168, 169). Both buffers are
/// secrets and are wiped on drop.
pub struct TpmsSensitiveCreate {
    pub user_auth: Vec<u8>,
    pub data: Vec<u8>,
}

impl TpmsSensitiveCreate {
    /// `TPM_RC_SIZE` for `userAuth` above one digest or `data` above [`MAX_SYM_DATA`], or when the
    /// outer size differs from the two inner buffers.
    pub fn unmarshal_sized(r: &mut Reader<'_>) -> Result<Self, Rc> {
        let n = usize::from(r.u16()?);
        let mut sub = r.sub(n)?;
        let user_auth = sub.tpm2b(limits::MAX_DIGEST_SIZE)?.to_vec();
        let data = sub.tpm2b(MAX_SYM_DATA)?.to_vec();
        if sub.remaining() != 0 {
            return Err(r.err(Rc::SIZE));
        }
        Ok(TpmsSensitiveCreate { user_auth, data })
    }
}

impl Drop for TpmsSensitiveCreate {
    fn drop(&mut self) {
        self.user_auth.as_mut_slice().zeroize();
        self.data.as_mut_slice().zeroize();
    }
}

/// `TPMT_TK_CREATION` and `TPMT_TK_VERIFIED` (Tables 109, 110) for output: a tag, a hierarchy and
/// a digest. A NULL ticket has hierarchy `TPM_RH_NULL` and an empty digest.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct TpmtTicket {
    pub tag: u16,
    pub hierarchy: u32,
    pub digest: Vec<u8>,
}

impl TpmtTicket {
    pub fn marshal(&self, w: &mut Writer) {
        w.u16(self.tag);
        w.u32(self.hierarchy);
        w.tpm2b(&self.digest);
    }
}

/// `TPMT_SIGNATURE` (Table 208) for RSASSA and ECDSA.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum TpmtSignature {
    /// `TPMS_SIGNATURE_RSA`: hash and the signature of the key size.
    Rsassa { hash: u16, sig: Vec<u8> },
    /// `TPMS_SIGNATURE_ECC`: hash, `signatureR`, `signatureS`.
    Ecdsa { hash: u16, r: Vec<u8>, s: Vec<u8> },
}

impl TpmtSignature {
    pub fn marshal(&self, w: &mut Writer) {
        match self {
            TpmtSignature::Rsassa { hash, sig } => {
                w.u16(alg::RSASSA);
                w.u16(*hash);
                w.tpm2b(sig);
            }
            TpmtSignature::Ecdsa { hash, r, s } => {
                w.u16(alg::ECDSA);
                w.u16(*hash);
                w.tpm2b(r);
                w.tpm2b(s);
            }
        }
    }

    /// `TPM_RC_SCHEME` for a `sigAlg` other than RSASSA and ECDSA, including `TPM_ALG_NULL`
    /// (Table 84), `TPM_RC_HASH` for a hash other than SHA-256.
    pub fn unmarshal(r: &mut Reader<'_>) -> Result<Self, Rc> {
        let a = r.u16()?;
        if a != alg::RSASSA && a != alg::ECDSA {
            return Err(r.err(Rc::SCHEME));
        }
        let hash = r.u16()?;
        if hash != alg::SHA256 {
            return Err(r.err(Rc::HASH));
        }
        if a == alg::RSASSA {
            Ok(TpmtSignature::Rsassa {
                hash,
                sig: r.tpm2b(MAX_RSA_BYTES)?.to_vec(),
            })
        } else {
            Ok(TpmtSignature::Ecdsa {
                hash,
                r: r.tpm2b(MAX_ECC_BYTES)?.to_vec(),
                s: r.tpm2b(MAX_ECC_BYTES)?.to_vec(),
            })
        }
    }
}

/// `TPMS_CREATION_DATA` (Table 246).
pub struct TpmsCreationData {
    pub pcr_select: TpmlPcrSelection,
    pub pcr_digest: Vec<u8>,
    /// `TPMA_LOCALITY` octet (Table 41): bit 0 is locality 0.
    pub locality: u8,
    pub parent_name_alg: u16,
    pub parent_name: Vec<u8>,
    pub parent_qualified_name: Vec<u8>,
    pub outside_info: Vec<u8>,
}

impl TpmsCreationData {
    pub fn marshal(&self, w: &mut Writer) {
        self.pcr_select.marshal(w);
        w.tpm2b(&self.pcr_digest);
        w.u8(self.locality);
        w.u16(self.parent_name_alg);
        w.tpm2b(&self.parent_name);
        w.tpm2b(&self.parent_qualified_name);
        w.tpm2b(&self.outside_info);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rt<T: PartialEq + core::fmt::Debug>(
        v: &T,
        m: impl Fn(&T, &mut Writer),
        u: impl Fn(&mut Reader<'_>) -> Result<T, Rc>,
    ) {
        let mut w = Writer::new();
        m(v, &mut w);
        let bytes = w.finish();
        let mut r = Reader::new(&bytes);
        let back = u(&mut r).unwrap();
        r.finish().unwrap();
        assert_eq!(&back, v);
    }

    #[test]
    fn primitives_are_big_endian() {
        let mut w = Writer::new();
        w.u16(0x0102);
        w.u32(0x0304_0506);
        w.u64(0x0708_090A_0B0C_0D0E);
        w.tpm2b(&[0xAA, 0xBB]);
        assert_eq!(
            w.finish(),
            [
                1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 0, 2, 0xAA, 0xBB
            ]
        );
    }

    #[test]
    fn pcr_selection_round_trip_and_bit_order() {
        // Part 1, clause 15.5: PCR[n] is bit (n mod 8) of octet n / 8.
        let s = TpmsPcrSelection {
            hash: alg::SHA256,
            select: [0x01, 0x20, 0x00],
        };
        assert!(s.selects(0) && s.selects(13) && !s.selects(1) && !s.selects(24));
        rt(&s, TpmsPcrSelection::marshal, TpmsPcrSelection::unmarshal);
        let l = TpmlPcrSelection {
            selections: vec![s],
        };
        rt(&l, TpmlPcrSelection::marshal, TpmlPcrSelection::unmarshal);
        rt(
            &TpmlPcrSelection::default(),
            TpmlPcrSelection::marshal,
            TpmlPcrSelection::unmarshal,
        );
    }

    #[test]
    fn pcr_selection_errors() {
        // Unimplemented hash: TPM_RC_HASH with parameter 1.
        let bytes = [0, 4, 3, 0xFF, 0xFF, 0xFF];
        let mut r = Reader::new(&bytes);
        r.set_param(1);
        assert_eq!(
            TpmsPcrSelection::unmarshal(&mut r),
            Err(Rc::HASH.in_param(1))
        );
        // sizeofSelect other than 3: TPM_RC_VALUE.
        let bytes = [0, 0x0B, 2, 0xFF, 0xFF];
        assert_eq!(
            TpmsPcrSelection::unmarshal(&mut Reader::new(&bytes)),
            Err(Rc::VALUE)
        );
        // Truncated: TPM_RC_INSUFFICIENT.
        let bytes = [0, 0x0B, 3, 0xFF];
        assert_eq!(
            TpmsPcrSelection::unmarshal(&mut Reader::new(&bytes)),
            Err(Rc::INSUFFICIENT)
        );
        // count above HASH_COUNT: TPM_RC_SIZE.
        let bytes = [0, 0, 0, 2];
        assert_eq!(
            TpmlPcrSelection::unmarshal(&mut Reader::new(&bytes)),
            Err(Rc::SIZE)
        );
    }

    #[test]
    fn digest_values_round_trip_and_errors() {
        let v = TpmlDigestValues {
            digests: vec![TpmtHa {
                hash: alg::SHA256,
                digest: vec![7; 32],
            }],
        };
        rt(&v, TpmlDigestValues::marshal, TpmlDigestValues::unmarshal);
        rt(
            &TpmlDigestValues::default(),
            TpmlDigestValues::marshal,
            TpmlDigestValues::unmarshal,
        );
        let mut w = Writer::new();
        w.u32(1);
        w.u16(0x0004); // SHA-1 is not implemented
        w.bytes(&[0; 20]);
        assert_eq!(
            TpmlDigestValues::unmarshal(&mut Reader::new(&w.finish())),
            Err(Rc::HASH)
        );
        let mut w = Writer::new();
        w.u32(1);
        w.u16(alg::SHA256);
        w.bytes(&[0; 31]);
        assert_eq!(
            TpmlDigestValues::unmarshal(&mut Reader::new(&w.finish())),
            Err(Rc::INSUFFICIENT)
        );
    }

    #[test]
    fn digest_list_and_tpm2b_limits() {
        let l = TpmlDigest {
            digests: vec![vec![1; 32], vec![2; 32]],
        };
        rt(&l, TpmlDigest::marshal, TpmlDigest::unmarshal);
        let mut w = Writer::new();
        w.u32(9);
        assert_eq!(
            TpmlDigest::unmarshal(&mut Reader::new(&w.finish())),
            Err(Rc::SIZE)
        );
        let mut w = Writer::new();
        w.u16(1025);
        let bytes = w.finish();
        let mut r = Reader::new(&bytes);
        r.set_param(2);
        assert_eq!(r.tpm2b(1024), Err(Rc::SIZE.in_param(2)));
    }

    #[test]
    fn time_info_round_trip() {
        let t = TpmsTimeInfo {
            time: 12345,
            clock_info: TpmsClockInfo {
                clock: 99,
                reset_count: 3,
                restart_count: 1,
                safe: true,
            },
        };
        rt(&t, TpmsTimeInfo::marshal, TpmsTimeInfo::unmarshal);
        let mut w = Writer::new();
        t.marshal(&mut w);
        assert_eq!(w.finish().len(), 25);
        let mut w = Writer::new();
        w.u64(0);
        w.u64(0);
        w.u32(0);
        w.u32(0);
        w.u8(2);
        assert_eq!(
            TpmsClockInfo::unmarshal(&mut Reader::new(&w.finish()[8..])),
            Err(Rc::VALUE)
        );
    }

    #[test]
    fn hashcheck_ticket_round_trip_and_tag() {
        let t = TpmtTkHashcheck {
            hierarchy: crate::codes::handle::RH_NULL,
            digest: vec![],
        };
        rt(&t, TpmtTkHashcheck::marshal, TpmtTkHashcheck::unmarshal);
        let mut w = Writer::new();
        t.marshal(&mut w);
        assert_eq!(w.finish(), [0x80, 0x24, 0x40, 0, 0, 7, 0, 0]);
        let bytes = [0x80, 0x01, 0x40, 0, 0, 7, 0, 0];
        assert_eq!(
            TpmtTkHashcheck::unmarshal(&mut Reader::new(&bytes)),
            Err(Rc::TAG)
        );
    }

    #[test]
    fn auth_structures_round_trip() {
        let c = TpmsAuthCommand {
            session_handle: crate::codes::handle::RS_PW,
            nonce: vec![],
            attributes: 1,
            hmac: vec![1, 2, 3],
        };
        rt(&c, TpmsAuthCommand::marshal, TpmsAuthCommand::unmarshal);
        let r = TpmsAuthResponse {
            nonce: vec![],
            attributes: 1,
            hmac: vec![],
        };
        rt(&r, TpmsAuthResponse::marshal, TpmsAuthResponse::unmarshal);
        // The password never appears in Debug output.
        assert!(!format!("{c:?}").contains("[1, 2, 3]"));
    }

    #[test]
    fn property_structures_round_trip() {
        rt(
            &TpmsAlgProperty {
                alg: alg::SHA256,
                properties: TPMA_ALGORITHM_HASH,
            },
            TpmsAlgProperty::marshal,
            TpmsAlgProperty::unmarshal,
        );
        rt(
            &TpmsTaggedProperty {
                property: 0x100,
                value: 0x322E_3000,
            },
            TpmsTaggedProperty::marshal,
            TpmsTaggedProperty::unmarshal,
        );
    }

    #[test]
    fn tpma_cc_bits() {
        let a = TpmaCc::new(0x185, true, true, 2, false);
        assert_eq!(a.command_index(), 0x185);
        assert!(a.nv() && a.flushed() && !a.r_handle());
        assert_eq!(a.c_handles(), 2);
        assert_eq!(a.0, 0x0185 | 1 << 22 | 1 << 24 | 2 << 25);
        let b = TpmaCc::new(0x186, false, false, 0, true);
        assert!(b.r_handle() && !b.nv());
        assert_eq!(b.0, 0x1000_0186);
        let c = TpmaCc::new(0x126, true, false, 1, false).with_extensive();
        assert!(c.extensive() && c.nv() && !c.flushed());
        assert_eq!(c.0, 0x0126 | 1 << 22 | 1 << 23 | 1 << 25);
    }

    #[test]
    fn reader_finish_detects_trailing_octets() {
        let r = Reader::new(&[1]);
        assert_eq!(r.finish(), Err(Rc::SIZE));
    }
}
