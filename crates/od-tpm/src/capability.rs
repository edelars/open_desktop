//! `TPM2_GetCapability` (Part 3 v184, clause 30.2).
//!
//! Supported capabilities: `TPM_CAP_ALGS`, `TPM_CAP_HANDLES`, `TPM_CAP_COMMANDS`,
//! `TPM_CAP_PP_COMMANDS`, `TPM_CAP_AUDIT_COMMANDS` (both empty: no command needs physical
//! presence or is audited), `TPM_CAP_PCRS`, `TPM_CAP_PCR_PROPERTIES` and
//! `TPM_CAP_TPM_PROPERTIES`. An unknown capability returns `TPM_RC_VALUE` (parameter 1).
//!
//! `TPM_CAP_COMMANDS` lists exactly the commands of the dispatch table.
//!
//! Properties report only what is implemented. Properties of features that do not exist yet
//! (contexts, dictionary attack, audit, NV counters) are not listed instead of
//! being reported as zero; PTP 1.07, clause 4.2 item 2 asks for all defined properties, so this
//! is a recorded deviation until S7-23 is complete. Slice A of phase B adds the session and NV
//! limits and counts, `TPM_PT_PERMANENT` and `TPM_PT_STARTUP_CLEAR`.
//!
//! `TPM_CAP_ALGS` lists SHA-256 and, with slice B of phase B, RSA, KEYEDHASH, RSASSA, ECDSA and
//! ECC, with slice C1 also RSAES, OAEP and ECDH, with slice C2 HMAC, with slice C3 XOR (the
//! `TPMA_ALGORITHM` bits follow the type column of Part 2, Table 11, NOT VERIFIED); `TPM_CAP_ECC_CURVES` lists NIST P-256. Slice C1 adds the
//! dictionary-attack properties `TPM_PT_LOCKOUT_COUNTER`, `TPM_PT_MAX_AUTH_FAIL`,
//! `TPM_PT_LOCKOUT_INTERVAL` and `TPM_PT_LOCKOUT_RECOVERY`.
//!
//! `TPM_CAP_HANDLES`: PCR, defined NV indices, loaded sessions (`TPM_HT_LOADED_SESSION`: HMAC and
//! policy, slice C2) and saved sessions (`TPM_HT_SAVED_SESSION`),
//! loaded sequence and key objects (transient), persistent objects, the permanent handles of the hierarchies and the null
//! hierarchy (`TPM_RH_OWNER`, `TPM_RH_NULL`, `TPM_RH_LOCKOUT`, `TPM_RH_ENDORSEMENT`,
//! `TPM_RH_PLATFORM`); the other handle types list nothing.
//! NOT VERIFIED against TPM 2.0 Library v185: the answer for `TPM_CAP_TPM_PROPERTIES` with a
//! `property` outside the fixed and variable groups (an empty list is returned), the
//! `TPM_PT_MANUFACTURER` value `ODTP` (not a TCG-registered vendor ID) and the firmware version;
//! the content of the permanent-handle list of `TPM_CAP_HANDLES` (which of the `TPM_RH_*` values a
//! TPM lists is not stated in the open text).
//! Slice B of phase B: NOT VERIFIED that a TPM lists exactly these algorithms (the `TPMA_ALGORITHM`
//! bits follow the type column of Part 2, Table 11); `TPM_PT_HR_PERSISTENT_MIN` = the persistent
//! capacity of this implementation (16), the PTP 1.07 minimum was not checked; the transient
//! availability counts the free sequence and object slots (an estimate, as the text allows).

use crate::codes::{alg, cap, curve, handle, pt, spec};
use crate::crypto::{DIGEST, supported_rsa_exponent};
use crate::dispatch::COMMANDS;
use crate::marshal::{
    PublicParams, Reader, TPMA_ALGORITHM_ASYMMETRIC, TPMA_ALGORITHM_ENCRYPTING,
    TPMA_ALGORITHM_HASH, TPMA_ALGORITHM_METHOD, TPMA_ALGORITHM_OBJECT, TPMA_ALGORITHM_SIGNING,
    TPMA_ALGORITHM_SYMMETRIC, TpmaCc, TpmlPcrSelection, TpmsAlgProperty, TpmsPcrSelection,
    TpmsTaggedProperty, Writer, limits,
};
use crate::nv::{MAX_NV_INDEX_SIZE, NV_BUFFER_MAX};
use crate::object::MAX_PERSISTENT;
use crate::pcr::{PcrAttr, pcr_attr};
use crate::response::{Output, Rc};
use crate::session::MAX_SESSIONS;
use crate::state::Tpm;

/// Maximum size of the capability data in one response (`TPM_PT_MAX_CAP_BUFFER`).
const MAX_CAP_BUFFER: usize = 1024;

/// Vendor ID reported in `TPM_PT_MANUFACTURER` ("ODTP", not registered with TCG).
const VENDOR_ID: u32 = 0x4F44_5450;

/// Entries (already marshaled) for one capability, in ascending order, from `property` on.
struct Entries {
    items: Vec<Vec<u8>>,
}

// TPM Library v185 Part 2 §6.14 and Table 29. Locality 0 is the only
// supported locality. This profile exposes no D-RTM, PCR policy or PCR auth.
fn tagged_pcr_properties() -> [(u32, fn(PcrAttr) -> bool); 4] {
    [
        (0x00, |attr| attr.save),
        (0x02, |attr| attr.reset_l0),
        (0x11, |attr| attr.no_increment),
        (0x12, |_| false),
    ]
}

impl Entries {
    fn new() -> Self {
        Entries { items: Vec::new() }
    }

    fn push(&mut self, f: impl FnOnce(&mut Writer)) {
        let mut w = Writer::new();
        f(&mut w);
        self.items.push(w.finish());
    }
}

impl Tpm {
    fn active_sessions(&self) -> usize {
        self.sessions.loaded() + self.context.saved_count()
    }

    fn tagged_properties(&self) -> Vec<(u32, u32)> {
        let total = COMMANDS.len() as u32;
        vec![
            (pt::FAMILY_INDICATOR, spec::FAMILY),
            (pt::LEVEL, spec::LEVEL),
            (pt::REVISION, spec::REVISION),
            (pt::DAY_OF_YEAR, spec::DAY_OF_YEAR),
            (pt::YEAR, spec::YEAR),
            (pt::MANUFACTURER, VENDOR_ID),
            (pt::VENDOR_STRING_1, 0x4F70_656E),
            (pt::VENDOR_STRING_2, 0x4465_736B),
            (pt::VENDOR_STRING_3, 0x5450_4D00),
            (pt::VENDOR_STRING_4, 0),
            (pt::VENDOR_TPM_TYPE, 0),
            (pt::FIRMWARE_VERSION_1, 0x0001_0000),
            (pt::FIRMWARE_VERSION_2, 0),
            (pt::INPUT_BUFFER, limits::MAX_2B_BUFFER_SIZE as u32),
            (pt::HR_TRANSIENT_MIN, crate::hash::MAX_SEQUENCES as u32),
            (pt::HR_PERSISTENT_MIN, MAX_PERSISTENT as u32),
            (pt::HR_LOADED_MIN, MAX_SESSIONS as u32),
            (pt::ACTIVE_SESSIONS_MAX, MAX_SESSIONS as u32),
            (pt::PCR_COUNT, limits::PCR_COUNT as u32),
            (pt::PCR_SELECT_MIN, limits::PCR_SELECT_SIZE as u32),
            (pt::NV_INDEX_MAX, MAX_NV_INDEX_SIZE as u32),
            (pt::MAX_COMMAND_SIZE, limits::MAX_COMMAND_SIZE as u32),
            (pt::MAX_RESPONSE_SIZE, limits::MAX_RESPONSE_SIZE as u32),
            (pt::MAX_DIGEST, limits::MAX_DIGEST_SIZE as u32),
            // Platform-specific family: PC Client, level 0, revision 1.07 (PTP 1.07, Table 2).
            (pt::PS_FAMILY_INDICATOR, 1),
            (pt::PS_LEVEL, 0),
            (pt::PS_REVISION, 0x0000_0107),
            (pt::PS_DAY_OF_YEAR, 0),
            (pt::PS_YEAR, 0),
            (pt::TOTAL_COMMANDS, total),
            (pt::LIBRARY_COMMANDS, total),
            (pt::VENDOR_COMMANDS, 0),
            (pt::NV_BUFFER_MAX, NV_BUFFER_MAX as u32),
            (pt::MAX_CAP_BUFFER, MAX_CAP_BUFFER as u32),
            (pt::PERMANENT, self.permanent_attributes()),
            (pt::STARTUP_CLEAR, self.startup_clear_attributes()),
            (pt::HR_NV_INDEX, self.persistent.nv.count() as u32),
            (pt::HR_LOADED, self.sessions.loaded() as u32),
            (
                pt::HR_LOADED_AVAIL,
                (MAX_SESSIONS - self.active_sessions()) as u32,
            ),
            // Active sessions: loaded and saved ones (a saved session keeps its slot).
            (pt::HR_ACTIVE, self.active_sessions() as u32),
            (
                pt::HR_ACTIVE_AVAIL,
                (MAX_SESSIONS - self.active_sessions()) as u32,
            ),
            (
                pt::HR_TRANSIENT_AVAIL,
                (self.sequences.free_slots() + self.objects.free_slots()) as u32,
            ),
            (pt::HR_PERSISTENT, self.persistent.objects.len() as u32),
            (
                pt::HR_PERSISTENT_AVAIL,
                (MAX_PERSISTENT - self.persistent.objects.len()) as u32,
            ),
            // Dictionary attack (slice C1, `da.rs`).
            (pt::LOCKOUT_COUNTER, self.persistent.da.failed_tries),
            (pt::MAX_AUTH_FAIL, self.persistent.da.max_tries),
            (pt::LOCKOUT_INTERVAL, self.persistent.da.recovery_time),
            (pt::LOCKOUT_RECOVERY, self.persistent.da.lockout_recovery),
        ]
    }

    /// `TPM2_TestParms` (Part 3 v185 §30.3). There is no public object template or handle.
    pub(crate) fn cmd_test_parms(p: &mut Reader<'_>) -> Result<Output, Rc> {
        p.set_param(1);
        let type_alg = PublicParams::unmarshal_type(p)?;
        let params = PublicParams::unmarshal(p, type_alg)?;
        p.finish()?;
        if let PublicParams::Rsa { exponent, .. } = params
            && !supported_rsa_exponent(exponent)
        {
            return Err(p.err(Rc::RANGE));
        }
        Ok(Output::default())
    }

    /// `TPM2_GetCapability`.
    pub(crate) fn cmd_get_capability(&mut self, p: &mut Reader<'_>) -> Result<Output, Rc> {
        p.set_param(1);
        let capability = p.u32()?;
        p.set_param(2);
        let property = p.u32()?;
        p.set_param(3);
        let count = p.u32()? as usize;
        p.finish()?;

        let entry_size: usize;
        let mut entries = Entries::new();
        match capability {
            cap::ALGS => {
                entry_size = 6;
                // Ascending by algorithm ID (Part 2, Table 11 types: RSA and ECC are "A O",
                // KEYEDHASH "H O", RSASSA and ECDSA "A X"). AES (0x0006, symmetric, object) and CFB
                // (0x0043, encrypting mode) protect `outPrivate` and are the storage key template's
                // algorithms. NOT VERIFIED against Part 2 v184 (Tables 8 and 11 were not available):
                // the codes follow Table 8 as used elsewhere, the attribute bits of AES and CFB
                // are our reading of the "S O" and "E" types.
                let algs: [(u16, u32); 13] = [
                    (alg::RSA, TPMA_ALGORITHM_ASYMMETRIC | TPMA_ALGORITHM_OBJECT),
                    // `TPM_ALG_HMAC` (0x0005, "H K X" in Table 11): a hash-based signing scheme.
                    (alg::HMAC, TPMA_ALGORITHM_HASH | TPMA_ALGORITHM_SIGNING),
                    (alg::AES, TPMA_ALGORITHM_SYMMETRIC | TPMA_ALGORITHM_OBJECT),
                    (alg::KEYEDHASH, TPMA_ALGORITHM_HASH | TPMA_ALGORITHM_OBJECT),
                    // `TPM_ALG_XOR` (0x000A, "H S" in Table 11): XOR parameter encryption of a session
                    // (slice C3); NOT VERIFIED: the bits are our reading of the type column.
                    (alg::XOR, TPMA_ALGORITHM_HASH | TPMA_ALGORITHM_SYMMETRIC),
                    (alg::SHA256, TPMA_ALGORITHM_HASH),
                    (
                        alg::RSASSA,
                        TPMA_ALGORITHM_ASYMMETRIC | TPMA_ALGORITHM_SIGNING,
                    ),
                    (
                        alg::RSAES,
                        TPMA_ALGORITHM_ASYMMETRIC | TPMA_ALGORITHM_ENCRYPTING,
                    ),
                    (
                        alg::OAEP,
                        TPMA_ALGORITHM_ASYMMETRIC | TPMA_ALGORITHM_ENCRYPTING | TPMA_ALGORITHM_HASH,
                    ),
                    (
                        alg::ECDSA,
                        TPMA_ALGORITHM_ASYMMETRIC | TPMA_ALGORITHM_SIGNING,
                    ),
                    (alg::ECDH, TPMA_ALGORITHM_ASYMMETRIC | TPMA_ALGORITHM_METHOD),
                    (alg::ECC, TPMA_ALGORITHM_ASYMMETRIC | TPMA_ALGORITHM_OBJECT),
                    (alg::CFB, TPMA_ALGORITHM_ENCRYPTING),
                ];
                for (a, properties) in algs.into_iter().filter(|(a, _)| u32::from(*a) >= property) {
                    entries.push(|w| TpmsAlgProperty { alg: a, properties }.marshal(w));
                }
            }
            cap::ECC_CURVES => {
                entry_size = 2;
                // `TPML_ECC_CURVE`: NIST P-256 only.
                if u32::from(curve::NIST_P256) >= property {
                    entries.push(|w| w.u16(curve::NIST_P256));
                }
            }
            cap::HANDLES => {
                entry_size = 4;
                let ht = handle::handle_type(property);
                let session_type =
                    matches!(ht, handle::HT_HMAC_SESSION | handle::HT_POLICY_SESSION);
                let listed: Vec<u32> = match ht {
                    handle::HT_PCR => (0..limits::PCR_COUNT as u32).collect(),
                    handle::HT_TRANSIENT => {
                        let mut v = self.sequences.loaded_handles();
                        v.extend(self.objects.loaded_handles());
                        v
                    }
                    handle::HT_PERSISTENT => self.persistent.objects.handles(),
                    handle::HT_NV_INDEX => self.persistent.nv.handles(),
                    // `TPM_HT_LOADED_SESSION` (same value as the HMAC session type): the loaded
                    // HMAC and policy sessions, with their own handle types (Part 3, clause 30.2).
                    handle::HT_HMAC_SESSION => self.sessions.loaded_handles(),
                    // `TPM_HT_SAVED_SESSION` (same value as the policy session type): the saved
                    // sessions, all with the HMAC handle type (the TPM does not track their type).
                    handle::HT_POLICY_SESSION => self.context.saved_handles(),
                    handle::HT_PERMANENT => vec![
                        handle::RH_OWNER,
                        handle::RH_NULL,
                        handle::RH_LOCKOUT,
                        handle::RH_ENDORSEMENT,
                        handle::RH_PLATFORM,
                    ],
                    // Defined handle types without entries: external and permanent NV (0x11,
                    // 0x12).
                    0x11 | 0x12 => Vec::new(),
                    _ => return Err(Rc::VALUE.in_param(2)),
                };
                // The session types list handles of both session families, so they compare the
                // slot number; the other types the whole handle.
                let at_or_after = |h: u32| {
                    if session_type {
                        (h & 0x00FF_FFFF) >= (property & 0x00FF_FFFF)
                    } else {
                        h >= property && handle::handle_type(h) == ht
                    }
                };
                for h in listed.into_iter().filter(|h| at_or_after(*h)) {
                    entries.push(|w| w.u32(h));
                }
            }
            cap::COMMANDS => {
                entry_size = 4;
                for c in COMMANDS.iter().filter(|c| c.code >= property) {
                    entries.push(|w| w.u32(c.attributes().0));
                }
            }
            cap::PP_COMMANDS | cap::AUDIT_COMMANDS => {
                entry_size = 4;
            }
            cap::PCRS => {
                if property != 0 {
                    return Err(Rc::VALUE.in_param(2));
                }
                // Always the full allocation, moreData NO (Part 3, clause 30.2).
                let mut w = Writer::new();
                w.u8(0);
                w.u32(capability);
                TpmlPcrSelection {
                    selections: vec![TpmsPcrSelection {
                        hash: alg::SHA256,
                        select: [0xFF; limits::PCR_SELECT_SIZE],
                    }],
                }
                .marshal(&mut w);
                return Ok(Output {
                    handles: Vec::new(),
                    params: w.finish(),
                });
            }
            cap::PCR_PROPERTIES => {
                // TPMS_TAGGED_PCR_SELECT, Part 2 v185 §10.6: tag, size and bitmap.
                entry_size = 4 + 1 + limits::PCR_SELECT_SIZE;
                for (tag, applies) in tagged_pcr_properties() {
                    if tag < property {
                        continue;
                    }
                    let mut mask = [0; limits::PCR_SELECT_SIZE];
                    for index in 0..limits::PCR_COUNT {
                        if applies(pcr_attr(index)) {
                            mask[index / 8] |= 1 << (index % 8);
                        }
                    }
                    entries.push(|w| {
                        w.u32(tag);
                        w.u8(limits::PCR_SELECT_SIZE as u8);
                        w.bytes(&mask);
                    });
                }
            }
            cap::TPM_PROPERTIES => {
                entry_size = 8;
                let group = property / pt::GROUP;
                for (tag, value) in self.tagged_properties() {
                    if tag / pt::GROUP == group && tag >= property {
                        entries.push(|w| {
                            TpmsTaggedProperty {
                                property: tag,
                                value,
                            }
                            .marshal(w)
                        });
                    }
                }
            }
            cap::AUTH_POLICIES => {
                if handle::handle_type(property) != handle::HT_PERMANENT {
                    return Err(Rc::VALUE.in_param(2));
                }
                entry_size = 4 + 2 + DIGEST;
                for hierarchy in [
                    handle::RH_OWNER,
                    handle::RH_LOCKOUT,
                    handle::RH_ENDORSEMENT,
                    handle::RH_PLATFORM,
                ]
                .into_iter()
                .filter(|&h| h >= property)
                {
                    entries.push(|w| {
                        w.u32(hierarchy);
                        if let Some(digest) = self.permanent_auth_policy(hierarchy) {
                            w.u16(alg::SHA256);
                            w.bytes(digest);
                        } else {
                            w.u16(alg::NULL);
                        }
                    });
                }
            }
            _ => return Err(Rc::VALUE.in_param(1)),
        }

        let fit = MAX_CAP_BUFFER / entry_size;
        let taken = entries.items.len().min(count).min(fit);
        let more = entries.items.len() > taken;
        let mut w = Writer::new();
        w.u8(u8::from(more));
        w.u32(capability);
        w.u32(taken as u32);
        for item in &entries.items[..taken] {
            w.bytes(item);
        }
        Ok(Output {
            handles: Vec::new(),
            params: w.finish(),
        })
    }
}

/// Attributes of a command for `TPM_CAP_COMMANDS`.
impl crate::dispatch::CommandSpec {
    pub(crate) fn attributes(&self) -> TpmaCc {
        let a = TpmaCc::new(
            (self.code & 0xFFFF) as u16,
            self.nv,
            self.flushed,
            self.handles,
            self.rhandle,
        );
        if self.extensive {
            a.with_extensive()
        } else {
            a
        }
    }
}
