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
    assert!(!more && c == cap::ALGS && count == 19);
    let entries: Vec<(u16, u32)> = body
        .chunks(6)
        .map(|e| {
            (
                u16::from_be_bytes([e[0], e[1]]),
                u32::from_be_bytes([e[2], e[3], e[4], e[5]]),
            )
        })
        .collect();
    assert_eq!(entries.len(), 19);
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
fn signing_schemes_accept_sha2_hashes_and_ecdh_does_not() {
    let mut t = started();
    // TPM2_TestParms decodes TPMT_PUBLIC_PARMS: RSASSA, RSAPSS, OAEP and ECDSA with SHA-256/384/512
    // (OAEP and RSAPSS with SHA-384/512 since the RSA-3072 slice; they were TPM_RC_HASH before) ...
    for hash in ["000b", "000c", "000d"] {
        for scheme in ["0014", "0016", "0017"] {
            ok(&run(
                &mut t,
                plain(
                    0x18A,
                    &[],
                    &hex(&format!("0001001000{}{hash}080000000000", &scheme[2..])),
                ),
            ));
        }
        ok(&run(
            &mut t,
            plain(0x18A, &[], &hex(&format!("002300100018{hash}00030010"))),
        ));
    }
    // ... but not ECDH or a SHA-3 hash.
    for (encoded, error) in [
        ("002300100019000c00030010", Rc::HASH), // ECDH with SHA-384.
        ("002300100019000d00030010", Rc::HASH), // ECDH with SHA-512.
        ("0001001000140027080000000000", Rc::HASH), // RSASSA with SHA3-256.
        ("002300100018002700030010", Rc::HASH), // ECDSA with SHA3-256.
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
    // A public area names itself with SHA-256, SHA-384, SHA-512 or NULL (step C: object Names).
    for (name_alg, accepted) in [
        (alg::SHA256, true),
        (alg::SHA384, true),
        (alg::SHA512, true),
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

// ---------------------------------------- SHA-384 signatures: size and scheme rules of the refusals

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
fn signing_commands_size_the_digest_by_the_sha384_scheme_of_the_key() {
    let mut t = started();
    let key = parse_primary(&create_primary(
        &mut t,
        OWNER,
        b"pw",
        &ecc(base() | oa::SIGN, SymDef::Null, ECDSA_SHA384),
    ))
    .handle;
    // Slice 1b: the key's scheme (SHA-384) now signs, so a SHA-256 digest of 32 octets is the wrong
    // size (TPM2_Sign: digest is parameter 1; TPM2_SignDigest: digest is parameter 2) ...
    err(
        &sign(&mut t, key, b"pw", &[0x11; 32], &u16b(alg::NULL)),
        Rc::SIZE.in_param(1),
    );
    let params = cat(&[&tpm2b(b""), &tpm2b(&[0x11; 32]), &NULL_VALIDATION]);
    err(
        &run(&mut t, with_pw(cc::SIGN_DIGEST, &[key], &[b"pw"], &params)),
        Rc::SIZE.in_param(2),
    );
    // ... and a 48-octet digest is signed.
    ok(&sign(&mut t, key, b"pw", &[0x11; 48], &u16b(alg::NULL)));
    // An attestation command (TPM2_GetTime) signs with the key's SHA-384 scheme.
    ok(&run(
        &mut t,
        with_pw(
            cc::GET_TIME,
            &[ENDORSEMENT, key],
            &[b"", b"pw"],
            &cat(&[&tpm2b(b"q"), &u16b(alg::NULL)]),
        ),
    ));
    // A key with a NULL scheme takes SHA-384 from inScheme (slice 1b): a 32-octet digest is then
    // the wrong size.
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
        Rc::SIZE.in_param(1),
    );
}

#[test]
fn verify_commands_size_the_digest_by_a_sha384_signature_label() {
    let mut t = started();
    let key = parse_primary(&create_primary(&mut t, OWNER, b"pw", &ecc_sign())).handle;
    let signature = labelled_signature(false, alg::SHA384);
    // Slice 1b. TPM2_VerifySignature accepts the label (digest is parameter 1): a 32-octet digest
    // is the wrong size, a 48-octet one reaches the check and the dummy signature fails it.
    err(
        &verify(&mut t, key, &[0x11; 32], &signature),
        Rc::SIZE.in_param(1),
    );
    err(&verify(&mut t, key, &[0x11; 48], &signature), Rc::SIGNATURE);
    // TPM2_VerifyDigestSignature uses the fixed scheme of the key (ECDSA with SHA-256), so the
    // SHA-384 label is a scheme mismatch (signature is parameter 3).
    for digest in [vec![0x11; 32], vec![0x11; 48]] {
        let params = cat(&[&tpm2b(b""), &tpm2b(&digest), &signature]);
        err(
            &run(&mut t, plain(cc::VERIFY_DIGEST_SIGNATURE, &[key], &params)),
            Rc::SCHEME.in_param(3),
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

// ------------------------------------------------------------------ step C: objects with SHA-384/512

fn hex_of(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn sha256_hex(b: &[u8]) -> String {
    hex_of(hash(HashAlg::Sha256, &[b]).as_slice())
}

/// Every reply of the SHA-256 object flow, as the SHA-256 digest of its bytes, and the size of
/// each private blob. Run once on the code before step C and pinned in the golden test below.
fn sha256_object_flow() -> Vec<(String, usize)> {
    use super::tests_obj::{create_child, evict, load, parse_child, sealed};
    let mut t = started();
    let mut out = Vec::new();
    let storage = create_primary(&mut t, OWNER, b"gold", &super::tests_obj::ecc_storage());
    out.push((sha256_hex(&storage), 0));
    let parent = parse_primary(&storage).handle;
    let flow = |t: &mut Tpm, public: &TpmtPublic, auth: &[u8], data: &[u8]| {
        let resp = create_child(t, parent, b"gold", auth, data, public);
        ok(&resp);
        let child = parse_child(&resp);
        let loaded = load(t, parent, b"gold", &child.private, &child.public);
        ok(&loaded);
        (
            vec![
                (sha256_hex(&resp), child.private.len()),
                (sha256_hex(&loaded), 0),
            ],
            parse_loaded(&loaded).0,
        )
    };
    let (rows, h) = flow(&mut t, &ecc_sign(), b"child-auth", b"");
    out.extend(rows);
    ok(&evict(&mut t, OWNER, b"", h, 0x8100_0001));
    out.push((sha256_hex(&t.persistent_snapshot()), 0));
    let (rows, h) = flow(&mut t, &sealed(), b"seal-auth", b"sealed-secret");
    out.extend(rows);
    ok(&flush(&mut t, h));
    let mut hmac_key = sealed();
    hmac_key.attributes = oa::FIXED_TPM
        | oa::FIXED_PARENT
        | oa::SENSITIVE_DATA_ORIGIN
        | oa::USER_WITH_AUTH
        | oa::SIGN;
    let (rows, h) = flow(&mut t, &hmac_key, b"", b"");
    out.extend(rows);
    ok(&flush(&mut t, h));
    out
}

// Taken on the code before step C (HEAD ea477d9): CreatePrimary (ECC storage), then Create and Load
// of an ECC signing key, sealed data and an HMAC key under it, the persistent snapshot after
// EvictControl of the first child. Rows are SHA-256 of the reply bytes (the private blobs and
// creation data are included, the keys come from the fixed entropy) and the private blob size.
const SHA256_FLOW_GOLDEN: [(&str, usize); 8] = [
    (
        "4e2320c599d9f9ba6de5905398b20dbb286d7ea187e15988933e2cf7a8642a59",
        0,
    ),
    (
        "88d8b52563e5c221c6a212e8bb3b859a2ab109bbce4c2a985bb674f5b2e38c71",
        126,
    ),
    (
        "99526b325d6ec2e762e4ae9acb5380316ea568d031e294012525367eb1877a2b",
        0,
    ),
    (
        "dd85da7377ca26be3b0f3370db5f139c4add19570fbac58656141168098c444c",
        0,
    ),
    (
        "6988f704f73ed933ec96095a36b3b9c66dbfb65979876208eb8bc3cab65cb6e5",
        139,
    ),
    (
        "283229ab8975a8a44df4cf755aa23485f685f5778b8f9d7c20e00ed404fef86e",
        0,
    ),
    (
        "3f70c29c4456816fb3de6af77eb67a0c37530019933c8c89019ab7a62249006b",
        158,
    ),
    (
        "14532f0a3d8ddd95aa54ea4fe7904d0042c0cc632ae50cae11b8a738c521fa53",
        0,
    ),
];

#[test]
fn golden_sha256_primary_and_private_blob_are_byte_identical() {
    let flow = sha256_object_flow();
    assert_eq!(flow.len(), SHA256_FLOW_GOLDEN.len());
    for (i, ((digest, size), (want, want_size))) in flow.iter().zip(SHA256_FLOW_GOLDEN).enumerate()
    {
        assert_eq!((digest.as_str(), *size), (want, want_size), "row {i}");
    }
}

/// Compares `actual` with the hex string `expected`.
fn expect_hex(label: &str, actual: &[u8], expected: &str) {
    assert_eq!(hex_of(actual), expected, "{label}");
}

fn named(mut public: TpmtPublic, name_alg: u16) -> TpmtPublic {
    public.name_alg = name_alg;
    public
}

/// `(handle, Name)` of a Load reply; a SHA-512 Name is 66 octets, one more than the shared helper takes.
fn loaded_name(resp: &[u8]) -> (u32, Vec<u8>) {
    ok(resp);
    let mut r = Reader::new(params(resp, 1));
    let name = r.tpm2b(66).unwrap().to_vec();
    r.finish().unwrap();
    (u32::from_be_bytes(resp[10..14].try_into().unwrap()), name)
}

/// `(handle, public, Name)` of a CreatePrimary reply, with a Name of up to 66 octets.
fn primary_parts(resp: &[u8]) -> (u32, Vec<u8>, Vec<u8>) {
    ok(resp);
    let mut r = Reader::new(params(resp, 1));
    let public = r.tpm2b(limits::MAX_RESPONSE_SIZE).unwrap().to_vec();
    r.tpm2b(1024).unwrap();
    r.tpm2b(64).unwrap();
    r.u16().unwrap();
    r.u32().unwrap();
    r.tpm2b(64).unwrap();
    let name = r.tpm2b(66).unwrap().to_vec();
    r.finish().unwrap();
    (
        u32::from_be_bytes(resp[10..14].try_into().unwrap()),
        public,
        name,
    )
}

fn child_of(
    t: &mut Tpm,
    parent: u32,
    public: &TpmtPublic,
    auth: &[u8],
    data: &[u8],
) -> (u32, Vec<u8>, Vec<u8>, Vec<u8>) {
    use super::tests_obj::{create_child, load, parse_child};
    let created = create_child(t, parent, b"parent-pw", auth, data, public);
    ok(&created);
    let child = parse_child(&created);
    let (h, name) = loaded_name(&load(
        t,
        parent,
        b"parent-pw",
        &child.private,
        &child.public,
    ));
    (h, name, child.public, child.private)
}

/// \`(public, Name, Qualified Name)\` of ReadPublic.
fn read_public_triple(t: &mut Tpm, h: u32) -> (Vec<u8>, Vec<u8>, Vec<u8>) {
    let resp = super::tests_obj::read_public(t, h);
    ok(&resp);
    let mut r = Reader::new(params(&resp, 0));
    let public = r.tpm2b(limits::MAX_RESPONSE_SIZE).unwrap().to_vec();
    let name = r.tpm2b(66).unwrap().to_vec();
    let qn = r.tpm2b(66).unwrap().to_vec();
    r.finish().unwrap();
    (public, name, qn)
}

fn startup_clear(t: &mut Tpm) {
    ok(&run(t, plain(cc::STARTUP, &[], &u16b(0))));
}

const DIGEST32: [u8; 32] = [0x5A; 32];
// Computed with Python hashlib from the parent public below: Name = nameAlg || H(public), QN = nameAlg
// || H(parentQN || Name) (the QN of a primary hashes the 4 octets of its hierarchy handle). The ECDSA and
// RSASSA signatures were verified with plain Python integer arithmetic (affine P-256, `pow(s, 65537, n)`
// against the EMSA-PKCS1-v1_5 block of SHA-256 over 0x5a x 32), not with the crates under test.
const PARENT_PUBLIC: &str = "0023000b0003007200000006008000430010000300100020970cb503b2b0fe8daa962a2ab82757af1d106a1e28670ad56ecc5371b096a9b50020fd45993854190f90d12faf794bbe18eba771f5f8dcc555f9a68a8ddd7f210b73";
const PARENT_QN: &str = "000b2f5cf7f48c43a91fd75c352a6b4e5ab54d33a8d9922a9ec9eca07a2d3881953c";
const SHA384_ECC_PUBLIC: &str = "0023000c00040072000000100018000b000300100020e433ca504e2f38bd4b2fda7857e9789b3c8c90b5069e9629aac9ce0f0b074e060020ef727aa1daa612aa1e964f251407541e829296be23c8402eaec1ce57d0dda789";
const SHA384_ECC_NAME: &str = "000c2229ec4350284ce4caa1e908bb19aaaad6022c3ab6f639925e5925bfd305bf0dad85e34cd9ebf67bf44537f31a5c5224";
const SHA384_ECC_QN: &str = "000cd1ee4adacf749e99acdb1e0db82fc11e1841cdcbe94b64b703981ba9febd8a858e204de66053b1cbd0eb6a5fd8657c96";
const SHA384_ECC_SIG: &str = "0018000b00207511e1dd918b4170c3de8699ae91da9d8a6f2de98f270058b5dd16bd474d665a0020670e3bffb06b2595cb7e4d2e1189247ebb3a060e2afea225367b93eda7c737bc";
const SHA384_RSA_PUBLIC: &str = "0001000c00040072000000100014000b0800000000000100c3a5201be5b52e1a36857f17df760dd0a68f4a8909e551eefb7440a90e0cf2a92fb0428eb626e964c5fd2973f6ae446e307da774e56826bad6c640adb054b3d4281c4a987c4a762ee059ed75c88c6b7c943787b23f1fd4dff3f9336d64daa0a46aa1527554326f0fd833eefdd0867518ddcea6045a38271fdd64e6aa12f92eefad7372b89f7c7228466ea9f728d03933df0073faa5756d9d59174177f1c671730c75c24388c91e4c9db81b8f0b7bd89fefde229b964f9c79158dfebaa58c2dd0a4399419e0871d67bc4c34a0676f7a037e79b66471ae9ead6cfa92f121719f57dc22734c31c099aeb3ac4985d73e5f6d02d90920a4ef4a915331e8664f6e257d";
const SHA384_RSA_NAME: &str = "000ca64543c05fc9f6b8d9815bbf168df1f2374b30a2694c8d59df07907741841fe0bd601f6fd82569b6195ab2a51ef352a7";
const SHA384_RSA_QN: &str = "000c75e04c00857d11eb5fafa32162c5513bf325809be9584c0786c351e414784192a6ee3ce645201a32f90f4b6822cb15b5";
const SHA384_RSA_SIG: &str = "0014000b0100b65540ded305834084478a22a129c79c8bd1d785536c46475d790d49459f2d5ef4be3c80c9f89c027ce5e61137e35a0e2afa34d12db4dbe13c163543775330c397a1ed226520f2c98158584c173240884facede3173e44360ba3467da2766a25dc35f49f4dba337f30f8280d8506c3e7f5c6f8ee9c58fa597314dcdf72f870a75c30705edc9cdeca5e06a51f605afb30f919dd69b28bcf1dcc6785cea5238114c20a129061fb25574fd54f7d6234bb18412384067509423e05da60382e35bcb5761c97aec4f772285d7de364de06779d5806115a09e1f2f32a47da1015879a3f3c0f245094d11d3798025e08b197e81a3adecc65ec84c672be42911a140b8232";
const SHA512_CHILD_PUBLIC: &str = "0023000d00040072000000100018000b000300100020e433ca504e2f38bd4b2fda7857e9789b3c8c90b5069e9629aac9ce0f0b074e060020ef727aa1daa612aa1e964f251407541e829296be23c8402eaec1ce57d0dda789";
const SHA512_CHILD_NAME: &str = "000d1ff1ece8919071b8bbdba740d8b186121e1b9978b7e577f257c6ba814af6e24ea03c5058f0a4deb4dfe3d3dabbe00764f9927a765591a5b21c72e7d25ba7c797";
const SHA512_CHILD_QN: &str = "000dcaeab9d991633e77e56202451b7e0244cb508fa44e5f3499ebe92a5529af50403506e4e9631347e495983b88ec16ce82b0c211f0abc0ddd3d704d031153031d6";
const SHA512_PRIMARY_PUBLIC: &str = "0023000d00040072000000100018000b0003001000202852a51f980a2493f630b9e89331c2fa95b9f618856f4f9bda314d75ce7db9db0020f32244f4dc210b13d59addc275f84f0f1b3c50f3207c957f3b156d6537ef4fa7";
const SHA512_PRIMARY_NAME: &str = "000db3600eaf617635d8ded900898f0ed6c7fd7c902c16c3bd55f1c12b5210a9b8cf1d42a4a212ff90a9896e1e9f1fe17ab034c1ab84906bc2ec0d48557a43db3b4a";
const SHA512_PRIMARY_QN: &str = "000dfb1126c07fd1cecc19bc57a733392c871948b7badd08297fc76191b3a238ff8d56e3e434f607bfb8422f7b0ee26e06caa91f6c5f777940025b17f95a57f89670";

#[test]
fn sha384_object_create_load_sign_verify_snapshot_restore() {
    use super::tests_obj::{evict, rsa_sig_scheme, rsa_sign};
    let (mut t, parent) = super::tests_obj::with_ecc_parent();
    let (parent_public, _, parent_qn) = read_public_triple(&mut t, parent);
    expect_hex("parent_public", &parent_public, PARENT_PUBLIC);
    expect_hex("parent_qn", &parent_qn, PARENT_QN);
    for (kind, template, scheme, want) in [
        (
            "ecc",
            ecc_sign(),
            ecc_sig_scheme(),
            [
                SHA384_ECC_PUBLIC,
                SHA384_ECC_NAME,
                SHA384_ECC_QN,
                SHA384_ECC_SIG,
            ],
        ),
        (
            "rsa",
            rsa_sign(),
            rsa_sig_scheme(),
            [
                SHA384_RSA_PUBLIC,
                SHA384_RSA_NAME,
                SHA384_RSA_QN,
                SHA384_RSA_SIG,
            ],
        ),
    ] {
        let template = named(template, alg::SHA384);
        let (h, name, public, _) = child_of(&mut t, parent, &template, b"child-auth", b"");
        assert_eq!(unmarshal_public(&public).name_alg, alg::SHA384);
        expect_hex(&format!("{kind}_public"), &public, want[0]);
        expect_hex(&format!("{kind}_name"), &name, want[1]);
        let (rp, rname, rqn) = read_public_triple(&mut t, h);
        assert_eq!((rp, rname.clone()), (public.clone(), name.clone()));
        expect_hex(&format!("{kind}_qn"), &rqn, want[2]);
        let signed = sign(&mut t, h, b"child-auth", &DIGEST32, &scheme);
        ok(&signed);
        let signature = params(&signed, 0).to_vec();
        expect_hex(&format!("{kind}_sig"), &signature, want[3]);
        ok(&verify(&mut t, h, &DIGEST32, &signature));
        // Persist, snapshot, restore: the same Name and Qualified Name, the same signature.
        let persistent = if kind == "ecc" {
            0x8100_0001
        } else {
            0x8100_0002
        };
        ok(&evict(&mut t, OWNER, b"", h, persistent));
        let mut r = Tpm::restore(&t.persistent_snapshot(), &[9; 48]).unwrap();
        startup_clear(&mut r);
        let (rp2, rname2, rqn2) = read_public_triple(&mut r, persistent);
        assert_eq!((rp2, rname2, rqn2), (public, name, rqn));
        let again = sign(&mut r, persistent, b"child-auth", &DIGEST32, &scheme);
        ok(&again);
        assert_eq!(params(&again, 0), &signature[..]);
        ok(&verify(&mut r, persistent, &DIGEST32, &signature));
    }
}

#[test]
#[allow(non_snake_case)]
fn sha512_object_name_is_nameAlg_000d() {
    let (mut t, parent) = super::tests_obj::with_ecc_parent();
    let template = named(ecc_sign(), alg::SHA512);
    let (h, name, public, _) = child_of(&mut t, parent, &template, b"child-auth", b"");
    assert_eq!(&name[..2], &[0x00, 0x0d]);
    assert_eq!(name.len(), 2 + 64);
    expect_hex("child_public", &public, SHA512_CHILD_PUBLIC);
    expect_hex("child_name", &name, SHA512_CHILD_NAME);
    let (_, _, qn) = read_public_triple(&mut t, h);
    assert_eq!(&qn[..2], &[0x00, 0x0d]);
    expect_hex("child_qn", &qn, SHA512_CHILD_QN);
    // A primary: the Qualified Name hashes the hierarchy handle and the Name.
    let (handle, primary_public, primary_name) = primary_parts(&create_primary(
        &mut t,
        OWNER,
        b"pw",
        &named(ecc_sign(), alg::SHA512),
    ));
    assert_eq!(&primary_name[..2], &[0x00, 0x0d]);
    expect_hex("primary_public", &primary_public, SHA512_PRIMARY_PUBLIC);
    expect_hex("primary_name", &primary_name, SHA512_PRIMARY_NAME);
    let (_, _, pqn) = read_public_triple(&mut t, handle);
    expect_hex("primary_qn", &pqn, SHA512_PRIMARY_QN);
    ok(&sign(&mut t, handle, b"pw", &DIGEST32, &ecc_sig_scheme()));
}

#[test]
fn sha384_object_context_save_load() {
    let (mut t, parent) = super::tests_obj::with_ecc_parent();
    let template = named(ecc_sign(), alg::SHA384);
    let (h, name, _, _) = child_of(&mut t, parent, &template, b"child-auth", b"");
    let (_, _, qn) = read_public_triple(&mut t, h);
    let before = sign(&mut t, h, b"child-auth", &DIGEST32, &ecc_sig_scheme());
    ok(&before);
    let saved = run(&mut t, plain(cc::CONTEXT_SAVE, &[h], &[]));
    ok(&saved);
    let context = params(&saved, 0).to_vec();
    ok(&flush(&mut t, h));
    let resp = run(&mut t, plain(cc::CONTEXT_LOAD, &[], &context));
    ok(&resp);
    let h2 = u32::from_be_bytes(resp[10..14].try_into().unwrap());
    let (_, name2, qn2) = read_public_triple(&mut t, h2);
    assert_eq!((name2, qn2), (name, qn));
    let after = sign(&mut t, h2, b"child-auth", &DIGEST32, &ecc_sig_scheme());
    assert_eq!(params(&after, 0), params(&before, 0));
    // The authValue and the private scalar are not in the clear in the context.
    assert!(!super::tests_obj::contains(&context, b"child-auth"));
    assert!(!super::tests_obj::contains(
        &context,
        &super::tests_obj::secret_of(&t, h2)
    ));
    // A SHA-384 storage parent keeps its seed through a context as well.
    let storage = parse_primary(&create_primary(
        &mut t,
        OWNER,
        b"sp",
        &named(super::tests_obj::ecc_storage(), alg::SHA384),
    ));
    let child = super::tests_obj::parse_child(&super::tests_obj::create_child(
        &mut t,
        storage.handle,
        b"sp",
        b"x",
        b"data",
        &named(super::tests_obj::sealed(), alg::SHA384),
    ));
    let saved = run(&mut t, plain(cc::CONTEXT_SAVE, &[storage.handle], &[]));
    ok(&saved);
    let pctx = params(&saved, 0).to_vec();
    ok(&flush(&mut t, storage.handle));
    let resp = run(&mut t, plain(cc::CONTEXT_LOAD, &[], &pctx));
    ok(&resp);
    let p2 = u32::from_be_bytes(resp[10..14].try_into().unwrap());
    ok(&super::tests_obj::load(
        &mut t,
        p2,
        b"sp",
        &child.private,
        &child.public,
    ));
}

#[test]
fn object_authpolicy_length_follows_name_alg() {
    use super::tests_obj::create_child;
    let (mut t, parent) = super::tests_obj::with_ecc_parent();
    // The accepted authPolicy is exactly one digest of the nameAlg: 32, 48 or 64 octets.
    for (name_alg, size) in [(alg::SHA256, 32), (alg::SHA384, 48), (alg::SHA512, 64)] {
        let mut template = named(ecc_sign(), name_alg);
        template.auth_policy = vec![0xA5; size];
        ok(&create_child(
            &mut t,
            parent,
            b"parent-pw",
            b"",
            b"",
            &template,
        ));
        ok(&create_primary(&mut t, OWNER, b"", &template));
        for h in t.objects.loaded_handles() {
            if h != parent {
                ok(&flush(&mut t, h));
            }
        }
        let wrong = [1, 16, 32, 48, 64, size + 1, 65, 66];
        for len in wrong.into_iter().filter(|len| *len != size) {
            template.auth_policy = vec![0xA5; len];
            err(
                &create_child(&mut t, parent, b"parent-pw", b"", b"", &template),
                Rc::SIZE.in_param(2),
            );
            err(
                &create_primary(&mut t, OWNER, b"", &template),
                Rc::SIZE.in_param(2),
            );
        }
        // A public-only object follows the same rule.
        let mut external = named(ecc_sign(), name_alg);
        external.attributes = oa::SIGN | oa::USER_WITH_AUTH;
        let (public, ..) = {
            let created = super::tests_obj::parse_child(&create_child(
                &mut t,
                parent,
                b"parent-pw",
                b"",
                b"",
                &named(ecc_sign(), name_alg),
            ));
            (created.public,)
        };
        external.unique = unmarshal_public(&public).unique;
        external.auth_policy = vec![0xA5; size];
        ok(&load_external(&mut t, &external, NULL));
        external.auth_policy = vec![0xA5; if size == 48 { 32 } else { 48 }];
        err(
            &load_external(&mut t, &external, NULL),
            Rc::SIZE.in_param(2),
        );
    }
}

#[test]
fn user_auth_length_follows_name_alg() {
    // Frees every transient object but `keep`: the TPM has eight object slots.
    fn flush_all_but(t: &mut Tpm, keep: u32) {
        for h in t.objects.loaded_handles() {
            if h != keep {
                ok(&flush(t, h));
            }
        }
    }
    use super::tests_obj::create_child;
    let (mut t, parent) = super::tests_obj::with_ecc_parent();
    for (name_alg, size) in [(alg::SHA256, 32), (alg::SHA384, 48), (alg::SHA512, 64)] {
        let template = named(ecc_sign(), name_alg);
        let auth: Vec<u8> = (1..=size as u8).collect();
        let (h, name, ..) = child_of(&mut t, parent, &template, &auth, b"");
        // The whole authValue is the key of an HMAC session, however long it is. (A password session
        // carries at most 32 octets until the session layer follows, steps D and E.) The cpHash
        // covers the Name of the object, 34, 50 or 66 octets.
        let mut session = super::tests_b::start_hmac(&mut t, NULL, b"", &[0x11; 16]);
        let mut command = super::tests_b::hc(
            cc::SIGN,
            &[h],
            super::tests_obj::sign_params(&DIGEST32, &ecc_sig_scheme()),
            (h, &auth),
        );
        command.names = vec![name];
        ok(&super::tests_b::run_hmac(&mut t, &mut session, &command));
        // One octet more than the digest is TPM_RC_SIZE for parameter 1 (inSensitive).
        let mut long = auth.clone();
        long.push(0x7f);
        err(
            &create_child(&mut t, parent, b"parent-pw", &long, b"", &template),
            Rc::SIZE.in_param(1),
        );
        err(
            &create_primary(&mut t, OWNER, &long, &template),
            Rc::SIZE.in_param(1),
        );
        ok(&create_primary(&mut t, OWNER, &auth, &template));
        flush_all_but(&mut t, parent);
    }
    // ObjectChangeAuth takes the digest size of the object's nameAlg as well.
    for (name_alg, size) in [(alg::SHA256, 32), (alg::SHA384, 48), (alg::SHA512, 64)] {
        let mut template = named(super::tests_obj::sealed(), name_alg);
        template.attributes |= oa::NO_DA;
        let (h, ..) = child_of(&mut t, parent, &template, b"old", b"secret");
        let change = |t: &mut Tpm, auth: &[u8]| {
            super::tests_b::pw_cmd(
                t,
                cc::OBJECT_CHANGE_AUTH,
                &[h, parent],
                b"old",
                &tpm2b(auth),
            )
        };
        let reply = change(&mut t, &vec![0x33; size]);
        ok(&reply);
        err(&change(&mut t, &vec![0x33; size + 1]), Rc::SIZE.in_param(1));
        // The new blob loads with the new authValue and unseals the same data.
        let mut r = Reader::new(params(&reply, 0));
        let private = r.tpm2b(512).unwrap().to_vec();
        r.finish().unwrap();
        let public = read_public_triple(&mut t, h).0;
        let (h2, _) = loaded_name(&super::tests_obj::load(
            &mut t,
            parent,
            b"parent-pw",
            &private,
            &public,
        ));
        let (_, name2) = (h2, read_public_triple(&mut t, h2).1);
        let mut session = super::tests_b::start_hmac(&mut t, NULL, b"", &[0x22; 16]);
        let mut command =
            super::tests_b::hc(cc::UNSEAL, &[h2], Vec::new(), (h2, &vec![0x33; size]));
        command.names = vec![name2];
        let unsealed = super::tests_b::run_hmac(&mut t, &mut session, &command);
        ok(&unsealed);
        assert_eq!(params(&unsealed, 0), tpm2b(b"secret"));
        flush_all_but(&mut t, parent);
    }
}

// Computed with Python hashlib/hmac and the `openssl enc -aes-128-cfb` command line: HMACkey =
// KDFa(SHA-384, parentSeed, "INTEGRITY", NULL, NULL, 384), tag = HMAC-SHA-384(HMACkey, symIv TPM2B ||
// encSensitive || Name), symKey = KDFa(SHA-384, parentSeed, "STORAGE", Name, NULL, 128); the decrypted
// sensitive is the TPM2B of (TPM_ALG_KEYEDHASH, authValue padded to 48 octets, a 48-octet seedValue,
// "sealed-data").
const SHA384_PARENT_SEED: &str = "fff823da7b2a9ce5c712c358bd3e601da02eab57fca61fc1bacdf80638a0624d6c1c17e8d612a5364290a9813e95611e";
const SHA384_BLOB_TAG: &str = "037830b6e9415aeedbf76d45465ce6644e24569cbcc06ab2277d5d736ab6ae18ef985c9c13853843fda17706cad833b1";
const SHA384_BLOB_PUBLIC: &str = "0008000c00000452000000100030fa128e3101c3fcadab930854ef1c290f70a3ea26e1086c5a85c4bc3ae0e384b53c068064271123a0e0c7e96ca1273405";
const SHA384_BLOB_NAME: &str = "000c7d8db4ad95e19a8db14d4ccf777c0f3d01c994c6645315cfab58863a0fc0a341e4c76c279be99a0a355b38a61c507bec";
const SHA384_BLOB: &str = "0030037830b6e9415aeedbf76d45465ce6644e24569cbcc06ab2277d5d736ab6ae18ef985c9c13853843fda17706cad833b10010fff9690bde8aad0397efc5f7e644b5aa425247fe9aa1206259adeda84343ba7caacd87e821d520824c23b33811043ce747012bb81a9a98d0e967875075b1aa2133ec8c746aa592318564151b0de6f41c75114a0bf5703858c79af61fed34bd40fa750cbff86b26e01a92cf07316dfc693bd2c8f6d907cb6c3ba2c15a7e0d738bfaf509a60a";

#[test]
fn protect_unprotect_round_trip_with_sha384_parent() {
    use super::tests_obj::{create_child, ecc_storage, load, parse_child, sealed};
    let mut t = started();
    let storage = parse_primary(&create_primary(
        &mut t,
        OWNER,
        b"sp",
        &named(ecc_storage(), alg::SHA384),
    ));
    assert_eq!(unmarshal_public(&storage.public).name_alg, alg::SHA384);
    let seed = t
        .object(storage.handle)
        .unwrap()
        .sensitive
        .as_ref()
        .unwrap()
        .seed
        .to_vec();
    // The seed value of a storage key is one digest of its nameAlg.
    assert_eq!(seed.len(), 48);
    expect_hex("parent_seed", &seed, SHA384_PARENT_SEED);
    let mut template = named(sealed(), alg::SHA384);
    template.attributes |= oa::NO_DA;
    let auth = [0x61u8; 48];
    let created = parse_child(&create_child(
        &mut t,
        storage.handle,
        b"sp",
        &auth,
        b"sealed-data",
        &template,
    ));
    let blob = created.private.clone();
    // integrity (TPM2B, an HMAC-SHA-384 tag) || symIv (TPM2B, 16) || encSensitive: size 2 + type 2
    // + authValue 2 + 48 + seedValue 2 + 48 + data 2 + 11.
    assert_eq!(&blob[..2], &[0, 48]);
    assert_eq!(&blob[50..52], &[0, 16]);
    assert_eq!(blob.len(), 50 + 18 + 2 + 2 + 50 + 50 + 13);
    expect_hex("blob_tag", &blob[2..50], SHA384_BLOB_TAG);
    expect_hex("blob", &blob, SHA384_BLOB);
    expect_hex("blob_public", &created.public, SHA384_BLOB_PUBLIC);
    let name = {
        let mut n = alg::SHA384.to_be_bytes().to_vec();
        n.extend_from_slice(hash(HashAlg::Sha384, &[&created.public]).as_slice());
        n
    };
    expect_hex("blob_name", &name, SHA384_BLOB_NAME);
    let (h, reloaded_name) =
        loaded_name(&load(&mut t, storage.handle, b"sp", &blob, &created.public));
    assert_eq!(reloaded_name, name);
    let mut session = super::tests_b::start_hmac(&mut t, NULL, b"", &[0x33; 16]);
    let mut command = super::tests_b::hc(cc::UNSEAL, &[h], Vec::new(), (h, &auth));
    command.names = vec![name.clone()];
    let unsealed = super::tests_b::run_hmac(&mut t, &mut session, &command);
    ok(&unsealed);
    assert_eq!(params(&unsealed, 0), tpm2b(b"sealed-data"));
    // Any changed octet of the tag, the IV or the ciphertext is TPM_RC_INTEGRITY, before decryption.
    for at in [2, 20, 49, 52, 60, 67, 68, 100, blob.len() - 1] {
        let mut bad = blob.clone();
        bad[at] ^= 0x01;
        err(
            &load(&mut t, storage.handle, b"sp", &bad, &created.public),
            Rc::INTEGRITY,
        );
    }
    // A public area with another nameAlg is another Name, and the same blob does not open for it.
    let mut other = unmarshal_public(&created.public);
    other.name_alg = alg::SHA512;
    err(
        &load(&mut t, storage.handle, b"sp", &blob, &other.to_bytes()),
        Rc::INTEGRITY,
    );
    // A tag of the SHA-256 size in front of the rest of the blob does not split into the container.
    let short = [&[0u8, 32][..], &blob[50..]].concat();
    err(
        &load(&mut t, storage.handle, b"sp", &short, &created.public),
        Rc::SIZE,
    );
    // A SHA-256 child of the SHA-384 parent is protected with the parent's hash as well.
    let child = parse_child(&create_child(
        &mut t,
        storage.handle,
        b"sp",
        b"c",
        b"d",
        &sealed(),
    ));
    assert_eq!(&child.private[..2], &[0, 48]);
    let (_, child_name) = loaded_name(&load(
        &mut t,
        storage.handle,
        b"sp",
        &child.private,
        &child.public,
    ));
    assert_eq!(&child_name[..2], &[0x00, 0x0b]);
    assert_eq!(child_name.len(), 34);
}

// Python hashlib: SHA384_CREATION_HASH = SHA-384(SHA384_CREATION), and the pcrDigest inside it is
// SHA-384 of the 32 zero octets of PCR 0 in the SHA-256 bank.
const SHA384_CREATION: &str = "00000001000b030100000030a38fff4ba26c15e4ac9cde8c03103ac89080fd47545fde9446c8f192729eab7bd03a4d5c3187f75fe2a71b0ee50a4a4001000b0022000b67f34e33d70b07b2ed7d3a7427cc5abf19e60fd72cfa754305c2a29739fb750d0022000b2f5cf7f48c43a91fd75c352a6b4e5ab54d33a8d9922a9ec9eca07a2d3881953c00076f757473696465";
const SHA384_CREATION_HASH: &str = "5270337d9e130cfbe053c46ce167e85ad9b4b7b5834c23616621207f5ab9119a915fe7526da91f4606c60b72aed577fb";
const SHA384_PCR0: &str = "0000000000000000000000000000000000000000000000000000000000000000";

#[test]
fn sha384_object_creation_hash_and_pcr_digest_use_the_name_alg() {
    use super::tests_obj::{parse_child, sealed};
    let (mut t, parent) = super::tests_obj::with_ecc_parent();
    // PCR 0 of the SHA-256 bank is the only selected value; the banks themselves stay SHA-256/384.
    let selection = [0, 0, 0, 1, 0x00, 0x0b, 3, 0x01, 0, 0];
    let pcr0 = t.pcrs.value(alg::SHA256, 0).unwrap().to_vec();
    expect_hex("pcr0", &pcr0, SHA384_PCR0);
    let create = |t: &mut Tpm, name_alg: u16| {
        let params = super::tests_obj::creation_params(
            b"auth",
            b"data",
            &named(sealed(), name_alg),
            b"outside",
            &selection,
        );
        let resp = run(t, with_pw(cc::CREATE, &[parent], &[b"parent-pw"], &params));
        ok(&resp);
        parse_child(&resp)
    };
    let child = create(&mut t, alg::SHA384);
    expect_hex("creation", &child.creation, SHA384_CREATION);
    // creationHash = H_nameAlg(creationData); the ticket stays an HMAC-SHA-256 under the hierarchy proof.
    expect_hex("creation_hash", &child.hash, SHA384_CREATION_HASH);
    assert_eq!(child.hash.len(), 48);
    assert_eq!(
        (child.ticket.0, child.ticket.1, child.ticket.2.len()),
        (st::CREATION, OWNER, 32)
    );
    let sha256_child = create(&mut t, alg::SHA256);
    assert_eq!(sha256_child.hash.len(), 32);
    assert_eq!(
        sha256_child.hash,
        hash(HashAlg::Sha256, &[&sha256_child.creation]).as_slice()
    );
}
