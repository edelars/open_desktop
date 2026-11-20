//! SHA-384 and SHA-512 as hash algorithms of the capability, hash and sequence commands (hash agility,
//! steps A and B). Expected digests come from Python `hashlib`; the HASHCHECK tickets are the HMAC-SHA-256
//! of the owner proof, as for SHA-256. Sessions, objects, policies and snapshots stay SHA-256.

use super::tests_b::start_session;
use super::tests_obj::{
    ENDORSEMENT, NULL, OWNER, base, create_primary, ecc, ecc_sig_scheme, ecc_sign, flush,
    load_external, parse_loaded, parse_primary, rsa, sign, unmarshal_public, verify,
};
use super::*;
use crate::codes::{object_attr as oa, se};
use crate::crypto::{HashAlg, hash, hmac_sha256, kdfa_sha256};
use crate::marshal::{
    Scheme, SymDef, TpmsPcrSelection, TpmtHa, TpmtPublic, TpmtSignature, Writer, digest_size,
    pcr_digest_size,
};

const SHA384_FOX: &str = "ca737f1014a48f4c0b6dd43cb177b0afd9e5169367544c494011e3317dbf9a509cb1e5dc1e85a941bbee3d7f2afbc9b1";
const SHA512_FOX: &str = "07e547d9586f6a73f73fbac0435ed76951218fb7d0c8d788a309d785436bbb642e93a252a954f23912547d1e8a3b5ed6e1bfd7097821233fa0538f3db854fee6";
const SHA384_GENERATED: &str = "ba05f98ca6b36e61d294d9b1326c1937d17daad8cba3098f896fed8c13ea27ccefa4bf4a629a7bbf798ad56e767d4cc1";
const SHA512_GENERATED: &str = "f8d507472d08d17f11241d8b4ff7f7608a18c3ac64c11540038e0e9a6cceee0291e5aa7ee05da69fe1768ce2877460ecef9e4903ba247cb92ecdbeca7cba6b0f";
const SHA512_ABC: &str = "ddaf35a193617abacc417349ae20413112e6fa4e89a97ea20a9eeee64b55d39a2192992a274fc1a836ba3c23a3feebbd454d4423643ce80e2a9ac94fa54ca49f";
const FOX: &[u8] = b"The quick brown fox jumps over the lazy dog";
/// Starts with `TPM_GENERATED_VALUE` (0xFF544347): no ticket may bind it.
const GENERATED_LOOKALIKE: &[u8] = b"\xffTCG-attest-lookalike";

/// `TPM2B_DIGEST` of up to 64 octets and the HASHCHECK ticket of a Hash/SequenceComplete reply.
fn wide_digest_and_ticket(resp: &[u8]) -> (Vec<u8>, TpmtTkHashcheck) {
    let mut rd = Reader::new(params(resp, 0));
    let d = rd.tpm2b(limits::MAX_HASH_SIZE).unwrap().to_vec();
    let t = TpmtTkHashcheck::unmarshal(&mut rd).unwrap();
    rd.finish().unwrap();
    (d, t)
}

/// The ticket the TPM computes: `HMAC-SHA-256(proof, TPM_ST_HASHCHECK || digest)`, the owner proof
/// being `KDFa(SHA-256, ownerSeed, "ODTPM-SHPROOF", NULL, NULL, 256)`.
fn owner_ticket(t: &Tpm, digest: &[u8]) -> Vec<u8> {
    let proof = kdfa_sha256(
        &t.persistent.owner_seed[..32],
        b"ODTPM-SHPROOF",
        &[],
        &[],
        256,
    );
    hmac_sha256(&proof, &[&u16b(st::HASHCHECK), digest]).to_vec()
}

#[test]
fn hash_sequence_start_error_order_for_an_oversized_auth_and_unknown_hash_is_provisional() {
    // PROVISIONAL, NOT VERIFIED against the order of checks in the TPM 2.0 text (Part 4 is not
    // available): an authValue longer than one SHA-256 digest together with an unimplemented hashAlg
    // gives TPM_RC_SIZE for parameter 1 (the size is checked first) rather than TPM_RC_HASH.
    let mut t = started();
    err(&start_seq(&mut t, &[7; 40], 0x0027), Rc::SIZE.in_param(1));
    err(&start_seq(&mut t, &[7; 64], 0x0027), Rc::SIZE.in_param(1));
    assert_eq!(t.sequences.free_slots(), crate::hash::MAX_SEQUENCES);
}

#[test]
fn capability_reports_sha512_and_max_digest_64() {
    let mut t = started();
    let (more, c, body, count) = cap_list(&get_cap(&mut t, cap::ALGS, 0, 100));
    assert!(!more && c == cap::ALGS && count == 16);
    let entries: Vec<(u16, u32)> = body
        .chunks(6)
        .map(|e| {
            (
                u16::from_be_bytes([e[0], e[1]]),
                u32::from_be_bytes([e[2], e[3], e[4], e[5]]),
            )
        })
        .collect();
    assert_eq!(entries.len(), 16);
    assert!(entries.windows(2).all(|w| w[0].0 < w[1].0));
    for (id, name) in [
        (0x000B, "SHA-256"),
        (0x000C, "SHA-384"),
        (0x000D, "SHA-512"),
    ] {
        // Each hash once, with exactly the TPMA_ALGORITHM hash bit.
        assert_eq!(
            entries.iter().filter(|e| e.0 == id).collect::<Vec<_>>(),
            [&(id, 4)],
            "{name}"
        );
    }
    // Paging from SHA-512 starts with it; SHA-384 and below are skipped.
    let (_, _, body, count) = cap_list(&get_cap(&mut t, cap::ALGS, 0x0D, 1));
    assert_eq!((count, body), (1, hex("000d00000004")));
    // TPM_PT_MAX_DIGEST is the SHA-512 digest size, and TPM2_GetRandom returns that many octets.
    let (_, _, body, _) = cap_list(&get_cap(&mut t, cap::TPM_PROPERTIES, pt::MAX_DIGEST, 1));
    assert_eq!(body, [&u32b(pt::MAX_DIGEST)[..], &u32b(64)[..]].concat());
    for (asked, got) in [(63u16, 63usize), (64, 64), (65, 64)] {
        let r = run(&mut t, plain(cc::GET_RANDOM, &[], &u16b(asked)));
        let p = params(&r, 0);
        assert_eq!(usize::from(u16::from_be_bytes([p[0], p[1]])), got);
        assert_eq!(p.len(), 2 + got);
    }
    // The PCR banks stay SHA-256 and SHA-384.
    let r = get_cap(&mut t, cap::PCRS, 0, 100);
    let sel = TpmlPcrSelection::unmarshal(&mut Reader::new(&params(&r, 0)[5..])).unwrap();
    assert_eq!(
        sel.selections.iter().map(|s| s.hash).collect::<Vec<_>>(),
        [alg::SHA256, alg::SHA384]
    );
}

#[test]
fn hash_sha512_and_sha384_with_hashcheck_ticket() {
    let mut t = started();
    for (algorithm, data, expected, size) in [
        (alg::SHA384, FOX, SHA384_FOX, 48),
        (alg::SHA512, FOX, SHA512_FOX, 64),
    ] {
        let reply = run(&mut t, hash_cmd(data, algorithm, handle::RH_OWNER));
        let (digest, ticket) = wide_digest_and_ticket(&reply);
        assert_eq!(digest, hex(expected));
        assert_eq!(digest.len(), size);
        // A HASHCHECK ticket of the owner hierarchy over the digest (what a restricted signing key
        // needs), whatever the width of the digest.
        assert_eq!(ticket.hierarchy, handle::RH_OWNER);
        assert_eq!(ticket.digest, owner_ticket(&t, &digest));
        assert!(t.valid_hashcheck_ticket(&digest, &ticket));
        // The ticket binds the digest: another digest of the same width is refused.
        let mut other = digest.clone();
        other[0] ^= 1;
        assert!(!t.valid_hashcheck_ticket(&other, &ticket));
        // The Null hierarchy gives the NULL ticket.
        let (null_digest, null_ticket) =
            wide_digest_and_ticket(&run(&mut t, hash_cmd(data, algorithm, handle::RH_NULL)));
        assert_eq!(null_digest, digest);
        assert_eq!(
            null_ticket,
            TpmtTkHashcheck {
                hierarchy: handle::RH_NULL,
                digest: vec![]
            }
        );
    }
    // The published SHA-512("abc") vector. Data shorter than four octets cannot be checked against
    // the TPM_GENERATED_VALUE prefix, so it gets no ticket either.
    let (digest, ticket) = wide_digest_and_ticket(&run(
        &mut t,
        hash_cmd(b"abc", alg::SHA512, handle::RH_OWNER),
    ));
    assert_eq!(digest, hex(SHA512_ABC));
    assert_eq!(ticket.hierarchy, handle::RH_NULL);
    // Data that starts with TPM_GENERATED_VALUE never gets a ticket (Part 3, clause 15.4).
    for (algorithm, expected) in [
        (alg::SHA384, SHA384_GENERATED),
        (alg::SHA512, SHA512_GENERATED),
    ] {
        let reply = run(
            &mut t,
            hash_cmd(GENERATED_LOOKALIKE, algorithm, handle::RH_OWNER),
        );
        let (digest, ticket) = wide_digest_and_ticket(&reply);
        assert_eq!(digest, hex(expected));
        assert_eq!(
            ticket,
            TpmtTkHashcheck {
                hierarchy: handle::RH_NULL,
                digest: vec![]
            }
        );
    }
    // The implementation's own hash agrees (SHA-3 and SHA-1 stay unimplemented).
    assert_eq!(
        hash(HashAlg::Sha512, &[FOX]).as_slice(),
        &hex(SHA512_FOX)[..]
    );
    err(
        &run(&mut t, hash_cmd(b"a", 0x0027, handle::RH_NULL)),
        Rc::HASH.in_param(2),
    );
    err(
        &run(&mut t, hash_cmd(b"a", 0x0004, handle::RH_NULL)),
        Rc::HASH.in_param(2),
    );
}

#[test]
fn hash_sequence_sha512_matches_one_shot_and_takes_an_auth_of_the_hash_size() {
    let mut t = started();
    let direct = run(&mut t, hash_cmd(FOX, alg::SHA512, handle::RH_OWNER));
    // Any split of the data, with an empty authValue (a password session carries at most one
    // SHA-256 digest until sessions have hash agility).
    for split in [4, 16, FOX.len()] {
        let seq = seq_handle(&start_seq(&mut t, b"", alg::SHA512));
        ok(&seq_update(&mut t, seq, b"", &FOX[..split]));
        let streamed = seq_complete(&mut t, seq, b"", &FOX[split..], handle::RH_OWNER);
        assert_eq!(params(&streamed, 0), params(&direct, 0), "split {split}");
    }
    // A sequence that starts with the generated prefix gets the NULL ticket, like SHA-256.
    let seq = seq_handle(&start_seq(&mut t, b"", alg::SHA512));
    ok(&seq_update(&mut t, seq, b"", GENERATED_LOOKALIKE));
    let (digest, ticket) =
        wide_digest_and_ticket(&seq_complete(&mut t, seq, b"", b"", handle::RH_OWNER));
    assert_eq!(digest, hex(SHA512_GENERATED));
    assert_eq!(ticket.hierarchy, handle::RH_NULL);

    // The authValue of a hash sequence may be as long as the digest of its hash; one octet more,
    // or more than a SHA-256 digest for the NULL (event) and SHA-256 sequences, is TPM_RC_SIZE.
    for (algorithm, size) in [(alg::SHA256, 32), (alg::SHA384, 48), (alg::SHA512, 64)] {
        let ok_auth = vec![7u8; size];
        let seq = seq_handle(&start_seq(&mut t, &ok_auth, algorithm));
        ok(&flush(&mut t, seq));
        let too_long = vec![7u8; size + 1];
        err(
            &start_seq(&mut t, &too_long, algorithm),
            Rc::SIZE.in_param(1),
        );
    }
    err(
        &start_seq(&mut t, &[7; 33], alg::NULL),
        Rc::SIZE.in_param(1),
    );
    err(
        &start_seq(&mut t, &[7; 65], alg::SHA512),
        Rc::SIZE.in_param(1),
    );
    // No slot was leaked by the errors above.
    assert_eq!(t.sequences.free_slots(), crate::hash::MAX_SEQUENCES);
}

#[test]
fn sha512_is_a_hash_but_not_a_pcr_bank() {
    let mut t = started();
    // TPMS_PCR_SELECTION and TPMT_HA take the PCR banks only (SHA-256, SHA-384).
    err(
        &run(&mut t, pcr_read_cmd(alg::SHA512, 3, &[1, 0, 0])),
        Rc::HASH.in_param(1),
    );
    let extend_sha512 = cat(&[&u32b(1), &u16b(alg::SHA512), &[0; 64]]);
    err(
        &run(
            &mut t,
            with_pw(cc::PCR_EXTEND, &[3], &[b""], &extend_sha512),
        ),
        Rc::HASH.in_param(1),
    );
    assert_eq!(read_pcr(&mut t, 3), vec![0u8; 32]);
    let mut w = Writer::new();
    w.u16(alg::SHA512);
    w.bytes(&[0; 64]);
    assert_eq!(
        TpmtHa::unmarshal(&mut Reader::new(&w.finish())),
        Err(Rc::HASH)
    );
    assert_eq!(
        TpmsPcrSelection::unmarshal(&mut Reader::new(&[0x00, 0x0D, 3, 1, 0, 0])),
        Err(Rc::HASH)
    );
    let mut w = Writer::new();
    w.u32(1);
    w.u16(alg::SHA512);
    w.bytes(&[0; 64]);
    assert_eq!(
        TpmlDigestValues::unmarshal(&mut Reader::new(&w.finish())),
        Err(Rc::HASH)
    );
    // The sizes: three hashes, two banks.
    assert_eq!(
        [alg::SHA256, alg::SHA384, alg::SHA512, 0x0004, alg::NULL].map(digest_size),
        [Some(32), Some(48), Some(64), None, None]
    );
    assert_eq!(
        [alg::SHA256, alg::SHA384, alg::SHA512, 0x0004, alg::NULL].map(pcr_digest_size),
        [Some(32), Some(48), None, None, None]
    );
    // TPM2_PCR_Event still returns the two banks only.
    let r = run(&mut t, with_pw(cc::PCR_EVENT, &[0], &[b""], &tpm2b(b"abc")));
    let list = TpmlDigestValues::unmarshal(&mut Reader::new(params(&r, 0))).unwrap();
    assert_eq!(
        list.digests.iter().map(|d| d.hash).collect::<Vec<_>>(),
        [alg::SHA256, alg::SHA384]
    );
}

#[test]
fn signing_schemes_accept_sha2_hashes_and_encryption_schemes_do_not() {
    let mut t = started();
    // TPM2_TestParms decodes TPMT_PUBLIC_PARMS: RSASSA and ECDSA with SHA-256/384/512 ...
    for hash in ["000b", "000c", "000d"] {
        ok(&run(
            &mut t,
            plain(0x18A, &[], &hex(&format!("000100100014{hash}080000000000"))),
        ));
        ok(&run(
            &mut t,
            plain(0x18A, &[], &hex(&format!("002300100018{hash}00030010"))),
        ));
    }
    // ... but not OAEP, ECDH or a SHA-3 hash.
    for (encoded, error) in [
        ("000100100017000c080000000000", Rc::HASH), // OAEP with SHA-384.
        ("000100100017000d080000000000", Rc::HASH), // OAEP with SHA-512.
        ("002300100019000c00030010", Rc::HASH),     // ECDH with SHA-384.
        ("002300100019000d00030010", Rc::HASH),     // ECDH with SHA-512.
        ("0001001000140027080000000000", Rc::HASH), // RSASSA with SHA3-256.
        ("002300100018002700030010", Rc::HASH),     // ECDSA with SHA3-256.
        ("00080005000c", Rc::HASH), // The HMAC scheme of a keyed-hash key stays SHA-256.
    ] {
        err(
            &run(&mut t, plain(0x18A, &[], &hex(encoded))),
            error.in_param(1),
        );
    }
    // The signature structure accepts the three hashes, nothing else.
    for hash in [alg::SHA256, alg::SHA384, alg::SHA512] {
        let mut w = Writer::new();
        w.u16(alg::RSASSA);
        w.u16(hash);
        w.tpm2b(&[1; 256]);
        let sig = TpmtSignature::unmarshal(&mut Reader::new(&w.finish())).unwrap();
        assert_eq!(
            sig,
            TpmtSignature::Rsassa {
                hash,
                sig: vec![1; 256]
            }
        );
        let mut w = Writer::new();
        w.u16(alg::ECDSA);
        w.u16(hash);
        w.tpm2b(&[1; 32]);
        w.tpm2b(&[2; 32]);
        assert!(TpmtSignature::unmarshal(&mut Reader::new(&w.finish())).is_ok());
    }
    let mut w = Writer::new();
    w.u16(alg::ECDSA);
    w.u16(0x0027);
    assert_eq!(
        TpmtSignature::unmarshal(&mut Reader::new(&w.finish())),
        Err(Rc::HASH)
    );
    // A public area still names itself with SHA-256 or NULL only: nameAlg SHA-384/SHA-512 is a
    // later step (TPM_RC_HASH, as before hash agility).
    for (name_alg, accepted) in [
        (alg::SHA256, true),
        (alg::SHA384, false),
        (alg::SHA512, false),
        (alg::NULL, true),
        (0x0027, false),
        (0x0004, false),
    ] {
        let mut w = Writer::new();
        w.u16(alg::KEYEDHASH);
        w.u16(name_alg);
        w.u32(0);
        w.tpm2b(&[]);
        w.u16(alg::NULL);
        w.tpm2b(&[]);
        let parsed = TpmtPublic::unmarshal(&mut Reader::new(&w.finish()));
        assert_eq!(parsed.is_ok(), accepted, "nameAlg {name_alg:#06x}");
        if !accepted {
            assert_eq!(parsed.unwrap_err(), Rc::HASH);
        }
    }
}

// ------------------------------------------------- SHA-384 signatures stay refused (negative)

const NULL_VALIDATION: [u8; 8] = [0x80, 0x24, 0x40, 0, 0, 7, 0, 0];
const ECDSA_SHA384: Scheme = Scheme {
    alg: alg::ECDSA,
    hash: alg::SHA384,
};
const RSASSA_SHA384: Scheme = Scheme {
    alg: alg::RSASSA,
    hash: alg::SHA384,
};

/// An ECDSA or RSASSA signature structure labelled `hash` with dummy values.
fn labelled_signature(rsa: bool, hash: u16) -> Vec<u8> {
    let mut w = Writer::new();
    if rsa {
        w.u16(alg::RSASSA);
        w.u16(hash);
        w.tpm2b(&[1; 256]);
    } else {
        w.u16(alg::ECDSA);
        w.u16(hash);
        w.tpm2b(&[1; 32]);
        w.tpm2b(&[2; 32]);
    }
    w.finish()
}

/// `TPM2_PolicySigned` parameters (nonceTPM, cpHashA, policyRef, expiration) and the signature.
fn policy_signed_params(signature: &[u8]) -> Vec<u8> {
    cat(&[
        &tpm2b(b""),
        &tpm2b(b""),
        &tpm2b(b"ref"),
        &u32b(0),
        signature,
    ])
}

#[test]
fn policy_signed_refuses_a_sha384_signature_with_hash_for_parameter_5() {
    let mut t = started();
    // The authority key has a NULL scheme (RSA only: an ECC signing key must name ECDSA), or
    // ECDSA/RSASSA with SHA-384 (accepted by Create).
    let templates = [
        (false, ecc(base() | oa::SIGN, SymDef::Null, ECDSA_SHA384)),
        (true, rsa(base() | oa::SIGN, SymDef::Null, Scheme::NULL)),
        (true, rsa(base() | oa::SIGN, SymDef::Null, RSASSA_SHA384)),
    ];
    for (is_rsa, template) in templates {
        let created = create_primary(&mut t, OWNER, b"", &template);
        assert_eq!(rc(&created), 0, "{template:?}: {:#x}", rc(&created));
        let key = parse_primary(&created);
        let public = unmarshal_public(&key.public);
        let (authority, _) = parse_loaded(&load_external(&mut t, &public, NULL));
        let session = start_session(&mut t, se::POLICY, NULL, b"", &[0x44; 16]);
        for hash in [alg::SHA384, alg::SHA512] {
            let params = policy_signed_params(&labelled_signature(is_rsa, hash));
            err(
                &run(
                    &mut t,
                    plain(cc::POLICY_SIGNED, &[authority, session.handle], &params),
                ),
                Rc::HASH.in_param(5),
            );
        }
        // The same structure labelled SHA-256 passes the hash guard (and fails the signature).
        let params = policy_signed_params(&labelled_signature(is_rsa, alg::SHA256));
        let reply = run(
            &mut t,
            plain(cc::POLICY_SIGNED, &[authority, session.handle], &params),
        );
        assert_ne!(rc(&reply), Rc::HASH.in_param(5).0);
        ok(&flush(&mut t, authority));
        ok(&flush(&mut t, key.handle));
        ok(&flush(&mut t, session.handle));
    }
}

#[test]
fn signing_commands_refuse_a_key_whose_scheme_hash_is_sha384_with_scheme() {
    let mut t = started();
    let key = parse_primary(&create_primary(
        &mut t,
        OWNER,
        b"pw",
        &ecc(base() | oa::SIGN, SymDef::Null, ECDSA_SHA384),
    ))
    .handle;
    // TPM2_Sign: the key's scheme wins, so SHA-384 is TPM_RC_SCHEME for inScheme (parameter 2).
    err(
        &sign(&mut t, key, b"pw", &[0x11; 32], &u16b(alg::NULL)),
        Rc::SCHEME.in_param(2),
    );
    // TPM2_SignDigest: the key scheme is rejected for handle 1 before the digest is looked at.
    for digest in [vec![0x11; 32], vec![0x11; 48]] {
        let params = cat(&[&tpm2b(b""), &tpm2b(&digest), &NULL_VALIDATION]);
        err(
            &run(&mut t, with_pw(cc::SIGN_DIGEST, &[key], &[b"pw"], &params)),
            Rc::SCHEME.in_handle(1),
        );
    }
    // An attestation command (TPM2_GetTime, signHandle 2, inScheme parameter 2).
    err(
        &run(
            &mut t,
            with_pw(
                cc::GET_TIME,
                &[ENDORSEMENT, key],
                &[b"", b"pw"],
                &cat(&[&tpm2b(b"q"), &u16b(alg::NULL)]),
            ),
        ),
        Rc::SCHEME.in_param(2),
    );
    // A key with a NULL scheme cannot be pushed to SHA-384 through inScheme either: the scheme
    // structure of a signing command accepts SHA-256 only (TPM_RC_HASH).
    let open = parse_primary(&create_primary(
        &mut t,
        OWNER,
        b"pw",
        &rsa(base() | oa::SIGN, SymDef::Null, Scheme::NULL),
    ))
    .handle;
    err(
        &sign(
            &mut t,
            open,
            b"pw",
            &[0x11; 32],
            &cat(&[&u16b(alg::RSASSA), &u16b(alg::SHA384)]),
        ),
        Rc::HASH.in_param(2),
    );
}

#[test]
fn verify_commands_refuse_a_sha384_signature_label_with_hash() {
    let mut t = started();
    let key = parse_primary(&create_primary(&mut t, OWNER, b"pw", &ecc_sign())).handle;
    let signature = labelled_signature(false, alg::SHA384);
    // TPM2_VerifySignature: signature is parameter 2.
    err(
        &verify(&mut t, key, &[0x11; 32], &signature),
        Rc::HASH.in_param(2),
    );
    // TPM2_VerifyDigestSignature: signature is parameter 3.
    for digest in [vec![0x11; 32], vec![0x11; 48]] {
        let params = cat(&[&tpm2b(b""), &tpm2b(&digest), &signature]);
        err(
            &run(&mut t, plain(cc::VERIFY_DIGEST_SIGNATURE, &[key], &params)),
            Rc::HASH.in_param(3),
        );
    }
}

#[test]
fn object_verify_signature_binds_the_hash_label_to_the_digest_length() {
    let mut t = started();
    let key = parse_primary(&create_primary(&mut t, OWNER, b"pw", &ecc_sign())).handle;
    let digest = [0x5a; 32];
    let signed = sign(&mut t, key, b"pw", &digest, &ecc_sig_scheme());
    ok(&signed);
    let signature = TpmtSignature::unmarshal(&mut Reader::new(params(&signed, 0))).unwrap();
    let TpmtSignature::Ecdsa { r, s, .. } = &signature else {
        panic!("not an ECDSA signature");
    };
    let object = t.object(key).unwrap();
    assert!(object.verify_signature(&digest, &signature));
    // The same (r, s) under another hash label, or with a digest of another width, is refused.
    for hash in [alg::SHA384, alg::SHA512, 0x0027] {
        let relabelled = TpmtSignature::Ecdsa {
            hash,
            r: r.clone(),
            s: s.clone(),
        };
        assert!(
            !object.verify_signature(&digest, &relabelled),
            "{hash:#06x}"
        );
    }
    assert!(!object.verify_signature(&digest[..31], &signature));
    assert!(!object.verify_signature(&[0x5a; 48], &signature));
}
