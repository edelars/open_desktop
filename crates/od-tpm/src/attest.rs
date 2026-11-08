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
//! **`TPM2_GetTime`** (slice C3, Part 3, clause 18.7): the key checks are those of `Quote`; two handles:
//! `privacyAdminHandle` (`TPM_RH_ENDORSEMENT`, authorization 1, needs `ehEnable`) and `signHandle`
//! (authorization 2). The attested structure is `TPMS_TIME_ATTEST_INFO` (Part 2, Table 141):
//! `TPMS_TIME_INFO` (`Time` and the clock info) and the firmware version, both in clear; the outer
//! `clockInfo` and `firmwareVersion` of `TPMS_ATTEST` are obfuscated like in every attestation
//! when the key is not in the Endorsement or Platform hierarchy, so the two copies differ for a
//! storage key (clause 18.7.1, the note on the two copies).
//!
//! **`signHandle` = `TPM_RH_NULL`** (clause 18.1; `Quote`, `Certify`, `GetTime`): the command does
//! everything else and the attestation is "signed" with the NULL signature (`TPMT_SIGNATURE` with
//! `sigAlg` = `TPM_ALG_NULL`, no further fields); `inScheme` must still be a valid signing scheme
//! (RSASSA, ECDSA or NULL here) but has no effect; the values are obfuscated with the Qualified
//! Name `TPM_RH_NULL` (clause 18.1: "the QN for TPM_RH_NULL is TPM_RH_NULL"); `qualifiedSigner`
//! ("Qualified Name of the signing key", Part 2, Table 151) holds the same 4 octets of the handle.
//! No authorization value exists for `TPM_RH_NULL`: the session is checked
//! against the empty authValue like for every other use of this handle (`dispatch.rs`).
//!
//! Not implemented: `TPM2_CertifyCreation`, `TPM2_GetSessionAuditDigest`,
//! `TPM2_GetCommandAuditDigest`, `TPM2_NV_Certify`, `TPM2_CertifyX509`, ECDAA and other schemes
//! (a valid signing scheme other than RSASSA, ECDSA is `TPM_RC_SCHEME`), a non-NULL `TPM2_Sign`
//! of HMAC keys.
//!
//! NOT VERIFIED against TPM 2.0 Library v185/v184 (Part 4 is not available): the codes
//! `TPM_RC_KEY`/`TPM_RC_ATTRIBUTES` with a handle number for `x509sign` and non-signing keys of
//! attestation commands, `TPM_RC_SCHEME` (parameter 2) for a scheme that does not fit the key
//! type, `TPM_RC_SIZE` for a `qualifyingData` above `TPM2B_DATA`, `TPM_RC_VALUE` (parameter 2,
//! `TPM2_HMAC`) for the hash selection, `TPM_RC_HASH` for other hash algorithms; the value of
//! `firmwareVersion` (`TPM_PT_FIRMWARE_VERSION_1 << 32 | _2`); the obfuscation layout; the
//! `pcrDigest` of an empty PCR selection (SHA-256 of the empty string). Slice C3: that
//! a NULL signer's `qualifiedSigner` is the 4 octets of `TPM_RH_NULL` and the obfuscation uses
//! the same 4 octets as the QN with SHA-256 as the hash "used for context integrity" (clause 18.1
//! of v184 was read: it gives "the QN for TPM_RH_NULL is TPM_RH_NULL" and the formula
//! `KDFa(nameAlg, shProof, "OBFUSCATE", QN, 0, 128)`, but not what `qualifiedSigner` holds for a
//! NULL signer; that it holds the QN follows from Table 151, an inference, not a quote); the
//! codes of `GetTime` (`TPM_RC_VALUE`, handle 1, for a `privacyAdminHandle` other than
//! `TPM_RH_ENDORSEMENT`; `TPM_RC_KEY`/`TPM_RC_ATTRIBUTES`/`TPM_RC_SCHEME`/`TPM_RC_SIZE` as for
//! `Quote`, with the handle number 2 and parameter numbers 1 and 2); that `Time` in
//! `TPMS_TIME_INFO` is the milliseconds since the TPM was powered (`power_ms`), the same value
//! `TPM2_ReadClock` reports.

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

/// The Qualified Name of `TPM_RH_NULL`: the handle itself (clause 18.1).
const NULL_QN: [u8; 4] = handle::RH_NULL.to_be_bytes();

/// `TPM_PT_FIRMWARE_VERSION_1 << 32 | TPM_PT_FIRMWARE_VERSION_2` (`capability.rs`).
const FIRMWARE_VERSION: u64 = 0x0001_0000 << 32;

/// The checked signing key of an attestation command and its scheme, or `None` for
/// `TPM_RH_NULL` (the NULL signature).
type Signing<'a> = Option<(&'a Object, Scheme)>;

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
    /// The signing key of an attestation at handle position `n` (one-based) and its scheme;
    /// `None` for `TPM_RH_NULL` (clause 18.1: the scheme must be a valid signing scheme, which
    /// the parser checked, and has no effect).
    fn attest_signer(&self, h: u32, in_scheme: Scheme, n: u8) -> Result<Signing<'_>, Rc> {
        if h == handle::RH_NULL {
            return Ok(None);
        }
        let signer = self
            .object(h)
            .ok_or(Rc::reference_handle(usize::from(n) - 1))?;
        Ok(Some((signer, attest_scheme(signer, in_scheme, n)?)))
    }

    /// The marshaled `TPMS_ATTEST` for a signing key `signer` (`None`: `TPM_RH_NULL`), clause
    /// 10.12.12; `attested` is the marshaled attested structure of the type.
    fn attest_bytes(
        &self,
        signer: Option<&Object>,
        attest_type: u16,
        extra: &[u8],
        attested: &[u8],
    ) -> Vec<u8> {
        let mut info = self.time_info().clock_info;
        let mut firmware = FIRMWARE_VERSION;
        let (qualified_signer, qn, hierarchy): (&[u8], Vec<u8>, u32) = match signer {
            Some(o) => (&o.qualified_name, o.qualified_name.clone(), o.hierarchy),
            // Clause 18.1: "The QN for TPM_RH_NULL is TPM_RH_NULL"; `qualifiedSigner` is the
            // "Qualified Name of the signing key" (Part 2, Table 151), so it holds those 4 octets
            // and the obfuscation uses the same value: one QN for both.
            None => (&NULL_QN, NULL_QN.to_vec(), handle::RH_NULL),
        };
        if !matches!(hierarchy, handle::RH_ENDORSEMENT | handle::RH_PLATFORM) {
            // Obfuscation (clause 18.1): the module comment.
            let sh_proof = self.proof(handle::RH_OWNER).unwrap_or_default();
            let o = kdfa_sha256(&sh_proof, b"OBFUSCATE", &qn, &[], 128);
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
        w.tpm2b(qualified_signer);
        w.tpm2b(extra);
        info.marshal(&mut w);
        w.u64(firmware);
        w.bytes(attested);
        w.finish()
    }

    /// The response of an attestation command: `TPM2B_ATTEST` and the signature over its SHA-256
    /// (the NULL signature without a key).
    fn attest_response(
        &mut self,
        sign_handle: u32,
        scheme: Option<Scheme>,
        attest: Vec<u8>,
    ) -> Result<Output, Rc> {
        let signature = match scheme {
            Some(scheme) => {
                let digest = Sha256::digest(&attest);
                let signer = find_object(&self.objects, &self.persistent.objects, sign_handle)
                    .ok_or(Rc::reference_handle(0))?;
                sign_digest(signer, scheme, &digest, &mut self.drbg)?
            }
            None => TpmtSignature::Null,
        };
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
        let signing = self.attest_signer(inv.handles[0], in_scheme, 1)?;
        let mut attested = Writer::new();
        selection.marshal(&mut attested);
        attested.tpm2b(&pcr_selection_digest(&self.pcrs, &selection));
        let attest = self.attest_bytes(
            signing.map(|(o, _)| o),
            st::ATTEST_QUOTE,
            qualifying,
            &attested.buf,
        );
        self.attest_response(inv.handles[0], signing.map(|(_, s)| s), attest)
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
        let signing = self.attest_signer(inv.handles[1], in_scheme, 2)?;
        let mut attested = Writer::new();
        attested.tpm2b(&certified.name);
        attested.tpm2b(&certified.qualified_name);
        let attest = self.attest_bytes(
            signing.map(|(o, _)| o),
            st::ATTEST_CERTIFY,
            qualifying,
            &attested.buf,
        );
        self.attest_response(inv.handles[1], signing.map(|(_, s)| s), attest)
    }

    /// `TPM2_GetTime` (Part 3, clause 18.7): `privacyAdminHandle` (endorsement, checked by the
    /// dispatcher), `signHandle`, `qualifyingData`, `inScheme`.
    pub(crate) fn cmd_get_time(
        &mut self,
        inv: &Invocation,
        p: &mut Reader<'_>,
    ) -> Result<Output, Rc> {
        p.set_param(1);
        let qualifying = p.tpm2b(MAX_DATA)?;
        p.set_param(2);
        let in_scheme = Scheme::unmarshal(p, &[alg::RSASSA, alg::ECDSA], Rc::SCHEME)?;
        p.finish()?;
        let signing = self.attest_signer(inv.handles[1], in_scheme, 2)?;
        // `TPMS_TIME_ATTEST_INFO`: the clock values in clear (clause 18.7.1, note).
        let mut attested = Writer::new();
        self.time_info().marshal(&mut attested);
        attested.u64(FIRMWARE_VERSION);
        let attest = self.attest_bytes(
            signing.map(|(o, _)| o),
            st::ATTEST_TIME,
            qualifying,
            &attested.buf,
        );
        self.attest_response(inv.handles[1], signing.map(|(_, s)| s), attest)
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
