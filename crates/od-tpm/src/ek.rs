//! TCG EK Credential Profile 2.7, low-range default templates L-1 and L-2.
//!
//! The client sends this public area explicitly to TPM2_CreatePrimary under
//! TPM_RH_ENDORSEMENT with empty inSensitive.data. Creating or restoring a TPM
//! does not provision an endorsement certificate.

use crate::codes::{alg, curve, object_attr};
use crate::marshal::{PublicParams, Scheme, SymDef, TpmtPublic, Unique};

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
