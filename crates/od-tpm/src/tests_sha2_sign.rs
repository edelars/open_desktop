//! SHA-384 and SHA-512 signing schemes (hash agility, slice 1b): TPM2_Sign, SignDigest, the sign and
//! verify sequences, the attestation commands and the verification commands with RSASSA and ECDSA
//! keys whose scheme names SHA-384 or SHA-512. The expected signatures are checked outside this
//! implementation, by `openssl` (`dgst -verify`, `pkeyutl -verify`) in the tests that run it; the
//! SHA-256 flow is pinned to the bytes the code produced before this slice (commit 077f9bc).

use super::tests_b::{define, nv_write};
use super::tests_c2::parse_attestation;
use super::tests_c2::{certify, quote, sel04};
use super::tests_obj::{
    ENDORSEMENT, NO_PCR, NULL, OWNER, Primary, base, create_child, create_primary, ecc,
    ecc_sig_scheme, ecc_sign, load, load_external, parse_child, parse_loaded, parse_primary, rsa,
    rsa_sign, sign, unmarshal_public, verify,
};
use super::*;
use crate::codes::object_attr as oa;
use crate::crypto::{HashAlg, ecc_sign as ecc_sign_digest, hash};
use crate::marshal::{Scheme, SymDef, TpmtPublic, Unique, Writer};
use crate::nv::attr as nv_attr;
use crate::object::Secret;

fn hex_of(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn digest_of(hash_alg: HashAlg, data: &[u8]) -> Vec<u8> {
    hash(hash_alg, &[data]).to_vec()
}

fn sha256_hex(b: &[u8]) -> String {
    hex_of(&digest_of(HashAlg::Sha256, b))
}

fn scheme_bytes(s: Scheme) -> Vec<u8> {
    let mut w = Writer::new();
    s.marshal(&mut w);
    w.finish()
}

/// `TPMT_TK_HASHCHECK` of the owner hierarchy as the `Hash` command returns it.
fn hash_ticket(resp: &[u8]) -> (Vec<u8>, Vec<u8>) {
    let mut rd = Reader::new(params(resp, 0));
    let digest = rd.tpm2b(limits::MAX_HASH_SIZE).unwrap().to_vec();
    let ticket = rd.bytes(rd.remaining()).unwrap().to_vec();
    (digest, ticket)
}

fn sequence_sign(t: &mut Tpm, key: u32, key_pw: &[u8], parts: &[&[u8]]) -> Vec<u8> {
    let auth = b"seq";
    let start = run(
        t,
        plain(
            cc::SIGN_SEQUENCE_START,
            &[key],
            &cat(&[&tpm2b(auth), &tpm2b(&[])]),
        ),
    );
    let sequence = seq_handle(&start);
    let (last, head) = parts.split_last().unwrap();
    for part in head {
        ok(&seq_update(t, sequence, auth, part));
    }
    run(
        t,
        with_pw(
            cc::SIGN_SEQUENCE_COMPLETE,
            &[sequence, key],
            &[auth, key_pw],
            &tpm2b(last),
        ),
    )
}

fn sequence_verify(t: &mut Tpm, key: u32, signature: &[u8], parts: &[&[u8]]) -> Vec<u8> {
    let auth = b"vseq";
    let start = run(
        t,
        plain(
            cc::VERIFY_SEQUENCE_START,
            &[key],
            &cat(&[&tpm2b(auth), &tpm2b(&[]), &tpm2b(&[])]),
        ),
    );
    let sequence = seq_handle(&start);
    for part in parts {
        ok(&seq_update(t, sequence, auth, part));
    }
    run(
        t,
        with_pw(
            cc::VERIFY_SEQUENCE_COMPLETE,
            &[sequence, key],
            &[auth],
            signature,
        ),
    )
}

fn sign_digest_cmd(t: &mut Tpm, key: u32, pw: &[u8], digest: &[u8]) -> Vec<u8> {
    let params = cat(&[&tpm2b(b""), &tpm2b(digest), &NULL_VALIDATION]);
    run(t, with_pw(cc::SIGN_DIGEST, &[key], &[pw], &params))
}

fn verify_digest_cmd(t: &mut Tpm, key: u32, digest: &[u8], signature: &[u8]) -> Vec<u8> {
    let params = cat(&[&tpm2b(b""), &tpm2b(digest), signature]);
    run(t, plain(cc::VERIFY_DIGEST_SIGNATURE, &[key], &params))
}

const NULL_VALIDATION: [u8; 8] = [0x80, 0x24, 0x40, 0, 0, 7, 0, 0];
const NV0: u32 = 0x0100_0000;

/// Every reply of the SHA-256 signing flow (keys, Sign, SignDigest, legacy Sign with a HASHCHECK
/// ticket, sign and verify sequences, the three verification commands and the five attestation
/// commands), as the SHA-256 of its bytes. Taken on commit 077f9bc, before this slice.
fn sha256_sign_flow() -> Vec<String> {
    let mut t = started();
    let mut out = Vec::new();
    let mut note = |reply: Vec<u8>| {
        ok(&reply);
        out.push(sha256_hex(&reply));
        reply
    };
    let message = b"golden message";
    let digest = digest_of(HashAlg::Sha256, message);
    let rsa_scheme = scheme_bytes(Scheme {
        alg: alg::RSASSA,
        hash: alg::SHA256,
    });
    let null = u16b(alg::NULL);
    let ecc_key = parse_primary(&note(create_primary(&mut t, OWNER, b"pw", &ecc_sign())));
    let rsa_key = parse_primary(&note(create_primary(&mut t, OWNER, b"pw", &rsa_sign())));
    let restricted = parse_primary(&note(create_primary(
        &mut t,
        OWNER,
        b"pw",
        &ecc(
            base() | oa::RESTRICTED | oa::SIGN,
            SymDef::Null,
            ECDSA_SHA256,
        ),
    )));
    let mut ecc_sig = Vec::new();
    let mut rsa_sig = Vec::new();
    for (key, scheme) in [(&ecc_key, ecc_sig_scheme()), (&rsa_key, rsa_scheme.clone())] {
        let a = note(sign(&mut t, key.handle, b"pw", &digest, &null));
        let b = note(sign(&mut t, key.handle, b"pw", &digest, &scheme));
        assert_eq!(a, b);
        let c = note(sign_digest_cmd(&mut t, key.handle, b"pw", &digest));
        assert_eq!(params(&a, 0), params(&c, 0));
        note(verify(&mut t, key.handle, &digest, params(&a, 0)));
        note(verify_digest_cmd(
            &mut t,
            key.handle,
            &digest,
            params(&a, 0),
        ));
        if key.handle == ecc_key.handle {
            ecc_sig = params(&a, 0).to_vec();
        } else {
            rsa_sig = params(&a, 0).to_vec();
        }
        let sequence = note(sequence_sign(
            &mut t,
            key.handle,
            b"pw",
            &[b"golden ", b"message"],
        ));
        assert_eq!(params(&sequence, 0), params(&a, 0));
    }
    let verified = note(sequence_verify(
        &mut t,
        rsa_key.handle,
        &rsa_sig,
        &[b"golden message"],
    ));
    assert!(!verified.is_empty() && !ecc_sig.is_empty());
    let hashed = note(run(&mut t, hash_cmd(message, alg::SHA256, OWNER)));
    let (hashed_digest, ticket) = hash_ticket(&hashed);
    note(run(
        &mut t,
        with_pw(
            cc::SIGN,
            &[restricted.handle],
            &[b"pw"],
            &cat(&[&tpm2b(&hashed_digest), &null, &ticket]),
        ),
    ));
    let qualifying = b"fresh";
    for key in [&ecc_key, &rsa_key] {
        note(quote(&mut t, key.handle, b"pw", qualifying, &null, &NO_PCR));
        note(certify(
            &mut t,
            ecc_key.handle,
            key.handle,
            [b"pw", b"pw"],
            qualifying,
            &null,
        ));
        note(run(
            &mut t,
            with_pw(
                cc::GET_TIME,
                &[ENDORSEMENT, key.handle],
                &[b"", b"pw"],
                &cat(&[&tpm2b(qualifying), &null]),
            ),
        ));
    }
    let index = NV0 + 1;
    ok(&define(
        &mut t,
        OWNER,
        index,
        nv_attr::OWNERWRITE | nv_attr::OWNERREAD,
        40,
    ));
    ok(&nv_write(&mut t, OWNER, index, b"", &[0x5c; 40], 0));
    for (size, offset) in [(8u16, 4u16), (0, 0)] {
        for key in [&ecc_key, &rsa_key] {
            note(run(
                &mut t,
                with_pw(
                    cc::NV_CERTIFY,
                    &[key.handle, OWNER, index],
                    &[b"pw", b""],
                    &cat(&[&tpm2b(qualifying), &null, &u16b(size), &u16b(offset)]),
                ),
            ));
        }
    }
    let storage = parse_primary(&create_primary(
        &mut t,
        OWNER,
        b"parent",
        &super::tests_obj::ecc_storage(),
    ));
    let child = parse_child(&create_child(
        &mut t,
        storage.handle,
        b"parent",
        b"child",
        b"",
        &ecc_sign(),
    ));
    let (loaded, _) = parse_loaded(&load(
        &mut t,
        storage.handle,
        b"parent",
        &child.private,
        &child.public,
    ));
    let creation = cat(&[
        &tpm2b(qualifying),
        &tpm2b(&child.hash),
        &null,
        &u16b(child.ticket.0),
        &u32b(child.ticket.1),
        &tpm2b(&child.ticket.2),
    ]);
    note(run(
        &mut t,
        with_pw(
            cc::CERTIFY_CREATION,
            &[ecc_key.handle, loaded],
            &[b"pw"],
            &creation,
        ),
    ));
    out
}

const ECDSA_SHA256: Scheme = Scheme {
    alg: alg::ECDSA,
    hash: alg::SHA256,
};

const GOLDEN: [&str; 29] = [
    "29e2e3f0909910c6d2d376156a7ade399dfe34a3dbfc71b30191466b2db7d182",
    "5ff8a6f6d9dd196b1d85a1989a8a5d2a31bf61e591be407b910af389d5af5e73",
    "6b50226577b04d6d271dc82476644460db23cfecf2876c8d49fa0c6ed29a34c8",
    "436595108cd3988e2a17154552c5d3f4f88271f7cc3d27abd8511b80b7aae81e",
    "436595108cd3988e2a17154552c5d3f4f88271f7cc3d27abd8511b80b7aae81e",
    "436595108cd3988e2a17154552c5d3f4f88271f7cc3d27abd8511b80b7aae81e",
    "d7ca35db032435a178e8c83d97c28b477a7a6b75c10bd6af75497360b54dc87e",
    "7b033128468c96d6861eee75b38f31d62b89fed6a6347374f30abaa675743b02",
    "efb189e1cc33479e3594e6ceaddc7c197351aecbaead03bddc6854665c7545ce",
    "ce009866512a7537c4e7fb45b3d976e256c31fa21ad5607f1c87f1e64fe5d9f7",
    "ce009866512a7537c4e7fb45b3d976e256c31fa21ad5607f1c87f1e64fe5d9f7",
    "ce009866512a7537c4e7fb45b3d976e256c31fa21ad5607f1c87f1e64fe5d9f7",
    "630bf288b1236e97bf564c9a22f73ef4e738e283004288ce031355e471b4a92d",
    "6d8dd632274509d5c7d38e65892dcea5d5d8769fc48d2d76673c90106c08d6c7",
    "561bbebfc007e937f7d146a16a1816bc0057985c651e1f9a6045906726227d1e",
    "dc7d410eb234b7735b1d7ada4b6ca666e9643887d4513eafba745a727357ea1c",
    "bfcf5f4b9414c2de83afd9f69b32fc154bddb259e7e0e02120f3b2569277e1c4",
    "ef6d7179e6cfb235cac4aaaf009ac0c451832385273844665293b2f742c93c79",
    "1e89ca4a7b15c4a3fe1dffc8c12c7ecee97928c4d93dded13efbb4819bf01b11",
    "c3c01e41756324fd7103e06a3f48db54d4e2530a351e01ad1a106e053ad25608",
    "276f04a11d019aa28549985e985d2c06b5470f6baf4b48a2d793c0ef5bf43228",
    "d94b33e80c79dc62c02cb9ddc8d05839f20a8f873242f465c128bfbf38b257be",
    "729ef4fe2d8f453602ea561ffa534e0053b712bd0af042764ae91399623da0e4",
    "16fa8e61dd83b76d60081bb0a1187cfcd3ef11234aabd456aa0e6c4c16888f9d",
    "11d498fb085f1e3270769d6e3402c6cfe6514ad1cf3faf82915af5fbe020b1c4",
    "846494ad2ad9428e05e3b26c54493144058d359d359c0a89d57f7c6791b7bce3",
    "6ab4ecc4d95767ef045fe106d367afc940f7fae6668ba34f5c8e169fa878766f",
    "051f03e187f9a77d06b73ecacca6081820587ce58c9819cf1305e99289af92f0",
    "eb420116611dab4e19ccf02dea74fc509cce16cab4cbe9cbc3b4639b9103c12e",
];

#[test]
fn sha256_sign_golden_unchanged() {
    let flow = sha256_sign_flow();
    assert_eq!(flow, GOLDEN.to_vec());
}

/// Vectors checked outside this implementation (OpenSSL: 'openssl dgst -shaX -verify' on the message, and
/// 'openssl pkeyutl -verify' on the raw digest for ECDSA): the SignDigest signature of the digest of
/// MESSAGE and a Quote (qualifyingData "fresh", no PCR) of a deterministic owner primary key with the
/// named scheme and the auth "pw". The pcrDigest is the digest of the empty string under the hash of the scheme (Part 3, clause
/// 18.4.1; Quote.c: PCRComputeCurrentDigest with the hash of the scheme), so 48 and 64 octets for
/// SHA-384 and SHA-512. The attests and signatures were regenerated and verified by
/// gen_quote_vectors.py (kept outside the repository, in the session scratchpad): it takes the header
/// of the previous TPMS_ATTEST and replaces only the pcrDigest tail, which hashlib computes independently;
/// the signature is verified with the openssl CLI. The header fields (magic, clock, firmware) are not
/// independently verified by this script.
struct Vector {
    rsa: bool,
    hash: u16,
    /// The modulus (RSA) or x || y (ECC) of the public key.
    public: &'static str,
    /// TPMT_SIGNATURE of SignDigest(H(MESSAGE)).
    signature: &'static str,
    /// TPMS_ATTEST of the Quote and its TPMT_SIGNATURE.
    attest: &'static str,
    attest_signature: &'static str,
}

const VECTORS: [Vector; 4] = [
    Vector {
        rsa: true,
        hash: alg::SHA384,
        public: "c2decf9bc011dd85b6db04865946660f98d798fcc78204a6bbf94944cd32bfe0e32f05b3a2a8433e322e2878afe90322911e96cfd68323ddc4e114ee3a156da5d92345ed6ee2446bb6c678847ab95debd6252f44fc11b86af502a85e97cbd7588697ed3152b316e089ec28a706f2b2b12f547eedf7b1c73db69a64cfe79dd5588c9d932caefef380f0e9bd291a9603b512ac4f9b64b194e3a94c4b23b9d67eed04432f8e72f765b619d4ced16bee11286ef2a0ca153f1aad7337fe702c81c3a29096adc4c641e03e5c3fe60efbdcdc93e770544b9176eaa63331f90b9ab2fb15cc25d877c5a54e6b770dc580979aedfccceb954ae37815a028dd1d35aabf9901",
        signature: "0014000c01003a03651ad761a65d9360965bad1949697c1cd7194e1c4d303cb24968b73766e766900b21c88a576f270ade1039ec35b5dde60d72c61286f14ffb7a0651aaa57d0ddf471c376402377568c29f5d22f20c475b3efab7ce2cd081fdad6e205014d83b65bb7f4caa9b91c00344a62b8626a13a88debf22529dbf81f89eed14ecd2feaaca541bcbece4f506e9dace481f8b5ebd7d67a535f08328f7db2ebc86c03f0b3cf6472f4a9ba7d6f3abc8f199d41c4ae1b3c666e1862e4c60b30435d72f9f93d7d8d550e32c9a741870415005379b56bd1efc8bd52597b511ced0d0ef9cb54f189fda4b98f6fbbe6ed34d7d8e3ca1c7ef7cca3ae547875caafb5af6985ea03f",
        attest: "ff54434780180022000bae524f93e9219d35c16a43c64473399fec5bddf6ac9a59a3c02c988ccbd52580000566726573680000000000000000320b3c5392d07306015e591ac5ec86697400000000003038b060a751ac96384cd9327eb1b1e36a21fdb71114be07434c0cc7bf63f6e1da274edebfe76f65fbd51ad2f14898b95b",
        attest_signature: "0014000c01002ba18e2b69c7bb8a25ea50da402455a8b37ac280b509b3cc82e29568b64f5c0d1b3971613427e912eb1ecb600f14417c075a73c4c4e4e4cccaf54abc23b1b861a4c2266caa8e0ff7a6c0c3428cb2184a376acc7153715f5f2c053d24b3d39bf39078371ab2a9e9cdbfbe5401d7b74d58886162ec60ba37bab0e26e86498e24dacc09964b2cf7e08f29b78685bf8e2f5fb44b1426a74a807fca3316fa9f31214e09e982e2fc347b990f38c6d9bfa6246dd88d2b82ef73fc519c01049294eca325adf1aab5292745b9bd3b64fdbd4f9b634cee0cd745eb304fe2caacd847e77222835f7e4a2e1eb07ae969a82a27e654f2c207b28a5f06e17653831088168ea612",
    },
    Vector {
        rsa: false,
        hash: alg::SHA384,
        public: "6b3e7a69cc0c59971737655ecfa37d7a996ede5bd3e88c5c41e558ceb02355f02d67f0aad841a6458509cb38b840089fe257729a6ffa1e00142d27161d8ad795",
        signature: "0018000c00206e97d1a96613a8f2a3f257690abe32b7d69bb50da30ee897198d6183a91247c90020646a89653ddb62da9275f9e9452c14714546f0c2fabd5678ffd258b5caa6f731",
        attest: "ff54434780180022000b720e4607b7a96a8355070a580fb3b84e708e46c2b5bd8e7d6264d12d9fda34260005667265736800000000000000004748f2c8ed76035e01108febf8f71f3a8400000000003038b060a751ac96384cd9327eb1b1e36a21fdb71114be07434c0cc7bf63f6e1da274edebfe76f65fbd51ad2f14898b95b",
        attest_signature: "0018000c00207599a513503d4dd812c12ddcc1c913a22861183eb518354652c05fb5e0a7db9a002068e3f688351e4a22a10bcc1c536d8f6cc099528d2fea0d2af1a998c0a807a06b",
    },
    Vector {
        rsa: true,
        hash: alg::SHA512,
        public: "9d6ef2cc5c8cc5b1a726cc36dc1c0edf3cc3af6ef25f6a8135646de6df10da8ceaf3bbe0c68991f8642c746d483ac469036185267b0d9382f360974db796fd680544631215b70f990d647215fd242bc2c1041415b7d38f6d886201bd2b54dd51f4e159bb82878833aee54e93e25007b6921572c7fc108aa8543ce4585e01862b98dd9c1e7450dc7015a0632fe0f16bd20822f792e26d9bae2a4781b6b4a9dfaf8ddfc87bd50a2c70717f582dd9e7adbde9c0fc5f27c9c064f9fc4577bced90ccad9e0ce940705a24f9daad8a60e2ae36a6fdb29eed6d3b4d57f5b714841445b047efdd7075d29b62f2b9486da88b8204a6ddb4d83f3f02ae9f2465f97899c0cd",
        signature: "0014000d010079a4d119bce721744b40aebcb40f97edf26c3cb1868feafb93882a44f383f24ae7d133cd2578c5725ca832c723133597e123f2d7912eb22048ad2b77cbbe79dcc274435877c460239cc201c822531cd7da97dd6b4ad4c9c8fe37418db08f5731294ec4b43ddd3c6fee3c3c6bd3310c9572f512e6c3c2a63fff7ada5a18af50b4af0d8c371d9edf41d27a686b976cf9305708221434b48d8ee79ad0d6bd80089fa4926e03caee8fe0f8fd1f82099553c8748b4b0dbd9bc351feac25a9e463ae7b6b7811eacfd4c8b1193550a046d6458fe973a327000eb748b859048b9018455f177989646e414d6b87b8e08b3006080986bdbf585b9f089321b30ac6c4735d41",
        attest: "ff54434780180022000bb91b2955512fe3eb83196501e6609ea894cac5242f844691768dcee8e2b00ee300056672657368000000000000000000f7cafddd13230e0199c1cbd4338a8870000000000040cf83e1357eefb8bdf1542850d66d8007d620e4050b5715dc83f4a921d36ce9ce47d0d13c5d85f2b0ff8318d2877eec2f63b931bd47417a81a538327af927da3e",
        attest_signature: "0014000d01007111a09a8c414d502ac5560d860312a9cdc6c027e45eebde52216b5d8f7691f6b3fbd21786590e2740af781b31866485ed897ffff286dec0941d570c008c12698573699566cc0a77e79a3967514ca8f473c4b8212d527233521c64de3f2f405ec8a844dcbfd7fa6677462eee41fe781844ca115f3ae12dd6ce57d407b0f1823ba2cda4c644eb8199ceeedcd405eb9c2957db15eaf48e59a8974ef0a8cd96582f4f1f31b1da92c317e9626ab28ed6ad507ddb343cff68b571236c6f4f328e6f5c46a3956988446d3a8c0a41c3c2867796fd0bd7305eb0e31a00f2a8a319070df8546a370ba4e85d802bf7637b4d4f3d2cfa5597614ae2e37599c39fd056f62dca",
    },
    Vector {
        rsa: false,
        hash: alg::SHA512,
        public: "ad5fa46a51590c1528ea3bf97612035f579c00fb39bb4057f67fab428bf8d68f6820f819e38af55003d0e78952b0ec0a6348eedf3032888c7cfa33147389db67",
        signature: "0018000d0020c84bc505e0c32eb941bc6f8d2715fb63b2f98433c3ce7798ea9519450d1fca730020970da2ffe9aed2e4d44f77c421c6706d024e0956c01b50bceca0f4139b11c354",
        attest: "ff54434780180022000b61e98dd7ba53f761a8a6ef0f027b31a04eb036e93f7c8ec8edf30b4107d3fde6000566726573680000000000000000058cf7c3daa6829801ea77264113466178000000000040cf83e1357eefb8bdf1542850d66d8007d620e4050b5715dc83f4a921d36ce9ce47d0d13c5d85f2b0ff8318d2877eec2f63b931bd47417a81a538327af927da3e",
        attest_signature: "0018000d00208760e4c24e6780d19c3d4f0db8ae3782ca51bb2f4bae021fa273a8556cc3457e00209a884560f709478248ec531fcb364b90b51ce4aa3b4d1d397445ec7828215a72",
    },
];

const MESSAGE: &[u8] = b"slice 1b: hash agility signing";
/// Python hashlib: sha384(MESSAGE), sha512(MESSAGE), and the digests of 40 octets 0x5c (the NV data).
const MESSAGE_SHA384: &str = "012d2fbb824b2484ad3977ea449de6d24640143dfc18b25fc33a6f25c17353e093f7a266ba85a93cbac50da349d3d262";
const MESSAGE_SHA512: &str = "fdff692bc78e1a7378665b7d775e317b67ef644a91ef2eb00193e53934da8c5a6743302b5ed3db31b046ab3c881bd4feca322bc45850c8e55d6e94eca5e7c9b0";
const NV_DATA_SHA384: &str = "bc28600eb0712be191175331349440dc796f17a65f717188e72014bd30c5075ebaa7a4765f18d353b4006baa449006df";
const NV_DATA_SHA512: &str = "2f9bbdf581fe28a0a63a35749922e96398c5223952d52b573879d1547af01cbdb3a936a34927d7793df064f1025473878420f8237106e6e256ab0f3848f8de9e";
/// SHA3-256 and SM3: algorithms that are no signing hash of this TPM.
const SHA3_256: u16 = 0x0027;
const SM3_256: u16 = 0x0012;

fn scheme_of(rsa_key: bool, hash: u16) -> Scheme {
    Scheme {
        alg: if rsa_key { alg::RSASSA } else { alg::ECDSA },
        hash,
    }
}

fn signing_template(rsa_key: bool, hash: u16) -> TpmtPublic {
    let scheme = scheme_of(rsa_key, hash);
    if rsa_key {
        rsa(base() | oa::SIGN, SymDef::Null, scheme)
    } else {
        ecc(base() | oa::SIGN, SymDef::Null, scheme)
    }
}

fn hash_of(hash: u16) -> HashAlg {
    HashAlg::from_alg(hash).expect("a signing hash")
}

fn vector(rsa_key: bool, hash: u16) -> &'static Vector {
    VECTORS
        .iter()
        .find(|v| v.rsa == rsa_key && v.hash == hash)
        .expect("a pinned vector")
}

/// The owner primary key of a vector (deterministic in the seed and the template), checked to be
/// the key the vector was verified against.
fn vector_key(t: &mut Tpm, v: &Vector) -> Primary {
    let key = parse_primary(&create_primary(
        t,
        OWNER,
        b"pw",
        &signing_template(v.rsa, v.hash),
    ));
    let public = match unmarshal_public(&key.public).unique {
        Unique::Rsa(n) => hex_of(&n),
        Unique::Ecc { x, y } => hex_of(&[x, y].concat()),
        _ => panic!("not an asymmetric key"),
    };
    assert_eq!(public, v.public);
    key
}

/// The same signature structure relabelled with another hash.
fn relabel(signature: &[u8], hash: u16) -> Vec<u8> {
    let mut relabelled = signature.to_vec();
    relabelled[2..4].copy_from_slice(&hash.to_be_bytes());
    relabelled
}

/// A key with the given attributes and scheme, owner primary, auth "pw".
fn primary_with(t: &mut Tpm, public: &TpmtPublic) -> Primary {
    let reply = create_primary(t, OWNER, b"pw", public);
    assert_eq!(rc(&reply), 0, "{public:?}");
    parse_primary(&reply)
}

#[test]
fn sign_digest_rsassa_sha384_matches_openssl() {
    // The signature below was verified outside this implementation:
    //   openssl dgst -sha384 -verify rsa.pem -signature sig.bin message
    // (verify_vectors.py: the same command with the message altered fails).
    let v = vector(true, alg::SHA384);
    let mut t = started();
    let key = vector_key(&mut t, v);
    let digest = digest_of(HashAlg::Sha384, MESSAGE);
    assert_eq!(hex_of(&digest), MESSAGE_SHA384);
    let reply = sign_digest_cmd(&mut t, key.handle, b"pw", &digest);
    ok(&reply);
    assert_eq!(hex_of(params(&reply, 0)), v.signature);
    // TPM2_Sign over the same digest gives the same bytes, with or without a matching inScheme.
    for scheme in [u16b(alg::NULL), scheme_bytes(scheme_of(true, alg::SHA384))] {
        let again = sign(&mut t, key.handle, b"pw", &digest, &scheme);
        ok(&again);
        assert_eq!(params(&again, 0), params(&reply, 0));
    }
}

#[test]
fn sign_digest_ecdsa_p256_sha384_verifies_with_openssl() {
    // Checked outside this implementation, for the message and for the raw 48-octet digest (which
    // OpenSSL cuts to the leftmost 256 bits itself, FIPS 186-4 section 6.4):
    //   openssl dgst -sha384 -verify ec.pem -signature sig.der message
    //   openssl pkeyutl -verify -pubin -inkey ec.pem -in digest.bin -sigfile sig.der
    let v = vector(false, alg::SHA384);
    let mut t = started();
    let key = vector_key(&mut t, v);
    let digest = digest_of(HashAlg::Sha384, MESSAGE);
    assert_eq!(digest.len(), 48);
    let reply = sign_digest_cmd(&mut t, key.handle, b"pw", &digest);
    ok(&reply);
    assert_eq!(hex_of(params(&reply, 0)), v.signature);
    ok(&verify(&mut t, key.handle, &digest, params(&reply, 0)));
    // P-256 uses the leftmost 256 bits of a longer digest (FIPS 186-4 section 6.4: the leftmost
    // min(N, outlen) bits, N = 256; the p256 crate's prehash signer keeps the leftmost field-size
    // octets of a longer prehash), so the signature (RFC 6979 nonce included) equals the one over
    // the first 32 octets of the digest. OpenSSL agrees (pkeyutl above).
    let object = t.object(key.handle).unwrap();
    let Some(Secret::Ecc(private)) = object.sensitive.as_ref().map(|s| &s.secret) else {
        panic!("not an ECC private key");
    };
    assert_eq!(
        ecc_sign_digest(private, &digest),
        ecc_sign_digest(private, &digest[..32])
    );
    assert!(ecc_sign_digest(private, &digest).is_some());
}

#[test]
fn sign_sha512_scheme_rsassa_and_ecdsa() {
    // Checked outside this implementation like the SHA-384 vectors (openssl dgst -sha512 -verify).
    let digest = digest_of(HashAlg::Sha512, MESSAGE);
    assert_eq!(hex_of(&digest), MESSAGE_SHA512);
    for rsa_key in [true, false] {
        let v = vector(rsa_key, alg::SHA512);
        let mut t = started();
        let key = vector_key(&mut t, v);
        let reply = sign_digest_cmd(&mut t, key.handle, b"pw", &digest);
        ok(&reply);
        assert_eq!(hex_of(params(&reply, 0)), v.signature, "rsa {rsa_key}");
        assert_eq!(params(&reply, 0)[..4], {
            let mut head = u16b(scheme_of(rsa_key, alg::SHA512).alg);
            head.extend_from_slice(&alg::SHA512.to_be_bytes());
            head
        });
        for scheme in [
            u16b(alg::NULL),
            scheme_bytes(scheme_of(rsa_key, alg::SHA512)),
        ] {
            let again = sign(&mut t, key.handle, b"pw", &digest, &scheme);
            ok(&again);
            assert_eq!(params(&again, 0), params(&reply, 0));
        }
        // The 64-octet digest is sized by the scheme: 32 and 48 octets are refused.
        for wrong in [32, 48, 63, 65] {
            let bytes = vec![0x77; wrong];
            err(
                &sign(&mut t, key.handle, b"pw", &bytes, &u16b(alg::NULL)),
                Rc::SIZE.in_param(1),
            );
            err(
                &sign_digest_cmd(&mut t, key.handle, b"pw", &bytes),
                Rc::SIZE.in_param(2),
            );
        }
    }
}

#[test]
fn sign_refuses_inscheme_hash_mismatch() {
    let mut t = started();
    let digest384 = vec![0x33; 48];
    let digest512 = vec![0x44; 64];
    let key = primary_with(&mut t, &signing_template(false, alg::SHA384)).handle;
    // The key's scheme (ECDSA, SHA-384) wins; inScheme must repeat it (algorithm and hash) or be NULL.
    for in_scheme in [
        scheme_of(false, alg::SHA256),
        scheme_of(false, alg::SHA512),
        scheme_of(true, alg::SHA384),
    ] {
        for digest in [&digest384[..], &digest512[..], &[0x55; 32][..]] {
            err(
                &sign(&mut t, key, b"pw", digest, &scheme_bytes(in_scheme)),
                Rc::SCHEME.in_param(2),
            );
        }
    }
    for in_scheme in [u16b(alg::NULL), scheme_bytes(scheme_of(false, alg::SHA384))] {
        ok(&sign(&mut t, key, b"pw", &digest384, &in_scheme));
    }
    // A SHA-256 key does not accept SHA-384 or SHA-512 either.
    let key256 = primary_with(&mut t, &rsa_sign()).handle;
    for hash in [alg::SHA384, alg::SHA512] {
        err(
            &sign(
                &mut t,
                key256,
                b"pw",
                &digest384,
                &scheme_bytes(scheme_of(true, hash)),
            ),
            Rc::SCHEME.in_param(2),
        );
    }
    // The attestation commands name inScheme as parameter 2 too (TPM2_GetTime).
    let get_time = |t: &mut Tpm, scheme: &[u8]| {
        run(
            t,
            with_pw(
                cc::GET_TIME,
                &[ENDORSEMENT, key],
                &[b"", b"pw"],
                &cat(&[&tpm2b(b"q"), scheme]),
            ),
        )
    };
    err(
        &get_time(&mut t, &scheme_bytes(scheme_of(false, alg::SHA256))),
        Rc::SCHEME.in_param(2),
    );
    ok(&get_time(
        &mut t,
        &scheme_bytes(scheme_of(false, alg::SHA384)),
    ));
    // A key without a scheme needs inScheme, which then gives the hash (and so the digest size).
    let open = primary_with(&mut t, &rsa(base() | oa::SIGN, SymDef::Null, Scheme::NULL)).handle;
    err(
        &sign(&mut t, open, b"pw", &digest384, &u16b(alg::NULL)),
        Rc::SCHEME.in_param(2),
    );
    err(
        &sign(
            &mut t,
            open,
            b"pw",
            &digest384,
            &scheme_bytes(scheme_of(true, alg::SHA512)),
        ),
        Rc::SIZE.in_param(1),
    );
    ok(&sign(
        &mut t,
        open,
        b"pw",
        &digest512,
        &scheme_bytes(scheme_of(true, alg::SHA512)),
    ));
    // A hash that is no signing hash is TPM_RC_HASH, as before.
    for hash in [SHA3_256, SM3_256] {
        err(
            &sign(
                &mut t,
                open,
                b"pw",
                &digest384,
                &scheme_bytes(scheme_of(true, hash)),
            ),
            Rc::HASH.in_param(2),
        );
        err(
            &get_time(&mut t, &scheme_bytes(scheme_of(false, hash))),
            Rc::HASH.in_param(2),
        );
    }
}

#[test]
fn sign_sequence_sha384_matches_one_shot() {
    for (rsa_key, hash) in [
        (true, alg::SHA384),
        (false, alg::SHA384),
        (true, alg::SHA512),
        (false, alg::SHA512),
    ] {
        let v = vector(rsa_key, hash);
        let mut t = started();
        let key = vector_key(&mut t, v);
        // The sequence hashes with the scheme's hash: the result is the SignDigest signature of the
        // pinned vector (which OpenSSL verified), whatever the split of the message.
        let sequence = sequence_sign(
            &mut t,
            key.handle,
            b"pw",
            &[&MESSAGE[..7], &MESSAGE[7..20], &MESSAGE[20..]],
        );
        ok(&sequence);
        assert_eq!(hex_of(params(&sequence, 0)), v.signature);
        let one_shot = sign_digest_cmd(
            &mut t,
            key.handle,
            b"pw",
            &digest_of(hash_of(hash), MESSAGE),
        );
        assert_eq!(params(&sequence, 0), params(&one_shot, 0));
        // The verify sequence hashes with the scheme's hash and accepts it; a signature labelled
        // with another hash is a scheme mismatch (signature is parameter 1), another message a
        // bad signature. The failed completions leave the sequence in place.
        let auth = b"vseq";
        let start = run(
            &mut t,
            plain(
                cc::VERIFY_SEQUENCE_START,
                &[key.handle],
                &cat(&[&tpm2b(auth), &tpm2b(&[]), &tpm2b(&[])]),
            ),
        );
        let handle = seq_handle(&start);
        ok(&seq_update(&mut t, handle, auth, &MESSAGE[..9]));
        let complete = |t: &mut Tpm, signature: &[u8]| {
            run(
                t,
                with_pw(
                    cc::VERIFY_SEQUENCE_COMPLETE,
                    &[handle, key.handle],
                    &[auth],
                    signature,
                ),
            )
        };
        let signature = params(&sequence, 0).to_vec();
        err(&complete(&mut t, &signature), Rc::SIGNATURE);
        ok(&seq_update(&mut t, handle, auth, &MESSAGE[9..]));
        let other = if hash == alg::SHA384 {
            alg::SHA512
        } else {
            alg::SHA256
        };
        err(
            &complete(&mut t, &relabel(&signature, other)),
            Rc::SCHEME.in_param(1),
        );
        err(
            &complete(&mut t, &relabel(&signature, SHA3_256)),
            Rc::HASH.in_param(1),
        );
        ok(&complete(&mut t, &signature));
    }
}

#[test]
fn legacy_sign_hashcheck_ticket_follows_the_scheme_hash() {
    let mut t = started();
    // A restricted key signs only a digest the TPM hashed itself (HASHCHECK ticket).
    let restricted = primary_with(
        &mut t,
        &ecc(
            base() | oa::RESTRICTED | oa::SIGN,
            SymDef::Null,
            scheme_of(false, alg::SHA384),
        ),
    )
    .handle;
    let sign_with_ticket = |t: &mut Tpm, digest: &[u8], ticket: &[u8]| {
        run(
            t,
            with_pw(
                cc::SIGN,
                &[restricted],
                &[b"pw"],
                &cat(&[&tpm2b(digest), &u16b(alg::NULL), ticket]),
            ),
        )
    };
    let hashed = run(&mut t, hash_cmd(MESSAGE, alg::SHA384, OWNER));
    let (digest, ticket) = hash_ticket(&hashed);
    assert_eq!(hex_of(&digest), MESSAGE_SHA384);
    let signed = sign_with_ticket(&mut t, &digest, &ticket);
    ok(&signed);
    ok(&verify(&mut t, restricted, &digest, params(&signed, 0)));
    // A tampered digest does not match the ticket.
    let mut forged = digest.clone();
    forged[47] ^= 1;
    err(
        &sign_with_ticket(&mut t, &forged, &ticket),
        Rc::TICKET.in_param(3),
    );
    // A SHA-256 digest and its ticket are the wrong size for the SHA-384 scheme.
    let narrow = run(&mut t, hash_cmd(MESSAGE, alg::SHA256, OWNER));
    let (narrow_digest, narrow_ticket) = hash_ticket(&narrow);
    err(
        &sign_with_ticket(&mut t, &narrow_digest, &narrow_ticket),
        Rc::SIZE.in_param(1),
    );
    // SignDigest takes the same ticket for the digest of the scheme's hash.
    let params_ = cat(&[&tpm2b(b""), &tpm2b(&digest), &ticket]);
    let by_digest = run(
        &mut t,
        with_pw(cc::SIGN_DIGEST, &[restricted], &[b"pw"], &params_),
    );
    ok(&by_digest);
    assert_eq!(params(&by_digest, 0), params(&signed, 0));
}

#[test]
fn verify_signature_accepts_sha384_label_with_48_octet_digest() {
    for (rsa_key, hash) in [
        (true, alg::SHA384),
        (false, alg::SHA384),
        (true, alg::SHA512),
        (false, alg::SHA512),
    ] {
        let v = vector(rsa_key, hash);
        let mut t = started();
        let key = vector_key(&mut t, v);
        let signature = hex(v.signature);
        let digest = digest_of(hash_of(hash), MESSAGE);
        // A public-only copy of the key verifies as well.
        let (external, _) =
            parse_loaded(&load_external(&mut t, &unmarshal_public(&key.public), NULL));
        for handle in [key.handle, external] {
            ok(&verify(&mut t, handle, &digest, &signature));
            ok(&verify_digest_cmd(&mut t, handle, &digest, &signature));
            // The digest must have the width of the label.
            for width in [0, 31, 32, 47, 48, 49, 64, 65] {
                if width == digest.len() {
                    continue;
                }
                let wrong = vec![0x11; width];
                err(
                    &verify(&mut t, handle, &wrong, &signature),
                    Rc::SIZE.in_param(1),
                );
                err(
                    &verify_digest_cmd(&mut t, handle, &wrong, &signature),
                    Rc::SIZE.in_param(2),
                );
            }
            // A wrong digest of the right width is a bad signature.
            let mut other = digest.clone();
            other[0] ^= 1;
            err(&verify(&mut t, handle, &other, &signature), Rc::SIGNATURE);
            err(
                &verify_digest_cmd(&mut t, handle, &other, &signature),
                Rc::SIGNATURE,
            );
            // The label selects the width: SHA-256 or the other wide hash with this digest.
            for label in [alg::SHA256, alg::SHA384, alg::SHA512] {
                if label == hash {
                    continue;
                }
                err(
                    &verify(&mut t, handle, &digest, &relabel(&signature, label)),
                    Rc::SIZE.in_param(1),
                );
                // TPM2_VerifyDigestSignature checks the key's fixed scheme (signature is
                // parameter 3).
                err(
                    &verify_digest_cmd(&mut t, handle, &digest, &relabel(&signature, label)),
                    Rc::SCHEME.in_param(3),
                );
            }
            // A hash that is no signing hash is TPM_RC_HASH for the signature.
            for label in [SHA3_256, SM3_256] {
                err(
                    &verify(&mut t, handle, &digest, &relabel(&signature, label)),
                    Rc::HASH.in_param(2),
                );
                err(
                    &verify_digest_cmd(&mut t, handle, &digest, &relabel(&signature, label)),
                    Rc::HASH.in_param(3),
                );
            }
        }
    }
}

#[test]
fn certify_creation_with_sha384_object_creation_hash() {
    let mut t = started();
    let signer = primary_with(&mut t, &ecc_sign());
    let signer384 = primary_with(&mut t, &signing_template(false, alg::SHA384));
    // The certified object names itself with SHA-384: its creation hash has 48 octets.
    let mut template = ecc_sign();
    template.name_alg = alg::SHA384;
    let object = parse_primary(&create_primary(&mut t, OWNER, b"object", &template));
    assert_eq!(object.hash.len(), 48);
    let creation_args = |hash: &[u8], ticket: &(u16, u32, Vec<u8>)| {
        cat(&[
            &tpm2b(b"fresh"),
            &tpm2b(hash),
            &u16b(alg::NULL),
            &u16b(ticket.0),
            &u32b(ticket.1),
            &tpm2b(&ticket.2),
        ])
    };
    let certify_creation = |t: &mut Tpm, signer: u32, args: &[u8]| {
        run(
            t,
            with_pw(
                cc::CERTIFY_CREATION,
                &[signer, object.handle],
                &[b"pw"],
                args,
            ),
        )
    };
    for (signer, hash) in [(&signer, alg::SHA256), (&signer384, alg::SHA384)] {
        let reply = certify_creation(
            &mut t,
            signer.handle,
            &creation_args(&object.hash, &object.ticket),
        );
        ok(&reply);
        let (attest, signature) = parse_attestation(&reply);
        assert_eq!(attest[4..6], [0x80, 0x1a]);
        assert!(attest.ends_with(&cat(&[&tpm2b(&object.name), &tpm2b(&object.hash)])));
        // The attestation is signed over its digest of the signing scheme's hash.
        assert_eq!(signature[2..4], hash.to_be_bytes());
        let attest_digest = digest_of(hash_of(hash), &attest);
        ok(&verify(&mut t, signer.handle, &attest_digest, &signature));
    }
    // Only the size of creationHash is checked against the nameAlg of the object (a SHA-256
    // object keeps its 32 octets); the ticket digest remains an HMAC-SHA-256.
    let mut too_long = object.hash.clone();
    too_long.push(0);
    err(
        &certify_creation(
            &mut t,
            signer.handle,
            &creation_args(&too_long, &object.ticket),
        ),
        Rc::SIZE.in_param(2),
    );
    // A shorter value fits the limit and fails the ticket instead, as for any creation hash.
    err(
        &certify_creation(
            &mut t,
            signer.handle,
            &creation_args(&[0x11; 32], &object.ticket),
        ),
        Rc::TICKET.in_param(4),
    );
    // A different 48-octet value is a creation hash the ticket does not cover.
    err(
        &certify_creation(
            &mut t,
            signer.handle,
            &creation_args(&[0x11; 48], &object.ticket),
        ),
        Rc::TICKET.in_param(4),
    );
    let small = parse_primary(&create_primary(&mut t, OWNER, b"small", &ecc_sign()));
    for width in [33, 48] {
        let args = creation_args(&vec![0x11; width], &small.ticket);
        err(
            &run(
                &mut t,
                with_pw(
                    cc::CERTIFY_CREATION,
                    &[signer.handle, small.handle],
                    &[b"pw"],
                    &args,
                ),
            ),
            Rc::SIZE.in_param(2),
        );
    }
}

#[test]
fn quote_or_certify_with_sha384_scheme_verifies() {
    for v in &VECTORS {
        let mut t = started();
        let key = vector_key(&mut t, v);
        // The pinned Quote was verified by OpenSSL over the TPMS_ATTEST with the scheme's hash.
        let reply = quote(
            &mut t,
            key.handle,
            b"pw",
            b"fresh",
            &u16b(alg::NULL),
            &NO_PCR,
        );
        ok(&reply);
        let (attest, signature) = parse_attestation(&reply);
        assert_eq!(hex_of(&attest), v.attest);
        assert_eq!(hex_of(&signature), v.attest_signature);
        assert_eq!(attest[4..6], [0x80, 0x18]);
        // The attestation commands accept the same hash in inScheme.
        let named = quote(
            &mut t,
            key.handle,
            b"pw",
            b"fresh",
            &scheme_bytes(scheme_of(v.rsa, v.hash)),
            &NO_PCR,
        );
        assert_eq!(
            parse_attestation(&named),
            (attest.clone(), signature.clone())
        );
        // The digest of the TPMS_ATTEST has the width of the hash: only that one verifies.
        let attest_digest = digest_of(hash_of(v.hash), &attest);
        ok(&verify(&mut t, key.handle, &attest_digest, &signature));
        let narrow = digest_of(HashAlg::Sha256, &attest);
        err(
            &verify(&mut t, key.handle, &narrow, &signature),
            Rc::SIZE.in_param(1),
        );
    }
}

/// `TPM2_Quote` hashes `pcrDigest` with the hash of the signing scheme (Part 3, clause 18.4.1;
/// ms-tpm-20-ref `Quote.c`: `PCRComputeCurrentDigest(in->inScheme.details.any.hashAlg, ...)` after
/// `CryptSelectSignScheme`), not always with SHA-256: this fails if SHA-384 or SHA-512 keys get a
/// 32-octet pcrDigest. The expected digests are Python `hashlib` values of the SHA-256
/// bank values of PCR 0 (32 zero octets) and PCR 4 (the selection of `sel04`) concatenated, and of
/// the empty string (an empty selection still hashes, with the scheme hash); the generator of the pinned attests is `gen_quote_vectors.py` (outside the
/// repository: it rebuilds the TPMS_ATTEST and verifies each signature with the openssl CLI).
#[test]
fn quote_pcr_digest_follows_scheme_hash() {
    // (hash of the scheme, digest of PCR 0 and 4 under it, digest of the empty string under it)
    const PCR4_VALUE: &str = "589f9ffed4c477966bfb8d41f37895b08c69047df8f911d6f3b57fbe08faee8d";
    let cases = [
        (
            alg::SHA256,
            "b5ab2eaee749a8f5fe3e847815d70e8c15332cb6ab8a80491cfe7afc8dd7f8bc",
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
        ),
        (
            alg::SHA384,
            "a352cd0bee5c00d2890597add212d9e8f46657efabc36fe8b61420c15781d02c7e7c93e42ef1eb94085a4fedda02ca2d",
            "38b060a751ac96384cd9327eb1b1e36a21fdb71114be07434c0cc7bf63f6e1da274edebfe76f65fbd51ad2f14898b95b",
        ),
        (
            alg::SHA512,
            "156018e17b540f1379eded5d75b729cfa2c0ace3b789b87df49f35605774e8a2c2e011a4a2bc7f602a7e0287cf2fd29be3b621b7aa0d948a7826104e0c3044eb",
            "cf83e1357eefb8bdf1542850d66d8007d620e4050b5715dc83f4a921d36ce9ce47d0d13c5d85f2b0ff8318d2877eec2f63b931bd47417a81a538327af927da3e",
        ),
    ];
    for (hash, selected, empty) in cases {
        let mut t = started();
        ok(&extend(&mut t, 4, &hex(SHA256_ABC)));
        assert_eq!(hex_of(&read_pcr(&mut t, 4)), PCR4_VALUE);
        // The key's scheme names the hash (ECC P-256 key with ECDSA); inScheme is NULL or the same.
        let key = parse_primary(&create_primary(
            &mut t,
            OWNER,
            b"pw",
            &signing_template(false, hash),
        ));
        let tail = |reply: &[u8]| {
            let (attest, _) = parse_attestation(reply);
            let size = digest_of(hash_of(hash), b"").len();
            // pcrDigest is the last field of the TPMS_ATTEST: TPM2B_DIGEST.
            let (head, digest) = attest.split_at(attest.len() - size);
            assert_eq!(head[head.len() - 2..], (size as u16).to_be_bytes());
            hex_of(digest)
        };
        let selected_digest = tail(&quote(
            &mut t,
            key.handle,
            b"pw",
            b"fresh",
            &u16b(alg::NULL),
            &sel04(),
        ));
        assert_eq!(selected_digest, selected);
        let named = tail(&quote(
            &mut t,
            key.handle,
            b"pw",
            b"fresh",
            &scheme_bytes(scheme_of(false, hash)),
            &sel04(),
        ));
        assert_eq!(named, selected);
        let none = tail(&quote(
            &mut t,
            key.handle,
            b"pw",
            b"fresh",
            &u16b(alg::NULL),
            &NO_PCR,
        ));
        assert_eq!(none, empty);
        // TPM_RH_NULL signs nothing: the hash of inScheme hashes the pcrDigest.
        let unsigned = quote(
            &mut t,
            NULL,
            b"",
            b"fresh",
            &scheme_bytes(scheme_of(false, hash)),
            &sel04(),
        );
        assert_eq!(tail(&unsigned), selected);
    }
}

#[test]
fn certify_get_time_and_nv_certify_use_the_hash_of_the_scheme() {
    let nv_digests = [(alg::SHA384, NV_DATA_SHA384), (alg::SHA512, NV_DATA_SHA512)];
    for rsa_key in [true, false] {
        for (hash, nv_digest) in nv_digests {
            let mut t = started();
            let key = primary_with(&mut t, &signing_template(rsa_key, hash));
            let target = primary_with(&mut t, &ecc_sign());
            let attest_of = |reply: &[u8]| -> (Vec<u8>, Vec<u8>) {
                ok(reply);
                parse_attestation(reply)
            };
            let check_signature = |t: &mut Tpm, attest: &[u8], signature: &[u8]| {
                assert_eq!(signature[2..4], hash.to_be_bytes());
                let digest = digest_of(hash_of(hash), attest);
                ok(&verify(t, key.handle, &digest, signature));
            };
            // TPM2_Certify (type TPM_ST_ATTEST_CERTIFY).
            let (attest, signature) = attest_of(&certify(
                &mut t,
                target.handle,
                key.handle,
                [b"pw", b"pw"],
                b"fresh",
                &u16b(alg::NULL),
            ));
            assert_eq!(attest[4..6], [0x80, 0x17]);
            assert!(attest.ends_with(&cat(&[
                &tpm2b(&target.name),
                &tpm2b(&{
                    // The qualified name of a primary key: nameAlg || H(parentQN || name).
                    let mut qn = u16b(alg::SHA256);
                    qn.extend_from_slice(&digest_of(
                        HashAlg::Sha256,
                        &cat(&[&u32b(OWNER), &target.name]),
                    ));
                    qn
                })
            ])));
            check_signature(&mut t, &attest, &signature);
            // TPM2_GetTime (TPM_ST_ATTEST_TIME).
            let (attest, signature) = attest_of(&run(
                &mut t,
                with_pw(
                    cc::GET_TIME,
                    &[ENDORSEMENT, key.handle],
                    &[b"", b"pw"],
                    &cat(&[&tpm2b(b"fresh"), &scheme_bytes(scheme_of(rsa_key, hash))]),
                ),
            ));
            assert_eq!(attest[4..6], [0x80, 0x19]);
            check_signature(&mut t, &attest, &signature);
            // TPM2_NV_Certify: a range and the digest of the whole index, which is hashed with the
            // hash of the scheme.
            let index = NV0 + 1;
            ok(&define(
                &mut t,
                OWNER,
                index,
                nv_attr::OWNERWRITE | nv_attr::OWNERREAD,
                40,
            ));
            ok(&nv_write(&mut t, OWNER, index, b"", &[0x5c; 40], 0));
            let nv_certify = |t: &mut Tpm, handle: u32, scheme: &[u8], size: u16, offset: u16| {
                // The password session of TPM_RH_NULL as signHandle is empty.
                let key_pw: &[u8] = if handle == NULL { b"" } else { b"pw" };
                let passwords = [key_pw, b"".as_slice()];
                run(
                    t,
                    with_pw(
                        cc::NV_CERTIFY,
                        &[handle, OWNER, index],
                        &passwords,
                        &cat(&[&tpm2b(b"fresh"), scheme, &u16b(size), &u16b(offset)]),
                    ),
                )
            };
            let (attest, signature) =
                attest_of(&nv_certify(&mut t, key.handle, &u16b(alg::NULL), 8, 4));
            assert_eq!(attest[4..6], [0x80, 0x14]);
            assert!(attest.ends_with(&cat(&[&u16b(4), &tpm2b(&[0x5c; 8])])));
            check_signature(&mut t, &attest, &signature);
            let (attest, signature) =
                attest_of(&nv_certify(&mut t, key.handle, &u16b(alg::NULL), 0, 0));
            assert_eq!(attest[4..6], [0x80, 0x1c]);
            assert!(attest.ends_with(&tpm2b(&hex(nv_digest))));
            check_signature(&mut t, &attest, &signature);
            // inScheme may name the hash; another hash is a scheme mismatch.
            let named = nv_certify(
                &mut t,
                key.handle,
                &scheme_bytes(scheme_of(rsa_key, hash)),
                0,
                0,
            );
            assert_eq!(attest_of(&named).0, attest);
            err(
                &nv_certify(
                    &mut t,
                    key.handle,
                    &scheme_bytes(scheme_of(rsa_key, alg::SHA256)),
                    0,
                    0,
                ),
                Rc::SCHEME.in_param(2),
            );
            // Without a signing key the hash of inScheme digests the index (NULL signature).
            let unsigned = nv_certify(&mut t, NULL, &scheme_bytes(scheme_of(rsa_key, hash)), 0, 0);
            let (attest, signature) = attest_of(&unsigned);
            assert!(attest.ends_with(&tpm2b(&hex(nv_digest))));
            assert_eq!(signature, [0, 0x10]);
        }
    }
}
