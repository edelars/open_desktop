//! `TPM2_GetCapability` (Part 3 v184, clause 30.2).
//!
//! Supported capabilities: `TPM_CAP_ALGS`, `TPM_CAP_HANDLES`, `TPM_CAP_COMMANDS`,
//! `TPM_CAP_PP_COMMANDS`, `TPM_CAP_AUDIT_COMMANDS` (both empty: no command needs physical
//! presence or is audited), `TPM_CAP_PCRS`, `TPM_CAP_TPM_PROPERTIES`. Every other capability
//! is `TPM_RC_VALUE` (parameter 1); in particular `TPM_CAP_PCR_PROPERTIES` is not implemented
//! (PTP 1.07, clause 4.7.1 item 4 asks for it; recorded as a deviation in `docs/specs/VTPM.md`).
//!
//! `TPM_CAP_COMMANDS` lists exactly the commands of the dispatch table.
//!
//! Properties report only what phase A implements. Properties of features that do not exist yet
//! (sessions, NV, contexts, dictionary attack, audit, persistent objects, hierarchy attributes)
//! are not listed instead of being reported as zero; PTP 1.07, clause 4.2 item 2 asks for all
//! defined properties, so this is a recorded deviation until S7-23.
//! NOT VERIFIED against TPM 2.0 Library v185: the answer for `TPM_CAP_TPM_PROPERTIES` with a
//! `property` outside the fixed and variable groups (an empty list is returned), the
//! `TPM_PT_MANUFACTURER` value `ODTP` (not a TCG-registered vendor ID) and the firmware version.

use crate::codes::{alg, cap, handle, pt, spec};
use crate::dispatch::COMMANDS;
use crate::marshal::{
    Reader, TPMA_ALGORITHM_HASH, TpmaCc, TpmlPcrSelection, TpmsAlgProperty, TpmsPcrSelection,
    TpmsTaggedProperty, Writer, limits,
};
use crate::response::{Output, Rc};
use crate::state::Tpm;

/// Maximum size of the capability data in one response (`TPM_PT_MAX_CAP_BUFFER`).
const MAX_CAP_BUFFER: usize = 1024;

/// Vendor ID reported in `TPM_PT_MANUFACTURER` ("ODTP", not registered with TCG).
const VENDOR_ID: u32 = 0x4F44_5450;

/// Entries (already marshaled) for one capability, in ascending order, from `property` on.
struct Entries {
    items: Vec<Vec<u8>>,
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
            (pt::PCR_COUNT, limits::PCR_COUNT as u32),
            (pt::PCR_SELECT_MIN, limits::PCR_SELECT_SIZE as u32),
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
            (pt::MAX_CAP_BUFFER, MAX_CAP_BUFFER as u32),
            (pt::HR_TRANSIENT_AVAIL, self.sequences.free_slots() as u32),
        ]
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
                if u32::from(alg::SHA256) >= property {
                    entries.push(|w| {
                        TpmsAlgProperty {
                            alg: alg::SHA256,
                            properties: TPMA_ALGORITHM_HASH,
                        }
                        .marshal(w)
                    });
                }
            }
            cap::HANDLES => {
                entry_size = 4;
                let ht = handle::handle_type(property);
                let listed: Vec<u32> = match ht {
                    handle::HT_PCR => (0..limits::PCR_COUNT as u32).collect(),
                    handle::HT_TRANSIENT => self.sequences.loaded_handles(),
                    // Defined handle types without objects in phase A. Permanent handles belong
                    // to the hierarchies of phase B and are not listed yet (NOT VERIFIED).
                    0x01 | 0x02 | 0x03 | 0x11 | 0x12 | 0x40 | handle::HT_PERSISTENT => Vec::new(),
                    _ => return Err(Rc::VALUE.in_param(2)),
                };
                for h in listed
                    .into_iter()
                    .filter(|h| *h >= property && handle::handle_type(*h) == ht)
                {
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
        TpmaCc::new(
            (self.code & 0xFFFF) as u16,
            self.nv,
            self.flushed,
            self.handles,
            self.rhandle,
        )
    }
}
