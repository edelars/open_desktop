//! Asymmetric primitives (slice C1 of phase B, S7-23): `TPM2_RSA_Encrypt`, `TPM2_RSA_Decrypt` and
//! `TPM2_ECDH_ZGen` (Part 3 v184, clauses 14.2, 14.3 and 14.5). The key model is in `object.rs`,
//! the primitives in `crypto.rs`.
//!
//! **RSA.** The padding follows Part 3, Table 42: the scheme of the key and `inScheme` select
//! none (raw RSA), RSAES-PKCS1-v1_5 or RSAES-OAEP (SHA-256 for the label hash and for MGF1). A
//! label must end with a zero octet, which belongs to the label used for the padding
//! (`TPM_RC_VALUE` otherwise). `TPM2_RSA_Encrypt` needs only the public part of an RSA key and
//! checks no attributes; `TPM2_RSA_Decrypt` needs an RSA key with `restricted` CLEAR and
//! `decrypt` SET (`TPM_RC_KEY`, `TPM_RC_ATTRIBUTES`) and an authorization. The private
//! operation is the `rsa` crate's, without confirmed constant-time arithmetic: ISSUE-064
//! (RUSTSEC-2023-0071), now also reachable through `TPM2_RSA_Decrypt`.
//!
//! **ECDH.** `TPM2_ECDH_ZGen` multiplies the point `inPoint` by the private scalar of an ECC
//! P-256 key (cofactor 1) and returns the full point `Z = (x, y)` as `TPM2B_ECC_POINT`; the key
//! needs `restricted` CLEAR, `decrypt` SET and a scheme of ECDH or NULL; the point must lie on the
//! curve (`TPM_RC_ECC_POINT`). Hashing or a KDF of `x` is the caller's business.
//!
//! NOT VERIFIED against TPM 2.0 Library v185/v184 (Part 4, which gives the per-command response
//! codes, was not available): the parameter and handle numbers of `TPM_RC_VALUE`, `TPM_RC_SCHEME`,
//! `TPM_RC_KEY`, `TPM_RC_ATTRIBUTES`, `TPM_RC_ECC_POINT` and `TPM_RC_SIZE` below; `TPM_RC_VALUE`
//! for a message or a ciphertext that does not fit the padding, for a failed padding check of
//! OAEP and for an integer not below `n` in the raw scheme; `TPM_RC_SCHEME` (parameter 2) for an
//! RSA key whose scheme is a signing scheme; `TPM_RC_HASH` for an OAEP scheme with a hash other
//! than SHA-256. Deviation: RSAES decryption uses the implicit rejection of the `rsa` crate (a
//! defence against padding oracles), so an invalid RSAES padding does not give `TPM_RC_VALUE` but a
//! deterministic synthetic message, unlike the text of clause 14.3.1.

use crate::codes::{alg, object_attr};
use crate::crypto::{RsaPadding, ecc_ecdh, rsa_decrypt_private, rsa_encrypt_public};
use crate::dispatch::Invocation;
use crate::marshal::{
    MAX_DATA, MAX_ECC_BYTES, MAX_RSA_BYTES, PublicParams, Reader, Scheme, Unique, Writer,
};
use crate::object::{Secret, find_object};
use crate::response::{Output, Rc};
use crate::state::Tpm;

/// Padding for the scheme of the key and `inScheme` (Part 3, Table 42).
fn select_padding(key: Scheme, requested: Scheme) -> Result<RsaPadding, Rc> {
    let bad = Rc::SCHEME.in_param(2);
    match (key.alg, requested.alg) {
        (alg::NULL, alg::NULL) => Ok(RsaPadding::None),
        (alg::NULL, alg::RSAES) | (alg::RSAES, alg::NULL | alg::RSAES) => Ok(RsaPadding::Rsaes),
        (alg::NULL, alg::OAEP) | (alg::OAEP, alg::NULL | alg::OAEP) => Ok(RsaPadding::Oaep),
        _ => Err(bad),
    }
}

/// The label rule of clauses 14.2.1 and 14.3.1: empty, or ending with a zero octet.
fn check_label(label: &[u8]) -> Result<(), Rc> {
    if label.last().is_some_and(|b| *b != 0) {
        return Err(Rc::VALUE.in_param(3));
    }
    Ok(())
}

impl Tpm {
    /// `TPM2_RSA_Encrypt` (clause 14.2): `message`, `inScheme` (`TPMT_RSA_DECRYPT`), `label`.
    pub(crate) fn cmd_rsa_encrypt(
        &mut self,
        inv: &Invocation,
        p: &mut Reader<'_>,
    ) -> Result<Output, Rc> {
        p.set_param(1);
        let message = p.tpm2b(MAX_RSA_BYTES)?;
        p.set_param(2);
        let in_scheme = Scheme::unmarshal(p, &[alg::RSAES, alg::OAEP], Rc::VALUE)?;
        p.set_param(3);
        let label = p.tpm2b(MAX_DATA)?;
        p.finish()?;
        let o = self.object(inv.handles[0]).ok_or(Rc::reference_handle(0))?;
        let (PublicParams::Rsa { scheme, .. }, Unique::Rsa(n)) =
            (&o.public.params, &o.public.unique)
        else {
            return Err(Rc::KEY.in_handle(1));
        };
        let padding = select_padding(*scheme, in_scheme)?;
        check_label(label)?;
        let n = n.clone();
        let out = rsa_encrypt_public(&n, padding, label, message, &mut self.drbg)
            .ok_or(Rc::VALUE.in_param(1))?;
        let mut w = Writer::new();
        w.tpm2b(&out);
        Ok(Output {
            handles: Vec::new(),
            params: w.finish(),
        })
    }

    /// `TPM2_RSA_Decrypt` (clause 14.3): `cipherText`, `inScheme`, `label`; the key is
    /// authorized by the dispatcher.
    pub(crate) fn cmd_rsa_decrypt(
        &mut self,
        inv: &Invocation,
        p: &mut Reader<'_>,
    ) -> Result<Output, Rc> {
        p.set_param(1);
        let cipher = p.tpm2b(MAX_RSA_BYTES)?;
        p.set_param(2);
        let in_scheme = Scheme::unmarshal(p, &[alg::RSAES, alg::OAEP], Rc::VALUE)?;
        p.set_param(3);
        let label = p.tpm2b(MAX_DATA)?;
        p.finish()?;
        // The fields are borrowed separately so that the key is used in place, not copied.
        let o = find_object(&self.objects, &self.persistent.objects, inv.handles[0])
            .ok_or(Rc::reference_handle(0))?;
        let PublicParams::Rsa { scheme, .. } = &o.public.params else {
            return Err(Rc::KEY.in_handle(1));
        };
        if o.has(object_attr::RESTRICTED) || !o.has(object_attr::DECRYPT) {
            return Err(Rc::ATTRIBUTES.in_handle(1));
        }
        let padding = select_padding(*scheme, in_scheme)?;
        check_label(label)?;
        let Some(Secret::Rsa(key)) = o.sensitive.as_ref().map(|s| &s.secret) else {
            return Err(Rc::AUTH_UNAVAILABLE);
        };
        // NOT VERIFIED: a failure of the private operation (padding, range, the RNG) is
        // `TPM_RC_VALUE` for the cipher text (module comment).
        let plain = rsa_decrypt_private(key, padding, label, cipher, &mut self.drbg)
            .ok_or(Rc::VALUE.in_param(1))?;
        let mut w = Writer {
            buf: Vec::with_capacity(2 + plain.len()),
        };
        w.tpm2b(&plain);
        Ok(Output {
            handles: Vec::new(),
            params: w.finish(),
        })
    }

    /// `TPM2_ECDH_ZGen` (clause 14.5): `inPoint` (`TPM2B_ECC_POINT`) → `outPoint`.
    pub(crate) fn cmd_ecdh_zgen(
        &mut self,
        inv: &Invocation,
        p: &mut Reader<'_>,
    ) -> Result<Output, Rc> {
        p.set_param(1);
        let size = usize::from(p.u16()?);
        let mut point = p.sub(size)?;
        let x = point.tpm2b(MAX_ECC_BYTES)?;
        let y = point.tpm2b(MAX_ECC_BYTES)?;
        if point.remaining() != 0 {
            return Err(Rc::SIZE.in_param(1));
        }
        p.finish()?;
        let o = self.object(inv.handles[0]).ok_or(Rc::reference_handle(0))?;
        let PublicParams::Ecc { scheme, .. } = &o.public.params else {
            return Err(Rc::KEY.in_handle(1));
        };
        if o.has(object_attr::RESTRICTED) || !o.has(object_attr::DECRYPT) {
            return Err(Rc::ATTRIBUTES.in_handle(1));
        }
        if !matches!(scheme.alg, alg::NULL | alg::ECDH) {
            return Err(Rc::SCHEME.in_handle(1));
        }
        let Some(Secret::Ecc(key)) = o.sensitive.as_ref().map(|s| &s.secret) else {
            return Err(Rc::AUTH_UNAVAILABLE);
        };
        let (zx, zy) = ecc_ecdh(key, x, y).ok_or(Rc::ECC_POINT.in_param(1))?;
        let mut body = Writer {
            buf: Vec::with_capacity(2 + 32 + 2 + 32),
        };
        body.tpm2b(&zx);
        body.tpm2b(&zy);
        let mut w = Writer {
            buf: Vec::with_capacity(2 + 68),
        };
        w.tpm2b(&body.buf);
        zeroize::Zeroize::zeroize(&mut body.buf);
        Ok(Output {
            handles: Vec::new(),
            params: w.finish(),
        })
    }
}
