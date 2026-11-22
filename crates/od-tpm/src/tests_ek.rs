use sha2::{Digest, Sha256};

use super::tests_b::{
    define, nv_read, nv_read_data, nv_write, pw_cmd, set_auth, shutdown, startup,
};
use super::tests_obj::{
    NO_PCR, create_primary_raw, creation_params, flush, parse_primary, read_public,
};
use super::*;
use crate::ek::LowRangeEk;
use crate::marshal::{TpmtPublic, Unique};
use crate::nv::{NV_BUFFER_MAX, attr};

#[test]
fn low_range_ek_templates_match_profile_wire_bytes() {
    let policy = hex("837197674484b3f81a90cc8d46a5d724fd52d76e06520b64f2a1da1b331469aa");
    let cases = [
        (
            LowRangeEk::Rsa2048,
            "0001",
            "0800000000000100",
            256,
            0x01c0_0002,
        ),
        (LowRangeEk::EccP256, "0023", "000300100020", 32, 0x01c0_000a),
    ];
    for (profile, algorithm, params, length, index) in cases {
        let mut expected = hex(algorithm);
        expected.extend(hex("000b000300b20020"));
        expected.extend(&policy);
        expected.extend(hex("0006008000430010"));
        expected.extend(hex(params));
        expected.resize(expected.len() + length, 0);
        if profile == LowRangeEk::EccP256 {
            expected.extend(hex("0020"));
            expected.extend([0; 32]);
        }
        assert_eq!(profile.template().to_bytes(), expected);
        assert_eq!(profile.certificate_index(), index);
    }
}

fn recreate_ek(t: &mut Tpm, password: &[u8], template: &TpmtPublic) -> Vec<u8> {
    let inputs = creation_params(b"", b"", template, b"", &NO_PCR);
    let response = create_primary_raw(t, handle::RH_ENDORSEMENT, password, &inputs);
    ok(&response);
    let primary = parse_primary(&response);

    let response = read_public(t, primary.handle);
    ok(&response);
    let mut reader = Reader::new(params(&response, 0));
    let public = reader.tpm2b(1024).unwrap();
    assert_eq!(public, primary.public);
    let mut expected_name = hex("000b");
    expected_name.extend(Sha256::digest(public));
    assert_eq!(reader.tpm2b(64).unwrap(), expected_name);
    assert_eq!(primary.name, expected_name);
    ok(&flush(t, primary.handle));
    primary.public
}

#[test]
fn ek_uses_eps_and_the_exact_template_across_auth_change_clear_and_restore() {
    let mut t = started();
    let profiles = [LowRangeEk::Rsa2048, LowRangeEk::EccP256];
    let templates = profiles.map(LowRangeEk::template);
    let expected = templates.each_ref().map(|template| {
        let public = recreate_ek(&mut t, b"", template);
        let mut reader = Reader::new(&public);
        let mut generated = TpmtPublic::unmarshal(&mut reader).unwrap();
        reader.finish().unwrap();
        match &generated.unique {
            Unique::Rsa(modulus) => {
                assert_eq!(modulus.len(), 256);
                assert_ne!(modulus, &[0; 256]);
            }
            Unique::Ecc { x, y } => {
                assert_eq!((x.len(), y.len()), (32, 32));
                assert_ne!(x, &[0; 32]);
                assert_ne!(y, &[0; 32]);
            }
            unexpected => panic!("unexpected EK unique field: {unexpected:?}"),
        }
        generated.unique = template.unique.clone();
        assert_eq!(generated, *template);
        public
    });

    ok(&set_auth(&mut t, handle::RH_ENDORSEMENT, b"", b"new"));
    for (template, public) in templates.iter().zip(&expected) {
        assert_eq!(recreate_ek(&mut t, b"new", template), *public);
    }

    let mut changed = templates[1].clone();
    changed.unique = Unique::Ecc {
        x: vec![1; 32],
        y: vec![0; 32],
    };
    assert_ne!(recreate_ek(&mut t, b"new", &changed), expected[1]);

    ok(&pw_cmd(&mut t, cc::CLEAR, &[handle::RH_LOCKOUT], b"", &[]));
    for (template, public) in templates.iter().zip(&expected) {
        assert_eq!(recreate_ek(&mut t, b"", template), *public);
    }

    shutdown(&mut t, 0);
    let mut t = Tpm::restore(&t.persistent_snapshot(), &[6; 48]).unwrap();
    startup(&mut t, 0);
    for (template, public) in templates.iter().zip(&expected) {
        assert_eq!(recreate_ek(&mut t, b"", template), *public);
    }
    let mut other_tpm = Tpm::new(&[7; 48]).unwrap();
    startup(&mut other_tpm, 0);
    assert_ne!(recreate_ek(&mut other_tpm, b"", &templates[1]), expected[1]);
}

/// The snapshot of `version` (3 through 8) that the writer of that version produced for the same
/// state, cut from a current snapshot of a TPM without NV indices, persistent objects or primary
/// policies: each later version only appended fields (the NULL policies of v4, `totalResetCount`
/// of v5, the PCR allocation of v6, the establishment flag of v7, the seed tails of v8).
fn snapshot_as_version(current: &[u8], version: u16) -> Vec<u8> {
    let payload = &current[16..current.len() - 32];
    let appended = match version {
        8 => 0,
        7 => 96,
        6 => 96 + 1,
        5 => 96 + 1 + 1,
        4 => 96 + 1 + 1 + 8,
        3 => 96 + 1 + 1 + 8 + 6,
        _ => unreachable!(),
    };
    let body = &payload[..payload.len() - appended];
    let mut bytes = b"ODTPMSTA".to_vec();
    bytes.extend_from_slice(&version.to_be_bytes());
    bytes.extend_from_slice(&[0; 2]);
    bytes.extend_from_slice(&(body.len() as u32).to_be_bytes());
    bytes.extend_from_slice(body);
    bytes.extend_from_slice(&Sha256::digest(body));
    bytes
}

#[test]
fn old_snapshot_versions_migrate_to_the_current_one_and_keep_the_ek() {
    // ADR-0009, decision 6: every readable version is restored, saved again as the current one and
    // restored once more; the EK of template L-1 is the same before and after.
    let current = tpm_new().persistent_snapshot();
    assert_eq!(current[8..10], crate::SNAPSHOT_VERSION.to_be_bytes());
    let template = LowRangeEk::Rsa2048.template();
    for version in 3..=7 {
        let old = snapshot_as_version(&current, version);
        assert_eq!(old[8..10], version.to_be_bytes());
        let mut first = Tpm::restore(&old, &[6; 48]).unwrap();
        let migrated = first.persistent_snapshot();
        assert_eq!(
            migrated[8..10],
            crate::SNAPSHOT_VERSION.to_be_bytes(),
            "v{version}"
        );
        // Written only as the current version, and stable: a second round changes nothing.
        let mut second = Tpm::restore(&migrated, &[7; 48]).unwrap();
        assert_eq!(second.persistent_snapshot(), migrated, "v{version}");
        startup(&mut first, 0);
        startup(&mut second, 0);
        let before = recreate_ek(&mut first, b"", &template);
        let after = recreate_ek(&mut second, b"", &template);
        assert_eq!(before, after, "v{version}");
    }
}

#[test]
fn ek_certificate_nv_indices_are_opt_in_platform_storage() {
    const PLATFORM: u32 = handle::RH_PLATFORM;
    const OWNER: u32 = handle::RH_OWNER;
    const ATTRIBUTES: u32 = attr::PLATFORMCREATE
        | attr::PPWRITE
        | attr::OWNERREAD
        | attr::AUTHREAD
        | attr::NO_DA
        | attr::WRITEDEFINE;
    let cases = [
        (0x01c0_0002, vec![0x52; 800]),
        (0x01c0_000a, vec![0x45; 650]),
    ];
    let mut t = started();
    for (index, data) in &cases {
        // Opaque bytes exercise chunking without assuming an issuer or an X.509 parser.
        err(
            &run(&mut t, plain(cc::NV_READ_PUBLIC, &[*index], &[])),
            Rc::HANDLE.in_handle(1),
        );
        err(
            &define(&mut t, OWNER, *index, ATTRIBUTES, data.len() as u16),
            Rc::ATTRIBUTES.in_param(2),
        );
        ok(&define(
            &mut t,
            PLATFORM,
            *index,
            ATTRIBUTES,
            data.len() as u16,
        ));
        for (i, chunk) in data.chunks(NV_BUFFER_MAX).enumerate() {
            ok(&nv_write(
                &mut t,
                PLATFORM,
                *index,
                b"",
                chunk,
                (i * NV_BUFFER_MAX) as u16,
            ));
        }
        ok(&pw_cmd(
            &mut t,
            cc::NV_WRITE_LOCK,
            &[PLATFORM, *index],
            b"",
            &[],
        ));
    }

    ok(&pw_cmd(&mut t, cc::CLEAR, &[handle::RH_LOCKOUT], b"", &[]));
    shutdown(&mut t, 0);
    let mut t = Tpm::restore(&t.persistent_snapshot(), &[6; 48]).unwrap();
    startup(&mut t, 0);
    for (index, data) in &cases {
        for auth in [OWNER, *index] {
            for (i, chunk) in data.chunks(NV_BUFFER_MAX).enumerate() {
                let response = nv_read(
                    &mut t,
                    auth,
                    *index,
                    b"",
                    chunk.len() as u16,
                    (i * NV_BUFFER_MAX) as u16,
                );
                ok(&response);
                assert_eq!(nv_read_data(&response), chunk);
            }
        }
        err(
            &nv_write(&mut t, PLATFORM, *index, b"", b"X", 0),
            Rc::NV_LOCKED,
        );
    }
}
