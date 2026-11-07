//! `TPM2_HMAC`, `TPM2_Quote` and `TPM2_Certify` (slice C2 of phase B, S7-23).
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
//! **Attestation** (Part 3, clauses 18.1, 18.2 and 18.4; Part 2, clauses 10.12.2-10.12.13). The TPM
//! builds a `TPMS_ATTEST` (`magic` = `TPM_GENERATED_VALUE`, `type`, `qualifiedSigner` = Qualified
//! Name of the signing key, `extraData` = `qualifyingData`, `clockInfo`, `firmwareVersion`, the
//! attested structure), hashes it with SHA-256 and signs the digest with the key's scheme (RSASSA
//! or ECDSA; the key's scheme wins, `inScheme` is used for a key with a NULL scheme and may not
//! be NULL then: `TPM_RC_SCHEME`). The response holds the `TPM2B_ATTEST` and the `TPMT_SIGNATURE`.
//! `TPM2_Quote` attests `TPMS_QUOTE_INFO` (the PCR selection and the SHA-256 of the selected PCR
//! values in index order); `TPM2_Certify` attests `TPMS_CERTIFY_INFO` (Name and Qualified Name of
//! the certified object, which needs its private part: the ADMIN-role authorization of
//! `objectHandle` checks that; with `adminWithPolicy` SET a policy session with
//! `TPM2_PolicyCommandCode(TPM_CC_Certify)` is required, Part 3, clause 5.6). Restricted signing
//! keys sign attestations (that is their purpose, clause 18.1).
//!
//! **Privacy obfuscation** (clause 18.1): unless the signing key is in the Endorsement or Platform
//! hierarchy, `resetCount`, `restartCount` and `firmwareVersion` are increased by parts of
//! `KDFa(SHA-256, shProof, "OBFUSCATE", Qualified Name of the key, Empty Buffer, 128)`: octets
//! 0-7 go to `firmwareVersion`, 8-11 to `resetCount`, 12-15 to `restartCount` (big-endian,
//! wrapping; the text leaves the order to the implementation). `Clock` is not obfuscated.
//!
//! Not implemented: `TPM2_GetTime`, `TPM2_CertifyCreation`, `TPM2_GetSessionAuditDigest`,
//! `TPM2_GetCommandAuditDigest`, `TPM2_NV_Certify`, `TPM2_CertifyX509`, a `signHandle` of
//! `TPM_RH_NULL` (the text allows an unsigned attestation: here `TPM_RC_VALUE`, handle 1), ECDAA
//! and other schemes, a non-NULL `TPM2_Sign` of HMAC keys.
//!
//! NOT VERIFIED against TPM 2.0 Library v185/v184 (Part 4 is not available): the codes
//! `TPM_RC_KEY`/`TPM_RC_ATTRIBUTES` with a handle number for `x509sign` and non-signing keys of
//! attestation commands, `TPM_RC_SCHEME` (parameter 2) for a scheme that does not fit the key
//! type, `TPM_RC_SIZE` for a `qualifyingData` above `TPM2B_DATA`, `TPM_RC_VALUE` (parameter 2,
//! `TPM2_HMAC`) for the hash selection, `TPM_RC_HASH` for other hash algorithms; the value of
//! `firmwareVersion` (`TPM_PT_FIRMWARE_VERSION_1 << 32 | _2`); the obfuscation layout; the
//! `pcrDigest` of an empty PCR selection (SHA-256 of the empty string).

use sha2::{Digest, Sha256};

use crate::codes::{TPM_GENERATED_VALUE, alg, handle, object_attr, st};
use crate::crypto::{ecc_sign, hmac_sha256, kdfa_sha256, rsa_sign};
use crate::dispatch::Invocation;
use crate::marshal::{
    MAX_DATA, PublicParams, Reader, Scheme, TpmlPcrSelection, TpmsClockInfo, TpmtSignature, Writer,
    limits,
};
use crate::object::{Object, Secret, find_object};
use crate::policy::pcr_selection_digest;
use crate::random::Drbg;
use crate::response::{Output, Rc};
use crate::state::Tpm;

/// `TPM_PT_FIRMWARE_VERSION_1 << 32 | TPM_PT_FIRMWARE_VERSION_2` (`capability.rs`).
const FIRMWARE_VERSION: u64 = 0x0001_0000 << 32;

/// The scheme an attestation is signed with (Part 3, clause 18.1) and the checks of the key.
fn attest_scheme(o: &Object, in_scheme: Scheme, handle_no: u8) -> Result<Scheme, Rc> {
    if !o.has(object_attr::SIGN) {
        return Err(Rc::KEY.in_handle(handle_no));
    }
    if o.has(object_attr::X509_SIGN) {
        return Err(Rc::ATTRIBUTES.in_handle(handle_no));
    }
    let key_scheme = match &o.public.params {
        PublicParams::Rsa { scheme, .. } | PublicParams::Ecc { scheme, .. } => *scheme,
        PublicParams::KeyedHash { .. } => return Err(Rc::KEY.in_handle(handle_no)),
    };
    let scheme = if key_scheme.alg != alg::NULL {
        if in_scheme.alg != alg::NULL && in_scheme.alg != key_scheme.alg {
            return Err(Rc::SCHEME.in_param(2));
        }
        key_scheme
    } else {
        in_scheme
    };
    let fits = matches!(
        (&o.public.params, scheme.alg),
        (PublicParams::Rsa { .. }, alg::RSASSA) | (PublicParams::Ecc { .. }, alg::ECDSA)
    );
    if !fits || scheme.hash != alg::SHA256 {
        return Err(Rc::SCHEME.in_param(2));
    }
    Ok(scheme)
}

/// Signs `digest` with the private part of `o` under `scheme`.
fn sign_digest(
    o: &Object,
    scheme: Scheme,
    digest: &[u8],
    drbg: &mut Drbg,
) -> Result<TpmtSignature, Rc> {
    let sensitive = o.sensitive.as_ref().ok_or(Rc::AUTH_UNAVAILABLE)?;
    match &sensitive.secret {
        Secret::Rsa(key) => Ok(TpmtSignature::Rsassa {
            hash: scheme.hash,
            sig: rsa_sign(key, digest, drbg).ok_or(Rc::FAILURE)?,
        }),
        Secret::Ecc(key) => {
            let (r, s) = ecc_sign(key, digest).ok_or(Rc::FAILURE)?;
            Ok(TpmtSignature::Ecdsa {
                hash: scheme.hash,
                r,
                s,
            })
        }
        Secret::Data(_) => Err(Rc::KEY.in_handle(1)),
    }
}

impl Tpm {
    /// The marshaled `TPMS_ATTEST` for a signing key `signer` (clause 10.12.12).
    fn attest_bytes(
        &self,
        signer: &Object,
        attest_type: u16,
        extra: &[u8],
        attested: &[u8],
    ) -> Vec<u8> {
        let mut info = self.time_info().clock_info;
        let mut firmware = FIRMWARE_VERSION;
        if !matches!(
            signer.hierarchy,
            handle::RH_ENDORSEMENT | handle::RH_PLATFORM
        ) {
            // Obfuscation (clause 18.1): the module comment.
            let sh_proof = self.proof(handle::RH_OWNER).unwrap_or_default();
            let o = kdfa_sha256(&sh_proof, b"OBFUSCATE", &signer.qualified_name, &[], 128);
            let be64 =
                |b: &[u8]| u64::from_be_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]]);
            let be32 = |b: &[u8]| u32::from_be_bytes([b[0], b[1], b[2], b[3]]);
            firmware = firmware.wrapping_add(be64(&o[0..8]));
            info = TpmsClockInfo {
                reset_count: info.reset_count.wrapping_add(be32(&o[8..12])),
                restart_count: info.restart_count.wrapping_add(be32(&o[12..16])),
                ..info
            };
        }
        let mut w = Writer::new();
        w.u32(TPM_GENERATED_VALUE);
        w.u16(attest_type);
        w.tpm2b(&signer.qualified_name);
        w.tpm2b(extra);
        info.marshal(&mut w);
        w.u64(firmware);
        w.bytes(attested);
        w.finish()
    }

    /// The response of an attestation command: `TPM2B_ATTEST` and the signature over its SHA-256.
    fn attest_response(
        &mut self,
        sign_handle: u32,
        scheme: Scheme,
        attest: Vec<u8>,
    ) -> Result<Output, Rc> {
        let digest = Sha256::digest(&attest);
        let signer = find_object(&self.objects, &self.persistent.objects, sign_handle)
            .ok_or(Rc::reference_handle(0))?;
        let signature = sign_digest(signer, scheme, &digest, &mut self.drbg)?;
        let mut w = Writer::new();
        w.tpm2b(&attest);
        signature.marshal(&mut w);
        Ok(Output {
            handles: Vec::new(),
            params: w.finish(),
        })
    }

    /// `TPM2_Quote` (Part 3, clause 18.4).
    pub(crate) fn cmd_quote(&mut self, inv: &Invocation, p: &mut Reader<'_>) -> Result<Output, Rc> {
        p.set_param(1);
        let qualifying = p.tpm2b(MAX_DATA)?;
        p.set_param(2);
        let in_scheme = Scheme::unmarshal(p, &[alg::RSASSA, alg::ECDSA], Rc::SCHEME)?;
        p.set_param(3);
        let selection = TpmlPcrSelection::unmarshal(p)?;
        p.finish()?;
        let signer = self.object(inv.handles[0]).ok_or(Rc::reference_handle(0))?;
        let scheme = attest_scheme(signer, in_scheme, 1)?;
        let mut attested = Writer::new();
        selection.marshal(&mut attested);
        attested.tpm2b(&pcr_selection_digest(&self.pcrs, &selection));
        let attest = self.attest_bytes(signer, st::ATTEST_QUOTE, qualifying, &attested.buf);
        self.attest_response(inv.handles[0], scheme, attest)
    }

    /// `TPM2_Certify` (Part 3, clause 18.2).
    pub(crate) fn cmd_certify(
        &mut self,
        inv: &Invocation,
        p: &mut Reader<'_>,
    ) -> Result<Output, Rc> {
        p.set_param(1);
        let qualifying = p.tpm2b(MAX_DATA)?;
        p.set_param(2);
        let in_scheme = Scheme::unmarshal(p, &[alg::RSASSA, alg::ECDSA], Rc::SCHEME)?;
        p.finish()?;
        let certified = self.object(inv.handles[0]).ok_or(Rc::reference_handle(0))?;
        let signer = self.object(inv.handles[1]).ok_or(Rc::reference_handle(1))?;
        let scheme = attest_scheme(signer, in_scheme, 2)?;
        let mut attested = Writer::new();
        attested.tpm2b(&certified.name);
        attested.tpm2b(&certified.qualified_name);
        let attest = self.attest_bytes(signer, st::ATTEST_CERTIFY, qualifying, &attested.buf);
        self.attest_response(inv.handles[1], scheme, attest)
    }

    /// `TPM2_HMAC` (Part 3, clause 15.5).
    pub(crate) fn cmd_hmac(&mut self, inv: &Invocation, p: &mut Reader<'_>) -> Result<Output, Rc> {
        p.set_param(1);
        let buffer = p.tpm2b(limits::MAX_2B_BUFFER_SIZE)?;
        p.set_param(2);
        let hash_alg = p.u16()?;
        p.finish()?;
        let o = self.object(inv.handles[0]).ok_or(Rc::reference_handle(0))?;
        if !o.has(object_attr::SIGN) {
            return Err(Rc::KEY.in_handle(1));
        }
        let PublicParams::KeyedHash { scheme } = &o.public.params else {
            return Err(Rc::TYPE.in_handle(1));
        };
        if o.has(object_attr::RESTRICTED) {
            return Err(Rc::ATTRIBUTES.in_handle(1));
        }
        if hash_alg != alg::SHA256 && hash_alg != alg::NULL {
            return Err(Rc::HASH.in_param(2));
        }
        // Hash selection (Part 3, Table 75): the key's default, or the caller's for a NULL scheme.
        let ok = if scheme.alg != alg::NULL {
            hash_alg == alg::NULL || hash_alg == scheme.hash
        } else {
            hash_alg == alg::SHA256
        };
        if !ok {
            return Err(Rc::VALUE.in_param(2));
        }
        let Some(Secret::Data(key)) = o.sensitive.as_ref().map(|s| &s.secret) else {
            return Err(Rc::AUTH_UNAVAILABLE);
        };
        let mac = hmac_sha256(key, &[buffer]);
        let mut w = Writer::new();
        w.tpm2b(&mac);
        Ok(Output {
            handles: Vec::new(),
            params: w.finish(),
        })
    }
}
