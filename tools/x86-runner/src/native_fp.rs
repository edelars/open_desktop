//! Narrow FP-state codec for the native SSE oracle. Linux ptrace state is an
//! OS transport structure; it is not the guest FXSAVE memory format.

#![cfg_attr(
    not(all(target_os = "linux", target_arch = "x86_64")),
    allow(dead_code)
)]

pub(crate) fn native_profile_local_only(profile: od_testkit::v2::Profile) -> bool {
    matches!(
        profile,
        od_testkit::v2::Profile::LongFsBaseV1
            | od_testkit::v2::Profile::LongFsBaseMsrV1
            | od_testkit::v2::Profile::LongKernelGsBaseV1
            | od_testkit::v2::Profile::LongCliV1
            | od_testkit::v2::Profile::LongStiV1
            | od_testkit::v2::Profile::LongHltV1
            | od_testkit::v2::Profile::LongIretqCpl0ValidV1
            | od_testkit::v2::Profile::LongLretqCpl0ValidV1
            | od_testkit::v2::Profile::LongGsBaseV1
            | od_testkit::v2::Profile::LongFsSelectorV1
            | od_testkit::v2::Profile::LongGsSelectorV1
            | od_testkit::v2::Profile::LongVerwGdtV1
            | od_testkit::v2::Profile::LongFsLoadV1
            | od_testkit::v2::Profile::LongGsLoadV1
            | od_testkit::v2::Profile::LongNullDsEsV1
            | od_testkit::v2::Profile::LongX87StateV1
            | od_testkit::v2::Profile::LongX87SseStateV1
            | od_testkit::v2::Profile::LongCvtps2pdRegistersV1
            | od_testkit::v2::Profile::LongCvtpi2pdRegisterMaskedV1
            | od_testkit::v2::Profile::LongCvtpi2psRegisterMaskedV1
            | od_testkit::v2::Profile::LongCvttps2piMemoryRaxV1
            | od_testkit::v2::Profile::LongCvtps2piMemoryRaxV1
            | od_testkit::v2::Profile::LongCvtpd2piRegisterMaskedV1
            | od_testkit::v2::Profile::LongCvttpd2piRegisterMaskedV1
            | od_testkit::v2::Profile::LongCvttpd2piMemoryRaxV1
            | od_testkit::v2::Profile::LongCvtpd2piMemoryRaxV1
            | od_testkit::v2::Profile::LongCvtdq2pdMemoryV1
            | od_testkit::v2::Profile::LongCvtdq2psMemoryRaxV1
            | od_testkit::v2::Profile::LongCvttps2dqMemoryRaxV1
            | od_testkit::v2::Profile::LongCvtps2dqMemoryRaxV1
            | od_testkit::v2::Profile::LongCvttpd2dqMemoryRaxV1
            | od_testkit::v2::Profile::LongCvtpd2dqMemoryRaxV1
            | od_testkit::v2::Profile::LongComissUcomissRegistersV1
            | od_testkit::v2::Profile::LongCmpssRegistersV1
            | od_testkit::v2::Profile::LongCmpsdRegistersV1
            | od_testkit::v2::Profile::LongCmppdRegistersV1
            | od_testkit::v2::Profile::LongComisdUcomisdRegistersV1
            | od_testkit::v2::Profile::LongX87SelectorsStateV1
            | od_testkit::v2::Profile::LongCr0FpInitV1
            | od_testkit::v2::Profile::LongCr4FpInitV1
            | od_testkit::v2::Profile::LongCr0Cr4FpInitV1
            | od_testkit::v2::Profile::LongCr0Cr4FpInitFxsave64V1
            | od_testkit::v2::Profile::LongCr0Cr4FpInitFxrstor64MinssV1
            | od_testkit::v2::Profile::LongFxsaveRipV1
            | od_testkit::v2::Profile::LongFxsave64RaxV1
            | od_testkit::v2::Profile::LongFxrstor64RbxV1
            | od_testkit::v2::Profile::LongMovupsRegV1
            | od_testkit::v2::Profile::LongMovapsRegV1
            | od_testkit::v2::Profile::LongMovapsReverseRegV1
            | od_testkit::v2::Profile::LongMovapdRegV1
            | od_testkit::v2::Profile::LongMovapdReverseRegV1
            | od_testkit::v2::Profile::LongMovupdRegV1
            | od_testkit::v2::Profile::LongMovupdReverseRegV1
            | od_testkit::v2::Profile::LongXorpsRegV1
            | od_testkit::v2::Profile::LongXorpdRegV1
            | od_testkit::v2::Profile::LongXorpdPrerequisitesV1
            | od_testkit::v2::Profile::LongMinssInvalidControlsV1
            | od_testkit::v2::Profile::LongMinssUnmaskedDenormalV1
            | od_testkit::v2::Profile::LongMaxssUnmaskedDenormalV1
            | od_testkit::v2::Profile::LongPavgbRegV1
            | od_testkit::v2::Profile::LongPavgwRegV1
            | od_testkit::v2::Profile::LongPmaxubRegV1
            | od_testkit::v2::Profile::LongPminubRegV1
            | od_testkit::v2::Profile::LongPmaxswRegV1
            | od_testkit::v2::Profile::LongPminswRegV1
            | od_testkit::v2::Profile::LongPmullwRegV1
            | od_testkit::v2::Profile::LongPmulhuwRegV1
            | od_testkit::v2::Profile::LongPsadbwRegV1
            | od_testkit::v2::Profile::LongPmovmskbXmmGprV1
            | od_testkit::v2::Profile::LongPmuludqRegV1
            | od_testkit::v2::Profile::LongPmaddwdRegV1
            | od_testkit::v2::Profile::LongPshufdRegV1
            | od_testkit::v2::Profile::LongMovsdStoreRegV1
            | od_testkit::v2::Profile::LongMovupdMemSibV1
            | od_testkit::v2::Profile::LongAndpdRegV1
            | od_testkit::v2::Profile::LongOrpdRegV1
            | od_testkit::v2::Profile::LongAndnpdRegV1
            | od_testkit::v2::Profile::LongPxorRegV1
            | od_testkit::v2::Profile::LongPandRegV1
            | od_testkit::v2::Profile::LongPandnRegV1
            | od_testkit::v2::Profile::LongPorRegV1
            | od_testkit::v2::Profile::LongPaddbRegV1
            | od_testkit::v2::Profile::LongPaddwRegV1
            | od_testkit::v2::Profile::LongPsubwRegV1
            | od_testkit::v2::Profile::LongPadduswRegV1
            | od_testkit::v2::Profile::LongPsubuswRegV1
            | od_testkit::v2::Profile::LongPsubbRegV1
            | od_testkit::v2::Profile::LongPaddusbRegV1
            | od_testkit::v2::Profile::LongPaddsbRegV1
            | od_testkit::v2::Profile::LongPsubsbRegV1
            | od_testkit::v2::Profile::LongPaddswRegV1
            | od_testkit::v2::Profile::LongPsubswRegV1
            | od_testkit::v2::Profile::LongPadddRegV1
            | od_testkit::v2::Profile::LongPsubdRegV1
            | od_testkit::v2::Profile::LongPaddqRegV1
            | od_testkit::v2::Profile::LongPsubqRegV1
            | od_testkit::v2::Profile::LongPcmpeqbRegV1
            | od_testkit::v2::Profile::LongPcmpeqwRegV1
            | od_testkit::v2::Profile::LongPcmpeqdRegV1
            | od_testkit::v2::Profile::LongPcmpgtbRegV1
            | od_testkit::v2::Profile::LongPcmpgtwRegV1
            | od_testkit::v2::Profile::LongPcmpgtdRegV1
            | od_testkit::v2::Profile::LongPunpcklbwRegV1
            | od_testkit::v2::Profile::LongPunpckhbwRegV1
            | od_testkit::v2::Profile::LongPunpcklwdRegV1
            | od_testkit::v2::Profile::LongPunpckhwdRegV1
            | od_testkit::v2::Profile::LongPunpckldqRegV1
            | od_testkit::v2::Profile::LongPunpckhdqRegV1
            | od_testkit::v2::Profile::LongPunpcklqdqRegV1
            | od_testkit::v2::Profile::LongPunpckhqdqRegV1
            | od_testkit::v2::Profile::LongPacksswbRegV1
            | od_testkit::v2::Profile::LongPackuswbRegV1
            | od_testkit::v2::Profile::LongPackssdwRegV1
            | od_testkit::v2::Profile::LongPsllwImmRegV1
            | od_testkit::v2::Profile::LongPslldImmRegV1
            | od_testkit::v2::Profile::LongPsllqImmRegV1
            | od_testkit::v2::Profile::LongPslldqImmRegV1
            | od_testkit::v2::Profile::LongPsrldqImmRegV1
            | od_testkit::v2::Profile::LongPsrlqImmRegV1
            | od_testkit::v2::Profile::LongPsrldImmRegV1
            | od_testkit::v2::Profile::LongPsrlwImmRegV1
            | od_testkit::v2::Profile::LongPsrawImmRegV1
            | od_testkit::v2::Profile::LongPsradImmRegV1
            | od_testkit::v2::Profile::LongPsubusbRegV1
            | od_testkit::v2::Profile::LongAndpsRegV1
            | od_testkit::v2::Profile::LongOrpsRegV1
            | od_testkit::v2::Profile::LongAndnpsRegV1
            | od_testkit::v2::Profile::LongRsqrtssRegV1
            | od_testkit::v2::Profile::LongRsqrtpsRegV1
            | od_testkit::v2::Profile::LongRcppsRegV1
            | od_testkit::v2::Profile::LongMinssMaskedRegV1
            | od_testkit::v2::Profile::LongMinsdMaskedRegV1
            | od_testkit::v2::Profile::LongMaxsdMaskedRegV1
            | od_testkit::v2::Profile::LongMinpdMaskedRegV1
            | od_testkit::v2::Profile::LongMaxpdMaskedRegV1
            | od_testkit::v2::Profile::LongMaxssMaskedRegV1
            | od_testkit::v2::Profile::LongMinpsMaskedRegV1
            | od_testkit::v2::Profile::LongMaxpsMaskedRegV1
            | od_testkit::v2::Profile::LongRcpssRegV1
            | od_testkit::v2::Profile::LongMovssRegV1
            | od_testkit::v2::Profile::LongMovsdRegV1
            | od_testkit::v2::Profile::LongMovsdMemLoadV1
            | od_testkit::v2::Profile::LongMovsdMemStoreV1
            | od_testkit::v2::Profile::LongMovssStoreRegV1
            | od_testkit::v2::Profile::LongMovssMemLoadV1
            | od_testkit::v2::Profile::LongMovssMemStoreV1
            | od_testkit::v2::Profile::LongMovupsMemLoadV1
            | od_testkit::v2::Profile::LongMovupdMemLoadV1
            | od_testkit::v2::Profile::LongMovupdMemStoreV1
            | od_testkit::v2::Profile::LongMovupsMemStoreV1
            | od_testkit::v2::Profile::LongMovupsMemBaseV1
            | od_testkit::v2::Profile::LongMovupdMemBaseV1
            | od_testkit::v2::Profile::LongMovupdMemRipV1
    )
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct SseImage {
    pub xmm: [[u8; 16]; 16],
    pub mxcsr: u32,
}

impl SseImage {
    pub(crate) fn from_words(mxcsr: u32, words: &[u32; 64]) -> Self {
        let mut xmm = [[0; 16]; 16];
        for (register, bytes) in xmm.iter_mut().enumerate() {
            for (word, chunk) in bytes.chunks_exact_mut(4).enumerate() {
                let value = words[register * 4 + word].to_le_bytes();
                chunk.copy_from_slice(&value);
            }
        }
        Self { xmm, mxcsr }
    }

    pub(crate) fn write_words(&self, mxcsr: &mut u32, words: &mut [u32; 64]) {
        *mxcsr = self.mxcsr;
        for (register, bytes) in self.xmm.iter().enumerate() {
            for (word, chunk) in bytes.chunks_exact(4).enumerate() {
                words[register * 4 + word] = u32::from_le_bytes(chunk.try_into().unwrap());
            }
        }
    }
}

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
mod linux {
    use super::SseImage;
    use std::io;

    pub(super) fn read(pid: libc::pid_t) -> Result<libc::user_fpregs_struct, String> {
        // SAFETY: ptrace fills the ABI-defined x86_64 FP register struct for a stopped child.
        let mut fp = unsafe { std::mem::zeroed::<libc::user_fpregs_struct>() };
        if unsafe {
            libc::ptrace(
                libc::PTRACE_GETFPREGS,
                pid,
                std::ptr::null_mut::<libc::c_void>(),
                (&mut fp as *mut libc::user_fpregs_struct).cast::<libc::c_void>(),
            )
        } < 0
        {
            return Err(format!("PTRACE_GETFPREGS: {}", io::Error::last_os_error()));
        }
        Ok(fp)
    }

    pub(super) fn write(pid: libc::pid_t, fp: &mut libc::user_fpregs_struct) -> Result<(), String> {
        if unsafe {
            libc::ptrace(
                libc::PTRACE_SETFPREGS,
                pid,
                std::ptr::null_mut::<libc::c_void>(),
                (fp as *mut libc::user_fpregs_struct).cast::<libc::c_void>(),
            )
        } < 0
        {
            return Err(format!("PTRACE_SETFPREGS: {}", io::Error::last_os_error()));
        }
        Ok(())
    }

    pub(super) fn image(fp: &libc::user_fpregs_struct) -> SseImage {
        SseImage::from_words(fp.mxcsr, &fp.xmm_space)
    }

    pub(super) fn replace_sse(fp: &mut libc::user_fpregs_struct, image: &SseImage) {
        image.write_words(&mut fp.mxcsr, &mut fp.xmm_space);
    }
}

pub(crate) fn verify_readback(expected: &SseImage, actual: &SseImage) -> Result<(), String> {
    if actual == expected {
        Ok(())
    } else {
        Err("initial XMM0-15/MXCSR readback differs".into())
    }
}

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
pub(crate) fn install_and_verify(pid: libc::pid_t, expected: &SseImage) -> Result<(), String> {
    let mut fp = linux::read(pid)?;
    linux::replace_sse(&mut fp, expected);
    linux::write(pid, &mut fp)?;
    let actual = linux::image(&linux::read(pid)?);
    verify_readback(expected, &actual)
}

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
pub(crate) fn read_image(pid: libc::pid_t) -> Result<SseImage, String> {
    Ok(linux::image(&linux::read(pid)?))
}

#[cfg(test)]
mod tests {
    use super::SseImage;

    #[test]
    fn narrow_xorpd_is_not_blocked_with_existing_sse_and_x87_profiles_denied() {
        use od_testkit::v2::Profile;
        assert!(!super::native_profile_local_only(
            Profile::LongNativeXorpdXmm0Xmm1V1
        ));
        for denied in [
            Profile::LongKernelGsBaseV1,
            Profile::LongCliV1,
            Profile::LongStiV1,
            Profile::LongHltV1,
            Profile::LongIretqCpl0ValidV1,
            Profile::LongLretqCpl0ValidV1,
            Profile::LongXorpdRegV1,
            Profile::LongCmpssRegistersV1,
            Profile::LongCmpsdRegistersV1,
            Profile::LongCmppdRegistersV1,
            Profile::LongXorpdPrerequisitesV1,
            Profile::LongMinssInvalidControlsV1,
            Profile::LongMinssUnmaskedDenormalV1,
            Profile::LongMaxssUnmaskedDenormalV1,
            Profile::LongPavgbRegV1,
            Profile::LongPavgwRegV1,
            Profile::LongPmaxubRegV1,
            Profile::LongPminubRegV1,
            Profile::LongPmaxswRegV1,
            Profile::LongPminswRegV1,
            Profile::LongPmullwRegV1,
            Profile::LongPmulhuwRegV1,
            Profile::LongPsadbwRegV1,
            Profile::LongPmovmskbXmmGprV1,
            Profile::LongPmuludqRegV1,
            Profile::LongPmaddwdRegV1,
            Profile::LongPshufdRegV1,
            Profile::LongMovsdStoreRegV1,
            Profile::LongMovupdMemSibV1,
            Profile::LongCvtpi2pdRegisterMaskedV1,
            Profile::LongCvtpi2psRegisterMaskedV1,
            Profile::LongCvttps2piMemoryRaxV1,
            Profile::LongCvtps2piMemoryRaxV1,
            Profile::LongCvtpd2piRegisterMaskedV1,
            Profile::LongCvttpd2piRegisterMaskedV1,
            Profile::LongCvttpd2piMemoryRaxV1,
            Profile::LongCvtpd2piMemoryRaxV1,
            Profile::LongCvtdq2psMemoryRaxV1,
            Profile::LongCvttps2dqMemoryRaxV1,
            Profile::LongCvtps2dqMemoryRaxV1,
            Profile::LongCvttpd2dqMemoryRaxV1,
            Profile::LongCvtpd2dqMemoryRaxV1,
            Profile::LongX87SseStateV1,
            Profile::LongComisdUcomisdRegistersV1,
            Profile::LongCr0FpInitV1,
            Profile::LongCr4FpInitV1,
            Profile::LongCr0Cr4FpInitV1,
            Profile::LongCr0Cr4FpInitFxsave64V1,
            Profile::LongCr0Cr4FpInitFxrstor64MinssV1,
            Profile::LongFxsaveRipV1,
        ] {
            assert!(super::native_profile_local_only(denied));
        }
    }

    #[test]
    fn xmm_codec_roundtrips_all_register_bytes_and_mxcsr() {
        let mut words = [0; 64];
        for (index, word) in words.iter_mut().enumerate() {
            *word = 0x1020_3040u32.wrapping_add(index as u32 * 0x0101_0101);
        }
        let image = SseImage::from_words(0x1f80, &words);
        let mut actual_words = [0; 64];
        let mut mxcsr = 0;
        image.write_words(&mut mxcsr, &mut actual_words);
        assert_eq!(actual_words, words);
        assert_eq!(mxcsr, 0x1f80);
        assert_eq!(SseImage::from_words(mxcsr, &actual_words), image);
    }

    #[test]
    fn sse_readback_comparison_rejects_any_byte_or_mask_difference() {
        let expected = SseImage {
            xmm: std::array::from_fn(|register| {
                std::array::from_fn(|byte| (register * 13 + byte) as u8)
            }),
            mxcsr: 0x1f80,
        };
        let mut mismatched = expected.clone();
        mismatched.xmm[15][15] ^= 1;
        assert!(super::verify_readback(&expected, &mismatched).is_err());
        let mut mismatched = expected.clone();
        mismatched.mxcsr ^= 0x40;
        assert!(super::verify_readback(&expected, &mismatched).is_err());
        assert!(super::verify_readback(&expected, &expected).is_ok());
    }
}
