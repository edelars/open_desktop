//! Big-endian marshaling of the TPM 2.0 Part 2 structures that phase A uses.
//!
//! Every structure has `marshal` and `unmarshal`; unmarshaling checks sizes
//! (`TPM_RC_SIZE`, `TPM_RC_INSUFFICIENT`) and, through [`Reader::set_param`], attaches the
//! one-based parameter number to format-one codes as Part 2, clause 6.6.2, requires.
//! Structure layouts: Part 2 v184, Tables 91, 92, 97, 98, 105, 106, 112-114, 120-128, 136,
//! 139, 140, 153, 154 (checked 2026-11-07, see `codes.rs`).

use zeroize::Zeroize;

use crate::codes::{alg, st};
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

/// `TPMA_ALGORITHM` bit 2: hash algorithm (Table 38).
pub const TPMA_ALGORITHM_HASH: u32 = 1 << 2;

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
