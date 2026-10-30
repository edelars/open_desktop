//! Strict long-mode integer fixtures/results. V1 readers and digests are unchanged.

use crate::case::{
    MAX_INSTRUCTION_BUDGET, MAX_WALL_TIMEOUT_MS, MemoryComparisonScope, MemoryPermissions,
    MemoryRegionSpec, Provenance, validate_case_id, validate_hex_bytes,
};
use crate::result::{
    Backend, BuildMetadata, ExceptionSnapshot, MachineMetadata, MemoryRegionSnapshot, Observation,
};
use crate::{FormatError, HexU64, MAX_RESULT_JSON_BYTES, sha256_hex};
use od_isa::LongModeState;
use od_isa::OperandSize;
use od_isa::binary_float::{Binary32, BinaryClass};
use od_isa::long_decode::Operation;
use od_isa::memory::{AccessContext, AccessKind, GuestMemory, MemoryError};
use serde::{Deserialize, Serialize, de::DeserializeOwned};

pub const SCHEMA_VERSION: u32 = 2;
/// Accommodates 64 KiB of bytes plus per-byte comparison masks and metadata.
pub const MAX_FIXTURE_JSON_BYTES: usize = 512 * 1024;
pub const PROFILE: &str = "s2-long-integer-v1";
pub const PROFILE_CVTSI2SS_REGISTERS: &str = "s2-long-cvtsi2ss-registers-v1";
pub const PROFILE_FS_BASE: &str = "s2-long-fs-base-v1";
pub const PROFILE_GS_BASE: &str = "s2-long-gs-base-v1";
pub const PROFILE_FS_BASE_MSR: &str = "s2-long-fs-base-msr-v1";
pub const PROFILE_SEP_DISABLED: &str = "s2-long-sep-disabled-v1";
pub const PROFILE_FS_SELECTOR: &str = "s2-long-fs-selector-v1";
pub const PROFILE_GS_SELECTOR: &str = "s2-long-gs-selector-v1";
pub const PROFILE_VERW_GDT: &str = "s2-long-verw-gdt-v1";
pub const PROFILE_FS_LOAD: &str = "s2-long-fs-load-v1";
pub const PROFILE_GS_LOAD: &str = "s2-long-gs-load-v1";
pub const PROFILE_NULL_DS_ES: &str = "s2-long-null-ds-es-v1";
pub const PROFILE_X87_STATE: &str = "s2-long-x87-state-v1";
pub const PROFILE_X87_SSE_STATE: &str = "s2-long-x87-sse-state-v1";
pub const PROFILE_X87_SELECTORS_STATE: &str = "s2-long-x87-selectors-state-v1";
pub const PROFILE_FXSAVE_RIP: &str = "s2-long-fxsave-rip-v1";
pub const PROFILE_FXSAVE64_RAX: &str = "s2-long-fxsave64-rax-disp8-v1";
pub const PROFILE_FXRSTOR64_RBX: &str = "s2-long-fxrstor64-rbx-disp8-v1";
pub const PROFILE_FXRSTOR64_CR0_GATES: &str = "s2-long-fxrstor64-cr0-gates-v1";
pub const PROFILE_CR0_CR4_FP_INIT_FXRSTOR64: &str = "s2-long-cr0-cr4-fp-init-fxrstor64-v1";
pub const PROFILE_CR0_CR4_FP_INIT_FXRSTOR64_MINSS: &str =
    "s2-long-cr0-cr4-fp-init-fxrstor64-minss-v1";
pub const PROFILE_MOVUPS_REGISTER: &str = "s2-long-movups-reg-v1";
pub const PROFILE_MOVAPS_REGISTER: &str = "s2-long-movaps-reg-v1";
pub const PROFILE_MOVAPS_REVERSE_REGISTER: &str = "s2-long-movaps-reg-reverse-v1";
pub const PROFILE_MOVAPD_REGISTER: &str = "s2-long-movapd-reg-v1";
pub const PROFILE_MOVAPD_REVERSE_REGISTER: &str = "s2-long-movapd-reg-reverse-v1";
pub const PROFILE_MOVUPD_REGISTER: &str = "s2-long-movupd-reg-v1";
pub const PROFILE_MOVUPD_REVERSE_REGISTER: &str = "s2-long-movupd-reg-reverse-v1";
pub const PROFILE_XORPS_REGISTER: &str = "s2-long-xorps-reg-v1";
pub const PROFILE_XORPD_REGISTER: &str = "s2-long-xorpd-reg-v1";
pub const PROFILE_XORPD_PREREQUISITES: &str = "s2-long-xorpd-prerequisites-v1";
pub const PROFILE_NATIVE_XORPD_XMM0_XMM1: &str = "s2-long-native-xorpd-xmm0-xmm1-v1";
pub const PROFILE_ANDPD_REGISTER: &str = "s2-long-andpd-reg-v1";
pub const PROFILE_PXOR_REGISTER: &str = "s2-long-pxor-reg-v1";
pub const PROFILE_PAND_REGISTER: &str = "s2-long-pand-reg-v1";
pub const PROFILE_PANDN_REGISTER: &str = "s2-long-pandn-reg-v1";
pub const PROFILE_POR_REGISTER: &str = "s2-long-por-reg-v1";
pub const PROFILE_PAVGB_REGISTER: &str = "s2-long-pavgb-reg-v1";
pub const PROFILE_PAVGW_REGISTER: &str = "s2-long-pavgw-reg-v1";
pub const PROFILE_PMAXUB_REGISTER: &str = "s2-long-pmaxub-reg-v1";
pub const PROFILE_PMINUB_REGISTER: &str = "s2-long-pminub-reg-v1";
pub const PROFILE_PMAXSW_REGISTER: &str = "s2-long-pmaxsw-reg-v1";
pub const PROFILE_PMINSW_REGISTER: &str = "s2-long-pminsw-reg-v1";
pub const PROFILE_PMULLW_REGISTER: &str = "s2-long-pmullw-reg-v1";
pub const PROFILE_PMULHUW_REGISTER: &str = "s2-long-pmulhuw-reg-v1";
pub const PROFILE_PSADBW_REGISTER: &str = "s2-long-psadbw-reg-v1";
pub const PROFILE_PMOVMSKB_XMM_GPR: &str = "s2-long-pmovmskb-xmm-gpr-v1";
pub const PROFILE_PMULUDQ_REGISTER: &str = "s2-long-pmuludq-reg-v1";
pub const PROFILE_PMADDWD_REGISTER: &str = "s2-long-pmaddwd-reg-v1";
pub const PROFILE_PADDB_REGISTER: &str = "s2-long-paddb-reg-v1";
pub const PROFILE_PADDUSW_REGISTER: &str = "s2-long-paddusw-reg-v1";
pub const PROFILE_PSUBD_REGISTER: &str = "s2-long-psubd-reg-v1";
pub const PROFILE_PSUBQ_REGISTER: &str = "s2-long-psubq-reg-v1";
pub const PROFILE_PCMPEQB_REGISTER: &str = "s2-long-pcmpeqb-reg-v1";
pub const PROFILE_PCMPEQW_REGISTER: &str = "s2-long-pcmpeqw-reg-v1";
pub const PROFILE_PCMPEQD_REGISTER: &str = "s2-long-pcmpeqd-reg-v1";
pub const PROFILE_PCMPGTB_REGISTER: &str = "s2-long-pcmpgtb-reg-v1";
pub const PROFILE_PCMPGTW_REGISTER: &str = "s2-long-pcmpgtw-reg-v1";
pub const PROFILE_PCMPGTD_REGISTER: &str = "s2-long-pcmpgtd-reg-v1";
pub const PROFILE_PUNPCKLBW_REGISTER: &str = "s2-long-punpcklbw-reg-v1";
pub const PROFILE_PUNPCKHBW_REGISTER: &str = "s2-long-punpckhbw-reg-v1";
pub const PROFILE_PUNPCKLWD_REGISTER: &str = "s2-long-punpcklwd-reg-v1";
pub const PROFILE_PUNPCKHWD_REGISTER: &str = "s2-long-punpckhwd-reg-v1";
pub const PROFILE_PUNPCKLDQ_REGISTER: &str = "s2-long-punpckldq-reg-v1";
pub const PROFILE_PUNPCKHDQ_REGISTER: &str = "s2-long-punpckhdq-reg-v1";
pub const PROFILE_PUNPCKLQDQ_REGISTER: &str = "s2-long-punpcklqdq-reg-v1";
pub const PROFILE_PSUBUSW_REGISTER: &str = "s2-long-psubusw-reg-v1";
pub const PROFILE_PSUBW_REGISTER: &str = "s2-long-psubw-reg-v1";
pub const PROFILE_PSUBB_REGISTER: &str = "s2-long-psubb-reg-v1";
pub const PROFILE_PADDUSB_REGISTER: &str = "s2-long-paddusb-reg-v1";
pub const PROFILE_PADDSB_REGISTER: &str = "s2-long-paddsb-reg-v1";
pub const PROFILE_PSUBSB_REGISTER: &str = "s2-long-psubsb-reg-v1";
pub const PROFILE_PADDSW_REGISTER: &str = "s2-long-paddsw-reg-v1";
pub const PROFILE_PSUBSW_REGISTER: &str = "s2-long-psubsw-reg-v1";
pub const PROFILE_PSUBUSB_REGISTER: &str = "s2-long-psubusb-reg-v1";
pub const PROFILE_ANDPS_REGISTER: &str = "s2-long-andps-reg-v1";
pub const PROFILE_ORPS_REGISTER: &str = "s2-long-orps-reg-v1";
pub const PROFILE_ANDNPS_REGISTER: &str = "s2-long-andnps-reg-v1";
pub const PROFILE_MOVSS_REGISTER: &str = "s2-long-movss-reg-v1";
pub const PROFILE_RSQRTSS_REGISTER: &str = "s2-long-rsqrtss-reg-v1";
pub const PROFILE_RCPSS_REGISTER: &str = "s2-long-rcpss-reg-v1";
pub const PROFILE_RSQRTPS_REGISTER: &str = "s2-long-rsqrtps-reg-v1";
pub const PROFILE_RCPPS_REGISTER: &str = "s2-long-rcpps-reg-v1";
pub const PROFILE_MINSS_MASKED_REGISTER: &str = "s2-long-minss-reg-masked-v1";
pub const PROFILE_MINSS_INVALID_CONTROLS: &str = "s2-long-minss-invalid-controls-v1";
pub const PROFILE_MINSS_UNMASKED_DENORMAL: &str = "s2-long-minss-unmasked-denormal-v1";
pub const PROFILE_MAXSS_UNMASKED_DENORMAL: &str = "s2-long-maxss-unmasked-denormal-v1";
pub const PROFILE_MINSD_UNMASKED_DENORMAL: &str = "s2-long-minsd-unmasked-denormal-v1";
pub const PROFILE_MAXSD_UNMASKED_DENORMAL: &str = "s2-long-maxsd-unmasked-denormal-v1";
pub const PROFILE_MINPD_UNMASKED_DENORMAL: &str = "s2-long-minpd-unmasked-denormal-v1";
pub const PROFILE_MAXPD_UNMASKED_DENORMAL: &str = "s2-long-maxpd-unmasked-denormal-v1";
pub const PROFILE_MINSD_MASKED_REGISTER: &str = "s2-long-minsd-reg-masked-v1";
pub const PROFILE_MAXSD_MASKED_REGISTER: &str = "s2-long-maxsd-reg-masked-v1";
pub const PROFILE_MINPD_MASKED_REGISTER: &str = "s2-long-minpd-reg-masked-v1";
pub const PROFILE_MAXPD_MASKED_REGISTER: &str = "s2-long-maxpd-reg-masked-v1";
pub const PROFILE_MAXSS_MASKED_REGISTER: &str = "s2-long-maxss-reg-masked-v1";
pub const PROFILE_MOVSD_REGISTER: &str = "s2-long-movsd-reg-v1";
pub const PROFILE_MOVSD_STORE_REGISTER: &str = "s2-long-movsd-store-reg-v1";
pub const PROFILE_MOVSD_MEMORY_LOAD: &str = "s2-long-movsd-mem-load-v1";
pub const PROFILE_MOVSS_STORE_REGISTER: &str = "s2-long-movss-store-reg-v1";
pub const PROFILE_MOVSS_MEMORY_STORE: &str = "s2-long-movss-mem-store-v1";
pub const PROFILE_MOVUPS_MEMORY_LOAD: &str = "s2-long-movups-mem-load-v1";
pub const PROFILE_MOVUPD_MEMORY_LOAD: &str = "s2-long-movupd-mem-load-v1";
pub const PROFILE_MOVUPD_MEMORY_STORE: &str = "s2-long-movupd-mem-store-v1";
pub const PROFILE_MOVUPD_MEMORY_SIB: &str = "s2-long-movupd-mem-sib-v1";
pub const PROFILE_MOVUPS_MEMORY_STORE: &str = "s2-long-movups-mem-store-v1";
pub const PROFILE_MOVUPS_MEMORY_BASE: &str = "s2-long-movups-mem-base-v1";
pub const PROFILE_SWAPGS: &str = "s2-long-swapgs-v1";
pub const PROFILE_CLI: &str = "s2-long-cli-v1";
pub const PROFILE_STI: &str = "s2-long-sti-v1";
pub const PROFILE_HLT: &str = "s2-long-hlt-v1";
pub const PROFILE_IRETQ_CPL0_VALID: &str = "s2-long-iretq-cpl0-valid-v1";
pub const PROFILE_LRETQ_CPL0_VALID: &str = "s2-long-lretq-cpl0-valid-v1";
pub const PROFILE_KERNEL_GS_BASE: &str = "s2-long-msr-kernel-gs-base-v1";
pub const PROFILE_EFER_RDMSR: &str = "s2-long-efer-rdmsr-v1";
/// Diagnostic mask for this bounded local fixture profile only. It is not a
/// universal hardware CPUID mask and does not model guest #GP behavior.
pub const PROFILE_SSE_MXCSR_DIAGNOSTIC_MASK: u32 = 0x0000_ffbf;
/// Integer arithmetic flags, reserved bit 1 and DF. System flags are not yet modeled.
pub const INTEGER_FLAGS: u64 = 0x8d7 | (1 << 10);

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum Profile {
    #[serde(rename = "s2-long-integer-v1")]
    LongIntegerV1,
    #[serde(rename = "s2-long-sep-disabled-v1")]
    LongSepDisabledV1,
    #[serde(rename = "s2-long-rdtsc-v1")]
    LongRdtscV1,
    #[serde(rename = "s2-long-fs-base-v1")]
    LongFsBaseV1,
    #[serde(rename = "s2-long-gs-base-v1")]
    LongGsBaseV1,
    #[serde(rename = "s2-long-swapgs-v1")]
    LongSwapgsV1,
    #[serde(rename = "s2-long-cli-v1")]
    LongCliV1,
    #[serde(rename = "s2-long-sti-v1")]
    LongStiV1,
    #[serde(rename = "s2-long-hlt-v1")]
    LongHltV1,
    #[serde(rename = "s2-long-iretq-cpl0-valid-v1")]
    LongIretqCpl0ValidV1,
    #[serde(rename = "s2-long-lretq-cpl0-valid-v1")]
    LongLretqCpl0ValidV1,
    #[serde(rename = "s2-long-msr-kernel-gs-base-v1")]
    LongKernelGsBaseV1,
    #[serde(rename = "s2-long-efer-rdmsr-v1")]
    LongEferRdmsrV1,
    #[serde(rename = "s2-long-fs-base-msr-v1")]
    LongFsBaseMsrV1,
    #[serde(rename = "s2-long-fs-selector-v1")]
    LongFsSelectorV1,
    #[serde(rename = "s2-long-gs-selector-v1")]
    LongGsSelectorV1,
    #[serde(rename = "s2-long-verw-gdt-v1")]
    LongVerwGdtV1,
    #[serde(rename = "s2-long-fs-load-v1")]
    LongFsLoadV1,
    #[serde(rename = "s2-long-gs-load-v1")]
    LongGsLoadV1,
    #[serde(rename = "s2-long-null-ds-es-v1")]
    LongNullDsEsV1,
    #[serde(rename = "s2-long-x87-state-v1")]
    LongX87StateV1,
    #[serde(rename = "s2-long-x87-sse-state-v1")]
    LongX87SseStateV1,
    #[serde(rename = "s2-long-x87-selectors-state-v1")]
    LongX87SelectorsStateV1,
    #[serde(rename = "s2-long-fxsave-rip-v1")]
    LongFxsaveRipV1,
    #[serde(rename = "s2-long-fxsave64-rax-disp8-v1")]
    LongFxsave64RaxV1,
    #[serde(rename = "s2-long-fxsave64-cr0-gates-v1")]
    LongFxsave64Cr0GatesV1,
    #[serde(rename = "s2-long-fxrstor64-rbx-disp8-v1")]
    LongFxrstor64RbxV1,
    #[serde(rename = "s2-long-fxrstor64-cr0-gates-v1")]
    LongFxrstor64Cr0GatesV1,
    #[serde(rename = "s2-long-cr0-fp-init-v1")]
    LongCr0FpInitV1,
    #[serde(rename = "s2-long-cr4-fp-init-v1")]
    LongCr4FpInitV1,
    #[serde(rename = "s2-long-cr0-cr4-fp-init-v1")]
    LongCr0Cr4FpInitV1,
    #[serde(rename = "s2-long-cr0-cr4-fp-init-fxsave64-v1")]
    LongCr0Cr4FpInitFxsave64V1,
    #[serde(rename = "s2-long-cr0-cr4-fp-init-fxrstor64-v1")]
    LongCr0Cr4FpInitFxrstor64V1,
    #[serde(rename = "s2-long-cr0-cr4-fp-init-fxrstor64-minss-v1")]
    LongCr0Cr4FpInitFxrstor64MinssV1,
    #[serde(rename = "s2-long-movups-reg-v1")]
    LongMovupsRegV1,
    #[serde(rename = "s2-long-movaps-reg-v1")]
    LongMovapsRegV1,
    #[serde(rename = "s2-long-movaps-reg-reverse-v1")]
    LongMovapsReverseRegV1,
    #[serde(rename = "s2-long-movapd-reg-v1")]
    LongMovapdRegV1,
    #[serde(rename = "s2-long-movapd-reg-reverse-v1")]
    LongMovapdReverseRegV1,
    #[serde(rename = "s2-long-movupd-reg-v1")]
    LongMovupdRegV1,
    #[serde(rename = "s2-long-movupd-reg-reverse-v1")]
    LongMovupdReverseRegV1,
    #[serde(rename = "s2-long-xorps-reg-v1")]
    LongXorpsRegV1,
    #[serde(rename = "s2-long-xorpd-reg-v1")]
    LongXorpdRegV1,
    #[serde(rename = "s2-long-xorpd-prerequisites-v1")]
    LongXorpdPrerequisitesV1,
    #[serde(rename = "s2-long-minss-invalid-controls-v1")]
    LongMinssInvalidControlsV1,
    #[serde(rename = "s2-long-minss-unmasked-denormal-v1")]
    LongMinssUnmaskedDenormalV1,
    #[serde(rename = "s2-long-maxss-unmasked-denormal-v1")]
    LongMaxssUnmaskedDenormalV1,
    #[serde(rename = "s2-long-minsd-unmasked-denormal-v1")]
    LongMinsdUnmaskedDenormalV1,
    #[serde(rename = "s2-long-maxsd-unmasked-denormal-v1")]
    LongMaxsdUnmaskedDenormalV1,
    #[serde(rename = "s2-long-minpd-unmasked-denormal-v1")]
    LongMinpdUnmaskedDenormalV1,
    #[serde(rename = "s2-long-minps-unmasked-denormal-v1")]
    LongMinpsUnmaskedDenormalV1,
    #[serde(rename = "s2-long-maxps-unmasked-denormal-v1")]
    LongMaxpsUnmaskedDenormalV1,
    #[serde(rename = "s2-long-maxpd-unmasked-denormal-v1")]
    LongMaxpdUnmaskedDenormalV1,
    #[serde(rename = "s2-long-cvtdq2pd-reg-exact-v1")]
    LongCvtdq2pdRegExactV1,
    #[serde(rename = "s2-long-cvtdq2pd-registers-v1")]
    LongCvtdq2pdRegistersV1,
    #[serde(rename = "s2-long-cvtdq2ps-registers-v1")]
    LongCvtdq2psRegistersV1,
    #[serde(rename = "s2-long-cvtss2sd-registers-v1")]
    LongCvtss2sdRegistersV1,
    #[serde(rename = "s2-long-cvtps2pd-registers-v1")]
    LongCvtps2pdRegistersV1,
    #[serde(rename = "s2-long-comiss-ucomiss-registers-v1")]
    LongComissUcomissRegistersV1,
    #[serde(rename = "s2-long-comisd-ucomisd-registers-v1")]
    LongComisdUcomisdRegistersV1,
    #[serde(rename = "s2-long-cvtsi2sd-r32-exact-v1")]
    LongCvtsi2sdR32ExactV1,
    #[serde(rename = "s2-long-cvtsi2sd-r64-exact-v1")]
    LongCvtsi2sdR64ExactV1,
    #[serde(rename = "s2-long-cvtsi2sd-registers-v1")]
    LongCvtsi2sdRegistersV1,
    #[serde(rename = "s2-long-cvtsi2ss-registers-v1")]
    LongCvtsi2ssRegistersV1,
    #[serde(rename = "s2-long-native-xorpd-xmm0-xmm1-v1")]
    LongNativeXorpdXmm0Xmm1V1,
    #[serde(rename = "s2-long-andpd-reg-v1")]
    LongAndpdRegV1,
    #[serde(rename = "s2-long-orpd-reg-v1")]
    LongOrpdRegV1,
    #[serde(rename = "s2-long-andnpd-reg-v1")]
    LongAndnpdRegV1,
    #[serde(rename = "s2-long-pxor-reg-v1")]
    LongPxorRegV1,
    #[serde(rename = "s2-long-pand-reg-v1")]
    LongPandRegV1,
    #[serde(rename = "s2-long-pandn-reg-v1")]
    LongPandnRegV1,
    #[serde(rename = "s2-long-por-reg-v1")]
    LongPorRegV1,
    #[serde(rename = "s2-long-pavgb-reg-v1")]
    LongPavgbRegV1,
    #[serde(rename = "s2-long-pavgw-reg-v1")]
    LongPavgwRegV1,
    #[serde(rename = "s2-long-pmaxub-reg-v1")]
    LongPmaxubRegV1,
    #[serde(rename = "s2-long-pminub-reg-v1")]
    LongPminubRegV1,
    #[serde(rename = "s2-long-pmaxsw-reg-v1")]
    LongPmaxswRegV1,
    #[serde(rename = "s2-long-pminsw-reg-v1")]
    LongPminswRegV1,
    #[serde(rename = "s2-long-pmullw-reg-v1")]
    LongPmullwRegV1,
    #[serde(rename = "s2-long-pmulhuw-reg-v1")]
    LongPmulhuwRegV1,
    #[serde(rename = "s2-long-psadbw-reg-v1")]
    LongPsadbwRegV1,
    #[serde(rename = "s2-long-pmovmskb-xmm-gpr-v1")]
    LongPmovmskbXmmGprV1,
    #[serde(rename = "s2-long-pmuludq-reg-v1")]
    LongPmuludqRegV1,
    #[serde(rename = "s2-long-pmaddwd-reg-v1")]
    LongPmaddwdRegV1,
    #[serde(rename = "s2-long-paddb-reg-v1")]
    LongPaddbRegV1,
    #[serde(rename = "s2-long-paddw-reg-v1")]
    LongPaddwRegV1,
    #[serde(rename = "s2-long-psubw-reg-v1")]
    LongPsubwRegV1,
    #[serde(rename = "s2-long-paddusw-reg-v1")]
    LongPadduswRegV1,
    #[serde(rename = "s2-long-psubusw-reg-v1")]
    LongPsubuswRegV1,
    #[serde(rename = "s2-long-psubb-reg-v1")]
    LongPsubbRegV1,
    #[serde(rename = "s2-long-paddusb-reg-v1")]
    LongPaddusbRegV1,
    #[serde(rename = "s2-long-paddsb-reg-v1")]
    LongPaddsbRegV1,
    #[serde(rename = "s2-long-psubsb-reg-v1")]
    LongPsubsbRegV1,
    #[serde(rename = "s2-long-paddsw-reg-v1")]
    LongPaddswRegV1,
    #[serde(rename = "s2-long-psubsw-reg-v1")]
    LongPsubswRegV1,
    #[serde(rename = "s2-long-paddd-reg-v1")]
    LongPadddRegV1,
    #[serde(rename = "s2-long-psubd-reg-v1")]
    LongPsubdRegV1,
    #[serde(rename = "s2-long-paddq-reg-v1")]
    LongPaddqRegV1,
    #[serde(rename = "s2-long-psubq-reg-v1")]
    LongPsubqRegV1,
    #[serde(rename = "s2-long-pcmpeqb-reg-v1")]
    LongPcmpeqbRegV1,
    #[serde(rename = "s2-long-pcmpeqw-reg-v1")]
    LongPcmpeqwRegV1,
    #[serde(rename = "s2-long-pcmpeqd-reg-v1")]
    LongPcmpeqdRegV1,
    #[serde(rename = "s2-long-pcmpgtb-reg-v1")]
    LongPcmpgtbRegV1,
    #[serde(rename = "s2-long-pcmpgtw-reg-v1")]
    LongPcmpgtwRegV1,
    #[serde(rename = "s2-long-pcmpgtd-reg-v1")]
    LongPcmpgtdRegV1,
    #[serde(rename = "s2-long-punpcklbw-reg-v1")]
    LongPunpcklbwRegV1,
    #[serde(rename = "s2-long-punpckhbw-reg-v1")]
    LongPunpckhbwRegV1,
    #[serde(rename = "s2-long-punpcklwd-reg-v1")]
    LongPunpcklwdRegV1,
    #[serde(rename = "s2-long-punpckhwd-reg-v1")]
    LongPunpckhwdRegV1,
    #[serde(rename = "s2-long-punpckldq-reg-v1")]
    LongPunpckldqRegV1,
    #[serde(rename = "s2-long-punpckhdq-reg-v1")]
    LongPunpckhdqRegV1,
    #[serde(rename = "s2-long-punpcklqdq-reg-v1")]
    LongPunpcklqdqRegV1,
    #[serde(rename = "s2-long-punpckhqdq-reg-v1")]
    LongPunpckhqdqRegV1,
    #[serde(rename = "s2-long-packsswb-reg-v1")]
    LongPacksswbRegV1,
    #[serde(rename = "s2-long-packuswb-reg-v1")]
    LongPackuswbRegV1,
    #[serde(rename = "s2-long-packssdw-reg-v1")]
    LongPackssdwRegV1,
    #[serde(rename = "s2-long-pshufd-reg-v1")]
    LongPshufdRegV1,
    #[serde(rename = "s2-long-psllw-imm-reg-v1")]
    LongPsllwImmRegV1,
    #[serde(rename = "s2-long-pslld-imm-reg-v1")]
    LongPslldImmRegV1,
    #[serde(rename = "s2-long-psllq-imm-reg-v1")]
    LongPsllqImmRegV1,
    #[serde(rename = "s2-long-pslldq-imm-reg-v1")]
    LongPslldqImmRegV1,
    #[serde(rename = "s2-long-psrldq-imm-reg-v1")]
    LongPsrldqImmRegV1,
    #[serde(rename = "s2-long-psrlq-imm-reg-v1")]
    LongPsrlqImmRegV1,
    #[serde(rename = "s2-long-psrld-imm-reg-v1")]
    LongPsrldImmRegV1,
    #[serde(rename = "s2-long-psrlw-imm-reg-v1")]
    LongPsrlwImmRegV1,
    #[serde(rename = "s2-long-psraw-imm-reg-v1")]
    LongPsrawImmRegV1,
    #[serde(rename = "s2-long-psrad-imm-reg-v1")]
    LongPsradImmRegV1,
    #[serde(rename = "s2-long-psubusb-reg-v1")]
    LongPsubusbRegV1,
    #[serde(rename = "s2-long-andps-reg-v1")]
    LongAndpsRegV1,
    #[serde(rename = "s2-long-orps-reg-v1")]
    LongOrpsRegV1,
    #[serde(rename = "s2-long-andnps-reg-v1")]
    LongAndnpsRegV1,
    #[serde(rename = "s2-long-movss-reg-v1")]
    LongMovssRegV1,
    #[serde(rename = "s2-long-rsqrtss-reg-v1")]
    LongRsqrtssRegV1,
    #[serde(rename = "s2-long-rcpss-reg-v1")]
    LongRcpssRegV1,
    #[serde(rename = "s2-long-rsqrtps-reg-v1")]
    LongRsqrtpsRegV1,
    #[serde(rename = "s2-long-rcpps-reg-v1")]
    LongRcppsRegV1,
    #[serde(rename = "s2-long-minss-reg-masked-v1")]
    LongMinssMaskedRegV1,
    #[serde(rename = "s2-long-minsd-reg-masked-v1")]
    LongMinsdMaskedRegV1,
    #[serde(rename = "s2-long-maxsd-reg-masked-v1")]
    LongMaxsdMaskedRegV1,
    #[serde(rename = "s2-long-minpd-reg-masked-v1")]
    LongMinpdMaskedRegV1,
    #[serde(rename = "s2-long-maxpd-reg-masked-v1")]
    LongMaxpdMaskedRegV1,
    #[serde(rename = "s2-long-maxss-reg-masked-v1")]
    LongMaxssMaskedRegV1,
    #[serde(rename = "s2-long-minps-reg-masked-v1")]
    LongMinpsMaskedRegV1,
    #[serde(rename = "s2-long-maxps-reg-masked-v1")]
    LongMaxpsMaskedRegV1,
    #[serde(rename = "s2-long-movsd-reg-v1")]
    LongMovsdRegV1,
    #[serde(rename = "s2-long-movsd-store-reg-v1")]
    LongMovsdStoreRegV1,
    #[serde(rename = "s2-long-movss-store-reg-v1")]
    LongMovssStoreRegV1,
    #[serde(rename = "s2-long-movss-mem-load-v1")]
    LongMovssMemLoadV1,
    #[serde(rename = "s2-long-movsd-mem-load-v1")]
    LongMovsdMemLoadV1,
    #[serde(rename = "s2-long-movsd-mem-store-v1")]
    LongMovsdMemStoreV1,
    #[serde(rename = "s2-long-movss-mem-store-v1")]
    LongMovssMemStoreV1,
    #[serde(rename = "s2-long-movups-mem-load-v1")]
    LongMovupsMemLoadV1,
    #[serde(rename = "s2-long-movupd-mem-load-v1")]
    LongMovupdMemLoadV1,
    #[serde(rename = "s2-long-movupd-mem-store-v1")]
    LongMovupdMemStoreV1,
    #[serde(rename = "s2-long-movups-mem-store-v1")]
    LongMovupsMemStoreV1,
    #[serde(rename = "s2-long-movups-mem-base-v1")]
    LongMovupsMemBaseV1,
    #[serde(rename = "s2-long-movupd-mem-base-v1")]
    LongMovupdMemBaseV1,
    #[serde(rename = "s2-long-movupd-mem-rip-v1")]
    LongMovupdMemRipV1,
    #[serde(rename = "s2-long-movupd-mem-sib-v1")]
    LongMovupdMemSibV1,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    Long64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct IntegerState {
    pub mode: Mode,
    /// x86 encoding order: RAX,RCX,RDX,RBX,RSP,RBP,RSI,RDI,R8…R15.
    pub gpr: [HexU64; 16],
    pub rip: HexU64,
    pub rflags: HexU64,
    /// Present only in `s2-long-cr0-fp-init-v1`.
    #[serde(
        default,
        deserialize_with = "deserialize_present_hex_u64",
        skip_serializing_if = "Option::is_none"
    )]
    pub cr0: Option<HexU64>,
    /// Present only in `s2-long-cr4-fp-init-v1`.
    #[serde(
        default,
        deserialize_with = "deserialize_present_hex_u64",
        skip_serializing_if = "Option::is_none"
    )]
    pub cr4: Option<HexU64>,
    /// Present only in `s2-long-fs-base-v1`; omitted to preserve the old profile wire shape.
    #[serde(
        default,
        deserialize_with = "deserialize_present_hex_u64",
        skip_serializing_if = "Option::is_none"
    )]
    pub fs_base: Option<HexU64>,
    /// Present only in `s2-long-gs-base-v1`; omitted for older profile wire shapes.
    #[serde(
        default,
        deserialize_with = "deserialize_present_hex_u64",
        skip_serializing_if = "Option::is_none"
    )]
    pub gs_base: Option<HexU64>,
    /// Present only in `s2-long-swapgs-v1`; `Some(0)` is significant.
    #[serde(
        default,
        deserialize_with = "deserialize_present_hex_u64",
        skip_serializing_if = "Option::is_none"
    )]
    pub kernel_gs_base: Option<HexU64>,
    /// Present only in `s2-long-fs-selector-v1` and constrained to a 16-bit selector.
    #[serde(
        default,
        deserialize_with = "deserialize_present_hex_u64",
        skip_serializing_if = "Option::is_none"
    )]
    pub fs_selector: Option<HexU64>,
    /// Present only in `s2-long-gs-selector-v1` and constrained to a 16-bit selector.
    #[serde(
        default,
        deserialize_with = "deserialize_present_hex_u64",
        skip_serializing_if = "Option::is_none"
    )]
    pub gs_selector: Option<HexU64>,
    /// Present only in `s2-long-null-ds-es-v1`.
    #[serde(
        default,
        deserialize_with = "deserialize_present_hex_u64",
        skip_serializing_if = "Option::is_none"
    )]
    pub ds_selector: Option<HexU64>,
    #[serde(
        default,
        deserialize_with = "deserialize_present_hex_u64",
        skip_serializing_if = "Option::is_none"
    )]
    pub es_selector: Option<HexU64>,
    #[serde(
        default,
        deserialize_with = "deserialize_present_bool",
        skip_serializing_if = "Option::is_none"
    )]
    pub ds_unusable: Option<bool>,
    #[serde(
        default,
        deserialize_with = "deserialize_present_bool",
        skip_serializing_if = "Option::is_none"
    )]
    pub es_unusable: Option<bool>,
    /// Present only in the GDT-only VERW profile.
    #[serde(
        default,
        deserialize_with = "deserialize_present_hex_u64",
        skip_serializing_if = "Option::is_none"
    )]
    pub gdtr_base: Option<HexU64>,
    /// Present only in the GDT-only VERW profile; constrained to 16 bits.
    #[serde(
        default,
        deserialize_with = "deserialize_present_hex_u64",
        skip_serializing_if = "Option::is_none"
    )]
    pub gdtr_limit: Option<HexU64>,
    /// CS/SS selectors and hidden caches exist only in the exact IRETQ profile.
    #[serde(
        default,
        deserialize_with = "deserialize_present_hex_u64",
        skip_serializing_if = "Option::is_none"
    )]
    pub cs_selector: Option<HexU64>,
    #[serde(
        default,
        deserialize_with = "deserialize_present_hex_u64",
        skip_serializing_if = "Option::is_none"
    )]
    pub cs_cache_base: Option<HexU64>,
    #[serde(
        default,
        deserialize_with = "deserialize_present_hex_u64",
        skip_serializing_if = "Option::is_none"
    )]
    pub cs_cache_limit: Option<HexU64>,
    #[serde(
        default,
        deserialize_with = "deserialize_present_hex_u64",
        skip_serializing_if = "Option::is_none"
    )]
    pub cs_cache_access: Option<HexU64>,
    #[serde(
        default,
        deserialize_with = "deserialize_present_hex_u64",
        skip_serializing_if = "Option::is_none"
    )]
    pub cs_cache_flags: Option<HexU64>,
    #[serde(
        default,
        deserialize_with = "deserialize_present_hex_u64",
        skip_serializing_if = "Option::is_none"
    )]
    pub ss_selector: Option<HexU64>,
    #[serde(
        default,
        deserialize_with = "deserialize_present_hex_u64",
        skip_serializing_if = "Option::is_none"
    )]
    pub ss_cache_base: Option<HexU64>,
    #[serde(
        default,
        deserialize_with = "deserialize_present_hex_u64",
        skip_serializing_if = "Option::is_none"
    )]
    pub ss_cache_limit: Option<HexU64>,
    #[serde(
        default,
        deserialize_with = "deserialize_present_hex_u64",
        skip_serializing_if = "Option::is_none"
    )]
    pub ss_cache_access: Option<HexU64>,
    #[serde(
        default,
        deserialize_with = "deserialize_present_hex_u64",
        skip_serializing_if = "Option::is_none"
    )]
    pub ss_cache_flags: Option<HexU64>,
    /// Present only in the local descriptor-loading profile; effective hidden limit.
    #[serde(
        default,
        deserialize_with = "deserialize_present_hex_u64",
        skip_serializing_if = "Option::is_none"
    )]
    pub fs_cache_limit: Option<HexU64>,
    /// Present only in the local descriptor-loading profile; raw descriptor access byte.
    #[serde(
        default,
        deserialize_with = "deserialize_present_hex_u64",
        skip_serializing_if = "Option::is_none"
    )]
    pub fs_cache_access: Option<HexU64>,
    /// Present only in the local descriptor-loading profile; descriptor flag nibble.
    #[serde(
        default,
        deserialize_with = "deserialize_present_hex_u64",
        skip_serializing_if = "Option::is_none"
    )]
    pub fs_cache_flags: Option<HexU64>,
    /// Present only in the local MOV-GS-load profile.
    #[serde(
        default,
        deserialize_with = "deserialize_present_hex_u64",
        skip_serializing_if = "Option::is_none"
    )]
    pub gs_cache_limit: Option<HexU64>,
    #[serde(
        default,
        deserialize_with = "deserialize_present_hex_u64",
        skip_serializing_if = "Option::is_none"
    )]
    pub gs_cache_access: Option<HexU64>,
    #[serde(
        default,
        deserialize_with = "deserialize_present_hex_u64",
        skip_serializing_if = "Option::is_none"
    )]
    pub gs_cache_flags: Option<HexU64>,
    /// Full x87 state only in x87-bearing profiles; absent from SSE-only profiles.
    #[serde(
        default,
        deserialize_with = "deserialize_present_x87_snapshot",
        skip_serializing_if = "Option::is_none"
    )]
    pub x87: Option<X87Snapshot>,
    /// Present only in `s2-long-x87-sse-state-v1`.
    #[serde(
        default,
        deserialize_with = "deserialize_present_sse_snapshot",
        skip_serializing_if = "Option::is_none"
    )]
    pub sse: Option<SseSnapshot>,
    /// Present only in STI/HLT profiles; records the architectural interrupt shadow.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sti_shadow: Option<StiShadowSnapshot>,
    /// Present only in `s2-long-hlt-v1`; records running or halted activity.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub activity: Option<CpuActivitySnapshot>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StiShadowSnapshot {
    Clear,
    Blocking,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CpuActivitySnapshot {
    Running,
    Halted,
}

/// Lossless Long64 x87 observation. Register hex strings contain ten bytes in
/// increasing memory-address order; FIP/FDP are 64-bit offsets, FOP is 11 bits.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct X87Snapshot {
    pub registers: [String; 8],
    pub control_word: HexU64,
    pub status_word: HexU64,
    pub tag_word: HexU64,
    pub instruction_pointer: HexU64,
    pub data_pointer: HexU64,
    #[serde(
        default,
        deserialize_with = "deserialize_present_hex_u64",
        skip_serializing_if = "Option::is_none"
    )]
    pub instruction_selector: Option<HexU64>,
    #[serde(
        default,
        deserialize_with = "deserialize_present_hex_u64",
        skip_serializing_if = "Option::is_none"
    )]
    pub data_selector: Option<HexU64>,
    pub last_opcode: HexU64,
}

impl From<&od_isa::state::X87State> for X87Snapshot {
    fn from(state: &od_isa::state::X87State) -> Self {
        Self {
            registers: state
                .registers
                .map(|bytes| bytes.iter().map(|byte| format!("{byte:02x}")).collect()),
            control_word: HexU64(u64::from(state.control_word)),
            status_word: HexU64(u64::from(state.status_word)),
            tag_word: HexU64(u64::from(state.tag_word)),
            instruction_pointer: HexU64(state.instruction_pointer),
            data_pointer: HexU64(state.data_pointer),
            instruction_selector: state
                .instruction_selector
                .map(|value| HexU64(u64::from(value))),
            data_selector: state.data_selector.map(|value| HexU64(u64::from(value))),
            last_opcode: HexU64(u64::from(state.last_opcode)),
        }
    }
}

impl X87Snapshot {
    pub fn to_cpu(&self) -> Result<od_isa::state::X87State, FormatError> {
        self.validate()?;
        let mut registers = [[0; 10]; 8];
        for (source, destination) in self.registers.iter().zip(&mut registers) {
            for (index, byte) in destination.iter_mut().enumerate() {
                let pair = &source[index * 2..index * 2 + 2];
                *byte = u8::from_str_radix(pair, 16)
                    .map_err(|_| invalid("state.x87.registers", "invalid byte encoding"))?;
            }
        }
        Ok(od_isa::state::X87State {
            registers,
            control_word: self.control_word.get() as u16,
            status_word: self.status_word.get() as u16,
            tag_word: self.tag_word.get() as u16,
            instruction_pointer: self.instruction_pointer.get(),
            data_pointer: self.data_pointer.get(),
            instruction_selector: self.instruction_selector.map(|value| value.get() as u16),
            data_selector: self.data_selector.map(|value| value.get() as u16),
            last_opcode: self.last_opcode.get() as u16,
        })
    }

    fn validate(&self) -> Result<(), FormatError> {
        for register in &self.registers {
            if register.len() != 20
                || !register
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
            {
                return Err(invalid(
                    "state.x87.registers",
                    "each register must be exactly 20 lowercase hex digits",
                ));
            }
        }
        for (field, value) in [
            ("state.x87.control_word", self.control_word),
            ("state.x87.status_word", self.status_word),
            ("state.x87.tag_word", self.tag_word),
        ] {
            if value.get() > u16::MAX as u64 {
                return Err(invalid(field, "must fit in 16 bits"));
            }
        }
        for (field, selector) in [
            ("state.x87.instruction_selector", self.instruction_selector),
            ("state.x87.data_selector", self.data_selector),
        ] {
            if selector.is_some_and(|value| value.get() > u16::MAX as u64) {
                return Err(invalid(field, "must fit in 16 bits"));
            }
        }
        if self.last_opcode.get() > 0x7ff {
            return Err(invalid("state.x87.last_opcode", "must fit in 11 bits"));
        }
        Ok(())
    }
}

/// Lossless Long64 SSE snapshot. `diagnostic_mxcsr_mask` is profile metadata,
/// not part of architectural state; `mxcsr` remains an unrestricted raw u32.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SseSnapshot {
    pub xmm: [String; 16],
    pub mxcsr: HexU64,
    pub diagnostic_mxcsr_mask: HexU64,
}

impl From<&od_isa::state::SseState> for SseSnapshot {
    fn from(state: &od_isa::state::SseState) -> Self {
        Self {
            xmm: state
                .xmm
                .map(|bytes| bytes.iter().map(|byte| format!("{byte:02x}")).collect()),
            mxcsr: HexU64(u64::from(state.mxcsr)),
            diagnostic_mxcsr_mask: HexU64(u64::from(PROFILE_SSE_MXCSR_DIAGNOSTIC_MASK)),
        }
    }
}

impl SseSnapshot {
    pub fn to_cpu(&self) -> Result<od_isa::state::SseState, FormatError> {
        self.validate()?;
        let mut xmm = [[0; 16]; 16];
        for (source, destination) in self.xmm.iter().zip(&mut xmm) {
            for (index, byte) in destination.iter_mut().enumerate() {
                let pair = &source[index * 2..index * 2 + 2];
                *byte = u8::from_str_radix(pair, 16)
                    .map_err(|_| invalid("state.sse.xmm", "invalid byte encoding"))?;
            }
        }
        Ok(od_isa::state::SseState {
            xmm,
            mxcsr: self.mxcsr.get() as u32,
        })
    }

    fn validate(&self) -> Result<(), FormatError> {
        for register in &self.xmm {
            if register.len() != 32
                || !register
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
            {
                return Err(invalid(
                    "state.sse.xmm",
                    "each register must be exactly 32 lowercase hex digits",
                ));
            }
        }
        if self.mxcsr.get() > u32::MAX as u64 {
            return Err(invalid("state.sse.mxcsr", "must fit in 32 bits"));
        }
        if self.diagnostic_mxcsr_mask.get() != u64::from(PROFILE_SSE_MXCSR_DIAGNOSTIC_MASK) {
            return Err(invalid(
                "state.sse.diagnostic_mxcsr_mask",
                "must equal the bounded profile metadata mask 0x0000ffbf",
            ));
        }
        Ok(())
    }
}

fn deserialize_present_hex_u64<'de, D>(deserializer: D) -> Result<Option<HexU64>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    HexU64::deserialize(deserializer).map(Some)
}

fn deserialize_present_sse_snapshot<'de, D>(
    deserializer: D,
) -> Result<Option<SseSnapshot>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    SseSnapshot::deserialize(deserializer).map(Some)
}

fn deserialize_present_x87_snapshot<'de, D>(
    deserializer: D,
) -> Result<Option<X87Snapshot>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    X87Snapshot::deserialize(deserializer).map(Some)
}

fn deserialize_present_bool<'de, D>(deserializer: D) -> Result<Option<bool>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    bool::deserialize(deserializer).map(Some)
}

impl TryFrom<&LongModeState> for IntegerState {
    type Error = FormatError;

    fn try_from(state: &LongModeState) -> Result<Self, Self::Error> {
        if state.ds_cache.is_some() {
            return Err(FormatError::InvalidField {
                field: "state.ds_cache",
                reason: "DS hidden descriptor cache is not represented by this snapshot schema"
                    .into(),
            });
        }
        if state.es_cache.is_some() {
            return Err(FormatError::InvalidField {
                field: "state.es_cache",
                reason: "ES hidden descriptor cache is not represented by this snapshot schema"
                    .into(),
            });
        }
        if state.cs_selector.is_some() || state.cs_cache.is_some() {
            return Err(FormatError::InvalidField {
                field: "state.cs_selector/state.cs_cache",
                reason: "CS visible/hidden state is not represented by this snapshot schema".into(),
            });
        }
        if state.ss_selector.is_some() || state.ss_cache.is_some() {
            return Err(FormatError::InvalidField {
                field: "state.ss_selector/state.ss_cache",
                reason: "SS visible/hidden state is not represented by this snapshot schema".into(),
            });
        }
        if let Some(idtr) = state.idtr {
            return Err(FormatError::InvalidField {
                field: "state.idtr",
                reason: format!("IDTR state is not represented by this snapshot schema: {idtr:?}"),
            });
        }
        Ok(Self {
            mode: Mode::Long64,
            gpr: state.gpr.map(HexU64),
            rip: HexU64(state.rip),
            rflags: HexU64(state.rflags),
            cr0: state.cr0.map(HexU64),
            cr4: state.cr4.map(HexU64),
            fs_base: None,
            gs_base: None,
            kernel_gs_base: state.kernel_gs_base.map(HexU64),
            fs_selector: None,
            gs_selector: None,
            ds_selector: None,
            es_selector: None,
            ds_unusable: None,
            es_unusable: None,
            gdtr_base: None,
            gdtr_limit: None,
            cs_selector: None,
            cs_cache_base: None,
            cs_cache_limit: None,
            cs_cache_access: None,
            cs_cache_flags: None,
            ss_selector: None,
            ss_cache_base: None,
            ss_cache_limit: None,
            ss_cache_access: None,
            ss_cache_flags: None,
            fs_cache_limit: None,
            fs_cache_access: None,
            fs_cache_flags: None,
            gs_cache_limit: None,
            gs_cache_access: None,
            gs_cache_flags: None,
            x87: None,
            sse: None,
            sti_shadow: state.sti_shadow.map(|shadow| match shadow {
                od_isa::StiShadow::Clear => StiShadowSnapshot::Clear,
                od_isa::StiShadow::Blocking => StiShadowSnapshot::Blocking,
            }),
            activity: state.activity.map(|activity| match activity {
                od_isa::CpuActivity::Running => CpuActivitySnapshot::Running,
                od_isa::CpuActivity::Halted => CpuActivitySnapshot::Halted,
            }),
        })
    }
}

impl IntegerState {
    pub fn to_cpu(&self) -> Result<LongModeState, FormatError> {
        self.to_cpu_for(Profile::LongIntegerV1)
    }

    pub fn to_cpu_for(&self, profile: Profile) -> Result<LongModeState, FormatError> {
        self.validate_for(profile)?;
        Ok(LongModeState {
            sti_shadow: self.sti_shadow.map(|shadow| match shadow {
                StiShadowSnapshot::Clear => od_isa::StiShadow::Clear,
                StiShadowSnapshot::Blocking => od_isa::StiShadow::Blocking,
            }),
            activity: self.activity.map(|activity| match activity {
                CpuActivitySnapshot::Running => od_isa::CpuActivity::Running,
                CpuActivitySnapshot::Halted => od_isa::CpuActivity::Halted,
            }),
            gpr: self.gpr.map(HexU64::get),
            rip: self.rip.get(),
            rflags: self.rflags.get(),
            cr0: self.cr0.map(HexU64::get),
            cr4: self.cr4.map(HexU64::get),
            fs_base: self.fs_base.map_or(0, HexU64::get),
            gs_base: self.gs_base.map_or(0, HexU64::get),
            kernel_gs_base: self.kernel_gs_base.map(HexU64::get),
            fs_selector: self.fs_selector.map_or(0, |selector| selector.get() as u16),
            gs_selector: self.gs_selector.map_or(0, |selector| selector.get() as u16),
            ds_selector: self.ds_selector.map_or(0, |selector| selector.get() as u16),
            es_selector: self.es_selector.map_or(0, |selector| selector.get() as u16),
            cs_selector: self.cs_selector.map(|v| v.get() as u16),
            cs_cache: match (
                self.cs_cache_base,
                self.cs_cache_limit,
                self.cs_cache_access,
                self.cs_cache_flags,
            ) {
                (Some(base), Some(limit), Some(access), Some(flags)) => {
                    Some(od_isa::state::CodeSegmentDescriptorCache {
                        base: base.get(),
                        limit: limit.get() as u32,
                        access: access.get() as u8,
                        flags: flags.get() as u8,
                    })
                }
                _ => None,
            },
            ss_selector: self.ss_selector.map(|v| v.get() as u16),
            ss_cache: match (
                self.ss_cache_base,
                self.ss_cache_limit,
                self.ss_cache_access,
                self.ss_cache_flags,
            ) {
                (Some(base), Some(limit), Some(access), Some(flags)) => {
                    Some(od_isa::state::DataSegmentDescriptorCache {
                        base: base.get() as u32,
                        limit: limit.get() as u32,
                        access: access.get() as u8,
                        flags: flags.get() as u8,
                    })
                }
                _ => None,
            },
            ds_unusable: self.ds_unusable.unwrap_or(false),
            es_unusable: self.es_unusable.unwrap_or(false),
            ds_cache: None,
            es_cache: None,
            gdtr: match (self.gdtr_base, self.gdtr_limit) {
                (Some(base), Some(limit)) => Some(od_isa::state::Gdtr {
                    base: base.get(),
                    limit: limit.get() as u16,
                }),
                _ => None,
            },
            idtr: None,
            fs_cache: match (
                self.fs_cache_limit,
                self.fs_cache_access,
                self.fs_cache_flags,
            ) {
                (Some(limit), Some(access), Some(flags)) => {
                    Some(od_isa::state::FsDescriptorCache {
                        limit: limit.get() as u32,
                        access: access.get() as u8,
                        flags: flags.get() as u8,
                    })
                }
                _ => None,
            },
            gs_cache: match (
                self.gs_cache_limit,
                self.gs_cache_access,
                self.gs_cache_flags,
            ) {
                (Some(limit), Some(access), Some(flags)) => {
                    Some(od_isa::state::FsDescriptorCache {
                        limit: limit.get() as u32,
                        access: access.get() as u8,
                        flags: flags.get() as u8,
                    })
                }
                _ => None,
            },
            x87: match (profile, &self.x87) {
                (Profile::LongX87StateV1, Some(x87)) => Some(x87.to_cpu()?),
                (
                    Profile::LongCr0FpInitV1
                    | Profile::LongCr4FpInitV1
                    | Profile::LongCr0Cr4FpInitFxsave64V1
                    | Profile::LongCr0Cr4FpInitFxrstor64V1
                    | Profile::LongCr0Cr4FpInitFxrstor64MinssV1
                    | Profile::LongCr0Cr4FpInitV1,
                    Some(x87),
                ) => Some(x87.to_cpu()?),
                (
                    Profile::LongX87SseStateV1
                    | Profile::LongX87SelectorsStateV1
                    | Profile::LongFxsaveRipV1
                    | Profile::LongFxsave64RaxV1
                    | Profile::LongFxsave64Cr0GatesV1
                    | Profile::LongFxrstor64RbxV1
                    | Profile::LongFxrstor64Cr0GatesV1
                    | Profile::LongMovupsRegV1
                    | Profile::LongMovapsRegV1
                    | Profile::LongMovapsReverseRegV1
                    | Profile::LongMovapdRegV1
                    | Profile::LongMovupdRegV1
                    | Profile::LongMovupdReverseRegV1
                    | Profile::LongMovapdReverseRegV1
                    | Profile::LongXorpsRegV1
                    | Profile::LongXorpdRegV1
                    | Profile::LongXorpdPrerequisitesV1
                    | Profile::LongMinssInvalidControlsV1
                    | Profile::LongMinssUnmaskedDenormalV1
                    | Profile::LongMaxssUnmaskedDenormalV1
                    | Profile::LongMinsdUnmaskedDenormalV1
                    | Profile::LongMaxsdUnmaskedDenormalV1
                    | Profile::LongMinpdUnmaskedDenormalV1
                    | Profile::LongMinpsUnmaskedDenormalV1
                    | Profile::LongMaxpsUnmaskedDenormalV1
                    | Profile::LongMaxpdUnmaskedDenormalV1
                    | Profile::LongAndpdRegV1
                    | Profile::LongOrpdRegV1
                    | Profile::LongAndnpdRegV1
                    | Profile::LongPxorRegV1
                    | Profile::LongPandRegV1
                    | Profile::LongPandnRegV1
                    | Profile::LongPorRegV1
                    | Profile::LongPavgbRegV1
                    | Profile::LongPavgwRegV1
                    | Profile::LongPmaxubRegV1
                    | Profile::LongPminubRegV1
                    | Profile::LongPmaxswRegV1
                    | Profile::LongPminswRegV1
                    | Profile::LongPmullwRegV1
                    | Profile::LongPmulhuwRegV1
                    | Profile::LongPsadbwRegV1
                    | Profile::LongPmovmskbXmmGprV1
                    | Profile::LongPmuludqRegV1
                    | Profile::LongPmaddwdRegV1
                    | Profile::LongPaddbRegV1
                    | Profile::LongPaddwRegV1
                    | Profile::LongPsubwRegV1
                    | Profile::LongPadduswRegV1
                    | Profile::LongPsubuswRegV1
                    | Profile::LongPsubbRegV1
                    | Profile::LongPaddusbRegV1
                    | Profile::LongPaddsbRegV1
                    | Profile::LongPsubsbRegV1
                    | Profile::LongPaddswRegV1
                    | Profile::LongPsubswRegV1
                    | Profile::LongPadddRegV1
                    | Profile::LongPsubdRegV1
                    | Profile::LongPaddqRegV1
                    | Profile::LongPsubqRegV1
                    | Profile::LongPcmpeqbRegV1
                    | Profile::LongPcmpeqwRegV1
                    | Profile::LongPcmpeqdRegV1
                    | Profile::LongPcmpgtbRegV1
                    | Profile::LongPcmpgtwRegV1
                    | Profile::LongPcmpgtdRegV1
                    | Profile::LongPunpcklbwRegV1
                    | Profile::LongPunpckhbwRegV1
                    | Profile::LongPunpcklwdRegV1
                    | Profile::LongPunpckhwdRegV1
                    | Profile::LongPunpckldqRegV1
                    | Profile::LongPunpckhdqRegV1
                    | Profile::LongPunpcklqdqRegV1
                    | Profile::LongPunpckhqdqRegV1
                    | Profile::LongPacksswbRegV1
                    | Profile::LongPackuswbRegV1
                    | Profile::LongPackssdwRegV1
                    | Profile::LongPshufdRegV1
                    | Profile::LongPsllwImmRegV1
                    | Profile::LongPslldImmRegV1
                    | Profile::LongPsllqImmRegV1
                    | Profile::LongPslldqImmRegV1
                    | Profile::LongPsrldqImmRegV1
                    | Profile::LongPsrlqImmRegV1
                    | Profile::LongPsrldImmRegV1
                    | Profile::LongPsrlwImmRegV1
                    | Profile::LongPsrawImmRegV1
                    | Profile::LongPsradImmRegV1
                    | Profile::LongPsubusbRegV1
                    | Profile::LongAndpsRegV1
                    | Profile::LongOrpsRegV1
                    | Profile::LongAndnpsRegV1
                    | Profile::LongMovsdRegV1
                    | Profile::LongMovsdStoreRegV1
                    | Profile::LongRcpssRegV1
                    | Profile::LongRsqrtpsRegV1
                    | Profile::LongRcppsRegV1
                    | Profile::LongMinssMaskedRegV1
                    | Profile::LongMinsdMaskedRegV1
                    | Profile::LongMaxsdMaskedRegV1
                    | Profile::LongMinpdMaskedRegV1
                    | Profile::LongMaxpdMaskedRegV1
                    | Profile::LongMaxssMaskedRegV1
                    | Profile::LongMinpsMaskedRegV1
                    | Profile::LongMaxpsMaskedRegV1
                    | Profile::LongRsqrtssRegV1
                    | Profile::LongMovssRegV1
                    | Profile::LongMovssMemLoadV1
                    | Profile::LongMovsdMemLoadV1
                    | Profile::LongMovsdMemStoreV1
                    | Profile::LongMovssMemStoreV1
                    | Profile::LongMovssStoreRegV1
                    | Profile::LongMovupsMemLoadV1
                    | Profile::LongMovupdMemLoadV1
                    | Profile::LongMovupdMemStoreV1
                    | Profile::LongMovupsMemStoreV1
                    | Profile::LongMovupsMemBaseV1
                    | Profile::LongMovupdMemBaseV1
                    | Profile::LongMovupdMemRipV1
                    | Profile::LongMovupdMemSibV1,
                    Some(x87),
                ) => Some(x87.to_cpu()?),
                _ => None,
            },
            sse: match (profile, &self.sse) {
                (
                    Profile::LongX87SseStateV1
                    | Profile::LongCr0FpInitV1
                    | Profile::LongCr4FpInitV1
                    | Profile::LongCr0Cr4FpInitFxsave64V1
                    | Profile::LongCr0Cr4FpInitFxrstor64V1
                    | Profile::LongCr0Cr4FpInitFxrstor64MinssV1
                    | Profile::LongCr0Cr4FpInitV1
                    | Profile::LongX87SelectorsStateV1
                    | Profile::LongFxsaveRipV1
                    | Profile::LongFxsave64RaxV1
                    | Profile::LongFxsave64Cr0GatesV1
                    | Profile::LongFxrstor64RbxV1
                    | Profile::LongFxrstor64Cr0GatesV1
                    | Profile::LongMovupsRegV1
                    | Profile::LongMovapsRegV1
                    | Profile::LongMovapsReverseRegV1
                    | Profile::LongMovapdRegV1
                    | Profile::LongMovupdRegV1
                    | Profile::LongMovupdReverseRegV1
                    | Profile::LongMovapdReverseRegV1
                    | Profile::LongXorpsRegV1
                    | Profile::LongXorpdRegV1
                    | Profile::LongXorpdPrerequisitesV1
                    | Profile::LongMinssInvalidControlsV1
                    | Profile::LongMinssUnmaskedDenormalV1
                    | Profile::LongMaxssUnmaskedDenormalV1
                    | Profile::LongMinsdUnmaskedDenormalV1
                    | Profile::LongMaxsdUnmaskedDenormalV1
                    | Profile::LongMinpdUnmaskedDenormalV1
                    | Profile::LongMinpsUnmaskedDenormalV1
                    | Profile::LongMaxpsUnmaskedDenormalV1
                    | Profile::LongMaxpdUnmaskedDenormalV1
                    | Profile::LongCvtdq2pdRegExactV1
                    | Profile::LongCvtdq2pdRegistersV1
                    | Profile::LongCvtdq2psRegistersV1
                    | Profile::LongCvtss2sdRegistersV1
                    | Profile::LongCvtps2pdRegistersV1
                    | Profile::LongComissUcomissRegistersV1
                    | Profile::LongComisdUcomisdRegistersV1
                    | Profile::LongCvtsi2sdR32ExactV1
                    | Profile::LongCvtsi2sdR64ExactV1
                    | Profile::LongCvtsi2sdRegistersV1
                    | Profile::LongCvtsi2ssRegistersV1
                    | Profile::LongNativeXorpdXmm0Xmm1V1
                    | Profile::LongAndpdRegV1
                    | Profile::LongOrpdRegV1
                    | Profile::LongAndnpdRegV1
                    | Profile::LongPxorRegV1
                    | Profile::LongPandRegV1
                    | Profile::LongPandnRegV1
                    | Profile::LongPorRegV1
                    | Profile::LongPavgbRegV1
                    | Profile::LongPavgwRegV1
                    | Profile::LongPmaxubRegV1
                    | Profile::LongPminubRegV1
                    | Profile::LongPmaxswRegV1
                    | Profile::LongPminswRegV1
                    | Profile::LongPmullwRegV1
                    | Profile::LongPmulhuwRegV1
                    | Profile::LongPsadbwRegV1
                    | Profile::LongPmovmskbXmmGprV1
                    | Profile::LongPmuludqRegV1
                    | Profile::LongPmaddwdRegV1
                    | Profile::LongPaddbRegV1
                    | Profile::LongPaddwRegV1
                    | Profile::LongPsubwRegV1
                    | Profile::LongPadduswRegV1
                    | Profile::LongPsubuswRegV1
                    | Profile::LongPsubbRegV1
                    | Profile::LongPaddusbRegV1
                    | Profile::LongPaddsbRegV1
                    | Profile::LongPsubsbRegV1
                    | Profile::LongPaddswRegV1
                    | Profile::LongPsubswRegV1
                    | Profile::LongPadddRegV1
                    | Profile::LongPsubdRegV1
                    | Profile::LongPaddqRegV1
                    | Profile::LongPsubqRegV1
                    | Profile::LongPcmpeqbRegV1
                    | Profile::LongPcmpeqwRegV1
                    | Profile::LongPcmpeqdRegV1
                    | Profile::LongPcmpgtbRegV1
                    | Profile::LongPcmpgtwRegV1
                    | Profile::LongPcmpgtdRegV1
                    | Profile::LongPunpcklbwRegV1
                    | Profile::LongPunpckhbwRegV1
                    | Profile::LongPunpcklwdRegV1
                    | Profile::LongPunpckhwdRegV1
                    | Profile::LongPunpckldqRegV1
                    | Profile::LongPunpckhdqRegV1
                    | Profile::LongPunpcklqdqRegV1
                    | Profile::LongPunpckhqdqRegV1
                    | Profile::LongPacksswbRegV1
                    | Profile::LongPackuswbRegV1
                    | Profile::LongPackssdwRegV1
                    | Profile::LongPshufdRegV1
                    | Profile::LongPsllwImmRegV1
                    | Profile::LongPslldImmRegV1
                    | Profile::LongPsllqImmRegV1
                    | Profile::LongPslldqImmRegV1
                    | Profile::LongPsrldqImmRegV1
                    | Profile::LongPsrlqImmRegV1
                    | Profile::LongPsrldImmRegV1
                    | Profile::LongPsrlwImmRegV1
                    | Profile::LongPsrawImmRegV1
                    | Profile::LongPsradImmRegV1
                    | Profile::LongPsubusbRegV1
                    | Profile::LongAndpsRegV1
                    | Profile::LongOrpsRegV1
                    | Profile::LongAndnpsRegV1
                    | Profile::LongMovsdRegV1
                    | Profile::LongMovsdStoreRegV1
                    | Profile::LongRcpssRegV1
                    | Profile::LongRsqrtpsRegV1
                    | Profile::LongRcppsRegV1
                    | Profile::LongMinssMaskedRegV1
                    | Profile::LongMinsdMaskedRegV1
                    | Profile::LongMaxsdMaskedRegV1
                    | Profile::LongMinpdMaskedRegV1
                    | Profile::LongMaxpdMaskedRegV1
                    | Profile::LongMaxssMaskedRegV1
                    | Profile::LongMinpsMaskedRegV1
                    | Profile::LongMaxpsMaskedRegV1
                    | Profile::LongRsqrtssRegV1
                    | Profile::LongMovssRegV1
                    | Profile::LongMovssMemLoadV1
                    | Profile::LongMovsdMemLoadV1
                    | Profile::LongMovsdMemStoreV1
                    | Profile::LongMovssMemStoreV1
                    | Profile::LongMovssStoreRegV1
                    | Profile::LongMovupsMemLoadV1
                    | Profile::LongMovupdMemLoadV1
                    | Profile::LongMovupdMemStoreV1
                    | Profile::LongMovupsMemStoreV1
                    | Profile::LongMovupsMemBaseV1
                    | Profile::LongMovupdMemBaseV1
                    | Profile::LongMovupdMemRipV1
                    | Profile::LongMovupdMemSibV1,
                    Some(sse),
                ) => Some(sse.to_cpu()?),
                _ => None,
            },
        })
    }

    pub fn from_cpu_for(state: &LongModeState, profile: Profile) -> Result<Self, FormatError> {
        match (profile, state.kernel_gs_base) {
            (Profile::LongSwapgsV1 | Profile::LongKernelGsBaseV1, Some(base)) => {
                let upper = base >> 47;
                if upper != 0 && upper != 0x1ffff {
                    return Err(invalid("state.kernel_gs_base", "must be canonical48"));
                }
            }
            (Profile::LongSwapgsV1 | Profile::LongKernelGsBaseV1, None) => {
                return Err(invalid("state.kernel_gs_base", "required by this profile"));
            }
            (_, Some(_)) => {
                return Err(invalid("state.kernel_gs_base", "not part of this profile"));
            }
            (_, None) => {}
        }
        if profile != Profile::LongX87SelectorsStateV1
            && profile != Profile::LongFxsaveRipV1
            && profile != Profile::LongFxsave64RaxV1
            && profile != Profile::LongFxsave64Cr0GatesV1
            && profile != Profile::LongFxrstor64RbxV1
            && profile != Profile::LongFxrstor64Cr0GatesV1
            && state.x87.as_ref().is_some_and(|x87| {
                x87.instruction_selector.is_some() || x87.data_selector.is_some()
            })
        {
            return Err(invalid(
                "state.x87.selectors",
                "selectors are not part of this profile",
            ));
        }
        let mut result = if matches!(
            profile,
            Profile::LongIretqCpl0ValidV1 | Profile::LongLretqCpl0ValidV1
        ) {
            let mut projected = state.clone();
            projected.cs_selector = None;
            projected.cs_cache = None;
            projected.ss_selector = None;
            projected.ss_cache = None;
            Self::try_from(&projected)?
        } else {
            Self::try_from(state)?
        };
        result.x87 = if profile == Profile::LongX87StateV1 {
            state.x87.as_ref().map(X87Snapshot::from)
        } else if matches!(
            profile,
            Profile::LongX87SseStateV1
                | Profile::LongX87SelectorsStateV1
                | Profile::LongCr0FpInitV1
                | Profile::LongCr4FpInitV1
                | Profile::LongCr0Cr4FpInitFxsave64V1
                | Profile::LongCr0Cr4FpInitFxrstor64V1
                | Profile::LongCr0Cr4FpInitFxrstor64MinssV1
                | Profile::LongCr0Cr4FpInitV1
                | Profile::LongFxsaveRipV1
                | Profile::LongFxsave64RaxV1
                | Profile::LongFxsave64Cr0GatesV1
                | Profile::LongFxrstor64RbxV1
                | Profile::LongFxrstor64Cr0GatesV1
                | Profile::LongMovupsRegV1
                | Profile::LongMovapsRegV1
                | Profile::LongMovapsReverseRegV1
                | Profile::LongMovapdRegV1
                | Profile::LongMovupdRegV1
                | Profile::LongMovupdReverseRegV1
                | Profile::LongMovapdReverseRegV1
                | Profile::LongXorpsRegV1
                | Profile::LongXorpdRegV1
                | Profile::LongXorpdPrerequisitesV1
                | Profile::LongMinssInvalidControlsV1
                | Profile::LongMinssUnmaskedDenormalV1
                | Profile::LongMaxssUnmaskedDenormalV1
                | Profile::LongMinsdUnmaskedDenormalV1
                | Profile::LongMaxsdUnmaskedDenormalV1
                | Profile::LongMinpdUnmaskedDenormalV1
                | Profile::LongMinpsUnmaskedDenormalV1
                | Profile::LongMaxpsUnmaskedDenormalV1
                | Profile::LongMaxpdUnmaskedDenormalV1
                | Profile::LongCvtdq2pdRegExactV1
                | Profile::LongCvtdq2pdRegistersV1
                | Profile::LongCvtdq2psRegistersV1
                | Profile::LongCvtss2sdRegistersV1
                | Profile::LongCvtps2pdRegistersV1
                | Profile::LongComissUcomissRegistersV1
                | Profile::LongComisdUcomisdRegistersV1
                | Profile::LongCvtsi2sdR32ExactV1
                | Profile::LongCvtsi2sdR64ExactV1
                | Profile::LongCvtsi2sdRegistersV1
                | Profile::LongCvtsi2ssRegistersV1
                | Profile::LongAndpdRegV1
                | Profile::LongOrpdRegV1
                | Profile::LongAndnpdRegV1
                | Profile::LongPxorRegV1
                | Profile::LongPandRegV1
                | Profile::LongPandnRegV1
                | Profile::LongPorRegV1
                | Profile::LongPavgbRegV1
                | Profile::LongPavgwRegV1
                | Profile::LongPmaxubRegV1
                | Profile::LongPminubRegV1
                | Profile::LongPmaxswRegV1
                | Profile::LongPminswRegV1
                | Profile::LongPmullwRegV1
                | Profile::LongPmulhuwRegV1
                | Profile::LongPsadbwRegV1
                | Profile::LongPmovmskbXmmGprV1
                | Profile::LongPmuludqRegV1
                | Profile::LongPmaddwdRegV1
                | Profile::LongPaddbRegV1
                | Profile::LongPaddwRegV1
                | Profile::LongPsubwRegV1
                | Profile::LongPadduswRegV1
                | Profile::LongPsubuswRegV1
                | Profile::LongPsubbRegV1
                | Profile::LongPaddusbRegV1
                | Profile::LongPaddsbRegV1
                | Profile::LongPsubsbRegV1
                | Profile::LongPaddswRegV1
                | Profile::LongPsubswRegV1
                | Profile::LongPadddRegV1
                | Profile::LongPsubdRegV1
                | Profile::LongPaddqRegV1
                | Profile::LongPsubqRegV1
                | Profile::LongPcmpeqbRegV1
                | Profile::LongPcmpeqwRegV1
                | Profile::LongPcmpeqdRegV1
                | Profile::LongPcmpgtbRegV1
                | Profile::LongPcmpgtwRegV1
                | Profile::LongPcmpgtdRegV1
                | Profile::LongPunpcklbwRegV1
                | Profile::LongPunpckhbwRegV1
                | Profile::LongPunpcklwdRegV1
                | Profile::LongPunpckhwdRegV1
                | Profile::LongPunpckldqRegV1
                | Profile::LongPunpckhdqRegV1
                | Profile::LongPunpcklqdqRegV1
                | Profile::LongPunpckhqdqRegV1
                | Profile::LongPacksswbRegV1
                | Profile::LongPackuswbRegV1
                | Profile::LongPackssdwRegV1
                | Profile::LongPshufdRegV1
                | Profile::LongPsllwImmRegV1
                | Profile::LongPslldImmRegV1
                | Profile::LongPsllqImmRegV1
                | Profile::LongPslldqImmRegV1
                | Profile::LongPsrldqImmRegV1
                | Profile::LongPsrlqImmRegV1
                | Profile::LongPsrldImmRegV1
                | Profile::LongPsrlwImmRegV1
                | Profile::LongPsrawImmRegV1
                | Profile::LongPsradImmRegV1
                | Profile::LongPsubusbRegV1
                | Profile::LongAndpsRegV1
                | Profile::LongOrpsRegV1
                | Profile::LongAndnpsRegV1
                | Profile::LongMovsdRegV1
                | Profile::LongMovsdStoreRegV1
                | Profile::LongRcpssRegV1
                | Profile::LongRsqrtpsRegV1
                | Profile::LongRcppsRegV1
                | Profile::LongMinssMaskedRegV1
                | Profile::LongMinsdMaskedRegV1
                | Profile::LongMaxsdMaskedRegV1
                | Profile::LongMinpdMaskedRegV1
                | Profile::LongMaxpdMaskedRegV1
                | Profile::LongMaxssMaskedRegV1
                | Profile::LongMinpsMaskedRegV1
                | Profile::LongMaxpsMaskedRegV1
                | Profile::LongRsqrtssRegV1
                | Profile::LongMovssRegV1
                | Profile::LongMovssMemLoadV1
                | Profile::LongMovsdMemLoadV1
                | Profile::LongMovsdMemStoreV1
                | Profile::LongMovssMemStoreV1
                | Profile::LongMovssStoreRegV1
                | Profile::LongMovupsMemLoadV1
                | Profile::LongMovupdMemLoadV1
                | Profile::LongMovupdMemStoreV1
                | Profile::LongMovupsMemStoreV1
                | Profile::LongMovupsMemBaseV1
                | Profile::LongMovupdMemBaseV1
                | Profile::LongMovupdMemRipV1
                | Profile::LongMovupdMemSibV1
        ) {
            state.x87.as_ref().map(X87Snapshot::from)
        } else {
            None
        };
        result.sse = if matches!(
            profile,
            Profile::LongX87SseStateV1
                | Profile::LongX87SelectorsStateV1
                | Profile::LongCr0FpInitV1
                | Profile::LongCr4FpInitV1
                | Profile::LongCr0Cr4FpInitFxsave64V1
                | Profile::LongCr0Cr4FpInitFxrstor64V1
                | Profile::LongCr0Cr4FpInitFxrstor64MinssV1
                | Profile::LongCr0Cr4FpInitV1
                | Profile::LongFxsaveRipV1
                | Profile::LongFxsave64RaxV1
                | Profile::LongFxsave64Cr0GatesV1
                | Profile::LongFxrstor64RbxV1
                | Profile::LongFxrstor64Cr0GatesV1
                | Profile::LongMovupsRegV1
                | Profile::LongMovapsRegV1
                | Profile::LongMovapsReverseRegV1
                | Profile::LongMovapdRegV1
                | Profile::LongMovupdRegV1
                | Profile::LongMovupdReverseRegV1
                | Profile::LongMovapdReverseRegV1
                | Profile::LongXorpsRegV1
                | Profile::LongXorpdRegV1
                | Profile::LongXorpdPrerequisitesV1
                | Profile::LongMinssInvalidControlsV1
                | Profile::LongMinssUnmaskedDenormalV1
                | Profile::LongMaxssUnmaskedDenormalV1
                | Profile::LongMinsdUnmaskedDenormalV1
                | Profile::LongMaxsdUnmaskedDenormalV1
                | Profile::LongMinpdUnmaskedDenormalV1
                | Profile::LongMinpsUnmaskedDenormalV1
                | Profile::LongMaxpsUnmaskedDenormalV1
                | Profile::LongMaxpdUnmaskedDenormalV1
                | Profile::LongCvtdq2pdRegExactV1
                | Profile::LongCvtdq2pdRegistersV1
                | Profile::LongCvtdq2psRegistersV1
                | Profile::LongCvtss2sdRegistersV1
                | Profile::LongCvtps2pdRegistersV1
                | Profile::LongComissUcomissRegistersV1
                | Profile::LongComisdUcomisdRegistersV1
                | Profile::LongCvtsi2sdR32ExactV1
                | Profile::LongCvtsi2sdR64ExactV1
                | Profile::LongCvtsi2sdRegistersV1
                | Profile::LongCvtsi2ssRegistersV1
                | Profile::LongNativeXorpdXmm0Xmm1V1
                | Profile::LongAndpdRegV1
                | Profile::LongOrpdRegV1
                | Profile::LongAndnpdRegV1
                | Profile::LongPxorRegV1
                | Profile::LongPandRegV1
                | Profile::LongPandnRegV1
                | Profile::LongPorRegV1
                | Profile::LongPavgbRegV1
                | Profile::LongPavgwRegV1
                | Profile::LongPmaxubRegV1
                | Profile::LongPminubRegV1
                | Profile::LongPmaxswRegV1
                | Profile::LongPminswRegV1
                | Profile::LongPmullwRegV1
                | Profile::LongPmulhuwRegV1
                | Profile::LongPsadbwRegV1
                | Profile::LongPmovmskbXmmGprV1
                | Profile::LongPmuludqRegV1
                | Profile::LongPmaddwdRegV1
                | Profile::LongPaddbRegV1
                | Profile::LongPaddwRegV1
                | Profile::LongPsubwRegV1
                | Profile::LongPadduswRegV1
                | Profile::LongPsubuswRegV1
                | Profile::LongPsubbRegV1
                | Profile::LongPaddusbRegV1
                | Profile::LongPaddsbRegV1
                | Profile::LongPsubsbRegV1
                | Profile::LongPaddswRegV1
                | Profile::LongPsubswRegV1
                | Profile::LongPadddRegV1
                | Profile::LongPsubdRegV1
                | Profile::LongPaddqRegV1
                | Profile::LongPsubqRegV1
                | Profile::LongPcmpeqbRegV1
                | Profile::LongPcmpeqwRegV1
                | Profile::LongPcmpeqdRegV1
                | Profile::LongPcmpgtbRegV1
                | Profile::LongPcmpgtwRegV1
                | Profile::LongPcmpgtdRegV1
                | Profile::LongPunpcklbwRegV1
                | Profile::LongPunpckhbwRegV1
                | Profile::LongPunpcklwdRegV1
                | Profile::LongPunpckhwdRegV1
                | Profile::LongPunpckldqRegV1
                | Profile::LongPunpckhdqRegV1
                | Profile::LongPunpcklqdqRegV1
                | Profile::LongPunpckhqdqRegV1
                | Profile::LongPacksswbRegV1
                | Profile::LongPackuswbRegV1
                | Profile::LongPackssdwRegV1
                | Profile::LongPshufdRegV1
                | Profile::LongPsllwImmRegV1
                | Profile::LongPslldImmRegV1
                | Profile::LongPsllqImmRegV1
                | Profile::LongPslldqImmRegV1
                | Profile::LongPsrldqImmRegV1
                | Profile::LongPsrlqImmRegV1
                | Profile::LongPsrldImmRegV1
                | Profile::LongPsrlwImmRegV1
                | Profile::LongPsrawImmRegV1
                | Profile::LongPsradImmRegV1
                | Profile::LongPsubusbRegV1
                | Profile::LongAndpsRegV1
                | Profile::LongOrpsRegV1
                | Profile::LongAndnpsRegV1
                | Profile::LongMovsdRegV1
                | Profile::LongMovsdStoreRegV1
                | Profile::LongRcpssRegV1
                | Profile::LongRsqrtpsRegV1
                | Profile::LongRcppsRegV1
                | Profile::LongMinssMaskedRegV1
                | Profile::LongMinsdMaskedRegV1
                | Profile::LongMaxsdMaskedRegV1
                | Profile::LongMinpdMaskedRegV1
                | Profile::LongMaxpdMaskedRegV1
                | Profile::LongMaxssMaskedRegV1
                | Profile::LongMinpsMaskedRegV1
                | Profile::LongMaxpsMaskedRegV1
                | Profile::LongRsqrtssRegV1
                | Profile::LongMovssRegV1
                | Profile::LongMovssMemLoadV1
                | Profile::LongMovsdMemLoadV1
                | Profile::LongMovsdMemStoreV1
                | Profile::LongMovssMemStoreV1
                | Profile::LongMovssStoreRegV1
                | Profile::LongMovupsMemLoadV1
                | Profile::LongMovupdMemLoadV1
                | Profile::LongMovupdMemStoreV1
                | Profile::LongMovupsMemStoreV1
                | Profile::LongMovupsMemBaseV1
                | Profile::LongMovupdMemBaseV1
                | Profile::LongMovupdMemRipV1
                | Profile::LongMovupdMemSibV1
        ) {
            state.sse.as_ref().map(SseSnapshot::from)
        } else {
            None
        };
        if profile == Profile::LongFsBaseV1 || profile == Profile::LongFsBaseMsrV1 {
            result.fs_base = Some(HexU64(state.fs_base));
        } else if profile == Profile::LongGsBaseV1 {
            result.gs_base = Some(HexU64(state.gs_base));
        } else if profile == Profile::LongSwapgsV1 || profile == Profile::LongKernelGsBaseV1 {
            result.gs_base = Some(HexU64(state.gs_base));
            result.kernel_gs_base = state.kernel_gs_base.map(HexU64);
        } else if profile == Profile::LongFsSelectorV1 {
            result.fs_base = Some(HexU64(state.fs_base));
            result.fs_selector = Some(HexU64(u64::from(state.fs_selector)));
        } else if profile == Profile::LongGsSelectorV1 {
            result.gs_base = Some(HexU64(state.gs_base));
            result.gs_selector = Some(HexU64(u64::from(state.gs_selector)));
        } else if profile == Profile::LongVerwGdtV1 {
            if let Some(gdtr) = state.gdtr {
                result.gdtr_base = Some(HexU64(gdtr.base));
                result.gdtr_limit = Some(HexU64(u64::from(gdtr.limit)));
            }
        } else if profile == Profile::LongFsLoadV1 {
            result.fs_base = Some(HexU64(state.fs_base));
            result.fs_selector = Some(HexU64(u64::from(state.fs_selector)));
            if let Some(gdtr) = state.gdtr {
                result.gdtr_base = Some(HexU64(gdtr.base));
                result.gdtr_limit = Some(HexU64(u64::from(gdtr.limit)));
            }
            if let Some(cache) = state.fs_cache {
                result.fs_cache_limit = Some(HexU64(u64::from(cache.limit)));
                result.fs_cache_access = Some(HexU64(u64::from(cache.access)));
                result.fs_cache_flags = Some(HexU64(u64::from(cache.flags)));
            }
        } else if profile == Profile::LongGsLoadV1 {
            result.gs_base = Some(HexU64(state.gs_base));
            result.gs_selector = Some(HexU64(u64::from(state.gs_selector)));
            if let Some(gdtr) = state.gdtr {
                result.gdtr_base = Some(HexU64(gdtr.base));
                result.gdtr_limit = Some(HexU64(u64::from(gdtr.limit)));
            }
            if let Some(cache) = state.gs_cache {
                result.gs_cache_limit = Some(HexU64(u64::from(cache.limit)));
                result.gs_cache_access = Some(HexU64(u64::from(cache.access)));
                result.gs_cache_flags = Some(HexU64(u64::from(cache.flags)));
            }
        } else if profile == Profile::LongNullDsEsV1 {
            result.ds_selector = Some(HexU64(u64::from(state.ds_selector)));
            result.es_selector = Some(HexU64(u64::from(state.es_selector)));
            result.ds_unusable = Some(state.ds_unusable);
            result.es_unusable = Some(state.es_unusable);
        } else if matches!(
            profile,
            Profile::LongIretqCpl0ValidV1 | Profile::LongLretqCpl0ValidV1
        ) {
            if let Some(gdtr) = state.gdtr {
                result.gdtr_base = Some(HexU64(gdtr.base));
                result.gdtr_limit = Some(HexU64(u64::from(gdtr.limit)));
            }
            result.cs_selector = state.cs_selector.map(|v| HexU64(u64::from(v)));
            result.cs_cache_base = state.cs_cache.map(|v| HexU64(v.base));
            result.cs_cache_limit = state.cs_cache.map(|v| HexU64(u64::from(v.limit)));
            result.cs_cache_access = state.cs_cache.map(|v| HexU64(u64::from(v.access)));
            result.cs_cache_flags = state.cs_cache.map(|v| HexU64(u64::from(v.flags)));
            result.ss_selector = state.ss_selector.map(|v| HexU64(u64::from(v)));
            result.ss_cache_base = state.ss_cache.map(|v| HexU64(u64::from(v.base)));
            result.ss_cache_limit = state.ss_cache.map(|v| HexU64(u64::from(v.limit)));
            result.ss_cache_access = state.ss_cache.map(|v| HexU64(u64::from(v.access)));
            result.ss_cache_flags = state.ss_cache.map(|v| HexU64(u64::from(v.flags)));
        }
        Ok(result)
    }

    fn validate_for(&self, profile: Profile) -> Result<(), FormatError> {
        match (profile, self.sti_shadow) {
            (
                Profile::LongStiV1
                | Profile::LongHltV1
                | Profile::LongIretqCpl0ValidV1
                | Profile::LongLretqCpl0ValidV1
                | Profile::LongCr0FpInitV1
                | Profile::LongCr4FpInitV1
                | Profile::LongCr0Cr4FpInitFxsave64V1
                | Profile::LongCr0Cr4FpInitFxrstor64V1
                | Profile::LongCr0Cr4FpInitFxrstor64MinssV1
                | Profile::LongCr0Cr4FpInitV1,
                Some(_),
            ) => {}
            (
                Profile::LongStiV1
                | Profile::LongHltV1
                | Profile::LongIretqCpl0ValidV1
                | Profile::LongLretqCpl0ValidV1
                | Profile::LongCr0FpInitV1
                | Profile::LongCr4FpInitV1
                | Profile::LongCr0Cr4FpInitFxsave64V1
                | Profile::LongCr0Cr4FpInitFxrstor64V1
                | Profile::LongCr0Cr4FpInitFxrstor64MinssV1
                | Profile::LongCr0Cr4FpInitV1,
                None,
            ) => {
                return Err(invalid(
                    "state.sti_shadow",
                    "required by the STI/HLT profile",
                ));
            }
            (_, Some(_)) => return Err(invalid("state.sti_shadow", "not part of this profile")),
            (_, None) => {}
        }
        match (profile, self.activity) {
            (
                Profile::LongHltV1
                | Profile::LongIretqCpl0ValidV1
                | Profile::LongLretqCpl0ValidV1
                | Profile::LongCr0FpInitV1
                | Profile::LongCr4FpInitV1
                | Profile::LongCr0Cr4FpInitFxsave64V1
                | Profile::LongCr0Cr4FpInitFxrstor64V1
                | Profile::LongCr0Cr4FpInitFxrstor64MinssV1
                | Profile::LongCr0Cr4FpInitV1,
                Some(_),
            ) => {}
            (
                Profile::LongHltV1
                | Profile::LongIretqCpl0ValidV1
                | Profile::LongLretqCpl0ValidV1
                | Profile::LongCr0FpInitV1
                | Profile::LongCr4FpInitV1
                | Profile::LongCr0Cr4FpInitFxsave64V1
                | Profile::LongCr0Cr4FpInitFxrstor64V1
                | Profile::LongCr0Cr4FpInitFxrstor64MinssV1
                | Profile::LongCr0Cr4FpInitV1,
                None,
            ) => {
                return Err(invalid("state.activity", "required by the HLT profile"));
            }
            (_, Some(_)) => return Err(invalid("state.activity", "not part of this profile")),
            (_, None) => {}
        }
        if matches!(
            profile,
            Profile::LongCr0FpInitV1
                | Profile::LongCr4FpInitV1
                | Profile::LongCr0Cr4FpInitFxsave64V1
                | Profile::LongCr0Cr4FpInitFxrstor64V1
                | Profile::LongCr0Cr4FpInitFxrstor64MinssV1
                | Profile::LongCr0Cr4FpInitV1,
        ) && (self.activity != Some(CpuActivitySnapshot::Running)
            || self.sti_shadow != Some(StiShadowSnapshot::Clear))
        {
            return Err(invalid(
                "state.activity/state.sti_shadow",
                "CR0 FP-init requires Running activity and Clear STI shadow",
            ));
        }
        if matches!(
            profile,
            Profile::LongHltV1 | Profile::LongIretqCpl0ValidV1 | Profile::LongLretqCpl0ValidV1
        ) && self.activity == Some(CpuActivitySnapshot::Halted)
            && self.sti_shadow == Some(StiShadowSnapshot::Blocking)
        {
            return Err(invalid(
                "state.activity",
                "halted state cannot carry Blocking STI shadow",
            ));
        }
        if matches!(
            profile,
            Profile::LongStiV1 | Profile::LongIretqCpl0ValidV1 | Profile::LongLretqCpl0ValidV1
        ) && self.sti_shadow == Some(StiShadowSnapshot::Blocking)
            && self.rflags.get() & (1 << 9) == 0
        {
            return Err(invalid("state.sti_shadow", "blocking state requires IF=1"));
        }
        if profile == Profile::LongNativeXorpdXmm0Xmm1V1 {
            if self.x87.is_some() {
                return Err(invalid(
                    "state.x87",
                    "x87 state is forbidden by the native XORPD profile",
                ));
            }
            let Some(sse) = &self.sse else {
                return Err(invalid("state.sse", "required by the native XORPD profile"));
            };
            if sse.mxcsr.get() != 0x1f80 {
                return Err(invalid(
                    "state.sse.mxcsr",
                    "must equal 0x00001f80 for native XORPD",
                ));
            }
        }
        let state_profile = match profile {
            Profile::LongX87StateV1
            | Profile::LongX87SseStateV1
            | Profile::LongX87SelectorsStateV1
            | Profile::LongFxsaveRipV1
            | Profile::LongFxsave64RaxV1
            | Profile::LongFxsave64Cr0GatesV1
            | Profile::LongFxrstor64RbxV1
            | Profile::LongFxrstor64Cr0GatesV1
            | Profile::LongMovupsRegV1
            | Profile::LongMovapsRegV1
            | Profile::LongMovapsReverseRegV1
            | Profile::LongMovapdRegV1
            | Profile::LongMovupdRegV1
            | Profile::LongMovupdReverseRegV1
            | Profile::LongMovapdReverseRegV1
            | Profile::LongXorpsRegV1
            | Profile::LongXorpdRegV1
            | Profile::LongXorpdPrerequisitesV1
            | Profile::LongMinssInvalidControlsV1
            | Profile::LongMinssUnmaskedDenormalV1
            | Profile::LongMaxssUnmaskedDenormalV1
            | Profile::LongMinsdUnmaskedDenormalV1
            | Profile::LongMaxsdUnmaskedDenormalV1
            | Profile::LongMinpdUnmaskedDenormalV1
            | Profile::LongMinpsUnmaskedDenormalV1
            | Profile::LongMaxpsUnmaskedDenormalV1
            | Profile::LongMaxpdUnmaskedDenormalV1
            | Profile::LongCvtdq2pdRegExactV1
            | Profile::LongCvtdq2pdRegistersV1
            | Profile::LongCvtdq2psRegistersV1
            | Profile::LongCvtss2sdRegistersV1
            | Profile::LongCvtps2pdRegistersV1
            | Profile::LongCvtsi2sdR32ExactV1
            | Profile::LongCvtsi2sdR64ExactV1
            | Profile::LongCvtsi2sdRegistersV1
            | Profile::LongCvtsi2ssRegistersV1
            | Profile::LongNativeXorpdXmm0Xmm1V1
            | Profile::LongAndpdRegV1
            | Profile::LongOrpdRegV1
            | Profile::LongAndnpdRegV1
            | Profile::LongPxorRegV1
            | Profile::LongPandRegV1
            | Profile::LongPandnRegV1
            | Profile::LongPorRegV1
            | Profile::LongPavgbRegV1
            | Profile::LongPavgwRegV1
            | Profile::LongPmaxubRegV1
            | Profile::LongPminubRegV1
            | Profile::LongPmaxswRegV1
            | Profile::LongPminswRegV1
            | Profile::LongPmullwRegV1
            | Profile::LongPmulhuwRegV1
            | Profile::LongPsadbwRegV1
            | Profile::LongPmovmskbXmmGprV1
            | Profile::LongPmuludqRegV1
            | Profile::LongPmaddwdRegV1
            | Profile::LongPaddbRegV1
            | Profile::LongPaddwRegV1
            | Profile::LongPsubwRegV1
            | Profile::LongPadduswRegV1
            | Profile::LongPsubuswRegV1
            | Profile::LongPsubbRegV1
            | Profile::LongPaddusbRegV1
            | Profile::LongPaddsbRegV1
            | Profile::LongPsubsbRegV1
            | Profile::LongPaddswRegV1
            | Profile::LongPsubswRegV1
            | Profile::LongPadddRegV1
            | Profile::LongPsubdRegV1
            | Profile::LongPaddqRegV1
            | Profile::LongPsubqRegV1
            | Profile::LongPcmpeqbRegV1
            | Profile::LongPcmpeqwRegV1
            | Profile::LongPcmpeqdRegV1
            | Profile::LongPcmpgtbRegV1
            | Profile::LongPcmpgtwRegV1
            | Profile::LongPcmpgtdRegV1
            | Profile::LongPunpcklbwRegV1
            | Profile::LongPunpckhbwRegV1
            | Profile::LongPunpcklwdRegV1
            | Profile::LongPunpckhwdRegV1
            | Profile::LongPunpckldqRegV1
            | Profile::LongPunpckhdqRegV1
            | Profile::LongPunpcklqdqRegV1
            | Profile::LongPunpckhqdqRegV1
            | Profile::LongPacksswbRegV1
            | Profile::LongPackuswbRegV1
            | Profile::LongPackssdwRegV1
            | Profile::LongPshufdRegV1
            | Profile::LongPsllwImmRegV1
            | Profile::LongPslldImmRegV1
            | Profile::LongPsllqImmRegV1
            | Profile::LongPslldqImmRegV1
            | Profile::LongPsrldqImmRegV1
            | Profile::LongPsrlqImmRegV1
            | Profile::LongPsrldImmRegV1
            | Profile::LongPsrlwImmRegV1
            | Profile::LongPsrawImmRegV1
            | Profile::LongPsradImmRegV1
            | Profile::LongPsubusbRegV1
            | Profile::LongAndpsRegV1
            | Profile::LongOrpsRegV1
            | Profile::LongAndnpsRegV1
            | Profile::LongMovsdRegV1
            | Profile::LongMovsdStoreRegV1
            | Profile::LongRcpssRegV1
            | Profile::LongRsqrtpsRegV1
            | Profile::LongRcppsRegV1
            | Profile::LongMinssMaskedRegV1
            | Profile::LongMinsdMaskedRegV1
            | Profile::LongMaxsdMaskedRegV1
            | Profile::LongMinpdMaskedRegV1
            | Profile::LongMaxpdMaskedRegV1
            | Profile::LongMaxssMaskedRegV1
            | Profile::LongMinpsMaskedRegV1
            | Profile::LongMaxpsMaskedRegV1
            | Profile::LongRsqrtssRegV1
            | Profile::LongMovssRegV1
            | Profile::LongMovssMemLoadV1
            | Profile::LongMovsdMemLoadV1
            | Profile::LongMovsdMemStoreV1
            | Profile::LongMovssMemStoreV1
            | Profile::LongMovssStoreRegV1
            | Profile::LongMovupsMemLoadV1
            | Profile::LongMovupdMemLoadV1
            | Profile::LongMovupdMemStoreV1
            | Profile::LongMovupsMemStoreV1
            | Profile::LongMovupsMemBaseV1
            | Profile::LongMovupdMemBaseV1
            | Profile::LongMovupdMemRipV1
            | Profile::LongMovupdMemSibV1
            | Profile::LongSepDisabledV1
            | Profile::LongRdtscV1
            | Profile::LongCliV1 => Profile::LongIntegerV1,
            Profile::LongStiV1 | Profile::LongHltV1 => Profile::LongIntegerV1,
            other => other,
        };
        match (state_profile, self.fs_base) {
            (
                Profile::LongIntegerV1
                | Profile::LongCliV1
                | Profile::LongStiV1
                | Profile::LongHltV1
                | Profile::LongIretqCpl0ValidV1
                | Profile::LongLretqCpl0ValidV1
                | Profile::LongSepDisabledV1
                | Profile::LongRdtscV1
                | Profile::LongX87StateV1
                | Profile::LongX87SseStateV1
                | Profile::LongX87SelectorsStateV1
                | Profile::LongEferRdmsrV1
                | Profile::LongCr0FpInitV1
                | Profile::LongCr4FpInitV1
                | Profile::LongCr0Cr4FpInitFxsave64V1
                | Profile::LongCr0Cr4FpInitFxrstor64V1
                | Profile::LongCr0Cr4FpInitFxrstor64MinssV1
                | Profile::LongCr0Cr4FpInitV1
                | Profile::LongFxsaveRipV1
                | Profile::LongFxsave64RaxV1
                | Profile::LongFxsave64Cr0GatesV1
                | Profile::LongFxrstor64RbxV1
                | Profile::LongFxrstor64Cr0GatesV1
                | Profile::LongMovupsRegV1
                | Profile::LongMovapsRegV1
                | Profile::LongMovapsReverseRegV1
                | Profile::LongMovapdRegV1
                | Profile::LongMovupdRegV1
                | Profile::LongMovupdReverseRegV1
                | Profile::LongMovapdReverseRegV1
                | Profile::LongXorpsRegV1
                | Profile::LongXorpdRegV1
                | Profile::LongXorpdPrerequisitesV1
                | Profile::LongMinssInvalidControlsV1
                | Profile::LongMinssUnmaskedDenormalV1
                | Profile::LongMaxssUnmaskedDenormalV1
                | Profile::LongMinsdUnmaskedDenormalV1
                | Profile::LongMaxsdUnmaskedDenormalV1
                | Profile::LongMinpdUnmaskedDenormalV1
                | Profile::LongMinpsUnmaskedDenormalV1
                | Profile::LongMaxpsUnmaskedDenormalV1
                | Profile::LongMaxpdUnmaskedDenormalV1
                | Profile::LongCvtdq2pdRegExactV1
                | Profile::LongCvtdq2pdRegistersV1
                | Profile::LongCvtdq2psRegistersV1
                | Profile::LongCvtss2sdRegistersV1
                | Profile::LongCvtps2pdRegistersV1
                | Profile::LongComissUcomissRegistersV1
                | Profile::LongComisdUcomisdRegistersV1
                | Profile::LongCvtsi2sdR32ExactV1
                | Profile::LongCvtsi2sdR64ExactV1
                | Profile::LongCvtsi2sdRegistersV1
                | Profile::LongCvtsi2ssRegistersV1
                | Profile::LongNativeXorpdXmm0Xmm1V1
                | Profile::LongAndpdRegV1
                | Profile::LongOrpdRegV1
                | Profile::LongAndnpdRegV1
                | Profile::LongPxorRegV1
                | Profile::LongPandRegV1
                | Profile::LongPandnRegV1
                | Profile::LongPorRegV1
                | Profile::LongPavgbRegV1
                | Profile::LongPavgwRegV1
                | Profile::LongPmaxubRegV1
                | Profile::LongPminubRegV1
                | Profile::LongPmaxswRegV1
                | Profile::LongPminswRegV1
                | Profile::LongPmullwRegV1
                | Profile::LongPmulhuwRegV1
                | Profile::LongPsadbwRegV1
                | Profile::LongPmovmskbXmmGprV1
                | Profile::LongPmuludqRegV1
                | Profile::LongPmaddwdRegV1
                | Profile::LongPaddbRegV1
                | Profile::LongPaddwRegV1
                | Profile::LongPsubwRegV1
                | Profile::LongPadduswRegV1
                | Profile::LongPsubuswRegV1
                | Profile::LongPsubbRegV1
                | Profile::LongPaddusbRegV1
                | Profile::LongPaddsbRegV1
                | Profile::LongPsubsbRegV1
                | Profile::LongPaddswRegV1
                | Profile::LongPsubswRegV1
                | Profile::LongPadddRegV1
                | Profile::LongPsubdRegV1
                | Profile::LongPaddqRegV1
                | Profile::LongPsubqRegV1
                | Profile::LongPcmpeqbRegV1
                | Profile::LongPcmpeqwRegV1
                | Profile::LongPcmpeqdRegV1
                | Profile::LongPcmpgtbRegV1
                | Profile::LongPcmpgtwRegV1
                | Profile::LongPcmpgtdRegV1
                | Profile::LongPunpcklbwRegV1
                | Profile::LongPunpckhbwRegV1
                | Profile::LongPunpcklwdRegV1
                | Profile::LongPunpckhwdRegV1
                | Profile::LongPunpckldqRegV1
                | Profile::LongPunpckhdqRegV1
                | Profile::LongPunpcklqdqRegV1
                | Profile::LongPunpckhqdqRegV1
                | Profile::LongPacksswbRegV1
                | Profile::LongPackuswbRegV1
                | Profile::LongPackssdwRegV1
                | Profile::LongPshufdRegV1
                | Profile::LongPsllwImmRegV1
                | Profile::LongPslldImmRegV1
                | Profile::LongPsllqImmRegV1
                | Profile::LongPslldqImmRegV1
                | Profile::LongPsrldqImmRegV1
                | Profile::LongPsrlqImmRegV1
                | Profile::LongPsrldImmRegV1
                | Profile::LongPsrlwImmRegV1
                | Profile::LongPsrawImmRegV1
                | Profile::LongPsradImmRegV1
                | Profile::LongPsubusbRegV1
                | Profile::LongAndpsRegV1
                | Profile::LongOrpsRegV1
                | Profile::LongAndnpsRegV1
                | Profile::LongMovsdRegV1
                | Profile::LongMovsdStoreRegV1
                | Profile::LongRcpssRegV1
                | Profile::LongRsqrtpsRegV1
                | Profile::LongRcppsRegV1
                | Profile::LongMinssMaskedRegV1
                | Profile::LongMinsdMaskedRegV1
                | Profile::LongMaxsdMaskedRegV1
                | Profile::LongMinpdMaskedRegV1
                | Profile::LongMaxpdMaskedRegV1
                | Profile::LongMaxssMaskedRegV1
                | Profile::LongMinpsMaskedRegV1
                | Profile::LongMaxpsMaskedRegV1
                | Profile::LongRsqrtssRegV1
                | Profile::LongMovssRegV1
                | Profile::LongMovssMemLoadV1
                | Profile::LongMovsdMemLoadV1
                | Profile::LongMovsdMemStoreV1
                | Profile::LongMovssMemStoreV1
                | Profile::LongMovssStoreRegV1
                | Profile::LongMovupsMemLoadV1
                | Profile::LongMovupdMemLoadV1
                | Profile::LongMovupdMemStoreV1
                | Profile::LongMovupsMemStoreV1
                | Profile::LongMovupsMemBaseV1
                | Profile::LongMovupdMemBaseV1
                | Profile::LongMovupdMemRipV1
                | Profile::LongMovupdMemSibV1
                | Profile::LongGsBaseV1
                | Profile::LongGsSelectorV1
                | Profile::LongVerwGdtV1
                | Profile::LongGsLoadV1
                | Profile::LongNullDsEsV1
                | Profile::LongSwapgsV1
                | Profile::LongKernelGsBaseV1,
                None,
            )
            | (Profile::LongFsBaseV1 | Profile::LongFsBaseMsrV1 | Profile::LongFsLoadV1, Some(_)) =>
                {}
            (Profile::LongFsSelectorV1, Some(_)) => {}
            (Profile::LongSepDisabledV1 | Profile::LongRdtscV1, Some(_)) => {
                return Err(invalid("state.fs_base", "not part of this profile"));
            }
            (
                Profile::LongIntegerV1
                | Profile::LongCliV1
                | Profile::LongStiV1
                | Profile::LongHltV1
                | Profile::LongIretqCpl0ValidV1
                | Profile::LongLretqCpl0ValidV1
                | Profile::LongX87StateV1
                | Profile::LongX87SseStateV1
                | Profile::LongX87SelectorsStateV1
                | Profile::LongEferRdmsrV1
                | Profile::LongCr0FpInitV1
                | Profile::LongCr4FpInitV1
                | Profile::LongCr0Cr4FpInitFxsave64V1
                | Profile::LongCr0Cr4FpInitFxrstor64V1
                | Profile::LongCr0Cr4FpInitFxrstor64MinssV1
                | Profile::LongCr0Cr4FpInitV1
                | Profile::LongFxsaveRipV1
                | Profile::LongFxsave64RaxV1
                | Profile::LongFxsave64Cr0GatesV1
                | Profile::LongFxrstor64RbxV1
                | Profile::LongFxrstor64Cr0GatesV1
                | Profile::LongMovupsRegV1
                | Profile::LongMovapsRegV1
                | Profile::LongMovapsReverseRegV1
                | Profile::LongMovapdRegV1
                | Profile::LongMovupdRegV1
                | Profile::LongMovupdReverseRegV1
                | Profile::LongMovapdReverseRegV1
                | Profile::LongXorpsRegV1
                | Profile::LongXorpdRegV1
                | Profile::LongXorpdPrerequisitesV1
                | Profile::LongMinssInvalidControlsV1
                | Profile::LongMinssUnmaskedDenormalV1
                | Profile::LongMaxssUnmaskedDenormalV1
                | Profile::LongMinsdUnmaskedDenormalV1
                | Profile::LongMaxsdUnmaskedDenormalV1
                | Profile::LongMinpdUnmaskedDenormalV1
                | Profile::LongMinpsUnmaskedDenormalV1
                | Profile::LongMaxpsUnmaskedDenormalV1
                | Profile::LongMaxpdUnmaskedDenormalV1
                | Profile::LongCvtdq2pdRegExactV1
                | Profile::LongCvtdq2pdRegistersV1
                | Profile::LongCvtdq2psRegistersV1
                | Profile::LongCvtss2sdRegistersV1
                | Profile::LongCvtps2pdRegistersV1
                | Profile::LongComissUcomissRegistersV1
                | Profile::LongComisdUcomisdRegistersV1
                | Profile::LongCvtsi2sdR32ExactV1
                | Profile::LongCvtsi2sdR64ExactV1
                | Profile::LongCvtsi2sdRegistersV1
                | Profile::LongCvtsi2ssRegistersV1
                | Profile::LongNativeXorpdXmm0Xmm1V1
                | Profile::LongAndpdRegV1
                | Profile::LongOrpdRegV1
                | Profile::LongAndnpdRegV1
                | Profile::LongPxorRegV1
                | Profile::LongPandRegV1
                | Profile::LongPandnRegV1
                | Profile::LongPorRegV1
                | Profile::LongPavgbRegV1
                | Profile::LongPavgwRegV1
                | Profile::LongPmaxubRegV1
                | Profile::LongPminubRegV1
                | Profile::LongPmaxswRegV1
                | Profile::LongPminswRegV1
                | Profile::LongPmullwRegV1
                | Profile::LongPmulhuwRegV1
                | Profile::LongPsadbwRegV1
                | Profile::LongPmovmskbXmmGprV1
                | Profile::LongPmuludqRegV1
                | Profile::LongPmaddwdRegV1
                | Profile::LongPaddbRegV1
                | Profile::LongPaddwRegV1
                | Profile::LongPsubwRegV1
                | Profile::LongPadduswRegV1
                | Profile::LongPsubuswRegV1
                | Profile::LongPsubbRegV1
                | Profile::LongPaddusbRegV1
                | Profile::LongPaddsbRegV1
                | Profile::LongPsubsbRegV1
                | Profile::LongPaddswRegV1
                | Profile::LongPsubswRegV1
                | Profile::LongPadddRegV1
                | Profile::LongPsubdRegV1
                | Profile::LongPaddqRegV1
                | Profile::LongPsubqRegV1
                | Profile::LongPcmpeqbRegV1
                | Profile::LongPcmpeqwRegV1
                | Profile::LongPcmpeqdRegV1
                | Profile::LongPcmpgtbRegV1
                | Profile::LongPcmpgtwRegV1
                | Profile::LongPcmpgtdRegV1
                | Profile::LongPunpcklbwRegV1
                | Profile::LongPunpckhbwRegV1
                | Profile::LongPunpcklwdRegV1
                | Profile::LongPunpckhwdRegV1
                | Profile::LongPunpckldqRegV1
                | Profile::LongPunpckhdqRegV1
                | Profile::LongPunpcklqdqRegV1
                | Profile::LongPunpckhqdqRegV1
                | Profile::LongPacksswbRegV1
                | Profile::LongPackuswbRegV1
                | Profile::LongPackssdwRegV1
                | Profile::LongPshufdRegV1
                | Profile::LongPsllwImmRegV1
                | Profile::LongPslldImmRegV1
                | Profile::LongPsllqImmRegV1
                | Profile::LongPslldqImmRegV1
                | Profile::LongPsrldqImmRegV1
                | Profile::LongPsrlqImmRegV1
                | Profile::LongPsrldImmRegV1
                | Profile::LongPsrlwImmRegV1
                | Profile::LongPsrawImmRegV1
                | Profile::LongPsradImmRegV1
                | Profile::LongPsubusbRegV1
                | Profile::LongAndpsRegV1
                | Profile::LongOrpsRegV1
                | Profile::LongAndnpsRegV1
                | Profile::LongMovsdRegV1
                | Profile::LongMovsdStoreRegV1
                | Profile::LongRcpssRegV1
                | Profile::LongRsqrtpsRegV1
                | Profile::LongRcppsRegV1
                | Profile::LongMinssMaskedRegV1
                | Profile::LongMinsdMaskedRegV1
                | Profile::LongMaxsdMaskedRegV1
                | Profile::LongMinpdMaskedRegV1
                | Profile::LongMaxpdMaskedRegV1
                | Profile::LongMaxssMaskedRegV1
                | Profile::LongMinpsMaskedRegV1
                | Profile::LongMaxpsMaskedRegV1
                | Profile::LongRsqrtssRegV1
                | Profile::LongMovssRegV1
                | Profile::LongMovssMemLoadV1
                | Profile::LongMovsdMemLoadV1
                | Profile::LongMovsdMemStoreV1
                | Profile::LongMovssMemStoreV1
                | Profile::LongMovssStoreRegV1
                | Profile::LongMovupsMemLoadV1
                | Profile::LongMovupdMemLoadV1
                | Profile::LongMovupdMemStoreV1
                | Profile::LongMovupsMemStoreV1
                | Profile::LongMovupsMemBaseV1
                | Profile::LongMovupdMemBaseV1
                | Profile::LongMovupdMemRipV1
                | Profile::LongMovupdMemSibV1
                | Profile::LongGsBaseV1
                | Profile::LongGsSelectorV1
                | Profile::LongVerwGdtV1
                | Profile::LongGsLoadV1
                | Profile::LongNullDsEsV1
                | Profile::LongSwapgsV1
                | Profile::LongKernelGsBaseV1,
                Some(_),
            ) => {
                return Err(invalid("state.fs_base", "not part of this profile"));
            }
            (
                Profile::LongFsBaseV1
                | Profile::LongFsBaseMsrV1
                | Profile::LongFsSelectorV1
                | Profile::LongFsLoadV1,
                None,
            ) => {
                return Err(invalid("state.fs_base", "required by this profile"));
            }
        }
        match (state_profile, self.gs_base) {
            (
                Profile::LongIntegerV1
                | Profile::LongCliV1
                | Profile::LongStiV1
                | Profile::LongHltV1
                | Profile::LongIretqCpl0ValidV1
                | Profile::LongLretqCpl0ValidV1
                | Profile::LongSepDisabledV1
                | Profile::LongRdtscV1
                | Profile::LongX87StateV1
                | Profile::LongX87SseStateV1
                | Profile::LongX87SelectorsStateV1
                | Profile::LongEferRdmsrV1
                | Profile::LongCr0FpInitV1
                | Profile::LongCr4FpInitV1
                | Profile::LongCr0Cr4FpInitFxsave64V1
                | Profile::LongCr0Cr4FpInitFxrstor64V1
                | Profile::LongCr0Cr4FpInitFxrstor64MinssV1
                | Profile::LongCr0Cr4FpInitV1
                | Profile::LongFxsaveRipV1
                | Profile::LongFxsave64RaxV1
                | Profile::LongFxsave64Cr0GatesV1
                | Profile::LongFxrstor64RbxV1
                | Profile::LongFxrstor64Cr0GatesV1
                | Profile::LongMovupsRegV1
                | Profile::LongMovapsRegV1
                | Profile::LongMovapsReverseRegV1
                | Profile::LongMovapdRegV1
                | Profile::LongMovupdRegV1
                | Profile::LongMovupdReverseRegV1
                | Profile::LongMovapdReverseRegV1
                | Profile::LongXorpsRegV1
                | Profile::LongXorpdRegV1
                | Profile::LongXorpdPrerequisitesV1
                | Profile::LongMinssInvalidControlsV1
                | Profile::LongMinssUnmaskedDenormalV1
                | Profile::LongMaxssUnmaskedDenormalV1
                | Profile::LongMinsdUnmaskedDenormalV1
                | Profile::LongMaxsdUnmaskedDenormalV1
                | Profile::LongMinpdUnmaskedDenormalV1
                | Profile::LongMinpsUnmaskedDenormalV1
                | Profile::LongMaxpsUnmaskedDenormalV1
                | Profile::LongMaxpdUnmaskedDenormalV1
                | Profile::LongCvtdq2pdRegExactV1
                | Profile::LongCvtdq2pdRegistersV1
                | Profile::LongCvtdq2psRegistersV1
                | Profile::LongCvtss2sdRegistersV1
                | Profile::LongCvtps2pdRegistersV1
                | Profile::LongComissUcomissRegistersV1
                | Profile::LongComisdUcomisdRegistersV1
                | Profile::LongCvtsi2sdR32ExactV1
                | Profile::LongCvtsi2sdR64ExactV1
                | Profile::LongCvtsi2sdRegistersV1
                | Profile::LongCvtsi2ssRegistersV1
                | Profile::LongNativeXorpdXmm0Xmm1V1
                | Profile::LongAndpdRegV1
                | Profile::LongOrpdRegV1
                | Profile::LongAndnpdRegV1
                | Profile::LongPxorRegV1
                | Profile::LongPandRegV1
                | Profile::LongPandnRegV1
                | Profile::LongPorRegV1
                | Profile::LongPavgbRegV1
                | Profile::LongPavgwRegV1
                | Profile::LongPmaxubRegV1
                | Profile::LongPminubRegV1
                | Profile::LongPmaxswRegV1
                | Profile::LongPminswRegV1
                | Profile::LongPmullwRegV1
                | Profile::LongPmulhuwRegV1
                | Profile::LongPsadbwRegV1
                | Profile::LongPmovmskbXmmGprV1
                | Profile::LongPmuludqRegV1
                | Profile::LongPmaddwdRegV1
                | Profile::LongPaddbRegV1
                | Profile::LongPaddwRegV1
                | Profile::LongPsubwRegV1
                | Profile::LongPadduswRegV1
                | Profile::LongPsubuswRegV1
                | Profile::LongPsubbRegV1
                | Profile::LongPaddusbRegV1
                | Profile::LongPaddsbRegV1
                | Profile::LongPsubsbRegV1
                | Profile::LongPaddswRegV1
                | Profile::LongPsubswRegV1
                | Profile::LongPadddRegV1
                | Profile::LongPsubdRegV1
                | Profile::LongPaddqRegV1
                | Profile::LongPsubqRegV1
                | Profile::LongPcmpeqbRegV1
                | Profile::LongPcmpeqwRegV1
                | Profile::LongPcmpeqdRegV1
                | Profile::LongPcmpgtbRegV1
                | Profile::LongPcmpgtwRegV1
                | Profile::LongPcmpgtdRegV1
                | Profile::LongPunpcklbwRegV1
                | Profile::LongPunpckhbwRegV1
                | Profile::LongPunpcklwdRegV1
                | Profile::LongPunpckhwdRegV1
                | Profile::LongPunpckldqRegV1
                | Profile::LongPunpckhdqRegV1
                | Profile::LongPunpcklqdqRegV1
                | Profile::LongPunpckhqdqRegV1
                | Profile::LongPacksswbRegV1
                | Profile::LongPackuswbRegV1
                | Profile::LongPackssdwRegV1
                | Profile::LongPshufdRegV1
                | Profile::LongPsllwImmRegV1
                | Profile::LongPslldImmRegV1
                | Profile::LongPsllqImmRegV1
                | Profile::LongPslldqImmRegV1
                | Profile::LongPsrldqImmRegV1
                | Profile::LongPsrlqImmRegV1
                | Profile::LongPsrldImmRegV1
                | Profile::LongPsrlwImmRegV1
                | Profile::LongPsrawImmRegV1
                | Profile::LongPsradImmRegV1
                | Profile::LongPsubusbRegV1
                | Profile::LongAndpsRegV1
                | Profile::LongOrpsRegV1
                | Profile::LongAndnpsRegV1
                | Profile::LongMovsdRegV1
                | Profile::LongMovsdStoreRegV1
                | Profile::LongRcpssRegV1
                | Profile::LongRsqrtpsRegV1
                | Profile::LongRcppsRegV1
                | Profile::LongMinssMaskedRegV1
                | Profile::LongMinsdMaskedRegV1
                | Profile::LongMaxsdMaskedRegV1
                | Profile::LongMinpdMaskedRegV1
                | Profile::LongMaxpdMaskedRegV1
                | Profile::LongMaxssMaskedRegV1
                | Profile::LongMinpsMaskedRegV1
                | Profile::LongMaxpsMaskedRegV1
                | Profile::LongRsqrtssRegV1
                | Profile::LongMovssRegV1
                | Profile::LongMovssMemLoadV1
                | Profile::LongMovsdMemLoadV1
                | Profile::LongMovsdMemStoreV1
                | Profile::LongMovssMemStoreV1
                | Profile::LongMovssStoreRegV1
                | Profile::LongMovupsMemLoadV1
                | Profile::LongMovupdMemLoadV1
                | Profile::LongMovupdMemStoreV1
                | Profile::LongMovupsMemStoreV1
                | Profile::LongMovupsMemBaseV1
                | Profile::LongMovupdMemBaseV1
                | Profile::LongMovupdMemRipV1
                | Profile::LongMovupdMemSibV1
                | Profile::LongFsBaseV1
                | Profile::LongFsBaseMsrV1
                | Profile::LongFsSelectorV1
                | Profile::LongVerwGdtV1
                | Profile::LongFsLoadV1
                | Profile::LongNullDsEsV1,
                None,
            )
            | (
                Profile::LongGsBaseV1
                | Profile::LongGsSelectorV1
                | Profile::LongGsLoadV1
                | Profile::LongSwapgsV1
                | Profile::LongKernelGsBaseV1,
                Some(_),
            ) => {}
            (_, Some(_)) => return Err(invalid("state.gs_base", "not part of this profile")),
            (
                Profile::LongGsBaseV1
                | Profile::LongGsSelectorV1
                | Profile::LongGsLoadV1
                | Profile::LongSwapgsV1
                | Profile::LongKernelGsBaseV1,
                None,
            ) => {
                return Err(invalid("state.gs_base", "required by this profile"));
            }
        }
        match (state_profile, self.fs_selector) {
            (
                Profile::LongIntegerV1
                | Profile::LongCliV1
                | Profile::LongStiV1
                | Profile::LongHltV1
                | Profile::LongIretqCpl0ValidV1
                | Profile::LongLretqCpl0ValidV1
                | Profile::LongSepDisabledV1
                | Profile::LongRdtscV1
                | Profile::LongX87StateV1
                | Profile::LongX87SseStateV1
                | Profile::LongX87SelectorsStateV1
                | Profile::LongEferRdmsrV1
                | Profile::LongCr0FpInitV1
                | Profile::LongCr4FpInitV1
                | Profile::LongCr0Cr4FpInitFxsave64V1
                | Profile::LongCr0Cr4FpInitFxrstor64V1
                | Profile::LongCr0Cr4FpInitFxrstor64MinssV1
                | Profile::LongCr0Cr4FpInitV1
                | Profile::LongFxsaveRipV1
                | Profile::LongFxsave64RaxV1
                | Profile::LongFxsave64Cr0GatesV1
                | Profile::LongFxrstor64RbxV1
                | Profile::LongFxrstor64Cr0GatesV1
                | Profile::LongMovupsRegV1
                | Profile::LongMovapsRegV1
                | Profile::LongMovapsReverseRegV1
                | Profile::LongMovapdRegV1
                | Profile::LongMovupdRegV1
                | Profile::LongMovupdReverseRegV1
                | Profile::LongMovapdReverseRegV1
                | Profile::LongXorpsRegV1
                | Profile::LongXorpdRegV1
                | Profile::LongXorpdPrerequisitesV1
                | Profile::LongMinssInvalidControlsV1
                | Profile::LongMinssUnmaskedDenormalV1
                | Profile::LongMaxssUnmaskedDenormalV1
                | Profile::LongMinsdUnmaskedDenormalV1
                | Profile::LongMaxsdUnmaskedDenormalV1
                | Profile::LongMinpdUnmaskedDenormalV1
                | Profile::LongMinpsUnmaskedDenormalV1
                | Profile::LongMaxpsUnmaskedDenormalV1
                | Profile::LongMaxpdUnmaskedDenormalV1
                | Profile::LongCvtdq2pdRegExactV1
                | Profile::LongCvtdq2pdRegistersV1
                | Profile::LongCvtdq2psRegistersV1
                | Profile::LongCvtss2sdRegistersV1
                | Profile::LongCvtps2pdRegistersV1
                | Profile::LongComissUcomissRegistersV1
                | Profile::LongComisdUcomisdRegistersV1
                | Profile::LongCvtsi2sdR32ExactV1
                | Profile::LongCvtsi2sdR64ExactV1
                | Profile::LongCvtsi2sdRegistersV1
                | Profile::LongCvtsi2ssRegistersV1
                | Profile::LongNativeXorpdXmm0Xmm1V1
                | Profile::LongAndpdRegV1
                | Profile::LongOrpdRegV1
                | Profile::LongAndnpdRegV1
                | Profile::LongPxorRegV1
                | Profile::LongPandRegV1
                | Profile::LongPandnRegV1
                | Profile::LongPorRegV1
                | Profile::LongPavgbRegV1
                | Profile::LongPavgwRegV1
                | Profile::LongPmaxubRegV1
                | Profile::LongPminubRegV1
                | Profile::LongPmaxswRegV1
                | Profile::LongPminswRegV1
                | Profile::LongPmullwRegV1
                | Profile::LongPmulhuwRegV1
                | Profile::LongPsadbwRegV1
                | Profile::LongPmovmskbXmmGprV1
                | Profile::LongPmuludqRegV1
                | Profile::LongPmaddwdRegV1
                | Profile::LongPaddbRegV1
                | Profile::LongPaddwRegV1
                | Profile::LongPsubwRegV1
                | Profile::LongPadduswRegV1
                | Profile::LongPsubuswRegV1
                | Profile::LongPsubbRegV1
                | Profile::LongPaddusbRegV1
                | Profile::LongPaddsbRegV1
                | Profile::LongPsubsbRegV1
                | Profile::LongPaddswRegV1
                | Profile::LongPsubswRegV1
                | Profile::LongPadddRegV1
                | Profile::LongPsubdRegV1
                | Profile::LongPaddqRegV1
                | Profile::LongPsubqRegV1
                | Profile::LongPcmpeqbRegV1
                | Profile::LongPcmpeqwRegV1
                | Profile::LongPcmpeqdRegV1
                | Profile::LongPcmpgtbRegV1
                | Profile::LongPcmpgtwRegV1
                | Profile::LongPcmpgtdRegV1
                | Profile::LongPunpcklbwRegV1
                | Profile::LongPunpckhbwRegV1
                | Profile::LongPunpcklwdRegV1
                | Profile::LongPunpckhwdRegV1
                | Profile::LongPunpckldqRegV1
                | Profile::LongPunpckhdqRegV1
                | Profile::LongPunpcklqdqRegV1
                | Profile::LongPunpckhqdqRegV1
                | Profile::LongPacksswbRegV1
                | Profile::LongPackuswbRegV1
                | Profile::LongPackssdwRegV1
                | Profile::LongPshufdRegV1
                | Profile::LongPsllwImmRegV1
                | Profile::LongPslldImmRegV1
                | Profile::LongPsllqImmRegV1
                | Profile::LongPslldqImmRegV1
                | Profile::LongPsrldqImmRegV1
                | Profile::LongPsrlqImmRegV1
                | Profile::LongPsrldImmRegV1
                | Profile::LongPsrlwImmRegV1
                | Profile::LongPsrawImmRegV1
                | Profile::LongPsradImmRegV1
                | Profile::LongPsubusbRegV1
                | Profile::LongAndpsRegV1
                | Profile::LongOrpsRegV1
                | Profile::LongAndnpsRegV1
                | Profile::LongMovsdRegV1
                | Profile::LongMovsdStoreRegV1
                | Profile::LongRcpssRegV1
                | Profile::LongRsqrtpsRegV1
                | Profile::LongRcppsRegV1
                | Profile::LongMinssMaskedRegV1
                | Profile::LongMinsdMaskedRegV1
                | Profile::LongMaxsdMaskedRegV1
                | Profile::LongMinpdMaskedRegV1
                | Profile::LongMaxpdMaskedRegV1
                | Profile::LongMaxssMaskedRegV1
                | Profile::LongMinpsMaskedRegV1
                | Profile::LongMaxpsMaskedRegV1
                | Profile::LongRsqrtssRegV1
                | Profile::LongMovssRegV1
                | Profile::LongMovssMemLoadV1
                | Profile::LongMovsdMemLoadV1
                | Profile::LongMovsdMemStoreV1
                | Profile::LongMovssMemStoreV1
                | Profile::LongMovssStoreRegV1
                | Profile::LongMovupsMemLoadV1
                | Profile::LongMovupdMemLoadV1
                | Profile::LongMovupdMemStoreV1
                | Profile::LongMovupsMemStoreV1
                | Profile::LongMovupsMemBaseV1
                | Profile::LongMovupdMemBaseV1
                | Profile::LongMovupdMemRipV1
                | Profile::LongMovupdMemSibV1
                | Profile::LongFsBaseV1
                | Profile::LongFsBaseMsrV1
                | Profile::LongGsBaseV1
                | Profile::LongVerwGdtV1
                | Profile::LongGsLoadV1
                | Profile::LongNullDsEsV1
                | Profile::LongSwapgsV1
                | Profile::LongKernelGsBaseV1,
                None,
            )
            | (Profile::LongFsSelectorV1 | Profile::LongFsLoadV1, Some(_)) => {}
            (Profile::LongGsSelectorV1, None) => {}
            (_, Some(_)) => {
                return Err(invalid("state.fs_selector", "not part of this profile"));
            }
            (Profile::LongFsSelectorV1 | Profile::LongFsLoadV1, None) => {
                return Err(invalid("state.fs_selector", "required by this profile"));
            }
        }
        if self
            .fs_selector
            .is_some_and(|selector| selector.get() > u16::MAX as u64)
        {
            return Err(invalid("state.fs_selector", "must fit in 16 bits"));
        }
        match (state_profile, self.gs_selector) {
            (Profile::LongGsSelectorV1 | Profile::LongGsLoadV1, Some(_)) => {}
            (Profile::LongGsSelectorV1 | Profile::LongGsLoadV1, None) => {
                return Err(invalid("state.gs_selector", "required by this profile"));
            }
            (_, None) => {}
            (_, Some(_)) => return Err(invalid("state.gs_selector", "not part of this profile")),
        }
        if self
            .gs_selector
            .is_some_and(|selector| selector.get() > u16::MAX as u64)
        {
            return Err(invalid("state.gs_selector", "must fit in 16 bits"));
        }
        match (
            state_profile,
            self.ds_selector,
            self.es_selector,
            self.ds_unusable,
            self.es_unusable,
        ) {
            (Profile::LongNullDsEsV1, Some(ds), Some(es), Some(_), Some(_)) => {
                if ds.get() > u16::MAX as u64 {
                    return Err(invalid("state.ds_selector", "must fit in 16 bits"));
                }
                if es.get() > u16::MAX as u64 {
                    return Err(invalid("state.es_selector", "must fit in 16 bits"));
                }
            }
            (Profile::LongNullDsEsV1, _, _, _, _) => {
                return Err(invalid(
                    "state.ds_es",
                    "selectors and unusable state are required by this profile",
                ));
            }
            (_, None, None, None, None) => {}
            _ => return Err(invalid("state.ds_es", "not part of this profile")),
        }
        let cs_fields = [
            self.cs_selector,
            self.cs_cache_base,
            self.cs_cache_limit,
            self.cs_cache_access,
            self.cs_cache_flags,
        ];
        let ss_fields = [
            self.ss_selector,
            self.ss_cache_base,
            self.ss_cache_limit,
            self.ss_cache_access,
            self.ss_cache_flags,
        ];
        if !matches!(
            state_profile,
            Profile::LongIretqCpl0ValidV1 | Profile::LongLretqCpl0ValidV1
        ) {
            if cs_fields.iter().any(Option::is_some) || ss_fields.iter().any(Option::is_some) {
                return Err(invalid(
                    "state.cs_ss",
                    "CS/SS state is only part of the IRETQ/LRETQ profiles",
                ));
            }
        } else {
            if cs_fields.iter().any(Option::is_none) || ss_fields.iter().any(Option::is_none) {
                return Err(invalid(
                    "state.cs_ss",
                    "selectors and complete hidden caches are required by this profile",
                ));
            }
            if self.cs_selector.is_some_and(|v| v.get() > u16::MAX as u64)
                || self.ss_selector.is_some_and(|v| v.get() > u16::MAX as u64)
                || self
                    .cs_cache_limit
                    .is_some_and(|v| v.get() > u32::MAX as u64)
                || self
                    .cs_cache_access
                    .is_some_and(|v| v.get() > u8::MAX as u64)
                || self.cs_cache_flags.is_some_and(|v| v.get() > 0x0f)
                || self
                    .ss_cache_base
                    .is_some_and(|v| v.get() > u32::MAX as u64)
                || self
                    .ss_cache_limit
                    .is_some_and(|v| v.get() > u32::MAX as u64)
                || self
                    .ss_cache_access
                    .is_some_and(|v| v.get() > u8::MAX as u64)
                || self.ss_cache_flags.is_some_and(|v| v.get() > 0x0f)
            {
                return Err(invalid(
                    "state.cs_ss",
                    "selector/cache field exceeds its architectural width",
                ));
            }
        }
        match (state_profile, self.gdtr_base, self.gdtr_limit) {
            (
                Profile::LongVerwGdtV1
                | Profile::LongFsLoadV1
                | Profile::LongGsLoadV1
                | Profile::LongIretqCpl0ValidV1
                | Profile::LongLretqCpl0ValidV1,
                Some(_),
                Some(_),
            ) => {}
            (
                Profile::LongVerwGdtV1
                | Profile::LongFsLoadV1
                | Profile::LongGsLoadV1
                | Profile::LongIretqCpl0ValidV1
                | Profile::LongLretqCpl0ValidV1,
                _,
                _,
            ) => {
                return Err(invalid(
                    "state.gdtr",
                    "base and limit are required by this profile",
                ));
            }
            (_, None, None) => {}
            (_, _, _) => return Err(invalid("state.gdtr", "not part of this profile")),
        }
        if self
            .gdtr_limit
            .is_some_and(|limit| limit.get() > u16::MAX as u64)
        {
            return Err(invalid("state.gdtr_limit", "must fit in 16 bits"));
        }
        if let (Some(base), Some(limit)) = (self.gdtr_base, self.gdtr_limit) {
            let base = base.get();
            let upper = base >> 47;
            if upper != 0 && upper != 0x1ffff {
                return Err(invalid("state.gdtr_base", "must be canonical48"));
            }
            let last = base
                .checked_add(limit.get())
                .ok_or_else(|| invalid("state.gdtr", "base plus limit overflows"))?;
            let upper = last >> 47;
            if upper != 0 && upper != 0x1ffff {
                return Err(invalid(
                    "state.gdtr",
                    "entire table range must be canonical48",
                ));
            }
        }
        if let Some(base) = self.fs_base.map(HexU64::get) {
            let upper = base >> 47;
            if upper != 0 && upper != 0x1ffff {
                return Err(invalid("state.fs_base", "must be canonical48"));
            }
        }
        if let Some(base) = self.gs_base.map(HexU64::get) {
            let upper = base >> 47;
            if upper != 0 && upper != 0x1ffff {
                return Err(invalid("state.gs_base", "must be canonical48"));
            }
        }
        match (state_profile, self.kernel_gs_base) {
            (Profile::LongSwapgsV1 | Profile::LongKernelGsBaseV1, Some(base)) => {
                let upper = base.get() >> 47;
                if upper != 0 && upper != 0x1ffff {
                    return Err(invalid("state.kernel_gs_base", "must be canonical48"));
                }
            }
            (Profile::LongSwapgsV1 | Profile::LongKernelGsBaseV1, None) => {
                return Err(invalid("state.kernel_gs_base", "required by this profile"));
            }
            (_, None) => {}
            (_, Some(_)) => {
                return Err(invalid("state.kernel_gs_base", "not part of this profile"));
            }
        }
        match (
            state_profile,
            self.fs_cache_limit,
            self.fs_cache_access,
            self.fs_cache_flags,
        ) {
            (Profile::LongFsLoadV1, Some(limit), Some(access), Some(flags)) => {
                if limit.get() > u32::MAX as u64 {
                    return Err(invalid("state.fs_cache_limit", "must fit in 32 bits"));
                }
                if access.get() > u8::MAX as u64 {
                    return Err(invalid("state.fs_cache_access", "must fit in 8 bits"));
                }
                if flags.get() > 0x0f {
                    return Err(invalid("state.fs_cache_flags", "must fit in 4 bits"));
                }
            }
            (Profile::LongFsLoadV1, _, _, _) => {
                return Err(invalid(
                    "state.fs_cache",
                    "limit, access and flags are required by this profile",
                ));
            }
            (_, None, None, None) => {}
            _ => return Err(invalid("state.fs_cache", "not part of this profile")),
        }
        match (
            state_profile,
            self.gs_cache_limit,
            self.gs_cache_access,
            self.gs_cache_flags,
        ) {
            (Profile::LongGsLoadV1, Some(limit), Some(access), Some(flags)) => {
                if limit.get() > u32::MAX as u64 {
                    return Err(invalid("state.gs_cache_limit", "must fit in 32 bits"));
                }
                if access.get() > u8::MAX as u64 {
                    return Err(invalid("state.gs_cache_access", "must fit in 8 bits"));
                }
                if flags.get() > 0x0f {
                    return Err(invalid("state.gs_cache_flags", "must fit in 4 bits"));
                }
            }
            (Profile::LongGsLoadV1, _, _, _) => {
                return Err(invalid(
                    "state.gs_cache",
                    "limit, access and flags are required by this profile",
                ));
            }
            (_, None, None, None) => {}
            _ => return Err(invalid("state.gs_cache", "not part of this profile")),
        }
        let system_flags = if matches!(
            profile,
            Profile::LongCliV1
                | Profile::LongStiV1
                | Profile::LongHltV1
                | Profile::LongIretqCpl0ValidV1
                | Profile::LongLretqCpl0ValidV1
                | Profile::LongCr0FpInitV1
                | Profile::LongCr4FpInitV1
                | Profile::LongCr0Cr4FpInitFxsave64V1
                | Profile::LongCr0Cr4FpInitFxrstor64V1
                | Profile::LongCr0Cr4FpInitFxrstor64MinssV1
                | Profile::LongCr0Cr4FpInitV1,
        ) {
            (1 << 9) | (3 << 12)
        } else {
            0
        };
        if self.rflags.get() & !(INTEGER_FLAGS | system_flags) != 0 || self.rflags.get() & 2 == 0 {
            return Err(invalid(
                "state.rflags",
                "outside the profile's admitted RFLAGS bits or missing reserved bit 1",
            ));
        }
        match (profile, self.cr0) {
            (Profile::LongEferRdmsrV1 | Profile::LongCr0FpInitV1, Some(value))
            | (
                Profile::LongCr4FpInitV1
                | Profile::LongCr0Cr4FpInitFxsave64V1
                | Profile::LongCr0Cr4FpInitFxrstor64V1
                | Profile::LongCr0Cr4FpInitFxrstor64MinssV1
                | Profile::LongCr0Cr4FpInitV1,
                Some(value),
            ) if [
                od_isa::execute::Cr0Profile::BASE,
                od_isa::execute::Cr0Profile::BASE | (1 << 2),
                od_isa::execute::Cr0Profile::BASE | (1 << 3),
                od_isa::execute::Cr0Profile::BASE | (1 << 2) | (1 << 3),
            ]
            .contains(&value.get()) => {}
            (
                Profile::LongCr0FpInitV1
                | Profile::LongEferRdmsrV1
                | Profile::LongCr4FpInitV1
                | Profile::LongCr0Cr4FpInitFxsave64V1
                | Profile::LongCr0Cr4FpInitFxrstor64V1
                | Profile::LongCr0Cr4FpInitFxrstor64MinssV1
                | Profile::LongCr0Cr4FpInitV1,
                Some(_),
            ) => {
                return Err(invalid(
                    "state.cr0",
                    "must equal the fixed profile base with only EM and TS varying",
                ));
            }
            (
                Profile::LongCr0FpInitV1
                | Profile::LongEferRdmsrV1
                | Profile::LongCr4FpInitV1
                | Profile::LongCr0Cr4FpInitFxsave64V1
                | Profile::LongCr0Cr4FpInitFxrstor64V1
                | Profile::LongCr0Cr4FpInitFxrstor64MinssV1
                | Profile::LongCr0Cr4FpInitV1,
                None,
            ) => {
                return Err(invalid("state.cr0", "required by this profile"));
            }
            (_, Some(_)) => return Err(invalid("state.cr0", "not part of this profile")),
            (_, None) => {}
        }
        match (profile, self.cr4) {
            (
                Profile::LongCr4FpInitV1
                | Profile::LongEferRdmsrV1
                | Profile::LongCr0Cr4FpInitFxsave64V1
                | Profile::LongCr0Cr4FpInitFxrstor64V1
                | Profile::LongCr0Cr4FpInitFxrstor64MinssV1
                | Profile::LongCr0Cr4FpInitV1,
                Some(value),
            ) if [
                od_isa::execute::Cr4Profile::BASE,
                od_isa::execute::Cr4Profile::BASE | (1 << 9),
                od_isa::execute::Cr4Profile::BASE | (1 << 10),
                od_isa::execute::Cr4Profile::BASE | (1 << 9) | (1 << 10),
            ]
            .contains(&value.get()) => {}
            (
                Profile::LongCr4FpInitV1
                | Profile::LongEferRdmsrV1
                | Profile::LongCr0Cr4FpInitFxsave64V1
                | Profile::LongCr0Cr4FpInitFxrstor64V1
                | Profile::LongCr0Cr4FpInitFxrstor64MinssV1
                | Profile::LongCr0Cr4FpInitV1,
                Some(_),
            ) => {
                return Err(invalid(
                    "state.cr4",
                    "must equal the fixed PAE base with only OSFXSR and OSXMMEXCPT varying",
                ));
            }
            (
                Profile::LongCr4FpInitV1
                | Profile::LongEferRdmsrV1
                | Profile::LongCr0Cr4FpInitFxsave64V1
                | Profile::LongCr0Cr4FpInitFxrstor64V1
                | Profile::LongCr0Cr4FpInitFxrstor64MinssV1
                | Profile::LongCr0Cr4FpInitV1,
                None,
            ) => {
                return Err(invalid("state.cr4", "required by this profile"));
            }
            (_, Some(_)) => return Err(invalid("state.cr4", "not part of this profile")),
            (_, None) => {}
        }
        match (profile, &self.x87) {
            (Profile::LongX87StateV1, Some(x87)) => x87.validate()?,
            (
                Profile::LongCr0FpInitV1
                | Profile::LongCr4FpInitV1
                | Profile::LongCr0Cr4FpInitFxsave64V1
                | Profile::LongCr0Cr4FpInitFxrstor64V1
                | Profile::LongCr0Cr4FpInitFxrstor64MinssV1
                | Profile::LongCr0Cr4FpInitV1,
                Some(x87),
            ) => {
                x87.validate()?;
                if x87.instruction_selector.is_some() || x87.data_selector.is_some() {
                    return Err(invalid(
                        "state.x87",
                        "CR0 FP-init profile requires unavailable FCS/FDS selectors",
                    ));
                }
            }
            (
                Profile::LongX87SseStateV1
                | Profile::LongX87SelectorsStateV1
                | Profile::LongFxsaveRipV1
                | Profile::LongFxsave64RaxV1
                | Profile::LongFxsave64Cr0GatesV1
                | Profile::LongFxrstor64RbxV1
                | Profile::LongFxrstor64Cr0GatesV1
                | Profile::LongMovupsRegV1
                | Profile::LongMovapsRegV1
                | Profile::LongMovapsReverseRegV1
                | Profile::LongMovapdRegV1
                | Profile::LongMovupdRegV1
                | Profile::LongMovupdReverseRegV1
                | Profile::LongMovapdReverseRegV1
                | Profile::LongXorpsRegV1
                | Profile::LongXorpdRegV1
                | Profile::LongXorpdPrerequisitesV1
                | Profile::LongMinssInvalidControlsV1
                | Profile::LongMinssUnmaskedDenormalV1
                | Profile::LongMaxssUnmaskedDenormalV1
                | Profile::LongMinsdUnmaskedDenormalV1
                | Profile::LongMaxsdUnmaskedDenormalV1
                | Profile::LongMinpdUnmaskedDenormalV1
                | Profile::LongMinpsUnmaskedDenormalV1
                | Profile::LongMaxpsUnmaskedDenormalV1
                | Profile::LongMaxpdUnmaskedDenormalV1
                | Profile::LongAndpdRegV1
                | Profile::LongOrpdRegV1
                | Profile::LongAndnpdRegV1
                | Profile::LongPxorRegV1
                | Profile::LongPandRegV1
                | Profile::LongPandnRegV1
                | Profile::LongPorRegV1
                | Profile::LongPavgbRegV1
                | Profile::LongPavgwRegV1
                | Profile::LongPmaxubRegV1
                | Profile::LongPminubRegV1
                | Profile::LongPmaxswRegV1
                | Profile::LongPminswRegV1
                | Profile::LongPmullwRegV1
                | Profile::LongPmulhuwRegV1
                | Profile::LongPsadbwRegV1
                | Profile::LongPmovmskbXmmGprV1
                | Profile::LongPmuludqRegV1
                | Profile::LongPmaddwdRegV1
                | Profile::LongPaddbRegV1
                | Profile::LongPaddwRegV1
                | Profile::LongPsubwRegV1
                | Profile::LongPadduswRegV1
                | Profile::LongPsubuswRegV1
                | Profile::LongPsubbRegV1
                | Profile::LongPaddusbRegV1
                | Profile::LongPaddsbRegV1
                | Profile::LongPsubsbRegV1
                | Profile::LongPaddswRegV1
                | Profile::LongPsubswRegV1
                | Profile::LongPadddRegV1
                | Profile::LongPsubdRegV1
                | Profile::LongPaddqRegV1
                | Profile::LongPsubqRegV1
                | Profile::LongPcmpeqbRegV1
                | Profile::LongPcmpeqwRegV1
                | Profile::LongPcmpeqdRegV1
                | Profile::LongPcmpgtbRegV1
                | Profile::LongPcmpgtwRegV1
                | Profile::LongPcmpgtdRegV1
                | Profile::LongPunpcklbwRegV1
                | Profile::LongPunpckhbwRegV1
                | Profile::LongPunpcklwdRegV1
                | Profile::LongPunpckhwdRegV1
                | Profile::LongPunpckldqRegV1
                | Profile::LongPunpckhdqRegV1
                | Profile::LongPunpcklqdqRegV1
                | Profile::LongPunpckhqdqRegV1
                | Profile::LongPacksswbRegV1
                | Profile::LongPackuswbRegV1
                | Profile::LongPackssdwRegV1
                | Profile::LongPshufdRegV1
                | Profile::LongPsllwImmRegV1
                | Profile::LongPslldImmRegV1
                | Profile::LongPsllqImmRegV1
                | Profile::LongPslldqImmRegV1
                | Profile::LongPsrldqImmRegV1
                | Profile::LongPsrlqImmRegV1
                | Profile::LongPsrldImmRegV1
                | Profile::LongPsrlwImmRegV1
                | Profile::LongPsrawImmRegV1
                | Profile::LongPsradImmRegV1
                | Profile::LongPsubusbRegV1
                | Profile::LongAndpsRegV1
                | Profile::LongOrpsRegV1
                | Profile::LongAndnpsRegV1
                | Profile::LongMovsdRegV1
                | Profile::LongMovsdStoreRegV1
                | Profile::LongRcpssRegV1
                | Profile::LongRsqrtpsRegV1
                | Profile::LongRcppsRegV1
                | Profile::LongMinssMaskedRegV1
                | Profile::LongMinsdMaskedRegV1
                | Profile::LongMaxsdMaskedRegV1
                | Profile::LongMinpdMaskedRegV1
                | Profile::LongMaxpdMaskedRegV1
                | Profile::LongMaxssMaskedRegV1
                | Profile::LongMinpsMaskedRegV1
                | Profile::LongMaxpsMaskedRegV1
                | Profile::LongRsqrtssRegV1
                | Profile::LongMovssRegV1
                | Profile::LongMovssMemLoadV1
                | Profile::LongMovsdMemLoadV1
                | Profile::LongMovsdMemStoreV1
                | Profile::LongMovssMemStoreV1
                | Profile::LongMovssStoreRegV1
                | Profile::LongMovupsMemLoadV1
                | Profile::LongMovupdMemLoadV1
                | Profile::LongMovupdMemStoreV1
                | Profile::LongMovupsMemStoreV1
                | Profile::LongMovupsMemBaseV1
                | Profile::LongMovupdMemBaseV1
                | Profile::LongMovupdMemRipV1
                | Profile::LongMovupdMemSibV1,
                Some(x87),
            ) => x87.validate()?,
            (
                Profile::LongX87StateV1
                | Profile::LongCr0FpInitV1
                | Profile::LongCr4FpInitV1
                | Profile::LongCr0Cr4FpInitFxsave64V1
                | Profile::LongCr0Cr4FpInitFxrstor64V1
                | Profile::LongCr0Cr4FpInitFxrstor64MinssV1
                | Profile::LongCr0Cr4FpInitV1,
                None,
            ) => {
                return Err(invalid("state.x87", "required by this profile"));
            }
            (
                Profile::LongX87SseStateV1
                | Profile::LongX87SelectorsStateV1
                | Profile::LongFxsaveRipV1
                | Profile::LongFxsave64RaxV1
                | Profile::LongFxsave64Cr0GatesV1
                | Profile::LongFxrstor64RbxV1
                | Profile::LongFxrstor64Cr0GatesV1
                | Profile::LongMovupsRegV1
                | Profile::LongMovapsRegV1
                | Profile::LongMovapsReverseRegV1
                | Profile::LongMovapdRegV1
                | Profile::LongMovupdRegV1
                | Profile::LongMovupdReverseRegV1
                | Profile::LongMovapdReverseRegV1
                | Profile::LongXorpsRegV1
                | Profile::LongXorpdRegV1
                | Profile::LongXorpdPrerequisitesV1
                | Profile::LongMinssInvalidControlsV1
                | Profile::LongMinssUnmaskedDenormalV1
                | Profile::LongMaxssUnmaskedDenormalV1
                | Profile::LongMinsdUnmaskedDenormalV1
                | Profile::LongMaxsdUnmaskedDenormalV1
                | Profile::LongMinpdUnmaskedDenormalV1
                | Profile::LongMinpsUnmaskedDenormalV1
                | Profile::LongMaxpsUnmaskedDenormalV1
                | Profile::LongMaxpdUnmaskedDenormalV1
                | Profile::LongAndpdRegV1
                | Profile::LongOrpdRegV1
                | Profile::LongAndnpdRegV1
                | Profile::LongPxorRegV1
                | Profile::LongPandRegV1
                | Profile::LongPandnRegV1
                | Profile::LongPorRegV1
                | Profile::LongPavgbRegV1
                | Profile::LongPavgwRegV1
                | Profile::LongPmaxubRegV1
                | Profile::LongPminubRegV1
                | Profile::LongPmaxswRegV1
                | Profile::LongPminswRegV1
                | Profile::LongPmullwRegV1
                | Profile::LongPmulhuwRegV1
                | Profile::LongPsadbwRegV1
                | Profile::LongPmovmskbXmmGprV1
                | Profile::LongPmuludqRegV1
                | Profile::LongPmaddwdRegV1
                | Profile::LongPaddbRegV1
                | Profile::LongPaddwRegV1
                | Profile::LongPsubwRegV1
                | Profile::LongPadduswRegV1
                | Profile::LongPsubuswRegV1
                | Profile::LongPsubbRegV1
                | Profile::LongPaddusbRegV1
                | Profile::LongPaddsbRegV1
                | Profile::LongPsubsbRegV1
                | Profile::LongPaddswRegV1
                | Profile::LongPsubswRegV1
                | Profile::LongPadddRegV1
                | Profile::LongPsubdRegV1
                | Profile::LongPaddqRegV1
                | Profile::LongPsubqRegV1
                | Profile::LongPcmpeqbRegV1
                | Profile::LongPcmpeqwRegV1
                | Profile::LongPcmpeqdRegV1
                | Profile::LongPcmpgtbRegV1
                | Profile::LongPcmpgtwRegV1
                | Profile::LongPcmpgtdRegV1
                | Profile::LongPunpcklbwRegV1
                | Profile::LongPunpckhbwRegV1
                | Profile::LongPunpcklwdRegV1
                | Profile::LongPunpckhwdRegV1
                | Profile::LongPunpckldqRegV1
                | Profile::LongPunpckhdqRegV1
                | Profile::LongPunpcklqdqRegV1
                | Profile::LongPunpckhqdqRegV1
                | Profile::LongPacksswbRegV1
                | Profile::LongPackuswbRegV1
                | Profile::LongPackssdwRegV1
                | Profile::LongPshufdRegV1
                | Profile::LongPsllwImmRegV1
                | Profile::LongPslldImmRegV1
                | Profile::LongPsllqImmRegV1
                | Profile::LongPslldqImmRegV1
                | Profile::LongPsrldqImmRegV1
                | Profile::LongPsrlqImmRegV1
                | Profile::LongPsrldImmRegV1
                | Profile::LongPsrlwImmRegV1
                | Profile::LongPsrawImmRegV1
                | Profile::LongPsradImmRegV1
                | Profile::LongPsubusbRegV1
                | Profile::LongAndpsRegV1
                | Profile::LongOrpsRegV1
                | Profile::LongAndnpsRegV1
                | Profile::LongMovsdRegV1
                | Profile::LongMovsdStoreRegV1
                | Profile::LongRcpssRegV1
                | Profile::LongRsqrtpsRegV1
                | Profile::LongRcppsRegV1
                | Profile::LongMinssMaskedRegV1
                | Profile::LongMinsdMaskedRegV1
                | Profile::LongMaxsdMaskedRegV1
                | Profile::LongMinpdMaskedRegV1
                | Profile::LongMaxpdMaskedRegV1
                | Profile::LongMaxssMaskedRegV1
                | Profile::LongMinpsMaskedRegV1
                | Profile::LongMaxpsMaskedRegV1
                | Profile::LongRsqrtssRegV1
                | Profile::LongMovssRegV1
                | Profile::LongMovssMemLoadV1
                | Profile::LongMovsdMemLoadV1
                | Profile::LongMovsdMemStoreV1
                | Profile::LongMovssMemStoreV1
                | Profile::LongMovssStoreRegV1
                | Profile::LongMovupsMemLoadV1
                | Profile::LongMovupdMemLoadV1
                | Profile::LongMovupdMemStoreV1
                | Profile::LongMovupsMemStoreV1
                | Profile::LongMovupsMemBaseV1
                | Profile::LongMovupdMemBaseV1
                | Profile::LongMovupdMemRipV1
                | Profile::LongMovupdMemSibV1,
                None,
            ) => {
                return Err(invalid("state.x87", "required by this profile"));
            }
            (_, None) => {}
            (_, Some(_)) => return Err(invalid("state.x87", "not part of this profile")),
        }
        if let Some(x87) = &self.x87 {
            if matches!(
                profile,
                Profile::LongX87SelectorsStateV1 | Profile::LongFxsaveRipV1
            ) {
                if x87.instruction_selector.is_none() || x87.data_selector.is_none() {
                    return Err(invalid(
                        "state.x87.selectors",
                        "both selectors are required by this profile",
                    ));
                }
            } else if profile != Profile::LongFxsave64RaxV1
                && profile != Profile::LongFxsave64Cr0GatesV1
                && profile != Profile::LongFxrstor64RbxV1
                && profile != Profile::LongFxrstor64Cr0GatesV1
                && (x87.instruction_selector.is_some() || x87.data_selector.is_some())
            {
                return Err(invalid(
                    "state.x87.selectors",
                    "selectors are not part of this profile",
                ));
            }
        }
        match (profile, &self.sse) {
            (
                Profile::LongX87SseStateV1
                | Profile::LongCr0FpInitV1
                | Profile::LongCr4FpInitV1
                | Profile::LongCr0Cr4FpInitFxsave64V1
                | Profile::LongCr0Cr4FpInitFxrstor64V1
                | Profile::LongCr0Cr4FpInitFxrstor64MinssV1
                | Profile::LongCr0Cr4FpInitV1
                | Profile::LongX87SelectorsStateV1
                | Profile::LongFxsaveRipV1
                | Profile::LongFxsave64RaxV1
                | Profile::LongFxsave64Cr0GatesV1
                | Profile::LongFxrstor64RbxV1
                | Profile::LongFxrstor64Cr0GatesV1
                | Profile::LongMovupsRegV1
                | Profile::LongMovapsRegV1
                | Profile::LongMovapsReverseRegV1
                | Profile::LongMovapdRegV1
                | Profile::LongMovupdRegV1
                | Profile::LongMovupdReverseRegV1
                | Profile::LongMovapdReverseRegV1
                | Profile::LongXorpsRegV1
                | Profile::LongXorpdRegV1
                | Profile::LongXorpdPrerequisitesV1
                | Profile::LongMinssInvalidControlsV1
                | Profile::LongMinssUnmaskedDenormalV1
                | Profile::LongMaxssUnmaskedDenormalV1
                | Profile::LongMinsdUnmaskedDenormalV1
                | Profile::LongMaxsdUnmaskedDenormalV1
                | Profile::LongMinpdUnmaskedDenormalV1
                | Profile::LongMinpsUnmaskedDenormalV1
                | Profile::LongMaxpsUnmaskedDenormalV1
                | Profile::LongMaxpdUnmaskedDenormalV1
                | Profile::LongCvtdq2pdRegExactV1
                | Profile::LongCvtdq2pdRegistersV1
                | Profile::LongCvtdq2psRegistersV1
                | Profile::LongCvtss2sdRegistersV1
                | Profile::LongCvtps2pdRegistersV1
                | Profile::LongComissUcomissRegistersV1
                | Profile::LongComisdUcomisdRegistersV1
                | Profile::LongCvtsi2sdR32ExactV1
                | Profile::LongCvtsi2sdR64ExactV1
                | Profile::LongCvtsi2sdRegistersV1
                | Profile::LongCvtsi2ssRegistersV1
                | Profile::LongNativeXorpdXmm0Xmm1V1
                | Profile::LongAndpdRegV1
                | Profile::LongOrpdRegV1
                | Profile::LongAndnpdRegV1
                | Profile::LongPxorRegV1
                | Profile::LongPandRegV1
                | Profile::LongPandnRegV1
                | Profile::LongPorRegV1
                | Profile::LongPavgbRegV1
                | Profile::LongPavgwRegV1
                | Profile::LongPmaxubRegV1
                | Profile::LongPminubRegV1
                | Profile::LongPmaxswRegV1
                | Profile::LongPminswRegV1
                | Profile::LongPmullwRegV1
                | Profile::LongPmulhuwRegV1
                | Profile::LongPsadbwRegV1
                | Profile::LongPmovmskbXmmGprV1
                | Profile::LongPmuludqRegV1
                | Profile::LongPmaddwdRegV1
                | Profile::LongPaddbRegV1
                | Profile::LongPaddwRegV1
                | Profile::LongPsubwRegV1
                | Profile::LongPadduswRegV1
                | Profile::LongPsubuswRegV1
                | Profile::LongPsubbRegV1
                | Profile::LongPaddusbRegV1
                | Profile::LongPaddsbRegV1
                | Profile::LongPsubsbRegV1
                | Profile::LongPaddswRegV1
                | Profile::LongPsubswRegV1
                | Profile::LongPadddRegV1
                | Profile::LongPsubdRegV1
                | Profile::LongPaddqRegV1
                | Profile::LongPsubqRegV1
                | Profile::LongPcmpeqbRegV1
                | Profile::LongPcmpeqwRegV1
                | Profile::LongPcmpeqdRegV1
                | Profile::LongPcmpgtbRegV1
                | Profile::LongPcmpgtwRegV1
                | Profile::LongPcmpgtdRegV1
                | Profile::LongPunpcklbwRegV1
                | Profile::LongPunpckhbwRegV1
                | Profile::LongPunpcklwdRegV1
                | Profile::LongPunpckhwdRegV1
                | Profile::LongPunpckldqRegV1
                | Profile::LongPunpckhdqRegV1
                | Profile::LongPunpcklqdqRegV1
                | Profile::LongPunpckhqdqRegV1
                | Profile::LongPacksswbRegV1
                | Profile::LongPackuswbRegV1
                | Profile::LongPackssdwRegV1
                | Profile::LongPshufdRegV1
                | Profile::LongPsllwImmRegV1
                | Profile::LongPslldImmRegV1
                | Profile::LongPsllqImmRegV1
                | Profile::LongPslldqImmRegV1
                | Profile::LongPsrldqImmRegV1
                | Profile::LongPsrlqImmRegV1
                | Profile::LongPsrldImmRegV1
                | Profile::LongPsrlwImmRegV1
                | Profile::LongPsrawImmRegV1
                | Profile::LongPsradImmRegV1
                | Profile::LongPsubusbRegV1
                | Profile::LongAndpsRegV1
                | Profile::LongOrpsRegV1
                | Profile::LongAndnpsRegV1
                | Profile::LongMovsdRegV1
                | Profile::LongMovsdStoreRegV1
                | Profile::LongRcpssRegV1
                | Profile::LongRsqrtpsRegV1
                | Profile::LongRcppsRegV1
                | Profile::LongMinssMaskedRegV1
                | Profile::LongMinsdMaskedRegV1
                | Profile::LongMaxsdMaskedRegV1
                | Profile::LongMinpdMaskedRegV1
                | Profile::LongMaxpdMaskedRegV1
                | Profile::LongMaxssMaskedRegV1
                | Profile::LongMinpsMaskedRegV1
                | Profile::LongMaxpsMaskedRegV1
                | Profile::LongRsqrtssRegV1
                | Profile::LongMovssRegV1
                | Profile::LongMovssMemLoadV1
                | Profile::LongMovsdMemLoadV1
                | Profile::LongMovsdMemStoreV1
                | Profile::LongMovssMemStoreV1
                | Profile::LongMovssStoreRegV1
                | Profile::LongMovupsMemLoadV1
                | Profile::LongMovupdMemLoadV1
                | Profile::LongMovupdMemStoreV1
                | Profile::LongMovupsMemStoreV1
                | Profile::LongMovupsMemBaseV1
                | Profile::LongMovupdMemBaseV1
                | Profile::LongMovupdMemRipV1
                | Profile::LongMovupdMemSibV1,
                Some(sse),
            ) => sse.validate()?,
            (
                Profile::LongX87SseStateV1
                | Profile::LongX87SelectorsStateV1
                | Profile::LongFxsaveRipV1
                | Profile::LongFxsave64RaxV1
                | Profile::LongFxsave64Cr0GatesV1
                | Profile::LongFxrstor64RbxV1
                | Profile::LongFxrstor64Cr0GatesV1
                | Profile::LongMovupsRegV1
                | Profile::LongMovapsRegV1
                | Profile::LongMovapsReverseRegV1
                | Profile::LongMovapdRegV1
                | Profile::LongMovupdRegV1
                | Profile::LongMovupdReverseRegV1
                | Profile::LongMovapdReverseRegV1
                | Profile::LongXorpsRegV1
                | Profile::LongXorpdRegV1
                | Profile::LongXorpdPrerequisitesV1
                | Profile::LongMinssInvalidControlsV1
                | Profile::LongMinssUnmaskedDenormalV1
                | Profile::LongMaxssUnmaskedDenormalV1
                | Profile::LongMinsdUnmaskedDenormalV1
                | Profile::LongMaxsdUnmaskedDenormalV1
                | Profile::LongMinpdUnmaskedDenormalV1
                | Profile::LongMinpsUnmaskedDenormalV1
                | Profile::LongMaxpsUnmaskedDenormalV1
                | Profile::LongMaxpdUnmaskedDenormalV1
                | Profile::LongCvtdq2pdRegExactV1
                | Profile::LongCvtdq2pdRegistersV1
                | Profile::LongCvtdq2psRegistersV1
                | Profile::LongCvtss2sdRegistersV1
                | Profile::LongCvtps2pdRegistersV1
                | Profile::LongComissUcomissRegistersV1
                | Profile::LongComisdUcomisdRegistersV1
                | Profile::LongCvtsi2sdR32ExactV1
                | Profile::LongCvtsi2sdR64ExactV1
                | Profile::LongCvtsi2sdRegistersV1
                | Profile::LongCvtsi2ssRegistersV1
                | Profile::LongNativeXorpdXmm0Xmm1V1
                | Profile::LongAndpdRegV1
                | Profile::LongOrpdRegV1
                | Profile::LongAndnpdRegV1
                | Profile::LongPxorRegV1
                | Profile::LongPandRegV1
                | Profile::LongPandnRegV1
                | Profile::LongPorRegV1
                | Profile::LongPavgbRegV1
                | Profile::LongPavgwRegV1
                | Profile::LongPmaxubRegV1
                | Profile::LongPminubRegV1
                | Profile::LongPmaxswRegV1
                | Profile::LongPminswRegV1
                | Profile::LongPmullwRegV1
                | Profile::LongPmulhuwRegV1
                | Profile::LongPsadbwRegV1
                | Profile::LongPmovmskbXmmGprV1
                | Profile::LongPmuludqRegV1
                | Profile::LongPmaddwdRegV1
                | Profile::LongPaddbRegV1
                | Profile::LongPaddwRegV1
                | Profile::LongPsubwRegV1
                | Profile::LongPadduswRegV1
                | Profile::LongPsubuswRegV1
                | Profile::LongPsubbRegV1
                | Profile::LongPaddusbRegV1
                | Profile::LongPaddsbRegV1
                | Profile::LongPsubsbRegV1
                | Profile::LongPaddswRegV1
                | Profile::LongPsubswRegV1
                | Profile::LongPadddRegV1
                | Profile::LongPsubdRegV1
                | Profile::LongPaddqRegV1
                | Profile::LongPsubqRegV1
                | Profile::LongPcmpeqbRegV1
                | Profile::LongPcmpeqwRegV1
                | Profile::LongPcmpeqdRegV1
                | Profile::LongPcmpgtbRegV1
                | Profile::LongPcmpgtwRegV1
                | Profile::LongPcmpgtdRegV1
                | Profile::LongPunpcklbwRegV1
                | Profile::LongPunpckhbwRegV1
                | Profile::LongPunpcklwdRegV1
                | Profile::LongPunpckhwdRegV1
                | Profile::LongPunpckldqRegV1
                | Profile::LongPunpckhdqRegV1
                | Profile::LongPunpcklqdqRegV1
                | Profile::LongPunpckhqdqRegV1
                | Profile::LongPacksswbRegV1
                | Profile::LongPackuswbRegV1
                | Profile::LongPackssdwRegV1
                | Profile::LongPshufdRegV1
                | Profile::LongPsllwImmRegV1
                | Profile::LongPslldImmRegV1
                | Profile::LongPsllqImmRegV1
                | Profile::LongPslldqImmRegV1
                | Profile::LongPsrldqImmRegV1
                | Profile::LongPsrlqImmRegV1
                | Profile::LongPsrldImmRegV1
                | Profile::LongPsrlwImmRegV1
                | Profile::LongPsrawImmRegV1
                | Profile::LongPsradImmRegV1
                | Profile::LongPsubusbRegV1
                | Profile::LongAndpsRegV1
                | Profile::LongOrpsRegV1
                | Profile::LongAndnpsRegV1
                | Profile::LongMovsdRegV1
                | Profile::LongMovsdStoreRegV1
                | Profile::LongRcpssRegV1
                | Profile::LongRsqrtpsRegV1
                | Profile::LongRcppsRegV1
                | Profile::LongMinssMaskedRegV1
                | Profile::LongMinsdMaskedRegV1
                | Profile::LongMaxsdMaskedRegV1
                | Profile::LongMinpdMaskedRegV1
                | Profile::LongMaxpdMaskedRegV1
                | Profile::LongMaxssMaskedRegV1
                | Profile::LongMinpsMaskedRegV1
                | Profile::LongMaxpsMaskedRegV1
                | Profile::LongRsqrtssRegV1
                | Profile::LongMovssRegV1
                | Profile::LongMovssMemLoadV1
                | Profile::LongMovsdMemLoadV1
                | Profile::LongMovsdMemStoreV1
                | Profile::LongMovssMemStoreV1
                | Profile::LongMovssStoreRegV1
                | Profile::LongMovupsMemLoadV1
                | Profile::LongMovupdMemLoadV1
                | Profile::LongMovupdMemStoreV1
                | Profile::LongMovupsMemStoreV1
                | Profile::LongMovupsMemBaseV1
                | Profile::LongMovupdMemBaseV1
                | Profile::LongMovupdMemRipV1
                | Profile::LongMovupdMemSibV1,
                None,
            ) => {
                return Err(invalid("state.sse", "required by this profile"));
            }
            (_, None) => {}
            (_, Some(_)) => return Err(invalid("state.sse", "not part of this profile")),
        }
        if matches!(
            profile,
            Profile::LongMinssMaskedRegV1
                | Profile::LongMinsdMaskedRegV1
                | Profile::LongMaxsdMaskedRegV1
                | Profile::LongMinpdMaskedRegV1
                | Profile::LongMaxpdMaskedRegV1
                | Profile::LongMaxssMaskedRegV1
                | Profile::LongMinpsMaskedRegV1
                | Profile::LongMaxpsMaskedRegV1
        ) {
            let mxcsr = self
                .sse
                .as_ref()
                .expect("masked MIN profile requires SSE state above")
                .mxcsr
                .get();
            if !min_max_masked_mxcsr_supported(mxcsr) {
                return Err(invalid(
                    "state.sse.mxcsr",
                    "MINSS/MAXSS/MINPS/MAXPS/MINSD/MAXSD/MINPD masked profiles require reserved bits clear, all exception masks set, and DAZ/FTZ disabled",
                ));
            }
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ExpectedExit {
    StopAddress,
    Halted,
    Unsupported,
    InstructionBudgetExhausted,
    RepIterationBudgetExhausted,
    StopRequested,
    Exception,
    /// The bounded fixture backend could not complete an access; this is not a guest exception.
    HarnessError,
}

/// CPL input is present only for the bounded CLI/STI/HLT profiles.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(try_from = "u8", into = "u8")]
pub enum CliCpl {
    Cpl0,
    Cpl1,
    Cpl2,
    Cpl3,
}

impl TryFrom<u8> for CliCpl {
    type Error = &'static str;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(Self::Cpl0),
            1 => Ok(Self::Cpl1),
            2 => Ok(Self::Cpl2),
            3 => Ok(Self::Cpl3),
            _ => Err("CPL must be between 0 and 3"),
        }
    }
}

impl From<CliCpl> for u8 {
    fn from(value: CliCpl) -> Self {
        match value {
            CliCpl::Cpl0 => 0,
            CliCpl::Cpl1 => 1,
            CliCpl::Cpl2 => 2,
            CliCpl::Cpl3 => 3,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Fixture {
    pub schema_version: u32,
    pub case_id: String,
    pub profile: Profile,
    /// Narrow CPL input used only by the exact CLI profile.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cli_cpl: Option<CliCpl>,
    /// CPL input required only by the bounded IA32_EFER RDMSR profile.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub efer_cpl: Option<CliCpl>,
    /// Immutable caller-selected EFER value; never stored in CPU state.
    #[serde(
        default,
        deserialize_with = "deserialize_present_hex_u64",
        skip_serializing_if = "Option::is_none"
    )]
    pub efer_value: Option<HexU64>,
    pub provenance: Provenance,
    pub seed: Option<HexU64>,
    /// Set only by explicit conversion; never reuse the old input digest as this fixture's digest.
    pub converted_from_v1_sha256: Option<String>,
    pub initial_state: IntegerState,
    /// Explicit controls required only by the local S2-38b exact XORPD profile.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub xorpd_prerequisites: Option<XorpdPrerequisiteSnapshot>,
    /// Explicit controls required only by the exact S2-38c MINSS profile.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub minss_invalid_controls: Option<MinssInvalidControlSnapshot>,
    /// Immutable control inputs required only by the exact S2-38f MAXSS profile.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub maxss_unmasked_controls: Option<MaxssUnmaskedControlSnapshot>,
    /// Immutable MINSD gate inputs, echoed only for S2-38g.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub minsd_unmasked_controls: Option<MinsdUnmaskedControlSnapshot>,
    /// Immutable MAXSD gate inputs, echoed only for S2-38h.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub maxsd_unmasked_controls: Option<MaxsdUnmaskedControlSnapshot>,
    /// Immutable MINPD gate inputs, echoed only for S2-38i.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub minpd_unmasked_controls: Option<MinpdUnmaskedControlSnapshot>,
    /// Immutable MINPS gate inputs, echoed only for S2-38k.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub minps_unmasked_controls: Option<MinpsUnmaskedControlSnapshot>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub maxps_unmasked_controls: Option<MaxpsUnmaskedControlSnapshot>,
    /// Immutable MAXPD gate inputs, echoed only for S2-38j.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub maxpd_unmasked_controls: Option<MaxpdUnmaskedControlSnapshot>,
    /// Immutable CVTDQ2PD gate inputs, echoed for S2-37m.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cvtdq2pd_controls: Option<Cvtdq2pdControlSnapshot>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cvtdq2ps_controls: Option<Cvtdq2psControlSnapshot>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cvtss2sd_controls: Option<Cvtss2sdControlSnapshot>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cvtps2pd_controls: Option<Cvtps2pdControlSnapshot>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub comiss_ucomiss_controls: Option<ComissUcomissControlSnapshot>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub comisd_ucomisd_controls: Option<ComisdUcomisdControlSnapshot>,
    /// Immutable CVTSI2SD r32 gate inputs and the irrelevant OSXMMEXCPT echo.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cvtsi2sd_r32_controls: Option<Cvtsi2sdR32ControlSnapshot>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cvtsi2sd_r64_controls: Option<Cvtsi2sdR32ControlSnapshot>,
    /// Immutable SSE/CR0/CR4 inputs for the canonical S2-37r CVTSI2SS profile.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cvtsi2ss_controls: Option<Cvtsi2ssControlSnapshot>,
    /// Explicit CR0 inputs required only by the exact S2-39f FXSAVE64 profile.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fxsave64_cr0_controls: Option<Fxsave64Cr0Snapshot>,
    /// Explicit CR0 inputs required only by the exact S2-39g FXRSTOR64 profile.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fxrstor64_cr0_controls: Option<Fxrstor64Cr0Snapshot>,
    /// Explicit TSC source and CR4.TSD input required only by S2-26e.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rdtsc_controls: Option<RdtscSnapshot>,
    /// Code and data share the same guest address space. No implicit host buffer.
    pub memory: Vec<MemoryRegionSpec>,
    /// Optional external harness boundary, checked before fetch, not an x86 opcode.
    pub stop_rip: Option<HexU64>,
    pub instruction_budget: u64,
    pub wall_timeout_ms: u32,
    pub rflags_mask: HexU64,
    pub expected_exit: ExpectedExit,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct XorpdPrerequisiteSnapshot {
    pub sse2_available: bool,
    pub cr0_em: bool,
    pub cr0_ts: bool,
    pub cr4_osfxsr: bool,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct MinssInvalidControlSnapshot {
    pub sse_available: bool,
    pub cr0_em: bool,
    pub cr0_ts: bool,
    pub cr4_osfxsr: bool,
    pub cr4_osxmmexcpt: bool,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct MaxssUnmaskedControlSnapshot {
    pub sse_available: bool,
    pub cr0_em: bool,
    pub cr0_ts: bool,
    pub cr4_osfxsr: bool,
    pub cr4_osxmmexcpt: bool,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct MinsdUnmaskedControlSnapshot {
    pub sse2_available: bool,
    pub cr0_em: bool,
    pub cr0_ts: bool,
    pub cr4_osfxsr: bool,
    pub cr4_osxmmexcpt: bool,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct MaxsdUnmaskedControlSnapshot {
    pub sse2_available: bool,
    pub cr0_em: bool,
    pub cr0_ts: bool,
    pub cr4_osfxsr: bool,
    pub cr4_osxmmexcpt: bool,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct MinpdUnmaskedControlSnapshot {
    pub sse2_available: bool,
    pub cr0_em: bool,
    pub cr0_ts: bool,
    pub cr4_osfxsr: bool,
    pub cr4_osxmmexcpt: bool,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct MinpsUnmaskedControlSnapshot {
    pub sse_available: bool,
    pub cr0_em: bool,
    pub cr0_ts: bool,
    pub cr4_osfxsr: bool,
    pub cr4_osxmmexcpt: bool,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct MaxpsUnmaskedControlSnapshot {
    pub sse_available: bool,
    pub cr0_em: bool,
    pub cr0_ts: bool,
    pub cr4_osfxsr: bool,
    pub cr4_osxmmexcpt: bool,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct MaxpdUnmaskedControlSnapshot {
    pub sse2_available: bool,
    pub cr0_em: bool,
    pub cr0_ts: bool,
    pub cr4_osfxsr: bool,
    pub cr4_osxmmexcpt: bool,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Cvtdq2pdControlSnapshot {
    pub sse2_available: bool,
    pub cr0_em: bool,
    pub cr0_ts: bool,
    pub cr4_osfxsr: bool,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Cvtdq2psControlSnapshot {
    pub sse2_available: bool,
    pub cr0_em: bool,
    pub cr0_ts: bool,
    pub cr4_osfxsr: bool,
    pub cr4_osxmmexcpt: bool,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Cvtss2sdControlSnapshot {
    pub sse2_available: bool,
    pub cr0_em: bool,
    pub cr0_ts: bool,
    pub cr4_osfxsr: bool,
    pub cr4_osxmmexcpt: bool,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Cvtps2pdControlSnapshot {
    pub sse2_available: bool,
    pub cr0_em: bool,
    pub cr0_ts: bool,
    pub cr4_osfxsr: bool,
    pub cr4_osxmmexcpt: bool,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ComissUcomissControlSnapshot {
    pub sse_available: bool,
    pub cr0_em: bool,
    pub cr0_ts: bool,
    pub cr4_osfxsr: bool,
    pub cr4_osxmmexcpt: bool,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ComisdUcomisdControlSnapshot {
    pub sse2_available: bool,
    pub cr0_em: bool,
    pub cr0_ts: bool,
    pub cr4_osfxsr: bool,
    pub cr4_osxmmexcpt: bool,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Cvtsi2sdR32ControlSnapshot {
    pub sse2_available: bool,
    pub cr0_em: bool,
    pub cr0_ts: bool,
    pub cr4_osfxsr: bool,
    pub cr4_osxmmexcpt: bool,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Cvtsi2ssControlSnapshot {
    pub sse_available: bool,
    pub cr0_em: bool,
    pub cr0_ts: bool,
    pub cr4_osfxsr: bool,
    pub cr4_osxmmexcpt: bool,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Fxsave64Cr0Snapshot {
    pub cr0_em: bool,
    pub cr0_ts: bool,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Fxrstor64Cr0Snapshot {
    pub cr0_em: bool,
    pub cr0_ts: bool,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RdtscSnapshot {
    pub cr4_tsd: bool,
    pub source_value: HexU64,
    pub expected_source_calls: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LoadedFixture {
    pub fixture: Fixture,
    pub input_sha256: String,
}

impl Fixture {
    pub fn from_json(bytes: &[u8]) -> Result<LoadedFixture, FormatError> {
        let fixture: Self = load(bytes, MAX_FIXTURE_JSON_BYTES)?;
        fixture.validate()?;
        Ok(LoadedFixture {
            fixture,
            input_sha256: sha256_hex(bytes),
        })
    }

    pub fn to_json(&self) -> Result<Vec<u8>, FormatError> {
        self.validate()?;
        save(self, MAX_FIXTURE_JSON_BYTES)
    }

    pub fn validate(&self) -> Result<(), FormatError> {
        version(self.schema_version)?;
        validate_case_id(&self.case_id)?;
        self.initial_state.validate_for(self.profile)?;
        match (self.profile, self.efer_cpl, self.efer_value) {
            (Profile::LongEferRdmsrV1, Some(_), Some(value))
                if od_isa::msr::MsrProfile::accepts_efer_value(value.get()) => {}
            (Profile::LongEferRdmsrV1, _, Some(_)) => {
                return Err(invalid(
                    "efer_value",
                    "must be one of 0x500, 0x501, 0xd00, 0xd01",
                ));
            }
            (Profile::LongEferRdmsrV1, _, None) => {
                return Err(invalid("efer_value", "required by this profile"));
            }
            (_, Some(_), _) => return Err(invalid("efer_cpl", "not part of this profile")),
            (_, _, Some(_)) => return Err(invalid("efer_value", "not part of this profile")),
            (_, None, None) => {}
        }
        if self.profile != Profile::LongEferRdmsrV1 && self.efer_cpl.is_some() {
            return Err(invalid("efer_cpl", "not part of this profile"));
        }
        for value in [
            &self.provenance.kind,
            &self.provenance.source,
            &self.provenance.specification,
            &self.provenance.specification_version,
        ] {
            text(value, "provenance")?;
        }
        if let Some(digest) = &self.converted_from_v1_sha256 {
            digest_field(digest)?;
        }
        MemoryComparisonScope::Compare {
            regions: self.memory.clone(),
        }
        .validate()?;
        if self.memory.is_empty() {
            return Err(invalid("memory", "must contain a mapping"));
        }
        if self.instruction_budget > MAX_INSTRUCTION_BUDGET {
            return Err(invalid(
                "instruction_budget",
                "must be between 0 and 10000000",
            ));
        }
        if !(1..=MAX_WALL_TIMEOUT_MS).contains(&self.wall_timeout_ms) {
            return Err(invalid("wall_timeout_ms", "must be between 1 and 30000"));
        }
        const IF_IOPL: u64 = (1 << 9) | (3 << 12);
        let cli_flags_mask = INTEGER_FLAGS | IF_IOPL;
        if matches!(
            self.profile,
            Profile::LongCliV1
                | Profile::LongStiV1
                | Profile::LongHltV1
                | Profile::LongIretqCpl0ValidV1
                | Profile::LongLretqCpl0ValidV1
                | Profile::LongCr0FpInitV1
                | Profile::LongCr4FpInitV1
                | Profile::LongCr0Cr4FpInitFxsave64V1
                | Profile::LongCr0Cr4FpInitFxrstor64V1
                | Profile::LongCr0Cr4FpInitFxrstor64MinssV1
                | Profile::LongCr0Cr4FpInitV1,
        ) {
            if self.rflags_mask.get() != cli_flags_mask {
                return Err(invalid(
                    "rflags_mask",
                    "system-state fixtures must compare INTEGER_FLAGS, IF, and IOPL",
                ));
            }
        } else if self.rflags_mask.get() & !INTEGER_FLAGS != 0 {
            return Err(invalid("rflags_mask", "outside the integer profile"));
        }
        match (self.profile, self.cli_cpl) {
            (
                Profile::LongCliV1
                | Profile::LongStiV1
                | Profile::LongHltV1
                | Profile::LongIretqCpl0ValidV1
                | Profile::LongLretqCpl0ValidV1
                | Profile::LongCr0FpInitV1
                | Profile::LongCr4FpInitV1
                | Profile::LongCr0Cr4FpInitFxsave64V1
                | Profile::LongCr0Cr4FpInitFxrstor64V1
                | Profile::LongCr0Cr4FpInitFxrstor64MinssV1
                | Profile::LongCr0Cr4FpInitV1,
                Some(_),
            ) => {}
            (
                Profile::LongCliV1
                | Profile::LongStiV1
                | Profile::LongHltV1
                | Profile::LongIretqCpl0ValidV1
                | Profile::LongLretqCpl0ValidV1
                | Profile::LongCr0FpInitV1
                | Profile::LongCr4FpInitV1
                | Profile::LongCr0Cr4FpInitFxsave64V1
                | Profile::LongCr0Cr4FpInitFxrstor64V1
                | Profile::LongCr0Cr4FpInitFxrstor64MinssV1
                | Profile::LongCr0Cr4FpInitV1,
                None,
            ) => {
                return Err(invalid("cli_cpl", "required by the CLI/STI/HLT profile"));
            }
            (_, Some(_)) => return Err(invalid("cli_cpl", "not part of this profile")),
            (_, None) => {}
        }
        if self.profile == Profile::LongCr0FpInitV1 {
            let Some(code) = self.memory.iter().find(|region| region.region_id == "code") else {
                return Err(invalid("memory.code", "required by CR0 FP-init profile"));
            };
            let bytes = code.initial_bytes_hex.to_ascii_lowercase();
            let (length, budget, operation) = match bytes.as_str() {
                "0f20c0" => (3, 1, "read"),
                "0f22c0" => (3, 1, "write"),
                "dbe3" => (2, 1, "fninit"),
                "0f22c0dbe3" => (5, 2, "write_fninit"),
                "0f20c04883e0f30f22c0dbe3" => (12, 4, "full"),
                _ => {
                    return Err(invalid(
                        "memory.code",
                        "CR0 FP-init admits only exact MOV CR0, AND RAX,-13, and FNINIT sequences",
                    ));
                }
            };
            let cpl: u8 = self.cli_cpl.expect("validated CR0 profile CPL").into();
            let initial_cr0 = self.initial_state.cr0.expect("validated CR0 state").get();
            let target_cr0 = self.initial_state.gpr[0].get();
            let expected = match operation {
                "read" => {
                    if cpl == 0 {
                        ExpectedExit::StopAddress
                    } else {
                        ExpectedExit::Exception
                    }
                }
                "write" => {
                    if cpl != 0 {
                        ExpectedExit::Exception
                    } else if ![
                        od_isa::execute::Cr0Profile::BASE,
                        od_isa::execute::Cr0Profile::BASE | 4,
                        od_isa::execute::Cr0Profile::BASE | 8,
                        od_isa::execute::Cr0Profile::BASE | 12,
                    ]
                    .contains(&target_cr0)
                    {
                        ExpectedExit::Unsupported
                    } else {
                        ExpectedExit::StopAddress
                    }
                }
                "fninit" => {
                    if initial_cr0 & 0xcu64 != 0 {
                        ExpectedExit::Exception
                    } else {
                        ExpectedExit::StopAddress
                    }
                }
                "write_fninit" => {
                    if cpl != 0 {
                        ExpectedExit::Exception
                    } else if ![
                        od_isa::execute::Cr0Profile::BASE,
                        od_isa::execute::Cr0Profile::BASE | 4,
                        od_isa::execute::Cr0Profile::BASE | 8,
                        od_isa::execute::Cr0Profile::BASE | 12,
                    ]
                    .contains(&target_cr0)
                    {
                        ExpectedExit::Unsupported
                    } else if target_cr0 & 0xcu64 != 0 {
                        ExpectedExit::Exception
                    } else {
                        ExpectedExit::StopAddress
                    }
                }
                "full" => {
                    if cpl == 0 {
                        ExpectedExit::StopAddress
                    } else {
                        ExpectedExit::Exception
                    }
                }
                _ => ExpectedExit::StopAddress,
            };
            if code.guest_address.get() != self.initial_state.rip.get()
                || code.length != length
                || (operation == "full" && cpl != 0)
                || code.permissions.read
                || code.permissions.write
                || !code.permissions.execute
                || code.comparison_mask_hex.to_ascii_lowercase() != "ff".repeat(length as usize)
                || self.memory.len() != 1
                || self.instruction_budget != budget
                || self.stop_rip.map(HexU64::get)
                    != self.initial_state.rip.get().checked_add(length as u64)
                || self.expected_exit != expected
                || self.rflags_mask.get() != (INTEGER_FLAGS | (1 << 9) | (3 << 12))
            {
                return Err(invalid(
                    "memory.code",
                    "CR0 FP-init requires one exact executable code mapping, fixed budget/STOP, complete state masks, and the matching CPL/gate outcome",
                ));
            }
        }
        if self.profile == Profile::LongCr4FpInitV1 {
            let code = self
                .memory
                .iter()
                .find(|region| region.region_id == "code")
                .ok_or_else(|| invalid("memory.code", "required by CR4 FP-init profile"))?;
            let bytes = code.initial_bytes_hex.to_ascii_lowercase();
            let cpl: u8 = self.cli_cpl.expect("validated CR4 profile CPL").into();
            let (length, budget, expected) = match bytes.as_str() {
                "660f57c1" => {
                    let cr0 = self.initial_state.cr0.unwrap().get();
                    let cr4 = self.initial_state.cr4.unwrap().get();
                    let gate = if cr0 & 4 != 0 || cr4 & (1 << 9) == 0 {
                        ExpectedExit::Exception
                    } else if cr0 & 8 != 0 {
                        ExpectedExit::Exception
                    } else {
                        ExpectedExit::StopAddress
                    };
                    (4, 1, gate)
                }
                "dbe30f20e0480d000600000f22e0660f57c1" => {
                    if cpl != 0 {
                        return Err(invalid(
                            "cli_cpl",
                            "CR4 initialization composition requires CPL0",
                        ));
                    }
                    if self.initial_state.cr0.map(HexU64::get)
                        != Some(od_isa::execute::Cr0Profile::BASE)
                    {
                        return Err(invalid(
                            "initial_state.cr0",
                            "CR4 initialization composition requires CR0.EM=CR0.TS=0 for its successful FNINIT",
                        ));
                    }
                    (18, 5, ExpectedExit::StopAddress)
                }
                _ => {
                    return Err(invalid(
                        "memory.code",
                        "CR4 FP-init admits only the exact XORPD consumer or FNINIT plus CR4 read/OR/write/XORPD composition",
                    ));
                }
            };
            if code.guest_address.get() != self.initial_state.rip.get()
                || code.length != length
                || !code.permissions.execute
                || code.comparison_mask_hex.to_ascii_lowercase() != "ff".repeat(length as usize)
                || self.memory.len() != 1
                || self.instruction_budget != budget
                || self.stop_rip.map(HexU64::get)
                    != self.initial_state.rip.get().checked_add(length as u64)
                || self.expected_exit != expected
                || self.rflags_mask.get() != (INTEGER_FLAGS | (1 << 9) | (3 << 12))
            {
                return Err(invalid(
                    "memory.code",
                    "CR4 FP-init requires exact bytes, one executable mapping, fixed budget/STOP, complete state masks, and the matching current-state gate outcome",
                ));
            }
        }
        if self.profile == Profile::LongCr0Cr4FpInitV1 {
            let code = self
                .memory
                .iter()
                .find(|region| region.region_id == "code")
                .ok_or_else(|| invalid("memory.code", "required by CR0/CR4 FP-init profile"))?;
            let length = 28;
            if code.initial_bytes_hex.to_ascii_lowercase()
                != "0f20c04883e0f30f22c0dbe30f20e0480d000600000f22e0660f57c1"
                || self.cli_cpl != Some(CliCpl::Cpl0)
                || code.guest_address != self.initial_state.rip
                || code.length != length
                || code.permissions.read
                || code.permissions.write
                || !code.permissions.execute
                || code.comparison_mask_hex.to_ascii_lowercase() != "ff".repeat(length as usize)
                || self.memory.len() != 1
                || self.instruction_budget != 8
                || self.stop_rip.map(HexU64::get)
                    != self.initial_state.rip.get().checked_add(length as u64)
                || self.expected_exit != ExpectedExit::StopAddress
                || self.rflags_mask.get() != (INTEGER_FLAGS | (1 << 9) | (3 << 12))
            {
                return Err(invalid(
                    "memory.code",
                    "CR0/CR4 FP-init requires the exact 28-byte eight-instruction sequence, CPL0, one executable mapping, fixed budget/STOP, and complete state masks",
                ));
            }
        }
        if self.profile == Profile::LongCr0Cr4FpInitFxsave64V1 {
            let code = self.memory.iter().find(|region| region.region_id == "code");
            let save = self.memory.iter().find(|region| region.region_id == "save");
            let expected_program =
                "0f20c04883e0f30f22c0dbe30f20e0480d000600000f22e0660f57c1480fae4040";
            let exact_code = code.is_some_and(|region| {
                region.guest_address == self.initial_state.rip
                    && region.length == 33
                    && region.permissions
                        == crate::case::MemoryPermissions {
                            read: false,
                            write: false,
                            execute: true,
                        }
                    && region
                        .initial_bytes_hex
                        .eq_ignore_ascii_case(expected_program)
                    && region
                        .comparison_mask_hex
                        .eq_ignore_ascii_case(&"ff".repeat(33))
            });
            let exact_save = save.is_some_and(|region| {
                region.guest_address.get() == 0x660
                    && region.length == 512
                    && region.permissions
                        == crate::case::MemoryPermissions {
                            read: true,
                            write: true,
                            execute: false,
                        }
                    && region
                        .initial_bytes_hex
                        .eq_ignore_ascii_case(&"a5".repeat(512))
            });
            if !exact_code
                || !exact_save
                || self.cli_cpl != Some(CliCpl::Cpl0)
                || self.initial_state.gpr[0].get() != 0x620
                || self.memory.len() != 2
                || self.instruction_budget != 9
                || self.stop_rip.map(HexU64::get) != self.initial_state.rip.get().checked_add(33)
                || self.expected_exit != ExpectedExit::StopAddress
                || self.rflags_mask.get() != (INTEGER_FLAGS | (1 << 9) | (3 << 12))
            {
                return Err(invalid(
                    "memory.code/memory.save",
                    "S2-39h requires the exact 33-byte composition, RAX=0x620, a private aligned 512-byte save area at 0x660, CPL0, budget 9, and terminal STOP",
                ));
            }
        }
        if self.profile == Profile::LongCr0Cr4FpInitFxrstor64V1 {
            let code = self.memory.iter().find(|region| region.region_id == "code");
            let image = self
                .memory
                .iter()
                .find(|region| region.region_id == "image");
            let expected_program =
                "0f20c04883e0f30f22c0dbe30f20e0480d000600000f22e0660f57c1480fae4b40";
            let exact_code = code.is_some_and(|region| {
                region.guest_address == self.initial_state.rip
                    && region.length == 33
                    && region.permissions
                        == crate::case::MemoryPermissions {
                            read: false,
                            write: false,
                            execute: true,
                        }
                    && region
                        .initial_bytes_hex
                        .eq_ignore_ascii_case(expected_program)
                    && region
                        .comparison_mask_hex
                        .eq_ignore_ascii_case(&"ff".repeat(33))
            });
            let exact_image = image.is_some_and(|region| {
                region.guest_address.get() == 0x660
                    && region.length == 512
                    && region.permissions
                        == crate::case::MemoryPermissions {
                            read: true,
                            write: false,
                            execute: false,
                        }
                    && region
                        .comparison_mask_hex
                        .eq_ignore_ascii_case(&"ff".repeat(512))
            });
            let image_bytes = image.and_then(|region| decode_bytes(&region.initial_bytes_hex).ok());
            let mxcsr_valid = image_bytes.as_ref().is_some_and(|bytes| {
                bytes.get(24..28).is_some_and(|value| {
                    let mxcsr = u32::from_le_bytes(value.try_into().unwrap());
                    mxcsr & !od_isa::TARGET_FXSAVE_MXCSR_MASK == 0
                })
            });
            if !exact_code
                || !exact_image
                || !mxcsr_valid
                || self.cli_cpl != Some(CliCpl::Cpl0)
                || self.initial_state.gpr[3].get() != 0x620
                || self.memory.len() != 2
                || self.instruction_budget != 9
                || self.stop_rip.map(HexU64::get) != self.initial_state.rip.get().checked_add(33)
                || self.expected_exit != ExpectedExit::StopAddress
                || self.rflags_mask.get() != (INTEGER_FLAGS | (1 << 9) | (3 << 12))
            {
                return Err(invalid(
                    "memory.code/memory.image",
                    "S2-39i requires the exact 33-byte composition, RBX=0x620, an aligned private 512-byte readable image at 0x660, valid target MXCSR, CPL0, budget 9, and terminal STOP",
                ));
            }
        }
        if self.expected_exit == ExpectedExit::StopAddress && self.stop_rip.is_none() {
            return Err(invalid("stop_rip", "required for stop_address"));
        }
        if self.profile == Profile::LongSepDisabledV1 {
            let Some(code) = self.memory.iter().find(|region| region.region_id == "code") else {
                return Err(invalid(
                    "memory.code",
                    "required by the SEP-disabled profile",
                ));
            };
            let bytes = code.initial_bytes_hex.to_ascii_lowercase();
            let expected_stop = self.initial_state.rip.get().checked_add(2);
            if !matches!(bytes.as_str(), "0f34" | "0f35")
                || code.guest_address.get() != self.initial_state.rip.get()
                || code.length != 2
                || !code.permissions.execute
                || self.instruction_budget != 1
                || self.stop_rip.map(HexU64::get) != expected_stop
                || self.expected_exit != ExpectedExit::Exception
            {
                return Err(invalid(
                    "memory.code",
                    "SEP-disabled fixtures require one exact bare instruction, one instruction of budget, its following stop_rip, and an exception result",
                ));
            }
        }
        if self.profile == Profile::LongSwapgsV1 {
            let Some(code) = self.memory.iter().find(|region| region.region_id == "code") else {
                return Err(invalid("memory.code", "required by the SWAPGS profile"));
            };
            let bytes = code.initial_bytes_hex.to_ascii_lowercase();
            let (budget, stop_offset, expected_exit) = match bytes.as_str() {
                "0f01f8" => (1, 3, ExpectedExit::StopAddress),
                "0f01f80f01f8" => (2, 6, ExpectedExit::StopAddress),
                "0f01f865488b00" => (2, 7, ExpectedExit::StopAddress),
                _ => {
                    return Err(invalid(
                        "memory.code",
                        "SWAPGS profile admits only the exact one, double, and faulting GS-load programs",
                    ));
                }
            };
            if code.guest_address.get() != self.initial_state.rip.get()
                || !code.permissions.execute
                || self.instruction_budget != budget
                || self.expected_exit != expected_exit
                || self.stop_rip.map(HexU64::get)
                    != (expected_exit == ExpectedExit::StopAddress)
                        .then(|| self.initial_state.rip.get().checked_add(stop_offset))
                        .flatten()
            {
                return Err(invalid(
                    "memory.code",
                    "SWAPGS fixtures require the exact profile program, instruction budget, entry address, and terminal expectation",
                ));
            }
        }
        if self.profile == Profile::LongCliV1 {
            let Some(code) = self.memory.iter().find(|region| region.region_id == "code") else {
                return Err(invalid("memory.code", "required by the CLI profile"));
            };
            let cpl: u8 = self.cli_cpl.expect("validated CLI CPL").into();
            let iopl = ((self.initial_state.rflags.get() >> 12) & 3) as u8;
            let success = cpl <= iopl;
            let expected = if success {
                ExpectedExit::StopAddress
            } else {
                ExpectedExit::Exception
            };
            if code.initial_bytes_hex.to_ascii_lowercase() != "fa"
                || code.guest_address.get() != self.initial_state.rip.get()
                || code.length != 1
                || !code.permissions.execute
                || self.instruction_budget != 1
                || self.expected_exit != expected
                || self.stop_rip.map(HexU64::get)
                    != success
                        .then(|| self.initial_state.rip.get().checked_add(1))
                        .flatten()
            {
                return Err(invalid(
                    "memory.code",
                    "CLI fixtures require one bare FA, budget one, and outcome/stop matching CPL versus IOPL",
                ));
            }
        }
        if self.profile == Profile::LongStiV1 {
            let Some(code) = self.memory.iter().find(|region| region.region_id == "code") else {
                return Err(invalid("memory.code", "required by the STI profile"));
            };
            let bytes = code.initial_bytes_hex.to_ascii_lowercase();
            let (length, budget, privilege_gate) = match bytes.as_str() {
                "fb" => (1, 1, true),
                "fb90" | "fbfb" | "fbfa" => (2, 2, false),
                "fafb90" => (3, 3, false),
                _ => {
                    return Err(invalid(
                        "memory.code",
                        "STI profile admits only FB, FB90, FBFB, FBFA, and FAFB90",
                    ));
                }
            };
            let cpl: u8 = self.cli_cpl.expect("validated STI CPL").into();
            let iopl = ((self.initial_state.rflags.get() >> 12) & 3) as u8;
            let success = cpl <= iopl;
            let expected_exit = if privilege_gate && !success {
                ExpectedExit::Exception
            } else {
                ExpectedExit::StopAddress
            };
            let expected_stop = (expected_exit == ExpectedExit::StopAddress)
                .then(|| self.initial_state.rip.get().checked_add(length as u64))
                .flatten();
            if self.initial_state.rflags.get() & (1 << 9) != 0
                || self.initial_state.sti_shadow != Some(StiShadowSnapshot::Clear)
                || code.guest_address.get() != self.initial_state.rip.get()
                || code.length != length
                || !code.permissions.execute
                || self.instruction_budget != budget
                || (!privilege_gate && !success)
                || self.expected_exit != expected_exit
                || self.stop_rip.map(HexU64::get) != expected_stop
            {
                return Err(invalid(
                    "memory.code",
                    "STI fixtures require IF=0, Clear shadow, allowed CPL for sequence forms, exact budget, and the expected terminal boundary",
                ));
            }
        }
        if self.profile == Profile::LongHltV1 {
            let Some(code) = self.memory.iter().find(|region| region.region_id == "code") else {
                return Err(invalid("memory.code", "required by the HLT profile"));
            };
            let bytes = code.initial_bytes_hex.to_ascii_lowercase();
            let (budget, cpl0_only) = match bytes.as_str() {
                "f4" => (1, false),
                "fbf4" | "faf4" => (2, true),
                "fafbf4" => (3, true),
                _ => {
                    return Err(invalid(
                        "memory.code",
                        "HLT profile admits only F4, FB F4, FA F4, and FA FB F4",
                    ));
                }
            };
            let cpl: u8 = self.cli_cpl.expect("validated HLT CPL").into();
            let iopl = ((self.initial_state.rflags.get() >> 12) & 3) as u8;
            let success = cpl == 0;
            let expected_exit = if success {
                ExpectedExit::Halted
            } else {
                ExpectedExit::Exception
            };
            if (cpl0_only && !success)
                || !matches!(iopl, 0 | 3)
                || self.initial_state.activity != Some(CpuActivitySnapshot::Running)
                || self.initial_state.sti_shadow != Some(StiShadowSnapshot::Clear)
                || code.guest_address.get() != self.initial_state.rip.get()
                || code.length as usize != bytes.len() / 2
                || !code.permissions.execute
                || self.instruction_budget != budget
                || self.expected_exit != expected_exit
                || self.stop_rip.is_some()
            {
                return Err(invalid(
                    "memory.code",
                    "HLT fixtures require Running activity, Clear shadow, exact code/budget, CPL0 Halted or CPL1-3 #GP, and no stop_rip",
                ));
            }
        }
        if self.profile == Profile::LongIretqCpl0ValidV1 {
            let Some(code) = self.memory.iter().find(|region| region.region_id == "code") else {
                return Err(invalid("memory.code", "required by the IRETQ profile"));
            };
            let Some(frame) = self
                .memory
                .iter()
                .find(|region| region.region_id == "frame")
            else {
                return Err(invalid("memory.frame", "required by the IRETQ profile"));
            };
            let Some(gdt) = self.memory.iter().find(|region| region.region_id == "gdt") else {
                return Err(invalid("memory.gdt", "required by the IRETQ profile"));
            };
            let frame_bytes = decode_bytes(&frame.initial_bytes_hex)
                .map_err(|_| invalid("memory.frame", "invalid frame bytes"))?;
            let gdt_bytes = decode_bytes(&gdt.initial_bytes_hex)
                .map_err(|_| invalid("memory.gdt", "invalid GDT bytes"))?;
            if frame_bytes.len() != 40 {
                return Err(invalid("memory.frame", "must contain exactly five qwords"));
            }
            let saved_rip = u64::from_le_bytes(frame_bytes[0..8].try_into().unwrap());
            let saved_cs = u64::from_le_bytes(frame_bytes[8..16].try_into().unwrap()) as u16;
            let saved_flags = u64::from_le_bytes(frame_bytes[16..24].try_into().unwrap());
            let saved_rsp = u64::from_le_bytes(frame_bytes[24..32].try_into().unwrap());
            let saved_ss = u64::from_le_bytes(frame_bytes[32..40].try_into().unwrap()) as u16;
            let gdtr_base = self.initial_state.gdtr_base.expect("required GDTR").get();
            let gdtr_limit = self
                .initial_state
                .gdtr_limit
                .expect("required GDTR limit")
                .get();
            let iret_flags_mask = INTEGER_FLAGS | (1 << 9) | (3 << 12);
            let cpl = self.cli_cpl.expect("validated IRETQ CPL");
            let descriptor_matches = |selector: u16, expected: [u8; 8]| {
                let start = usize::from(selector & !7);
                gdt_bytes.get(start..start + 8) == Some(expected.as_slice())
            };
            if cpl != CliCpl::Cpl0
                || code.guest_address != self.initial_state.rip
                || code.initial_bytes_hex.to_ascii_lowercase() != "48cf"
                || code.length != 2
                || !code.permissions.execute
                || self.instruction_budget != 1
                || self.expected_exit != ExpectedExit::StopAddress
                || self.stop_rip != Some(HexU64(saved_rip))
                || self.rflags_mask.get() != iret_flags_mask
                || self.initial_state.activity != Some(CpuActivitySnapshot::Running)
                || self.initial_state.sti_shadow != Some(StiShadowSnapshot::Clear)
                || frame.guest_address.get() != self.initial_state.gpr[4].get()
                || frame.length != 40
                || !frame.permissions.read
                || gdt.guest_address.get() != gdtr_base
                || u64::from(gdt.length) <= gdtr_limit
                || !gdt.permissions.read
                || saved_flags & 2 == 0
                || saved_flags & !iret_flags_mask != 0
                || saved_rsp >> 47 != 0 && saved_rsp >> 47 != 0x1ffff
                || saved_rip >> 47 != 0 && saved_rip >> 47 != 0x1ffff
                || saved_cs & 7 != 0
                || saved_ss & 7 != 0
                || saved_cs == 0
                || saved_ss == 0
                || u64::from(saved_cs & !7) + 7 > gdtr_limit
                || u64::from(saved_ss & !7) + 7 > gdtr_limit
                || !descriptor_matches(saved_cs, [0xff, 0xff, 0, 0, 0, 0x9b, 0xaf, 0])
                || !descriptor_matches(saved_ss, [0xff, 0xff, 0, 0, 0, 0x93, 0xcf, 0])
            {
                return Err(invalid(
                    "memory.code",
                    "IRETQ requires exact 48 CF, one retirement, CPL0, a separate 40-byte frame, exact bounded CS/SS descriptors, valid saved flags/selectors, and stop at saved RIP",
                ));
            }
        }
        if self.profile == Profile::LongLretqCpl0ValidV1 {
            let code = self
                .memory
                .iter()
                .find(|r| r.region_id == "code")
                .ok_or(invalid("memory.code", "required by the LRETQ profile"))?;
            let frame = self
                .memory
                .iter()
                .find(|r| r.region_id == "frame")
                .ok_or(invalid("memory.frame", "required by the LRETQ profile"))?;
            let gdt = self
                .memory
                .iter()
                .find(|r| r.region_id == "gdt")
                .ok_or(invalid("memory.gdt", "required by the LRETQ profile"))?;
            let frame_bytes = decode_bytes(&frame.initial_bytes_hex)
                .map_err(|_| invalid("memory.frame", "invalid frame bytes"))?;
            let gdt_bytes = decode_bytes(&gdt.initial_bytes_hex)
                .map_err(|_| invalid("memory.gdt", "invalid GDT bytes"))?;
            if frame_bytes.len() != 16 {
                return Err(invalid("memory.frame", "must contain exactly two qwords"));
            }
            let target_rip = u64::from_le_bytes(frame_bytes[0..8].try_into().unwrap());
            let selector = u64::from_le_bytes(frame_bytes[8..16].try_into().unwrap()) as u16;
            let gdtr_base = self.initial_state.gdtr_base.expect("required GDTR").get();
            let gdtr_limit = self
                .initial_state
                .gdtr_limit
                .expect("required GDTR limit")
                .get();
            let canonical48 = |value: u64| value >> 47 == 0 || value >> 47 == 0x1ffff;
            let old_rsp = self.initial_state.gpr[4].get();
            let current_cs = self
                .initial_state
                .cs_selector
                .expect("required current CS")
                .get();
            let current_access = self
                .initial_state
                .cs_cache_access
                .expect("required CS cache")
                .get();
            let current_base = self
                .initial_state
                .cs_cache_base
                .expect("required CS cache")
                .get();
            let current_limit = self
                .initial_state
                .cs_cache_limit
                .expect("required CS cache")
                .get();
            let current_flags = self
                .initial_state
                .cs_cache_flags
                .expect("required CS cache")
                .get();
            let frame_end = old_rsp.checked_add(16);
            let descriptor_start = usize::from(selector & !7);
            let expected_cs = [0xff, 0xff, 0, 0, 0, 0x9b, 0xaf, 0];
            let descriptor_matches = gdt_bytes.get(descriptor_start..descriptor_start + 8)
                == Some(expected_cs.as_slice());
            let cpl = self.cli_cpl.expect("validated LRETQ CPL");
            if cpl != CliCpl::Cpl0
                || code.guest_address != self.initial_state.rip
                || code.initial_bytes_hex.to_ascii_lowercase() != "48cb"
                || code.length != 2
                || !code.permissions.execute
                || self.instruction_budget != 1
                || self.expected_exit != ExpectedExit::StopAddress
                || self.stop_rip != Some(HexU64(target_rip))
                || self.rflags_mask.get() != (INTEGER_FLAGS | (1 << 9) | (3 << 12))
                || self.initial_state.activity != Some(CpuActivitySnapshot::Running)
                || self.initial_state.sti_shadow != Some(StiShadowSnapshot::Clear)
                || self.initial_state.rflags.get() & 2 == 0
                || self.initial_state.rflags.get() & !(INTEGER_FLAGS | (1 << 9) | (3 << 12)) != 0
                || !canonical48(target_rip)
                || !canonical48(old_rsp)
                || frame_end.is_none_or(|end| !canonical48(end))
                || frame.guest_address.get() != self.initial_state.gpr[4].get()
                || frame.length != 16
                || !frame.permissions.read
                || gdt.guest_address.get() != gdtr_base
                || u64::from(gdt.length) <= gdtr_limit
                || !gdt.permissions.read
                || selector == 0
                || selector & 7 != 0
                || current_cs == 0
                || current_cs & 7 != 0
                || current_access != 0x9b
                || current_base != 0
                || current_limit != u32::MAX as u64
                || current_flags != 0xa
                || u64::from(selector & !7) + 7 > gdtr_limit
                || !descriptor_matches
            {
                return Err(invalid(
                    "memory.code",
                    "LRETQ requires exact 48 CB, budget 1, CPL0, a separate 16-byte frame, an in-GDTR accessed CS descriptor, and stop at target RIP",
                ));
            }
        }
        if self.profile == Profile::LongKernelGsBaseV1 {
            let Some(code) = self.memory.iter().find(|region| region.region_id == "code") else {
                return Err(invalid(
                    "memory.code",
                    "required by the kernel-GS-base profile",
                ));
            };
            let bytes = code.initial_bytes_hex.to_ascii_lowercase();
            let (budget, stop_offset) = match bytes.as_str() {
                "0f32" | "0f30" => (1, 2),
                "0f300f01f80f32" => (3, 7),
                "0f300f01f865488b00" => (3, 9),
                _ => {
                    return Err(invalid(
                        "memory.code",
                        "kernel-GS-base profile admits only the four exact MSR/SWAPGS/GS-load programs",
                    ));
                }
            };
            if code.guest_address.get() != self.initial_state.rip.get()
                || !code.permissions.execute
                || self.instruction_budget != budget
                || self.expected_exit != ExpectedExit::StopAddress
                || self.stop_rip.map(HexU64::get)
                    != self.initial_state.rip.get().checked_add(stop_offset)
            {
                return Err(invalid(
                    "memory.code",
                    "kernel-GS-base fixtures require the exact instruction budget and stop RIP",
                ));
            }
        }
        if self.profile == Profile::LongEferRdmsrV1 {
            let code = self
                .memory
                .iter()
                .find(|region| region.region_id == "code")
                .ok_or_else(|| invalid("memory.code", "required by the EFER RDMSR profile"))?;
            let cpl: u8 = self.efer_cpl.expect("validated EFER CPL").into();
            let expected_exit = if cpl == 0 {
                ExpectedExit::StopAddress
            } else {
                ExpectedExit::Exception
            };
            if code.initial_bytes_hex.to_ascii_lowercase() != "0f32"
                || code.guest_address != self.initial_state.rip
                || code.length != 2
                || code.permissions.read
                || code.permissions.write
                || !code.permissions.execute
                || code.comparison_mask_hex.to_ascii_lowercase() != "ffff"
                || self.memory.len() != 1
                || self.instruction_budget != 1
                || self.stop_rip.map(HexU64::get) != self.initial_state.rip.get().checked_add(2)
                || self.expected_exit != expected_exit
                || self.rflags_mask.get() != INTEGER_FLAGS
                || self.initial_state.gpr[1].get() as u32 != 0xc000_0080
            {
                return Err(invalid(
                    "memory.code",
                    "EFER RDMSR requires exact bare 0F 32, budget 1, stop RIP entry+2, and exit matching the CPL0 gate",
                ));
            }
        }
        match (self.profile, self.rdtsc_controls) {
            (Profile::LongRdtscV1, Some(controls)) => {
                let Some(code) = self.memory.iter().find(|region| region.region_id == "code")
                else {
                    return Err(invalid("memory.code", "required by the RDTSC profile"));
                };
                let expected_exit = if controls.cr4_tsd {
                    ExpectedExit::Exception
                } else {
                    ExpectedExit::StopAddress
                };
                let expected_calls = u64::from(!controls.cr4_tsd);
                if code.initial_bytes_hex.to_ascii_lowercase() != "0f31"
                    || code.guest_address.get() != self.initial_state.rip.get()
                    || code.length != 2
                    || !code.permissions.execute
                    || self.instruction_budget != 1
                    || self.stop_rip.map(HexU64::get) != self.initial_state.rip.get().checked_add(2)
                    || self.expected_exit != expected_exit
                    || controls.expected_source_calls != expected_calls
                {
                    return Err(invalid(
                        "memory.code",
                        "RDTSC fixtures require exact bare 0F 31, budget 1, stop RIP entry+2, and source-call/exit expectations consistent with the ring-3 CR4.TSD gate",
                    ));
                }
            }
            (Profile::LongRdtscV1, None) => {
                return Err(invalid("rdtsc_controls", "required by the RDTSC profile"));
            }
            (_, Some(_)) => {
                return Err(invalid(
                    "rdtsc_controls",
                    "only valid for the RDTSC profile",
                ));
            }
            (_, None) => {}
        }
        if matches!(
            self.profile,
            Profile::LongFsLoadV1 | Profile::LongGsLoadV1 | Profile::LongNullDsEsV1
        ) {
            let code = self.memory.iter().find(|region| region.region_id == "code");
            let Some(code) = code else {
                return Err(invalid("memory.code", "required by this profile"));
            };
            let bytes = code.initial_bytes_hex.to_ascii_lowercase();
            let start = self.initial_state.rip.get();
            let admitted = match (self.profile, bytes.as_str()) {
                (Profile::LongFsLoadV1, "8ee0") => (1, 2),
                (Profile::LongFsLoadV1, "8ee064488b00") => (2, 6),
                (Profile::LongGsLoadV1, "8ee8") => (1, 2),
                (Profile::LongGsLoadV1, "8ee865488b00") => (2, 6),
                (Profile::LongNullDsEsV1, "8ed8" | "8ec0") => (1, 2),
                (Profile::LongNullDsEsV1, "8ed88ed8" | "8ed88ec0") => (2, 4),
                _ => {
                    return Err(invalid(
                        "memory.code",
                        "outside the bounded segment-load profile",
                    ));
                }
            };
            let expected_stop = start.checked_add(u64::from(admitted.1));
            if code.guest_address.get() != start
                || code.length != admitted.1
                || !code.permissions.execute
                || self.instruction_budget != admitted.0
                || self.stop_rip.map(HexU64::get) != expected_stop
            {
                return Err(invalid(
                    "memory.code",
                    "address, budget and stop_rip must match the exact admitted program",
                ));
            }
        }
        if self.profile == Profile::LongFsBaseMsrV1 {
            let code = self
                .memory
                .iter()
                .find(|region| region.region_id == "code")
                .ok_or_else(|| invalid("memory.code", "required by the FS_BASE MSR profile"))?;
            let bytes = code.initial_bytes_hex.to_ascii_lowercase();
            let is_single = matches!(bytes.as_str(), "0f32" | "0f30");
            let is_write_then_fs_load = bytes == "0f3064488b00";
            let expected_budget = if is_write_then_fs_load { 2 } else { 1 };
            let expected_length = if is_write_then_fs_load { 6 } else { 2 };
            let valid_exit = self.expected_exit == ExpectedExit::StopAddress
                || (is_write_then_fs_load && self.expected_exit == ExpectedExit::Exception);
            if !(is_single || is_write_then_fs_load)
                || code.guest_address != self.initial_state.rip
                || code.length != expected_length
                || !code.permissions.execute
                || self.instruction_budget != expected_budget
                || self.stop_rip.map(HexU64::get)
                    != self
                        .initial_state
                        .rip
                        .get()
                        .checked_add(u64::from(expected_length))
                || !valid_exit
            {
                return Err(invalid(
                    "memory.code",
                    "FS_BASE MSR fixtures require exact bare 0F 32 or 0F 30 (budget 1), or 0F 30 64 48 8B 00 (budget 2), with stop RIP immediately after the sequence",
                ));
            }
        }
        if self.profile == Profile::LongNativeXorpdXmm0Xmm1V1 {
            let code = self
                .memory
                .iter()
                .find(|region| region.region_id == "code")
                .ok_or_else(|| invalid("memory.code", "required by native XORPD profile"))?;
            let start = self.initial_state.rip.get();
            if code.guest_address.get() != start
                || code.length != 4
                || !code.permissions.execute
                || code.initial_bytes_hex.to_ascii_lowercase() != "660f57c1"
                || self.instruction_budget != 1
                || self.stop_rip.map(HexU64::get) != start.checked_add(4)
                || self.expected_exit != ExpectedExit::StopAddress
            {
                return Err(invalid(
                    "memory.code",
                    "native XORPD requires exactly 66 0F 57 C1, budget 1, and stop RIP entry+4",
                ));
            }
            if self.memory.len() != 1 {
                return Err(invalid(
                    "memory",
                    "native XORPD profile has only the code mapping",
                ));
            }
        }
        if self.profile == Profile::LongXorpdPrerequisitesV1 {
            let controls = self.xorpd_prerequisites.ok_or_else(|| {
                invalid(
                    "xorpd_prerequisites",
                    "required by the XORPD prerequisite profile",
                )
            })?;
            let _ = controls;
            let code = self
                .memory
                .iter()
                .find(|region| region.region_id == "code")
                .ok_or_else(|| {
                    invalid("memory.code", "required by the XORPD prerequisite profile")
                })?;
            let start = self.initial_state.rip.get();
            if code.guest_address.get() != start
                || code.length != 4
                || !code.permissions.execute
                || code.initial_bytes_hex.to_ascii_lowercase() != "660f57c1"
                || self.memory.len() != 1
                || self.instruction_budget != 1
                || self.stop_rip.map(HexU64::get) != start.checked_add(4)
                || !matches!(
                    self.expected_exit,
                    ExpectedExit::Exception | ExpectedExit::StopAddress
                )
            {
                return Err(invalid(
                    "memory.code",
                    "XORPD prerequisite fixtures require exactly 66 0F 57 C1, one executable code mapping, budget 1, and stop RIP entry+4",
                ));
            }
        } else if self.xorpd_prerequisites.is_some() {
            return Err(invalid(
                "xorpd_prerequisites",
                "only valid for the XORPD prerequisite profile",
            ));
        }
        match (self.profile, self.fxsave64_cr0_controls) {
            (Profile::LongFxsave64Cr0GatesV1, Some(_)) => {}
            (Profile::LongFxsave64Cr0GatesV1, None) => {
                return Err(invalid(
                    "fxsave64_cr0_controls",
                    "required by the FXSAVE64 CR0 gate profile",
                ));
            }
            (_, Some(_)) => {
                return Err(invalid(
                    "fxsave64_cr0_controls",
                    "only valid for the FXSAVE64 CR0 gate profile",
                ));
            }
            (_, None) => {}
        }
        match (self.profile, self.fxrstor64_cr0_controls) {
            (Profile::LongFxrstor64Cr0GatesV1, Some(_)) => {}
            (Profile::LongFxrstor64Cr0GatesV1, None) => {
                return Err(invalid(
                    "fxrstor64_cr0_controls",
                    "required by the FXRSTOR64 CR0 gate profile",
                ));
            }
            (_, Some(_)) => {
                return Err(invalid(
                    "fxrstor64_cr0_controls",
                    "only valid for the FXRSTOR64 CR0 gate profile",
                ));
            }
            (_, None) => {}
        }
        if matches!(
            self.profile,
            Profile::LongMinssInvalidControlsV1 | Profile::LongMinssUnmaskedDenormalV1
        ) {
            let controls = self.minss_invalid_controls.ok_or_else(|| {
                invalid(
                    "minss_invalid_controls",
                    "required by the MINSS invalid-controls profile",
                )
            })?;
            let _ = controls;
            let code = self
                .memory
                .iter()
                .find(|region| region.region_id == "code")
                .ok_or_else(|| {
                    invalid(
                        "memory.code",
                        "required by the MINSS invalid-controls profile",
                    )
                })?;
            let start = self.initial_state.rip.get();
            if code.guest_address.get() != start
                || code.length != 4
                || !code.permissions.execute
                || code.initial_bytes_hex.to_ascii_lowercase() != "f30f5dc1"
                || self.memory.len() != 1
                || self.instruction_budget != 1
                || self.stop_rip.map(HexU64::get) != start.checked_add(4)
                || !matches!(
                    self.expected_exit,
                    ExpectedExit::Exception | ExpectedExit::StopAddress
                )
            {
                return Err(invalid(
                    "memory.code",
                    "MINSS invalid-controls fixtures require exactly F3 0F 5D C1, one executable code mapping, budget 1, and stop RIP entry+4",
                ));
            }
            let mxcsr = self
                .initial_state
                .sse
                .as_ref()
                .expect("MINSS SSE state required above")
                .mxcsr
                .get();
            let masks = if self.profile == Profile::LongMinssInvalidControlsV1 {
                0x0000_1f00
            } else {
                0x0000_1e00
            };
            if mxcsr & 0xffff_0000 != 0 || mxcsr & masks != masks || mxcsr & 0x0000_8040 != 0 {
                return Err(invalid(
                    "state.sse.mxcsr",
                    "MINSS fault-control profiles require reserved bits clear, ZE/OE/UE/PE masks set, DAZ/FTZ clear, and allow IM, DM, RC and old sticky bits according to profile",
                ));
            }
        } else if self.minss_invalid_controls.is_some() {
            return Err(invalid(
                "minss_invalid_controls",
                "only valid for the MINSS invalid-controls profile",
            ));
        }
        if self.profile == Profile::LongMinsdUnmaskedDenormalV1 {
            let _ = self.minsd_unmasked_controls.ok_or_else(|| {
                invalid("minsd_unmasked_controls", "required by the MINSD profile")
            })?;
            let code = self
                .memory
                .iter()
                .find(|region| region.region_id == "code")
                .ok_or_else(|| invalid("memory.code", "required by the MINSD profile"))?;
            let start = self.initial_state.rip.get();
            if code.guest_address.get() != start
                || code.length != 4
                || !code.permissions.execute
                || !code.initial_bytes_hex.eq_ignore_ascii_case("f20f5dc1")
                || self.memory.len() != 1
                || self.instruction_budget != 1
                || self.stop_rip.map(HexU64::get) != start.checked_add(4)
                || !matches!(
                    self.expected_exit,
                    ExpectedExit::Exception | ExpectedExit::StopAddress
                )
            {
                return Err(invalid(
                    "memory.code",
                    "MINSD fixtures require exactly F2 0F 5D C1, one executable code mapping, budget 1, and stop RIP entry+4",
                ));
            }
            let mxcsr = self
                .initial_state
                .sse
                .as_ref()
                .expect("MINSD SSE state required above")
                .mxcsr
                .get();
            if mxcsr & 0xffff_0000 != 0
                || mxcsr & 0x0000_1e00 != 0x0000_1e00
                || mxcsr & 0x0000_8040 != 0
            {
                return Err(invalid(
                    "state.sse.mxcsr",
                    "MINSD requires reserved bits clear, ZE/OE/UE/PE masks set, and DAZ/FTZ clear; IM and DM may vary",
                ));
            }
        } else if self.minsd_unmasked_controls.is_some() {
            return Err(invalid(
                "minsd_unmasked_controls",
                "only valid for the MINSD unmasked-denormal profile",
            ));
        }
        if self.profile == Profile::LongMaxsdUnmaskedDenormalV1 {
            let _ = self.maxsd_unmasked_controls.ok_or_else(|| {
                invalid("maxsd_unmasked_controls", "required by the MAXSD profile")
            })?;
            let code = self
                .memory
                .iter()
                .find(|region| region.region_id == "code")
                .ok_or_else(|| invalid("memory.code", "required by the MAXSD profile"))?;
            let start = self.initial_state.rip.get();
            if code.guest_address.get() != start
                || code.length != 4
                || !code.permissions.execute
                || !code.initial_bytes_hex.eq_ignore_ascii_case("f20f5fc1")
                || self.memory.len() != 1
                || self.instruction_budget != 1
                || self.stop_rip.map(HexU64::get) != start.checked_add(4)
                || !matches!(
                    self.expected_exit,
                    ExpectedExit::Exception | ExpectedExit::StopAddress
                )
            {
                return Err(invalid(
                    "memory.code",
                    "MAXSD fixtures require exactly F2 0F 5F C1, one executable code mapping, budget 1, and stop RIP entry+4",
                ));
            }
            let mxcsr = self
                .initial_state
                .sse
                .as_ref()
                .expect("MAXSD SSE state required above")
                .mxcsr
                .get();
            if mxcsr & 0xffff_0000 != 0
                || mxcsr & 0x0000_1e00 != 0x0000_1e00
                || mxcsr & 0x0000_8040 != 0
            {
                return Err(invalid(
                    "state.sse.mxcsr",
                    "MAXSD requires reserved bits clear, ZE/OE/UE/PE masks set, and DAZ/FTZ clear; IM and DM may vary",
                ));
            }
        } else if self.maxsd_unmasked_controls.is_some() {
            return Err(invalid(
                "maxsd_unmasked_controls",
                "only valid for the MAXSD unmasked-denormal profile",
            ));
        }
        if self.profile == Profile::LongMinpdUnmaskedDenormalV1 {
            let _ = self.minpd_unmasked_controls.ok_or_else(|| {
                invalid("minpd_unmasked_controls", "required by the MINPD profile")
            })?;
            let code = self
                .memory
                .iter()
                .find(|region| region.region_id == "code")
                .ok_or_else(|| invalid("memory.code", "required by the MINPD profile"))?;
            let start = self.initial_state.rip.get();
            if code.guest_address.get() != start
                || code.length != 4
                || !code.permissions.execute
                || !code.initial_bytes_hex.eq_ignore_ascii_case("660f5dc1")
                || self.memory.len() != 1
                || self.instruction_budget != 1
                || self.stop_rip.map(HexU64::get) != start.checked_add(4)
                || !matches!(
                    self.expected_exit,
                    ExpectedExit::Exception | ExpectedExit::StopAddress
                )
            {
                return Err(invalid(
                    "memory.code",
                    "MINPD fixtures require exactly 66 0F 5D C1, one executable code mapping, budget 1, and stop RIP entry+4",
                ));
            }
            let mxcsr = self
                .initial_state
                .sse
                .as_ref()
                .expect("MINPD SSE state required above")
                .mxcsr
                .get();
            if mxcsr & 0xffff_0000 != 0
                || mxcsr & 0x0000_1e00 != 0x0000_1e00
                || mxcsr & 0x0000_8040 != 0
            {
                return Err(invalid(
                    "state.sse.mxcsr",
                    "MINPD requires reserved bits clear, ZE/OE/UE/PE masks set, and DAZ/FTZ clear; IM and DM may vary",
                ));
            }
        } else if self.minpd_unmasked_controls.is_some() {
            return Err(invalid(
                "minpd_unmasked_controls",
                "only valid for the MINPD unmasked-denormal profile",
            ));
        }
        if self.profile == Profile::LongMinpsUnmaskedDenormalV1 {
            let _ = self.minps_unmasked_controls.ok_or_else(|| {
                invalid("minps_unmasked_controls", "required by the MINPS profile")
            })?;
            let code = self
                .memory
                .iter()
                .find(|region| region.region_id == "code")
                .ok_or_else(|| invalid("memory.code", "required by the MINPS profile"))?;
            let start = self.initial_state.rip.get();
            if code.guest_address.get() != start
                || code.length != 3
                || !code.permissions.execute
                || !code.initial_bytes_hex.eq_ignore_ascii_case("0f5dc1")
                || self.memory.len() != 1
                || self.instruction_budget != 1
                || self.stop_rip.map(HexU64::get) != start.checked_add(3)
                || !matches!(
                    self.expected_exit,
                    ExpectedExit::Exception | ExpectedExit::StopAddress
                )
            {
                return Err(invalid(
                    "memory.code",
                    "MINPS fixtures require exactly 0F 5D C1, one executable code mapping, budget 1, and stop RIP entry+3",
                ));
            }
            let mxcsr = self
                .initial_state
                .sse
                .as_ref()
                .expect("MINPS SSE state required above")
                .mxcsr
                .get();
            if mxcsr & 0xffff_0000 != 0
                || mxcsr & 0x0000_1e00 != 0x0000_1e00
                || mxcsr & 0x0000_8040 != 0
            {
                return Err(invalid(
                    "state.sse.mxcsr",
                    "MINPS requires reserved bits clear, ZE/OE/UE/PE masks set, and DAZ/FTZ clear; IM, DM, RC and sticky status may vary",
                ));
            }
        } else if self.minps_unmasked_controls.is_some() {
            return Err(invalid(
                "minps_unmasked_controls",
                "only valid for the MINPS unmasked-denormal profile",
            ));
        }
        if self.profile == Profile::LongMaxpsUnmaskedDenormalV1 {
            let _ = self.maxps_unmasked_controls.ok_or_else(|| {
                invalid("maxps_unmasked_controls", "required by the MAXPS profile")
            })?;
            let code = self
                .memory
                .iter()
                .find(|region| region.region_id == "code")
                .ok_or_else(|| invalid("memory.code", "required by the MAXPS profile"))?;
            let start = self.initial_state.rip.get();
            if code.guest_address.get() != start
                || code.length != 3
                || !code.permissions.execute
                || !code.initial_bytes_hex.eq_ignore_ascii_case("0f5fc1")
                || self.memory.len() != 1
                || self.instruction_budget != 1
                || self.stop_rip.map(HexU64::get) != start.checked_add(3)
                || !matches!(
                    self.expected_exit,
                    ExpectedExit::Exception | ExpectedExit::StopAddress
                )
            {
                return Err(invalid(
                    "memory.code",
                    "MAXPS fixtures require exactly 0F 5F C1, one executable code mapping, budget 1, and stop RIP entry+3",
                ));
            }
            let mxcsr = self
                .initial_state
                .sse
                .as_ref()
                .expect("MAXPS SSE state required above")
                .mxcsr
                .get();
            if mxcsr & 0xffff_0000 != 0
                || mxcsr & 0x0000_1e00 != 0x0000_1e00
                || mxcsr & 0x0000_8040 != 0
            {
                return Err(invalid(
                    "state.sse.mxcsr",
                    "MAXPS requires reserved bits clear, ZE/OE/UE/PE masks set, and DAZ/FTZ clear; IM, DM, RC and sticky status may vary",
                ));
            }
        } else if self.maxps_unmasked_controls.is_some() {
            return Err(invalid(
                "maxps_unmasked_controls",
                "only valid for the MAXPS unmasked-denormal profile",
            ));
        }
        if self.profile == Profile::LongMaxpdUnmaskedDenormalV1 {
            let _ = self.maxpd_unmasked_controls.ok_or_else(|| {
                invalid("maxpd_unmasked_controls", "required by the MAXPD profile")
            })?;
            let code = self
                .memory
                .iter()
                .find(|region| region.region_id == "code")
                .ok_or_else(|| invalid("memory.code", "required by the MAXPD profile"))?;
            let start = self.initial_state.rip.get();
            if code.guest_address.get() != start
                || code.length != 4
                || !code.permissions.execute
                || !code.initial_bytes_hex.eq_ignore_ascii_case("660f5fc1")
                || self.memory.len() != 1
                || self.instruction_budget != 1
                || self.stop_rip.map(HexU64::get) != start.checked_add(4)
                || !matches!(
                    self.expected_exit,
                    ExpectedExit::Exception | ExpectedExit::StopAddress
                )
            {
                return Err(invalid(
                    "memory.code",
                    "MAXPD fixtures require exactly 66 0F 5F C1, one executable code mapping, budget 1, and stop RIP entry+4",
                ));
            }
            let mxcsr = self
                .initial_state
                .sse
                .as_ref()
                .expect("MAXPD SSE state required above")
                .mxcsr
                .get();
            if mxcsr & 0xffff_0000 != 0
                || mxcsr & 0x0000_1e00 != 0x0000_1e00
                || mxcsr & 0x0000_8040 != 0
            {
                return Err(invalid(
                    "state.sse.mxcsr",
                    "MAXPD requires reserved bits clear, ZE/OE/UE/PE masks set, and DAZ/FTZ clear; IM and DM may vary",
                ));
            }
        } else if self.maxpd_unmasked_controls.is_some() {
            return Err(invalid(
                "maxpd_unmasked_controls",
                "only valid for the MAXPD unmasked-denormal profile",
            ));
        }
        if matches!(
            self.profile,
            Profile::LongCvtdq2pdRegExactV1 | Profile::LongCvtdq2pdRegistersV1
        ) {
            let _ = self
                .cvtdq2pd_controls
                .ok_or_else(|| invalid("cvtdq2pd_controls", "required by the CVTDQ2PD profile"))?;
            let code = self
                .memory
                .iter()
                .find(|region| region.region_id == "code")
                .ok_or_else(|| invalid("memory.code", "required by the CVTDQ2PD profile"))?;
            let start = self.initial_state.rip.get();
            let (destination, source, length) = cvtdq2pd_register_encoding(&code.initial_bytes_hex)
                .ok_or_else(|| {
                    invalid(
                        "memory.code",
                        "outside the canonical CVTDQ2PD register profile",
                    )
                })?;
            if self.profile == Profile::LongCvtdq2pdRegExactV1
                && (destination != 0 || source != 1 || length != 4)
            {
                return Err(invalid(
                    "memory.code",
                    "exact CVTDQ2PD profile requires XMM0,XMM1",
                ));
            }
            let stop = start.checked_add(u64::from(length));
            if code.guest_address.get() != start
                || code.length != length
                || !code.permissions.execute
                || self.memory.len() != 1
                || self.instruction_budget != 1
                || stop.is_none()
                || self.stop_rip.map(HexU64::get) != stop
                || !matches!(
                    self.expected_exit,
                    ExpectedExit::Exception | ExpectedExit::StopAddress
                )
            {
                return Err(invalid(
                    "memory.code",
                    "CVTDQ2PD fixtures require one canonical register encoding, one executable code mapping, budget 1, and stop RIP entry+instruction length",
                ));
            }
            let mxcsr = self
                .initial_state
                .sse
                .as_ref()
                .expect("SSE required for CVTDQ2PD")
                .mxcsr
                .get();
            if mxcsr & !0x0000_ffbf != 0 {
                return Err(invalid(
                    "state.sse.mxcsr",
                    "CVTDQ2PD requires MXCSR reserved bits clear",
                ));
            }
        } else if self.cvtdq2pd_controls.is_some() {
            return Err(invalid(
                "cvtdq2pd_controls",
                "only valid for CVTDQ2PD profile",
            ));
        }
        if self.profile == Profile::LongCvtdq2psRegistersV1 {
            let _ = self
                .cvtdq2ps_controls
                .ok_or_else(|| invalid("cvtdq2ps_controls", "required by the CVTDQ2PS profile"))?;
            let code = self
                .memory
                .iter()
                .find(|region| region.region_id == "code")
                .ok_or_else(|| invalid("memory.code", "required by the CVTDQ2PS profile"))?;
            let (_, _, length) =
                cvtdq2ps_register_encoding(&code.initial_bytes_hex).ok_or_else(|| {
                    invalid("memory.code", "outside canonical CVTDQ2PS register profile")
                })?;
            let start = self.initial_state.rip.get();
            if code.guest_address.get() != start
                || code.length != length
                || !code.permissions.execute
                || self.memory.len() != 1
                || self.instruction_budget != 1
                || self.stop_rip.map(HexU64::get) != start.checked_add(u64::from(length))
                || !matches!(
                    self.expected_exit,
                    ExpectedExit::Exception | ExpectedExit::StopAddress
                )
            {
                return Err(invalid(
                    "memory.code",
                    "CVTDQ2PS requires one canonical executable encoding, budget 1, and stop RIP entry+length",
                ));
            }
            let mxcsr = self
                .initial_state
                .sse
                .as_ref()
                .expect("SSE required by CVTDQ2PS")
                .mxcsr
                .get();
            if mxcsr & !0x0000_ffbf != 0 {
                return Err(invalid(
                    "state.sse.mxcsr",
                    "CVTDQ2PS requires supported MXCSR bits only",
                ));
            }
        } else if self.cvtdq2ps_controls.is_some() {
            return Err(invalid(
                "cvtdq2ps_controls",
                "only valid for CVTDQ2PS register-space profile",
            ));
        }
        if self.profile == Profile::LongCvtss2sdRegistersV1 {
            let _ = self
                .cvtss2sd_controls
                .ok_or_else(|| invalid("cvtss2sd_controls", "required by the CVTSS2SD profile"))?;
            let code = self
                .memory
                .iter()
                .find(|region| region.region_id == "code")
                .ok_or_else(|| invalid("memory.code", "required by the CVTSS2SD profile"))?;
            let (_, _, length) =
                cvtss2sd_register_encoding(&code.initial_bytes_hex).ok_or_else(|| {
                    invalid("memory.code", "outside canonical CVTSS2SD register profile")
                })?;
            let start = self.initial_state.rip.get();
            if code.guest_address.get() != start
                || code.length != length
                || !code.permissions.execute
                || self.memory.len() != 1
                || self.instruction_budget != 1
                || self.stop_rip.map(HexU64::get) != start.checked_add(u64::from(length))
                || !matches!(
                    self.expected_exit,
                    ExpectedExit::Exception | ExpectedExit::StopAddress
                )
            {
                return Err(invalid(
                    "memory.code",
                    "CVTSS2SD requires one canonical executable encoding, budget 1, and stop RIP entry+length",
                ));
            }
            let mxcsr = self
                .initial_state
                .sse
                .as_ref()
                .expect("SSE required by CVTSS2SD")
                .mxcsr
                .get();
            if mxcsr & !0x0000_ffbf != 0 || mxcsr & (1 << 6) != 0 {
                return Err(invalid(
                    "state.sse.mxcsr",
                    "CVTSS2SD requires supported MXCSR bits only and DAZ clear",
                ));
            }
        } else if self.cvtss2sd_controls.is_some() {
            return Err(invalid(
                "cvtss2sd_controls",
                "only valid for CVTSS2SD register-space profile",
            ));
        }
        if self.profile == Profile::LongCvtps2pdRegistersV1 {
            let _ = self
                .cvtps2pd_controls
                .ok_or_else(|| invalid("cvtps2pd_controls", "required by the CVTPS2PD profile"))?;
            let code = self
                .memory
                .iter()
                .find(|region| region.region_id == "code")
                .ok_or_else(|| invalid("memory.code", "required by the CVTPS2PD profile"))?;
            let (_, _, length) =
                cvtps2pd_register_encoding(&code.initial_bytes_hex).ok_or_else(|| {
                    invalid("memory.code", "outside canonical CVTPS2PD register profile")
                })?;
            let start = self.initial_state.rip.get();
            if code.guest_address.get() != start
                || code.length != length
                || !code.permissions.execute
                || self.memory.len() != 1
                || self.instruction_budget != 1
                || self.stop_rip.map(HexU64::get) != start.checked_add(u64::from(length))
                || !matches!(
                    self.expected_exit,
                    ExpectedExit::Exception | ExpectedExit::StopAddress
                )
            {
                return Err(invalid(
                    "memory.code",
                    "CVTPS2PD requires one canonical executable encoding, budget 1, and stop RIP entry+length",
                ));
            }
            let mxcsr = self
                .initial_state
                .sse
                .as_ref()
                .expect("SSE required by CVTPS2PD")
                .mxcsr
                .get();
            if mxcsr & !0x0000_ffbf != 0 || mxcsr & (1 << 6) != 0 {
                return Err(invalid(
                    "state.sse.mxcsr",
                    "CVTPS2PD requires supported MXCSR bits only and DAZ clear",
                ));
            }
        } else if self.cvtps2pd_controls.is_some() {
            return Err(invalid(
                "cvtps2pd_controls",
                "only valid for CVTPS2PD register-space profile",
            ));
        }
        if self.profile == Profile::LongComissUcomissRegistersV1 {
            let _ = self.comiss_ucomiss_controls.ok_or_else(|| {
                invalid(
                    "comiss_ucomiss_controls",
                    "required by COMISS/UCOMISS profile",
                )
            })?;
            let code = self
                .memory
                .iter()
                .find(|region| region.region_id == "code")
                .ok_or_else(|| invalid("memory.code", "required by COMISS/UCOMISS profile"))?;
            let (_, _, _, length) = comiss_ucomiss_register_encoding(&code.initial_bytes_hex)
                .ok_or_else(|| {
                    invalid(
                        "memory.code",
                        "outside canonical COMISS/UCOMISS register profile",
                    )
                })?;
            let start = self.initial_state.rip.get();
            if code.guest_address.get() != start
                || code.length != length
                || !code.permissions.execute
                || self.memory.len() != 1
                || self.instruction_budget != 1
                || self.stop_rip.map(HexU64::get) != start.checked_add(u64::from(length))
                || !matches!(
                    self.expected_exit,
                    ExpectedExit::Exception | ExpectedExit::StopAddress
                )
            {
                return Err(invalid(
                    "memory.code",
                    "COMISS/UCOMISS requires one canonical executable encoding, budget 1, and stop RIP entry+length",
                ));
            }
            let mxcsr = self
                .initial_state
                .sse
                .as_ref()
                .expect("SSE required by COMISS/UCOMISS")
                .mxcsr
                .get();
            if mxcsr & !0x0000_ffbf != 0 || mxcsr & (1 << 6) != 0 {
                return Err(invalid(
                    "state.sse.mxcsr",
                    "COMISS/UCOMISS requires supported MXCSR bits only and DAZ clear",
                ));
            }
        } else if self.comiss_ucomiss_controls.is_some() {
            return Err(invalid(
                "comiss_ucomiss_controls",
                "only valid for COMISS/UCOMISS register-space profile",
            ));
        }
        if self.profile == Profile::LongComisdUcomisdRegistersV1 {
            let _ = self.comisd_ucomisd_controls.ok_or_else(|| {
                invalid(
                    "comisd_ucomisd_controls",
                    "required by COMISD/UCOMISD profile",
                )
            })?;
            let code = self
                .memory
                .iter()
                .find(|region| region.region_id == "code")
                .ok_or_else(|| invalid("memory.code", "required by COMISD/UCOMISD profile"))?;
            let (_, _, _, length) = comisd_ucomisd_register_encoding(&code.initial_bytes_hex)
                .ok_or_else(|| {
                    invalid(
                        "memory.code",
                        "outside canonical COMISD/UCOMISD register profile",
                    )
                })?;
            let start = self.initial_state.rip.get();
            if code.guest_address.get() != start
                || code.length != length
                || !code.permissions.execute
                || self.memory.len() != 1
                || self.instruction_budget != 1
                || self.stop_rip.map(HexU64::get) != start.checked_add(u64::from(length))
                || !matches!(
                    self.expected_exit,
                    ExpectedExit::Exception | ExpectedExit::StopAddress
                )
            {
                return Err(invalid(
                    "memory.code",
                    "COMISD/UCOMISD requires one canonical executable encoding, budget 1, and stop RIP entry+length",
                ));
            }
            let mxcsr = self
                .initial_state
                .sse
                .as_ref()
                .expect("SSE required by COMISD/UCOMISD")
                .mxcsr
                .get();
            if mxcsr & !0x0000_ffbf != 0 || mxcsr & (1 << 6) != 0 {
                return Err(invalid(
                    "state.sse.mxcsr",
                    "COMISD/UCOMISD requires supported MXCSR bits only and DAZ clear",
                ));
            }
        } else if self.comisd_ucomisd_controls.is_some() {
            return Err(invalid(
                "comisd_ucomisd_controls",
                "only valid for COMISD/UCOMISD register-space profile",
            ));
        }
        if self.profile == Profile::LongCvtsi2sdR32ExactV1 {
            let _ = self.cvtsi2sd_r32_controls.ok_or_else(|| {
                invalid(
                    "cvtsi2sd_r32_controls",
                    "required by the CVTSI2SD r32 profile",
                )
            })?;
            let code = self
                .memory
                .iter()
                .find(|region| region.region_id == "code")
                .ok_or_else(|| invalid("memory.code", "required by the CVTSI2SD r32 profile"))?;
            let start = self.initial_state.rip.get();
            if code.guest_address.get() != start
                || code.length != 4
                || !code.permissions.execute
                || !code.initial_bytes_hex.eq_ignore_ascii_case("f20f2ac0")
                || self.memory.len() != 1
                || self.instruction_budget != 1
                || self.stop_rip.map(HexU64::get) != start.checked_add(4)
                || !matches!(
                    self.expected_exit,
                    ExpectedExit::Exception | ExpectedExit::StopAddress
                )
            {
                return Err(invalid(
                    "memory.code",
                    "CVTSI2SD r32 fixtures require exactly F2 0F 2A C0, one executable code mapping, budget 1, and stop RIP entry+4",
                ));
            }
            let mxcsr = self
                .initial_state
                .sse
                .as_ref()
                .expect("SSE required for CVTSI2SD")
                .mxcsr
                .get();
            if mxcsr & !0x0000_ffbf != 0 {
                return Err(invalid(
                    "state.sse.mxcsr",
                    "CVTSI2SD requires supported MXCSR bits only",
                ));
            }
        } else if self.cvtsi2sd_r32_controls.is_some()
            && self.profile != Profile::LongCvtsi2sdRegistersV1
        {
            return Err(invalid(
                "cvtsi2sd_r32_controls",
                "only valid for CVTSI2SD r32 profile",
            ));
        }
        if self.profile == Profile::LongCvtsi2sdR64ExactV1 {
            let _ = self.cvtsi2sd_r64_controls.ok_or_else(|| {
                invalid(
                    "cvtsi2sd_r64_controls",
                    "required by the CVTSI2SD r64 profile",
                )
            })?;
            let code = self
                .memory
                .iter()
                .find(|region| region.region_id == "code")
                .ok_or_else(|| invalid("memory.code", "required for the CVTSI2SD r64 profile"))?;
            let start = self.initial_state.rip.get();
            if code.guest_address.get() != start
                || code.length != 5
                || !code.permissions.execute
                || !code.initial_bytes_hex.eq_ignore_ascii_case("f2480f2ac0")
                || self.memory.len() != 1
                || self.instruction_budget != 1
                || self.stop_rip.map(HexU64::get) != start.checked_add(5)
                || !matches!(
                    self.expected_exit,
                    ExpectedExit::Exception | ExpectedExit::StopAddress
                )
            {
                return Err(invalid(
                    "memory.code",
                    "CVTSI2SD r64 fixtures require exactly F2 48 0F 2A C0, one executable code mapping, budget 1, and stop RIP entry+5",
                ));
            }
            let mxcsr = self
                .initial_state
                .sse
                .as_ref()
                .expect("SSE required for CVTSI2SD")
                .mxcsr
                .get();
            if mxcsr & !0x0000_ffbf != 0 {
                return Err(invalid(
                    "state.sse.mxcsr",
                    "CVTSI2SD r64 requires supported MXCSR bits only",
                ));
            }
        } else if self.cvtsi2sd_r64_controls.is_some()
            && self.profile != Profile::LongCvtsi2sdRegistersV1
        {
            return Err(invalid(
                "cvtsi2sd_r64_controls",
                "only valid for the CVTSI2SD r64 profile",
            ));
        }
        if self.profile == Profile::LongCvtsi2ssRegistersV1 {
            let _ = self.cvtsi2ss_controls.ok_or_else(|| {
                invalid(
                    "cvtsi2ss_controls",
                    "required by CVTSI2SS register-space profile",
                )
            })?;
            let code = self
                .memory
                .iter()
                .find(|region| region.region_id == "code")
                .ok_or_else(|| {
                    invalid("memory.code", "required by CVTSI2SS register-space profile")
                })?;
            let (_, _, _, length) = cvtsi2ss_register_encoding(&code.initial_bytes_hex)
                .ok_or_else(|| {
                    invalid("memory.code", "outside canonical CVTSI2SS register space")
                })?;
            let start = self.initial_state.rip.get();
            let bytes = decode_bytes(&code.initial_bytes_hex)?;
            if code.guest_address.get() != start
                || code.length != length
                || !code.permissions.execute
                || self.memory.len() != 1
                || self.instruction_budget != 1
                || self.stop_rip.map(HexU64::get) != start.checked_add(u64::from(length))
                || bytes.len() != length as usize
                || !matches!(
                    self.expected_exit,
                    ExpectedExit::Exception | ExpectedExit::StopAddress
                )
            {
                return Err(invalid(
                    "memory.code",
                    "CVTSI2SS register-space fixtures require one canonical executable form, budget 1, and stop RIP entry+length",
                ));
            }
            let mxcsr = self
                .initial_state
                .sse
                .as_ref()
                .expect("SSE required by profile")
                .mxcsr
                .get();
            if mxcsr & !0x0000_ffbf != 0 {
                return Err(invalid(
                    "state.sse.mxcsr",
                    "CVTSI2SS requires supported MXCSR bits only",
                ));
            }
        } else if self.cvtsi2ss_controls.is_some() {
            return Err(invalid(
                "cvtsi2ss_controls",
                "only valid for CVTSI2SS register-space profile",
            ));
        }
        if self.profile == Profile::LongCvtsi2sdRegistersV1 {
            let r32 = self.cvtsi2sd_r32_controls.ok_or_else(|| {
                invalid(
                    "cvtsi2sd_r32_controls",
                    "required by CVTSI2SD register-space profile",
                )
            })?;
            let r64 = self.cvtsi2sd_r64_controls.ok_or_else(|| {
                invalid(
                    "cvtsi2sd_r64_controls",
                    "required by CVTSI2SD register-space profile",
                )
            })?;
            if r32 != r64 {
                return Err(invalid(
                    "cvtsi2sd_controls",
                    "r32/r64 control echoes must match",
                ));
            }
            let code = self
                .memory
                .iter()
                .find(|region| region.region_id == "code")
                .ok_or_else(|| {
                    invalid("memory.code", "required by CVTSI2SD register-space profile")
                })?;
            let bytes = decode_bytes(&code.initial_bytes_hex)?;
            let (_, _, _, length) = cvtsi2sd_register_encoding(&code.initial_bytes_hex)
                .ok_or_else(|| {
                    invalid("memory.code", "outside canonical CVTSI2SD register space")
                })?;
            let start = self.initial_state.rip.get();
            if code.guest_address.get() != start
                || code.length != length
                || !code.permissions.execute
                || self.memory.len() != 1
                || self.instruction_budget != 1
                || self.stop_rip.map(HexU64::get) != start.checked_add(u64::from(length))
                || !matches!(
                    self.expected_exit,
                    ExpectedExit::Exception | ExpectedExit::StopAddress
                )
                || bytes.len() != length as usize
            {
                return Err(invalid(
                    "memory.code",
                    "CVTSI2SD register-space fixtures require one canonical executable form, budget 1, and stop RIP entry+length",
                ));
            }
            let mxcsr = self
                .initial_state
                .sse
                .as_ref()
                .ok_or_else(|| invalid("state.sse", "required"))?
                .mxcsr
                .get();
            if mxcsr & !0x0000_ffbf != 0 {
                return Err(invalid(
                    "state.sse.mxcsr",
                    "CVTSI2SD requires supported MXCSR bits only",
                ));
            }
        }
        if self.profile == Profile::LongMaxssUnmaskedDenormalV1 {
            let _ = self.maxss_unmasked_controls.ok_or_else(|| {
                invalid("maxss_unmasked_controls", "required by the MAXSS profile")
            })?;
            let code = self
                .memory
                .iter()
                .find(|region| region.region_id == "code")
                .ok_or_else(|| invalid("memory.code", "required by the MAXSS profile"))?;
            let start = self.initial_state.rip.get();
            if code.guest_address.get() != start
                || code.length != 4
                || !code.permissions.execute
                || !code.initial_bytes_hex.eq_ignore_ascii_case("f30f5fc1")
                || self.memory.len() != 1
                || self.instruction_budget != 1
                || self.stop_rip.map(HexU64::get) != start.checked_add(4)
                || !matches!(
                    self.expected_exit,
                    ExpectedExit::Exception | ExpectedExit::StopAddress
                )
            {
                return Err(invalid(
                    "memory.code",
                    "MAXSS fixtures require exactly F3 0F 5F C1, one executable code mapping, budget 1, and stop RIP entry+4",
                ));
            }
            let mxcsr = self
                .initial_state
                .sse
                .as_ref()
                .expect("MAXSS SSE state required above")
                .mxcsr
                .get();
            if mxcsr & 0xffff_0000 != 0
                || mxcsr & 0x0000_1e00 != 0x0000_1e00
                || mxcsr & 0x0000_8040 != 0
            {
                return Err(invalid(
                    "state.sse.mxcsr",
                    "MAXSS requires reserved bits clear, ZE/OE/UE/PE masks set, and DAZ/FTZ clear; IM and DM may vary",
                ));
            }
        } else if self.maxss_unmasked_controls.is_some() {
            return Err(invalid(
                "maxss_unmasked_controls",
                "only valid for the MAXSS unmasked-denormal profile",
            ));
        }
        if self.profile == Profile::LongCr0Cr4FpInitFxrstor64MinssV1 {
            let code = self.memory.iter().find(|region| region.region_id == "code");
            let image = self
                .memory
                .iter()
                .find(|region| region.region_id == "image");
            let expected_program =
                "0f20c04883e0f30f22c0dbe30f20e0480d000600000f22e0660f57c1480fae4b40f30f5dc1";
            let exact_code = code.is_some_and(|region| {
                region.guest_address == self.initial_state.rip
                    && region.length == 37
                    && region.permissions
                        == MemoryPermissions {
                            read: false,
                            write: false,
                            execute: true,
                        }
                    && region
                        .initial_bytes_hex
                        .eq_ignore_ascii_case(expected_program)
                    && region
                        .comparison_mask_hex
                        .eq_ignore_ascii_case(&"ff".repeat(37))
            });
            let exact_image = image.is_some_and(|region| {
                region.guest_address.get() == 0x660
                    && region.length == 512
                    && region.permissions
                        == MemoryPermissions {
                            read: true,
                            write: false,
                            execute: false,
                        }
                    && region
                        .comparison_mask_hex
                        .eq_ignore_ascii_case(&"ff".repeat(512))
            });
            let image_bytes = image.and_then(|region| decode_bytes(&region.initial_bytes_hex).ok());
            let valid_mxcsr = image_bytes.as_ref().is_some_and(|bytes| {
                bytes.get(24..28).is_some_and(|value| {
                    let mxcsr = u32::from_le_bytes(value.try_into().unwrap());
                    mxcsr & !od_isa::TARGET_FXSAVE_MXCSR_MASK == 0
                        && mxcsr & 0xffff_0000 == 0
                        && mxcsr & 0x0000_1e00 == 0x0000_1e00
                        && mxcsr & 0x0000_8040 == 0
                })
            });
            let valid_operands = image_bytes.as_ref().is_some_and(|bytes| {
                let Some(initial_sse) = self.initial_state.sse.as_ref() else {
                    return false;
                };
                let initial_xmm0 = decode_bytes(&initial_sse.xmm[0]).ok();
                let initial_xmm1 = decode_bytes(&initial_sse.xmm[1]).ok();
                bytes.get(160..176) != initial_xmm0.as_deref()
                    && bytes.get(176..192) != initial_xmm1.as_deref()
            });
            let expected_outcome = image_bytes.as_ref().and_then(|bytes| {
                let mxcsr = u32::from_le_bytes(bytes.get(24..28)?.try_into().ok()?);
                let lhs =
                    Binary32::from_bits(u32::from_le_bytes(bytes.get(160..164)?.try_into().ok()?));
                let rhs =
                    Binary32::from_bits(u32::from_le_bytes(bytes.get(176..180)?.try_into().ok()?));
                let invalid = matches!(
                    lhs.class(),
                    BinaryClass::QuietNaN | BinaryClass::SignalingNaN
                ) || matches!(
                    rhs.class(),
                    BinaryClass::QuietNaN | BinaryClass::SignalingNaN
                );
                let denormal =
                    lhs.class() == BinaryClass::Subnormal || rhs.class() == BinaryClass::Subnormal;
                Some(
                    if (invalid && mxcsr & (1 << 7) == 0) || (denormal && mxcsr & (1 << 8) == 0) {
                        ExpectedExit::Exception
                    } else {
                        ExpectedExit::StopAddress
                    },
                )
            });
            if !exact_code
                || !exact_image
                || !valid_mxcsr
                || !valid_operands
                || self.memory.len() != 2
                || self.cli_cpl != Some(CliCpl::Cpl0)
                || self.initial_state.gpr[3].get() != 0x620
                || self.instruction_budget != 10
                || self.stop_rip.map(HexU64::get) != self.initial_state.rip.get().checked_add(37)
                || expected_outcome != Some(self.expected_exit)
                || self.rflags_mask.get() != (INTEGER_FLAGS | (1 << 9) | (3 << 12))
            {
                return Err(invalid(
                    "memory.code/memory.image",
                    "S2-38e requires the exact 37-byte composition, RBX=0x620, private 512-byte image at 0x660, CPL0, budget 10, terminal STOP or #XM, and the S2-38d MXCSR domain",
                ));
            }
        }
        if matches!(
            self.profile,
            Profile::LongXorpsRegV1
                | Profile::LongXorpdRegV1
                | Profile::LongNativeXorpdXmm0Xmm1V1
                | Profile::LongAndpdRegV1
                | Profile::LongOrpdRegV1
                | Profile::LongAndnpdRegV1
                | Profile::LongPxorRegV1
                | Profile::LongPandRegV1
                | Profile::LongPandnRegV1
                | Profile::LongPorRegV1
                | Profile::LongPavgbRegV1
                | Profile::LongPavgwRegV1
                | Profile::LongPmaxubRegV1
                | Profile::LongPminubRegV1
                | Profile::LongPmaxswRegV1
                | Profile::LongPminswRegV1
                | Profile::LongPmullwRegV1
                | Profile::LongPmulhuwRegV1
                | Profile::LongPsadbwRegV1
                | Profile::LongPmovmskbXmmGprV1
                | Profile::LongPmuludqRegV1
                | Profile::LongPmaddwdRegV1
                | Profile::LongPaddbRegV1
                | Profile::LongPaddwRegV1
                | Profile::LongPsubwRegV1
                | Profile::LongPadduswRegV1
                | Profile::LongPsubuswRegV1
                | Profile::LongPsubbRegV1
                | Profile::LongPaddusbRegV1
                | Profile::LongPaddsbRegV1
                | Profile::LongPsubsbRegV1
                | Profile::LongPaddswRegV1
                | Profile::LongPsubswRegV1
                | Profile::LongPadddRegV1
                | Profile::LongPsubdRegV1
                | Profile::LongPaddqRegV1
                | Profile::LongPsubqRegV1
                | Profile::LongPcmpeqbRegV1
                | Profile::LongPcmpeqwRegV1
                | Profile::LongPcmpeqdRegV1
                | Profile::LongPcmpgtbRegV1
                | Profile::LongPcmpgtwRegV1
                | Profile::LongPcmpgtdRegV1
                | Profile::LongPunpcklbwRegV1
                | Profile::LongPunpckhbwRegV1
                | Profile::LongPunpcklwdRegV1
                | Profile::LongPunpckhwdRegV1
                | Profile::LongPunpckldqRegV1
                | Profile::LongPunpckhdqRegV1
                | Profile::LongPunpcklqdqRegV1
                | Profile::LongPunpckhqdqRegV1
                | Profile::LongPacksswbRegV1
                | Profile::LongPackuswbRegV1
                | Profile::LongPackssdwRegV1
                | Profile::LongPshufdRegV1
                | Profile::LongPsllwImmRegV1
                | Profile::LongPslldImmRegV1
                | Profile::LongPsllqImmRegV1
                | Profile::LongPslldqImmRegV1
                | Profile::LongPsrldqImmRegV1
                | Profile::LongPsrlqImmRegV1
                | Profile::LongPsrldImmRegV1
                | Profile::LongPsrlwImmRegV1
                | Profile::LongPsrawImmRegV1
                | Profile::LongPsradImmRegV1
                | Profile::LongPsubusbRegV1
                | Profile::LongAndpsRegV1
                | Profile::LongOrpsRegV1
                | Profile::LongAndnpsRegV1
                | Profile::LongMovsdRegV1
                | Profile::LongMovsdStoreRegV1
                | Profile::LongRcpssRegV1
                | Profile::LongRsqrtpsRegV1
                | Profile::LongRcppsRegV1
                | Profile::LongMinssMaskedRegV1
                | Profile::LongMinsdMaskedRegV1
                | Profile::LongMaxsdMaskedRegV1
                | Profile::LongMinpdMaskedRegV1
                | Profile::LongMaxpdMaskedRegV1
                | Profile::LongMaxssMaskedRegV1
                | Profile::LongMinpsMaskedRegV1
                | Profile::LongMaxpsMaskedRegV1
                | Profile::LongRsqrtssRegV1
                | Profile::LongMovssRegV1
                | Profile::LongMovssStoreRegV1
        ) {
            let Some(code) = self.memory.iter().find(|region| region.region_id == "code") else {
                return Err(invalid("memory.code", "required by this profile"));
            };
            let code_bytes = code.initial_bytes_hex.to_ascii_lowercase();
            let length = match self.profile {
                Profile::LongXorpsRegV1 => xorps_register_length(&code_bytes),
                Profile::LongXorpdRegV1
                | Profile::LongXorpdPrerequisitesV1
                | Profile::LongNativeXorpdXmm0Xmm1V1 => xorpd_register_length(&code_bytes),
                Profile::LongAndpdRegV1 => andpd_register_length(&code_bytes),
                Profile::LongOrpdRegV1 => orpd_register_length(&code_bytes),
                Profile::LongAndnpdRegV1 => andnpd_register_length(&code_bytes),
                Profile::LongPxorRegV1 => pxor_register_length(&code_bytes),
                Profile::LongPandRegV1 => pand_register_length(&code_bytes),
                Profile::LongPandnRegV1 => pandn_register_length(&code_bytes),
                Profile::LongPorRegV1 => por_register_length(&code_bytes),
                Profile::LongPavgbRegV1 => pavgb_register_length(&code_bytes),
                Profile::LongPavgwRegV1 => pavgw_register_length(&code_bytes),
                Profile::LongPmaxubRegV1 => pmaxub_register_length(&code_bytes),
                Profile::LongPminubRegV1 => pminub_register_length(&code_bytes),
                Profile::LongPmaxswRegV1 => pmaxsw_register_length(&code_bytes),
                Profile::LongPminswRegV1 => pminsw_register_length(&code_bytes),
                Profile::LongPmullwRegV1 => pmullw_register_length(&code_bytes),
                Profile::LongPmulhuwRegV1 => pmulhuw_register_length(&code_bytes),
                Profile::LongPsadbwRegV1 => psadbw_register_length(&code_bytes),
                Profile::LongPmovmskbXmmGprV1 => pmovmskb_xmm_gpr_length(&code_bytes),
                Profile::LongPmuludqRegV1 => pmuludq_register_length(&code_bytes),
                Profile::LongPmaddwdRegV1 => pmaddwd_register_length(&code_bytes),
                Profile::LongPaddbRegV1 => paddb_register_length(&code_bytes),
                Profile::LongPaddwRegV1 => paddw_register_length(&code_bytes),
                Profile::LongPsubwRegV1 => psubw_register_length(&code_bytes),
                Profile::LongPadduswRegV1 => paddusw_register_length(&code_bytes),
                Profile::LongPsubuswRegV1 => psubusw_register_length(&code_bytes),
                Profile::LongPsubbRegV1 => psubb_register_length(&code_bytes),
                Profile::LongPaddusbRegV1 => paddusb_register_length(&code_bytes),
                Profile::LongPaddsbRegV1 => paddsb_register_length(&code_bytes),
                Profile::LongPsubsbRegV1 => psubsb_register_length(&code_bytes),
                Profile::LongPaddswRegV1 => paddsw_register_length(&code_bytes),
                Profile::LongPsubswRegV1 => psubsw_register_length(&code_bytes),
                Profile::LongPadddRegV1 => paddd_register_length(&code_bytes),
                Profile::LongPsubdRegV1 => psubd_register_length(&code_bytes),
                Profile::LongPaddqRegV1 => paddq_register_length(&code_bytes),
                Profile::LongPsubqRegV1 => psubq_register_length(&code_bytes),
                Profile::LongPcmpeqbRegV1 => pcmpeqb_register_length(&code_bytes),
                Profile::LongPcmpeqwRegV1 => pcmpeqw_register_length(&code_bytes),
                Profile::LongPcmpeqdRegV1 => pcmpeqd_register_length(&code_bytes),
                Profile::LongPcmpgtbRegV1 => pcmpgtb_register_length(&code_bytes),
                Profile::LongPcmpgtwRegV1 => pcmpgtw_register_length(&code_bytes),
                Profile::LongPcmpgtdRegV1 => pcmpgtd_register_length(&code_bytes),
                Profile::LongPunpcklbwRegV1 => punpcklbw_register_length(&code_bytes),
                Profile::LongPunpckhbwRegV1 => punpckhbw_register_length(&code_bytes),
                Profile::LongPunpcklwdRegV1 => punpcklwd_register_length(&code_bytes),
                Profile::LongPunpckhwdRegV1 => punpckhwd_register_length(&code_bytes),
                Profile::LongPunpckldqRegV1 => punpckldq_register_length(&code_bytes),
                Profile::LongPunpckhdqRegV1 => punpckhdq_register_length(&code_bytes),
                Profile::LongPunpcklqdqRegV1 => punpcklqdq_register_length(&code_bytes),
                Profile::LongPunpckhqdqRegV1 => punpckhqdq_register_length(&code_bytes),
                Profile::LongPacksswbRegV1 => packsswb_register_length(&code_bytes),
                Profile::LongPackuswbRegV1 => packuswb_register_length(&code_bytes),
                Profile::LongPackssdwRegV1 => packssdw_register_length(&code_bytes),
                Profile::LongPshufdRegV1 => pshufd_register_length(&code_bytes),
                Profile::LongPsllwImmRegV1 => psllw_immediate_register_length(&code_bytes),
                Profile::LongPslldImmRegV1 => pslld_immediate_register_length(&code_bytes),
                Profile::LongPsllqImmRegV1 => psllq_immediate_register_length(&code_bytes),
                Profile::LongPslldqImmRegV1 => pslldq_immediate_register_length(&code_bytes),
                Profile::LongPsrldqImmRegV1 => psrldq_immediate_register_length(&code_bytes),
                Profile::LongPsrlqImmRegV1 => psrlq_immediate_register_length(&code_bytes),
                Profile::LongPsrldImmRegV1 => psrld_immediate_register_length(&code_bytes),
                Profile::LongPsrlwImmRegV1 => psrlw_immediate_register_length(&code_bytes),
                Profile::LongPsrawImmRegV1 => psraw_immediate_register_length(&code_bytes),
                Profile::LongPsradImmRegV1 => psrad_immediate_register_length(&code_bytes),
                Profile::LongPsubusbRegV1 => psubusb_register_length(&code_bytes),
                Profile::LongAndpsRegV1 => andps_register_length(&code_bytes),
                Profile::LongOrpsRegV1 => orps_register_length(&code_bytes),
                Profile::LongAndnpsRegV1 => andnps_register_length(&code_bytes),
                Profile::LongMovssRegV1 => movss_register_length(&code_bytes),
                Profile::LongRsqrtssRegV1 => rsqrtss_register_length(&code_bytes),
                Profile::LongRcpssRegV1 => rcpss_register_length(&code_bytes),
                Profile::LongRsqrtpsRegV1 => rsqrtps_register_length(&code_bytes),
                Profile::LongRcppsRegV1 => rcpps_register_length(&code_bytes),
                Profile::LongMinssMaskedRegV1
                | Profile::LongMinssInvalidControlsV1
                | Profile::LongMinssUnmaskedDenormalV1 => minss_register_masked_length(&code_bytes),
                Profile::LongMaxssUnmaskedDenormalV1 => maxss_register_masked_length(&code_bytes),
                Profile::LongMinsdUnmaskedDenormalV1 => minsd_register_masked_length(&code_bytes),
                Profile::LongMaxsdUnmaskedDenormalV1 => maxsd_register_masked_length(&code_bytes),
                Profile::LongMinpdUnmaskedDenormalV1 => minpd_register_masked_length(&code_bytes),
                Profile::LongMinpsUnmaskedDenormalV1 => minps_exact_unmasked_length(&code_bytes),
                Profile::LongMaxpsUnmaskedDenormalV1 => maxps_exact_unmasked_length(&code_bytes),
                Profile::LongMaxpdUnmaskedDenormalV1 => maxpd_register_masked_length(&code_bytes),
                Profile::LongCvtdq2pdRegExactV1 => cvtdq2pd_register_length(&code_bytes),
                Profile::LongCvtdq2pdRegistersV1 => cvtdq2pd_register_length(&code_bytes),
                Profile::LongCvtdq2psRegistersV1 => {
                    cvtdq2ps_register_encoding(&code_bytes).map(|(_, _, length)| length)
                }
                Profile::LongCvtss2sdRegistersV1 => {
                    cvtss2sd_register_encoding(&code_bytes).map(|(_, _, length)| length)
                }
                Profile::LongCvtps2pdRegistersV1 => {
                    cvtps2pd_register_encoding(&code_bytes).map(|(_, _, length)| length)
                }
                Profile::LongComissUcomissRegistersV1 => {
                    comiss_ucomiss_register_encoding(&code_bytes).map(|(_, _, _, length)| length)
                }
                Profile::LongComisdUcomisdRegistersV1 => {
                    comisd_ucomisd_register_encoding(&code_bytes).map(|(_, _, _, length)| length)
                }
                Profile::LongCvtsi2sdR32ExactV1 => cvtsi2sd_r32_register_length(&code_bytes),
                Profile::LongCvtsi2sdR64ExactV1 => cvtsi2sd_r64_register_length(&code_bytes),
                Profile::LongCvtsi2sdRegistersV1 => {
                    cvtsi2sd_register_encoding(&code_bytes).map(|(_, _, _, length)| length)
                }
                Profile::LongCvtsi2ssRegistersV1 => {
                    cvtsi2ss_register_encoding(&code_bytes).map(|(_, _, _, length)| length)
                }
                Profile::LongMinsdMaskedRegV1 => minsd_register_masked_length(&code_bytes),
                Profile::LongMaxsdMaskedRegV1 => maxsd_register_masked_length(&code_bytes),
                Profile::LongMinpdMaskedRegV1 => minpd_register_masked_length(&code_bytes),
                Profile::LongMaxpdMaskedRegV1 => maxpd_register_masked_length(&code_bytes),
                Profile::LongMaxssMaskedRegV1 => maxss_register_masked_length(&code_bytes),
                Profile::LongMinpsMaskedRegV1 => minps_register_masked_length(&code_bytes),
                Profile::LongMaxpsMaskedRegV1 => maxps_register_masked_length(&code_bytes),
                Profile::LongMovsdRegV1 => movsd_register_length(&code_bytes),
                Profile::LongMovsdStoreRegV1 => movsd_store_register_length(&code_bytes),
                Profile::LongMovssStoreRegV1 => movss_store_register_length(&code_bytes),
                _ => unreachable!("matched SSE logic profile above"),
            }
            .ok_or_else(|| invalid("memory.code", "outside the register-only SSE profile"))?;
            if code.guest_address != self.initial_state.rip
                || code.length != length
                || !code.permissions.execute
                || self.instruction_budget != 1
                || self.stop_rip.map(HexU64::get)
                    != self.initial_state.rip.get().checked_add(u64::from(length))
                || self.expected_exit != ExpectedExit::StopAddress
            {
                return Err(invalid(
                    "memory.code",
                    "SSE logic requires one executable instruction, budget 1, and stop_rip immediately afterward",
                ));
            }
        }
        if self.profile == Profile::LongMovupsRegV1 {
            let Some(code) = self.memory.iter().find(|region| region.region_id == "code") else {
                return Err(invalid("memory.code", "required by this profile"));
            };
            let code_bytes = code.initial_bytes_hex.to_ascii_lowercase();
            let length = movups_register_length(&code_bytes).ok_or_else(|| {
                invalid("memory.code", "outside the register-only MOVUPS profile")
            })?;
            if code.guest_address != self.initial_state.rip
                || code.length != length
                || !code.permissions.execute
                || self.instruction_budget != 1
                || self.stop_rip.map(HexU64::get)
                    != self.initial_state.rip.get().checked_add(u64::from(length))
                || self.expected_exit != ExpectedExit::StopAddress
            {
                return Err(invalid(
                    "memory.code",
                    "MOVUPS requires one executable instruction, budget 1, and stop_rip immediately afterward",
                ));
            }
        }
        if matches!(
            self.profile,
            Profile::LongMovapsRegV1 | Profile::LongMovapsReverseRegV1
        ) {
            let Some(code) = self.memory.iter().find(|region| region.region_id == "code") else {
                return Err(invalid("memory.code", "required by this profile"));
            };
            let code_bytes = code.initial_bytes_hex.to_ascii_lowercase();
            let length = movaps_register_length(
                &code_bytes,
                self.profile == Profile::LongMovapsReverseRegV1,
            )
            .ok_or_else(|| invalid("memory.code", "outside the register-only MOVAPS profile"))?;
            if code.guest_address != self.initial_state.rip
                || code.length != length
                || !code.permissions.execute
                || self.instruction_budget != 1
                || self.stop_rip.map(HexU64::get)
                    != self.initial_state.rip.get().checked_add(u64::from(length))
                || self.expected_exit != ExpectedExit::StopAddress
            {
                return Err(invalid(
                    "memory.code",
                    "MOVAPS requires one executable instruction, budget 1, and stop_rip immediately afterward",
                ));
            }
        }
        if self.profile == Profile::LongMovapdRegV1 {
            let Some(code) = self.memory.iter().find(|region| region.region_id == "code") else {
                return Err(invalid("memory.code", "required by this profile"));
            };
            let code_bytes = code.initial_bytes_hex.to_ascii_lowercase();
            let length = movapd_register_length(&code_bytes).ok_or_else(|| {
                invalid("memory.code", "outside the register-only MOVAPD profile")
            })?;
            if code.guest_address != self.initial_state.rip
                || code.length != length
                || !code.permissions.execute
                || self.instruction_budget != 1
                || self.stop_rip.map(HexU64::get)
                    != self.initial_state.rip.get().checked_add(u64::from(length))
                || self.expected_exit != ExpectedExit::StopAddress
            {
                return Err(invalid(
                    "memory.code",
                    "MOVAPD requires one executable instruction, budget 1, and stop_rip immediately afterward",
                ));
            }
        }
        if self.profile == Profile::LongMovupdRegV1 {
            let Some(code) = self.memory.iter().find(|region| region.region_id == "code") else {
                return Err(invalid("memory.code", "required by this profile"));
            };
            let code_bytes = code.initial_bytes_hex.to_ascii_lowercase();
            let length = movupd_register_length(&code_bytes).ok_or_else(|| {
                invalid("memory.code", "outside the register-only MOVUPD profile")
            })?;
            if code.guest_address != self.initial_state.rip
                || code.length != length
                || !code.permissions.execute
                || self.instruction_budget != 1
                || self.stop_rip.map(HexU64::get)
                    != self.initial_state.rip.get().checked_add(u64::from(length))
                || self.expected_exit != ExpectedExit::StopAddress
            {
                return Err(invalid(
                    "memory.code",
                    "MOVUPD requires one executable instruction, budget 1, and stop_rip immediately afterward",
                ));
            }
        }
        if self.profile == Profile::LongMovupdReverseRegV1 {
            let Some(code) = self.memory.iter().find(|region| region.region_id == "code") else {
                return Err(invalid("memory.code", "required by this profile"));
            };
            let code_bytes = code.initial_bytes_hex.to_ascii_lowercase();
            let length = movupd_reverse_register_length(&code_bytes).ok_or_else(|| {
                invalid(
                    "memory.code",
                    "outside the reverse-direction register-only MOVUPD profile",
                )
            })?;
            if code.guest_address != self.initial_state.rip
                || code.length != length
                || !code.permissions.execute
                || self.instruction_budget != 1
                || self.stop_rip.map(HexU64::get)
                    != self.initial_state.rip.get().checked_add(u64::from(length))
                || self.expected_exit != ExpectedExit::StopAddress
            {
                return Err(invalid(
                    "memory.code",
                    "reverse-direction MOVUPD requires one executable instruction, budget 1, and stop_rip immediately afterward",
                ));
            }
        }
        if self.profile == Profile::LongMovapdReverseRegV1 {
            let Some(code) = self.memory.iter().find(|region| region.region_id == "code") else {
                return Err(invalid("memory.code", "required by this profile"));
            };
            let code_bytes = code.initial_bytes_hex.to_ascii_lowercase();
            let length = movapd_reverse_register_length(&code_bytes).ok_or_else(|| {
                invalid(
                    "memory.code",
                    "outside the reverse-direction register-only MOVAPD profile",
                )
            })?;
            if code.guest_address != self.initial_state.rip
                || code.length != length
                || !code.permissions.execute
                || self.instruction_budget != 1
                || self.stop_rip.map(HexU64::get)
                    != self.initial_state.rip.get().checked_add(u64::from(length))
                || self.expected_exit != ExpectedExit::StopAddress
            {
                return Err(invalid(
                    "memory.code",
                    "reverse MOVAPD requires one executable instruction, budget 1, and stop_rip immediately afterward",
                ));
            }
        }
        if self.profile == Profile::LongMovupsMemLoadV1
            || self.profile == Profile::LongMovupdMemLoadV1
            || self.profile == Profile::LongMovupdMemStoreV1
        {
            let code = self.memory.iter().find(|region| region.region_id == "code");
            let Some(code) = code else {
                return Err(invalid("memory.code", "required by this profile"));
            };
            let code_bytes = code.initial_bytes_hex.to_ascii_lowercase();
            let decoded = if self.profile == Profile::LongMovupdMemLoadV1 {
                movupd_memory_load_destination(&code_bytes)
            } else if self.profile == Profile::LongMovupdMemStoreV1 {
                movupd_memory_store_source(&code_bytes)
            } else {
                movups_memory_load_destination(&code_bytes)
            };
            let (length, _) = decoded.ok_or_else(|| {
                invalid(
                    "memory.code",
                    "outside the exact [RAX] packed memory profile",
                )
            })?;
            let start = self.initial_state.gpr[0].get();
            let end = start
                .checked_add(15)
                .ok_or_else(|| invalid("state.gpr[0]", "MOVUPS source range overflows u64"))?;
            let data = self.memory.iter().find(|region| region.region_id == "data");
            let Some(data) = data else {
                return Err(invalid("memory.data", "required by this profile"));
            };
            let data_start = data.guest_address.get();
            let data_end = data_start
                .checked_add(u64::from(data.length))
                .ok_or_else(|| invalid("memory.data", "range overflows u64"))?;
            let code_end = self.initial_state.rip.get().checked_add(u64::from(length));
            if self.memory.len() != 2
                || code.region_id != "code"
                || code.guest_address.get() != self.initial_state.rip.get()
                || code.length != length
                || !code.permissions.execute
                || self.instruction_budget != 1
                || self.stop_rip.map(HexU64::get) != code_end
                || self.expected_exit != ExpectedExit::StopAddress
                || data.region_id != "data"
                || !data.permissions.read
                || data.permissions.write != (self.profile == Profile::LongMovupdMemStoreV1)
                || data.permissions.execute
                || start < data_start
                || end >= data_end
            {
                return Err(invalid(
                    "memory",
                    "packed memory profile requires one code instruction and one correctly permissioned data region covering [RAX..RAX+15]",
                ));
            }
        }
        if self.profile == Profile::LongMovssMemLoadV1 {
            let Some(code) = self.memory.iter().find(|region| region.region_id == "code") else {
                return Err(invalid("memory.code", "required by this profile"));
            };
            let bytes = code.initial_bytes_hex.to_ascii_lowercase();
            let (length, _) = movss_memory_load_destination(&bytes)
                .ok_or_else(|| invalid("memory.code", "outside the F3 MOVSS [RAX] load profile"))?;
            let start = self.initial_state.gpr[0].get();
            let end = start
                .checked_add(3)
                .ok_or_else(|| invalid("state.gpr[0]", "MOVSS source range overflows u64"))?;
            let Some(data) = self.memory.iter().find(|region| region.region_id == "data") else {
                return Err(invalid("memory.data", "required by this profile"));
            };
            let data_start = data.guest_address.get();
            let data_end = data_start
                .checked_add(u64::from(data.length))
                .ok_or_else(|| invalid("memory.data", "range overflows u64"))?;
            if self.memory.len() != 2
                || code.guest_address != self.initial_state.rip
                || code.length != length
                || !code.permissions.execute
                || self.instruction_budget != 1
                || self.stop_rip.map(HexU64::get)
                    != self.initial_state.rip.get().checked_add(u64::from(length))
                || self.expected_exit != ExpectedExit::StopAddress
                || data.permissions
                    != (MemoryPermissions {
                        read: true,
                        write: false,
                        execute: false,
                    })
                || data_start > start
                || end >= data_end
            {
                return Err(invalid(
                    "memory",
                    "MOVSS load requires one code instruction, one read-only data region covering [RAX..RAX+3], budget 1, and stop immediately afterward",
                ));
            }
        }
        if self.profile == Profile::LongMovsdMemLoadV1 {
            let Some(code) = self.memory.iter().find(|region| region.region_id == "code") else {
                return Err(invalid("memory.code", "required by this profile"));
            };
            let bytes = code.initial_bytes_hex.to_ascii_lowercase();
            let (length, _) = movsd_memory_load_destination(&bytes)
                .ok_or_else(|| invalid("memory.code", "outside the F2 MOVSD [RAX] load profile"))?;
            let start = self.initial_state.gpr[0].get();
            let end = start
                .checked_add(7)
                .ok_or_else(|| invalid("state.gpr[0]", "MOVSD source range overflows u64"))?;
            let Some(data) = self.memory.iter().find(|region| region.region_id == "data") else {
                return Err(invalid("memory.data", "required by this profile"));
            };
            let data_start = data.guest_address.get();
            let data_end = data_start
                .checked_add(u64::from(data.length))
                .ok_or_else(|| invalid("memory.data", "range overflows u64"))?;
            if self.memory.len() != 2
                || code.guest_address != self.initial_state.rip
                || code.length != length
                || !code.permissions.execute
                || self.instruction_budget != 1
                || self.stop_rip.map(HexU64::get)
                    != self.initial_state.rip.get().checked_add(u64::from(length))
                || self.expected_exit != ExpectedExit::StopAddress
                || data.permissions
                    != (MemoryPermissions {
                        read: true,
                        write: false,
                        execute: false,
                    })
                || data_start > start
                || end >= data_end
            {
                return Err(invalid(
                    "memory",
                    "MOVSD load requires one code instruction, one read-only data region covering [RAX..RAX+7], budget 1, and stop immediately afterward",
                ));
            }
        }
        if self.profile == Profile::LongMovsdMemStoreV1 {
            let Some(code) = self.memory.iter().find(|region| region.region_id == "code") else {
                return Err(invalid("memory.code", "required by this profile"));
            };
            let bytes = code.initial_bytes_hex.to_ascii_lowercase();
            let (length, _) = movsd_memory_store_source(&bytes).ok_or_else(|| {
                invalid("memory.code", "outside the F2 MOVSD [RAX] store profile")
            })?;
            let start = self.initial_state.gpr[0].get();
            let end = start
                .checked_add(7)
                .ok_or_else(|| invalid("state.gpr[0]", "MOVSD destination range overflows u64"))?;
            let Some(data) = self.memory.iter().find(|region| region.region_id == "data") else {
                return Err(invalid("memory.data", "required by this profile"));
            };
            let data_start = data.guest_address.get();
            let data_end = data_start
                .checked_add(u64::from(data.length))
                .ok_or_else(|| invalid("memory.data", "range overflows u64"))?;
            if self.memory.len() != 2
                || code.guest_address != self.initial_state.rip
                || code.length != length
                || !code.permissions.execute
                || self.instruction_budget != 1
                || self.stop_rip.map(HexU64::get)
                    != self.initial_state.rip.get().checked_add(u64::from(length))
                || self.expected_exit != ExpectedExit::StopAddress
                || data.permissions
                    != (MemoryPermissions {
                        read: true,
                        write: true,
                        execute: false,
                    })
                || data_start > start
                || end >= data_end
            {
                return Err(invalid(
                    "memory",
                    "MOVSD store requires one code instruction, one writable data region covering [RAX..RAX+7], budget 1, and stop immediately afterward",
                ));
            }
        }
        if self.profile == Profile::LongMovssMemStoreV1 {
            let Some(code) = self.memory.iter().find(|region| region.region_id == "code") else {
                return Err(invalid("memory.code", "required by this profile"));
            };
            let bytes = code.initial_bytes_hex.to_ascii_lowercase();
            let (length, _) = movss_memory_store_source(&bytes).ok_or_else(|| {
                invalid("memory.code", "outside the F3 MOVSS [RAX] store profile")
            })?;
            let start = self.initial_state.gpr[0].get();
            let end = start
                .checked_add(3)
                .ok_or_else(|| invalid("state.gpr[0]", "MOVSS destination range overflows u64"))?;
            let Some(data) = self.memory.iter().find(|region| region.region_id == "data") else {
                return Err(invalid("memory.data", "required by this profile"));
            };
            let data_start = data.guest_address.get();
            let data_end = data_start
                .checked_add(u64::from(data.length))
                .ok_or_else(|| invalid("memory.data", "range overflows u64"))?;
            if self.memory.len() != 2
                || code.guest_address != self.initial_state.rip
                || code.length != length
                || !code.permissions.execute
                || self.instruction_budget != 1
                || self.stop_rip.map(HexU64::get)
                    != self.initial_state.rip.get().checked_add(u64::from(length))
                || self.expected_exit != ExpectedExit::StopAddress
                || data.permissions
                    != (MemoryPermissions {
                        read: true,
                        write: true,
                        execute: false,
                    })
                || data_start > start
                || end >= data_end
            {
                return Err(invalid(
                    "memory",
                    "MOVSS store requires one code instruction, one readable/writable data region covering [RAX..RAX+3], budget 1, and stop immediately afterward",
                ));
            }
        }
        if self.profile == Profile::LongMovupsMemStoreV1 {
            let code = self.memory.iter().find(|region| region.region_id == "code");
            let Some(code) = code else {
                return Err(invalid("memory.code", "required by this profile"));
            };
            let code_bytes = code.initial_bytes_hex.to_ascii_lowercase();
            let length = movups_memory_store_length(&code_bytes).ok_or_else(|| {
                invalid(
                    "memory.code",
                    "outside the [RAX] MOVUPS memory-store profile",
                )
            })?;
            let start = self.initial_state.gpr[0].get();
            let end = start
                .checked_add(15)
                .ok_or_else(|| invalid("state.gpr[0]", "MOVUPS destination range overflows u64"))?;
            let data = self.memory.iter().find(|region| region.region_id == "data");
            let Some(data) = data else {
                return Err(invalid("memory.data", "required by this profile"));
            };
            let data_start = data.guest_address.get();
            let data_end = data_start
                .checked_add(u64::from(data.length))
                .ok_or_else(|| invalid("memory.data", "range overflows u64"))?;
            let code_end = self.initial_state.rip.get().checked_add(u64::from(length));
            if self.memory.len() != 2
                || code.region_id != "code"
                || code.guest_address.get() != self.initial_state.rip.get()
                || code.length != length
                || !code.permissions.execute
                || self.instruction_budget != 1
                || self.stop_rip.map(HexU64::get) != code_end
                || self.expected_exit != ExpectedExit::StopAddress
                || !data.permissions.write
                || data.permissions.execute
                || data.region_id != "data"
                || start < data_start
                || end >= data_end
            {
                return Err(invalid(
                    "memory",
                    "MOVUPS memory-store profile requires one code instruction and one writable, non-executable data region covering [RAX..RAX+15]",
                ));
            }
        }
        if self.profile == Profile::LongMovupsMemBaseV1
            || self.profile == Profile::LongMovupdMemBaseV1
            || self.profile == Profile::LongMovupdMemRipV1
            || self.profile == Profile::LongMovupdMemSibV1
        {
            let Some(code) = self.memory.iter().find(|region| region.region_id == "code") else {
                return Err(invalid("memory.code", "required by this profile"));
            };
            let code_bytes = code.initial_bytes_hex.to_ascii_lowercase();
            if matches!(
                self.profile,
                Profile::LongMovupdMemBaseV1 | Profile::LongMovupdMemRipV1
            ) && !code_bytes.starts_with("66")
            {
                return Err(invalid(
                    "memory.code",
                    "MOVUPD memory-base profile requires mandatory 66 before optional REX",
                ));
            }
            if self.profile == Profile::LongMovupsMemBaseV1 && code_bytes.starts_with("66") {
                return Err(invalid(
                    "memory.code",
                    "MOVUPS memory-base profile does not admit mandatory 66",
                ));
            }
            let cpu = self.initial_state.to_cpu_for(self.profile)?;
            let encoded_sib = movupd_sib_memory_form(&code_bytes).is_some();
            if (self.profile == Profile::LongMovupdMemSibV1) != encoded_sib {
                return Err(invalid(
                    "memory.code",
                    "MOVUPD SIB profile requires the original ModR/M r/m=100 SIB encoding",
                ));
            }
            let (length, store, start, rip_relative) = movups_memory_base_form(&code_bytes, &cpu)
                .ok_or_else(|| {
                invalid(
                    "memory.code",
                    if self.profile == Profile::LongMovupdMemRipV1 {
                        "outside the RIP-relative MOVUPD profile"
                    } else if self.profile == Profile::LongMovupdMemBaseV1 {
                        "outside the base+displacement MOVUPD profile"
                    } else {
                        "outside the base+displacement MOVUPS profile"
                    },
                )
            })?;
            if matches!(
                self.profile,
                Profile::LongMovupdMemBaseV1 | Profile::LongMovupdMemRipV1
            ) && rip_relative != (self.profile == Profile::LongMovupdMemRipV1)
            {
                return Err(invalid(
                    "memory.code",
                    if self.profile == Profile::LongMovupdMemBaseV1 {
                        "outside the base+displacement MOVUPD profile"
                    } else {
                        "RIP-relative encoding does not match the selected MOVUPD profile"
                    },
                ));
            }
            let end = start
                .checked_add(15)
                .ok_or_else(|| invalid("state.gpr", "MOVUPS access range overflows u64"))?;
            let data = self.memory.iter().find(|region| region.region_id == "data");
            let Some(data) = data else {
                return Err(invalid("memory.data", "required by this profile"));
            };
            let data_start = data.guest_address.get();
            let data_end = data_start
                .checked_add(u64::from(data.length))
                .ok_or_else(|| invalid("memory.data", "range overflows u64"))?;
            let code_end = self.initial_state.rip.get().checked_add(u64::from(length));
            let permission_ok = if store {
                data.permissions.write
            } else {
                data.permissions.read
            };
            if self.memory.len() != 2
                || code.region_id != "code"
                || code.guest_address.get() != self.initial_state.rip.get()
                || code.length != length
                || !code.permissions.execute
                || self.instruction_budget != 1
                || self.stop_rip.map(HexU64::get) != code_end
                || self.expected_exit != ExpectedExit::StopAddress
                || !permission_ok
                || data.permissions.execute
                || start < data_start
                || end >= data_end
            {
                return Err(invalid(
                    "memory",
                    "MOVUPS base profile requires one code instruction and one direction-appropriate data region covering the effective 16-byte range",
                ));
            }
        }
        if self.profile == Profile::LongFxsaveRipV1 {
            let code = self.memory.iter().find(|region| region.region_id == "code");
            let data = self.memory.iter().find(|region| region.region_id == "data");
            let (Some(code), Some(data)) = (code, data) else {
                return Err(invalid("memory", "FXSAVE requires code and data regions"));
            };
            let code_bytes = decode_bytes(&code.initial_bytes_hex)?;
            let rip = self.initial_state.rip.get();
            let Some(next_rip) = rip.checked_add(7) else {
                return Err(invalid("initial_state.rip", "instruction end overflows"));
            };
            if code_bytes.len() != 7 || code_bytes[..3] != [0x0f, 0xae, 0x05] {
                return Err(invalid(
                    "memory.code",
                    "FXSAVE profile admits exactly unprefixed 0F AE 05 disp32",
                ));
            }
            let displacement = i32::from_le_bytes(code_bytes[3..7].try_into().unwrap());
            let area = next_rip.wrapping_add_signed(i64::from(displacement));
            let area_end = area.checked_add(511);
            let is_canonical = |address: u64| {
                let sign = (address >> 47) & 1;
                address >> 48 == if sign == 0 { 0 } else { 0xffff }
            };
            if code.guest_address.get() != rip
                || !code.permissions.execute
                || code.permissions.write
                || data.guest_address.get() != area
                || data.length != 512
                || !data.permissions.write
                || data.permissions.execute
                || area & 0xf != 0
                || !is_canonical(area)
                || area_end.is_none_or(|end| !is_canonical(end))
                || self.stop_rip.map(HexU64::get) != Some(next_rip)
                || self.instruction_budget != 1
                || self.expected_exit != ExpectedExit::StopAddress
            {
                return Err(invalid(
                    "memory",
                    "FXSAVE profile requires one aligned canonical 512-byte writable area, budget 1, and stop immediately after the seven-byte instruction",
                ));
            }
        }
        if matches!(
            self.profile,
            Profile::LongFxsave64RaxV1 | Profile::LongFxsave64Cr0GatesV1
        ) {
            let code = self.memory.iter().find(|region| region.region_id == "code");
            let data = self.memory.iter().find(|region| region.region_id == "data");
            let (Some(code), Some(data)) = (code, data) else {
                return Err(invalid("memory", "FXSAVE64 requires code and data regions"));
            };
            let code_bytes = decode_bytes(&code.initial_bytes_hex)?;
            let rip = self.initial_state.rip.get();
            let Some(next_rip) = rip.checked_add(5) else {
                return Err(invalid("initial_state.rip", "instruction end overflows"));
            };
            // x86 effective-address arithmetic is modulo 2^64; validate the
            // resulting 512-byte linear image range separately below.
            let area = self.initial_state.gpr[0].get().wrapping_add(0x40);
            let area_end = area.checked_add(511);
            let is_canonical = |address: u64| {
                let sign = (address >> 47) & 1;
                address >> 48 == if sign == 0 { 0 } else { 0xffff }
            };
            if self.initial_state.mode != Mode::Long64
                || code.guest_address.get() != rip
                || !code.permissions.execute
                || code.permissions.write
                || code_bytes != [0x48, 0x0f, 0xae, 0x40, 0x40]
                || data.guest_address.get() != area
                || data.length != 512
                || !data.permissions.write
                || data.permissions.execute
                || area & 0xf != 0
                || !is_canonical(area)
                || area_end.is_none_or(|end| !is_canonical(end))
                || self.stop_rip.map(HexU64::get) != Some(next_rip)
                || self.instruction_budget != 1
                || !(self.expected_exit == ExpectedExit::StopAddress
                    || (self.profile == Profile::LongFxsave64Cr0GatesV1
                        && self.expected_exit == ExpectedExit::Exception))
            {
                return Err(invalid(
                    "memory",
                    "FXSAVE64 profile requires exact 48 0F AE 40 40, an aligned canonical 512-byte RAX+0x40 writable area, budget 1, and stop immediately after the five-byte instruction",
                ));
            }
        }
        if matches!(
            self.profile,
            Profile::LongFxrstor64RbxV1 | Profile::LongFxrstor64Cr0GatesV1
        ) {
            let code = self.memory.iter().find(|region| region.region_id == "code");
            let data = self.memory.iter().find(|region| region.region_id == "data");
            let (Some(code), Some(data)) = (code, data) else {
                return Err(invalid(
                    "memory",
                    "FXRSTOR64 requires code and data regions",
                ));
            };
            let code_bytes = decode_bytes(&code.initial_bytes_hex)?;
            let rip = self.initial_state.rip.get();
            let Some(next_rip) = rip.checked_add(5) else {
                return Err(invalid("initial_state.rip", "instruction end overflows"));
            };
            // x86 effective-address arithmetic wraps before the image range is checked.
            let area = self.initial_state.gpr[3].get().wrapping_add(0x40);
            let area_end = area.checked_add(511);
            let is_canonical = |address: u64| {
                let sign = (address >> 47) & 1;
                address >> 48 == if sign == 0 { 0 } else { 0xffff }
            };
            if self.initial_state.mode != Mode::Long64
                || code.guest_address.get() != rip
                || !code.permissions.execute
                || code.permissions.write
                || code_bytes != [0x48, 0x0f, 0xae, 0x4b, 0x40]
                || data.guest_address.get() != area
                || data.length != 512
                || !data.permissions.read
                || data.permissions.execute
                || area & 0xf != 0
                || !is_canonical(area)
                || area_end.is_none_or(|end| !is_canonical(end))
                || self.stop_rip.map(HexU64::get) != Some(next_rip)
                || self.instruction_budget != 1
                || !(self.expected_exit == ExpectedExit::StopAddress
                    || (self.profile == Profile::LongFxrstor64Cr0GatesV1
                        && self.expected_exit == ExpectedExit::Exception))
            {
                return Err(invalid(
                    "memory",
                    "FXRSTOR64 profile requires exact 48 0F AE 4B 40, an aligned canonical 512-byte RBX+0x40 readable area, budget 1, and stop immediately after the five-byte instruction",
                ));
            }
            if self.profile == Profile::LongFxrstor64Cr0GatesV1 {
                let image = decode_bytes(&data.initial_bytes_hex)?;
                let mxcsr_bytes: [u8; 4] = image
                    .get(24..28)
                    .and_then(|bytes| bytes.try_into().ok())
                    .ok_or_else(|| invalid("memory.data.initial_bytes_hex", "missing MXCSR"))?;
                let loaded_mxcsr = u32::from_le_bytes(mxcsr_bytes);
                if loaded_mxcsr & !od_isa::TARGET_FXSAVE_MXCSR_MASK != 0 {
                    return Err(invalid(
                        "memory.data.initial_bytes_hex",
                        "FXRSTOR64 CR0 gate fixtures require MXCSR valid under the immutable target mask",
                    ));
                }
            }
        }
        if matches!(
            self.profile,
            Profile::LongX87StateV1 | Profile::LongX87SseStateV1 | Profile::LongX87SelectorsStateV1
        ) {
            let code = self.memory.iter().find(|region| region.region_id == "code");
            let Some(code) = code else {
                return Err(invalid("memory.code", "required by this profile"));
            };
            let code_bytes = code.initial_bytes_hex.to_ascii_lowercase();
            if matches!(
                self.profile,
                Profile::LongX87SseStateV1 | Profile::LongX87SelectorsStateV1
            ) && code_bytes != "90"
            {
                return Err(invalid(
                    "memory.code",
                    "the local x87/SSE snapshot profile admits exactly one NOP",
                ));
            }
            let (expected_bytes, expected_length) = match code_bytes.as_str() {
                "90" => ("90", 1),
                "dbe3" => ("dbe3", 2),
                "dbe2" => ("dbe2", 2),
                "dbe9" => ("dbe9", 2),
                "dbe9dfe0" => ("dbe9dfe0", 4),
                "dbf1" => ("dbf1", 2),
                "dbf1dfe0" => ("dbf1dfe0", 4),
                _ if fcmovu_stream(&code_bytes) => {
                    (code_bytes.as_str(), (code_bytes.len() / 2) as u32)
                }
                _ if fcmovnu_stream(&code_bytes) => {
                    (code_bytes.as_str(), (code_bytes.len() / 2) as u32)
                }
                "dac9dac9dfe0"
                | "d9e8d9e8dbf1d9e0dac9dfe0"
                | "d9e8d9e8d9e0dbf1dac9dfe0"
                | "dbc9dbc9dfe0"
                | "d9e8d9e8dbf1d9e0dbc9dfe0"
                | "d9e8d9e8d9e0dbf1dbc9dfe0" => {
                    (code_bytes.as_str(), (code_bytes.len() / 2) as u32)
                }
                _ if fcmovbe_stream(&code_bytes) => {
                    (code_bytes.as_str(), (code_bytes.len() / 2) as u32)
                }
                _ if fcmovnbe_stream(&code_bytes) => {
                    (code_bytes.as_str(), (code_bytes.len() / 2) as u32)
                }
                _ if fcmove_family_source(&code_bytes).is_some() => {
                    (code_bytes.as_str(), (code_bytes.len() / 2) as u32)
                }
                _ if fcmovne_family_source(&code_bytes).is_some() => {
                    (code_bytes.as_str(), (code_bytes.len() / 2) as u32)
                }
                _ if fcmovbe_stream(&code_bytes) => {
                    (code_bytes.as_str(), (code_bytes.len() / 2) as u32)
                }
                _ if fcmovb_family_source(&code_bytes).is_some() => {
                    (code_bytes.as_str(), (code_bytes.len() / 2) as u32)
                }
                _ if fcmovnb_family_source(&code_bytes).is_some() => {
                    (code_bytes.as_str(), (code_bytes.len() / 2) as u32)
                }
                "dbc1dbc1dfe0" => ("dbc1dbc1dfe0", 6),
                "d9e8d9e8d9e0dbf1dbc1dfe0" => ("d9e8d9e8d9e0dbf1dbc1dfe0", 12),
                "d9e8d9e0d9e8dbf1dbc1dfe0" => ("d9e8d9e0d9e8dbf1dbc1dfe0", 12),
                "dac1dac1dfe0" => ("dac1dac1dfe0", 6),
                "d9e8d9e0dbf1dac1dfe0" => ("d9e8d9e0dbf1dac1dfe0", 10),
                "d9e8d9e0d9e8dbf1dac1dfe0" => ("d9e8d9e0d9e8dbf1dac1dfe0", 12),
                "dff1" => ("dff1", 2),
                "dff1dfe0" => ("dff1dfe0", 4),
                "dfe9" => ("dfe9", 2),
                "dfe9dfe0" => ("dfe9dfe0", 4),
                "dfe0" => ("dfe0", 2),
                _ if fcomi_family_source(&code_bytes).is_some() => {
                    (code_bytes.as_str(), (code_bytes.len() / 2) as u32)
                }
                "d9e0" => ("d9e0", 2),
                "d9e1" => ("d9e1", 2),
                "d9e4" => ("d9e4", 2),
                "d9e5" => ("d9e5", 2),
                "d9e5dfe0" => ("d9e5dfe0", 4),
                "d9e5d9e5dfe0" => ("d9e5d9e5dfe0", 6),
                "d9e8d9e5dfe0" => ("d9e8d9e5dfe0", 6),
                "d9eed9e5dfe0" => ("d9eed9e5dfe0", 6),
                "d9e8d9e0d9e5dfe0" => ("d9e8d9e0d9e5dfe0", 8),
                "d9eed9e0d9e5dfe0" => ("d9eed9e0d9e5dfe0", 8),
                "ded9" => ("ded9", 2),
                "dae9" => ("dae9", 2),
                "dae9dfe0" => ("dae9dfe0", 4),
                "dde1" => ("dde1", 2),
                "dde1dfe0" => ("dde1dfe0", 4),
                "dde9" => ("dde9", 2),
                "dde9dfe0" => ("dde9dfe0", 4),
                _ if code_bytes.len() == 4
                    && code_bytes.starts_with("d8d")
                    && matches!(code_bytes.as_bytes()[3], b'8'..=b'f') =>
                {
                    (code_bytes.as_str(), 2)
                }
                _ if code_bytes.len() == 4
                    && code_bytes.starts_with("d8d")
                    && matches!(code_bytes.as_bytes()[3], b'0'..=b'7') =>
                {
                    (code_bytes.as_str(), 2)
                }
                _ if code_bytes.len() == 8
                    && code_bytes.starts_with("d8d")
                    && matches!(code_bytes.as_bytes()[3], b'8'..=b'f')
                    && code_bytes.ends_with("dfe0") =>
                {
                    (code_bytes.as_str(), 4)
                }
                _ if code_bytes.len() == 8
                    && code_bytes.starts_with("d8d")
                    && matches!(code_bytes.as_bytes()[3], b'0'..=b'7')
                    && code_bytes.ends_with("dfe0") =>
                {
                    (code_bytes.as_str(), 4)
                }
                "ded9dfe0" => ("ded9dfe0", 4),
                "d9fcdfe0" => ("d9fcdfe0", 4),
                "d9fcdfe0dbe2" => ("d9fcdfe0dbe2", 6),
                "d9fcd9e0dfe0" => ("d9fcd9e0dfe0", 6),
                "d9fcd9e1dfe0" => ("d9fcd9e1dfe0", 6),
                "d910d9fcdfe0" => ("d910d9fcdfe0", 6),
                "d900d9fcdfe0" => ("d900d9fcdfe0", 6),
                "dd00d9fcdfe0" => ("dd00d9fcdfe0", 6),
                "d910d900dfe0dbe2" => ("d910d900dfe0dbe2", 8),
                "dd10dd00dfe0dbe2" => ("dd10dd00dfe0dbe2", 8),
                "db28d9fcdfe0" => ("db28d9fcdfe0", 6),
                "db28d9fcdfe0dbe2d9fc" => ("db28d9fcdfe0dbe2d9fc", 10),
                "db28d9fcd9fcdfe0dbe2d9fc" => ("db28d9fcd9fcdfe0dbe2d9fc", 12),
                "d9fc" => ("d9fc", 2),
                "d9f7" => ("d9f7", 2),
                "d9f6" => ("d9f6", 2),
                "d9e8" => ("d9e8", 2),
                "d9ee" => ("d9ee", 2),
                "d9e8dfe0" => ("d9e8dfe0", 4),
                "d9eedfe0" => ("d9eedfe0", 4),
                _ if code_bytes.len() == 4
                    && code_bytes.starts_with("d9c")
                    && matches!(code_bytes.as_bytes()[3], b'0'..=b'6') =>
                {
                    (code_bytes.as_str(), 2)
                }
                _ if code_bytes.len() == 8
                    && code_bytes.starts_with("d9c")
                    && matches!(code_bytes.as_bytes()[3], b'0'..=b'6')
                    && code_bytes.ends_with("dfe0") =>
                {
                    (code_bytes.as_str(), 4)
                }
                "d9e8d9c0dfe0" => ("d9e8d9c0dfe0", 6),
                "d9eed9c0dfe0" => ("d9eed9c0dfe0", 6),
                "d9f7dfe0" => ("d9f7dfe0", 4),
                "d9f6dfe0" => ("d9f6dfe0", 4),
                "d9f7d9f6dfe0" => ("d9f7d9f6dfe0", 6),
                "d9d0" => ("d9d0", 2),
                "d9d0dfe0" => ("d9d0dfe0", 4),
                "d9d0d9d0dfe0" => ("d9d0d9d0dfe0", 6),
                "ddc7d9d0d9e8dfe0" => ("ddc7d9d0d9e8dfe0", 8),
                "ddc0" => ("ddc0", 2),
                "ddc0dfe0" => ("ddc0dfe0", 4),
                "ddc0d9c0dfe0" => ("ddc0d9c0dfe0", 6),
                "ddc0d9f7d9e8dfe0" => ("ddc0d9f7d9e8dfe0", 8),
                "ddc7d9e8dfe0" => ("ddc7d9e8dfe0", 6),
                _ if code_bytes.len() == 4
                    && code_bytes.starts_with("ddc")
                    && matches!(code_bytes.as_bytes()[3], b'0'..=b'7') =>
                {
                    (code_bytes.as_str(), 2)
                }
                _ if code_bytes.len() == 8
                    && code_bytes.starts_with("ddc")
                    && matches!(code_bytes.as_bytes()[3], b'0'..=b'7')
                    && code_bytes.ends_with("dfe0") =>
                {
                    (code_bytes.as_str(), 4)
                }
                "d9f6d9f7dfe0" => ("d9f6d9f7dfe0", 6),
                _ if code_bytes.len() == 4
                    && code_bytes.starts_with("d9c")
                    && matches!(code_bytes.as_bytes()[3], b'8'..=b'f') =>
                {
                    (code_bytes.as_str(), 2)
                }
                _ if code_bytes.len() == 8
                    && code_bytes.starts_with("d9c")
                    && matches!(code_bytes.as_bytes()[3], b'8'..=b'f')
                    && code_bytes.ends_with("dfe0") =>
                {
                    (code_bytes.as_str(), 4)
                }
                _ if code_bytes.len() == 12
                    && code_bytes.starts_with("d9c")
                    && matches!(code_bytes.as_bytes()[3], b'8'..=b'f')
                    && code_bytes[0..4] == code_bytes[4..8]
                    && code_bytes.ends_with("dfe0") =>
                {
                    (code_bytes.as_str(), 6)
                }
                "d928" => ("d928", 2),
                "d928d9fcdfe0" => ("d928d9fcdfe0", 6),
                "d938" => ("d938", 2),
                "d938dfe0" => ("d938dfe0", 4),
                "dd38" => ("dd38", 2),
                "dd38dfe0" => ("dd38dfe0", 4),
                "dd18" => ("dd18", 2),
                "dd18dfe0" => ("dd18dfe0", 4),
                "dd10" => ("dd10", 2),
                "dd10dfe0" => ("dd10dfe0", 4),
                "dd00" => ("dd00", 2),
                "dd00dfe0" => ("dd00dfe0", 4),
                "d918" => ("d918", 2),
                "d918dfe0" => ("d918dfe0", 4),
                "d910" => ("d910", 2),
                "d910dfe0" => ("d910dfe0", 4),
                "db18" => ("db18", 2),
                "db18dfe0" => ("db18dfe0", 4),
                "db38" => ("db38", 2),
                "db38dfe0" => ("db38dfe0", 4),
                "db28" => ("db28", 2),
                "db28dfe0" => ("db28dfe0", 4),
                "d900" => ("d900", 2),
                "d900dfe0" => ("d900dfe0", 4),
                "d900d9e0d910dfe0" => ("d900d9e0d910dfe0", 8),
                "d900d9e1d910dfe0" => ("d900d9e1d910dfe0", 8),
                "d900d9e4dfe0" => ("d900d9e4dfe0", 6),
                "d910d9e0dfe0" => ("d910d9e0dfe0", 6),
                "dd10d9e1dfe0" => ("dd10d9e1dfe0", 6),
                "d900ddd1dfe0" => ("d900ddd1dfe0", 6),
                "d900ddd9dfe0" => ("d900ddd9dfe0", 6),
                "d900dd10" => ("d900dd10", 4),
                "d900dd18dfe0" => ("d900dd18dfe0", 6),
                "db2848b80140000000000000df38dfe0dbe2" => {
                    ("db2848b80140000000000000df38dfe0dbe2", 18)
                }
                "db10" => ("db10", 2),
                "db10dfe0" => ("db10dfe0", 4),
                "db00" => ("db00", 2),
                "db00dfe0" => ("db00dfe0", 4),
                "df00" => ("df00", 2),
                "df00dfe0" => ("df00dfe0", 4),
                "df28" => ("df28", 2),
                "df28dfe0" => ("df28dfe0", 4),
                "df18" => ("df18", 2),
                "df18dfe0" => ("df18dfe0", 4),
                "df38" => ("df38", 2),
                "df38dfe0" => ("df38dfe0", 4),
                "df38dfe0dbe2" => ("df38dfe0dbe2", 6),
                "df10" => ("df10", 2),
                "df10dfe0" => ("df10dfe0", 4),
                _ if code_bytes.len() == 4
                    && code_bytes.starts_with("d8c")
                    && matches!(code_bytes.as_bytes()[3], b'0'..=b'f') =>
                {
                    (code_bytes.as_str(), 2)
                }
                _ if code_bytes.len() == 4
                    && code_bytes.starts_with("d8e")
                    && matches!(code_bytes.as_bytes()[3], b'0'..=b'f') =>
                {
                    (code_bytes.as_str(), 2)
                }
                _ if code_bytes.len() == 4
                    && code_bytes.starts_with("d8f")
                    && matches!(code_bytes.as_bytes()[3], b'0'..=b'7') =>
                {
                    (code_bytes.as_str(), 2)
                }
                _ if code_bytes.len() == 4
                    && code_bytes.starts_with("d8f")
                    && matches!(code_bytes.as_bytes()[3], b'8'..=b'f') =>
                {
                    (code_bytes.as_str(), 2)
                }
                // Bare seeded FST/FSTP ST(i) fixtures are restricted to the
                // exact DD D0+i and DD D8+i register encodings.
                _ if code_bytes.len() == 4
                    && code_bytes.starts_with("ddd")
                    && matches!(code_bytes.as_bytes()[3], b'0'..=b'7' | b'8'..=b'f') =>
                {
                    (code_bytes.as_str(), 2)
                }
                _ if code_bytes.len() == 4
                    && code_bytes.starts_with("dcc")
                    && matches!(code_bytes.as_bytes()[3], b'0'..=b'7') =>
                {
                    (code_bytes.as_str(), 2)
                }
                _ if code_bytes.len() == 4
                    && code_bytes.starts_with("dcc")
                    && matches!(code_bytes.as_bytes()[3], b'8'..=b'f') =>
                {
                    (code_bytes.as_str(), 2)
                }
                _ if code_bytes.len() == 4
                    && code_bytes.starts_with("dce")
                    && matches!(code_bytes.as_bytes()[3], b'0'..=b'7') =>
                {
                    (code_bytes.as_str(), 2)
                }
                _ if code_bytes.len() == 4
                    && code_bytes.starts_with("dce")
                    && matches!(code_bytes.as_bytes()[3], b'8'..=b'9' | b'a'..=b'f') =>
                {
                    (code_bytes.as_str(), 2)
                }
                _ if code_bytes.len() == 4
                    && code_bytes.starts_with("dcf")
                    && matches!(code_bytes.as_bytes()[3], b'0'..=b'7') =>
                {
                    (code_bytes.as_str(), 2)
                }
                _ if code_bytes.len() == 4
                    && code_bytes.starts_with("dcf")
                    && matches!(code_bytes.as_bytes()[3], b'8'..=b'f') =>
                {
                    (code_bytes.as_str(), 2)
                }
                _ if code_bytes.len() == 4
                    && code_bytes.starts_with("dec")
                    && matches!(code_bytes.as_bytes()[3], b'0'..=b'7') =>
                {
                    (code_bytes.as_str(), 2)
                }
                _ if code_bytes.len() == 4
                    && code_bytes.starts_with("dec")
                    && matches!(code_bytes.as_bytes()[3], b'8'..=b'f') =>
                {
                    (code_bytes.as_str(), 2)
                }
                _ if code_bytes.len() == 4
                    && code_bytes.starts_with("dee")
                    && matches!(code_bytes.as_bytes()[3], b'0'..=b'f') =>
                {
                    (code_bytes.as_str(), 2)
                }
                _ if code_bytes.len() == 4
                    && code_bytes.starts_with("def")
                    && matches!(code_bytes.as_bytes()[3], b'0'..=b'7') =>
                {
                    (code_bytes.as_str(), 2)
                }
                _ if code_bytes.len() == 4
                    && code_bytes.starts_with("def")
                    && matches!(code_bytes.as_bytes()[3], b'8'..=b'f') =>
                {
                    (code_bytes.as_str(), 2)
                }
                _ if code_bytes.len() == 4
                    && code_bytes.starts_with("ddd")
                    && matches!(code_bytes.as_bytes()[3], b'0'..=b'7') =>
                {
                    (code_bytes.as_str(), 2)
                }
                _ if code_bytes.len() == 4
                    && code_bytes.starts_with("ddd")
                    && matches!(code_bytes.as_bytes()[3], b'8'..=b'f') =>
                {
                    (code_bytes.as_str(), 2)
                }
                _ => {
                    return Err(invalid(
                        "memory.code",
                        "x87 state profile admits one instruction or an exact listed composition",
                    ));
                }
            };
            let is_fsub_alias_unsupported = code_bytes == "d8e0"
                || code_bytes == "d8e8"
                || code_bytes == "dce8"
                || code_bytes == "dce0"
                || code_bytes == "dee0"
                || code_bytes == "dee8";
            let is_fmulp_i0_unsupported = code_bytes == "dec8" && self.stop_rip.is_none();
            let is_fdiv_inexact_unsupported = code_bytes == "d8f1" && self.stop_rip.is_none();
            let is_fdivr_inexact_unsupported = code_bytes == "d8f9" && self.stop_rip.is_none();
            let is_fdivr_sti_st0_inexact_unsupported =
                code_bytes == "dcf1" && self.stop_rip.is_none();
            let is_fdiv_sti_st0_inexact_unsupported =
                code_bytes == "dcf9" && self.stop_rip.is_none();
            let is_fdivp_inexact_unsupported = code_bytes == "def9" && self.stop_rip.is_none();
            let is_fdivrp_inexact_unsupported = code_bytes == "def1" && self.stop_rip.is_none();
            let is_fstp_source_rejected = code_bytes == "ddd9" && self.stop_rip.is_none();
            let is_fst_source_rejected = code_bytes == "ddd1" && self.stop_rip.is_none();
            let is_fxch_source_rejected = code_bytes.len() == 4
                && code_bytes.starts_with("d9c")
                && matches!(code_bytes.as_bytes()[3], b'8'..=b'f')
                && self.stop_rip.is_none()
                && self.instruction_budget == 1
                && self.initial_state.x87.as_ref().is_some_and(|x87| {
                    let top = ((x87.status_word.get() >> 11) & 7) as u8;
                    let source = (code_bytes.as_bytes()[3] as char).to_digit(16).unwrap() as u8 & 7;
                    let indexed = (top + source) & 7;
                    let tag = |index: u8| (x87.tag_word.get() >> (u32::from(index) * 2)) & 3;
                    let valid_operand = |index: u8| {
                        let Some(encoded) = x87.registers.get(usize::from(index)) else {
                            return false;
                        };
                        let Ok(bytes) = decode_bytes(encoded) else {
                            return false;
                        };
                        if bytes.len() != 10 {
                            return false;
                        }
                        let significand = u64::from_le_bytes(bytes[..8].try_into().unwrap());
                        let exponent = (u16::from(bytes[9] & 0x7f) << 8) | u16::from(bytes[8]);
                        match tag(index) {
                            0 => (1..0x7fff).contains(&exponent) && significand & (1 << 63) != 0,
                            1 => significand == 0 && exponent == 0,
                            2 | 3 => false,
                            _ => false,
                        }
                    };
                    !valid_operand(top) || (source != 0 && !valid_operand(indexed))
                });
            let is_top_adjustment_source_rejected = matches!(code_bytes.as_str(), "d9f7" | "d9f6")
                && self.stop_rip.is_none()
                && self.instruction_budget == 1
                && self.initial_state.x87.as_ref().is_some_and(|x87| {
                    x87.control_word.get() != 0x037f || x87.status_word.get() & 0x80ff != 0
                });
            let is_fld_constant_destination_rejected =
                matches!(code_bytes.as_str(), "d9e8" | "d9ee")
                    && self.stop_rip.is_none()
                    && self.instruction_budget == 1
                    && self.initial_state.x87.as_ref().is_some_and(|x87| {
                        let top = ((x87.status_word.get() >> 11) & 7) as u8;
                        let destination = top.wrapping_add(7) & 7;
                        x87.control_word.get() != 0x037f
                            || x87.status_word.get() & 0x80ff != 0
                            || (x87.tag_word.get() >> (u32::from(destination) * 2)) & 3 != 3
                    });
            let is_fld_sti_rejected = code_bytes.len() == 4
                && code_bytes.starts_with("d9c")
                && matches!(code_bytes.as_bytes()[3], b'0'..=b'6')
                && self.stop_rip.is_none()
                && self.instruction_budget == 1
                && self.initial_state.x87.as_ref().is_some_and(|x87| {
                    let top = ((x87.status_word.get() >> 11) & 7) as usize;
                    let source = (top + usize::from(code_bytes.as_bytes()[3] - b'0')) & 7;
                    let destination = (top + 7) & 7;
                    let tag = |index: usize| (x87.tag_word.get() >> (index * 2)) & 3;
                    let raw_is_canonical = |index: usize| {
                        let Some(encoded) = x87.registers.get(index) else {
                            return false;
                        };
                        let Ok(bytes) = decode_bytes(encoded) else {
                            return false;
                        };
                        if bytes.len() != 10 {
                            return false;
                        }
                        let significand = u64::from_le_bytes(bytes[..8].try_into().unwrap());
                        let exponent = (u16::from(bytes[9] & 0x7f) << 8) | u16::from(bytes[8]);
                        match tag(index) {
                            0 => (1..0x7fff).contains(&exponent) && significand & (1 << 63) != 0,
                            1 => significand == 0 && exponent == 0,
                            _ => false,
                        }
                    };
                    x87.control_word.get() != 0x037f
                        || x87.status_word.get() & 0x80ff != 0
                        || !raw_is_canonical(source)
                        || tag(destination) != 3
                });
            let is_fchs_source_rejected = code_bytes == "d9e0" && self.stop_rip.is_none();
            let is_fabs_source_rejected = code_bytes == "d9e1" && self.stop_rip.is_none();
            let is_ftst_source_rejected = code_bytes == "d9e4" && self.stop_rip.is_none();
            let is_frndint_source_rejected = code_bytes == "d9fc"
                && self.stop_rip.is_none()
                && self.instruction_budget == 1
                && self.initial_state.x87.as_ref().is_some_and(|x87| {
                    let top = ((x87.status_word.get() >> 11) & 7) as u8;
                    (x87.tag_word.get() >> (u32::from(top) * 2)) & 3 != 0
                });
            let is_fnstcw_noncanonical_range_exception = code_bytes == "d938"
                && self.stop_rip.is_none()
                && self.instruction_budget == 1
                && self.initial_state.gpr[0].get() == 0x0000_7fff_ffff_ffff;
            let is_fnstsw_noncanonical_range_exception = code_bytes == "dd38"
                && self.stop_rip.is_none()
                && self.instruction_budget == 1
                && self.initial_state.gpr[0].get() == 0x0000_7fff_ffff_ffff;
            let is_fldcw_old_environment_rejected = code_bytes == "d928"
                && self.stop_rip.is_none()
                && self.instruction_budget == 1
                && self.initial_state.x87.as_ref().is_some_and(|x87| {
                    !matches!(x87.control_word.get(), 0x037f | 0x077f | 0x0b7f | 0x0f7f)
                        || x87.status_word.get() & 0x80df != 0
                });
            let is_fldcw_noncanonical_range_exception = code_bytes == "d928"
                && self.stop_rip.is_none()
                && self.instruction_budget == 1
                && !is_fldcw_old_environment_rejected
                && self.initial_state.gpr[0].get() == 0x0000_7fff_ffff_ffff;
            let is_fistp_overflow_rejected = code_bytes == "db18"
                && self.stop_rip.is_none()
                && self.instruction_budget == 1
                && self.initial_state.x87.as_ref().is_some_and(|x87| {
                    let top = ((x87.status_word.get() >> 11) & 7) as u8;
                    let raw = &x87.registers[usize::from(top)];
                    // This schema gate recognizes the literal +2^32 overflow
                    // witness only; execution owns general RC/range semantics.
                    raw.eq_ignore_ascii_case("00000000000000801f40")
                });
            let is_fistp_tag_rejected = code_bytes == "db18"
                && self.stop_rip.is_none()
                && self.instruction_budget == 1
                && self.initial_state.x87.as_ref().is_some_and(|x87| {
                    let top = ((x87.status_word.get() >> 11) & 7) as u32;
                    (x87.tag_word.get() >> (top * 2)) & 3 != 0
                });
            let is_fist_overflow_rejected = code_bytes == "db10"
                && self.stop_rip.is_none()
                && self.instruction_budget == 1
                && self.initial_state.x87.as_ref().is_some_and(|x87| {
                    let top = ((x87.status_word.get() >> 11) & 7) as u8;
                    let raw = &x87.registers[usize::from(top)];
                    // This schema gate recognizes the literal +2^32 overflow
                    // witness only; execution owns general RC/range semantics.
                    raw.eq_ignore_ascii_case("00000000000000801f40")
                });
            let is_fist_tag_rejected = code_bytes == "db10"
                && self.stop_rip.is_none()
                && self.instruction_budget == 1
                && self.initial_state.x87.as_ref().is_some_and(|x87| {
                    let top = ((x87.status_word.get() >> 11) & 7) as u32;
                    (x87.tag_word.get() >> (top * 2)) & 3 != 0
                });
            let is_fistp_word_overflow_rejected = code_bytes == "df18"
                && self.stop_rip.is_none()
                && self.instruction_budget == 1
                && self.initial_state.x87.as_ref().is_some_and(|x87| {
                    let top = ((x87.status_word.get() >> 11) & 7) as u8;
                    let raw = &x87.registers[usize::from(top)];
                    raw.eq_ignore_ascii_case("00000000000000800e40")
                });
            let is_fistp_word_exponent_rejected = code_bytes == "df18"
                && self.stop_rip.is_none()
                && self.instruction_budget == 1
                && self.initial_state.x87.as_ref().is_some_and(|x87| {
                    let top = ((x87.status_word.get() >> 11) & 7) as u8;
                    let raw = &x87.registers[usize::from(top)];
                    raw.eq_ignore_ascii_case("00000000000000800f40")
                });
            let is_fistp_word_tag_rejected = code_bytes == "df18"
                && self.stop_rip.is_none()
                && self.instruction_budget == 1
                && self.initial_state.x87.as_ref().is_some_and(|x87| {
                    let top = ((x87.status_word.get() >> 11) & 7) as u32;
                    (x87.tag_word.get() >> (top * 2)) & 3 != 0
                });
            let is_fistp_word_environment_rejected = code_bytes == "df18"
                && self.stop_rip.is_none()
                && self.instruction_budget == 1
                && self.initial_state.x87.as_ref().is_some_and(|x87| {
                    !matches!(x87.control_word.get(), 0x037f | 0x077f | 0x0b7f | 0x0f7f)
                        || x87.status_word.get() & 0x80df != 0
                });
            let is_fistp_qword_overflow_rejected = code_bytes == "df38"
                && self.stop_rip.is_none()
                && self.instruction_budget == 1
                && self.initial_state.x87.as_ref().is_some_and(|x87| {
                    let top = ((x87.status_word.get() >> 11) & 7) as usize;
                    x87.registers[top].eq_ignore_ascii_case("00000000000000803e40")
                });
            let is_fist_word_overflow_rejected = code_bytes == "df10"
                && self.stop_rip.is_none()
                && self.instruction_budget == 1
                && self.initial_state.x87.as_ref().is_some_and(|x87| {
                    let top = ((x87.status_word.get() >> 11) & 7) as usize;
                    x87.registers[top].eq_ignore_ascii_case("00000000000000800e40")
                });
            let is_fist_word_exponent_rejected = code_bytes == "df10"
                && self.stop_rip.is_none()
                && self.instruction_budget == 1
                && self.initial_state.x87.as_ref().is_some_and(|x87| {
                    let top = ((x87.status_word.get() >> 11) & 7) as usize;
                    x87.registers[top].eq_ignore_ascii_case("00000000000000800f40")
                });
            let is_fist_word_tag_rejected = code_bytes == "df10"
                && self.stop_rip.is_none()
                && self.instruction_budget == 1
                && self.initial_state.x87.as_ref().is_some_and(|x87| {
                    let top = ((x87.status_word.get() >> 11) & 7) as u32;
                    (x87.tag_word.get() >> (top * 2)) & 3 != 0
                });
            let is_fist_word_environment_rejected = code_bytes == "df10"
                && self.stop_rip.is_none()
                && self.instruction_budget == 1
                && self.initial_state.x87.as_ref().is_some_and(|x87| {
                    !matches!(x87.control_word.get(), 0x037f | 0x077f | 0x0b7f | 0x0f7f)
                        || x87.status_word.get() & 0x80df != 0
                });
            let is_fld_m80_pseudo_infinity_rejected = code_bytes == "db28"
                && self.stop_rip.is_none()
                && self.instruction_budget == 1
                && self.memory.iter().any(|region| {
                    let address = self.initial_state.gpr[0].get();
                    let offset = address.checked_sub(region.guest_address.get());
                    offset.is_some_and(|offset| {
                        let start = usize::try_from(offset)
                            .ok()
                            .and_then(|offset| offset.checked_mul(2));
                        start.is_some_and(|start| {
                            start.checked_add(20).is_some_and(|end| {
                                region.initial_bytes_hex.len() >= end && {
                                    match region.initial_bytes_hex.get(start..end) {
                                        Some(raw) => {
                                            let mut bytes = [0_u8; 10];
                                            let decoded = bytes
                                                .iter_mut()
                                                .zip(raw.as_bytes().chunks_exact(2))
                                                .all(|(byte, pair)| {
                                                    std::str::from_utf8(pair)
                                                        .ok()
                                                        .and_then(|pair| {
                                                            u8::from_str_radix(pair, 16).ok()
                                                        })
                                                        .map(|value| *byte = value)
                                                        .is_some()
                                                });
                                            if !decoded {
                                                false
                                            } else {
                                                let significand = u64::from_le_bytes(
                                                    bytes[..8].try_into().expect("binary80 width"),
                                                );
                                                let sign_exponent =
                                                    u16::from_le_bytes([bytes[8], bytes[9]]);
                                                sign_exponent == 0x7fff && significand == 0
                                            }
                                        }
                                        None => false,
                                    }
                                }
                            })
                        })
                    })
                });
            let is_fld_m32_environment_rejected = code_bytes == "d900"
                && self.stop_rip.is_none()
                && self.instruction_budget == 1
                && self.initial_state.x87.as_ref().is_some_and(|x87| {
                    !matches!(x87.control_word.get(), 0x037f | 0x077f | 0x0b7f | 0x0f7f)
                        || x87.status_word.get() & 0x80c4 != 0
                });
            let is_fld_m64_environment_rejected = code_bytes == "dd00"
                && self.stop_rip.is_none()
                && self.instruction_budget == 1
                && self.initial_state.x87.as_ref().is_some_and(|x87| {
                    !matches!(x87.control_word.get(), 0x037f | 0x077f | 0x0b7f | 0x0f7f)
                        || x87.status_word.get() & 0x80c4 != 0
                });
            let is_fcom_source_rejected = code_bytes.len() == 4
                && code_bytes.starts_with("d8d")
                && matches!(code_bytes.as_bytes()[3], b'0'..=b'7')
                && self.stop_rip.is_none();
            let is_fcomp_source_rejected = code_bytes.len() == 4
                && code_bytes.starts_with("d8d")
                && matches!(code_bytes.as_bytes()[3], b'8'..=b'f')
                && self.stop_rip.is_none();
            let is_fcompp_source_rejected = code_bytes == "ded9"
                && self.stop_rip.is_none()
                && self.instruction_budget == 1
                && self.initial_state.x87.as_ref().is_some_and(|x87| {
                    let top = ((x87.status_word.get() >> 11) & 7) as u8;
                    let rhs = (top + 1) & 7;
                    (x87.tag_word.get() >> (u32::from(rhs) * 2)) & 3 != 0
                });
            let is_fucompp_source_rejected = code_bytes == "dae9"
                && self.stop_rip.is_none()
                && self.instruction_budget == 1
                && self.initial_state.x87.as_ref().is_some_and(|x87| {
                    let top = ((x87.status_word.get() >> 11) & 7) as u8;
                    let rhs = (top + 1) & 7;
                    (x87.tag_word.get() >> (u32::from(rhs) * 2)) & 3 != 0
                });
            let is_fucom_source_rejected = code_bytes == "dde1"
                && self.stop_rip.is_none()
                && self.instruction_budget == 1
                && self.initial_state.x87.as_ref().is_some_and(|x87| {
                    let top = ((x87.status_word.get() >> 11) & 7) as u8;
                    let rhs = (top + 1) & 7;
                    (x87.tag_word.get() >> (u32::from(rhs) * 2)) & 3 != 0
                });
            let is_fucomp_source_rejected = code_bytes == "dde9"
                && self.stop_rip.is_none()
                && self.instruction_budget == 1
                && self.initial_state.x87.as_ref().is_some_and(|x87| {
                    let top = ((x87.status_word.get() >> 11) & 7) as u8;
                    let rhs = (top + 1) & 7;
                    (x87.tag_word.get() >> (u32::from(rhs) * 2)) & 3 != 0
                });
            let is_fcomi_source_rejected = fcomi_family_source(&code_bytes).is_some_and(|source| {
                self.stop_rip.is_none()
                    && self.instruction_budget == 1
                    && self.initial_state.x87.as_ref().is_some_and(|x87| {
                        let top = ((x87.status_word.get() >> 11) & 7) as u8;
                        let actual = (top + source) & 7;
                        let tag = |index: u8| (x87.tag_word.get() >> (u32::from(index) * 2)) & 3;
                        x87.control_word.get() == 0x037f
                            && x87.status_word.get() & 0x80ff == 0
                            && tag(top) == 0
                            && if source == 0 {
                                x87.registers[usize::from(top)] == "00000000000000000000"
                                    && tag((top + 1) & 7) == 0
                            } else {
                                tag(actual) != 0 && (source == 1 || tag((top + 1) & 7) == 0)
                            }
                    })
            });
            let is_fcmovb_operand_rejected = fcmovb_family_source(&code_bytes).is_some()
                && self.stop_rip.is_none()
                && self.instruction_budget == 1
                && self.initial_state.x87.as_ref().is_some_and(|x87| {
                    let top = ((x87.status_word.get() >> 11) & 7) as u8;
                    let source = (top + fcmovb_family_source(&code_bytes).unwrap()) & 7;
                    let valid_operand = |index: u8| {
                        let tag = (x87.tag_word.get() >> (u32::from(index) * 2)) & 3;
                        let Some(encoded) = x87.registers.get(usize::from(index)) else {
                            return false;
                        };
                        let Ok(bytes) = decode_bytes(encoded) else {
                            return false;
                        };
                        if bytes.len() != 10 {
                            return false;
                        }
                        let significand = u64::from_le_bytes(bytes[..8].try_into().unwrap());
                        let exponent = u16::from_le_bytes([bytes[8], bytes[9] & 0x7f]);
                        match tag {
                            0 => (1..0x7fff).contains(&exponent) && significand & (1 << 63) != 0,
                            1 => significand == 0 && exponent == 0 && matches!(bytes[9], 0 | 0x80),
                            _ => false,
                        }
                    };
                    !valid_operand(top) || !valid_operand(source)
                });
            let is_fcmovb_environment_rejected = fcmovb_family_source(&code_bytes).is_some()
                && self.stop_rip.is_none()
                && self.instruction_budget == 1
                && self.initial_state.x87.as_ref().is_some_and(|x87| {
                    x87.control_word.get() != 0x037f || x87.status_word.get() & 0x82ff != 0
                });
            let is_fcmovnb_operand_rejected = fcmovnb_family_source(&code_bytes).is_some()
                && self.stop_rip.is_none()
                && self.instruction_budget == 1
                && self.initial_state.x87.as_ref().is_some_and(|x87| {
                    let top = ((x87.status_word.get() >> 11) & 7) as usize;
                    let source =
                        (top + usize::from(fcmovnb_family_source(&code_bytes).unwrap())) & 7;
                    let valid_operand = |index: usize| {
                        let tag = (x87.tag_word.get() >> (index * 2)) & 3;
                        let Ok(bytes) = decode_bytes(&x87.registers[index]) else {
                            return false;
                        };
                        if bytes.len() != 10 {
                            return false;
                        }
                        let significand = u64::from_le_bytes(bytes[..8].try_into().unwrap());
                        let exponent = u16::from_le_bytes([bytes[8], bytes[9] & 0x7f]);
                        match tag {
                            0 => (1..0x7fff).contains(&exponent) && significand & (1 << 63) != 0,
                            1 => significand == 0 && exponent == 0 && matches!(bytes[9], 0 | 0x80),
                            _ => false,
                        }
                    };
                    !valid_operand(top) || (source != top && !valid_operand(source))
                });
            let is_fcmovnb_environment_rejected = fcmovnb_family_source(&code_bytes).is_some()
                && self.stop_rip.is_none()
                && self.instruction_budget == 1
                && self.initial_state.x87.as_ref().is_some_and(|x87| {
                    x87.control_word.get() != 0x037f || x87.status_word.get() & 0x82ff != 0
                });
            let fcmove_index = (code_bytes.len() == 4
                && code_bytes.starts_with("dac")
                && matches!(code_bytes.as_bytes()[3], b'8'..=b'9' | b'a'..=b'f'))
            .then(|| u8::from_str_radix(&code_bytes[3..4], 16).unwrap() & 7);
            let is_fcmove_operand_rejected = fcmove_index.is_some()
                && self.stop_rip.is_none()
                && self.instruction_budget == 1
                && self.initial_state.x87.as_ref().is_some_and(|x87| {
                    let top = ((x87.status_word.get() >> 11) & 7) as usize;
                    let source = (top + usize::from(fcmove_index.unwrap())) & 7;
                    let valid_operand = |index: usize| {
                        let tag = (x87.tag_word.get() >> (index * 2)) & 3;
                        let Ok(bytes) = decode_bytes(&x87.registers[index]) else {
                            return false;
                        };
                        if bytes.len() != 10 {
                            return false;
                        }
                        let significand = u64::from_le_bytes(bytes[..8].try_into().unwrap());
                        let exponent = u16::from_le_bytes([bytes[8], bytes[9] & 0x7f]);
                        match tag {
                            0 => (1..0x7fff).contains(&exponent) && significand & (1 << 63) != 0,
                            1 => significand == 0 && exponent == 0 && matches!(bytes[9], 0 | 0x80),
                            _ => false,
                        }
                    };
                    !valid_operand(top) || (fcmove_index != Some(0) && !valid_operand(source))
                });
            let is_fcmove_environment_rejected = fcmove_index.is_some()
                && self.stop_rip.is_none()
                && self.instruction_budget == 1
                && self.initial_state.x87.as_ref().is_some_and(|x87| {
                    x87.control_word.get() != 0x037f || x87.status_word.get() & 0x82ff != 0
                });
            let is_fcmovne_operand_rejected = fcmovne_family_source(&code_bytes).is_some()
                && self.stop_rip.is_none()
                && self.instruction_budget == 1
                && self.initial_state.x87.as_ref().is_some_and(|x87| {
                    let top = ((x87.status_word.get() >> 11) & 7) as usize;
                    let source =
                        (top + usize::from(fcmovne_family_source(&code_bytes).unwrap())) & 7;
                    let valid_operand = |index: usize| {
                        let tag = (x87.tag_word.get() >> (index * 2)) & 3;
                        let Ok(bytes) = decode_bytes(&x87.registers[index]) else {
                            return false;
                        };
                        if bytes.len() != 10 {
                            return false;
                        }
                        let significand = u64::from_le_bytes(bytes[..8].try_into().unwrap());
                        let exponent = u16::from_le_bytes([bytes[8], bytes[9] & 0x7f]);
                        match tag {
                            0 => (1..0x7fff).contains(&exponent) && significand & (1 << 63) != 0,
                            1 => significand == 0 && exponent == 0 && matches!(bytes[9], 0 | 0x80),
                            _ => false,
                        }
                    };
                    !valid_operand(top) || !valid_operand(source)
                });
            let is_fcmovne_environment_rejected = fcmovne_family_source(&code_bytes).is_some()
                && self.stop_rip.is_none()
                && self.expected_exit == ExpectedExit::Unsupported
                && self.instruction_budget == 1
                && self.initial_state.x87.as_ref().is_some_and(|x87| {
                    x87.control_word.get() != 0x037f || x87.status_word.get() & 0x82ff != 0
                });
            let fcmovbe_source = fcmovbe_bare_source(&code_bytes);
            let is_fcmovbe_operand_rejected = fcmovbe_source.is_some()
                && self.stop_rip.is_none()
                && self.expected_exit == ExpectedExit::Unsupported
                && self.instruction_budget == 1
                && self.initial_state.x87.as_ref().is_some_and(|x87| {
                    let top = ((x87.status_word.get() >> 11) & 7) as usize;
                    let source = (top + usize::from(fcmovbe_source.unwrap())) & 7;
                    let valid_operand = |index: usize| {
                        let tag = (x87.tag_word.get() >> (index * 2)) & 3;
                        let Ok(bytes) = decode_bytes(&x87.registers[index]) else {
                            return false;
                        };
                        if bytes.len() != 10 {
                            return false;
                        }
                        let significand = u64::from_le_bytes(bytes[..8].try_into().unwrap());
                        let exponent = u16::from_le_bytes([bytes[8], bytes[9] & 0x7f]);
                        match tag {
                            0 => (1..0x7fff).contains(&exponent) && significand & (1 << 63) != 0,
                            1 => significand == 0 && exponent == 0 && matches!(bytes[9], 0 | 0x80),
                            _ => false,
                        }
                    };
                    !valid_operand(top) || (fcmovbe_source != Some(0) && !valid_operand(source))
                });
            let is_fcmovbe_environment_rejected = fcmovbe_source.is_some()
                && self.stop_rip.is_none()
                && self.expected_exit == ExpectedExit::Unsupported
                && self.instruction_budget == 1
                && self.initial_state.x87.as_ref().is_some_and(|x87| {
                    x87.control_word.get() != 0x037f || x87.status_word.get() & 0x82ff != 0
                });
            let fcmovnbe_source = fcmovnbe_bare_source(&code_bytes);
            let is_fcmovnbe_operand_rejected = fcmovnbe_source.is_some()
                && self.stop_rip.is_none()
                && self.expected_exit == ExpectedExit::Unsupported
                && self.instruction_budget == 1
                && self.initial_state.x87.as_ref().is_some_and(|x87| {
                    let top = ((x87.status_word.get() >> 11) & 7) as usize;
                    let source = (top + usize::from(fcmovnbe_source.unwrap())) & 7;
                    let valid_operand = |index: usize| {
                        let tag = (x87.tag_word.get() >> (index * 2)) & 3;
                        let Ok(bytes) = decode_bytes(&x87.registers[index]) else {
                            return false;
                        };
                        if bytes.len() != 10 {
                            return false;
                        }
                        let significand = u64::from_le_bytes(bytes[..8].try_into().unwrap());
                        let exponent = u16::from_le_bytes([bytes[8], bytes[9] & 0x7f]);
                        match tag {
                            0 => (1..0x7fff).contains(&exponent) && significand & (1 << 63) != 0,
                            1 => significand == 0 && exponent == 0 && matches!(bytes[9], 0 | 0x80),
                            _ => false,
                        }
                    };
                    !valid_operand(top) || !valid_operand(source)
                });
            let is_fcmovnbe_environment_rejected = fcmovnbe_source.is_some()
                && self.stop_rip.is_none()
                && self.expected_exit == ExpectedExit::Unsupported
                && self.instruction_budget == 1
                && self.initial_state.x87.as_ref().is_some_and(|x87| {
                    x87.control_word.get() != 0x037f || x87.status_word.get() & 0x82ff != 0
                });
            let fcmovnu_source = fcmovnu_bare_source(&code_bytes);
            let fcmovnu_operand_rejected = fcmovnu_source.is_some()
                && self.stop_rip.is_none()
                && self.expected_exit == ExpectedExit::Unsupported
                && self.instruction_budget == 1
                && self.initial_state.x87.as_ref().is_some_and(|x87| {
                    let top = ((x87.status_word.get() >> 11) & 7) as usize;
                    let source = (top + usize::from(fcmovnu_source.unwrap())) & 7;
                    let valid_operand = |index: usize| {
                        let tag = (x87.tag_word.get() >> (index * 2)) & 3;
                        let Ok(bytes) = decode_bytes(&x87.registers[index]) else {
                            return false;
                        };
                        if bytes.len() != 10 {
                            return false;
                        }
                        let significand = u64::from_le_bytes(bytes[..8].try_into().unwrap());
                        let exponent = u16::from_le_bytes([bytes[8], bytes[9] & 0x7f]);
                        match tag {
                            0 => (1..0x7fff).contains(&exponent) && significand & (1 << 63) != 0,
                            1 => significand == 0 && exponent == 0 && matches!(bytes[9], 0 | 0x80),
                            _ => false,
                        }
                    };
                    !valid_operand(top) || (fcmovnu_source.unwrap() != 0 && !valid_operand(source))
                });
            let fcmovnu_environment_rejected = fcmovnu_source.is_some()
                && self.stop_rip.is_none()
                && self.expected_exit == ExpectedExit::Unsupported
                && self.instruction_budget == 1
                && self.initial_state.x87.as_ref().is_some_and(|x87| {
                    x87.control_word.get() != 0x037f || x87.status_word.get() & 0x82ff != 0
                });
            let fcmovu_source = fcmovu_bare_source(&code_bytes);
            let is_fcmovu_operand_rejected = fcmovu_source.is_some()
                && self.stop_rip.is_none()
                && self.expected_exit == ExpectedExit::Unsupported
                && self.instruction_budget == 1
                && self.initial_state.x87.as_ref().is_some_and(|x87| {
                    let top = ((x87.status_word.get() >> 11) & 7) as usize;
                    let source = (top + usize::from(fcmovu_source.unwrap())) & 7;
                    let valid_operand = |index: usize| {
                        let tag = (x87.tag_word.get() >> (index * 2)) & 3;
                        let Ok(bytes) = decode_bytes(&x87.registers[index]) else {
                            return false;
                        };
                        if bytes.len() != 10 {
                            return false;
                        }
                        let significand = u64::from_le_bytes(bytes[..8].try_into().unwrap());
                        let exponent = u16::from_le_bytes([bytes[8], bytes[9] & 0x7f]);
                        match tag {
                            0 => (1..0x7fff).contains(&exponent) && significand & (1 << 63) != 0,
                            1 => significand == 0 && exponent == 0 && matches!(bytes[9], 0 | 0x80),
                            _ => false,
                        }
                    };
                    !valid_operand(top) || (fcmovu_source != Some(0) && !valid_operand(source))
                });
            let is_fcmovu_environment_rejected = fcmovu_source.is_some()
                && self.stop_rip.is_none()
                && self.expected_exit == ExpectedExit::Unsupported
                && self.instruction_budget == 1
                && self.initial_state.x87.as_ref().is_some_and(|x87| {
                    x87.control_word.get() != 0x037f || x87.status_word.get() & 0x82ff != 0
                });
            let is_ffree_environment_rejected = code_bytes.len() == 4
                && code_bytes.starts_with("ddc")
                && matches!(code_bytes.as_bytes()[3], b'0'..=b'7')
                && self.stop_rip.is_none()
                && self.expected_exit == ExpectedExit::Unsupported
                && self.instruction_budget == 1
                && self.initial_state.x87.as_ref().is_some_and(|x87| {
                    x87.control_word.get() != 0x037f || x87.status_word.get() & 0x80ff != 0
                });
            let is_fnop_environment_rejected = code_bytes == "d9d0"
                && self.stop_rip.is_none()
                && self.expected_exit == ExpectedExit::Unsupported
                && self.instruction_budget == 1
                && self.initial_state.x87.as_ref().is_some_and(|x87| {
                    x87.control_word.get() != 0x037f || x87.status_word.get() & 0x80ff != 0
                });
            let is_fxam_environment_rejected = code_bytes == "d9e5"
                && self.stop_rip.is_none()
                && self.expected_exit == ExpectedExit::Unsupported
                && self.instruction_budget == 1
                && self.initial_state.x87.as_ref().is_some_and(|x87| {
                    x87.control_word.get() != 0x037f
                        || x87.status_word.get() & 0x80ff != 0
                        || x87.tag_word.get() >> (2 * (((x87.status_word.get() >> 11) & 7) as u32))
                            & 3
                            != 0
                });
            let is_ffree_fld_st0_rejected = code_bytes == "ddc0d9c0dfe0"
                && self.stop_rip.is_none()
                && self.expected_exit == ExpectedExit::Unsupported
                && self.instruction_budget == 3;
            let expected_budget = match code_bytes.as_str() {
                _ if fcmovu_stream(&code_bytes) => fcmovu_budget(&code_bytes).unwrap(),
                _ if fcmovnu_stream(&code_bytes) => fcmovnu_budget(&code_bytes).unwrap(),
                "db2848b80140000000000000df38dfe0dbe2" | "db28d9fcdfe0dbe2d9fc" => 5,
                "db28d9fcd9fcdfe0dbe2d9fc"
                | "d9e8d9e0dbf1dac1dfe0"
                | "d9e8d9e0d9e8dbf1dac1dfe0"
                | "d9e8d9e8dbf1d9e0dac9dfe0"
                | "d9e8d9e8d9e0dbf1dac9dfe0"
                | "d9e8d9e8dbf1d9e0dbc9dfe0"
                | "d9e8d9e8d9e0dbf1dbc9dfe0"
                | "d9e8d9e0d9e8dbf1dad1dfe0"
                | "d9e8d9e8dbf1d9e0dad1dfe0"
                | "d9e8d9e8d9e0dbf1dad1dfe0"
                | "d9e8d9e8d9e0dbf1dbc1dfe0"
                | "d9e8d9e0d9e8dbf1dbc1dfe0" => 6,
                _ if fcmovnb_family_source(&code_bytes).is_some() => {
                    if code_bytes.len() == 4 {
                        1
                    } else {
                        2
                    }
                }
                "dac1dac1dfe0" => 3,
                "dac9dac9dfe0" => 3,
                "dad1dad1dfe0" => 3,
                "dbd1dbd1dfe0" => 3,
                "dbc9dbc9dfe0" => 3,
                _ if fcmove_family_source(&code_bytes).is_some() => {
                    if code_bytes.len() == 4 {
                        1
                    } else {
                        2
                    }
                }
                _ if fcmovne_family_source(&code_bytes).is_some() => {
                    if code_bytes.len() == 4 {
                        1
                    } else {
                        2
                    }
                }
                _ if fcmovnbe_stream(&code_bytes) => match code_bytes.as_str() {
                    _ if code_bytes.len() == 4 => 1,
                    _ if code_bytes.len() == 8 && code_bytes.ends_with("dfe0") => 2,
                    "dbd1dbd1dfe0" => 3,
                    _ => 6,
                },
                _ if fcmovbe_stream(&code_bytes) => {
                    if code_bytes.len() == 4 {
                        1
                    } else if code_bytes.len() == 8 && code_bytes.starts_with("da") {
                        2
                    } else if code_bytes == "dad1dad1dfe0" {
                        3
                    } else {
                        6
                    }
                }
                "dac1dfe0" | "dbc1dfe0" => 2,
                "dbc1dbc1dfe0" => 3,
                "dbc1" => 1,
                "d9fcdfe0dbe2" => 3,
                "ddc0d9c0dfe0" => 3,
                "ddc0d9f7d9e8dfe0" => 4,
                "d9d0" | "d9e5" => 1,
                "d9d0dfe0" => 2,
                "d9d0d9d0dfe0" => 3,
                "ddc7d9d0d9e8dfe0" => 4,
                "d9e5dfe0" => 2,
                "d9e5d9e5dfe0" => 3,
                "d9e8d9e5dfe0" | "d9eed9e5dfe0" => 3,
                "d9e8d9e0d9e5dfe0" | "d9eed9e0d9e5dfe0" => 4,
                "d910d900dfe0dbe2" | "dd10dd00dfe0dbe2" => 4,
                "d900d9e0d910dfe0" | "d900d9e1d910dfe0" => 4,
                "d900d9e4dfe0" | "d900ddd1dfe0" | "d900ddd9dfe0" | "d910d9e0dfe0"
                | "dd10d9e1dfe0" => 3,
                _ => match code_bytes.len() {
                    8 => 2,
                    12 => 3,
                    _ => 1,
                },
            };
            if code.guest_address != self.initial_state.rip
                || code.length != expected_length
                || !code.permissions.execute
                || !code.initial_bytes_hex.eq_ignore_ascii_case(expected_bytes)
                || self.instruction_budget != expected_budget
                || if is_fsub_alias_unsupported
                    || is_fmulp_i0_unsupported
                    || is_fdiv_inexact_unsupported
                    || is_fdivr_inexact_unsupported
                    || is_fdivr_sti_st0_inexact_unsupported
                    || is_fdiv_sti_st0_inexact_unsupported
                    || is_fdivp_inexact_unsupported
                    || is_fdivrp_inexact_unsupported
                    || is_fstp_source_rejected
                    || is_fst_source_rejected
                    || is_fxch_source_rejected
                    || is_top_adjustment_source_rejected
                    || is_fld_constant_destination_rejected
                    || is_fld_sti_rejected
                    || is_fchs_source_rejected
                    || is_fabs_source_rejected
                    || is_ftst_source_rejected
                    || is_frndint_source_rejected
                    || is_fnstcw_noncanonical_range_exception
                    || is_fnstsw_noncanonical_range_exception
                    || is_fldcw_noncanonical_range_exception
                    || is_fldcw_old_environment_rejected
                    || is_fistp_overflow_rejected
                    || is_fistp_tag_rejected
                    || is_fist_overflow_rejected
                    || is_fist_tag_rejected
                    || is_fistp_word_overflow_rejected
                    || is_fistp_word_exponent_rejected
                    || is_fistp_word_tag_rejected
                    || is_fistp_word_environment_rejected
                    || is_fistp_qword_overflow_rejected
                    || is_fist_word_overflow_rejected
                    || is_fist_word_exponent_rejected
                    || is_fist_word_tag_rejected
                    || is_fist_word_environment_rejected
                    || is_fld_m80_pseudo_infinity_rejected
                    || is_fld_m32_environment_rejected
                    || is_fld_m64_environment_rejected
                    || is_fcom_source_rejected
                    || is_fcomp_source_rejected
                    || is_fcompp_source_rejected
                    || is_fucompp_source_rejected
                    || is_fucom_source_rejected
                    || is_fucomp_source_rejected
                    || is_fcomi_source_rejected
                    || is_fcmovb_operand_rejected
                    || is_fcmovb_environment_rejected
                    || is_fcmovnb_operand_rejected
                    || is_fcmovnb_environment_rejected
                    || is_fcmove_operand_rejected
                    || is_fcmove_environment_rejected
                    || is_fcmovne_operand_rejected
                    || is_fcmovne_environment_rejected
                    || is_fcmovbe_operand_rejected
                    || is_fcmovbe_environment_rejected
                    || is_fcmovnbe_operand_rejected
                    || is_fcmovnbe_environment_rejected
                    || is_fcmovu_operand_rejected
                    || is_fcmovu_environment_rejected
                    || fcmovnu_operand_rejected
                    || fcmovnu_environment_rejected
                    || is_ffree_environment_rejected
                    || is_fnop_environment_rejected
                    || is_fxam_environment_rejected
                    || is_ffree_fld_st0_rejected
                {
                    self.stop_rip.is_some()
                        || self.expected_exit
                            != if is_fnstcw_noncanonical_range_exception
                                || is_fnstsw_noncanonical_range_exception
                                || is_fldcw_noncanonical_range_exception
                            {
                                ExpectedExit::Exception
                            } else {
                                ExpectedExit::Unsupported
                            }
                } else {
                    self.stop_rip.map(HexU64::get)
                        != self
                            .initial_state
                            .rip
                            .get()
                            .checked_add(u64::from(expected_length))
                        || self.expected_exit != ExpectedExit::StopAddress
                }
            {
                return Err(invalid(
                    "memory.code",
                    "x87 state profile admits one exact instruction with budget 1 and the following stop RIP",
                ));
            }
        }
        // RIP is architectural u64. Unmapped/noncanonical entries are valid fault tests.
        Ok(())
    }

    pub fn convert_v1(old: &crate::case::LoadedFixture) -> Result<Self, FormatError> {
        old.fixture.validate()?;
        digest_field(&old.input_sha256)?;
        let source = &old.fixture;
        let initial = &source.initial_state;
        let mut state = LongModeState::default();
        state.gpr[..4].copy_from_slice(&[
            initial.rax.get(),
            initial.rcx.get(),
            initial.rdx.get(),
            initial.rbx.get(),
        ]);
        state.rip = initial.rip.get();
        state.rflags = initial.rflags.get();
        let length = source.program.bytes()?.len() as u32;
        let fixture = Self {
            schema_version: SCHEMA_VERSION,
            case_id: source.case_id.clone(),
            profile: Profile::LongIntegerV1,
            cli_cpl: None,
            efer_cpl: None,
            efer_value: None,
            provenance: source.provenance.clone(),
            seed: source.seed.map(HexU64),
            converted_from_v1_sha256: Some(old.input_sha256.clone()),
            initial_state: IntegerState::try_from(&state)?,
            xorpd_prerequisites: None,
            minss_invalid_controls: None,
            maxss_unmasked_controls: None,
            minsd_unmasked_controls: None,
            maxsd_unmasked_controls: None,
            minpd_unmasked_controls: None,
            minps_unmasked_controls: None,
            maxps_unmasked_controls: None,
            maxpd_unmasked_controls: None,
            cvtdq2pd_controls: None,
            cvtdq2ps_controls: None,
            cvtss2sd_controls: None,
            cvtps2pd_controls: None,
            comiss_ucomiss_controls: None,
            comisd_ucomisd_controls: None,
            cvtsi2sd_r32_controls: None,
            cvtsi2sd_r64_controls: None,
            cvtsi2ss_controls: None,
            fxsave64_cr0_controls: None,
            fxrstor64_cr0_controls: None,
            rdtsc_controls: None,
            memory: vec![MemoryRegionSpec {
                region_id: "code".into(),
                guest_address: HexU64(0),
                length,
                permissions: MemoryPermissions {
                    read: true,
                    write: false,
                    execute: true,
                },
                initial_bytes_hex: source.program.bytes_hex.clone(),
                comparison_mask_hex: "ff".repeat(length as usize),
            }],
            stop_rip: Some(HexU64(u64::from(length))),
            instruction_budget: source.instruction_budget,
            wall_timeout_ms: source.wall_timeout_ms,
            rflags_mask: source.comparison.rflags_mask,
            expected_exit: match source.expected_exit {
                crate::case::ExpectedExit::ReachedCodeEnd => ExpectedExit::StopAddress,
                crate::case::ExpectedExit::InstructionBudgetExhausted => {
                    ExpectedExit::InstructionBudgetExhausted
                }
            },
        };
        fixture.validate()?;
        Ok(fixture)
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Outcome {
    StopAddress {},
    Halted {},
    InstructionBudgetExhausted {},
    RepIterationBudgetExhausted {},
    StopRequested {},
    Exception {},
    Unsupported { reason: String },
    TimedOut { reason: String },
    Signaled { signal: u32, reason: String },
    HarnessError { reason: String },
}

impl Outcome {
    fn architectural(&self) -> bool {
        matches!(
            self,
            Self::StopAddress {}
                | Self::Halted {}
                | Self::InstructionBudgetExhausted {}
                | Self::RepIterationBudgetExhausted {}
                | Self::StopRequested {}
                | Self::Exception {}
        )
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Snapshot {
    pub state: IntegerState,
    pub rflags_defined: HexU64,
    pub memory: Vec<MemoryRegionSnapshot>,
    /// Immutable XORPD gate inputs, echoed into results only for S2-38b.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub xorpd_prerequisites: Option<XorpdPrerequisiteSnapshot>,
    /// Immutable MINSS gate inputs, echoed into results only for S2-38c.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub minss_invalid_controls: Option<MinssInvalidControlSnapshot>,
    /// Immutable MAXSS gate inputs, echoed only for S2-38f.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub maxss_unmasked_controls: Option<MaxssUnmaskedControlSnapshot>,
    /// Immutable MINSD gate inputs, echoed only for S2-38g.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub minsd_unmasked_controls: Option<MinsdUnmaskedControlSnapshot>,
    /// Immutable MAXSD gate inputs, echoed only for S2-38h.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub maxsd_unmasked_controls: Option<MaxsdUnmaskedControlSnapshot>,
    /// Immutable MINPD gate inputs, echoed only for S2-38i.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub minpd_unmasked_controls: Option<MinpdUnmaskedControlSnapshot>,
    /// Immutable MINPS gate inputs, echoed only for S2-38k.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub minps_unmasked_controls: Option<MinpsUnmaskedControlSnapshot>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub maxps_unmasked_controls: Option<MaxpsUnmaskedControlSnapshot>,
    /// Immutable MAXPD gate inputs, echoed only for S2-38j.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub maxpd_unmasked_controls: Option<MaxpdUnmaskedControlSnapshot>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cvtdq2pd_controls: Option<Cvtdq2pdControlSnapshot>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cvtdq2ps_controls: Option<Cvtdq2psControlSnapshot>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cvtss2sd_controls: Option<Cvtss2sdControlSnapshot>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cvtps2pd_controls: Option<Cvtps2pdControlSnapshot>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub comiss_ucomiss_controls: Option<ComissUcomissControlSnapshot>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub comisd_ucomisd_controls: Option<ComisdUcomisdControlSnapshot>,
    /// Immutable CVTSI2SD r32 gate inputs and the irrelevant OSXMMEXCPT echo.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cvtsi2sd_r32_controls: Option<Cvtsi2sdR32ControlSnapshot>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cvtsi2sd_r64_controls: Option<Cvtsi2sdR32ControlSnapshot>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cvtsi2ss_controls: Option<Cvtsi2ssControlSnapshot>,
    /// Immutable FXSAVE64 CR0 gate inputs, echoed only for S2-39f.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fxsave64_cr0_controls: Option<Fxsave64Cr0Snapshot>,
    /// Immutable FXRSTOR64 CR0 gate inputs, echoed only for S2-39g.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fxrstor64_cr0_controls: Option<Fxrstor64Cr0Snapshot>,
    /// Number of calls made to the deterministic source, only for S2-26e.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rdtsc_source_calls: Option<u64>,
    /// Available(None) means observed no exception; unavailable is not an empty result.
    pub exception: Observation<Option<ExceptionSnapshot>>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RunResult {
    pub schema_version: u32,
    pub case_id: String,
    pub input_sha256: String,
    pub profile: Profile,
    pub backend: Backend,
    pub outcome: Outcome,
    pub instructions_executed: Observation<HexU64>,
    pub snapshot: Observation<Snapshot>,
    pub build: BuildMetadata,
    pub machine: MachineMetadata,
}

impl RunResult {
    pub fn from_json(bytes: &[u8]) -> Result<Self, FormatError> {
        let result: Self = load(bytes, MAX_RESULT_JSON_BYTES)?;
        result.validate()?;
        Ok(result)
    }

    pub fn to_json(&self) -> Result<Vec<u8>, FormatError> {
        self.validate()?;
        save(self, MAX_RESULT_JSON_BYTES)
    }

    pub fn validate(&self) -> Result<(), FormatError> {
        version(self.schema_version)?;
        validate_case_id(&self.case_id)?;
        digest_field(&self.input_sha256)?;
        for value in [
            &self.build.revision,
            &self.build.target,
            &self.build.profile,
            &self.build.compiler,
            &self.machine.host_os,
            &self.machine.host_arch,
        ] {
            text(value, "metadata")?;
        }
        for value in [&self.machine.cpu_model, &self.machine.kernel]
            .into_iter()
            .flatten()
        {
            text(value, "metadata")?;
        }
        if let Outcome::Unsupported { reason }
        | Outcome::TimedOut { reason }
        | Outcome::Signaled { reason, .. }
        | Outcome::HarnessError { reason } = &self.outcome
        {
            text(reason, "outcome.reason")?;
        }
        if let Outcome::Signaled { signal: 0, .. } = self.outcome {
            return Err(invalid("outcome.signal", "must be nonzero"));
        }
        if let Observation::Unavailable { reason } = &self.instructions_executed {
            text(reason, "instructions_executed")?;
        }
        let snapshot = match &self.snapshot {
            Observation::Available { value } => value,
            Observation::Unavailable { reason } => {
                text(reason, "snapshot")?;
                if self.outcome.architectural() {
                    return Err(invalid(
                        "snapshot",
                        "architectural exit requires observed state",
                    ));
                }
                return Ok(());
            }
        };
        match (self.profile, snapshot.xorpd_prerequisites) {
            (Profile::LongXorpdPrerequisitesV1, Some(_)) => {}
            (Profile::LongXorpdPrerequisitesV1, None) => {
                return Err(invalid(
                    "snapshot.xorpd_prerequisites",
                    "required by the XORPD prerequisite profile",
                ));
            }
            (_, Some(_)) => {
                return Err(invalid(
                    "snapshot.xorpd_prerequisites",
                    "only valid for the XORPD prerequisite profile",
                ));
            }
            (_, None) => {}
        }
        match (self.profile, snapshot.fxsave64_cr0_controls) {
            (Profile::LongFxsave64Cr0GatesV1, Some(_)) => {}
            (Profile::LongFxsave64Cr0GatesV1, None) => {
                return Err(invalid(
                    "snapshot.fxsave64_cr0_controls",
                    "required by the FXSAVE64 CR0 gate profile",
                ));
            }
            (_, Some(_)) => {
                return Err(invalid(
                    "snapshot.fxsave64_cr0_controls",
                    "only valid for the FXSAVE64 CR0 gate profile",
                ));
            }
            (_, None) => {}
        }
        match (self.profile, snapshot.fxrstor64_cr0_controls) {
            (Profile::LongFxrstor64Cr0GatesV1, Some(_)) => {}
            (Profile::LongFxrstor64Cr0GatesV1, None) => {
                return Err(invalid(
                    "snapshot.fxrstor64_cr0_controls",
                    "required by the FXRSTOR64 CR0 gate profile",
                ));
            }
            (_, Some(_)) => {
                return Err(invalid(
                    "snapshot.fxrstor64_cr0_controls",
                    "only valid for the FXRSTOR64 CR0 gate profile",
                ));
            }
            (_, None) => {}
        }
        match (self.profile, snapshot.rdtsc_source_calls) {
            (Profile::LongRdtscV1, Some(_)) => {}
            (Profile::LongRdtscV1, None) => {
                return Err(invalid(
                    "snapshot.rdtsc_source_calls",
                    "required by the RDTSC profile",
                ));
            }
            (_, Some(_)) => {
                return Err(invalid(
                    "snapshot.rdtsc_source_calls",
                    "only valid for the RDTSC profile",
                ));
            }
            (_, None) => {}
        }
        if matches!(
            self.profile,
            Profile::LongMinssInvalidControlsV1 | Profile::LongMinssUnmaskedDenormalV1
        ) {
            if snapshot.minss_invalid_controls.is_none() {
                return Err(invalid(
                    "snapshot.minss_invalid_controls",
                    "required by the MINSS invalid-controls profile",
                ));
            }
        } else if snapshot.minss_invalid_controls.is_some() {
            return Err(invalid(
                "snapshot.minss_invalid_controls",
                "only valid for the MINSS invalid-controls profile",
            ));
        }
        if self.profile == Profile::LongMinsdUnmaskedDenormalV1 {
            if snapshot.minsd_unmasked_controls.is_none() {
                return Err(invalid(
                    "snapshot.minsd_unmasked_controls",
                    "required by the MINSD profile",
                ));
            }
        } else if snapshot.minsd_unmasked_controls.is_some() {
            return Err(invalid(
                "snapshot.minsd_unmasked_controls",
                "only valid for the MINSD profile",
            ));
        }
        if self.profile == Profile::LongMaxsdUnmaskedDenormalV1 {
            if snapshot.maxsd_unmasked_controls.is_none() {
                return Err(invalid(
                    "snapshot.maxsd_unmasked_controls",
                    "required by the MAXSD profile",
                ));
            }
        } else if snapshot.maxsd_unmasked_controls.is_some() {
            return Err(invalid(
                "snapshot.maxsd_unmasked_controls",
                "only valid for the MAXSD profile",
            ));
        }
        if self.profile == Profile::LongMinpdUnmaskedDenormalV1 {
            if snapshot.minpd_unmasked_controls.is_none() {
                return Err(invalid(
                    "snapshot.minpd_unmasked_controls",
                    "required by the MINPD profile",
                ));
            }
        } else if snapshot.minpd_unmasked_controls.is_some() {
            return Err(invalid(
                "snapshot.minpd_unmasked_controls",
                "only valid for the MINPD profile",
            ));
        }
        if self.profile == Profile::LongMinpsUnmaskedDenormalV1 {
            if snapshot.minps_unmasked_controls.is_none() {
                return Err(invalid(
                    "snapshot.minps_unmasked_controls",
                    "required by the MINPS profile",
                ));
            }
        } else if snapshot.minps_unmasked_controls.is_some() {
            return Err(invalid(
                "snapshot.minps_unmasked_controls",
                "only valid for the MINPS profile",
            ));
        }
        if self.profile == Profile::LongMaxpsUnmaskedDenormalV1 {
            if snapshot.maxps_unmasked_controls.is_none() {
                return Err(invalid(
                    "snapshot.maxps_unmasked_controls",
                    "required by the MAXPS profile",
                ));
            }
        } else if snapshot.maxps_unmasked_controls.is_some() {
            return Err(invalid(
                "snapshot.maxps_unmasked_controls",
                "only valid for the MAXPS profile",
            ));
        }
        if self.profile == Profile::LongMaxpdUnmaskedDenormalV1 {
            if snapshot.maxpd_unmasked_controls.is_none() {
                return Err(invalid(
                    "snapshot.maxpd_unmasked_controls",
                    "required by the MAXPD profile",
                ));
            }
        } else if snapshot.maxpd_unmasked_controls.is_some() {
            return Err(invalid(
                "snapshot.maxpd_unmasked_controls",
                "only valid for the MAXPD profile",
            ));
        }
        if self.profile == Profile::LongCvtdq2psRegistersV1 {
            if snapshot.cvtdq2ps_controls.is_none() {
                return Err(invalid(
                    "snapshot.cvtdq2ps_controls",
                    "required by the CVTDQ2PS profile",
                ));
            }
        } else if snapshot.cvtdq2ps_controls.is_some() {
            return Err(invalid(
                "snapshot.cvtdq2ps_controls",
                "only valid for CVTDQ2PS profile",
            ));
        }
        if self.profile == Profile::LongCvtss2sdRegistersV1 {
            if snapshot.cvtss2sd_controls.is_none() {
                return Err(invalid(
                    "snapshot.cvtss2sd_controls",
                    "required by the CVTSS2SD profile",
                ));
            }
        } else if snapshot.cvtss2sd_controls.is_some() {
            return Err(invalid(
                "snapshot.cvtss2sd_controls",
                "only valid for CVTSS2SD profile",
            ));
        }
        if self.profile == Profile::LongCvtps2pdRegistersV1 {
            if snapshot.cvtps2pd_controls.is_none() {
                return Err(invalid(
                    "snapshot.cvtps2pd_controls",
                    "required by the CVTPS2PD profile",
                ));
            }
        } else if snapshot.cvtps2pd_controls.is_some() {
            return Err(invalid(
                "snapshot.cvtps2pd_controls",
                "only valid for CVTPS2PD profile",
            ));
        }
        if self.profile == Profile::LongComissUcomissRegistersV1 {
            if snapshot.comiss_ucomiss_controls.is_none() {
                return Err(invalid(
                    "snapshot.comiss_ucomiss_controls",
                    "required by the COMISS/UCOMISS profile",
                ));
            }
        } else if snapshot.comiss_ucomiss_controls.is_some() {
            return Err(invalid(
                "snapshot.comiss_ucomiss_controls",
                "only valid for COMISS/UCOMISS profile",
            ));
        }
        if self.profile == Profile::LongComisdUcomisdRegistersV1 {
            if snapshot.comisd_ucomisd_controls.is_none() {
                return Err(invalid(
                    "snapshot.comisd_ucomisd_controls",
                    "required by COMISD/UCOMISD profile",
                ));
            }
        } else if snapshot.comisd_ucomisd_controls.is_some() {
            return Err(invalid(
                "snapshot.comisd_ucomisd_controls",
                "only valid for COMISD/UCOMISD profile",
            ));
        }
        if self.profile == Profile::LongCvtdq2psRegistersV1 {
            if snapshot.cvtdq2ps_controls.is_none() {
                return Err(invalid(
                    "snapshot.cvtdq2ps_controls",
                    "required by the CVTDQ2PS profile",
                ));
            }
        } else if snapshot.cvtdq2ps_controls.is_some() {
            return Err(invalid(
                "snapshot.cvtdq2ps_controls",
                "only valid for CVTDQ2PS profile",
            ));
        }
        if matches!(
            self.profile,
            Profile::LongCvtdq2pdRegExactV1 | Profile::LongCvtdq2pdRegistersV1
        ) {
            if snapshot.cvtdq2pd_controls.is_none() {
                return Err(invalid(
                    "snapshot.cvtdq2pd_controls",
                    "required by the CVTDQ2PD profile",
                ));
            }
        } else if snapshot.cvtdq2pd_controls.is_some() {
            return Err(invalid(
                "snapshot.cvtdq2pd_controls",
                "only valid for the CVTDQ2PD profile",
            ));
        }
        if matches!(
            self.profile,
            Profile::LongCvtsi2sdR32ExactV1 | Profile::LongCvtsi2sdRegistersV1
        ) {
            if snapshot.cvtsi2sd_r32_controls.is_none() {
                return Err(invalid(
                    "snapshot.cvtsi2sd_r32_controls",
                    "required by the CVTSI2SD r32 profile",
                ));
            }
        } else if snapshot.cvtsi2sd_r32_controls.is_some()
            && self.profile != Profile::LongCvtsi2sdRegistersV1
        {
            return Err(invalid(
                "snapshot.cvtsi2sd_r32_controls",
                "only valid for the CVTSI2SD r32 profile",
            ));
        }
        if matches!(
            self.profile,
            Profile::LongCvtsi2sdR64ExactV1 | Profile::LongCvtsi2sdRegistersV1
        ) {
            if snapshot.cvtsi2sd_r64_controls.is_none() {
                return Err(invalid(
                    "snapshot.cvtsi2sd_r64_controls",
                    "required by the CVTSI2SD r64 profile",
                ));
            }
        } else if snapshot.cvtsi2sd_r64_controls.is_some()
            && self.profile != Profile::LongCvtsi2sdRegistersV1
        {
            return Err(invalid(
                "snapshot.cvtsi2sd_r64_controls",
                "only valid for the CVTSI2SD r64 profile",
            ));
        }
        if self.profile == Profile::LongCvtsi2ssRegistersV1 {
            if snapshot.cvtsi2ss_controls.is_none() {
                return Err(invalid(
                    "snapshot.cvtsi2ss_controls",
                    "required by CVTSI2SS profile",
                ));
            }
        } else if snapshot.cvtsi2ss_controls.is_some() {
            return Err(invalid(
                "snapshot.cvtsi2ss_controls",
                "only valid for CVTSI2SS profile",
            ));
        }
        if self.profile == Profile::LongMaxssUnmaskedDenormalV1 {
            if snapshot.maxss_unmasked_controls.is_none() {
                return Err(invalid(
                    "snapshot.maxss_unmasked_controls",
                    "required by the MAXSS profile",
                ));
            }
        } else if snapshot.maxss_unmasked_controls.is_some() {
            return Err(invalid(
                "snapshot.maxss_unmasked_controls",
                "only valid for the MAXSS profile",
            ));
        }
        snapshot.state.validate_for(self.profile)?;
        if snapshot.rflags_defined.get() & !INTEGER_FLAGS != 0 {
            return Err(invalid("snapshot.rflags_defined", "outside profile"));
        }
        if snapshot.memory.len() > crate::MAX_MEMORY_REGIONS {
            return Err(invalid("snapshot.memory", "too many regions"));
        }
        let mut total = 0usize;
        let mut ids = std::collections::HashSet::new();
        let mut ranges = Vec::new();
        for region in &snapshot.memory {
            validate_case_id(&region.region_id)?;
            if !ids.insert(&region.region_id) {
                return Err(invalid("snapshot.memory", "duplicate region"));
            }
            validate_hex_bytes(&region.bytes_hex, "snapshot.memory.bytes_hex")?;
            if region.bytes_hex.is_empty() {
                return Err(invalid("snapshot.memory", "empty region"));
            }
            let start = region.guest_address.get();
            let end = start
                .checked_add((region.bytes_hex.len() / 2) as u64)
                .ok_or_else(|| invalid("snapshot.memory", "address overflow"))?;
            if ranges.iter().any(|(lo, hi)| start < *hi && *lo < end) {
                return Err(invalid("snapshot.memory", "overlapping regions"));
            }
            ranges.push((start, end));
            total = total.saturating_add(region.bytes_hex.len() / 2);
            if total > crate::MAX_MEMORY_BYTES
                || region
                    .guest_address
                    .get()
                    .checked_add((region.bytes_hex.len() / 2) as u64)
                    .is_none()
            {
                return Err(invalid("snapshot.memory", "size limit or address overflow"));
            }
        }
        match &snapshot.exception {
            Observation::Available { value } => {
                if value.is_some() != matches!(self.outcome, Outcome::Exception {}) {
                    return Err(invalid("snapshot.exception", "conflicts with outcome"));
                }
                if let Some(fault) = value {
                    if fault.vector > 31 || fault.faulting_rip != snapshot.state.rip {
                        return Err(invalid("snapshot.exception", "invalid vector or fault RIP"));
                    }
                    for field in [&fault.error_code, &fault.fault_address] {
                        if let Observation::Unavailable { reason } = field {
                            text(reason, "snapshot.exception")?;
                        }
                    }
                    if matches!(fault.vector, 8 | 10 | 11 | 12 | 13 | 14 | 17 | 21 | 29 | 30)
                        && !fault.error_code.is_available()
                    {
                        return Err(invalid(
                            "snapshot.exception.error_code",
                            "architectural error code is unavailable",
                        ));
                    }
                    if fault.vector == 14 && !fault.fault_address.is_available() {
                        return Err(invalid(
                            "snapshot.exception.fault_address",
                            "page-fault address is unavailable",
                        ));
                    }
                }
            }
            Observation::Unavailable { reason } => {
                text(reason, "snapshot.exception")?;
                if self.outcome.architectural() {
                    return Err(invalid(
                        "snapshot.exception",
                        "architectural exit requires exception observation",
                    ));
                }
            }
        }
        Ok(())
    }

    pub fn validate_for(&self, loaded: &LoadedFixture) -> Result<(), FormatError> {
        loaded.fixture.validate()?;
        self.validate()?;
        if self.input_sha256 != loaded.input_sha256 {
            return Err(FormatError::CaseMismatch("input_sha256"));
        }
        if self.case_id != loaded.fixture.case_id {
            return Err(FormatError::CaseMismatch("case_id"));
        }
        if self.profile != loaded.fixture.profile {
            return Err(FormatError::CaseMismatch("profile"));
        }
        if let Observation::Available { value } = &self.snapshot {
            match (
                loaded.fixture.profile,
                loaded.fixture.maxss_unmasked_controls,
                value.maxss_unmasked_controls,
            ) {
                (Profile::LongMaxssUnmaskedDenormalV1, Some(input), Some(output))
                    if input == output => {}
                (Profile::LongMaxssUnmaskedDenormalV1, _, _) => {
                    return Err(FormatError::CaseMismatch("maxss_unmasked_controls"));
                }
                (_, None, None) => {}
                _ => return Err(FormatError::CaseMismatch("maxss_unmasked_controls")),
            }
            match (
                loaded.fixture.profile,
                loaded.fixture.minsd_unmasked_controls,
                value.minsd_unmasked_controls,
            ) {
                (Profile::LongMinsdUnmaskedDenormalV1, Some(input), Some(output))
                    if input == output => {}
                (Profile::LongMinsdUnmaskedDenormalV1, _, _) => {
                    return Err(FormatError::CaseMismatch("minsd_unmasked_controls"));
                }
                (_, None, None) => {}
                _ => return Err(FormatError::CaseMismatch("minsd_unmasked_controls")),
            }
            if loaded.fixture.profile == Profile::LongMaxssUnmaskedDenormalV1 {
                validate_maxss_unmasked_result(loaded, self, value)?;
            }
            if loaded.fixture.profile == Profile::LongMinsdUnmaskedDenormalV1 {
                validate_minsd_unmasked_result(loaded, self, value)?;
            }
            match (
                loaded.fixture.profile,
                loaded.fixture.maxsd_unmasked_controls,
                value.maxsd_unmasked_controls,
            ) {
                (Profile::LongMaxsdUnmaskedDenormalV1, Some(input), Some(output))
                    if input == output => {}
                (Profile::LongMaxsdUnmaskedDenormalV1, _, _) => {
                    return Err(FormatError::CaseMismatch("maxsd_unmasked_controls"));
                }
                (_, None, None) => {}
                _ => return Err(FormatError::CaseMismatch("maxsd_unmasked_controls")),
            }
            if loaded.fixture.profile == Profile::LongMaxsdUnmaskedDenormalV1 {
                validate_maxsd_unmasked_result(loaded, self, value)?;
            }
            match (
                loaded.fixture.profile,
                loaded.fixture.minpd_unmasked_controls,
                value.minpd_unmasked_controls,
            ) {
                (Profile::LongMinpdUnmaskedDenormalV1, Some(input), Some(output))
                    if input == output => {}
                (Profile::LongMinpdUnmaskedDenormalV1, _, _) => {
                    return Err(FormatError::CaseMismatch("minpd_unmasked_controls"));
                }
                (_, None, None) => {}
                _ => return Err(FormatError::CaseMismatch("minpd_unmasked_controls")),
            }
            if loaded.fixture.profile == Profile::LongMinpdUnmaskedDenormalV1 {
                validate_minpd_unmasked_result(loaded, self, value)?;
            }
            match (
                loaded.fixture.profile,
                loaded.fixture.minps_unmasked_controls,
                value.minps_unmasked_controls,
            ) {
                (Profile::LongMinpsUnmaskedDenormalV1, Some(input), Some(output))
                    if input == output => {}
                (Profile::LongMinpsUnmaskedDenormalV1, _, _) => {
                    return Err(FormatError::CaseMismatch("minps_unmasked_controls"));
                }
                (_, None, None) => {}
                _ => return Err(FormatError::CaseMismatch("minps_unmasked_controls")),
            }
            if loaded.fixture.profile == Profile::LongMinpsUnmaskedDenormalV1 {
                validate_minps_unmasked_result(loaded, self, value)?;
            }
            match (
                loaded.fixture.profile,
                loaded.fixture.maxps_unmasked_controls,
                value.maxps_unmasked_controls,
            ) {
                (Profile::LongMaxpsUnmaskedDenormalV1, Some(input), Some(output))
                    if input == output => {}
                (Profile::LongMaxpsUnmaskedDenormalV1, _, _) => {
                    return Err(FormatError::CaseMismatch("maxps_unmasked_controls"));
                }
                (_, None, None) => {}
                _ => return Err(FormatError::CaseMismatch("maxps_unmasked_controls")),
            }
            if loaded.fixture.profile == Profile::LongMaxpsUnmaskedDenormalV1 {
                validate_maxps_unmasked_result(loaded, self, value)?;
            }
            match (
                loaded.fixture.profile,
                loaded.fixture.maxpd_unmasked_controls,
                value.maxpd_unmasked_controls,
            ) {
                (Profile::LongMaxpdUnmaskedDenormalV1, Some(input), Some(output))
                    if input == output => {}
                (Profile::LongMaxpdUnmaskedDenormalV1, _, _) => {
                    return Err(FormatError::CaseMismatch("maxpd_unmasked_controls"));
                }
                (_, None, None) => {}
                _ => return Err(FormatError::CaseMismatch("maxpd_unmasked_controls")),
            }
            if loaded.fixture.profile == Profile::LongMaxpdUnmaskedDenormalV1 {
                validate_maxpd_unmasked_result(loaded, self, value)?;
            }
            match (
                loaded.fixture.profile,
                loaded.fixture.cvtdq2ps_controls,
                value.cvtdq2ps_controls,
            ) {
                (Profile::LongCvtdq2psRegistersV1, Some(input), Some(output))
                    if input == output => {}
                (Profile::LongCvtdq2psRegistersV1, _, _) => {
                    return Err(FormatError::CaseMismatch("cvtdq2ps_controls"));
                }
                (_, None, None) => {}
                _ => return Err(FormatError::CaseMismatch("cvtdq2ps_controls")),
            }
            if loaded.fixture.profile == Profile::LongCvtdq2psRegistersV1 {
                validate_cvtdq2ps_registers_result(loaded, self, value)?;
            }
            match (
                loaded.fixture.profile,
                loaded.fixture.cvtss2sd_controls,
                value.cvtss2sd_controls,
            ) {
                (Profile::LongCvtss2sdRegistersV1, Some(input), Some(output))
                    if input == output => {}
                (Profile::LongCvtss2sdRegistersV1, _, _) => {
                    return Err(FormatError::CaseMismatch("cvtss2sd_controls"));
                }
                (_, None, None) => {}
                _ => return Err(FormatError::CaseMismatch("cvtss2sd_controls")),
            }
            if loaded.fixture.profile == Profile::LongCvtss2sdRegistersV1 {
                validate_cvtss2sd_registers_result(loaded, self, value)?;
            }
            match (
                loaded.fixture.profile,
                loaded.fixture.cvtps2pd_controls,
                value.cvtps2pd_controls,
            ) {
                (Profile::LongCvtps2pdRegistersV1, Some(input), Some(output))
                    if input == output => {}
                (Profile::LongCvtps2pdRegistersV1, _, _) => {
                    return Err(FormatError::CaseMismatch("cvtps2pd_controls"));
                }
                (_, None, None) => {}
                _ => return Err(FormatError::CaseMismatch("cvtps2pd_controls")),
            }
            if loaded.fixture.profile == Profile::LongCvtps2pdRegistersV1 {
                validate_cvtps2pd_registers_result(loaded, self, value)?;
            }
            match (
                loaded.fixture.profile,
                loaded.fixture.comiss_ucomiss_controls,
                value.comiss_ucomiss_controls,
            ) {
                (Profile::LongComissUcomissRegistersV1, Some(input), Some(output))
                    if input == output => {}
                (Profile::LongComissUcomissRegistersV1, _, _) => {
                    return Err(FormatError::CaseMismatch("comiss_ucomiss_controls"));
                }
                (_, None, None) => {}
                _ => return Err(FormatError::CaseMismatch("comiss_ucomiss_controls")),
            }
            if loaded.fixture.profile == Profile::LongComissUcomissRegistersV1 {
                validate_comiss_ucomiss_registers_result(loaded, self, value)?;
            }
            match (
                loaded.fixture.profile,
                loaded.fixture.comisd_ucomisd_controls,
                value.comisd_ucomisd_controls,
            ) {
                (Profile::LongComisdUcomisdRegistersV1, Some(input), Some(output))
                    if input == output => {}
                (Profile::LongComisdUcomisdRegistersV1, _, _) => {
                    return Err(FormatError::CaseMismatch("comisd_ucomisd_controls"));
                }
                (_, None, None) => {}
                _ => return Err(FormatError::CaseMismatch("comisd_ucomisd_controls")),
            }
            if loaded.fixture.profile == Profile::LongComisdUcomisdRegistersV1 {
                validate_comisd_ucomisd_registers_result(loaded, self, value)?;
            }
            match (
                loaded.fixture.profile,
                loaded.fixture.cvtdq2pd_controls,
                value.cvtdq2pd_controls,
            ) {
                (
                    Profile::LongCvtdq2pdRegExactV1 | Profile::LongCvtdq2pdRegistersV1,
                    Some(input),
                    Some(output),
                ) if input == output => {}
                (Profile::LongCvtdq2pdRegExactV1 | Profile::LongCvtdq2pdRegistersV1, _, _) => {
                    return Err(FormatError::CaseMismatch("cvtdq2pd_controls"));
                }
                (_, None, None) => {}
                _ => return Err(FormatError::CaseMismatch("cvtdq2pd_controls")),
            }
            if matches!(
                loaded.fixture.profile,
                Profile::LongCvtdq2pdRegExactV1 | Profile::LongCvtdq2pdRegistersV1
            ) {
                validate_cvtdq2pd_result(loaded, self, value)?;
            }
            match (
                loaded.fixture.profile,
                loaded.fixture.cvtsi2sd_r32_controls,
                value.cvtsi2sd_r32_controls,
            ) {
                (
                    Profile::LongCvtsi2sdR32ExactV1 | Profile::LongCvtsi2sdRegistersV1,
                    Some(input),
                    Some(output),
                ) if input == output => {}
                (Profile::LongCvtsi2sdR32ExactV1 | Profile::LongCvtsi2sdRegistersV1, _, _) => {
                    return Err(FormatError::CaseMismatch("cvtsi2sd_r32_controls"));
                }
                (_, None, None) => {}
                _ => return Err(FormatError::CaseMismatch("cvtsi2sd_r32_controls")),
            }
            if loaded.fixture.profile == Profile::LongCvtsi2sdR32ExactV1 {
                validate_cvtsi2sd_r32_result(loaded, self, value)?;
            } else if loaded.fixture.profile == Profile::LongCvtsi2sdRegistersV1 {
                validate_cvtsi2sd_registers_result(loaded, self, value)?;
            }
            match (
                loaded.fixture.profile,
                loaded.fixture.cvtsi2ss_controls,
                value.cvtsi2ss_controls,
            ) {
                (Profile::LongCvtsi2ssRegistersV1, Some(input), Some(output))
                    if input == output => {}
                (Profile::LongCvtsi2ssRegistersV1, _, _) => {
                    return Err(FormatError::CaseMismatch("cvtsi2ss_controls"));
                }
                (_, None, None) => {}
                _ => return Err(FormatError::CaseMismatch("cvtsi2ss_controls")),
            }
            if loaded.fixture.profile == Profile::LongCvtsi2ssRegistersV1 {
                validate_cvtsi2ss_registers_result(loaded, self, value)?;
            }
            match (
                loaded.fixture.profile,
                loaded.fixture.cvtsi2sd_r64_controls,
                value.cvtsi2sd_r64_controls,
            ) {
                (
                    Profile::LongCvtsi2sdR64ExactV1 | Profile::LongCvtsi2sdRegistersV1,
                    Some(input),
                    Some(output),
                ) if input == output => {}
                (Profile::LongCvtsi2sdR64ExactV1 | Profile::LongCvtsi2sdRegistersV1, _, _) => {
                    return Err(FormatError::CaseMismatch("cvtsi2sd_r64_controls"));
                }
                (_, None, None) => {}
                _ => return Err(FormatError::CaseMismatch("cvtsi2sd_r64_controls")),
            }
            if loaded.fixture.profile == Profile::LongCvtsi2sdR64ExactV1 {
                validate_cvtsi2sd_r64_result(loaded, self, value)?;
            }
            if loaded.fixture.profile == Profile::LongEferRdmsrV1 {
                let initial = &loaded.fixture.initial_state;
                let efer = loaded
                    .fixture
                    .efer_value
                    .ok_or(FormatError::CaseMismatch("efer_value"))?;
                let cpl = loaded
                    .fixture
                    .efer_cpl
                    .ok_or(FormatError::CaseMismatch("efer_cpl"))?;
                let success = cpl == CliCpl::Cpl0;
                let mut expected_gpr = initial.gpr;
                if success {
                    expected_gpr[0] = HexU64(u64::from(efer.get() as u32));
                    expected_gpr[2] = HexU64(u64::from((efer.get() >> 32) as u32));
                }
                let expected_rip = HexU64(initial.rip.get() + if success { 2 } else { 0 });
                let expected_exit = if success {
                    Outcome::StopAddress {}
                } else {
                    Outcome::Exception {}
                };
                let exception_matches = if success {
                    value.exception == (Observation::Available { value: None })
                } else {
                    matches!(
                        &value.exception,
                        Observation::Available { value: Some(exception) }
                            if exception.vector == 13
                                && exception.error_code == (Observation::Available { value: HexU64(0) })
                                && exception.faulting_rip == initial.rip
                                && matches!(exception.fault_address, Observation::Unavailable { .. })
                    )
                };
                if value.state.gpr != expected_gpr
                    || value.state.rip != expected_rip
                    || value.state.rflags != initial.rflags
                    || value.state.cr0 != initial.cr0
                    || value.state.cr4 != initial.cr4
                    || value.state.sti_shadow != initial.sti_shadow
                    || value.rflags_defined != HexU64(INTEGER_FLAGS)
                    || self.instructions_executed
                        != (Observation::Available {
                            value: HexU64(u64::from(success)),
                        })
                    || self.outcome != expected_exit
                    || !exception_matches
                    || value.memory.len() != loaded.fixture.memory.len()
                    || loaded.fixture.memory.iter().any(|input| {
                        value
                            .memory
                            .iter()
                            .find(|output| output.region_id == input.region_id)
                            .is_none_or(|output| output.bytes_hex != input.initial_bytes_hex)
                    })
                {
                    return Err(FormatError::CaseMismatch("state.efer_rdmsr"));
                }
            }
            if loaded.fixture.profile == Profile::LongRdtscV1
                && value.rdtsc_source_calls
                    != loaded
                        .fixture
                        .rdtsc_controls
                        .map(|controls| controls.expected_source_calls)
            {
                return Err(FormatError::CaseMismatch("snapshot.rdtsc_source_calls"));
            }
            // A terminal outcome describes why execution stopped, not whether
            // earlier instructions committed. MOV FS may retire before a later
            // exception, unsupported instruction, or harness/backend error.
            // The observed retirement count is the evidence that its state
            // transition may have occurred.
            let fs_load_retired = loaded.fixture.profile == Profile::LongFsLoadV1
                && matches!(
                    self.instructions_executed,
                    Observation::Available { value } if value.get() > 0
                );
            let gs_load_retired = loaded.fixture.profile == Profile::LongGsLoadV1
                && matches!(
                    self.instructions_executed,
                    Observation::Available { value } if value.get() > 0
                );
            let fs_base_msr_committed = loaded.fixture.profile == Profile::LongFsBaseMsrV1
                && matches!(
                    self.instructions_executed,
                    Observation::Available { value } if value.get() > 0
                );
            let retired = match self.instructions_executed {
                Observation::Available { value } => value.get(),
                Observation::Unavailable { .. } => 0,
            };
            let kernel_gs_code = loaded
                .fixture
                .memory
                .iter()
                .find(|region| region.region_id == "code")
                .map(|region| region.initial_bytes_hex.to_ascii_lowercase());
            let kernel_gs_write_retired = loaded.fixture.profile == Profile::LongKernelGsBaseV1
                && retired > 0
                && kernel_gs_code
                    .as_deref()
                    .is_some_and(|code| code.starts_with("0f30"));
            let swapgs_retired = (loaded.fixture.profile == Profile::LongSwapgsV1 && retired > 0)
                || (loaded.fixture.profile == Profile::LongKernelGsBaseV1
                    && retired >= 2
                    && kernel_gs_code.as_deref().is_some_and(|code| {
                        matches!(code, "0f300f01f80f32" | "0f300f01f865488b00")
                    }));
            if !fs_load_retired
                && !fs_base_msr_committed
                && value.state.fs_base != loaded.fixture.initial_state.fs_base
            {
                return Err(FormatError::CaseMismatch("state.fs_base"));
            }
            if !gs_load_retired
                && !swapgs_retired
                && value.state.gs_base != loaded.fixture.initial_state.gs_base
            {
                return Err(FormatError::CaseMismatch("state.gs_base"));
            }
            if !swapgs_retired
                && !kernel_gs_write_retired
                && value.state.kernel_gs_base != loaded.fixture.initial_state.kernel_gs_base
            {
                return Err(FormatError::CaseMismatch("state.kernel_gs_base"));
            }
            if !fs_load_retired
                && value.state.fs_selector != loaded.fixture.initial_state.fs_selector
            {
                return Err(FormatError::CaseMismatch("state.fs_selector"));
            }
            if !gs_load_retired
                && value.state.gs_selector != loaded.fixture.initial_state.gs_selector
            {
                return Err(FormatError::CaseMismatch("state.gs_selector"));
            }
            if value.state.gdtr_base != loaded.fixture.initial_state.gdtr_base
                || value.state.gdtr_limit != loaded.fixture.initial_state.gdtr_limit
            {
                return Err(FormatError::CaseMismatch("state.gdtr"));
            }
            if !fs_load_retired
                && (value.state.fs_cache_limit != loaded.fixture.initial_state.fs_cache_limit
                    || value.state.fs_cache_access != loaded.fixture.initial_state.fs_cache_access
                    || value.state.fs_cache_flags != loaded.fixture.initial_state.fs_cache_flags)
            {
                return Err(FormatError::CaseMismatch("state.fs_cache"));
            }
            if !gs_load_retired
                && (value.state.gs_cache_limit != loaded.fixture.initial_state.gs_cache_limit
                    || value.state.gs_cache_access != loaded.fixture.initial_state.gs_cache_access
                    || value.state.gs_cache_flags != loaded.fixture.initial_state.gs_cache_flags)
            {
                return Err(FormatError::CaseMismatch("state.gs_cache"));
            }
            if loaded.fixture.profile == Profile::LongNullDsEsV1 {
                let code = loaded
                    .fixture
                    .memory
                    .iter()
                    .find(|region| region.region_id == "code")
                    .ok_or(FormatError::CaseMismatch("memory.code"))?;
                let code = decode_bytes(&code.initial_bytes_hex)
                    .map_err(|_| FormatError::CaseMismatch("memory.code"))?;
                let retired = match self.instructions_executed {
                    Observation::Available { value } => value.get() as usize,
                    _ => 0,
                };
                let mut ds = (
                    loaded.fixture.initial_state.ds_selector.unwrap(),
                    loaded.fixture.initial_state.ds_unusable.unwrap(),
                );
                let mut es = (
                    loaded.fixture.initial_state.es_selector.unwrap(),
                    loaded.fixture.initial_state.es_unusable.unwrap(),
                );
                for instruction in code.chunks_exact(2).take(retired) {
                    match instruction {
                        [0x8e, 0xd8] => ds = (HexU64(0), true),
                        [0x8e, 0xc0] => es = (HexU64(0), true),
                        _ => break,
                    }
                }
                if value.state.ds_selector != Some(ds.0)
                    || value.state.ds_unusable != Some(ds.1)
                    || value.state.es_selector != Some(es.0)
                    || value.state.es_unusable != Some(es.1)
                {
                    return Err(FormatError::CaseMismatch("state.ds_es"));
                }
            }
            if loaded.fixture.profile == Profile::LongIretqCpl0ValidV1 {
                let frame = loaded
                    .fixture
                    .memory
                    .iter()
                    .find(|r| r.region_id == "frame")
                    .unwrap();
                let frame_bytes = decode_bytes(&frame.initial_bytes_hex).unwrap();
                let saved_rip = HexU64(u64::from_le_bytes(frame_bytes[0..8].try_into().unwrap()));
                let saved_cs = HexU64(u64::from(u64::from_le_bytes(
                    frame_bytes[8..16].try_into().unwrap(),
                ) as u16));
                let saved_flags =
                    HexU64(u64::from_le_bytes(frame_bytes[16..24].try_into().unwrap()));
                let saved_rsp = HexU64(u64::from_le_bytes(frame_bytes[24..32].try_into().unwrap()));
                let saved_ss = HexU64(u64::from(u64::from_le_bytes(
                    frame_bytes[32..40].try_into().unwrap(),
                ) as u16));
                let expected = [
                    ("state.rip", value.state.rip, saved_rip),
                    ("state.rflags", value.state.rflags, saved_flags),
                    (
                        "state.cs_selector",
                        value.state.cs_selector.unwrap_or(HexU64(u64::MAX)),
                        saved_cs,
                    ),
                    (
                        "state.ss_selector",
                        value.state.ss_selector.unwrap_or(HexU64(u64::MAX)),
                        saved_ss,
                    ),
                ];
                if expected.iter().any(|(_, actual, wanted)| actual != wanted)
                    || (0..16).any(|i| {
                        i != 4 && value.state.gpr[i] != loaded.fixture.initial_state.gpr[i]
                    })
                    || value.state.gpr[4] != saved_rsp
                    || value.state.cs_cache_base != Some(HexU64(0))
                    || value.state.cs_cache_limit != Some(HexU64(u32::MAX as u64))
                    || value.state.cs_cache_access != Some(HexU64(0x9b))
                    || value.state.cs_cache_flags != Some(HexU64(0xa))
                    || value.state.ss_cache_base != Some(HexU64(0))
                    || value.state.ss_cache_limit != Some(HexU64(u32::MAX as u64))
                    || value.state.ss_cache_access != Some(HexU64(0x93))
                    || value.state.ss_cache_flags != Some(HexU64(0xc))
                    || value.state.activity != Some(CpuActivitySnapshot::Running)
                    || value.state.sti_shadow != Some(StiShadowSnapshot::Clear)
                    || value.rflags_defined != HexU64(INTEGER_FLAGS)
                    || retired != 1
                    || self.outcome != (Outcome::StopAddress {})
                {
                    return Err(FormatError::CaseMismatch("state.iretq"));
                }
                for id in ["frame", "gdt"] {
                    let input = loaded
                        .fixture
                        .memory
                        .iter()
                        .find(|r| r.region_id == id)
                        .unwrap();
                    let output = value
                        .memory
                        .iter()
                        .find(|r| r.region_id == id)
                        .ok_or(FormatError::CaseMismatch("memory.region_id"))?;
                    if output.bytes_hex != input.initial_bytes_hex {
                        return Err(FormatError::CaseMismatch("memory.iretq_preserved"));
                    }
                }
            }
            if loaded.fixture.profile == Profile::LongLretqCpl0ValidV1 {
                let frame = loaded
                    .fixture
                    .memory
                    .iter()
                    .find(|r| r.region_id == "frame")
                    .unwrap();
                let bytes = decode_bytes(&frame.initial_bytes_hex).unwrap();
                let target = HexU64(u64::from_le_bytes(bytes[0..8].try_into().unwrap()));
                let selector = HexU64(u64::from(
                    u64::from_le_bytes(bytes[8..16].try_into().unwrap()) as u16,
                ));
                let initial = &loaded.fixture.initial_state;
                if value.state.rip != target
                    || value.state.rflags != initial.rflags
                    || value.state.gpr[4] != HexU64(initial.gpr[4].get() + 16)
                    || value.state.cs_selector != Some(selector)
                    || value.state.cs_cache_base != Some(HexU64(0))
                    || value.state.cs_cache_limit != Some(HexU64(u32::MAX as u64))
                    || value.state.cs_cache_access != Some(HexU64(0x9b))
                    || value.state.cs_cache_flags != Some(HexU64(0xa))
                    || value.state.ss_selector != initial.ss_selector
                    || value.state.ss_cache_base != initial.ss_cache_base
                    || value.state.ss_cache_limit != initial.ss_cache_limit
                    || value.state.ss_cache_access != initial.ss_cache_access
                    || value.state.ss_cache_flags != initial.ss_cache_flags
                    || (0..16).any(|i| i != 4 && value.state.gpr[i] != initial.gpr[i])
                    || value.state.activity != Some(CpuActivitySnapshot::Running)
                    || value.state.sti_shadow != Some(StiShadowSnapshot::Clear)
                    || value.rflags_defined != HexU64(INTEGER_FLAGS)
                    || self.instructions_executed != (Observation::Available { value: HexU64(1) })
                    || self.outcome != (Outcome::StopAddress {})
                {
                    return Err(FormatError::CaseMismatch("state.lretq"));
                }
                for input in &loaded.fixture.memory {
                    let output = value
                        .memory
                        .iter()
                        .find(|r| r.region_id == input.region_id)
                        .ok_or(FormatError::CaseMismatch("memory.region_id"))?;
                    if output.bytes_hex != input.initial_bytes_hex {
                        return Err(FormatError::CaseMismatch("memory.lretq_preserved"));
                    }
                }
            }
            if loaded.fixture.profile == Profile::LongCr0FpInitV1 {
                let initial = &loaded.fixture.initial_state;
                let code = loaded
                    .fixture
                    .memory
                    .iter()
                    .find(|region| region.region_id == "code")
                    .ok_or(FormatError::CaseMismatch("memory.code"))?;
                let bytes = decode_bytes(&code.initial_bytes_hex)
                    .map_err(|_| FormatError::CaseMismatch("memory.code"))?;
                let cpl = loaded
                    .fixture
                    .cli_cpl
                    .ok_or(FormatError::CaseMismatch("cli_cpl"))?;
                let initial_cr0 = initial.cr0.ok_or(FormatError::CaseMismatch("state.cr0"))?;
                let target_cr0 = initial.gpr[0];
                let base = od_isa::execute::Cr0Profile::BASE;
                let allowed = [base, base | 4, base | 8, base | 12];
                let (operation, expected_cr0, expected_rax, retired, fninit_retired) =
                    match bytes.as_slice() {
                        [0x0f, 0x20, 0xc0] if cpl == CliCpl::Cpl0 => {
                            ("read", initial_cr0, initial_cr0, 1, false)
                        }
                        [0x0f, 0x20, 0xc0] => ("read_fault", initial_cr0, initial.gpr[0], 0, false),
                        [0x0f, 0x22, 0xc0] if cpl != CliCpl::Cpl0 => {
                            ("write_fault", initial_cr0, initial.gpr[0], 0, false)
                        }
                        [0x0f, 0x22, 0xc0] if allowed.contains(&target_cr0.get()) => {
                            ("write", target_cr0, target_cr0, 1, false)
                        }
                        [0x0f, 0x22, 0xc0] => {
                            ("write_unsupported", initial_cr0, initial.gpr[0], 0, false)
                        }
                        [0xdb, 0xe3] if initial_cr0.get() & 0xcu64 == 0 => {
                            ("fninit", initial_cr0, initial.gpr[0], 1, true)
                        }
                        [0xdb, 0xe3] => ("fninit_fault", initial_cr0, initial.gpr[0], 0, false),
                        [0x0f, 0x22, 0xc0, 0xdb, 0xe3] if cpl != CliCpl::Cpl0 => (
                            "write_fninit_privilege",
                            initial_cr0,
                            initial.gpr[0],
                            0,
                            false,
                        ),
                        [0x0f, 0x22, 0xc0, 0xdb, 0xe3] if !allowed.contains(&target_cr0.get()) => (
                            "write_fninit_unsupported",
                            initial_cr0,
                            initial.gpr[0],
                            0,
                            false,
                        ),
                        [0x0f, 0x22, 0xc0, 0xdb, 0xe3] if target_cr0.get() & 0xcu64 != 0 => {
                            ("write_fninit_fault", target_cr0, target_cr0, 1, false)
                        }
                        [0x0f, 0x22, 0xc0, 0xdb, 0xe3] => {
                            ("write_fninit", target_cr0, target_cr0, 2, true)
                        }
                        [
                            0x0f,
                            0x20,
                            0xc0,
                            0x48,
                            0x83,
                            0xe0,
                            0xf3,
                            0x0f,
                            0x22,
                            0xc0,
                            0xdb,
                            0xe3,
                        ] if cpl == CliCpl::Cpl0 => ("full", HexU64(base), HexU64(base), 4, true),
                        [
                            0x0f,
                            0x20,
                            0xc0,
                            0x48,
                            0x83,
                            0xe0,
                            0xf3,
                            0x0f,
                            0x22,
                            0xc0,
                            0xdb,
                            0xe3,
                        ] => ("full_privilege", initial_cr0, initial.gpr[0], 0, false),
                        _ => return Err(FormatError::CaseMismatch("memory.code")),
                    };
                let expected_outcome = match operation {
                    "read" | "write" | "fninit" | "write_fninit" | "full" => {
                        Outcome::StopAddress {}
                    }
                    "write_unsupported" | "write_fninit_unsupported" => Outcome::Unsupported {
                        reason: String::new(),
                    },
                    _ => Outcome::Exception {},
                };
                // Unsupported reasons are implementation text; compare only the variant.
                let outcome_matches = match (&expected_outcome, &self.outcome) {
                    (Outcome::Unsupported { .. }, Outcome::Unsupported { .. }) => true,
                    (expected, actual) => expected == actual,
                };
                let expected_rip = HexU64(
                    initial.rip.get()
                        + match (operation, retired) {
                            ("write_fninit_fault", _) => 3,
                            (_, 0) => 0,
                            ("fninit", _) | ("fninit_fault", _) => 2,
                            ("write_fninit", _) => 5,
                            ("full", _) => 12,
                            _ => 3,
                        },
                );
                let mut expected_x87 = initial.x87.clone();
                if fninit_retired {
                    let x87 = expected_x87
                        .as_mut()
                        .ok_or(FormatError::CaseMismatch("state.x87"))?;
                    x87.control_word = HexU64(0x037f);
                    x87.status_word = HexU64(0);
                    x87.tag_word = HexU64(0xffff);
                    x87.instruction_pointer = HexU64(0);
                    x87.data_pointer = HexU64(0);
                    x87.instruction_selector = None;
                    x87.data_selector = None;
                    x87.last_opcode = HexU64(0);
                }
                let mov_retired = retired > 0
                    && matches!(bytes.as_slice(), [0x0f, 0x20, 0xc0] | [0x0f, 0x22, 0xc0])
                    || matches!(operation, "write_fninit_fault" | "write_fninit" | "full");
                let arithmetic_flags = INTEGER_FLAGS & !((1 << 10) | 2);
                let expected_defined = if mov_retired {
                    INTEGER_FLAGS & !arithmetic_flags
                } else {
                    INTEGER_FLAGS
                };
                if value.state.cr0 != Some(expected_cr0)
                    || value.state.gpr[0] != expected_rax
                    || value.state.rip != expected_rip
                    || value
                        .state
                        .gpr
                        .iter()
                        .enumerate()
                        .any(|(index, actual)| index != 0 && *actual != initial.gpr[index])
                    || value.state.x87 != expected_x87
                    || value.rflags_defined != HexU64(expected_defined)
                    || self.instructions_executed
                        != (Observation::Available {
                            value: HexU64(retired),
                        })
                    || !outcome_matches
                {
                    return Err(FormatError::CaseMismatch("state.cr0_fp_init"));
                }
                if matches!(self.outcome, Outcome::Exception {}) {
                    let Observation::Available { value: Some(fault) } = &value.exception else {
                        return Err(FormatError::CaseMismatch("snapshot.exception"));
                    };
                    let fault_rip = match operation {
                        "write_fninit_fault" => HexU64(initial.rip.get() + 3),
                        _ => initial.rip,
                    };
                    let vector = if matches!(
                        operation,
                        "read_fault" | "write_fault" | "write_fninit_privilege" | "full_privilege"
                    ) {
                        13
                    } else {
                        7
                    };
                    if fault.faulting_rip != fault_rip
                        || fault.vector != vector
                        || (vector == 13
                            && fault.error_code != (Observation::Available { value: HexU64(0) }))
                    {
                        return Err(FormatError::CaseMismatch("snapshot.exception"));
                    }
                }
            }
            if loaded.fixture.profile == Profile::LongCr4FpInitV1 {
                let initial = &loaded.fixture.initial_state;
                let code = loaded
                    .fixture
                    .memory
                    .iter()
                    .find(|region| region.region_id == "code")
                    .ok_or(FormatError::CaseMismatch("memory.code"))?;
                let bytes = decode_bytes(&code.initial_bytes_hex)
                    .map_err(|_| FormatError::CaseMismatch("memory.code"))?;
                let full = bytes.len() == 18;
                let initial_cr0 = initial.cr0.ok_or(FormatError::CaseMismatch("state.cr0"))?;
                let initial_cr4 = initial.cr4.ok_or(FormatError::CaseMismatch("state.cr4"))?;
                let gate_vector = if initial_cr0.get() & 4 != 0 || initial_cr4.get() & (1 << 9) == 0
                {
                    Some(6)
                } else if initial_cr0.get() & 8 != 0 {
                    Some(7)
                } else {
                    None
                };
                let expected_exit = if full || gate_vector.is_none() {
                    Outcome::StopAddress {}
                } else {
                    Outcome::Exception {}
                };
                let retired = if full {
                    5
                } else if gate_vector.is_none() {
                    1
                } else {
                    0
                };
                let expected_cr0 = if full {
                    HexU64(od_isa::execute::Cr0Profile::BASE)
                } else {
                    initial_cr0
                };
                let expected_cr4 = if full {
                    HexU64(od_isa::execute::Cr4Profile::BASE | (1 << 9) | (1 << 10))
                } else {
                    initial_cr4
                };
                let expected_rax = if full { expected_cr4 } else { initial.gpr[0] };
                let expected_rip = if retired == 0 {
                    initial.rip
                } else {
                    HexU64(initial.rip.get() + if full { 18 } else { 4 })
                };
                let mut expected_x87 = initial.x87.clone();
                let mut expected_sse = initial.sse.clone();
                if full {
                    if let Some(x87) = expected_x87.as_mut() {
                        x87.control_word = HexU64(0x037f);
                        x87.status_word = HexU64(0);
                        x87.tag_word = HexU64(0xffff);
                        x87.instruction_pointer = HexU64(0);
                        x87.data_pointer = HexU64(0);
                        x87.instruction_selector = None;
                        x87.data_selector = None;
                        x87.last_opcode = HexU64(0);
                    }
                }
                if retired > 0 {
                    if let Some(sse) = expected_sse.as_mut() {
                        let lhs = decode_bytes(&sse.xmm[0])
                            .map_err(|_| FormatError::CaseMismatch("state.sse"))?;
                        let rhs = decode_bytes(&sse.xmm[1])
                            .map_err(|_| FormatError::CaseMismatch("state.sse"))?;
                        let output: Vec<u8> = lhs.iter().zip(rhs).map(|(a, b)| a ^ b).collect();
                        sse.xmm[0] = crate::memory::encode_bytes(&output);
                    }
                }
                if value.state.cr0 != Some(expected_cr0)
                    || value.state.cr4 != Some(expected_cr4)
                    || value.state.gpr[0] != expected_rax
                    || value.state.rip != expected_rip
                    || value.state.x87 != expected_x87
                    || value.state.sse != expected_sse
                    || value.rflags_defined
                        != HexU64(if full {
                            INTEGER_FLAGS & !(INTEGER_FLAGS & !((1 << 10) | 2))
                        } else {
                            INTEGER_FLAGS
                        })
                    || self.instructions_executed
                        != (Observation::Available {
                            value: HexU64(retired),
                        })
                    || self.outcome != expected_exit
                    || value
                        .state
                        .gpr
                        .iter()
                        .enumerate()
                        .any(|(i, reg)| i != 0 && *reg != initial.gpr[i])
                {
                    return Err(FormatError::CaseMismatch("state.cr4_fp_init"));
                }
                if let Some(vector) = gate_vector.filter(|_| !full) {
                    let Observation::Available { value: Some(fault) } = &value.exception else {
                        return Err(FormatError::CaseMismatch("snapshot.exception"));
                    };
                    if fault.vector != vector || fault.error_code.is_available() {
                        return Err(FormatError::CaseMismatch("snapshot.exception"));
                    }
                }
            }
            if matches!(
                loaded.fixture.profile,
                Profile::LongCr0Cr4FpInitV1 | Profile::LongCr0Cr4FpInitFxsave64V1
            ) {
                let with_fxsave64 = loaded.fixture.profile == Profile::LongCr0Cr4FpInitFxsave64V1;
                let initial = &loaded.fixture.initial_state;
                let initial_cr4 = initial.cr4.ok_or(FormatError::CaseMismatch("state.cr4"))?;
                let expected_cr4 = HexU64(initial_cr4.get() | (1 << 9) | (1 << 10));
                let mut expected_x87 = initial.x87.clone();
                let x87 = expected_x87
                    .as_mut()
                    .ok_or(FormatError::CaseMismatch("state.x87"))?;
                x87.control_word = HexU64(0x037f);
                x87.status_word = HexU64(0);
                x87.tag_word = HexU64(0xffff);
                x87.instruction_pointer = HexU64(0);
                x87.data_pointer = HexU64(0);
                x87.instruction_selector = None;
                x87.data_selector = None;
                x87.last_opcode = HexU64(0);
                let mut expected_sse = initial.sse.clone();
                let sse = expected_sse
                    .as_mut()
                    .ok_or(FormatError::CaseMismatch("state.sse"))?;
                let lhs = decode_bytes(&sse.xmm[0])
                    .map_err(|_| FormatError::CaseMismatch("state.sse"))?;
                let rhs = decode_bytes(&sse.xmm[1])
                    .map_err(|_| FormatError::CaseMismatch("state.sse"))?;
                sse.xmm[0] = crate::memory::encode_bytes(
                    &lhs.iter().zip(rhs).map(|(a, b)| a ^ b).collect::<Vec<_>>(),
                );
                if value.state.cr0 != Some(HexU64(od_isa::execute::Cr0Profile::BASE))
                    || value.state.cr4 != Some(expected_cr4)
                    || value.state.gpr[0] != expected_cr4
                    || value.state.rip
                        != HexU64(initial.rip.get() + if with_fxsave64 { 33 } else { 28 })
                    || value.state.rflags != HexU64(initial.rflags.get() & !0x8c5)
                    || value.state.x87 != expected_x87
                    || value.state.sse != expected_sse
                    || value.rflags_defined
                        != HexU64(
                            INTEGER_FLAGS
                                & !((1 << 0)
                                    | (1 << 2)
                                    | (1 << 4)
                                    | (1 << 6)
                                    | (1 << 7)
                                    | (1 << 11)),
                        )
                    || self.instructions_executed
                        != (Observation::Available {
                            value: HexU64(if with_fxsave64 { 9 } else { 8 }),
                        })
                    || self.outcome != (Outcome::StopAddress {})
                    || value
                        .state
                        .gpr
                        .iter()
                        .enumerate()
                        .any(|(i, reg)| i != 0 && *reg != initial.gpr[i])
                {
                    return Err(FormatError::CaseMismatch("state.cr0_cr4_fp_init"));
                }
                if with_fxsave64 {
                    let save = value
                        .memory
                        .iter()
                        .find(|region| region.region_id == "save")
                        .ok_or(FormatError::CaseMismatch("memory.save"))?;
                    let bytes = decode_bytes(&save.bytes_hex)?;
                    let initial_save = loaded
                        .fixture
                        .memory
                        .iter()
                        .find(|region| region.region_id == "save")
                        .ok_or(FormatError::CaseMismatch("memory.save"))?;
                    let initial_bytes = decode_bytes(&initial_save.initial_bytes_hex)?;
                    let sse = expected_sse
                        .as_ref()
                        .ok_or(FormatError::CaseMismatch("state.sse"))?;
                    let x87 = expected_x87
                        .as_ref()
                        .ok_or(FormatError::CaseMismatch("state.x87"))?;
                    let xmm = sse
                        .xmm
                        .iter()
                        .flat_map(|register| decode_bytes(register).unwrap_or_default())
                        .collect::<Vec<_>>();
                    let expected_fields = [
                        (0usize, vec![0x7f, 0x03, 0x00, 0x00, 0x00]),
                        (6, vec![0x00, 0x00]),
                        (8, vec![0; 16]),
                        (
                            24,
                            [
                                (sse.mxcsr.get() as u32).to_le_bytes().as_slice(),
                                &0x0000_ffbfu32.to_le_bytes(),
                            ]
                            .concat(),
                        ),
                        (160, xmm),
                    ];
                    if bytes.len() != 512 {
                        return Err(FormatError::CaseMismatch("memory.save.length"));
                    }
                    let stack_matches =
                        x87.registers.iter().enumerate().all(|(index, register)| {
                            decode_bytes(register).is_ok_and(|slot| {
                                bytes.get(32 + index * 16..42 + index * 16) == Some(slot.as_slice())
                            })
                        });
                    if expected_fields.iter().any(|(offset, expected)| {
                        bytes[*offset..*offset + expected.len()] != *expected
                    }) || !stack_matches
                        || bytes[464..512] != initial_bytes[464..512]
                    {
                        return Err(FormatError::CaseMismatch("memory.save.defined_fields"));
                    }
                }
            }
            if matches!(
                loaded.fixture.profile,
                Profile::LongCr0Cr4FpInitFxrstor64V1 | Profile::LongCr0Cr4FpInitFxrstor64MinssV1
            ) {
                let follows_minss =
                    loaded.fixture.profile == Profile::LongCr0Cr4FpInitFxrstor64MinssV1;
                let initial = &loaded.fixture.initial_state;
                let initial_cr4 = initial.cr4.ok_or(FormatError::CaseMismatch("state.cr4"))?;
                let expected_cr4 = HexU64(initial_cr4.get() | 0x600);
                let mut expected_x87 = initial
                    .x87
                    .as_ref()
                    .ok_or(FormatError::CaseMismatch("state.x87"))?
                    .to_cpu()?;
                let mut expected_sse = initial
                    .sse
                    .as_ref()
                    .ok_or(FormatError::CaseMismatch("state.sse"))?
                    .to_cpu()?;
                let image = loaded
                    .fixture
                    .memory
                    .iter()
                    .find(|region| region.region_id == "image")
                    .ok_or(FormatError::CaseMismatch("memory.image"))?;
                let bytes = decode_bytes(&image.initial_bytes_hex)?;
                expected_x87.control_word = u16::from_le_bytes(bytes[0..2].try_into().unwrap());
                expected_x87.status_word = u16::from_le_bytes(bytes[2..4].try_into().unwrap());
                let top = usize::from((expected_x87.status_word >> 11) & 7);
                let abridged = bytes[4];
                for logical in 0..8 {
                    let physical = (top + logical) & 7;
                    let start = 32 + logical * 16;
                    let raw: [u8; 10] = bytes[start..start + 10].try_into().unwrap();
                    expected_x87.registers[physical] = raw;
                    let significand = u64::from_le_bytes(raw[..8].try_into().unwrap());
                    let exponent = u16::from_le_bytes([raw[8], raw[9]]) & 0x7fff;
                    let tag = if abridged & (1 << physical) == 0 {
                        od_isa::state::X87Tag::Empty
                    } else if exponent == 0 && significand == 0 {
                        od_isa::state::X87Tag::Zero
                    } else if (1..=0x7ffe).contains(&exponent) && significand & (1 << 63) != 0 {
                        od_isa::state::X87Tag::Valid
                    } else {
                        od_isa::state::X87Tag::Special
                    };
                    expected_x87.set_tag(physical as u8, tag);
                }
                expected_x87.last_opcode =
                    u16::from_le_bytes(bytes[6..8].try_into().unwrap()) & 0x07ff;
                expected_x87.instruction_pointer =
                    u64::from_le_bytes(bytes[8..16].try_into().unwrap());
                expected_x87.data_pointer = u64::from_le_bytes(bytes[16..24].try_into().unwrap());
                expected_sse.mxcsr = u32::from_le_bytes(bytes[24..28].try_into().unwrap());
                for register in 0..16 {
                    expected_sse.xmm[register]
                        .copy_from_slice(&bytes[160 + register * 16..176 + register * 16]);
                }
                let (expected_retired, expected_rip, expected_outcome) = if follows_minss {
                    let lhs = u32::from_le_bytes(expected_sse.xmm[0][..4].try_into().unwrap());
                    let rhs = u32::from_le_bytes(expected_sse.xmm[1][..4].try_into().unwrap());
                    let lhs_value = Binary32::from_bits(lhs);
                    let rhs_value = Binary32::from_bits(rhs);
                    let invalid = matches!(
                        lhs_value.class(),
                        BinaryClass::QuietNaN | BinaryClass::SignalingNaN
                    ) || matches!(
                        rhs_value.class(),
                        BinaryClass::QuietNaN | BinaryClass::SignalingNaN
                    );
                    let denormal = lhs_value.class() == BinaryClass::Subnormal
                        || rhs_value.class() == BinaryClass::Subnormal;
                    let old_mxcsr = expected_sse.mxcsr;
                    let unmasked = (invalid && old_mxcsr & (1 << 7) == 0)
                        || (denormal && old_mxcsr & (1 << 8) == 0);
                    let sticky = if invalid && denormal && old_mxcsr & 0x180 == 0x180 {
                        1
                    } else {
                        u32::from(invalid) | (u32::from(denormal) << 1)
                    };
                    expected_sse.mxcsr |= sticky;
                    if !unmasked {
                        let result = Binary32::min_sse_raw_result(lhs_value, rhs_value)
                            .value()
                            .to_bits();
                        expected_sse.xmm[0][..4].copy_from_slice(&result.to_le_bytes());
                    }
                    (
                        if unmasked { 9 } else { 10 },
                        HexU64(initial.rip.get() + if unmasked { 33 } else { 37 }),
                        if unmasked {
                            Outcome::Exception {}
                        } else {
                            Outcome::StopAddress {}
                        },
                    )
                } else {
                    (9, HexU64(initial.rip.get() + 33), Outcome::StopAddress {})
                };
                if value.state.cr0 != Some(HexU64(od_isa::execute::Cr0Profile::BASE))
                    || value.state.cr4 != Some(expected_cr4)
                    || value.state.gpr[0] != expected_cr4
                    || value.state.rip != expected_rip
                    || value.state.rflags != HexU64(initial.rflags.get() & !0x8c5)
                    || value.state.x87 != Some(X87Snapshot::from(&expected_x87))
                    || value.state.sse != Some(SseSnapshot::from(&expected_sse))
                    || value.rflags_defined
                        != HexU64(
                            INTEGER_FLAGS
                                & !((1 << 0)
                                    | (1 << 2)
                                    | (1 << 4)
                                    | (1 << 6)
                                    | (1 << 7)
                                    | (1 << 11)),
                        )
                    || self.instructions_executed
                        != (Observation::Available {
                            value: HexU64(expected_retired),
                        })
                    || self.outcome != expected_outcome
                    || value
                        .state
                        .gpr
                        .iter()
                        .enumerate()
                        .any(|(i, reg)| i != 0 && *reg != initial.gpr[i])
                {
                    return Err(FormatError::CaseMismatch("state.cr0_cr4_fxrstor64"));
                }
                let found = value
                    .memory
                    .iter()
                    .find(|region| region.region_id == "image")
                    .ok_or(FormatError::CaseMismatch("memory.image"))?;
                if found.bytes_hex != image.initial_bytes_hex {
                    return Err(FormatError::CaseMismatch("memory.image.unchanged"));
                }
                if follows_minss && matches!(self.outcome, Outcome::Exception {}) {
                    let Observation::Available { value: Some(fault) } = &value.exception else {
                        return Err(FormatError::CaseMismatch("snapshot.exception"));
                    };
                    if fault.faulting_rip != HexU64(initial.rip.get() + 33)
                        || fault.vector != 19
                        || fault.error_code.is_available()
                    {
                        return Err(FormatError::CaseMismatch("snapshot.exception"));
                    }
                }
            }
            if value.memory.len() != loaded.fixture.memory.len() {
                return Err(FormatError::CaseMismatch("memory"));
            }
            for region in &loaded.fixture.memory {
                let found = value
                    .memory
                    .iter()
                    .find(|r| r.region_id == region.region_id)
                    .ok_or(FormatError::CaseMismatch("memory.region_id"))?;
                if found.guest_address != region.guest_address
                    || found.bytes_hex.len() != region.length as usize * 2
                {
                    return Err(FormatError::CaseMismatch("memory.layout"));
                }
            }
        }
        if let Observation::Available { value } = self.instructions_executed {
            if value.get() > loaded.fixture.instruction_budget {
                return Err(FormatError::CaseMismatch("instructions_executed"));
            }
        }
        Ok(())
    }
}

fn validate_maxss_unmasked_result(
    loaded: &LoadedFixture,
    result: &RunResult,
    snapshot: &Snapshot,
) -> Result<(), FormatError> {
    let fixture = &loaded.fixture;
    let controls = fixture
        .maxss_unmasked_controls
        .ok_or(FormatError::CaseMismatch("maxss_unmasked_controls"))?;
    let initial_sse = fixture
        .initial_state
        .sse
        .as_ref()
        .ok_or(FormatError::CaseMismatch("state.sse"))?;
    let lhs_bits = u32::from_le_bytes(
        decode_bytes(&initial_sse.xmm[0])?[..4]
            .try_into()
            .map_err(|_| FormatError::CaseMismatch("state.sse.xmm[0]"))?,
    );
    let rhs_bits = u32::from_le_bytes(
        decode_bytes(&initial_sse.xmm[1])?[..4]
            .try_into()
            .map_err(|_| FormatError::CaseMismatch("state.sse.xmm[1]"))?,
    );
    let lhs = Binary32::from_bits(lhs_bits);
    let rhs = Binary32::from_bits(rhs_bits);
    let invalid = matches!(
        lhs.class(),
        BinaryClass::QuietNaN | BinaryClass::SignalingNaN
    ) || matches!(
        rhs.class(),
        BinaryClass::QuietNaN | BinaryClass::SignalingNaN
    );
    let denormal = lhs.class() == BinaryClass::Subnormal || rhs.class() == BinaryClass::Subnormal;
    let mxcsr = initial_sse.mxcsr.get() as u32;
    let im = mxcsr & (1 << 7) != 0;
    let dm = mxcsr & (1 << 8) != 0;
    let reported_de = denormal && (!invalid || !im || !dm);
    let arithmetic_fault = invalid && !im || denormal && !dm;
    let gate_vector = if !controls.sse_available || controls.cr0_em || !controls.cr4_osfxsr {
        Some(6)
    } else if controls.cr0_ts {
        Some(7)
    } else {
        None
    };
    let fault_vector = gate_vector
        .or_else(|| arithmetic_fault.then_some(if controls.cr4_osxmmexcpt { 19 } else { 6 }));
    if !matches!(
        (fault_vector.is_some(), fixture.expected_exit),
        (true, ExpectedExit::Exception) | (false, ExpectedExit::StopAddress)
    ) {
        return Err(FormatError::CaseMismatch("MAXSS expected exit"));
    }
    let mut expected_state = fixture.initial_state.clone();
    if gate_vector.is_none() {
        let sse = expected_state
            .sse
            .as_mut()
            .ok_or(FormatError::CaseMismatch("state.sse"))?;
        let flags = u32::from(invalid) | (u32::from(reported_de) << 1);
        sse.mxcsr = HexU64(u64::from(mxcsr | flags));
        if fault_vector.is_none() {
            let result_bits = Binary32::max_sse_raw_result(lhs, rhs).value().to_bits();
            let mut xmm0 = decode_bytes(&sse.xmm[0])?;
            xmm0[..4].copy_from_slice(&result_bits.to_le_bytes());
            sse.xmm[0] = crate::memory::encode_bytes(&xmm0);
            expected_state.rip = HexU64(fixture.initial_state.rip.get() + 4);
        }
    }
    if snapshot.state != expected_state
        || snapshot.rflags_defined != HexU64(INTEGER_FLAGS)
        || snapshot.memory
            != fixture
                .memory
                .iter()
                .map(|region| MemoryRegionSnapshot {
                    region_id: region.region_id.clone(),
                    guest_address: region.guest_address,
                    bytes_hex: region.initial_bytes_hex.clone(),
                })
                .collect::<Vec<_>>()
    {
        return Err(FormatError::CaseMismatch("MAXSS output state"));
    }
    let expected_count = if fault_vector.is_some() { 0 } else { 1 };
    if result.instructions_executed
        != (Observation::Available {
            value: HexU64(expected_count),
        })
    {
        return Err(FormatError::CaseMismatch("MAXSS retirement count"));
    }
    match (fault_vector, &result.outcome, &snapshot.exception) {
        (
            Some(vector),
            Outcome::Exception {},
            Observation::Available {
                value: Some(exception),
            },
        ) if exception.vector == vector
            && exception.error_code
                == Observation::Unavailable {
                    reason: "exception has no architectural error code".into(),
                }
            && exception.faulting_rip == fixture.initial_state.rip
            && matches!(exception.fault_address, Observation::Unavailable { .. }) => {}
        (None, Outcome::StopAddress {}, Observation::Available { value: None }) => {}
        _ => return Err(FormatError::CaseMismatch("MAXSS outcome/exception")),
    }
    Ok(())
}

fn independent_minsd_result_bits(lhs: u64, rhs: u64) -> u64 {
    let is_nan = |bits: u64| (bits >> 52) & 0x7ff == 0x7ff && bits & ((1u64 << 52) - 1) != 0;
    if is_nan(lhs) || is_nan(rhs) {
        return rhs;
    }
    // The architectural MINSD choice is source 2 for equal values (including
    // signed-zero ties). A host comparison supplies only numeric ordering;
    // raw bits are selected directly and never canonicalized.
    if f64::from_bits(lhs) < f64::from_bits(rhs) {
        lhs
    } else {
        rhs
    }
}

fn validate_minsd_unmasked_result(
    loaded: &LoadedFixture,
    result: &RunResult,
    snapshot: &Snapshot,
) -> Result<(), FormatError> {
    let fixture = &loaded.fixture;
    let controls = fixture
        .minsd_unmasked_controls
        .ok_or(FormatError::CaseMismatch("minsd_unmasked_controls"))?;
    let initial_sse = fixture
        .initial_state
        .sse
        .as_ref()
        .ok_or(FormatError::CaseMismatch("state.sse"))?;
    let lhs_bytes = decode_bytes(&initial_sse.xmm[0])?;
    let rhs_bytes = decode_bytes(&initial_sse.xmm[1])?;
    let lhs_bits = u64::from_le_bytes(
        lhs_bytes[..8]
            .try_into()
            .map_err(|_| FormatError::CaseMismatch("state.sse.xmm[0]"))?,
    );
    let rhs_bits = u64::from_le_bytes(
        rhs_bytes[..8]
            .try_into()
            .map_err(|_| FormatError::CaseMismatch("state.sse.xmm[1]"))?,
    );
    let is_nan = |bits: u64| (bits >> 52) & 0x7ff == 0x7ff && bits & ((1u64 << 52) - 1) != 0;
    let is_subnormal = |bits: u64| (bits >> 52) & 0x7ff == 0 && bits & ((1u64 << 52) - 1) != 0;
    let invalid = is_nan(lhs_bits) || is_nan(rhs_bits);
    let raw_denormal = is_subnormal(lhs_bits) || is_subnormal(rhs_bits);
    let mxcsr = initial_sse.mxcsr.get() as u32;
    let im = mxcsr & (1 << 7) != 0;
    let dm = mxcsr & (1 << 8) != 0;
    let reported_de = raw_denormal && (!invalid || !im || !dm);
    let arithmetic_fault = invalid && !im || raw_denormal && !dm;
    let gate_vector = if !controls.sse2_available || controls.cr0_em || !controls.cr4_osfxsr {
        Some(6)
    } else if controls.cr0_ts {
        Some(7)
    } else {
        None
    };
    let fault_vector = gate_vector
        .or_else(|| arithmetic_fault.then_some(if controls.cr4_osxmmexcpt { 19 } else { 6 }));
    if !matches!(
        (fault_vector.is_some(), fixture.expected_exit),
        (true, ExpectedExit::Exception) | (false, ExpectedExit::StopAddress)
    ) {
        return Err(FormatError::CaseMismatch("MINSD expected exit"));
    }
    let mut expected_state = fixture.initial_state.clone();
    if gate_vector.is_none() {
        let sse = expected_state
            .sse
            .as_mut()
            .ok_or(FormatError::CaseMismatch("state.sse"))?;
        sse.mxcsr = HexU64(u64::from(
            mxcsr | u32::from(invalid) | (u32::from(reported_de) << 1),
        ));
        if fault_vector.is_none() {
            let result_bits = independent_minsd_result_bits(lhs_bits, rhs_bits);
            let mut xmm0 = decode_bytes(&sse.xmm[0])?;
            xmm0[..8].copy_from_slice(&result_bits.to_le_bytes());
            sse.xmm[0] = crate::memory::encode_bytes(&xmm0);
            expected_state.rip = HexU64(fixture.initial_state.rip.get() + 4);
        }
    }
    if snapshot.state != expected_state
        || snapshot.rflags_defined != HexU64(INTEGER_FLAGS)
        || snapshot.memory
            != fixture
                .memory
                .iter()
                .map(|region| MemoryRegionSnapshot {
                    region_id: region.region_id.clone(),
                    guest_address: region.guest_address,
                    bytes_hex: region.initial_bytes_hex.clone(),
                })
                .collect::<Vec<_>>()
    {
        return Err(FormatError::CaseMismatch("MINSD output state"));
    }
    let expected_count = if fault_vector.is_some() { 0 } else { 1 };
    if result.instructions_executed
        != (Observation::Available {
            value: HexU64(expected_count),
        })
    {
        return Err(FormatError::CaseMismatch("MINSD retirement count"));
    }
    match (fault_vector, &result.outcome, &snapshot.exception) {
        (
            Some(vector),
            Outcome::Exception {},
            Observation::Available {
                value: Some(exception),
            },
        ) if exception.vector == vector
            && exception.error_code
                == Observation::Unavailable {
                    reason: "exception has no architectural error code".into(),
                }
            && exception.faulting_rip == fixture.initial_state.rip
            && matches!(exception.fault_address, Observation::Unavailable { .. }) => {}
        (None, Outcome::StopAddress {}, Observation::Available { value: None }) => {}
        _ => return Err(FormatError::CaseMismatch("MINSD outcome/exception")),
    }
    Ok(())
}

fn independent_minpd_result_bits(lhs: u64, rhs: u64) -> u64 {
    let lhs_nan = (lhs >> 52) & 0x7ff == 0x7ff && lhs & ((1u64 << 52) - 1) != 0;
    let rhs_nan = (rhs >> 52) & 0x7ff == 0x7ff && rhs & ((1u64 << 52) - 1) != 0;
    if lhs_nan || rhs_nan || lhs == rhs || lhs & !(1u64 << 63) == 0 && rhs & !(1u64 << 63) == 0 {
        return rhs;
    }
    let lhs_negative = lhs >> 63 != 0;
    let rhs_negative = rhs >> 63 != 0;
    let less = if lhs_negative != rhs_negative {
        lhs_negative
    } else if lhs_negative {
        (lhs & !(1u64 << 63)) > (rhs & !(1u64 << 63))
    } else {
        lhs < rhs
    };
    if less { lhs } else { rhs }
}

fn validate_minpd_unmasked_result(
    loaded: &LoadedFixture,
    result: &RunResult,
    snapshot: &Snapshot,
) -> Result<(), FormatError> {
    let fixture = &loaded.fixture;
    let controls = fixture
        .minpd_unmasked_controls
        .ok_or(FormatError::CaseMismatch("minpd_unmasked_controls"))?;
    let initial_sse = fixture
        .initial_state
        .sse
        .as_ref()
        .ok_or(FormatError::CaseMismatch("state.sse"))?;
    let lhs_bytes = decode_bytes(&initial_sse.xmm[0])?;
    let rhs_bytes = decode_bytes(&initial_sse.xmm[1])?;
    let mut expected_xmm0 = lhs_bytes.clone();
    let mut lane_invalid = [false; 2];
    let mut lane_denormal = [false; 2];
    for lane in 0..2 {
        let offset = lane * 8;
        let lhs = u64::from_le_bytes(
            lhs_bytes[offset..offset + 8]
                .try_into()
                .map_err(|_| FormatError::CaseMismatch("state.sse.xmm"))?,
        );
        let rhs = u64::from_le_bytes(
            rhs_bytes[offset..offset + 8]
                .try_into()
                .map_err(|_| FormatError::CaseMismatch("state.sse.xmm"))?,
        );
        lane_invalid[lane] = [lhs, rhs]
            .into_iter()
            .any(|bits| ((bits >> 52) & 0x7ff) == 0x7ff && bits & ((1u64 << 52) - 1) != 0);
        lane_denormal[lane] = [lhs, rhs]
            .into_iter()
            .any(|bits| ((bits >> 52) & 0x7ff) == 0 && bits & ((1u64 << 52) - 1) != 0);
        expected_xmm0[offset..offset + 8]
            .copy_from_slice(&independent_minpd_result_bits(lhs, rhs).to_le_bytes());
    }
    let mxcsr = initial_sse.mxcsr.get() as u32;
    let any_invalid = lane_invalid.into_iter().any(|flag| flag);
    let any_denormal = lane_denormal.into_iter().any(|flag| flag);
    let im = mxcsr & (1 << 7) != 0;
    let dm = mxcsr & (1 << 8) != 0;
    let unmasked = any_invalid && !im || any_denormal && !dm;
    let masked_de = (0..2).any(|lane| lane_denormal[lane] && !lane_invalid[lane]);
    let reported_de = if unmasked { any_denormal } else { masked_de };
    let gate_vector = if !controls.sse2_available || controls.cr0_em || !controls.cr4_osfxsr {
        Some(6)
    } else if controls.cr0_ts {
        Some(7)
    } else {
        None
    };
    let fault_vector =
        gate_vector.or_else(|| (unmasked).then_some(if controls.cr4_osxmmexcpt { 19 } else { 6 }));
    if !matches!(
        (fault_vector.is_some(), fixture.expected_exit),
        (true, ExpectedExit::Exception) | (false, ExpectedExit::StopAddress)
    ) {
        return Err(FormatError::CaseMismatch("MINPD expected exit"));
    }
    let mut expected_state = fixture.initial_state.clone();
    if gate_vector.is_none() {
        let sse = expected_state
            .sse
            .as_mut()
            .ok_or(FormatError::CaseMismatch("state.sse"))?;
        sse.mxcsr = HexU64(u64::from(
            mxcsr | u32::from(any_invalid) | (u32::from(reported_de) << 1),
        ));
        if fault_vector.is_none() {
            sse.xmm[0] = crate::memory::encode_bytes(&expected_xmm0);
            expected_state.rip = HexU64(fixture.initial_state.rip.get() + 4);
        }
    }
    if snapshot.state != expected_state
        || snapshot.rflags_defined != HexU64(INTEGER_FLAGS)
        || snapshot.memory
            != fixture
                .memory
                .iter()
                .map(|region| MemoryRegionSnapshot {
                    region_id: region.region_id.clone(),
                    guest_address: region.guest_address,
                    bytes_hex: region.initial_bytes_hex.clone(),
                })
                .collect::<Vec<_>>()
    {
        return Err(FormatError::CaseMismatch("MINPD output state"));
    }
    let expected_count = if fault_vector.is_some() { 0 } else { 1 };
    if result.instructions_executed
        != (Observation::Available {
            value: HexU64(expected_count),
        })
    {
        return Err(FormatError::CaseMismatch("MINPD retirement count"));
    }
    match (fault_vector, &result.outcome, &snapshot.exception) {
        (
            Some(vector),
            Outcome::Exception {},
            Observation::Available {
                value: Some(exception),
            },
        ) if exception.vector == vector
            && exception.error_code
                == Observation::Unavailable {
                    reason: "exception has no architectural error code".into(),
                }
            && exception.faulting_rip == fixture.initial_state.rip
            && matches!(exception.fault_address, Observation::Unavailable { .. }) => {}
        (None, Outcome::StopAddress {}, Observation::Available { value: None }) => {}
        _ => return Err(FormatError::CaseMismatch("MINPD outcome/exception")),
    }
    Ok(())
}

fn independent_minps_result_bits(lhs: u32, rhs: u32) -> u32 {
    let is_nan = |bits: u32| (bits >> 23) & 0xff == 0xff && bits & 0x7f_ffff != 0;
    if is_nan(lhs) || is_nan(rhs) || lhs == rhs || lhs & !(1 << 31) == 0 && rhs & !(1 << 31) == 0 {
        return rhs;
    }
    if f32::from_bits(lhs) < f32::from_bits(rhs) {
        lhs
    } else {
        rhs
    }
}

fn validate_minps_unmasked_result(
    loaded: &LoadedFixture,
    result: &RunResult,
    snapshot: &Snapshot,
) -> Result<(), FormatError> {
    let fixture = &loaded.fixture;
    let controls = fixture
        .minps_unmasked_controls
        .ok_or(FormatError::CaseMismatch("minps_unmasked_controls"))?;
    let initial_sse = fixture
        .initial_state
        .sse
        .as_ref()
        .ok_or(FormatError::CaseMismatch("state.sse"))?;
    let lhs_bytes = decode_bytes(&initial_sse.xmm[0])?;
    let rhs_bytes = decode_bytes(&initial_sse.xmm[1])?;
    let mut expected_xmm0 = lhs_bytes.clone();
    let mut lane_invalid = [false; 4];
    let mut lane_denormal = [false; 4];
    for lane in 0..4 {
        let offset = lane * 4;
        let lhs = u32::from_le_bytes(
            lhs_bytes[offset..offset + 4]
                .try_into()
                .map_err(|_| FormatError::CaseMismatch("state.sse.xmm"))?,
        );
        let rhs = u32::from_le_bytes(
            rhs_bytes[offset..offset + 4]
                .try_into()
                .map_err(|_| FormatError::CaseMismatch("state.sse.xmm"))?,
        );
        lane_invalid[lane] = [lhs, rhs]
            .into_iter()
            .any(|bits| ((bits >> 23) & 0xff) == 0xff && bits & 0x7f_ffff != 0);
        lane_denormal[lane] = [lhs, rhs]
            .into_iter()
            .any(|bits| ((bits >> 23) & 0xff) == 0 && bits & 0x7f_ffff != 0);
        expected_xmm0[offset..offset + 4]
            .copy_from_slice(&independent_minps_result_bits(lhs, rhs).to_le_bytes());
    }
    let mxcsr = initial_sse.mxcsr.get() as u32;
    let any_invalid = lane_invalid.into_iter().any(|flag| flag);
    let any_denormal = lane_denormal.into_iter().any(|flag| flag);
    let im = mxcsr & (1 << 7) != 0;
    let dm = mxcsr & (1 << 8) != 0;
    let unmasked = any_invalid && !im || any_denormal && !dm;
    let masked_de = (0..4).any(|lane| lane_denormal[lane] && !lane_invalid[lane]);
    let reported_de = if unmasked { any_denormal } else { masked_de };
    let gate_vector = if !controls.sse_available || controls.cr0_em || !controls.cr4_osfxsr {
        Some(6)
    } else if controls.cr0_ts {
        Some(7)
    } else {
        None
    };
    let fault_vector =
        gate_vector.or_else(|| unmasked.then_some(if controls.cr4_osxmmexcpt { 19 } else { 6 }));
    if !matches!(
        (fault_vector.is_some(), fixture.expected_exit),
        (true, ExpectedExit::Exception) | (false, ExpectedExit::StopAddress)
    ) {
        return Err(FormatError::CaseMismatch("MINPS expected exit"));
    }
    let mut expected_state = fixture.initial_state.clone();
    if gate_vector.is_none() {
        let sse = expected_state
            .sse
            .as_mut()
            .ok_or(FormatError::CaseMismatch("state.sse"))?;
        sse.mxcsr = HexU64(u64::from(
            mxcsr | u32::from(any_invalid) | (u32::from(reported_de) << 1),
        ));
        if fault_vector.is_none() {
            sse.xmm[0] = crate::memory::encode_bytes(&expected_xmm0);
            expected_state.rip = HexU64(fixture.initial_state.rip.get() + 3);
        }
    }
    if snapshot.state != expected_state
        || snapshot.rflags_defined != HexU64(INTEGER_FLAGS)
        || snapshot.memory
            != fixture
                .memory
                .iter()
                .map(|region| MemoryRegionSnapshot {
                    region_id: region.region_id.clone(),
                    guest_address: region.guest_address,
                    bytes_hex: region.initial_bytes_hex.clone(),
                })
                .collect::<Vec<_>>()
    {
        return Err(FormatError::CaseMismatch("MINPS output state"));
    }
    let expected_count = if fault_vector.is_some() { 0 } else { 1 };
    if result.instructions_executed
        != (Observation::Available {
            value: HexU64(expected_count),
        })
    {
        return Err(FormatError::CaseMismatch("MINPS retirement count"));
    }
    match (fault_vector, &result.outcome, &snapshot.exception) {
        (
            Some(vector),
            Outcome::Exception {},
            Observation::Available {
                value: Some(exception),
            },
        ) if exception.vector == vector
            && exception.error_code
                == Observation::Unavailable {
                    reason: "exception has no architectural error code".into(),
                }
            && exception.faulting_rip == fixture.initial_state.rip
            && matches!(exception.fault_address, Observation::Unavailable { .. }) => {}
        (None, Outcome::StopAddress {}, Observation::Available { value: None }) => {}
        _ => return Err(FormatError::CaseMismatch("MINPS outcome/exception")),
    }
    Ok(())
}

fn independent_maxps_result_bits(lhs: u32, rhs: u32) -> u32 {
    let is_nan = |bits: u32| (bits >> 23) & 0xff == 0xff && bits & 0x7f_ffff != 0;
    if is_nan(lhs) || is_nan(rhs) || lhs == rhs || lhs & !(1 << 31) == 0 && rhs & !(1 << 31) == 0 {
        return rhs;
    }
    if f32::from_bits(lhs) > f32::from_bits(rhs) {
        lhs
    } else {
        rhs
    }
}

fn validate_maxps_unmasked_result(
    loaded: &LoadedFixture,
    result: &RunResult,
    snapshot: &Snapshot,
) -> Result<(), FormatError> {
    let fixture = &loaded.fixture;
    let controls = fixture
        .maxps_unmasked_controls
        .ok_or(FormatError::CaseMismatch("maxps_unmasked_controls"))?;
    let initial_sse = fixture
        .initial_state
        .sse
        .as_ref()
        .ok_or(FormatError::CaseMismatch("state.sse"))?;
    let lhs_bytes = decode_bytes(&initial_sse.xmm[0])?;
    let rhs_bytes = decode_bytes(&initial_sse.xmm[1])?;
    let mut expected_xmm0 = lhs_bytes.clone();
    let mut lane_invalid = [false; 4];
    let mut lane_denormal = [false; 4];
    for lane in 0..4 {
        let offset = lane * 4;
        let lhs = u32::from_le_bytes(
            lhs_bytes[offset..offset + 4]
                .try_into()
                .map_err(|_| FormatError::CaseMismatch("state.sse.xmm"))?,
        );
        let rhs = u32::from_le_bytes(
            rhs_bytes[offset..offset + 4]
                .try_into()
                .map_err(|_| FormatError::CaseMismatch("state.sse.xmm"))?,
        );
        lane_invalid[lane] = [lhs, rhs]
            .into_iter()
            .any(|bits| ((bits >> 23) & 0xff) == 0xff && bits & 0x7f_ffff != 0);
        lane_denormal[lane] = [lhs, rhs]
            .into_iter()
            .any(|bits| ((bits >> 23) & 0xff) == 0 && bits & 0x7f_ffff != 0);
        if let Some(encoded) = fixture.case_id.strip_prefix("s2_38l_maxps_matrix_") {
            let fields: Vec<_> = encoded.split('_').collect();
            let actual = match (lane_invalid[lane], lane_denormal[lane]) {
                (false, false) => "n",
                (true, false) => "i",
                (false, true) => "d",
                (true, true) => "id",
            };
            if fields.len() != 8 || fields[lane + 1] != actual {
                return Err(FormatError::CaseMismatch("MAXPS lane class in case_id"));
            }
        }
        expected_xmm0[offset..offset + 4]
            .copy_from_slice(&independent_maxps_result_bits(lhs, rhs).to_le_bytes());
    }
    let mxcsr = initial_sse.mxcsr.get() as u32;
    let any_invalid = lane_invalid.into_iter().any(|flag| flag);
    let any_denormal = lane_denormal.into_iter().any(|flag| flag);
    let im = mxcsr & (1 << 7) != 0;
    let dm = mxcsr & (1 << 8) != 0;
    let unmasked = any_invalid && !im || any_denormal && !dm;
    let masked_de = (0..4).any(|lane| lane_denormal[lane] && !lane_invalid[lane]);
    let reported_de = if unmasked { any_denormal } else { masked_de };
    let gate_vector = if !controls.sse_available || controls.cr0_em || !controls.cr4_osfxsr {
        Some(6)
    } else if controls.cr0_ts {
        Some(7)
    } else {
        None
    };
    let fault_vector =
        gate_vector.or_else(|| unmasked.then_some(if controls.cr4_osxmmexcpt { 19 } else { 6 }));
    if !matches!(
        (fault_vector.is_some(), fixture.expected_exit),
        (true, ExpectedExit::Exception) | (false, ExpectedExit::StopAddress)
    ) {
        return Err(FormatError::CaseMismatch("MAXPS expected exit"));
    }
    let mut expected_state = fixture.initial_state.clone();
    if gate_vector.is_none() {
        let sse = expected_state
            .sse
            .as_mut()
            .ok_or(FormatError::CaseMismatch("state.sse"))?;
        sse.mxcsr = HexU64(u64::from(
            mxcsr | u32::from(any_invalid) | (u32::from(reported_de) << 1),
        ));
        if fault_vector.is_none() {
            sse.xmm[0] = crate::memory::encode_bytes(&expected_xmm0);
            expected_state.rip = HexU64(fixture.initial_state.rip.get() + 3);
        }
    }
    if snapshot.state != expected_state
        || snapshot.rflags_defined != HexU64(INTEGER_FLAGS)
        || snapshot.memory
            != fixture
                .memory
                .iter()
                .map(|region| MemoryRegionSnapshot {
                    region_id: region.region_id.clone(),
                    guest_address: region.guest_address,
                    bytes_hex: region.initial_bytes_hex.clone(),
                })
                .collect::<Vec<_>>()
    {
        return Err(FormatError::CaseMismatch("MAXPS output state"));
    }
    let expected_count = if fault_vector.is_some() { 0 } else { 1 };
    if result.instructions_executed
        != (Observation::Available {
            value: HexU64(expected_count),
        })
    {
        return Err(FormatError::CaseMismatch("MAXPS retirement count"));
    }
    match (fault_vector, &result.outcome, &snapshot.exception) {
        (
            Some(vector),
            Outcome::Exception {},
            Observation::Available {
                value: Some(exception),
            },
        ) if exception.vector == vector
            && exception.error_code
                == Observation::Unavailable {
                    reason: "exception has no architectural error code".into(),
                }
            && exception.faulting_rip == fixture.initial_state.rip
            && matches!(exception.fault_address, Observation::Unavailable { .. }) => {}
        (None, Outcome::StopAddress {}, Observation::Available { value: None }) => {}
        _ => return Err(FormatError::CaseMismatch("MAXPS outcome/exception")),
    }
    Ok(())
}

fn independent_maxpd_result_bits(lhs: u64, rhs: u64) -> u64 {
    let is_nan = |bits: u64| (bits >> 52) & 0x7ff == 0x7ff && bits & ((1u64 << 52) - 1) != 0;
    if is_nan(lhs)
        || is_nan(rhs)
        || lhs == rhs
        || lhs & !(1u64 << 63) == 0 && rhs & !(1u64 << 63) == 0
    {
        return rhs;
    }
    if f64::from_bits(lhs) > f64::from_bits(rhs) {
        lhs
    } else {
        rhs
    }
}

fn independent_i32_to_binary64_bits(value: i32) -> u64 {
    if value == 0 {
        return 0;
    }
    let negative = value < 0;
    let magnitude = i64::from(value).unsigned_abs();
    let exponent = 63 - magnitude.leading_zeros() as u32;
    let significand = magnitude << (52 - exponent);
    (u64::from(negative) << 63)
        | (u64::from(exponent + 1023) << 52)
        | (significand & ((1u64 << 52) - 1))
}

fn independent_i64_to_binary64(value: i64, rounding_control: u32) -> (u64, bool) {
    if value == 0 {
        return (0, false);
    }
    let negative = value < 0;
    let magnitude = u128::from(value.unsigned_abs());
    let bit_length = 128 - magnitude.leading_zeros();
    let mut exponent = bit_length - 1;
    let mut significand: u128;
    let remainder_nonzero;
    if bit_length <= 53 {
        significand = magnitude << (53 - bit_length);
        remainder_nonzero = false;
    } else {
        let shift = bit_length - 53;
        significand = magnitude >> shift;
        let remainder_mask = (1_u128 << shift) - 1;
        let remainder = magnitude & remainder_mask;
        remainder_nonzero = remainder != 0;
        let increment = match rounding_control {
            0 => {
                let halfway = 1_u128 << (shift - 1);
                remainder > halfway || (remainder == halfway && significand & 1 != 0)
            }
            1 => negative && remainder_nonzero,
            2 => !negative && remainder_nonzero,
            _ => false,
        };
        significand += u128::from(increment);
        if significand == (1_u128 << 53) {
            significand >>= 1;
            exponent += 1;
        }
    }
    let bits = (u64::from(negative) << 63)
        | (u64::from(exponent + 1023) << 52)
        | ((significand as u64) & ((1_u64 << 52) - 1));
    (bits, remainder_nonzero)
}

/// Independent integer-only signed integer to binary32 conversion oracle.
fn independent_i64_to_binary32(value: i64, rounding_control: u32) -> (u32, bool) {
    if value == 0 {
        return (0, false);
    }
    let negative = value < 0;
    let magnitude = u128::from(value.unsigned_abs());
    let bit_length = 128 - magnitude.leading_zeros();
    let mut exponent = bit_length - 1;
    let mut significand: u128;
    let inexact;
    if bit_length <= 24 {
        significand = magnitude << (24 - bit_length);
        inexact = false;
    } else {
        let shift = bit_length - 24;
        significand = magnitude >> shift;
        let remainder = magnitude & ((1_u128 << shift) - 1);
        inexact = remainder != 0;
        let increment = match rounding_control {
            0 => {
                let halfway = 1_u128 << (shift - 1);
                remainder > halfway || (remainder == halfway && significand & 1 != 0)
            }
            1 => negative && inexact,
            2 => !negative && inexact,
            _ => false,
        };
        significand += u128::from(increment);
        if significand == (1_u128 << 24) {
            significand >>= 1;
            exponent += 1;
        }
    }
    let bits = (u32::from(negative) << 31)
        | ((exponent + 127) << 23)
        | ((significand as u32) & ((1_u32 << 23) - 1));
    (bits, inexact)
}

fn validate_cvtdq2pd_result(
    loaded: &LoadedFixture,
    result: &RunResult,
    snapshot: &Snapshot,
) -> Result<(), FormatError> {
    let fixture = &loaded.fixture;
    let controls = fixture
        .cvtdq2pd_controls
        .ok_or(FormatError::CaseMismatch("cvtdq2pd_controls"))?;
    let initial_sse = fixture
        .initial_state
        .sse
        .as_ref()
        .ok_or(FormatError::CaseMismatch("state.sse"))?;
    let code = fixture
        .memory
        .iter()
        .find(|region| region.region_id == "code")
        .ok_or(FormatError::CaseMismatch("memory.code"))?;
    let (destination, source_register, length) =
        cvtdq2pd_register_encoding(&code.initial_bytes_hex)
            .ok_or(FormatError::CaseMismatch("CVTDQ2PD encoding"))?;
    if fixture.profile == Profile::LongCvtdq2pdRegExactV1
        && (destination != 0 || source_register != 1 || length != 4)
    {
        return Err(FormatError::CaseMismatch("CVTDQ2PD exact encoding"));
    }
    let source = decode_bytes(&initial_sse.xmm[usize::from(source_register)])?;
    let mut expected_destination = [0u8; 16];
    for lane in 0..2 {
        let offset = lane * 4;
        let value = i32::from_le_bytes(
            source[offset..offset + 4]
                .try_into()
                .map_err(|_| FormatError::CaseMismatch("state.sse.xmm"))?,
        );
        expected_destination[lane * 8..lane * 8 + 8]
            .copy_from_slice(&independent_i32_to_binary64_bits(value).to_le_bytes());
    }
    let gate_vector = if !controls.sse2_available || controls.cr0_em || !controls.cr4_osfxsr {
        Some(6)
    } else if controls.cr0_ts {
        Some(7)
    } else {
        None
    };
    if !matches!(
        (gate_vector.is_some(), fixture.expected_exit),
        (true, ExpectedExit::Exception) | (false, ExpectedExit::StopAddress)
    ) {
        return Err(FormatError::CaseMismatch("CVTDQ2PD expected exit"));
    }
    let mut expected_state = fixture.initial_state.clone();
    if gate_vector.is_none() {
        let sse = expected_state
            .sse
            .as_mut()
            .ok_or(FormatError::CaseMismatch("state.sse"))?;
        sse.xmm[usize::from(destination)] = crate::memory::encode_bytes(&expected_destination);
        expected_state.rip = HexU64(
            fixture
                .initial_state
                .rip
                .get()
                .checked_add(u64::from(length))
                .ok_or(FormatError::CaseMismatch("CVTDQ2PD stop overflow"))?,
        );
    }
    let expected_memory = fixture
        .memory
        .iter()
        .map(|region| MemoryRegionSnapshot {
            region_id: region.region_id.clone(),
            guest_address: region.guest_address,
            bytes_hex: region.initial_bytes_hex.clone(),
        })
        .collect::<Vec<_>>();
    if snapshot.state != expected_state
        || snapshot.rflags_defined != HexU64(INTEGER_FLAGS)
        || snapshot.memory != expected_memory
    {
        return Err(FormatError::CaseMismatch("CVTDQ2PD output state"));
    }
    let expected_count = u64::from(gate_vector.is_none());
    if result.instructions_executed
        != (Observation::Available {
            value: HexU64(expected_count),
        })
    {
        return Err(FormatError::CaseMismatch("CVTDQ2PD retirement count"));
    }
    match (gate_vector, &result.outcome, &snapshot.exception) {
        (
            Some(vector),
            Outcome::Exception {},
            Observation::Available {
                value: Some(exception),
            },
        ) if exception.vector == vector
            && exception.error_code
                == Observation::Unavailable {
                    reason: "exception has no architectural error code".into(),
                }
            && exception.faulting_rip == fixture.initial_state.rip
            && matches!(exception.fault_address, Observation::Unavailable { .. }) => {}
        (None, Outcome::StopAddress {}, Observation::Available { value: None }) => {}
        _ => return Err(FormatError::CaseMismatch("CVTDQ2PD outcome/exception")),
    }
    Ok(())
}

fn validate_cvtdq2ps_registers_result(
    loaded: &LoadedFixture,
    result: &RunResult,
    snapshot: &Snapshot,
) -> Result<(), FormatError> {
    let fixture = &loaded.fixture;
    let controls = fixture
        .cvtdq2ps_controls
        .ok_or(FormatError::CaseMismatch("cvtdq2ps_controls"))?;
    let initial_sse = fixture
        .initial_state
        .sse
        .as_ref()
        .ok_or(FormatError::CaseMismatch("state.sse"))?;
    let code = fixture
        .memory
        .iter()
        .find(|region| region.region_id == "code")
        .ok_or(FormatError::CaseMismatch("memory.code"))?;
    let (destination, source, length) = cvtdq2ps_register_encoding(&code.initial_bytes_hex)
        .ok_or(FormatError::CaseMismatch("CVTDQ2PS encoding"))?;
    let source_bytes = decode_bytes(&initial_sse.xmm[usize::from(source)])?;
    let old_mxcsr = initial_sse.mxcsr.get() as u32;
    let rc = (old_mxcsr >> 13) & 3;
    let mut expected_destination = [0u8; 16];
    let mut lane_inexact_mask = 0u8;
    for lane in 0..4 {
        let offset = lane * 4;
        let value = i32::from_le_bytes(
            source_bytes[offset..offset + 4]
                .try_into()
                .map_err(|_| FormatError::CaseMismatch("state.sse.xmm"))?,
        );
        let (bits, inexact) = independent_i64_to_binary32(i64::from(value), rc);
        expected_destination[offset..offset + 4].copy_from_slice(&bits.to_le_bytes());
        lane_inexact_mask |= u8::from(inexact) << lane;
    }
    let new_precision = lane_inexact_mask != 0;
    let gate_vector = if !controls.sse2_available || controls.cr0_em || !controls.cr4_osfxsr {
        Some(6)
    } else if controls.cr0_ts {
        Some(7)
    } else {
        None
    };
    let unmasked_precision = gate_vector.is_none() && new_precision && old_mxcsr & (1 << 12) == 0;
    let fault_vector = gate_vector
        .or_else(|| unmasked_precision.then_some(if controls.cr4_osxmmexcpt { 19 } else { 6 }));
    if !matches!(
        (fault_vector.is_some(), fixture.expected_exit),
        (true, ExpectedExit::Exception) | (false, ExpectedExit::StopAddress)
    ) {
        return Err(FormatError::CaseMismatch("CVTDQ2PS expected exit"));
    }
    let mut expected_state = fixture.initial_state.clone();
    if gate_vector.is_none() {
        let sse = expected_state
            .sse
            .as_mut()
            .ok_or(FormatError::CaseMismatch("state.sse"))?;
        sse.mxcsr = HexU64(u64::from(old_mxcsr | (u32::from(new_precision) << 5)));
        if !unmasked_precision {
            sse.xmm[usize::from(destination)] = crate::memory::encode_bytes(&expected_destination);
            expected_state.rip = HexU64(
                fixture
                    .initial_state
                    .rip
                    .get()
                    .checked_add(u64::from(length))
                    .ok_or(FormatError::CaseMismatch("CVTDQ2PS stop overflow"))?,
            );
        }
    }
    let expected_memory = fixture
        .memory
        .iter()
        .map(|region| MemoryRegionSnapshot {
            region_id: region.region_id.clone(),
            guest_address: region.guest_address,
            bytes_hex: region.initial_bytes_hex.clone(),
        })
        .collect::<Vec<_>>();
    if snapshot.state != expected_state
        || snapshot.rflags_defined != HexU64(INTEGER_FLAGS)
        || snapshot.memory != expected_memory
    {
        return Err(FormatError::CaseMismatch("CVTDQ2PS output state"));
    }
    let expected_count = u64::from(fault_vector.is_none());
    if result.instructions_executed
        != (Observation::Available {
            value: HexU64(expected_count),
        })
    {
        return Err(FormatError::CaseMismatch("CVTDQ2PS retirement count"));
    }
    match (fault_vector, &result.outcome, &snapshot.exception) {
        (
            Some(vector),
            Outcome::Exception {},
            Observation::Available {
                value: Some(exception),
            },
        ) if exception.vector == vector
            && exception.error_code
                == Observation::Unavailable {
                    reason: "exception has no architectural error code".into(),
                }
            && exception.faulting_rip == fixture.initial_state.rip
            && matches!(exception.fault_address, Observation::Unavailable { .. }) => {}
        (None, Outcome::StopAddress {}, Observation::Available { value: None }) => {}
        _ => return Err(FormatError::CaseMismatch("CVTDQ2PS outcome/exception")),
    }
    Ok(())
}

fn validate_cvtss2sd_registers_result(
    loaded: &LoadedFixture,
    result: &RunResult,
    snapshot: &Snapshot,
) -> Result<(), FormatError> {
    let fixture = &loaded.fixture;
    let controls = fixture
        .cvtss2sd_controls
        .ok_or(FormatError::CaseMismatch("cvtss2sd_controls"))?;
    let initial_sse = fixture
        .initial_state
        .sse
        .as_ref()
        .ok_or(FormatError::CaseMismatch("state.sse"))?;
    let code = fixture
        .memory
        .iter()
        .find(|region| region.region_id == "code")
        .ok_or(FormatError::CaseMismatch("memory.code"))?;
    let (destination, source, length) = cvtss2sd_register_encoding(&code.initial_bytes_hex)
        .ok_or(FormatError::CaseMismatch("CVTSS2SD encoding"))?;
    let source_bytes = decode_bytes(&initial_sse.xmm[usize::from(source)])?;
    let raw = u32::from_le_bytes(
        source_bytes[..4]
            .try_into()
            .map_err(|_| FormatError::CaseMismatch("state.sse.xmm"))?,
    );
    let (wide, new_ie, new_de) = independent_binary32_to_binary64(raw);
    let old_mxcsr = initial_sse.mxcsr.get() as u32;
    let gate_vector = if !controls.sse2_available || controls.cr0_em || !controls.cr4_osfxsr {
        Some(6)
    } else if controls.cr0_ts {
        Some(7)
    } else {
        None
    };
    let unmasked = gate_vector.is_none()
        && ((new_ie && old_mxcsr & (1 << 7) == 0) || (new_de && old_mxcsr & (1 << 8) == 0));
    let fault_vector =
        gate_vector.or_else(|| unmasked.then_some(if controls.cr4_osxmmexcpt { 19 } else { 6 }));
    if !matches!(
        (fault_vector.is_some(), fixture.expected_exit),
        (true, ExpectedExit::Exception) | (false, ExpectedExit::StopAddress)
    ) {
        return Err(FormatError::CaseMismatch("CVTSS2SD expected exit"));
    }
    let mut expected_state = fixture.initial_state.clone();
    if gate_vector.is_none() {
        let sse = expected_state
            .sse
            .as_mut()
            .ok_or(FormatError::CaseMismatch("state.sse"))?;
        sse.mxcsr = HexU64(u64::from(
            old_mxcsr | u32::from(new_ie) | (u32::from(new_de) << 1),
        ));
        if !unmasked {
            let mut destination_bytes = decode_bytes(&initial_sse.xmm[usize::from(destination)])?;
            destination_bytes[..8].copy_from_slice(&wide.to_le_bytes());
            sse.xmm[usize::from(destination)] = crate::memory::encode_bytes(&destination_bytes);
            expected_state.rip = HexU64(
                fixture
                    .initial_state
                    .rip
                    .get()
                    .checked_add(u64::from(length))
                    .ok_or(FormatError::CaseMismatch("CVTSS2SD stop overflow"))?,
            );
        }
    }
    let expected_memory = fixture
        .memory
        .iter()
        .map(|region| MemoryRegionSnapshot {
            region_id: region.region_id.clone(),
            guest_address: region.guest_address,
            bytes_hex: region.initial_bytes_hex.clone(),
        })
        .collect::<Vec<_>>();
    if snapshot.state != expected_state
        || snapshot.rflags_defined != HexU64(INTEGER_FLAGS)
        || snapshot.memory != expected_memory
    {
        return Err(FormatError::CaseMismatch("CVTSS2SD output state"));
    }
    let expected_count = u64::from(fault_vector.is_none());
    if result.instructions_executed
        != (Observation::Available {
            value: HexU64(expected_count),
        })
    {
        return Err(FormatError::CaseMismatch("CVTSS2SD retirement count"));
    }
    match (fault_vector, &result.outcome, &snapshot.exception) {
        (
            Some(vector),
            Outcome::Exception {},
            Observation::Available {
                value: Some(exception),
            },
        ) if exception.vector == vector
            && exception.error_code
                == Observation::Unavailable {
                    reason: "exception has no architectural error code".into(),
                }
            && exception.faulting_rip == fixture.initial_state.rip
            && matches!(exception.fault_address, Observation::Unavailable { .. }) => {}
        (None, Outcome::StopAddress {}, Observation::Available { value: None }) => {}
        _ => return Err(FormatError::CaseMismatch("CVTSS2SD outcome/exception")),
    }
    Ok(())
}

fn validate_cvtps2pd_registers_result(
    loaded: &LoadedFixture,
    result: &RunResult,
    snapshot: &Snapshot,
) -> Result<(), FormatError> {
    let fixture = &loaded.fixture;
    let controls = fixture
        .cvtps2pd_controls
        .ok_or(FormatError::CaseMismatch("cvtps2pd_controls"))?;
    let initial_sse = fixture
        .initial_state
        .sse
        .as_ref()
        .ok_or(FormatError::CaseMismatch("state.sse"))?;
    let code = fixture
        .memory
        .iter()
        .find(|region| region.region_id == "code")
        .ok_or(FormatError::CaseMismatch("memory.code"))?;
    let (destination, source, length) = cvtps2pd_register_encoding(&code.initial_bytes_hex)
        .ok_or(FormatError::CaseMismatch("CVTPS2PD encoding"))?;
    let source_bytes = decode_bytes(&initial_sse.xmm[usize::from(source)])?;
    let mut converted = [0u64; 2];
    let mut new_ie = false;
    let mut new_de = false;
    for lane in 0..2 {
        let raw = u32::from_le_bytes(
            source_bytes[lane * 4..lane * 4 + 4]
                .try_into()
                .map_err(|_| FormatError::CaseMismatch("state.sse.xmm"))?,
        );
        let (wide, ie, de) = independent_binary32_to_binary64(raw);
        converted[lane] = wide;
        new_ie |= ie;
        new_de |= de;
    }
    let old_mxcsr = initial_sse.mxcsr.get() as u32;
    let gate_vector = if !controls.sse2_available || controls.cr0_em || !controls.cr4_osfxsr {
        Some(6)
    } else if controls.cr0_ts {
        Some(7)
    } else {
        None
    };
    let unmasked = gate_vector.is_none()
        && ((new_ie && old_mxcsr & (1 << 7) == 0) || (new_de && old_mxcsr & (1 << 8) == 0));
    let fault_vector =
        gate_vector.or_else(|| unmasked.then_some(if controls.cr4_osxmmexcpt { 19 } else { 6 }));
    if !matches!(
        (fault_vector.is_some(), fixture.expected_exit),
        (true, ExpectedExit::Exception) | (false, ExpectedExit::StopAddress)
    ) {
        return Err(FormatError::CaseMismatch("CVTPS2PD expected exit"));
    }
    let mut expected_state = fixture.initial_state.clone();
    if gate_vector.is_none() {
        let sse = expected_state
            .sse
            .as_mut()
            .ok_or(FormatError::CaseMismatch("state.sse"))?;
        sse.mxcsr = HexU64(u64::from(
            old_mxcsr | u32::from(new_ie) | (u32::from(new_de) << 1),
        ));
        if !unmasked {
            let mut dest = decode_bytes(&initial_sse.xmm[usize::from(destination)])?;
            dest[..8].copy_from_slice(&converted[0].to_le_bytes());
            dest[8..].copy_from_slice(&converted[1].to_le_bytes());
            sse.xmm[usize::from(destination)] = crate::memory::encode_bytes(&dest);
            expected_state.rip = HexU64(
                fixture
                    .initial_state
                    .rip
                    .get()
                    .checked_add(u64::from(length))
                    .ok_or(FormatError::CaseMismatch("CVTPS2PD stop overflow"))?,
            );
        }
    }
    let expected_memory = fixture
        .memory
        .iter()
        .map(|region| MemoryRegionSnapshot {
            region_id: region.region_id.clone(),
            guest_address: region.guest_address,
            bytes_hex: region.initial_bytes_hex.clone(),
        })
        .collect::<Vec<_>>();
    if snapshot.state != expected_state
        || snapshot.rflags_defined != HexU64(INTEGER_FLAGS)
        || snapshot.memory != expected_memory
    {
        return Err(FormatError::CaseMismatch("CVTPS2PD output state"));
    }
    let expected_count = u64::from(fault_vector.is_none());
    if result.instructions_executed
        != (Observation::Available {
            value: HexU64(expected_count),
        })
    {
        return Err(FormatError::CaseMismatch("CVTPS2PD retirement count"));
    }
    match (fault_vector, &result.outcome, &snapshot.exception) {
        (
            Some(vector),
            Outcome::Exception {},
            Observation::Available {
                value: Some(exception),
            },
        ) if exception.vector == vector
            && exception.error_code
                == Observation::Unavailable {
                    reason: "exception has no architectural error code".into(),
                }
            && exception.faulting_rip == fixture.initial_state.rip
            && matches!(exception.fault_address, Observation::Unavailable { .. }) => {}
        (None, Outcome::StopAddress {}, Observation::Available { value: None }) => {}
        _ => return Err(FormatError::CaseMismatch("CVTPS2PD outcome/exception")),
    }
    Ok(())
}

fn validate_comiss_ucomiss_registers_result(
    loaded: &LoadedFixture,
    result: &RunResult,
    snapshot: &Snapshot,
) -> Result<(), FormatError> {
    let fixture = &loaded.fixture;
    let controls = fixture
        .comiss_ucomiss_controls
        .ok_or(FormatError::CaseMismatch("comiss_ucomiss_controls"))?;
    let sse = fixture
        .initial_state
        .sse
        .as_ref()
        .ok_or(FormatError::CaseMismatch("state.sse"))?;
    let code = fixture
        .memory
        .iter()
        .find(|r| r.region_id == "code")
        .ok_or(FormatError::CaseMismatch("memory.code"))?;
    let (unordered_quiet, destination, source, length) =
        comiss_ucomiss_register_encoding(&code.initial_bytes_hex)
            .ok_or(FormatError::CaseMismatch("COMISS/UCOMISS encoding"))?;
    let lhs_bytes = decode_bytes(&sse.xmm[usize::from(destination)])?;
    let rhs_bytes = decode_bytes(&sse.xmm[usize::from(source)])?;
    let lhs = u32::from_le_bytes(
        lhs_bytes[..4]
            .try_into()
            .map_err(|_| FormatError::CaseMismatch("state.sse.xmm"))?,
    );
    let rhs = u32::from_le_bytes(
        rhs_bytes[..4]
            .try_into()
            .map_err(|_| FormatError::CaseMismatch("state.sse.xmm"))?,
    );
    let classify = |bits: u32| -> u8 {
        let exp = (bits >> 23) & 0xff;
        let frac = bits & 0x7f_ffff;
        if exp == 0xff && frac != 0 {
            if frac & 0x40_0000 != 0 { 3 } else { 4 }
        } else if exp == 0 && frac != 0 {
            2
        } else {
            0
        }
    };
    let lc = classify(lhs);
    let rc = classify(rhs);
    let has_nan = matches!(lc, 3 | 4) || matches!(rc, 3 | 4);
    let new_ie = if unordered_quiet {
        lc == 4 || rc == 4
    } else {
        has_nan
    };
    // Rev. 093 §4.9.2 gives NaN higher priority than denormal for these compares.
    let new_de = !has_nan && (lc == 2 || rc == 2);
    let old_mxcsr = sse.mxcsr.get() as u32;
    let gate = if !controls.sse_available || controls.cr0_em || !controls.cr4_osfxsr {
        Some(6)
    } else if controls.cr0_ts {
        Some(7)
    } else {
        None
    };
    let unmasked = gate.is_none()
        && ((new_ie && old_mxcsr & (1 << 7) == 0) || (new_de && old_mxcsr & (1 << 8) == 0));
    let fault = gate.or_else(|| unmasked.then_some(if controls.cr4_osxmmexcpt { 19 } else { 6 }));
    if !matches!(
        (fault.is_some(), fixture.expected_exit),
        (true, ExpectedExit::Exception) | (false, ExpectedExit::StopAddress)
    ) {
        return Err(FormatError::CaseMismatch("COMISS/UCOMISS expected exit"));
    }
    let mut expected = fixture.initial_state.clone();
    if gate.is_none() {
        let expected_sse = expected
            .sse
            .as_mut()
            .ok_or(FormatError::CaseMismatch("state.sse"))?;
        expected_sse.mxcsr = HexU64(u64::from(
            old_mxcsr | u32::from(new_ie) | (u32::from(new_de) << 1),
        ));
        if !unmasked {
            let flags_mask = (1 << 0) | (1 << 2) | (1 << 4) | (1 << 6) | (1 << 7) | (1 << 11);
            expected.rflags = HexU64(expected.rflags.get() & !flags_mask);
            if has_nan {
                expected.rflags = HexU64(expected.rflags.get() | (1 << 0) | (1 << 2) | (1 << 6));
            } else {
                // Compare signed magnitude from the encoded fields, without host FP.
                let ordering = if lhs & 0x7fff_ffff == 0 && rhs & 0x7fff_ffff == 0 {
                    core::cmp::Ordering::Equal
                } else if lhs >> 31 != rhs >> 31 {
                    if lhs >> 31 != 0 {
                        core::cmp::Ordering::Less
                    } else {
                        core::cmp::Ordering::Greater
                    }
                } else if lhs >> 31 == 0 {
                    (lhs & 0x7fff_ffff).cmp(&(rhs & 0x7fff_ffff))
                } else {
                    (rhs & 0x7fff_ffff).cmp(&(lhs & 0x7fff_ffff))
                };
                match ordering {
                    core::cmp::Ordering::Less => {
                        expected.rflags = HexU64(expected.rflags.get() | 1)
                    }
                    core::cmp::Ordering::Equal => {
                        expected.rflags = HexU64(expected.rflags.get() | (1 << 6))
                    }
                    core::cmp::Ordering::Greater => {}
                }
            }
            expected.rip = HexU64(
                fixture
                    .initial_state
                    .rip
                    .get()
                    .checked_add(u64::from(length))
                    .ok_or(FormatError::CaseMismatch("COMISS/UCOMISS stop overflow"))?,
            );
        }
    }
    let expected_memory = fixture
        .memory
        .iter()
        .map(|region| MemoryRegionSnapshot {
            region_id: region.region_id.clone(),
            guest_address: region.guest_address,
            bytes_hex: region.initial_bytes_hex.clone(),
        })
        .collect::<Vec<_>>();
    if snapshot.state != expected
        || snapshot.rflags_defined != HexU64(INTEGER_FLAGS)
        || snapshot.memory != expected_memory
    {
        return Err(FormatError::CaseMismatch("COMISS/UCOMISS output state"));
    }
    let count = u64::from(fault.is_none());
    if result.instructions_executed
        != (Observation::Available {
            value: HexU64(count),
        })
    {
        return Err(FormatError::CaseMismatch("COMISS/UCOMISS retirement count"));
    }
    match (fault, &result.outcome, &snapshot.exception) {
        (
            Some(vector),
            Outcome::Exception {},
            Observation::Available {
                value: Some(exception),
            },
        ) if exception.vector == vector
            && exception.error_code
                == Observation::Unavailable {
                    reason: "exception has no architectural error code".into(),
                }
            && exception.faulting_rip == fixture.initial_state.rip
            && matches!(exception.fault_address, Observation::Unavailable { .. }) => {}
        (None, Outcome::StopAddress {}, Observation::Available { value: None }) => {}
        _ => {
            return Err(FormatError::CaseMismatch(
                "COMISS/UCOMISS outcome/exception",
            ));
        }
    }
    Ok(())
}

/// Independent raw-bit oracle for CVTSS2SD; it does not use production FP helpers.
fn independent_binary32_to_binary64(raw: u32) -> (u64, bool, bool) {
    let sign = u64::from(raw >> 31) << 63;
    let exponent = (raw >> 23) & 0xff;
    let fraction = raw & 0x7f_ffff;
    if exponent == 0 {
        if fraction == 0 {
            return (sign, false, false);
        }
        let leading = 31 - fraction.leading_zeros();
        let unbiased = leading as i32 - 149;
        let wide_exp = (unbiased + 1023) as u64;
        let wide_fraction = (u64::from(fraction) << (52 - leading)) & ((1_u64 << 52) - 1);
        return (sign | (wide_exp << 52) | wide_fraction, false, true);
    }
    if exponent != 0xff {
        return (
            sign | (u64::from(exponent + (1023 - 127)) << 52) | (u64::from(fraction) << 29),
            false,
            false,
        );
    }
    if fraction == 0 {
        return (sign | (0x7ff_u64 << 52), false, false);
    }
    let signaling = fraction & (1 << 22) == 0;
    let mut wide_fraction = u64::from(fraction) << 29;
    if signaling {
        wide_fraction |= 1_u64 << 51;
    }
    (sign | (0x7ff_u64 << 52) | wide_fraction, signaling, false)
}

fn validate_cvtsi2sd_r32_result(
    loaded: &LoadedFixture,
    result: &RunResult,
    snapshot: &Snapshot,
) -> Result<(), FormatError> {
    let fixture = &loaded.fixture;
    let controls = fixture
        .cvtsi2sd_r32_controls
        .ok_or(FormatError::CaseMismatch("cvtsi2sd_r32_controls"))?;
    let initial_sse = fixture
        .initial_state
        .sse
        .as_ref()
        .ok_or(FormatError::CaseMismatch("state.sse"))?;
    let low = fixture.initial_state.gpr[0].get() as u32 as i32;
    let mut expected_state = fixture.initial_state.clone();
    let gate_vector = if !controls.sse2_available || controls.cr0_em || !controls.cr4_osfxsr {
        Some(6)
    } else if controls.cr0_ts {
        Some(7)
    } else {
        None
    };
    if !matches!(
        (gate_vector.is_some(), fixture.expected_exit),
        (true, ExpectedExit::Exception) | (false, ExpectedExit::StopAddress)
    ) {
        return Err(FormatError::CaseMismatch("CVTSI2SD r32 expected exit"));
    }
    if gate_vector.is_none() {
        let mut xmm0 = decode_bytes(&initial_sse.xmm[0])?;
        xmm0[..8].copy_from_slice(&independent_i32_to_binary64_bits(low).to_le_bytes());
        expected_state
            .sse
            .as_mut()
            .ok_or(FormatError::CaseMismatch("state.sse"))?
            .xmm[0] = crate::memory::encode_bytes(&xmm0);
        expected_state.rip = HexU64(fixture.initial_state.rip.get() + 4);
    }
    let expected_memory = fixture
        .memory
        .iter()
        .map(|region| MemoryRegionSnapshot {
            region_id: region.region_id.clone(),
            guest_address: region.guest_address,
            bytes_hex: region.initial_bytes_hex.clone(),
        })
        .collect::<Vec<_>>();
    if snapshot.state != expected_state
        || snapshot.rflags_defined != HexU64(INTEGER_FLAGS)
        || snapshot.memory != expected_memory
    {
        return Err(FormatError::CaseMismatch("CVTSI2SD r32 output state"));
    }
    let expected_count = u64::from(gate_vector.is_none());
    if result.instructions_executed
        != (Observation::Available {
            value: HexU64(expected_count),
        })
    {
        return Err(FormatError::CaseMismatch("CVTSI2SD r32 retirement count"));
    }
    match (gate_vector, &result.outcome, &snapshot.exception) {
        (
            Some(vector),
            Outcome::Exception {},
            Observation::Available {
                value: Some(exception),
            },
        ) if exception.vector == vector
            && exception.error_code
                == Observation::Unavailable {
                    reason: "exception has no architectural error code".into(),
                }
            && exception.faulting_rip == fixture.initial_state.rip
            && matches!(exception.fault_address, Observation::Unavailable { .. }) => {}
        (None, Outcome::StopAddress {}, Observation::Available { value: None }) => {}
        _ => return Err(FormatError::CaseMismatch("CVTSI2SD r32 outcome/exception")),
    }
    Ok(())
}

fn validate_cvtsi2sd_r64_result(
    loaded: &LoadedFixture,
    result: &RunResult,
    snapshot: &Snapshot,
) -> Result<(), FormatError> {
    let fixture = &loaded.fixture;
    let controls = fixture
        .cvtsi2sd_r64_controls
        .ok_or(FormatError::CaseMismatch("cvtsi2sd_r64_controls"))?;
    let mxcsr = fixture
        .initial_state
        .sse
        .as_ref()
        .ok_or(FormatError::CaseMismatch("state.sse"))?
        .mxcsr
        .get() as u32;
    let (bits, inexact) =
        independent_i64_to_binary64(fixture.initial_state.gpr[0].get() as i64, (mxcsr >> 13) & 3);
    let prerequisite = if !controls.sse2_available || controls.cr0_em || !controls.cr4_osfxsr {
        Some(6)
    } else if controls.cr0_ts {
        Some(7)
    } else {
        None
    };
    let unmasked_precision = prerequisite.is_none() && inexact && mxcsr & (1 << 12) == 0;
    let post_fault = if unmasked_precision {
        Some(if controls.cr4_osxmmexcpt { 19 } else { 6 })
    } else {
        None
    };
    let fault = prerequisite.or(post_fault);
    if !matches!(
        (fault.is_some(), fixture.expected_exit),
        (true, ExpectedExit::Exception) | (false, ExpectedExit::StopAddress)
    ) {
        return Err(FormatError::CaseMismatch("CVTSI2SD r64 expected exit"));
    }
    let mut expected_state = fixture.initial_state.clone();
    if prerequisite.is_none() {
        let sse = expected_state
            .sse
            .as_mut()
            .ok_or(FormatError::CaseMismatch("state.sse"))?;
        sse.mxcsr = HexU64(u64::from(mxcsr | (u32::from(inexact) << 5)));
        if post_fault.is_none() {
            let mut xmm0 = decode_bytes(&sse.xmm[0])?;
            xmm0[..8].copy_from_slice(&bits.to_le_bytes());
            sse.xmm[0] = crate::memory::encode_bytes(&xmm0);
            expected_state.rip = HexU64(fixture.initial_state.rip.get() + 5);
        }
    }
    let expected_memory = fixture
        .memory
        .iter()
        .map(|region| MemoryRegionSnapshot {
            region_id: region.region_id.clone(),
            guest_address: region.guest_address,
            bytes_hex: region.initial_bytes_hex.clone(),
        })
        .collect::<Vec<_>>();
    if snapshot.state != expected_state
        || snapshot.rflags_defined != HexU64(INTEGER_FLAGS)
        || snapshot.memory != expected_memory
    {
        return Err(FormatError::CaseMismatch("CVTSI2SD r64 output state"));
    }
    let count = u64::from(fault.is_none());
    if result.instructions_executed
        != (Observation::Available {
            value: HexU64(count),
        })
    {
        return Err(FormatError::CaseMismatch("CVTSI2SD r64 retirement count"));
    }
    match (fault, &result.outcome, &snapshot.exception) {
        (
            Some(vector),
            Outcome::Exception {},
            Observation::Available {
                value: Some(exception),
            },
        ) if exception.vector == vector
            && exception.error_code
                == Observation::Unavailable {
                    reason: "exception has no architectural error code".into(),
                }
            && exception.faulting_rip == fixture.initial_state.rip
            && matches!(exception.fault_address, Observation::Unavailable { .. }) => {}
        (None, Outcome::StopAddress {}, Observation::Available { value: None }) => {}
        _ => return Err(FormatError::CaseMismatch("CVTSI2SD r64 outcome/exception")),
    }
    Ok(())
}

fn validate_cvtsi2sd_registers_result(
    loaded: &LoadedFixture,
    result: &RunResult,
    snapshot: &Snapshot,
) -> Result<(), FormatError> {
    let fixture = &loaded.fixture;
    let code = fixture
        .memory
        .iter()
        .find(|region| region.region_id == "code")
        .ok_or(FormatError::CaseMismatch("memory.code"))?;
    let (wide, destination, source, length) =
        cvtsi2sd_register_encoding(&code.initial_bytes_hex)
            .ok_or(FormatError::CaseMismatch("CVTSI2SD encoding"))?;
    let controls = fixture
        .cvtsi2sd_r32_controls
        .ok_or(FormatError::CaseMismatch("cvtsi2sd_r32_controls"))?;
    if Some(controls) != fixture.cvtsi2sd_r64_controls {
        return Err(FormatError::CaseMismatch("cvtsi2sd control echo"));
    }
    let initial_sse = fixture
        .initial_state
        .sse
        .as_ref()
        .ok_or(FormatError::CaseMismatch("state.sse"))?;
    let mxcsr = initial_sse.mxcsr.get() as u32;
    let (bits, inexact) = if wide {
        independent_i64_to_binary64(
            fixture.initial_state.gpr[usize::from(source)].get() as i64,
            (mxcsr >> 13) & 3,
        )
    } else {
        (
            independent_i32_to_binary64_bits(
                fixture.initial_state.gpr[usize::from(source)].get() as u32 as i32
            ),
            false,
        )
    };
    let prerequisite = if !controls.sse2_available || controls.cr0_em || !controls.cr4_osfxsr {
        Some(6)
    } else if controls.cr0_ts {
        Some(7)
    } else {
        None
    };
    let unmasked_precision = wide && prerequisite.is_none() && inexact && mxcsr & (1 << 12) == 0;
    let post_fault = if unmasked_precision {
        Some(if controls.cr4_osxmmexcpt { 19 } else { 6 })
    } else {
        None
    };
    let fault = prerequisite.or(post_fault);
    if !matches!(
        (fault.is_some(), fixture.expected_exit),
        (true, ExpectedExit::Exception) | (false, ExpectedExit::StopAddress)
    ) {
        return Err(FormatError::CaseMismatch(
            "CVTSI2SD register-space expected exit",
        ));
    }
    let mut expected_state = fixture.initial_state.clone();
    if prerequisite.is_none() {
        let sse = expected_state
            .sse
            .as_mut()
            .ok_or(FormatError::CaseMismatch("state.sse"))?;
        if wide {
            sse.mxcsr = HexU64(u64::from(mxcsr | (u32::from(inexact) << 5)));
        }
        if post_fault.is_none() {
            let mut xmm = decode_bytes(&sse.xmm[usize::from(destination)])?;
            xmm[..8].copy_from_slice(&bits.to_le_bytes());
            sse.xmm[usize::from(destination)] = crate::memory::encode_bytes(&xmm);
            expected_state.rip = HexU64(
                fixture
                    .initial_state
                    .rip
                    .get()
                    .checked_add(u64::from(length))
                    .ok_or(FormatError::CaseMismatch("CVTSI2SD RIP overflow"))?,
            );
        }
    }
    let expected_memory = fixture
        .memory
        .iter()
        .map(|region| MemoryRegionSnapshot {
            region_id: region.region_id.clone(),
            guest_address: region.guest_address,
            bytes_hex: region.initial_bytes_hex.clone(),
        })
        .collect::<Vec<_>>();
    if snapshot.state != expected_state
        || snapshot.rflags_defined != HexU64(INTEGER_FLAGS)
        || snapshot.memory != expected_memory
    {
        return Err(FormatError::CaseMismatch(
            "CVTSI2SD register-space output state",
        ));
    }
    if result.instructions_executed
        != (Observation::Available {
            value: HexU64(u64::from(fault.is_none())),
        })
    {
        return Err(FormatError::CaseMismatch(
            "CVTSI2SD register-space retirement count",
        ));
    }
    match (fault, &result.outcome, &snapshot.exception) {
        (
            Some(vector),
            Outcome::Exception {},
            Observation::Available {
                value: Some(exception),
            },
        ) if exception.vector == vector
            && exception.error_code
                == Observation::Unavailable {
                    reason: "exception has no architectural error code".into(),
                }
            && exception.faulting_rip == fixture.initial_state.rip
            && matches!(exception.fault_address, Observation::Unavailable { .. }) => {}
        (None, Outcome::StopAddress {}, Observation::Available { value: None }) => {}
        _ => {
            return Err(FormatError::CaseMismatch(
                "CVTSI2SD register-space outcome/exception",
            ));
        }
    }
    Ok(())
}

fn validate_cvtsi2ss_registers_result(
    loaded: &LoadedFixture,
    result: &RunResult,
    snapshot: &Snapshot,
) -> Result<(), FormatError> {
    let fixture = &loaded.fixture;
    let code = fixture
        .memory
        .iter()
        .find(|region| region.region_id == "code")
        .ok_or(FormatError::CaseMismatch("memory.code"))?;
    let (wide, destination, source, length) =
        cvtsi2ss_register_encoding(&code.initial_bytes_hex)
            .ok_or(FormatError::CaseMismatch("CVTSI2SS encoding"))?;
    let controls = fixture
        .cvtsi2ss_controls
        .ok_or(FormatError::CaseMismatch("cvtsi2ss_controls"))?;
    let initial_sse = fixture
        .initial_state
        .sse
        .as_ref()
        .ok_or(FormatError::CaseMismatch("state.sse"))?;
    let mxcsr = initial_sse.mxcsr.get() as u32;
    let integer = if wide {
        fixture.initial_state.gpr[usize::from(source)].get() as i64
    } else {
        i64::from(fixture.initial_state.gpr[usize::from(source)].get() as u32 as i32)
    };
    let (bits, inexact) = independent_i64_to_binary32(integer, (mxcsr >> 13) & 3);
    let prerequisite = if !controls.sse_available || controls.cr0_em || !controls.cr4_osfxsr {
        Some(6)
    } else if controls.cr0_ts {
        Some(7)
    } else {
        None
    };
    let unmasked_precision = prerequisite.is_none() && inexact && mxcsr & (1 << 12) == 0;
    let post_fault = if unmasked_precision {
        Some(if controls.cr4_osxmmexcpt { 19 } else { 6 })
    } else {
        None
    };
    let fault = prerequisite.or(post_fault);
    if !matches!(
        (fault.is_some(), fixture.expected_exit),
        (true, ExpectedExit::Exception) | (false, ExpectedExit::StopAddress)
    ) {
        return Err(FormatError::CaseMismatch("CVTSI2SS expected exit"));
    }
    let mut expected_state = fixture.initial_state.clone();
    if prerequisite.is_none() {
        let sse = expected_state
            .sse
            .as_mut()
            .ok_or(FormatError::CaseMismatch("state.sse"))?;
        sse.mxcsr = HexU64(u64::from(mxcsr | (u32::from(inexact) << 5)));
        if post_fault.is_none() {
            let mut xmm = decode_bytes(&sse.xmm[usize::from(destination)])?;
            xmm[..4].copy_from_slice(&bits.to_le_bytes());
            sse.xmm[usize::from(destination)] = crate::memory::encode_bytes(&xmm);
            expected_state.rip = HexU64(
                fixture
                    .initial_state
                    .rip
                    .get()
                    .checked_add(u64::from(length))
                    .ok_or(FormatError::CaseMismatch("CVTSI2SS RIP overflow"))?,
            );
        }
    }
    let expected_memory = fixture
        .memory
        .iter()
        .map(|region| MemoryRegionSnapshot {
            region_id: region.region_id.clone(),
            guest_address: region.guest_address,
            bytes_hex: region.initial_bytes_hex.clone(),
        })
        .collect::<Vec<_>>();
    if snapshot.state != expected_state
        || snapshot.rflags_defined != HexU64(INTEGER_FLAGS)
        || snapshot.memory != expected_memory
    {
        return Err(FormatError::CaseMismatch("CVTSI2SS output state"));
    }
    if result.instructions_executed
        != (Observation::Available {
            value: HexU64(u64::from(fault.is_none())),
        })
    {
        return Err(FormatError::CaseMismatch("CVTSI2SS retirement count"));
    }
    match (fault, &result.outcome, &snapshot.exception) {
        (
            Some(vector),
            Outcome::Exception {},
            Observation::Available {
                value: Some(exception),
            },
        ) if exception.vector == vector
            && exception.error_code
                == Observation::Unavailable {
                    reason: "exception has no architectural error code".into(),
                }
            && exception.faulting_rip == fixture.initial_state.rip
            && matches!(exception.fault_address, Observation::Unavailable { .. }) => {}
        (None, Outcome::StopAddress {}, Observation::Available { value: None }) => {}
        _ => return Err(FormatError::CaseMismatch("CVTSI2SS outcome/exception")),
    }
    Ok(())
}

fn validate_maxpd_unmasked_result(
    loaded: &LoadedFixture,
    result: &RunResult,
    snapshot: &Snapshot,
) -> Result<(), FormatError> {
    let fixture = &loaded.fixture;
    let controls = fixture
        .maxpd_unmasked_controls
        .ok_or(FormatError::CaseMismatch("maxpd_unmasked_controls"))?;
    let initial_sse = fixture
        .initial_state
        .sse
        .as_ref()
        .ok_or(FormatError::CaseMismatch("state.sse"))?;
    let lhs_bytes = decode_bytes(&initial_sse.xmm[0])?;
    let rhs_bytes = decode_bytes(&initial_sse.xmm[1])?;
    let mut expected_xmm0 = lhs_bytes.clone();
    let mut lane_invalid = [false; 2];
    let mut lane_denormal = [false; 2];
    for lane in 0..2 {
        let offset = lane * 8;
        let lhs = u64::from_le_bytes(
            lhs_bytes[offset..offset + 8]
                .try_into()
                .map_err(|_| FormatError::CaseMismatch("state.sse.xmm"))?,
        );
        let rhs = u64::from_le_bytes(
            rhs_bytes[offset..offset + 8]
                .try_into()
                .map_err(|_| FormatError::CaseMismatch("state.sse.xmm"))?,
        );
        lane_invalid[lane] = [lhs, rhs]
            .into_iter()
            .any(|bits| ((bits >> 52) & 0x7ff) == 0x7ff && bits & ((1u64 << 52) - 1) != 0);
        lane_denormal[lane] = [lhs, rhs]
            .into_iter()
            .any(|bits| ((bits >> 52) & 0x7ff) == 0 && bits & ((1u64 << 52) - 1) != 0);
        expected_xmm0[offset..offset + 8]
            .copy_from_slice(&independent_maxpd_result_bits(lhs, rhs).to_le_bytes());
    }
    let mxcsr = initial_sse.mxcsr.get() as u32;
    let any_invalid = lane_invalid.into_iter().any(|flag| flag);
    let any_denormal = lane_denormal.into_iter().any(|flag| flag);
    let im = mxcsr & (1 << 7) != 0;
    let dm = mxcsr & (1 << 8) != 0;
    let unmasked = any_invalid && !im || any_denormal && !dm;
    let masked_de = (0..2).any(|lane| lane_denormal[lane] && !lane_invalid[lane]);
    let reported_de = if unmasked { any_denormal } else { masked_de };
    let gate_vector = if !controls.sse2_available || controls.cr0_em || !controls.cr4_osfxsr {
        Some(6)
    } else if controls.cr0_ts {
        Some(7)
    } else {
        None
    };
    let fault_vector =
        gate_vector.or_else(|| (unmasked).then_some(if controls.cr4_osxmmexcpt { 19 } else { 6 }));
    if !matches!(
        (fault_vector.is_some(), fixture.expected_exit),
        (true, ExpectedExit::Exception) | (false, ExpectedExit::StopAddress)
    ) {
        return Err(FormatError::CaseMismatch("MAXPD expected exit"));
    }
    let mut expected_state = fixture.initial_state.clone();
    if gate_vector.is_none() {
        let sse = expected_state
            .sse
            .as_mut()
            .ok_or(FormatError::CaseMismatch("state.sse"))?;
        sse.mxcsr = HexU64(u64::from(
            mxcsr | u32::from(any_invalid) | (u32::from(reported_de) << 1),
        ));
        if fault_vector.is_none() {
            sse.xmm[0] = crate::memory::encode_bytes(&expected_xmm0);
            expected_state.rip = HexU64(fixture.initial_state.rip.get() + 4);
        }
    }
    if snapshot.state != expected_state
        || snapshot.rflags_defined != HexU64(INTEGER_FLAGS)
        || snapshot.memory
            != fixture
                .memory
                .iter()
                .map(|region| MemoryRegionSnapshot {
                    region_id: region.region_id.clone(),
                    guest_address: region.guest_address,
                    bytes_hex: region.initial_bytes_hex.clone(),
                })
                .collect::<Vec<_>>()
    {
        return Err(FormatError::CaseMismatch("MAXPD output state"));
    }
    let expected_count = if fault_vector.is_some() { 0 } else { 1 };
    if result.instructions_executed
        != (Observation::Available {
            value: HexU64(expected_count),
        })
    {
        return Err(FormatError::CaseMismatch("MAXPD retirement count"));
    }
    match (fault_vector, &result.outcome, &snapshot.exception) {
        (
            Some(vector),
            Outcome::Exception {},
            Observation::Available {
                value: Some(exception),
            },
        ) if exception.vector == vector
            && exception.error_code
                == Observation::Unavailable {
                    reason: "exception has no architectural error code".into(),
                }
            && exception.faulting_rip == fixture.initial_state.rip
            && matches!(exception.fault_address, Observation::Unavailable { .. }) => {}
        (None, Outcome::StopAddress {}, Observation::Available { value: None }) => {}
        _ => return Err(FormatError::CaseMismatch("MAXPD outcome/exception")),
    }
    Ok(())
}

fn independent_maxsd_result_bits(lhs: u64, rhs: u64) -> u64 {
    let is_nan = |bits: u64| (bits >> 52) & 0x7ff == 0x7ff && bits & ((1u64 << 52) - 1) != 0;
    if is_nan(lhs) || is_nan(rhs) {
        return rhs;
    }
    if f64::from_bits(lhs) > f64::from_bits(rhs) {
        lhs
    } else {
        rhs
    }
}

fn validate_maxsd_unmasked_result(
    loaded: &LoadedFixture,
    result: &RunResult,
    snapshot: &Snapshot,
) -> Result<(), FormatError> {
    let fixture = &loaded.fixture;
    let controls = fixture
        .maxsd_unmasked_controls
        .ok_or(FormatError::CaseMismatch("maxsd_unmasked_controls"))?;
    let initial_sse = fixture
        .initial_state
        .sse
        .as_ref()
        .ok_or(FormatError::CaseMismatch("state.sse"))?;
    let lhs_bytes = decode_bytes(&initial_sse.xmm[0])?;
    let rhs_bytes = decode_bytes(&initial_sse.xmm[1])?;
    let lhs_bits = u64::from_le_bytes(
        lhs_bytes[..8]
            .try_into()
            .map_err(|_| FormatError::CaseMismatch("state.sse.xmm[0]"))?,
    );
    let rhs_bits = u64::from_le_bytes(
        rhs_bytes[..8]
            .try_into()
            .map_err(|_| FormatError::CaseMismatch("state.sse.xmm[1]"))?,
    );
    let is_nan = |bits: u64| (bits >> 52) & 0x7ff == 0x7ff && bits & ((1u64 << 52) - 1) != 0;
    let is_subnormal = |bits: u64| (bits >> 52) & 0x7ff == 0 && bits & ((1u64 << 52) - 1) != 0;
    let invalid = is_nan(lhs_bits) || is_nan(rhs_bits);
    let raw_denormal = is_subnormal(lhs_bits) || is_subnormal(rhs_bits);
    let mxcsr = initial_sse.mxcsr.get() as u32;
    let im = mxcsr & (1 << 7) != 0;
    let dm = mxcsr & (1 << 8) != 0;
    let reported_de = raw_denormal && (!invalid || !im || !dm);
    let arithmetic_fault = invalid && !im || raw_denormal && !dm;
    let gate_vector = if !controls.sse2_available || controls.cr0_em || !controls.cr4_osfxsr {
        Some(6)
    } else if controls.cr0_ts {
        Some(7)
    } else {
        None
    };
    let fault_vector = gate_vector
        .or_else(|| arithmetic_fault.then_some(if controls.cr4_osxmmexcpt { 19 } else { 6 }));
    if !matches!(
        (fault_vector.is_some(), fixture.expected_exit),
        (true, ExpectedExit::Exception) | (false, ExpectedExit::StopAddress)
    ) {
        return Err(FormatError::CaseMismatch("MAXSD expected exit"));
    }
    let mut expected_state = fixture.initial_state.clone();
    if gate_vector.is_none() {
        let sse = expected_state
            .sse
            .as_mut()
            .ok_or(FormatError::CaseMismatch("state.sse"))?;
        sse.mxcsr = HexU64(u64::from(
            mxcsr | u32::from(invalid) | (u32::from(reported_de) << 1),
        ));
        if fault_vector.is_none() {
            let result_bits = independent_maxsd_result_bits(lhs_bits, rhs_bits);
            let mut xmm0 = decode_bytes(&sse.xmm[0])?;
            xmm0[..8].copy_from_slice(&result_bits.to_le_bytes());
            sse.xmm[0] = crate::memory::encode_bytes(&xmm0);
            expected_state.rip = HexU64(fixture.initial_state.rip.get() + 4);
        }
    }
    if snapshot.state != expected_state
        || snapshot.rflags_defined != HexU64(INTEGER_FLAGS)
        || snapshot.memory
            != fixture
                .memory
                .iter()
                .map(|region| MemoryRegionSnapshot {
                    region_id: region.region_id.clone(),
                    guest_address: region.guest_address,
                    bytes_hex: region.initial_bytes_hex.clone(),
                })
                .collect::<Vec<_>>()
    {
        return Err(FormatError::CaseMismatch("MAXSD output state"));
    }
    let expected_count = if fault_vector.is_some() { 0 } else { 1 };
    if result.instructions_executed
        != (Observation::Available {
            value: HexU64(expected_count),
        })
    {
        return Err(FormatError::CaseMismatch("MAXSD retirement count"));
    }
    match (fault_vector, &result.outcome, &snapshot.exception) {
        (
            Some(vector),
            Outcome::Exception {},
            Observation::Available {
                value: Some(exception),
            },
        ) if exception.vector == vector
            && exception.error_code
                == Observation::Unavailable {
                    reason: "exception has no architectural error code".into(),
                }
            && exception.faulting_rip == fixture.initial_state.rip
            && matches!(exception.fault_address, Observation::Unavailable { .. }) => {}
        (None, Outcome::StopAddress {}, Observation::Available { value: None }) => {}
        _ => return Err(FormatError::CaseMismatch("MAXSD outcome/exception")),
    }
    Ok(())
}

/// Compare only completed architectural observations; unavailable/unsupported never matches.
pub fn compare(
    loaded: &LoadedFixture,
    left: &RunResult,
    right: &RunResult,
) -> Result<Vec<String>, FormatError> {
    left.validate_for(loaded)?;
    right.validate_for(loaded)?;
    if !left.outcome.architectural() || !right.outcome.architectural() {
        return Err(invalid(
            "outcome",
            "incomplete or unsupported results cannot be compared",
        ));
    }
    let (Observation::Available { value: a }, Observation::Available { value: b }) =
        (&left.snapshot, &right.snapshot)
    else {
        return Err(invalid("snapshot", "unavailable"));
    };
    let mask = loaded.fixture.rflags_mask.get();
    let cr0_mov_retired = loaded.fixture.profile == Profile::LongCr0FpInitV1
        && matches!(
            left.instructions_executed,
            Observation::Available { value } if value.get() > 0
        )
        && matches!(left.snapshot, Observation::Available { .. })
        && loaded.fixture.memory.iter().any(|region| {
            region.region_id == "code"
                && decode_bytes(&region.initial_bytes_hex).is_ok_and(|bytes| {
                    bytes.starts_with(&[0x0f, 0x20, 0xc0]) || bytes.starts_with(&[0x0f, 0x22, 0xc0])
                })
        });
    let arithmetic_flags = INTEGER_FLAGS & !((1 << 10) | 2);
    let defined_mask = if loaded.fixture.profile == Profile::LongCr0FpInitV1 {
        mask & INTEGER_FLAGS
            & if cr0_mov_retired {
                !arithmetic_flags
            } else {
                u64::MAX
            }
    } else if matches!(
        loaded.fixture.profile,
        Profile::LongCr0Cr4FpInitV1
            | Profile::LongCr0Cr4FpInitFxsave64V1
            | Profile::LongCr0Cr4FpInitFxrstor64V1
            | Profile::LongCr0Cr4FpInitFxrstor64MinssV1
    ) {
        mask & INTEGER_FLAGS & !arithmetic_flags
    } else if matches!(
        loaded.fixture.profile,
        Profile::LongCliV1
            | Profile::LongStiV1
            | Profile::LongHltV1
            | Profile::LongIretqCpl0ValidV1
            | Profile::LongLretqCpl0ValidV1
    ) {
        mask & INTEGER_FLAGS
    } else {
        mask
    };
    if a.rflags_defined.get() & defined_mask != defined_mask
        || b.rflags_defined.get() & defined_mask != defined_mask
    {
        return Err(invalid("rflags_defined", "requested flags are not defined"));
    }
    let mut differences = Vec::new();
    for (side, result, snapshot) in [("left", left, a), ("right", right, b)] {
        if matches!(result.outcome, Outcome::StopAddress {})
            && Some(snapshot.state.rip) != loaded.fixture.stop_rip
        {
            differences.push(format!("{side}.stop_rip"));
        }
        if matches!(result.outcome, Outcome::InstructionBudgetExhausted {}) {
            if let Observation::Available { value } = result.instructions_executed {
                if value.get() != loaded.fixture.instruction_budget {
                    differences.push(format!("{side}.budget_count"));
                }
            }
        }
    }
    let expected = loaded.fixture.expected_exit;
    for (side, outcome) in [
        ("left.expected_exit", &left.outcome),
        ("right.expected_exit", &right.outcome),
    ] {
        let matches = matches!(
            (expected, outcome),
            (ExpectedExit::StopAddress, Outcome::StopAddress {})
                | (ExpectedExit::Halted, Outcome::Halted {})
                | (
                    ExpectedExit::InstructionBudgetExhausted,
                    Outcome::InstructionBudgetExhausted {}
                )
                | (
                    ExpectedExit::RepIterationBudgetExhausted,
                    Outcome::RepIterationBudgetExhausted {}
                )
                | (ExpectedExit::StopRequested, Outcome::StopRequested {})
                | (ExpectedExit::Exception, Outcome::Exception {})
                | (ExpectedExit::HarnessError, Outcome::HarnessError { .. })
                | (ExpectedExit::Unsupported, Outcome::Unsupported { .. })
        );
        if !matches {
            differences.push(side.into());
        }
    }
    if left.outcome != right.outcome {
        differences.push("outcome".into());
    }
    for i in 0..16 {
        if a.state.gpr[i] != b.state.gpr[i] {
            differences.push(format!("gpr[{i}]"));
        }
    }
    if a.state.rip != b.state.rip {
        differences.push("rip".into());
    }
    if a.state.cr0 != b.state.cr0 {
        differences.push("cr0".into());
    }
    if a.state.mode != b.state.mode {
        differences.push("mode".into());
    }
    if a.state.fs_base != b.state.fs_base {
        differences.push("fs_base".into());
    }
    if a.state.gs_base != b.state.gs_base {
        differences.push("gs_base".into());
    }
    if a.state.kernel_gs_base != b.state.kernel_gs_base {
        differences.push("kernel_gs_base".into());
    }
    if a.state.fs_selector != b.state.fs_selector {
        differences.push("fs_selector".into());
    }
    if a.state.gs_selector != b.state.gs_selector {
        differences.push("gs_selector".into());
    }
    if a.state.gdtr_base != b.state.gdtr_base {
        differences.push("gdtr_base".into());
    }
    if a.state.gdtr_limit != b.state.gdtr_limit {
        differences.push("gdtr_limit".into());
    }
    for (name, left, right) in [
        ("cs_selector", a.state.cs_selector, b.state.cs_selector),
        (
            "cs_cache_base",
            a.state.cs_cache_base,
            b.state.cs_cache_base,
        ),
        (
            "cs_cache_limit",
            a.state.cs_cache_limit,
            b.state.cs_cache_limit,
        ),
        (
            "cs_cache_access",
            a.state.cs_cache_access,
            b.state.cs_cache_access,
        ),
        (
            "cs_cache_flags",
            a.state.cs_cache_flags,
            b.state.cs_cache_flags,
        ),
        ("ss_selector", a.state.ss_selector, b.state.ss_selector),
        (
            "ss_cache_base",
            a.state.ss_cache_base,
            b.state.ss_cache_base,
        ),
        (
            "ss_cache_limit",
            a.state.ss_cache_limit,
            b.state.ss_cache_limit,
        ),
        (
            "ss_cache_access",
            a.state.ss_cache_access,
            b.state.ss_cache_access,
        ),
        (
            "ss_cache_flags",
            a.state.ss_cache_flags,
            b.state.ss_cache_flags,
        ),
    ] {
        if left != right {
            differences.push(name.into());
        }
    }
    if a.state.fs_cache_limit != b.state.fs_cache_limit {
        differences.push("fs_cache_limit".into());
    }
    if a.state.fs_cache_access != b.state.fs_cache_access {
        differences.push("fs_cache_access".into());
    }
    if a.state.fs_cache_flags != b.state.fs_cache_flags {
        differences.push("fs_cache_flags".into());
    }
    if a.state.gs_cache_limit != b.state.gs_cache_limit {
        differences.push("gs_cache_limit".into());
    }
    if a.state.gs_cache_access != b.state.gs_cache_access {
        differences.push("gs_cache_access".into());
    }
    if a.state.gs_cache_flags != b.state.gs_cache_flags {
        differences.push("gs_cache_flags".into());
    }
    if a.state.x87 != b.state.x87 {
        differences.push("x87".into());
    }
    if a.state.sse != b.state.sse {
        differences.push("sse".into());
    }
    if a.state.sti_shadow != b.state.sti_shadow {
        differences.push("state.sti_shadow".into());
    }
    if a.state.activity != b.state.activity {
        differences.push("state.activity".into());
    }
    if a.xorpd_prerequisites != b.xorpd_prerequisites {
        differences.push("xorpd_prerequisites".into());
    }
    if a.xorpd_prerequisites != loaded.fixture.xorpd_prerequisites {
        differences.push("left.xorpd_prerequisites".into());
    }
    if b.xorpd_prerequisites != loaded.fixture.xorpd_prerequisites {
        differences.push("right.xorpd_prerequisites".into());
    }
    if a.fxsave64_cr0_controls != b.fxsave64_cr0_controls {
        differences.push("fxsave64_cr0_controls".into());
    }
    if a.fxsave64_cr0_controls != loaded.fixture.fxsave64_cr0_controls {
        differences.push("left.fxsave64_cr0_controls".into());
    }
    if b.fxsave64_cr0_controls != loaded.fixture.fxsave64_cr0_controls {
        differences.push("right.fxsave64_cr0_controls".into());
    }
    if a.fxrstor64_cr0_controls != b.fxrstor64_cr0_controls {
        differences.push("fxrstor64_cr0_controls".into());
    }
    if a.fxrstor64_cr0_controls != loaded.fixture.fxrstor64_cr0_controls {
        differences.push("left.fxrstor64_cr0_controls".into());
    }
    if b.fxrstor64_cr0_controls != loaded.fixture.fxrstor64_cr0_controls {
        differences.push("right.fxrstor64_cr0_controls".into());
    }
    if a.minss_invalid_controls != b.minss_invalid_controls {
        differences.push("minss_invalid_controls".into());
    }
    if a.maxss_unmasked_controls != b.maxss_unmasked_controls {
        differences.push("maxss_unmasked_controls".into());
    }
    if a.minsd_unmasked_controls != b.minsd_unmasked_controls {
        differences.push("minsd_unmasked_controls".into());
    }
    if a.maxsd_unmasked_controls != b.maxsd_unmasked_controls {
        differences.push("maxsd_unmasked_controls".into());
    }
    if a.maxsd_unmasked_controls != loaded.fixture.maxsd_unmasked_controls {
        differences.push("left.maxsd_unmasked_controls".into());
    }
    if b.maxsd_unmasked_controls != loaded.fixture.maxsd_unmasked_controls {
        differences.push("right.maxsd_unmasked_controls".into());
    }
    if a.minpd_unmasked_controls != b.minpd_unmasked_controls {
        differences.push("minpd_unmasked_controls".into());
    }
    if a.maxps_unmasked_controls != b.maxps_unmasked_controls {
        differences.push("maxps_unmasked_controls".into());
    }
    if a.maxps_unmasked_controls != loaded.fixture.maxps_unmasked_controls {
        differences.push("left.maxps_unmasked_controls".into());
    }
    if b.maxps_unmasked_controls != loaded.fixture.maxps_unmasked_controls {
        differences.push("right.maxps_unmasked_controls".into());
    }
    if a.minps_unmasked_controls != b.minps_unmasked_controls {
        differences.push("minps_unmasked_controls".into());
    }
    if a.minps_unmasked_controls != loaded.fixture.minps_unmasked_controls {
        differences.push("left.minps_unmasked_controls".into());
    }
    if b.minps_unmasked_controls != loaded.fixture.minps_unmasked_controls {
        differences.push("right.minps_unmasked_controls".into());
    }
    if a.minpd_unmasked_controls != loaded.fixture.minpd_unmasked_controls {
        differences.push("left.minpd_unmasked_controls".into());
    }
    if b.minpd_unmasked_controls != loaded.fixture.minpd_unmasked_controls {
        differences.push("right.minpd_unmasked_controls".into());
    }
    if a.maxpd_unmasked_controls != b.maxpd_unmasked_controls {
        differences.push("maxpd_unmasked_controls".into());
    }
    if a.maxpd_unmasked_controls != loaded.fixture.maxpd_unmasked_controls {
        differences.push("left.maxpd_unmasked_controls".into());
    }
    if b.maxpd_unmasked_controls != loaded.fixture.maxpd_unmasked_controls {
        differences.push("right.maxpd_unmasked_controls".into());
    }
    if a.cvtdq2ps_controls != b.cvtdq2ps_controls {
        differences.push("cvtdq2ps_controls".into());
    }
    if a.cvtdq2ps_controls != loaded.fixture.cvtdq2ps_controls {
        differences.push("left.cvtdq2ps_controls".into());
    }
    if b.cvtdq2ps_controls != loaded.fixture.cvtdq2ps_controls {
        differences.push("right.cvtdq2ps_controls".into());
    }
    if a.cvtps2pd_controls != b.cvtps2pd_controls {
        differences.push("cvtps2pd_controls".into());
    }
    if a.cvtss2sd_controls != b.cvtss2sd_controls {
        differences.push("cvtss2sd_controls".into());
    }
    if a.cvtps2pd_controls != loaded.fixture.cvtps2pd_controls {
        differences.push("left.cvtps2pd_controls".into());
    }
    if b.cvtps2pd_controls != loaded.fixture.cvtps2pd_controls {
        differences.push("right.cvtps2pd_controls".into());
    }
    if a.cvtss2sd_controls != loaded.fixture.cvtss2sd_controls {
        differences.push("left.cvtss2sd_controls".into());
    }
    if b.cvtss2sd_controls != loaded.fixture.cvtss2sd_controls {
        differences.push("right.cvtss2sd_controls".into());
    }
    if a.cvtdq2pd_controls != b.cvtdq2pd_controls {
        differences.push("cvtdq2pd_controls".into());
    }
    if a.cvtdq2pd_controls != loaded.fixture.cvtdq2pd_controls {
        differences.push("left.cvtdq2pd_controls".into());
    }
    if b.cvtdq2pd_controls != loaded.fixture.cvtdq2pd_controls {
        differences.push("right.cvtdq2pd_controls".into());
    }
    if a.minsd_unmasked_controls != loaded.fixture.minsd_unmasked_controls {
        differences.push("left.minsd_unmasked_controls".into());
    }
    if b.minsd_unmasked_controls != loaded.fixture.minsd_unmasked_controls {
        differences.push("right.minsd_unmasked_controls".into());
    }
    if a.maxss_unmasked_controls != loaded.fixture.maxss_unmasked_controls {
        differences.push("left.maxss_unmasked_controls".into());
    }
    if b.maxss_unmasked_controls != loaded.fixture.maxss_unmasked_controls {
        differences.push("right.maxss_unmasked_controls".into());
    }
    if a.minss_invalid_controls != loaded.fixture.minss_invalid_controls {
        differences.push("left.minss_invalid_controls".into());
    }
    if b.minss_invalid_controls != loaded.fixture.minss_invalid_controls {
        differences.push("right.minss_invalid_controls".into());
    }
    if rflags_differ(
        loaded.fixture.profile,
        mask,
        a.state.rflags.get(),
        b.state.rflags.get(),
    ) {
        differences.push("rflags".into());
    }
    if a.rflags_defined != b.rflags_defined {
        differences.push("rflags_defined".into());
    }
    let same_fault_field = |left: &Observation<HexU64>, right: &Observation<HexU64>| {
        match (left, right) {
            (Observation::Available { value: x }, Observation::Available { value: y }) => x == y,
            // Reasons describe the harness; required architectural fields were checked above.
            (Observation::Unavailable { .. }, Observation::Unavailable { .. }) => true,
            _ => false,
        }
    };
    let same_exception = match (&a.exception, &b.exception) {
        (Observation::Available { value: None }, Observation::Available { value: None }) => true,
        (Observation::Available { value: Some(x) }, Observation::Available { value: Some(y) }) => {
            x.vector == y.vector
                && x.faulting_rip == y.faulting_rip
                && same_fault_field(&x.error_code, &y.error_code)
                && same_fault_field(&x.fault_address, &y.fault_address)
        }
        _ => false,
    };
    if !same_exception {
        differences.push("exception".into());
    }
    // Hardware retired count can be unavailable; it must not substitute a guessed value.
    if let (Observation::Available { value: x }, Observation::Available { value: y }) =
        (&left.instructions_executed, &right.instructions_executed)
    {
        if x != y {
            differences.push("instructions_executed".into());
        }
    }
    for region in &loaded.fixture.memory {
        let x = a
            .memory
            .iter()
            .find(|r| r.region_id == region.region_id)
            .unwrap();
        let y = b
            .memory
            .iter()
            .find(|r| r.region_id == region.region_id)
            .unwrap();
        let x = decode_bytes(&x.bytes_hex)?;
        let y = decode_bytes(&y.bytes_hex)?;
        let masks = decode_bytes(&region.comparison_mask_hex)?;
        for (i, ((x, y), mask)) in x.into_iter().zip(y).zip(masks).enumerate() {
            if (x ^ y) & mask != 0 {
                differences.push(format!("memory.{}[{i}]", region.region_id));
            }
        }
    }
    Ok(differences)
}

fn rflags_differ(profile: Profile, mask: u64, left: u64, right: u64) -> bool {
    let allowed = if matches!(
        profile,
        Profile::LongCliV1
            | Profile::LongStiV1
            | Profile::LongHltV1
            | Profile::LongIretqCpl0ValidV1
            | Profile::LongLretqCpl0ValidV1
            | Profile::LongCr0FpInitV1
            | Profile::LongCr4FpInitV1
            | Profile::LongCr0Cr4FpInitFxsave64V1
            | Profile::LongCr0Cr4FpInitFxrstor64V1
            | Profile::LongCr0Cr4FpInitFxrstor64MinssV1
            | Profile::LongCr0Cr4FpInitV1,
    ) {
        INTEGER_FLAGS | (1 << 9) | (3 << 12)
    } else {
        INTEGER_FLAGS
    };
    debug_assert_eq!(mask & !allowed, 0, "fixture validation gates RFLAGS masks");
    (left ^ right) & mask != 0
}

pub fn decode_bytes(hex: &str) -> Result<Vec<u8>, FormatError> {
    if hex.len() / 2 > crate::MAX_MEMORY_BYTES {
        return Err(invalid("bytes_hex", "exceeds memory limit"));
    }
    validate_hex_bytes(hex, "bytes_hex")?;
    (0..hex.len())
        .step_by(2)
        .map(|i| {
            u8::from_str_radix(&hex[i..i + 2], 16).map_err(|e| invalid("bytes_hex", &e.to_string()))
        })
        .collect()
}

fn load<T: DeserializeOwned>(bytes: &[u8], limit: usize) -> Result<T, FormatError> {
    if bytes.len() > limit {
        return Err(FormatError::TooLarge {
            kind: "v2 JSON",
            max_bytes: limit,
        });
    }
    serde_json::from_slice(bytes).map_err(|e| FormatError::Json(e.to_string()))
}

fn save<T: Serialize>(value: &T, limit: usize) -> Result<Vec<u8>, FormatError> {
    let bytes = serde_json::to_vec_pretty(value).map_err(|e| FormatError::Json(e.to_string()))?;
    if bytes.len() > limit {
        return Err(FormatError::TooLarge {
            kind: "v2 JSON",
            max_bytes: limit,
        });
    }
    Ok(bytes)
}

fn version(value: u32) -> Result<(), FormatError> {
    if value == SCHEMA_VERSION {
        Ok(())
    } else {
        Err(FormatError::UnsupportedSchema(value))
    }
}

fn text(value: &str, field: &'static str) -> Result<(), FormatError> {
    if value.trim().is_empty() || value.len() > 512 {
        Err(invalid(field, "must be nonempty and at most 512 bytes"))
    } else {
        Ok(())
    }
}

fn digest_field(value: &str) -> Result<(), FormatError> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err(invalid("sha256", "expected 64 lowercase hex digits"));
    }
    Ok(())
}

fn invalid(field: &'static str, reason: &str) -> FormatError {
    FormatError::InvalidField {
        field,
        reason: reason.into(),
    }
}

/// Exact DB/DF indexed register bytes, optionally followed by FNSTSW AX.
/// Returns the encoded ST(i) source index for the bounded FCOMI profile.
fn fcomi_family_source(code_bytes: &str) -> Option<u8> {
    if !(code_bytes.len() == 4 || (code_bytes.len() == 8 && code_bytes.ends_with("dfe0"))) {
        return None;
    }
    let opcode = u8::from_str_radix(&code_bytes[0..2], 16).ok()?;
    let modrm = u8::from_str_radix(&code_bytes[2..4], 16).ok()?;
    if !matches!(opcode, 0xdb | 0xdf) {
        return None;
    }
    matches!(modrm, 0xe8..=0xef | 0xf0..=0xf7).then_some(modrm & 7)
}

/// Exact indexed DA C0+i bytes, optionally followed by FNSTSW AX.
fn fcmovb_family_source(code_bytes: &str) -> Option<u8> {
    if !(code_bytes.len() == 4 || (code_bytes.len() == 8 && code_bytes.ends_with("dfe0"))) {
        return None;
    }
    let opcode = u8::from_str_radix(&code_bytes[0..2], 16).ok()?;
    let modrm = u8::from_str_radix(&code_bytes[2..4], 16).ok()?;
    (opcode == 0xda && (0xc0..=0xc7).contains(&modrm)).then_some(modrm & 7)
}

/// Exact indexed DB C0+i bytes, optionally followed by FNSTSW AX.
fn fcmovnb_family_source(code_bytes: &str) -> Option<u8> {
    if !(code_bytes.len() == 4 || (code_bytes.len() == 8 && code_bytes.ends_with("dfe0"))) {
        return None;
    }
    let opcode = u8::from_str_radix(&code_bytes[0..2], 16).ok()?;
    let modrm = u8::from_str_radix(&code_bytes[2..4], 16).ok()?;
    (opcode == 0xdb && (0xc0..=0xc7).contains(&modrm)).then_some(modrm & 7)
}

/// Exact indexed DA C8+i bytes, optionally followed by FNSTSW AX.
fn fcmove_family_source(code_bytes: &str) -> Option<u8> {
    if !(code_bytes.len() == 4 || (code_bytes.len() == 8 && code_bytes.ends_with("dfe0"))) {
        return None;
    }
    let opcode = u8::from_str_radix(&code_bytes[0..2], 16).ok()?;
    let modrm = u8::from_str_radix(&code_bytes[2..4], 16).ok()?;
    (opcode == 0xda && (0xc8..=0xcf).contains(&modrm)).then_some(modrm & 7)
}

/// Exact bare DB C8+i FCMOVNE ST(0), ST(i), optionally followed by FNSTSW AX.
fn fcmovne_family_source(code_bytes: &str) -> Option<u8> {
    if !(code_bytes.len() == 4 || (code_bytes.len() == 8 && code_bytes.ends_with("dfe0"))) {
        return None;
    }
    let opcode = u8::from_str_radix(&code_bytes[0..2], 16).ok()?;
    let modrm = u8::from_str_radix(&code_bytes[2..4], 16).ok()?;
    (opcode == 0xdb && (0xc8..=0xcf).contains(&modrm)).then_some(modrm & 7)
}

/// Exact twenty-stream S2-30cd FCMOVBE schema, including the S2-30cc producers.
fn fcmovbe_stream(code_bytes: &str) -> bool {
    if code_bytes.len() == 4 || (code_bytes.len() == 8 && code_bytes.ends_with("dfe0")) {
        return fcmovbe_bare_source(&code_bytes[..4]).is_some();
    }
    matches!(
        code_bytes,
        "dad1dad1dfe0"
            | "d9e8d9e0d9e8dbf1dad1dfe0"
            | "d9e8d9e8dbf1d9e0dad1dfe0"
            | "d9e8d9e8d9e0dbf1dad1dfe0"
    )
}

fn fcmovbe_bare_source(code_bytes: &str) -> Option<u8> {
    if code_bytes.len() != 4 {
        return None;
    }
    let opcode = u8::from_str_radix(&code_bytes[0..2], 16).ok()?;
    let modrm = u8::from_str_radix(&code_bytes[2..4], 16).ok()?;
    (opcode == 0xda && (0xd0..=0xd7).contains(&modrm)).then_some(modrm & 7)
}

/// Exact twenty-stream S2-30cf FCMOVNBE ST(i) schema, retaining the producers.
fn fcmovnbe_stream(code_bytes: &str) -> bool {
    if code_bytes.len() == 4 || (code_bytes.len() == 8 && code_bytes.ends_with("dfe0")) {
        return fcmovnbe_bare_source(&code_bytes[..4]).is_some();
    }
    matches!(
        code_bytes,
        "dbd1dbd1dfe0"
            | "d9e8d9e0d9e8dbf1dbd1dfe0"
            | "d9e8d9e8dbf1d9e0dbd1dfe0"
            | "d9e8d9e8d9e0dbf1dbd1dfe0"
    )
}

fn fcmovnbe_bare_source(code_bytes: &str) -> Option<u8> {
    if code_bytes.len() != 4 {
        return None;
    }
    let opcode = u8::from_str_radix(&code_bytes[0..2], 16).ok()?;
    let modrm = u8::from_str_radix(&code_bytes[2..4], 16).ok()?;
    (opcode == 0xdb && (0xd0..=0xd7).contains(&modrm)).then_some(modrm & 7)
}

/// Exact S2-30ci FCMOVNU bare, status, repeat, and TEST producer streams.
fn fcmovnu_stream(code_bytes: &str) -> bool {
    fcmovnu_budget(code_bytes).is_some()
}

fn fcmovnu_budget(code_bytes: &str) -> Option<u64> {
    if code_bytes.len() == 4 {
        return fcmovnu_bare_source(code_bytes).map(|_| 1);
    }
    if code_bytes.len() == 8 && code_bytes.ends_with("dfe0") {
        return fcmovnu_bare_source(&code_bytes[..4]).map(|_| 2);
    }
    match code_bytes {
        "dbd9dbd9dfe0" | "d9e8d9ee4885c0dbd9dfe0" => {
            Some(if code_bytes.starts_with("dbd9") { 3 } else { 5 })
        }
        _ => None,
    }
}

fn fcmovnu_bare_source(code_bytes: &str) -> Option<u8> {
    if code_bytes.len() != 4 {
        return None;
    }
    let opcode = u8::from_str_radix(&code_bytes[0..2], 16).ok()?;
    let modrm = u8::from_str_radix(&code_bytes[2..4], 16).ok()?;
    (opcode == 0xdb && (0xd8..=0xdf).contains(&modrm)).then_some(modrm & 7)
}

fn fcmovu_bare_source(code_bytes: &str) -> Option<u8> {
    if code_bytes.len() != 4 {
        return None;
    }
    let opcode = u8::from_str_radix(&code_bytes[0..2], 16).ok()?;
    let modrm = u8::from_str_radix(&code_bytes[2..4], 16).ok()?;
    (opcode == 0xda && (0xd8..=0xdf).contains(&modrm)).then_some(modrm & 7)
}

fn xorps_register_length(code_bytes: &str) -> Option<u32> {
    if code_bytes.len() % 2 != 0 {
        return None;
    }
    let bytes = (0..code_bytes.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&code_bytes[index..index + 2], 16).ok())
        .collect::<Option<Vec<_>>>()?;
    match bytes.as_slice() {
        [0x0f, 0x57, modrm] if modrm >> 6 == 3 => Some(3),
        [rex, 0x0f, 0x57, modrm]
            if (0x40..=0x4f).contains(rex) && rex & 0x0a == 0 && modrm >> 6 == 3 =>
        {
            Some(4)
        }
        _ => None,
    }
}

fn xorpd_register_length(code_bytes: &str) -> Option<u32> {
    if code_bytes.len() % 2 != 0 {
        return None;
    }
    let bytes = (0..code_bytes.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&code_bytes[index..index + 2], 16).ok())
        .collect::<Option<Vec<_>>>()?;
    match bytes.as_slice() {
        [0x66, 0x0f, 0x57, modrm] if modrm >> 6 == 3 => Some(4),
        [0x66, rex, 0x0f, 0x57, modrm]
            if (0x40..=0x4f).contains(rex) && rex & 0x0a == 0 && modrm >> 6 == 3 =>
        {
            Some(5)
        }
        _ => None,
    }
}

fn andpd_register_length(code_bytes: &str) -> Option<u32> {
    if code_bytes.len() % 2 != 0 {
        return None;
    }
    let bytes = (0..code_bytes.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&code_bytes[index..index + 2], 16).ok())
        .collect::<Option<Vec<_>>>()?;
    match bytes.as_slice() {
        [0x66, 0x0f, 0x54, modrm] if modrm >> 6 == 3 => Some(4),
        [0x66, rex, 0x0f, 0x54, modrm]
            if (0x40..=0x4f).contains(rex) && rex & 0x0a == 0 && modrm >> 6 == 3 =>
        {
            Some(5)
        }
        _ => None,
    }
}

fn andnpd_register_length(code_bytes: &str) -> Option<u32> {
    if code_bytes.len() % 2 != 0 {
        return None;
    }
    let bytes = (0..code_bytes.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&code_bytes[index..index + 2], 16).ok())
        .collect::<Option<Vec<_>>>()?;
    match bytes.as_slice() {
        [0x66, 0x0f, 0x55, modrm] if modrm >> 6 == 3 => Some(4),
        [0x66, rex, 0x0f, 0x55, modrm]
            if (0x40..=0x4f).contains(rex) && rex & 0x0a == 0 && modrm >> 6 == 3 =>
        {
            Some(5)
        }
        _ => None,
    }
}

fn orpd_register_length(code_bytes: &str) -> Option<u32> {
    if code_bytes.len() % 2 != 0 {
        return None;
    }
    let bytes = (0..code_bytes.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&code_bytes[index..index + 2], 16).ok())
        .collect::<Option<Vec<_>>>()?;
    match bytes.as_slice() {
        [0x66, 0x0f, 0x56, modrm] if modrm >> 6 == 3 => Some(4),
        [0x66, rex, 0x0f, 0x56, modrm]
            if (0x40..=0x4f).contains(rex) && rex & 0x0a == 0 && modrm >> 6 == 3 =>
        {
            Some(5)
        }
        _ => None,
    }
}

fn pxor_register_length(code_bytes: &str) -> Option<u32> {
    if code_bytes.len() % 2 != 0 {
        return None;
    }
    let bytes = (0..code_bytes.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&code_bytes[index..index + 2], 16).ok())
        .collect::<Option<Vec<_>>>()?;
    match bytes.as_slice() {
        [0x66, 0x0f, 0xef, modrm] if modrm >> 6 == 3 => Some(4),
        [0x66, rex, 0x0f, 0xef, modrm]
            if (0x40..=0x4f).contains(rex) && rex & 0x0a == 0 && modrm >> 6 == 3 =>
        {
            Some(5)
        }
        _ => None,
    }
}

fn pand_register_length(code_bytes: &str) -> Option<u32> {
    if code_bytes.len() % 2 != 0 {
        return None;
    }
    let bytes = (0..code_bytes.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&code_bytes[index..index + 2], 16).ok())
        .collect::<Option<Vec<_>>>()?;
    match bytes.as_slice() {
        [0x66, 0x0f, 0xdb, modrm] if modrm >> 6 == 3 => Some(4),
        [0x66, rex, 0x0f, 0xdb, modrm]
            if (0x40..=0x4f).contains(rex) && rex & 0x0a == 0 && modrm >> 6 == 3 =>
        {
            Some(5)
        }
        _ => None,
    }
}

fn pandn_register_length(code_bytes: &str) -> Option<u32> {
    if code_bytes.len() % 2 != 0 {
        return None;
    }
    let bytes = (0..code_bytes.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&code_bytes[index..index + 2], 16).ok())
        .collect::<Option<Vec<_>>>()?;
    match bytes.as_slice() {
        [0x66, 0x0f, 0xdf, modrm] if modrm >> 6 == 3 => Some(4),
        [0x66, rex, 0x0f, 0xdf, modrm]
            if (0x40..=0x4f).contains(rex) && rex & 0x0a == 0 && modrm >> 6 == 3 =>
        {
            Some(5)
        }
        _ => None,
    }
}

fn por_register_length(code_bytes: &str) -> Option<u32> {
    if code_bytes.len() % 2 != 0 {
        return None;
    }
    let bytes = (0..code_bytes.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&code_bytes[index..index + 2], 16).ok())
        .collect::<Option<Vec<_>>>()?;
    match bytes.as_slice() {
        [0x66, 0x0f, 0xeb, modrm] if modrm >> 6 == 3 => Some(4),
        [0x66, rex, 0x0f, 0xeb, modrm]
            if (0x40..=0x4f).contains(rex) && rex & 0x0a == 0 && modrm >> 6 == 3 =>
        {
            Some(5)
        }
        _ => None,
    }
}

fn pavgb_register_length(code_bytes: &str) -> Option<u32> {
    if code_bytes.len() % 2 != 0 {
        return None;
    }
    let bytes = (0..code_bytes.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&code_bytes[index..index + 2], 16).ok())
        .collect::<Option<Vec<_>>>()?;
    match bytes.as_slice() {
        [0x66, 0x0f, 0xe0, modrm] if modrm >> 6 == 3 => Some(4),
        [0x66, rex, 0x0f, 0xe0, modrm]
            if (0x40..=0x4f).contains(rex) && rex & 0x0a == 0 && modrm >> 6 == 3 =>
        {
            Some(5)
        }
        _ => None,
    }
}

fn pavgw_register_length(code_bytes: &str) -> Option<u32> {
    if code_bytes.len() % 2 != 0 {
        return None;
    }
    let bytes = (0..code_bytes.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&code_bytes[index..index + 2], 16).ok())
        .collect::<Option<Vec<_>>>()?;
    match bytes.as_slice() {
        [0x66, 0x0f, 0xe3, modrm] if modrm >> 6 == 3 => Some(4),
        [0x66, rex, 0x0f, 0xe3, modrm]
            if (0x40..=0x4f).contains(rex) && rex & 0x0a == 0 && modrm >> 6 == 3 =>
        {
            Some(5)
        }
        _ => None,
    }
}

fn pmaxub_register_length(code_bytes: &str) -> Option<u32> {
    if code_bytes.len() % 2 != 0 {
        return None;
    }
    let bytes = (0..code_bytes.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&code_bytes[index..index + 2], 16).ok())
        .collect::<Option<Vec<_>>>()?;
    match bytes.as_slice() {
        [0x66, 0x0f, 0xde, modrm] if modrm >> 6 == 3 => Some(4),
        [0x66, rex, 0x0f, 0xde, modrm]
            if (0x40..=0x4f).contains(rex) && rex & 0x0a == 0 && modrm >> 6 == 3 =>
        {
            Some(5)
        }
        _ => None,
    }
}

fn pminub_register_length(code_bytes: &str) -> Option<u32> {
    if code_bytes.len() % 2 != 0 {
        return None;
    }
    let bytes = (0..code_bytes.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&code_bytes[index..index + 2], 16).ok())
        .collect::<Option<Vec<_>>>()?;
    match bytes.as_slice() {
        [0x66, 0x0f, 0xda, modrm] if modrm >> 6 == 3 => Some(4),
        [0x66, rex, 0x0f, 0xda, modrm]
            if (0x40..=0x4f).contains(rex) && rex & 0x0a == 0 && modrm >> 6 == 3 =>
        {
            Some(5)
        }
        _ => None,
    }
}

fn pmaxsw_register_length(code_bytes: &str) -> Option<u32> {
    if code_bytes.len() % 2 != 0 {
        return None;
    }
    let bytes = (0..code_bytes.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&code_bytes[index..index + 2], 16).ok())
        .collect::<Option<Vec<_>>>()?;
    match bytes.as_slice() {
        [0x66, 0x0f, 0xee, modrm] if modrm >> 6 == 3 => Some(4),
        [0x66, rex, 0x0f, 0xee, modrm]
            if (0x40..=0x4f).contains(rex) && rex & 0x0a == 0 && modrm >> 6 == 3 =>
        {
            Some(5)
        }
        _ => None,
    }
}

fn pminsw_register_length(code_bytes: &str) -> Option<u32> {
    if code_bytes.len() % 2 != 0 {
        return None;
    }
    let bytes = (0..code_bytes.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&code_bytes[index..index + 2], 16).ok())
        .collect::<Option<Vec<_>>>()?;
    match bytes.as_slice() {
        [0x66, 0x0f, 0xea, modrm] if modrm >> 6 == 3 => Some(4),
        [0x66, rex, 0x0f, 0xea, modrm]
            if (0x40..=0x4f).contains(rex) && rex & 0x0a == 0 && modrm >> 6 == 3 =>
        {
            Some(5)
        }
        _ => None,
    }
}

fn pmullw_register_length(code_bytes: &str) -> Option<u32> {
    if code_bytes.len() % 2 != 0 {
        return None;
    }
    let bytes = (0..code_bytes.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&code_bytes[index..index + 2], 16).ok())
        .collect::<Option<Vec<_>>>()?;
    match bytes.as_slice() {
        [0x66, 0x0f, 0xd5, modrm] if modrm >> 6 == 3 => Some(4),
        [0x66, rex, 0x0f, 0xd5, modrm]
            if (0x40..=0x4f).contains(rex) && rex & 0x0a == 0 && modrm >> 6 == 3 =>
        {
            Some(5)
        }
        _ => None,
    }
}

fn pmulhuw_register_length(code_bytes: &str) -> Option<u32> {
    if code_bytes.len() % 2 != 0 {
        return None;
    }
    let bytes = (0..code_bytes.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&code_bytes[index..index + 2], 16).ok())
        .collect::<Option<Vec<_>>>()?;
    match bytes.as_slice() {
        [0x66, 0x0f, 0xe4, modrm] if modrm >> 6 == 3 => Some(4),
        [0x66, rex, 0x0f, 0xe4, modrm]
            if (0x40..=0x4f).contains(rex) && rex & 0x0a == 0 && modrm >> 6 == 3 =>
        {
            Some(5)
        }
        _ => None,
    }
}

fn psadbw_register_length(code_bytes: &str) -> Option<u32> {
    if code_bytes.len() % 2 != 0 {
        return None;
    }
    let bytes = (0..code_bytes.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&code_bytes[index..index + 2], 16).ok())
        .collect::<Option<Vec<_>>>()?;
    match bytes.as_slice() {
        [0x66, 0x0f, 0xf6, modrm] if modrm >> 6 == 3 => Some(4),
        [0x66, rex, 0x0f, 0xf6, modrm]
            if (0x40..=0x4f).contains(rex) && rex & 0x0a == 0 && modrm >> 6 == 3 =>
        {
            Some(5)
        }
        _ => None,
    }
}

fn pmovmskb_xmm_gpr_length(code_bytes: &str) -> Option<u32> {
    if code_bytes.len() % 2 != 0 {
        return None;
    }
    let bytes = (0..code_bytes.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&code_bytes[index..index + 2], 16).ok())
        .collect::<Option<Vec<_>>>()?;
    match bytes.as_slice() {
        [0x66, 0x0f, 0xd7, modrm] if modrm >> 6 == 3 => Some(4),
        [0x66, rex, 0x0f, 0xd7, modrm]
            if (0x40..=0x4f).contains(rex) && rex & 0x0a == 0 && modrm >> 6 == 3 =>
        {
            Some(5)
        }
        _ => None,
    }
}

fn pmuludq_register_length(code_bytes: &str) -> Option<u32> {
    if code_bytes.len() % 2 != 0 {
        return None;
    }
    let bytes = (0..code_bytes.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&code_bytes[index..index + 2], 16).ok())
        .collect::<Option<Vec<_>>>()?;
    match bytes.as_slice() {
        [0x66, 0x0f, 0xf4, modrm] if modrm >> 6 == 3 => Some(4),
        [0x66, rex, 0x0f, 0xf4, modrm]
            if (0x40..=0x4f).contains(rex) && rex & 0x0a == 0 && modrm >> 6 == 3 =>
        {
            Some(5)
        }
        _ => None,
    }
}

fn pmaddwd_register_length(code_bytes: &str) -> Option<u32> {
    if code_bytes.len() % 2 != 0 {
        return None;
    }
    let bytes = (0..code_bytes.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&code_bytes[index..index + 2], 16).ok())
        .collect::<Option<Vec<_>>>()?;
    match bytes.as_slice() {
        [0x66, 0x0f, 0xf5, modrm] if modrm >> 6 == 3 => Some(4),
        [0x66, rex, 0x0f, 0xf5, modrm]
            if (0x40..=0x4f).contains(rex) && rex & 0x0a == 0 && modrm >> 6 == 3 =>
        {
            Some(5)
        }
        _ => None,
    }
}

fn paddb_register_length(code_bytes: &str) -> Option<u32> {
    if code_bytes.len() % 2 != 0 {
        return None;
    }
    let bytes = (0..code_bytes.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&code_bytes[index..index + 2], 16).ok())
        .collect::<Option<Vec<_>>>()?;
    match bytes.as_slice() {
        [0x66, 0x0f, 0xfc, modrm] if modrm >> 6 == 3 => Some(4),
        [0x66, rex, 0x0f, 0xfc, modrm]
            if (0x40..=0x4f).contains(rex) && rex & 0x0a == 0 && modrm >> 6 == 3 =>
        {
            Some(5)
        }
        _ => None,
    }
}

fn paddw_register_length(code_bytes: &str) -> Option<u32> {
    if code_bytes.len() % 2 != 0 {
        return None;
    }
    let bytes = (0..code_bytes.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&code_bytes[index..index + 2], 16).ok())
        .collect::<Option<Vec<_>>>()?;
    match bytes.as_slice() {
        [0x66, 0x0f, 0xfd, modrm] if modrm >> 6 == 3 => Some(4),
        [0x66, rex, 0x0f, 0xfd, modrm]
            if (0x40..=0x4f).contains(rex) && rex & 0x0a == 0 && modrm >> 6 == 3 =>
        {
            Some(5)
        }
        _ => None,
    }
}

fn paddusw_register_length(code_bytes: &str) -> Option<u32> {
    if code_bytes.len() % 2 != 0 {
        return None;
    }
    let bytes = (0..code_bytes.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&code_bytes[index..index + 2], 16).ok())
        .collect::<Option<Vec<_>>>()?;
    match bytes.as_slice() {
        [0x66, 0x0f, 0xdd, modrm] if modrm >> 6 == 3 => Some(4),
        [0x66, rex, 0x0f, 0xdd, modrm]
            if (0x40..=0x4f).contains(rex) && rex & 0x0a == 0 && modrm >> 6 == 3 =>
        {
            Some(5)
        }
        _ => None,
    }
}

fn psubusw_register_length(code_bytes: &str) -> Option<u32> {
    if code_bytes.len() % 2 != 0 {
        return None;
    }
    let bytes = (0..code_bytes.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&code_bytes[index..index + 2], 16).ok())
        .collect::<Option<Vec<_>>>()?;
    match bytes.as_slice() {
        [0x66, 0x0f, 0xd9, modrm] if modrm >> 6 == 3 => Some(4),
        [0x66, rex, 0x0f, 0xd9, modrm]
            if (0x40..=0x4f).contains(rex) && rex & 0x0a == 0 && modrm >> 6 == 3 =>
        {
            Some(5)
        }
        _ => None,
    }
}

fn psubw_register_length(code_bytes: &str) -> Option<u32> {
    if code_bytes.len() % 2 != 0 {
        return None;
    }
    let bytes = (0..code_bytes.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&code_bytes[index..index + 2], 16).ok())
        .collect::<Option<Vec<_>>>()?;
    match bytes.as_slice() {
        [0x66, 0x0f, 0xf9, modrm] if modrm >> 6 == 3 => Some(4),
        [0x66, rex, 0x0f, 0xf9, modrm]
            if (0x40..=0x4f).contains(rex) && rex & 0x0a == 0 && modrm >> 6 == 3 =>
        {
            Some(5)
        }
        _ => None,
    }
}

fn psubb_register_length(code_bytes: &str) -> Option<u32> {
    if code_bytes.len() % 2 != 0 {
        return None;
    }
    let bytes = (0..code_bytes.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&code_bytes[index..index + 2], 16).ok())
        .collect::<Option<Vec<_>>>()?;
    match bytes.as_slice() {
        [0x66, 0x0f, 0xf8, modrm] if modrm >> 6 == 3 => Some(4),
        [0x66, rex, 0x0f, 0xf8, modrm]
            if (0x40..=0x4f).contains(rex) && rex & 0x0a == 0 && modrm >> 6 == 3 =>
        {
            Some(5)
        }
        _ => None,
    }
}

fn paddusb_register_length(code_bytes: &str) -> Option<u32> {
    if code_bytes.len() % 2 != 0 {
        return None;
    }
    let bytes = (0..code_bytes.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&code_bytes[index..index + 2], 16).ok())
        .collect::<Option<Vec<_>>>()?;
    match bytes.as_slice() {
        [0x66, 0x0f, 0xdc, modrm] if modrm >> 6 == 3 => Some(4),
        [0x66, rex, 0x0f, 0xdc, modrm]
            if (0x40..=0x4f).contains(rex) && rex & 0x0a == 0 && modrm >> 6 == 3 =>
        {
            Some(5)
        }
        _ => None,
    }
}

fn paddsb_register_length(code_bytes: &str) -> Option<u32> {
    if code_bytes.len() % 2 != 0 {
        return None;
    }
    let bytes = (0..code_bytes.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&code_bytes[index..index + 2], 16).ok())
        .collect::<Option<Vec<_>>>()?;
    match bytes.as_slice() {
        [0x66, 0x0f, 0xec, modrm] if modrm >> 6 == 3 => Some(4),
        [0x66, rex, 0x0f, 0xec, modrm]
            if (0x40..=0x4f).contains(rex) && rex & 0x0a == 0 && modrm >> 6 == 3 =>
        {
            Some(5)
        }
        _ => None,
    }
}

fn psubsb_register_length(code_bytes: &str) -> Option<u32> {
    if code_bytes.len() % 2 != 0 {
        return None;
    }
    let bytes = (0..code_bytes.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&code_bytes[index..index + 2], 16).ok())
        .collect::<Option<Vec<_>>>()?;
    match bytes.as_slice() {
        [0x66, 0x0f, 0xe8, modrm] if modrm >> 6 == 3 => Some(4),
        [0x66, rex, 0x0f, 0xe8, modrm]
            if (0x40..=0x4f).contains(rex) && rex & 0x0a == 0 && modrm >> 6 == 3 =>
        {
            Some(5)
        }
        _ => None,
    }
}

fn paddsw_register_length(code_bytes: &str) -> Option<u32> {
    if code_bytes.len() % 2 != 0 {
        return None;
    }
    let bytes = (0..code_bytes.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&code_bytes[index..index + 2], 16).ok())
        .collect::<Option<Vec<_>>>()?;
    match bytes.as_slice() {
        [0x66, 0x0f, 0xed, modrm] if modrm >> 6 == 3 => Some(4),
        [0x66, rex, 0x0f, 0xed, modrm]
            if (0x40..=0x4f).contains(rex) && rex & 0x0a == 0 && modrm >> 6 == 3 =>
        {
            Some(5)
        }
        _ => None,
    }
}

fn paddd_register_length(code_bytes: &str) -> Option<u32> {
    if code_bytes.len() % 2 != 0 {
        return None;
    }
    let bytes = (0..code_bytes.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&code_bytes[index..index + 2], 16).ok())
        .collect::<Option<Vec<_>>>()?;
    match bytes.as_slice() {
        [0x66, 0x0f, 0xfe, modrm] if modrm >> 6 == 3 => Some(4),
        [0x66, rex, 0x0f, 0xfe, modrm]
            if (0x40..=0x4f).contains(rex) && rex & 0x0a == 0 && modrm >> 6 == 3 =>
        {
            Some(5)
        }
        _ => None,
    }
}

fn psubd_register_length(code_bytes: &str) -> Option<u32> {
    if code_bytes.len() % 2 != 0 {
        return None;
    }
    let bytes = (0..code_bytes.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&code_bytes[index..index + 2], 16).ok())
        .collect::<Option<Vec<_>>>()?;
    match bytes.as_slice() {
        [0x66, 0x0f, 0xfa, modrm] if modrm >> 6 == 3 => Some(4),
        [0x66, rex, 0x0f, 0xfa, modrm]
            if (0x40..=0x4f).contains(rex) && rex & 0x0a == 0 && modrm >> 6 == 3 =>
        {
            Some(5)
        }
        _ => None,
    }
}

fn psubsw_register_length(code_bytes: &str) -> Option<u32> {
    if code_bytes.len() % 2 != 0 {
        return None;
    }
    let bytes = (0..code_bytes.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&code_bytes[index..index + 2], 16).ok())
        .collect::<Option<Vec<_>>>()?;
    match bytes.as_slice() {
        [0x66, 0x0f, 0xe9, modrm] if modrm >> 6 == 3 => Some(4),
        [0x66, rex, 0x0f, 0xe9, modrm]
            if (0x40..=0x4f).contains(rex) && rex & 0x0a == 0 && modrm >> 6 == 3 =>
        {
            Some(5)
        }
        _ => None,
    }
}

fn punpckhbw_register_length(code_bytes: &str) -> Option<u32> {
    if code_bytes.len() % 2 != 0 {
        return None;
    }
    let bytes = (0..code_bytes.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&code_bytes[index..index + 2], 16).ok())
        .collect::<Option<Vec<_>>>()?;
    match bytes.as_slice() {
        [0x66, 0x0f, 0x68, modrm] if modrm >> 6 == 3 => Some(4),
        [0x66, rex, 0x0f, 0x68, modrm]
            if (0x40..=0x4f).contains(rex) && rex & 0x0a == 0 && modrm >> 6 == 3 =>
        {
            Some(5)
        }
        _ => None,
    }
}

fn punpcklwd_register_length(code_bytes: &str) -> Option<u32> {
    if code_bytes.len() % 2 != 0 {
        return None;
    }
    let bytes = (0..code_bytes.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&code_bytes[index..index + 2], 16).ok())
        .collect::<Option<Vec<_>>>()?;
    match bytes.as_slice() {
        [0x66, 0x0f, 0x61, modrm] if modrm >> 6 == 3 => Some(4),
        [0x66, rex, 0x0f, 0x61, modrm]
            if (0x40..=0x4f).contains(rex) && rex & 0x0a == 0 && modrm >> 6 == 3 =>
        {
            Some(5)
        }
        _ => None,
    }
}

fn punpckhwd_register_length(code_bytes: &str) -> Option<u32> {
    if code_bytes.len() % 2 != 0 {
        return None;
    }
    let bytes = (0..code_bytes.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&code_bytes[index..index + 2], 16).ok())
        .collect::<Option<Vec<_>>>()?;
    match bytes.as_slice() {
        [0x66, 0x0f, 0x69, modrm] if modrm >> 6 == 3 => Some(4),
        [0x66, rex, 0x0f, 0x69, modrm]
            if (0x40..=0x4f).contains(rex) && rex & 0x0a == 0 && modrm >> 6 == 3 =>
        {
            Some(5)
        }
        _ => None,
    }
}

fn punpckldq_register_length(code_bytes: &str) -> Option<u32> {
    if code_bytes.len() % 2 != 0 {
        return None;
    }
    let bytes = (0..code_bytes.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&code_bytes[index..index + 2], 16).ok())
        .collect::<Option<Vec<_>>>()?;
    match bytes.as_slice() {
        [0x66, 0x0f, 0x62, modrm] if modrm >> 6 == 3 => Some(4),
        [0x66, rex, 0x0f, 0x62, modrm]
            if (0x40..=0x4f).contains(rex) && rex & 0x0a == 0 && modrm >> 6 == 3 =>
        {
            Some(5)
        }
        _ => None,
    }
}

fn punpcklqdq_register_length(code_bytes: &str) -> Option<u32> {
    if code_bytes.len() % 2 != 0 {
        return None;
    }
    let bytes = (0..code_bytes.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&code_bytes[index..index + 2], 16).ok())
        .collect::<Option<Vec<_>>>()?;
    match bytes.as_slice() {
        [0x66, 0x0f, 0x6c, modrm] if modrm >> 6 == 3 => Some(4),
        [0x66, rex, 0x0f, 0x6c, modrm]
            if (0x40..=0x4f).contains(rex) && rex & 0x0a == 0 && modrm >> 6 == 3 =>
        {
            Some(5)
        }
        _ => None,
    }
}

fn packsswb_register_length(code_bytes: &str) -> Option<u32> {
    if code_bytes.len() % 2 != 0 {
        return None;
    }
    let bytes = (0..code_bytes.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&code_bytes[index..index + 2], 16).ok())
        .collect::<Option<Vec<_>>>()?;
    match bytes.as_slice() {
        [0x66, 0x0f, 0x63, modrm] if modrm >> 6 == 3 => Some(4),
        [0x66, rex, 0x0f, 0x63, modrm]
            if (0x40..=0x4f).contains(rex) && rex & 0x0a == 0 && modrm >> 6 == 3 =>
        {
            Some(5)
        }
        _ => None,
    }
}
fn packssdw_register_length(code_bytes: &str) -> Option<u32> {
    if code_bytes.len() % 2 != 0 {
        return None;
    }
    let bytes = (0..code_bytes.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&code_bytes[index..index + 2], 16).ok())
        .collect::<Option<Vec<_>>>()?;
    match bytes.as_slice() {
        [0x66, 0x0f, 0x6b, modrm] if modrm >> 6 == 3 => Some(4),
        [0x66, rex, 0x0f, 0x6b, modrm]
            if (0x40..=0x4f).contains(rex) && rex & 0x0a == 0 && modrm >> 6 == 3 =>
        {
            Some(5)
        }
        _ => None,
    }
}

fn pshufd_register_length(code_bytes: &str) -> Option<u32> {
    if code_bytes.len() % 2 != 0 {
        return None;
    }
    let bytes = (0..code_bytes.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&code_bytes[index..index + 2], 16).ok())
        .collect::<Option<Vec<_>>>()?;
    match bytes.as_slice() {
        [0x66, 0x0f, 0x70, modrm, _imm8] if modrm >> 6 == 3 => Some(5),
        [0x66, rex, 0x0f, 0x70, modrm, _imm8]
            if (0x40..=0x4f).contains(rex) && rex & 0x0a == 0 && modrm >> 6 == 3 =>
        {
            Some(6)
        }
        _ => None,
    }
}

fn psllw_immediate_register_length(code_bytes: &str) -> Option<u32> {
    if code_bytes.len() % 2 != 0 {
        return None;
    }
    let bytes = (0..code_bytes.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&code_bytes[index..index + 2], 16).ok())
        .collect::<Option<Vec<_>>>()?;
    match bytes.as_slice() {
        [0x66, 0x0f, 0x71, modrm, _imm8] if modrm >> 6 == 3 && (modrm >> 3) & 7 == 6 => Some(5),
        [0x66, rex, 0x0f, 0x71, modrm, _imm8]
            if *rex == 0x41 && modrm >> 6 == 3 && (modrm >> 3) & 7 == 6 =>
        {
            Some(6)
        }
        _ => None,
    }
}

fn pslld_immediate_register_length(code_bytes: &str) -> Option<u32> {
    if code_bytes.len() % 2 != 0 {
        return None;
    }
    let bytes = (0..code_bytes.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&code_bytes[index..index + 2], 16).ok())
        .collect::<Option<Vec<_>>>()?;
    match bytes.as_slice() {
        [0x66, 0x0f, 0x72, modrm, _imm8] if modrm >> 6 == 3 && (modrm >> 3) & 7 == 6 => Some(5),
        [0x66, rex, 0x0f, 0x72, modrm, _imm8]
            if *rex == 0x41 && modrm >> 6 == 3 && (modrm >> 3) & 7 == 6 =>
        {
            Some(6)
        }
        _ => None,
    }
}

fn psllq_immediate_register_length(code_bytes: &str) -> Option<u32> {
    if code_bytes.len() % 2 != 0 {
        return None;
    }
    let bytes = (0..code_bytes.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&code_bytes[index..index + 2], 16).ok())
        .collect::<Option<Vec<_>>>()?;
    match bytes.as_slice() {
        [0x66, 0x0f, 0x73, modrm, _imm8] if modrm >> 6 == 3 && (modrm >> 3) & 7 == 6 => Some(5),
        [0x66, rex, 0x0f, 0x73, modrm, _imm8]
            if *rex == 0x41 && modrm >> 6 == 3 && (modrm >> 3) & 7 == 6 =>
        {
            Some(6)
        }
        _ => None,
    }
}

fn pslldq_immediate_register_length(code_bytes: &str) -> Option<u32> {
    if code_bytes.len() % 2 != 0 {
        return None;
    }
    let bytes = (0..code_bytes.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&code_bytes[index..index + 2], 16).ok())
        .collect::<Option<Vec<_>>>()?;
    match bytes.as_slice() {
        [0x66, 0x0f, 0x73, modrm, _imm8] if modrm >> 6 == 3 && (modrm >> 3) & 7 == 7 => Some(5),
        [0x66, rex, 0x0f, 0x73, modrm, _imm8]
            if *rex == 0x41 && modrm >> 6 == 3 && (modrm >> 3) & 7 == 7 =>
        {
            Some(6)
        }
        _ => None,
    }
}

fn psrldq_immediate_register_length(code_bytes: &str) -> Option<u32> {
    if code_bytes.len() % 2 != 0 {
        return None;
    }
    let bytes = (0..code_bytes.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&code_bytes[index..index + 2], 16).ok())
        .collect::<Option<Vec<_>>>()?;
    match bytes.as_slice() {
        [0x66, 0x0f, 0x73, modrm, _imm8] if modrm >> 6 == 3 && (modrm >> 3) & 7 == 3 => Some(5),
        [0x66, 0x41, 0x0f, 0x73, modrm, _imm8] if modrm >> 6 == 3 && (modrm >> 3) & 7 == 3 => {
            Some(6)
        }
        _ => None,
    }
}

fn psrlq_immediate_register_length(code_bytes: &str) -> Option<u32> {
    if code_bytes.len() % 2 != 0 {
        return None;
    }
    let bytes = (0..code_bytes.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&code_bytes[index..index + 2], 16).ok())
        .collect::<Option<Vec<_>>>()?;
    match bytes.as_slice() {
        [0x66, 0x0f, 0x73, modrm, _imm8] if modrm >> 6 == 3 && (modrm >> 3) & 7 == 2 => Some(5),
        [0x66, rex, 0x0f, 0x73, modrm, _imm8]
            if *rex == 0x41 && modrm >> 6 == 3 && (modrm >> 3) & 7 == 2 =>
        {
            Some(6)
        }
        _ => None,
    }
}

fn psrld_immediate_register_length(code_bytes: &str) -> Option<u32> {
    if code_bytes.len() % 2 != 0 {
        return None;
    }
    let bytes = (0..code_bytes.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&code_bytes[index..index + 2], 16).ok())
        .collect::<Option<Vec<_>>>()?;
    match bytes.as_slice() {
        [0x66, 0x0f, 0x72, modrm, _imm8] if modrm >> 6 == 3 && (modrm >> 3) & 7 == 2 => Some(5),
        [0x66, rex, 0x0f, 0x72, modrm, _imm8]
            if *rex == 0x41 && modrm >> 6 == 3 && (modrm >> 3) & 7 == 2 =>
        {
            Some(6)
        }
        _ => None,
    }
}

fn psrlw_immediate_register_length(code_bytes: &str) -> Option<u32> {
    if code_bytes.len() % 2 != 0 {
        return None;
    }
    let bytes = (0..code_bytes.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&code_bytes[index..index + 2], 16).ok())
        .collect::<Option<Vec<_>>>()?;
    match bytes.as_slice() {
        [0x66, 0x0f, 0x71, modrm, _imm8] if modrm >> 6 == 3 && (modrm >> 3) & 7 == 2 => Some(5),
        [0x66, rex, 0x0f, 0x71, modrm, _imm8]
            if *rex == 0x41 && modrm >> 6 == 3 && (modrm >> 3) & 7 == 2 =>
        {
            Some(6)
        }
        _ => None,
    }
}

fn psraw_immediate_register_length(code_bytes: &str) -> Option<u32> {
    if code_bytes.len() % 2 != 0 {
        return None;
    }
    let bytes = (0..code_bytes.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&code_bytes[index..index + 2], 16).ok())
        .collect::<Option<Vec<_>>>()?;
    match bytes.as_slice() {
        [0x66, 0x0f, 0x71, modrm, _imm8] if modrm >> 6 == 3 && (modrm >> 3) & 7 == 4 => Some(5),
        [0x66, rex, 0x0f, 0x71, modrm, _imm8]
            if *rex == 0x41 && modrm >> 6 == 3 && (modrm >> 3) & 7 == 4 =>
        {
            Some(6)
        }
        _ => None,
    }
}

fn psrad_immediate_register_length(code_bytes: &str) -> Option<u32> {
    if code_bytes.len() % 2 != 0 {
        return None;
    }
    let bytes = (0..code_bytes.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&code_bytes[index..index + 2], 16).ok())
        .collect::<Option<Vec<_>>>()?;
    match bytes.as_slice() {
        [0x66, 0x0f, 0x72, modrm, _imm8] if modrm >> 6 == 3 && (modrm >> 3) & 7 == 4 => Some(5),
        [0x66, rex, 0x0f, 0x72, modrm, _imm8]
            if *rex == 0x41 && modrm >> 6 == 3 && (modrm >> 3) & 7 == 4 =>
        {
            Some(6)
        }
        _ => None,
    }
}

fn packuswb_register_length(code_bytes: &str) -> Option<u32> {
    if code_bytes.len() % 2 != 0 {
        return None;
    }
    let bytes = (0..code_bytes.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&code_bytes[index..index + 2], 16).ok())
        .collect::<Option<Vec<_>>>()?;
    match bytes.as_slice() {
        [0x66, 0x0f, 0x67, modrm] if modrm >> 6 == 3 => Some(4),
        [0x66, rex, 0x0f, 0x67, modrm]
            if (0x40..=0x4f).contains(rex) && rex & 0x0a == 0 && modrm >> 6 == 3 =>
        {
            Some(5)
        }
        _ => None,
    }
}

fn cvtdq2pd_register_length(code_bytes: &str) -> Option<u32> {
    cvtdq2pd_register_encoding(code_bytes).map(|(_, _, length)| length)
}

/// Independently parse the canonical byte shape for the bounded register profile.
fn cvtdq2pd_register_encoding(code_bytes: &str) -> Option<(u8, u8, u32)> {
    if code_bytes.len() % 2 != 0 {
        return None;
    }
    let bytes = (0..code_bytes.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&code_bytes[index..index + 2], 16).ok())
        .collect::<Option<Vec<_>>>()?;
    let (rex, opcode_offset) = match bytes.as_slice() {
        [0xf3, 0x0f, 0xe6, modrm] if modrm >> 6 == 3 => (0u8, 1usize),
        [0xf3, rex, 0x0f, 0xe6, modrm] if matches!(*rex, 0x41 | 0x44 | 0x45) && modrm >> 6 == 3 => {
            (*rex, 2usize)
        }
        _ => return None,
    };
    let modrm = *bytes.last()?;
    let destination = ((modrm >> 3) & 7) | (((rex >> 2) & 1) << 3);
    let source = (modrm & 7) | ((rex & 1) << 3);
    let needs_rex = destination >= 8 || source >= 8;
    if needs_rex != (opcode_offset == 2)
        || (rex & 4 != 0) != (destination >= 8)
        || (rex & 1 != 0) != (source >= 8)
        || rex & 0x0a != 0
    {
        return None;
    }
    Some((destination, source, bytes.len() as u32))
}

/// Independently parse the canonical Long64 CVTDQ2PS register byte forms.
fn cvtdq2ps_register_encoding(code_bytes: &str) -> Option<(u8, u8, u32)> {
    if code_bytes.len() % 2 != 0 {
        return None;
    }
    let bytes = (0..code_bytes.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&code_bytes[index..index + 2], 16).ok())
        .collect::<Option<Vec<_>>>()?;
    let (rex, modrm) = match bytes.as_slice() {
        [0x0f, 0x5b, modrm] => (0u8, *modrm),
        [rex, 0x0f, 0x5b, modrm] if matches!(*rex, 0x41 | 0x44 | 0x45) => (*rex, *modrm),
        _ => return None,
    };
    if modrm >> 6 != 3 {
        return None;
    }
    let destination = ((modrm >> 3) & 7) | (((rex >> 2) & 1) << 3);
    let source = (modrm & 7) | ((rex & 1) << 3);
    let needs_rex = destination >= 8 || source >= 8;
    if needs_rex != (bytes.len() == 4)
        || (rex & 4 != 0) != (destination >= 8)
        || (rex & 1 != 0) != (source >= 8)
        || rex & 0x0a != 0
    {
        return None;
    }
    Some((destination, source, bytes.len() as u32))
}

/// Independently parse the complete canonical Long64 CVTSS2SD register encoding.
fn cvtss2sd_register_encoding(code_bytes: &str) -> Option<(u8, u8, u32)> {
    if code_bytes.len() % 2 != 0 {
        return None;
    }
    let bytes = (0..code_bytes.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&code_bytes[index..index + 2], 16).ok())
        .collect::<Option<Vec<_>>>()?;
    let (rex, modrm) = match bytes.as_slice() {
        [0xf3, 0x0f, 0x5a, modrm] => (0u8, *modrm),
        [0xf3, rex, 0x0f, 0x5a, modrm] if matches!(*rex, 0x41 | 0x44 | 0x45) => (*rex, *modrm),
        _ => return None,
    };
    if modrm >> 6 != 3 {
        return None;
    }
    let destination = ((modrm >> 3) & 7) | (((rex >> 2) & 1) << 3);
    let source = (modrm & 7) | ((rex & 1) << 3);
    if (destination >= 8 || source >= 8) != (bytes.len() == 5)
        || (rex & 4 != 0) != (destination >= 8)
        || (rex & 1 != 0) != (source >= 8)
        || rex & 0x0a != 0
    {
        return None;
    }
    Some((destination, source, bytes.len() as u32))
}

fn cvtps2pd_register_encoding(code_bytes: &str) -> Option<(u8, u8, u32)> {
    if code_bytes.len() % 2 != 0 {
        return None;
    }
    let bytes = (0..code_bytes.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&code_bytes[index..index + 2], 16).ok())
        .collect::<Option<Vec<_>>>()?;
    let (rex, modrm) = match bytes.as_slice() {
        [0x0f, 0x5a, modrm] => (0u8, *modrm),
        [rex, 0x0f, 0x5a, modrm] if matches!(*rex, 0x41 | 0x44 | 0x45) => (*rex, *modrm),
        _ => return None,
    };
    if modrm >> 6 != 3 {
        return None;
    }
    let destination = ((modrm >> 3) & 7) | (((rex >> 2) & 1) << 3);
    let source = (modrm & 7) | ((rex & 1) << 3);
    if (destination >= 8 || source >= 8) != (bytes.len() == 4)
        || (rex & 4 != 0) != (destination >= 8)
        || (rex & 1 != 0) != (source >= 8)
        || rex & 0x0a != 0
    {
        return None;
    }
    Some((destination, source, bytes.len() as u32))
}

fn comiss_ucomiss_register_encoding(code_bytes: &str) -> Option<(bool, u8, u8, u32)> {
    if code_bytes.len() % 2 != 0 {
        return None;
    }
    let bytes = (0..code_bytes.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&code_bytes[index..index + 2], 16).ok())
        .collect::<Option<Vec<_>>>()?;
    let (rex, opcode, modrm) = match bytes.as_slice() {
        [0x0f, opcode @ (0x2e | 0x2f), modrm] => (0u8, *opcode, *modrm),
        [rex, 0x0f, opcode @ (0x2e | 0x2f), modrm] if matches!(*rex, 0x41 | 0x44 | 0x45) => {
            (*rex, *opcode, *modrm)
        }
        _ => return None,
    };
    if modrm >> 6 != 3 {
        return None;
    }
    let destination = ((modrm >> 3) & 7) | (((rex >> 2) & 1) << 3);
    let source = (modrm & 7) | ((rex & 1) << 3);
    if (destination >= 8 || source >= 8) != (bytes.len() == 4)
        || (rex & 4 != 0) != (destination >= 8)
        || (rex & 1 != 0) != (source >= 8)
        || rex & 0x0a != 0
    {
        return None;
    }
    Some((opcode == 0x2e, destination, source, bytes.len() as u32))
}

fn validate_comisd_ucomisd_registers_result(
    loaded: &LoadedFixture,
    result: &RunResult,
    snapshot: &Snapshot,
) -> Result<(), FormatError> {
    let fixture = &loaded.fixture;
    let controls = fixture
        .comisd_ucomisd_controls
        .ok_or(FormatError::CaseMismatch("comisd_ucomisd_controls"))?;
    let initial = fixture
        .initial_state
        .sse
        .as_ref()
        .ok_or(FormatError::CaseMismatch("state.sse"))?;
    let code = fixture
        .memory
        .iter()
        .find(|r| r.region_id == "code")
        .ok_or(FormatError::CaseMismatch("memory.code"))?;
    let (unordered_quiet, destination, source, length) =
        comisd_ucomisd_register_encoding(&code.initial_bytes_hex)
            .ok_or(FormatError::CaseMismatch("COMISD/UCOMISD encoding"))?;
    let left = u64::from_le_bytes(
        decode_bytes(&initial.xmm[usize::from(destination)])?[..8]
            .try_into()
            .map_err(|_| FormatError::CaseMismatch("xmm"))?,
    );
    let right = u64::from_le_bytes(
        decode_bytes(&initial.xmm[usize::from(source)])?[..8]
            .try_into()
            .map_err(|_| FormatError::CaseMismatch("xmm"))?,
    );
    let class = |x: u64| {
        let e = (x >> 52) & 0x7ff;
        let f = x & 0x000f_ffff_ffff_ffff;
        if e == 0x7ff && f != 0 {
            if f & (1 << 51) != 0 { 3 } else { 4 }
        } else if e == 0 && f != 0 {
            2
        } else {
            0
        }
    };
    let lc = class(left);
    let rc = class(right);
    let nan = matches!(lc, 3 | 4) || matches!(rc, 3 | 4);
    let ie = if unordered_quiet {
        lc == 4 || rc == 4
    } else {
        nan
    };
    let de = !nan && (lc == 2 || rc == 2);
    let old = initial.mxcsr.get() as u32;
    let gate = if !controls.sse2_available || controls.cr0_em || !controls.cr4_osfxsr {
        Some(6)
    } else if controls.cr0_ts {
        Some(7)
    } else {
        None
    };
    let unmasked = gate.is_none() && ((ie && old & (1 << 7) == 0) || (de && old & (1 << 8) == 0));
    let fault = gate.or_else(|| unmasked.then_some(if controls.cr4_osxmmexcpt { 19 } else { 6 }));
    if !matches!(
        (fault.is_some(), fixture.expected_exit),
        (true, ExpectedExit::Exception) | (false, ExpectedExit::StopAddress)
    ) {
        return Err(FormatError::CaseMismatch("COMISD/UCOMISD expected exit"));
    }
    let mut expected = fixture.initial_state.clone();
    if gate.is_none() {
        let s = expected
            .sse
            .as_mut()
            .ok_or(FormatError::CaseMismatch("state.sse"))?;
        s.mxcsr = HexU64(u64::from(old | u32::from(ie) | (u32::from(de) << 1)));
        if !unmasked {
            let mask = 0x7fff_ffff_ffff_ffff;
            let cmp = if nan {
                core::cmp::Ordering::Equal
            } else if left & mask == 0 && right & mask == 0 {
                core::cmp::Ordering::Equal
            } else {
                match ((left >> 63) != 0, (right >> 63) != 0) {
                    (false, true) => core::cmp::Ordering::Greater,
                    (true, false) => core::cmp::Ordering::Less,
                    (false, false) => (left & mask).cmp(&(right & mask)),
                    (true, true) => (right & mask).cmp(&(left & mask)),
                }
            };
            let fm = (1 << 0) | (1 << 2) | (1 << 4) | (1 << 6) | (1 << 7) | (1 << 11);
            expected.rflags = HexU64(
                (expected.rflags.get() & !fm)
                    | if nan {
                        (1 << 0) | (1 << 2) | (1 << 6)
                    } else {
                        match cmp {
                            core::cmp::Ordering::Less => 1,
                            core::cmp::Ordering::Equal => 1 << 6,
                            core::cmp::Ordering::Greater => 0,
                        }
                    },
            );
            expected.rip = HexU64(
                fixture
                    .initial_state
                    .rip
                    .get()
                    .checked_add(u64::from(length))
                    .ok_or(FormatError::CaseMismatch("COMISD stop overflow"))?,
            );
        }
    }
    let memory = fixture
        .memory
        .iter()
        .map(|r| MemoryRegionSnapshot {
            region_id: r.region_id.clone(),
            guest_address: r.guest_address,
            bytes_hex: r.initial_bytes_hex.clone(),
        })
        .collect::<Vec<_>>();
    if snapshot.state != expected
        || snapshot.rflags_defined != HexU64(INTEGER_FLAGS)
        || snapshot.memory != memory
    {
        return Err(FormatError::CaseMismatch("COMISD/UCOMISD output state"));
    }
    if result.instructions_executed
        != (Observation::Available {
            value: HexU64(u64::from(fault.is_none())),
        })
    {
        return Err(FormatError::CaseMismatch("COMISD/UCOMISD retirement"));
    }
    match (fault, &result.outcome, &snapshot.exception) {
        (Some(v), Outcome::Exception {}, Observation::Available { value: Some(e) })
            if e.vector == v
                && e.error_code
                    == Observation::Unavailable {
                        reason: "exception has no architectural error code".into(),
                    }
                && e.faulting_rip == fixture.initial_state.rip
                && matches!(e.fault_address, Observation::Unavailable { .. }) => {}
        (None, Outcome::StopAddress {}, Observation::Available { value: None }) => {}
        _ => return Err(FormatError::CaseMismatch("COMISD/UCOMISD outcome")),
    }
    Ok(())
}

fn comisd_ucomisd_register_encoding(code_bytes: &str) -> Option<(bool, u8, u8, u32)> {
    if code_bytes.len() % 2 != 0 {
        return None;
    }
    let bytes = (0..code_bytes.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&code_bytes[i..i + 2], 16).ok())
        .collect::<Option<Vec<_>>>()?;
    let (rex, op, modrm) = match bytes.as_slice() {
        [0x66, 0x0f, op @ (0x2e | 0x2f), m] => (0u8, *op, *m),
        [0x66, rex, 0x0f, op @ (0x2e | 0x2f), m] if matches!(*rex, 0x41 | 0x44 | 0x45) => {
            (*rex, *op, *m)
        }
        _ => return None,
    };
    if modrm >> 6 != 3 {
        return None;
    }
    let d = ((modrm >> 3) & 7) | (((rex >> 2) & 1) << 3);
    let r = (modrm & 7) | ((rex & 1) << 3);
    if (d >= 8 || r >= 8) != (bytes.len() == 5)
        || (rex & 4 != 0) != (d >= 8)
        || (rex & 1 != 0) != (r >= 8)
        || rex & 0x0a != 0
    {
        return None;
    }
    Some((op == 0x2e, d, r, bytes.len() as u32))
}

fn cvtsi2sd_r32_register_length(code_bytes: &str) -> Option<u32> {
    code_bytes.eq_ignore_ascii_case("f20f2ac0").then_some(4)
}

fn cvtsi2sd_r64_register_length(code_bytes: &str) -> Option<u32> {
    code_bytes.eq_ignore_ascii_case("f2480f2ac0").then_some(5)
}

/// Independently parse the complete canonical Long64 CVTSI2SD register encoding.
fn cvtsi2sd_register_encoding(code_bytes: &str) -> Option<(bool, u8, u8, u32)> {
    if code_bytes.len() % 2 != 0 {
        return None;
    }
    let bytes = (0..code_bytes.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&code_bytes[index..index + 2], 16).ok())
        .collect::<Option<Vec<_>>>()?;
    let (rex, modrm) = match bytes.as_slice() {
        [0xf2, 0x0f, 0x2a, modrm] => (0u8, *modrm),
        [0xf2, rex, 0x0f, 0x2a, modrm] => (*rex, *modrm),
        _ => return None,
    };
    if modrm >> 6 != 3 {
        return None;
    }
    let wide = rex & 8 != 0;
    let destination = ((modrm >> 3) & 7) | (((rex >> 2) & 1) << 3);
    let source = (modrm & 7) | ((rex & 1) << 3);
    let expected_rex = match (wide, destination >= 8, source >= 8) {
        (false, false, false) => None,
        (false, true, false) => Some(0x44),
        (false, false, true) => Some(0x41),
        (false, true, true) => Some(0x45),
        (true, false, false) => Some(0x48),
        (true, true, false) => Some(0x4c),
        (true, false, true) => Some(0x49),
        (true, true, true) => Some(0x4d),
    };
    if (bytes.len() == 4 && expected_rex.is_none())
        || (bytes.len() == 5 && expected_rex == Some(rex))
    {
        Some((wide, destination, source, bytes.len() as u32))
    } else {
        None
    }
}

/// Independently parse the complete canonical Long64 CVTSI2SS register encoding.
fn cvtsi2ss_register_encoding(code_bytes: &str) -> Option<(bool, u8, u8, u32)> {
    if code_bytes.len() % 2 != 0 {
        return None;
    }
    let bytes = (0..code_bytes.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&code_bytes[index..index + 2], 16).ok())
        .collect::<Option<Vec<_>>>()?;
    let (rex, modrm) = match bytes.as_slice() {
        [0xf3, 0x0f, 0x2a, modrm] => (0u8, *modrm),
        [0xf3, rex, 0x0f, 0x2a, modrm] => (*rex, *modrm),
        _ => return None,
    };
    if modrm >> 6 != 3 {
        return None;
    }
    let wide = rex & 8 != 0;
    let destination = ((modrm >> 3) & 7) | (((rex >> 2) & 1) << 3);
    let source = (modrm & 7) | ((rex & 1) << 3);
    let expected_rex = match (wide, destination >= 8, source >= 8) {
        (false, false, false) => None,
        (false, true, false) => Some(0x44),
        (false, false, true) => Some(0x41),
        (false, true, true) => Some(0x45),
        (true, false, false) => Some(0x48),
        (true, false, true) => Some(0x49),
        (true, true, false) => Some(0x4c),
        (true, true, true) => Some(0x4d),
    };
    let actual_rex = if bytes.len() == 5 {
        Some(bytes[1])
    } else {
        None
    };
    if actual_rex != expected_rex {
        return None;
    }
    Some((wide, destination, source, bytes.len() as u32))
}

fn punpckhqdq_register_length(code_bytes: &str) -> Option<u32> {
    if code_bytes.len() % 2 != 0 {
        return None;
    }
    let bytes = (0..code_bytes.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&code_bytes[index..index + 2], 16).ok())
        .collect::<Option<Vec<_>>>()?;
    match bytes.as_slice() {
        [0x66, 0x0f, 0x6d, modrm] if modrm >> 6 == 3 => Some(4),
        [0x66, rex, 0x0f, 0x6d, modrm]
            if (0x40..=0x4f).contains(rex) && rex & 0x0a == 0 && modrm >> 6 == 3 =>
        {
            Some(5)
        }
        _ => None,
    }
}

fn punpckhdq_register_length(code_bytes: &str) -> Option<u32> {
    if code_bytes.len() % 2 != 0 {
        return None;
    }
    let bytes = (0..code_bytes.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&code_bytes[index..index + 2], 16).ok())
        .collect::<Option<Vec<_>>>()?;
    match bytes.as_slice() {
        [0x66, 0x0f, 0x6a, modrm] if modrm >> 6 == 3 => Some(4),
        [0x66, rex, 0x0f, 0x6a, modrm]
            if (0x40..=0x4f).contains(rex) && rex & 0x0a == 0 && modrm >> 6 == 3 =>
        {
            Some(5)
        }
        _ => None,
    }
}

fn psubusb_register_length(code_bytes: &str) -> Option<u32> {
    if code_bytes.len() % 2 != 0 {
        return None;
    }
    let bytes = (0..code_bytes.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&code_bytes[index..index + 2], 16).ok())
        .collect::<Option<Vec<_>>>()?;
    match bytes.as_slice() {
        [0x66, 0x0f, 0xd8, modrm] if modrm >> 6 == 3 => Some(4),
        [0x66, rex, 0x0f, 0xd8, modrm]
            if (0x40..=0x4f).contains(rex) && rex & 0x0a == 0 && modrm >> 6 == 3 =>
        {
            Some(5)
        }
        _ => None,
    }
}

fn andps_register_length(code_bytes: &str) -> Option<u32> {
    if code_bytes.len() % 2 != 0 {
        return None;
    }
    let bytes = (0..code_bytes.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&code_bytes[index..index + 2], 16).ok())
        .collect::<Option<Vec<_>>>()?;
    match bytes.as_slice() {
        [0x0f, 0x54, modrm] if modrm >> 6 == 3 => Some(3),
        [rex, 0x0f, 0x54, modrm]
            if (0x40..=0x4f).contains(rex) && rex & 0x0a == 0 && modrm >> 6 == 3 =>
        {
            Some(4)
        }
        _ => None,
    }
}

fn orps_register_length(code_bytes: &str) -> Option<u32> {
    if code_bytes.len() % 2 != 0 {
        return None;
    }
    let bytes = (0..code_bytes.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&code_bytes[index..index + 2], 16).ok())
        .collect::<Option<Vec<_>>>()?;
    match bytes.as_slice() {
        [0x0f, 0x56, modrm] if modrm >> 6 == 3 => Some(3),
        [rex, 0x0f, 0x56, modrm]
            if (0x40..=0x4f).contains(rex) && rex & 0x0a == 0 && modrm >> 6 == 3 =>
        {
            Some(4)
        }
        _ => None,
    }
}

fn movss_register_length(code_bytes: &str) -> Option<u32> {
    if code_bytes.len() % 2 != 0 {
        return None;
    }
    let bytes = (0..code_bytes.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&code_bytes[index..index + 2], 16).ok())
        .collect::<Option<Vec<_>>>()?;
    match bytes.as_slice() {
        [0xf3, 0x0f, 0x10, modrm] if modrm >> 6 == 3 => Some(4),
        [0xf3, rex, 0x0f, 0x10, modrm]
            if (0x40..=0x4f).contains(rex) && rex & 0x0a == 0 && modrm >> 6 == 3 =>
        {
            Some(5)
        }
        _ => None,
    }
}

fn rsqrtss_register_length(code_bytes: &str) -> Option<u32> {
    let bytes = decode_bytes(code_bytes).ok()?;
    match bytes.as_slice() {
        [0xf3, 0x0f, 0x52, modrm] if modrm >> 6 == 3 => Some(4),
        [0xf3, rex, 0x0f, 0x52, modrm]
            if (0x40..=0x4f).contains(rex) && rex & 0x0a == 0 && modrm >> 6 == 3 =>
        {
            Some(5)
        }
        _ => None,
    }
}

fn rcpss_register_length(code_bytes: &str) -> Option<u32> {
    let bytes = decode_bytes(code_bytes).ok()?;
    match bytes.as_slice() {
        [0xf3, 0x0f, 0x53, modrm] if modrm >> 6 == 3 => Some(4),
        [0xf3, rex, 0x0f, 0x53, modrm]
            if (0x40..=0x4f).contains(rex) && rex & 0x0a == 0 && modrm >> 6 == 3 =>
        {
            Some(5)
        }
        _ => None,
    }
}

fn rsqrtps_register_length(code_bytes: &str) -> Option<u32> {
    let bytes = decode_bytes(code_bytes).ok()?;
    match bytes.as_slice() {
        [0x0f, 0x52, modrm] if modrm >> 6 == 3 => Some(3),
        [rex, 0x0f, 0x52, modrm]
            if (0x40..=0x4f).contains(rex) && rex & 0x0a == 0 && modrm >> 6 == 3 =>
        {
            Some(4)
        }
        _ => None,
    }
}

fn rcpps_register_length(code_bytes: &str) -> Option<u32> {
    let bytes = decode_bytes(code_bytes).ok()?;
    match bytes.as_slice() {
        [0x0f, 0x53, modrm] if modrm >> 6 == 3 => Some(3),
        [rex, 0x0f, 0x53, modrm]
            if (0x40..=0x4f).contains(rex) && rex & 0x0a == 0 && modrm >> 6 == 3 =>
        {
            Some(4)
        }
        _ => None,
    }
}

fn minss_register_masked_length(code_bytes: &str) -> Option<u32> {
    let bytes = decode_bytes(code_bytes).ok()?;
    match bytes.as_slice() {
        [0xf3, 0x0f, 0x5d, modrm] if modrm >> 6 == 3 => Some(4),
        [0xf3, rex, 0x0f, 0x5d, modrm]
            if (0x40..=0x4f).contains(rex) && rex & 0x0a == 0 && modrm >> 6 == 3 =>
        {
            Some(5)
        }
        _ => None,
    }
}

fn minsd_register_masked_length(code_bytes: &str) -> Option<u32> {
    let bytes = decode_bytes(code_bytes).ok()?;
    match bytes.as_slice() {
        [0xf2, 0x0f, 0x5d, modrm] if modrm >> 6 == 3 => Some(4),
        [0xf2, rex, 0x0f, 0x5d, modrm]
            if (0x40..=0x4f).contains(rex) && rex & 0x0a == 0 && modrm >> 6 == 3 =>
        {
            Some(5)
        }
        _ => None,
    }
}

fn minpd_register_masked_length(code_bytes: &str) -> Option<u32> {
    let bytes = decode_bytes(code_bytes).ok()?;
    match bytes.as_slice() {
        [0x66, 0x0f, 0x5d, modrm] if modrm >> 6 == 3 => Some(4),
        [0x66, rex, 0x0f, 0x5d, modrm]
            if (0x40..=0x4f).contains(rex) && rex & 0x0a == 0 && modrm >> 6 == 3 =>
        {
            Some(5)
        }
        _ => None,
    }
}

fn maxpd_register_masked_length(code_bytes: &str) -> Option<u32> {
    let bytes = decode_bytes(code_bytes).ok()?;
    match bytes.as_slice() {
        [0x66, 0x0f, 0x5f, modrm] if modrm >> 6 == 3 => Some(4),
        [0x66, rex, 0x0f, 0x5f, modrm]
            if (0x40..=0x4f).contains(rex) && rex & 0x0a == 0 && modrm >> 6 == 3 =>
        {
            Some(5)
        }
        _ => None,
    }
}

fn maxsd_register_masked_length(code_bytes: &str) -> Option<u32> {
    let bytes = decode_bytes(code_bytes).ok()?;
    match bytes.as_slice() {
        [0xf2, 0x0f, 0x5f, modrm] if modrm >> 6 == 3 => Some(4),
        [0xf2, rex, 0x0f, 0x5f, modrm]
            if (0x40..=0x4f).contains(rex) && rex & 0x0a == 0 && modrm >> 6 == 3 =>
        {
            Some(5)
        }
        _ => None,
    }
}

fn maxss_register_masked_length(code_bytes: &str) -> Option<u32> {
    let bytes = decode_bytes(code_bytes).ok()?;
    match bytes.as_slice() {
        [0xf3, 0x0f, 0x5f, modrm] if modrm >> 6 == 3 => Some(4),
        [0xf3, rex, 0x0f, 0x5f, modrm]
            if (0x40..=0x4f).contains(rex) && rex & 0x0a == 0 && modrm >> 6 == 3 =>
        {
            Some(5)
        }
        _ => None,
    }
}

fn min_max_masked_mxcsr_supported(mxcsr: u64) -> bool {
    let Ok(mxcsr) = u32::try_from(mxcsr) else {
        return false;
    };
    let exception_masks = 0x0000_1f80;
    let daz_ftz = 0x0000_8040;
    mxcsr & 0xffff_0000 == 0 && mxcsr & exception_masks == exception_masks && mxcsr & daz_ftz == 0
}

fn minps_register_masked_length(code_bytes: &str) -> Option<u32> {
    let bytes = decode_bytes(code_bytes).ok()?;
    match bytes.as_slice() {
        [0x0f, 0x5d, modrm] if modrm >> 6 == 3 => Some(3),
        [rex, 0x0f, 0x5d, modrm]
            if (0x40..=0x4f).contains(rex) && rex & 0x0a == 0 && modrm >> 6 == 3 =>
        {
            Some(4)
        }
        _ => None,
    }
}

fn minps_exact_unmasked_length(code_bytes: &str) -> Option<u32> {
    (decode_bytes(code_bytes).ok()?.as_slice() == [0x0f, 0x5d, 0xc1]).then_some(3)
}

fn maxps_exact_unmasked_length(code_bytes: &str) -> Option<u32> {
    (decode_bytes(code_bytes).ok()?.as_slice() == [0x0f, 0x5f, 0xc1]).then_some(3)
}

fn maxps_register_masked_length(code_bytes: &str) -> Option<u32> {
    let bytes = decode_bytes(code_bytes).ok()?;
    match bytes.as_slice() {
        [0x0f, 0x5f, modrm] if modrm >> 6 == 3 => Some(3),
        [rex, 0x0f, 0x5f, modrm]
            if (0x40..=0x4f).contains(rex) && rex & 0x0a == 0 && modrm >> 6 == 3 =>
        {
            Some(4)
        }
        _ => None,
    }
}

fn movsd_register_length(code_bytes: &str) -> Option<u32> {
    let bytes = decode_bytes(code_bytes).ok()?;
    match bytes.as_slice() {
        [0xf2, 0x0f, 0x10, modrm] if modrm >> 6 == 3 => Some(4),
        [0xf2, rex, 0x0f, 0x10, modrm]
            if (0x40..=0x4f).contains(rex) && rex & 0x0a == 0 && modrm >> 6 == 3 =>
        {
            Some(5)
        }
        _ => None,
    }
}

fn movsd_store_register_length(code_bytes: &str) -> Option<u32> {
    let bytes = decode_bytes(code_bytes).ok()?;
    match bytes.as_slice() {
        [0xf2, 0x0f, 0x11, modrm] if modrm >> 6 == 3 => Some(4),
        [0xf2, rex, 0x0f, 0x11, modrm]
            if (0x40..=0x4f).contains(rex) && rex & 0x0a == 0 && modrm >> 6 == 3 =>
        {
            Some(5)
        }
        _ => None,
    }
}

fn movss_memory_load_destination(code_bytes: &str) -> Option<(u32, u8)> {
    if code_bytes.len() % 2 != 0 {
        return None;
    }
    let bytes = (0..code_bytes.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&code_bytes[index..index + 2], 16).ok())
        .collect::<Option<Vec<_>>>()?;
    let (length, rex, modrm) = match bytes.as_slice() {
        [0xf3, 0x0f, 0x10, modrm] => (4, 0, *modrm),
        [0xf3, rex, 0x0f, 0x10, modrm] if (0x40..=0x4f).contains(rex) && rex & 0x0b == 0 => {
            (5, *rex, *modrm)
        }
        _ => return None,
    };
    (modrm >> 6 == 0 && modrm & 7 == 0).then_some((length, ((modrm >> 3) & 7) | ((rex & 4) << 1)))
}

fn movsd_memory_load_destination(code_bytes: &str) -> Option<(u32, u8)> {
    let bytes = decode_bytes(code_bytes).ok()?;
    let (length, rex, modrm) = match bytes.as_slice() {
        [0xf2, 0x0f, 0x10, modrm] => (4, 0, *modrm),
        [0xf2, rex, 0x0f, 0x10, modrm] if (0x40..=0x4f).contains(rex) && rex & 0x0b == 0 => {
            (5, *rex, *modrm)
        }
        _ => return None,
    };
    (modrm >> 6 == 0 && modrm & 7 == 0).then_some((length, ((modrm >> 3) & 7) | ((rex & 4) << 1)))
}

fn movsd_memory_store_source(code_bytes: &str) -> Option<(u32, u8)> {
    let bytes = decode_bytes(code_bytes).ok()?;
    let (length, rex, modrm) = match bytes.as_slice() {
        [0xf2, 0x0f, 0x11, modrm] => (4, 0, *modrm),
        [0xf2, rex, 0x0f, 0x11, modrm] if (0x40..=0x4f).contains(rex) && rex & 0x0b == 0 => {
            (5, *rex, *modrm)
        }
        _ => return None,
    };
    (modrm >> 6 == 0 && modrm & 7 == 0).then_some((length, ((modrm >> 3) & 7) | ((rex & 4) << 1)))
}

fn movss_memory_store_source(code_bytes: &str) -> Option<(u32, u8)> {
    if code_bytes.len() % 2 != 0 {
        return None;
    }
    let bytes = (0..code_bytes.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&code_bytes[index..index + 2], 16).ok())
        .collect::<Option<Vec<_>>>()?;
    let (length, rex, modrm) = match bytes.as_slice() {
        [0xf3, 0x0f, 0x11, modrm] => (4, 0, *modrm),
        [0xf3, rex, 0x0f, 0x11, modrm] if (0x40..=0x4f).contains(rex) && rex & 0x0b == 0 => {
            (5, *rex, *modrm)
        }
        _ => return None,
    };
    (modrm >> 6 == 0 && modrm & 7 == 0).then_some((length, ((modrm >> 3) & 7) | ((rex & 4) << 1)))
}

fn movss_store_register_length(code_bytes: &str) -> Option<u32> {
    if code_bytes.len() % 2 != 0 {
        return None;
    }
    let bytes = (0..code_bytes.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&code_bytes[index..index + 2], 16).ok())
        .collect::<Option<Vec<_>>>()?;
    match bytes.as_slice() {
        [0xf3, 0x0f, 0x11, modrm] if modrm >> 6 == 3 => Some(4),
        [0xf3, rex, 0x0f, 0x11, modrm]
            if (0x40..=0x4f).contains(rex) && rex & 0x0a == 0 && modrm >> 6 == 3 =>
        {
            Some(5)
        }
        _ => None,
    }
}

fn andnps_register_length(code_bytes: &str) -> Option<u32> {
    if code_bytes.len() % 2 != 0 {
        return None;
    }
    let bytes = (0..code_bytes.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&code_bytes[index..index + 2], 16).ok())
        .collect::<Option<Vec<_>>>()?;
    match bytes.as_slice() {
        [0x0f, 0x55, modrm] if modrm >> 6 == 3 => Some(3),
        [rex, 0x0f, 0x55, modrm]
            if (0x40..=0x4f).contains(rex) && rex & 0x0a == 0 && modrm >> 6 == 3 =>
        {
            Some(4)
        }
        _ => None,
    }
}

fn movups_register_length(code_bytes: &str) -> Option<u32> {
    if code_bytes.len() % 2 != 0 {
        return None;
    }
    let bytes = (0..code_bytes.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&code_bytes[index..index + 2], 16).ok())
        .collect::<Option<Vec<_>>>()?;
    let opcode_at = match bytes.as_slice() {
        [0x0f, 0x10 | 0x11, modrm] if modrm >> 6 == 3 => 0,
        [rex, 0x0f, 0x10 | 0x11, modrm]
            if (0x40..=0x4f).contains(rex) && rex & 0x0a == 0 && modrm >> 6 == 3 =>
        {
            1
        }
        _ => return None,
    };
    Some((opcode_at + 3) as u32)
}

fn movaps_register_length(code_bytes: &str, reverse: bool) -> Option<u32> {
    if code_bytes.len() % 2 != 0 {
        return None;
    }
    let bytes = (0..code_bytes.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&code_bytes[index..index + 2], 16).ok())
        .collect::<Option<Vec<_>>>()?;
    let opcode = if reverse { 0x29 } else { 0x28 };
    match bytes.as_slice() {
        [0x0f, op, modrm] if *op == opcode && modrm >> 6 == 3 => Some(3),
        [rex, 0x0f, op, modrm]
            if *op == opcode
                && (0x40..=0x4f).contains(rex)
                && rex & 0x0a == 0
                && modrm >> 6 == 3 =>
        {
            Some(4)
        }
        _ => None,
    }
}

fn movapd_register_length(code_bytes: &str) -> Option<u32> {
    if code_bytes.len() % 2 != 0 {
        return None;
    }
    let bytes = (0..code_bytes.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&code_bytes[index..index + 2], 16).ok())
        .collect::<Option<Vec<_>>>()?;
    match bytes.as_slice() {
        [0x66, 0x0f, 0x28, modrm] if modrm >> 6 == 3 => Some(4),
        [0x66, rex, 0x0f, 0x28, modrm]
            if (0x40..=0x4f).contains(rex) && rex & 0x0a == 0 && modrm >> 6 == 3 =>
        {
            Some(5)
        }
        _ => None,
    }
}

fn movupd_register_length(code_bytes: &str) -> Option<u32> {
    if code_bytes.len() % 2 != 0 {
        return None;
    }
    let bytes = (0..code_bytes.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&code_bytes[index..index + 2], 16).ok())
        .collect::<Option<Vec<_>>>()?;
    match bytes.as_slice() {
        [0x66, 0x0f, 0x10, modrm] if modrm >> 6 == 3 => Some(4),
        [0x66, rex, 0x0f, 0x10, modrm]
            if (0x40..=0x4f).contains(rex) && rex & 0x0a == 0 && modrm >> 6 == 3 =>
        {
            Some(5)
        }
        _ => None,
    }
}

fn movupd_reverse_register_length(code_bytes: &str) -> Option<u32> {
    if code_bytes.len() % 2 != 0 {
        return None;
    }
    let bytes = (0..code_bytes.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&code_bytes[index..index + 2], 16).ok())
        .collect::<Option<Vec<_>>>()?;
    match bytes.as_slice() {
        [0x66, 0x0f, 0x11, modrm] if modrm >> 6 == 3 => Some(4),
        [0x66, rex, 0x0f, 0x11, modrm]
            if (0x40..=0x4f).contains(rex) && rex & 0x0a == 0 && modrm >> 6 == 3 =>
        {
            Some(5)
        }
        _ => None,
    }
}

fn movapd_reverse_register_length(code_bytes: &str) -> Option<u32> {
    if code_bytes.len() % 2 != 0 {
        return None;
    }
    let bytes = (0..code_bytes.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&code_bytes[index..index + 2], 16).ok())
        .collect::<Option<Vec<_>>>()?;
    match bytes.as_slice() {
        [0x66, 0x0f, 0x29, modrm] if modrm >> 6 == 3 => Some(4),
        [0x66, rex, 0x0f, 0x29, modrm]
            if (0x40..=0x4f).contains(rex) && rex & 0x0a == 0 && modrm >> 6 == 3 =>
        {
            Some(5)
        }
        _ => None,
    }
}

fn movups_memory_load_destination(code_bytes: &str) -> Option<(u32, u8)> {
    if code_bytes.len() % 2 != 0 {
        return None;
    }
    let bytes = (0..code_bytes.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&code_bytes[index..index + 2], 16).ok())
        .collect::<Option<Vec<_>>>()?;
    let (prefix_len, modrm) = match bytes.as_slice() {
        [0x0f, 0x10, modrm] => (0, *modrm),
        [rex, 0x0f, 0x10, modrm] if (0x40..=0x4f).contains(rex) && rex & 0x0b == 0 => (1, *modrm),
        _ => return None,
    };
    (modrm >> 6 == 0 && modrm & 7 == 0).then_some((
        (prefix_len + 3) as u32,
        ((modrm >> 3) & 7) | ((bytes.first().copied().unwrap_or(0) & 4) << 1),
    ))
}

fn movupd_memory_load_destination(code_bytes: &str) -> Option<(u32, u8)> {
    if code_bytes.len() % 2 != 0 {
        return None;
    }
    let bytes = (0..code_bytes.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&code_bytes[index..index + 2], 16).ok())
        .collect::<Option<Vec<_>>>()?;
    let (rex, modrm) = match bytes.as_slice() {
        [0x66, 0x0f, 0x10, modrm] => (0, *modrm),
        [0x66, rex, 0x0f, 0x10, modrm] if matches!(rex, 0x40 | 0x44) => (*rex, *modrm),
        _ => return None,
    };
    (modrm >> 6 == 0 && modrm & 7 == 0)
        .then_some((bytes.len() as u32, ((modrm >> 3) & 7) | ((rex & 4) << 1)))
}

fn movupd_memory_store_source(code_bytes: &str) -> Option<(u32, u8)> {
    if code_bytes.len() % 2 != 0 {
        return None;
    }
    let bytes = (0..code_bytes.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&code_bytes[index..index + 2], 16).ok())
        .collect::<Option<Vec<_>>>()?;
    let (rex, modrm) = match bytes.as_slice() {
        [0x66, 0x0f, 0x11, modrm] => (0, *modrm),
        [0x66, rex, 0x0f, 0x11, modrm] if matches!(rex, 0x40 | 0x44) => (*rex, *modrm),
        _ => return None,
    };
    (modrm >> 6 == 0 && modrm & 7 == 0)
        .then_some((bytes.len() as u32, ((modrm >> 3) & 7) | ((rex & 4) << 1)))
}

fn movups_memory_store_length(code_bytes: &str) -> Option<u32> {
    if code_bytes.len() % 2 != 0 {
        return None;
    }
    let bytes = (0..code_bytes.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&code_bytes[index..index + 2], 16).ok())
        .collect::<Option<Vec<_>>>()?;
    let (prefix_len, modrm) = match bytes.as_slice() {
        [0x0f, 0x11, modrm] => (0, *modrm),
        [rex, 0x0f, 0x11, modrm] if (0x40..=0x4f).contains(rex) && rex & 0x0b == 0 => (1, *modrm),
        _ => return None,
    };
    (modrm >> 6 == 0 && modrm & 7 == 0).then_some((prefix_len + 3) as u32)
}

fn movups_memory_base_form(
    code_bytes: &str,
    state: &LongModeState,
) -> Option<(u32, bool, u64, bool)> {
    let bytes = decode_bytes(code_bytes).ok()?;
    let mut memory = InstructionBytes { bytes: &bytes };
    let instruction = od_isa::long_decode::decode(&mut memory, 0, AccessContext::default()).ok()?;
    if usize::from(instruction.length) != bytes.len() {
        return None;
    }
    let (store, operand) = match instruction.operation {
        Operation::MovupsLoadMemory { source, .. } => (false, source),
        Operation::MovupsStoreMemory { destination, .. } => (true, destination),
        Operation::MovupdLoadMemory { source, .. } => (false, source),
        Operation::MovupdStoreMemory { destination, .. } => (true, destination),
        _ => return None,
    };
    let next_rip = state.rip.wrapping_add(u64::from(instruction.length));
    let start = operand.effective_address(state, next_rip);
    Some((
        u32::from(instruction.length),
        store,
        start,
        matches!(operand.base, od_isa::long_decode::AddressBase::NextRip),
    ))
}

/// Checks the original instruction bytes, because SIB is an encoding class:
/// a normalized `MemoryOperand` alone cannot prove that ModR/M.r/m was 100b.
fn movupd_sib_memory_form(code_bytes: &str) -> Option<()> {
    let bytes = decode_bytes(code_bytes).ok()?;
    let opcode_at = match bytes.as_slice() {
        [0x66, 0x0f, 0x10 | 0x11, ..] => 1,
        [0x66, rex, 0x0f, 0x10 | 0x11, ..] if (0x40..=0x47).contains(rex) => 2,
        _ => return None,
    };
    let modrm_at = opcode_at + 2;
    let modrm = *bytes.get(modrm_at)?;
    if modrm >> 6 == 3 || modrm & 7 != 4 {
        return None;
    }
    let sib = *bytes.get(modrm_at + 1)?;
    let mode = modrm >> 6;
    let has_disp32 = mode == 2 || (mode == 0 && sib & 7 == 5);
    let has_disp8 = mode == 1;
    let expected_len = modrm_at
        + 2
        + if has_disp32 {
            4
        } else if has_disp8 {
            1
        } else {
            0
        };
    (bytes.len() == expected_len).then_some(())
}

/// Fetch-only source used by schema validation to apply the core x86 decoder.
struct InstructionBytes<'a> {
    bytes: &'a [u8],
}

impl GuestMemory for InstructionBytes<'_> {
    fn fetch(&mut self, address: u64, _: AccessContext) -> Result<u8, MemoryError> {
        self.bytes
            .get(usize::try_from(address).unwrap_or(usize::MAX))
            .copied()
            .ok_or(MemoryError::Unmapped {
                address,
                access: AccessKind::Fetch,
            })
    }

    fn read(&mut self, address: u64, _: AccessContext, _: &mut [u8]) -> Result<(), MemoryError> {
        Err(MemoryError::Unmapped {
            address,
            access: AccessKind::Read,
        })
    }

    fn write(&mut self, address: u64, _: AccessContext, _: &[u8]) -> Result<(), MemoryError> {
        Err(MemoryError::Unmapped {
            address,
            access: AccessKind::Write,
        })
    }

    fn atomic_exchange_integer(
        &mut self,
        address: u64,
        _: AccessContext,
        _: OperandSize,
        _: u64,
    ) -> Result<u64, MemoryError> {
        Err(MemoryError::Unmapped {
            address,
            access: AccessKind::Read,
        })
    }

    fn atomic_fetch_add_integer(
        &mut self,
        address: u64,
        _: AccessContext,
        _: OperandSize,
        _: u64,
    ) -> Result<u64, MemoryError> {
        Err(MemoryError::Unmapped {
            address,
            access: AccessKind::Read,
        })
    }

    fn atomic_compare_exchange_integer(
        &mut self,
        address: u64,
        _: AccessContext,
        _: OperandSize,
        _: u64,
        _: u64,
    ) -> Result<u64, MemoryError> {
        Err(MemoryError::Unmapped {
            address,
            access: AccessKind::Read,
        })
    }

    fn mfence(&mut self, _: AccessContext) -> Result<(), MemoryError> {
        Ok(())
    }
}

/// Exact S2-30ch FCMOVU bare, FNSTSW, repeat, and retained producer streams.
fn fcmovu_stream(code_bytes: &str) -> bool {
    fcmovu_budget(code_bytes).is_some()
}

fn fcmovu_budget(code_bytes: &str) -> Option<u64> {
    if code_bytes == "dad9dad9dfe0" {
        return Some(3);
    }
    if code_bytes == "d9e8d9ee4885c0dad9dfe0" {
        return Some(5);
    }
    if code_bytes.len() == 4 {
        let bytes = u16::from_str_radix(code_bytes, 16).ok()?;
        return (bytes >> 8 == 0xda && (0xd8..=0xdf).contains(&(bytes as u8))).then_some(1);
    }
    if code_bytes.len() == 8 {
        let instruction = &code_bytes[..4];
        let suffix = &code_bytes[4..];
        if suffix == "dfe0" {
            let bytes = u16::from_str_radix(instruction, 16).ok()?;
            if bytes >> 8 == 0xda && (0xd8..=0xdf).contains(&(bytes as u8)) {
                return Some(2);
            }
        }
    }
    None
}

#[cfg(test)]
#[path = "v2_tests.rs"]
mod tests;

fn paddq_register_length(code_bytes: &str) -> Option<u32> {
    if code_bytes.len() % 2 != 0 {
        return None;
    }
    let bytes = (0..code_bytes.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&code_bytes[index..index + 2], 16).ok())
        .collect::<Option<Vec<_>>>()?;
    match bytes.as_slice() {
        [0x66, 0x0f, 0xd4, modrm] if modrm >> 6 == 3 => Some(4),
        [0x66, rex, 0x0f, 0xd4, modrm]
            if (0x40..=0x4f).contains(rex) && rex & 0x0a == 0 && modrm >> 6 == 3 =>
        {
            Some(5)
        }
        _ => None,
    }
}

fn psubq_register_length(code_bytes: &str) -> Option<u32> {
    if code_bytes.len() % 2 != 0 {
        return None;
    }
    let bytes = (0..code_bytes.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&code_bytes[index..index + 2], 16).ok())
        .collect::<Option<Vec<_>>>()?;
    match bytes.as_slice() {
        [0x66, 0x0f, 0xfb, modrm] if modrm >> 6 == 3 => Some(4),
        [0x66, rex, 0x0f, 0xfb, modrm]
            if (0x40..=0x4f).contains(rex) && rex & 0x0a == 0 && modrm >> 6 == 3 =>
        {
            Some(5)
        }
        _ => None,
    }
}

fn pcmpeqb_register_length(code_bytes: &str) -> Option<u32> {
    if code_bytes.len() % 2 != 0 {
        return None;
    }
    let bytes = (0..code_bytes.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&code_bytes[index..index + 2], 16).ok())
        .collect::<Option<Vec<_>>>()?;
    match bytes.as_slice() {
        [0x66, 0x0f, 0x74, modrm] if modrm >> 6 == 3 => Some(4),
        [0x66, rex, 0x0f, 0x74, modrm]
            if (0x40..=0x4f).contains(rex) && rex & 0x0a == 0 && modrm >> 6 == 3 =>
        {
            Some(5)
        }
        _ => None,
    }
}

fn pcmpeqw_register_length(code_bytes: &str) -> Option<u32> {
    if code_bytes.len() % 2 != 0 {
        return None;
    }
    let bytes = (0..code_bytes.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&code_bytes[index..index + 2], 16).ok())
        .collect::<Option<Vec<_>>>()?;
    match bytes.as_slice() {
        [0x66, 0x0f, 0x75, modrm] if modrm >> 6 == 3 => Some(4),
        [0x66, rex, 0x0f, 0x75, modrm]
            if (0x40..=0x4f).contains(rex) && rex & 0x0a == 0 && modrm >> 6 == 3 =>
        {
            Some(5)
        }
        _ => None,
    }
}

fn pcmpeqd_register_length(code_bytes: &str) -> Option<u32> {
    if code_bytes.len() % 2 != 0 {
        return None;
    }
    let bytes = (0..code_bytes.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&code_bytes[index..index + 2], 16).ok())
        .collect::<Option<Vec<_>>>()?;
    match bytes.as_slice() {
        [0x66, 0x0f, 0x76, modrm] if modrm >> 6 == 3 => Some(4),
        [0x66, rex, 0x0f, 0x76, modrm]
            if (0x40..=0x4f).contains(rex) && rex & 0x0a == 0 && modrm >> 6 == 3 =>
        {
            Some(5)
        }
        _ => None,
    }
}

fn pcmpgtb_register_length(code_bytes: &str) -> Option<u32> {
    if code_bytes.len() % 2 != 0 {
        return None;
    }
    let bytes = (0..code_bytes.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&code_bytes[index..index + 2], 16).ok())
        .collect::<Option<Vec<_>>>()?;
    match bytes.as_slice() {
        [0x66, 0x0f, 0x64, modrm] if modrm >> 6 == 3 => Some(4),
        [0x66, rex, 0x0f, 0x64, modrm]
            if (0x40..=0x4f).contains(rex) && rex & 0x0a == 0 && modrm >> 6 == 3 =>
        {
            Some(5)
        }
        _ => None,
    }
}

fn pcmpgtw_register_length(code_bytes: &str) -> Option<u32> {
    if code_bytes.len() % 2 != 0 {
        return None;
    }
    let bytes = (0..code_bytes.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&code_bytes[index..index + 2], 16).ok())
        .collect::<Option<Vec<_>>>()?;
    match bytes.as_slice() {
        [0x66, 0x0f, 0x65, modrm] if modrm >> 6 == 3 => Some(4),
        [0x66, rex, 0x0f, 0x65, modrm]
            if (0x40..=0x4f).contains(rex) && rex & 0x0a == 0 && modrm >> 6 == 3 =>
        {
            Some(5)
        }
        _ => None,
    }
}

fn pcmpgtd_register_length(code_bytes: &str) -> Option<u32> {
    if code_bytes.len() % 2 != 0 {
        return None;
    }
    let bytes = (0..code_bytes.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&code_bytes[index..index + 2], 16).ok())
        .collect::<Option<Vec<_>>>()?;
    match bytes.as_slice() {
        [0x66, 0x0f, 0x66, modrm] if modrm >> 6 == 3 => Some(4),
        [0x66, rex, 0x0f, 0x66, modrm]
            if (0x40..=0x4f).contains(rex) && rex & 0x0a == 0 && modrm >> 6 == 3 =>
        {
            Some(5)
        }
        _ => None,
    }
}

fn punpcklbw_register_length(code_bytes: &str) -> Option<u32> {
    if code_bytes.len() % 2 != 0 {
        return None;
    }
    let bytes = (0..code_bytes.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&code_bytes[index..index + 2], 16).ok())
        .collect::<Option<Vec<_>>>()?;
    match bytes.as_slice() {
        [0x66, 0x0f, 0x60, modrm] if modrm >> 6 == 3 => Some(4),
        [0x66, rex, 0x0f, 0x60, modrm]
            if (0x40..=0x4f).contains(rex) && rex & 0x0a == 0 && modrm >> 6 == 3 =>
        {
            Some(5)
        }
        _ => None,
    }
}
