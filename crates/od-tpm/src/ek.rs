//! TCG EK Credential Profile 2.7: low-range default templates L-1 and L-2 and the High Range
//! storage templates H-3 (ECC NIST P-384), H-6 (RSA-3072) and H-27 (ML-KEM-768).
//!
//! The client sends this public area explicitly to TPM2_CreatePrimary under
//! TPM_RH_ENDORSEMENT with empty inSensitive.data. Creating or restoring a TPM
//! does not provision an endorsement certificate, and this module has no issuer: the DER
//! certificate, if any, is written by the caller to `certificate_index()`.

use crate::codes::{alg, curve, object_attr};
use crate::marshal::{MLKEM_768, PublicParams, Scheme, SymDef, TpmtPublic, Unique};

/// SHA-256 digest of PolicySecret(TPM_RH_ENDORSEMENT), with empty policyRef.
pub const POLICY_A: [u8; 32] = [
    0x83, 0x71, 0x97, 0x67, 0x44, 0x84, 0xb3, 0xf8, 0x1a, 0x90, 0xcc, 0x8d, 0x46, 0xa5, 0xd7, 0x24,
    0xfd, 0x52, 0xd7, 0x6e, 0x06, 0x52, 0x0b, 0x64, 0xf2, 0xa1, 0xda, 0x1b, 0x33, 0x14, 0x69, 0xaa,
];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LowRangeEk {
    Rsa2048,
    EccP256,
}

impl LowRangeEk {
    /// NV index assigned to the DER certificate for this profile (§2.2.2.4).
    pub const fn certificate_index(self) -> u32 {
        match self {
            Self::Rsa2048 => 0x01c0_0002,
            Self::EccP256 => 0x01c0_000a,
        }
    }

    /// Full input template, including the zero-padded Low Range unique field.
    pub fn template(self) -> TpmtPublic {
        let (params, unique) = match self {
            Self::Rsa2048 => (
                PublicParams::Rsa {
                    symmetric: SymDef::Aes128Cfb,
                    scheme: Scheme::NULL,
                    key_bits: 2048,
                    exponent: 0,
                },
                Unique::Rsa(vec![0; 256]),
            ),
            Self::EccP256 => (
                PublicParams::Ecc {
                    symmetric: SymDef::Aes128Cfb,
                    scheme: Scheme::NULL,
                    curve: curve::NIST_P256,
                },
                Unique::Ecc {
                    x: vec![0; 32],
                    y: vec![0; 32],
                },
            ),
        };
        TpmtPublic {
            name_alg: alg::SHA256,
            attributes: object_attr::FIXED_TPM
                | object_attr::FIXED_PARENT
                | object_attr::SENSITIVE_DATA_ORIGIN
                | object_attr::ADMIN_WITH_POLICY
                | object_attr::RESTRICTED
                | object_attr::DECRYPT,
            auth_policy: POLICY_A.to_vec(),
            params,
            unique,
        }
    }
}

/// SHA-384 digest PolicyB of the High Range (Table 8, A.2.5): PolicyOR(PolicyA, PolicyC) with PolicyA
/// = PolicySecret(TPM_RH_ENDORSEMENT) and PolicyC = PolicyAuthorizeNV(Policy Index I-2,
/// 0x01C07F02). The engine only stores this digest in the template.
pub const POLICY_B_SHA384: [u8; 48] = [
    0xb2, 0x6e, 0x7d, 0x28, 0xd1, 0x1a, 0x50, 0xbc, 0x53, 0xd8, 0x82, 0xbc, 0xf5, 0xfd, 0x3a, 0x1a,
    0x07, 0x41, 0x48, 0xbb, 0x35, 0xd3, 0xb4, 0xe4, 0xcb, 0x1c, 0x0a, 0xd9, 0xbd, 0xe4, 0x19, 0xca,
    0xcb, 0x47, 0xba, 0x09, 0x69, 0x96, 0x46, 0x15, 0x0f, 0x9f, 0xc0, 0x00, 0xf3, 0xf8, 0x0e, 0x12,
];

/// High Range storage templates (§5.4.6, Tables 9, 11 and 13), all with a restricted decryption
/// key, `userWithAuth` SET, SHA-384 as nameAlg, AES-256-CFB for the children and PolicyB.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HighRangeEk {
    /// H-6.
    Rsa3072,
    /// H-3.
    EccP384,
    /// H-27.
    MlKem768,
}

impl HighRangeEk {
    /// NV index assigned to the DER certificate for this profile (§2.2.2.5, H-x list).
    pub const fn certificate_index(self) -> u32 {
        match self {
            Self::Rsa3072 => 0x01c0_001c,
            Self::EccP384 => 0x01c0_0016,
            Self::MlKem768 => 0x01c0_0062,
        }
    }

    /// Full input template; the `unique` field is empty (an empty buffer, or the empty point).
    pub fn template(self) -> TpmtPublic {
        let (params, unique) = match self {
            Self::Rsa3072 => (
                PublicParams::Rsa {
                    symmetric: SymDef::Aes256Cfb,
                    scheme: Scheme::NULL,
                    key_bits: 3072,
                    exponent: 0,
                },
                Unique::Rsa(Vec::new()),
            ),
            Self::EccP384 => (
                PublicParams::Ecc {
                    symmetric: SymDef::Aes256Cfb,
                    scheme: Scheme::NULL,
                    curve: curve::NIST_P384,
                },
                Unique::Ecc {
                    x: Vec::new(),
                    y: Vec::new(),
                },
            ),
            Self::MlKem768 => (
                PublicParams::MlKem {
                    symmetric: SymDef::Aes256Cfb,
                    parameter_set: MLKEM_768,
                },
                Unique::MlKem(Vec::new()),
            ),
        };
        TpmtPublic {
            name_alg: alg::SHA384,
            attributes: object_attr::FIXED_TPM
                | object_attr::FIXED_PARENT
                | object_attr::SENSITIVE_DATA_ORIGIN
                | object_attr::USER_WITH_AUTH
                | object_attr::ADMIN_WITH_POLICY
                | object_attr::RESTRICTED
                | object_attr::DECRYPT,
            auth_policy: POLICY_B_SHA384.to_vec(),
            params,
            unique,
        }
    }
}
