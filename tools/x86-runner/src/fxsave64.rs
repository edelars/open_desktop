//! Dedicated successful CPL3 FXSAVE64 oracle. Kept outside native_v2 admission.

use std::ffi::CStr;
use std::mem::{MaybeUninit, size_of};
use std::process::{Command, ExitCode};
use std::ptr;

use od_testkit::fxsave64::{
    COMPARISON_RANGES, EVIDENCE_VERSION, Fxsave64Case, Fxsave64Environment, Fxsave64Evidence,
    Fxsave64Hashes, Fxsave64Run, INSTRUCTION, compare_fxsave64,
};
use od_testkit::sha256_hex;

const CONTEXT_SIZE: usize = 1056;
const IMAGE_OFFSET: usize = 32;
const RIGHT_GUARD_OFFSET: usize = IMAGE_OFFSET + 512;
const CONTEXT_OFFSET: usize = RIGHT_GUARD_OFFSET + 32;

#[repr(C, align(16))]
struct Context {
    caller_xmm: [u8; 256],
    caller_mxcsr: u32,
    target_mxcsr: u32,
    input_xmm: [u8; 256],
    xmm_before: [u8; 256],
    xmm_after: [u8; 256],
    mxcsr_before: [u8; 4],
    mxcsr_after: [u8; 4],
    entry: u64,
}

#[repr(C, align(16))]
struct Shared {
    left_guard: [u8; 32],
    image: [u8; 512],
    right_guard: [u8; 32],
    context: Context,
}

const _: () = {
    assert!(size_of::<Context>() == CONTEXT_SIZE);
    assert!(std::mem::offset_of!(Context, input_xmm) == 264);
    assert!(std::mem::offset_of!(Context, xmm_before) == 520);
    assert!(std::mem::offset_of!(Context, xmm_after) == 776);
    assert!(std::mem::offset_of!(Context, mxcsr_before) == 1032);
    assert!(std::mem::offset_of!(Context, mxcsr_after) == 1036);
    assert!(std::mem::offset_of!(Context, entry) == 1040);
    assert!(std::mem::offset_of!(Shared, image) == IMAGE_OFFSET);
    assert!(std::mem::offset_of!(Shared, right_guard) == RIGHT_GUARD_OFFSET);
    assert!(std::mem::offset_of!(Shared, context) == CONTEXT_OFFSET);
};

unsafe extern "C" {
    fn od_fxsave64_sequence(context: *mut Context, image: *mut u8);
    fn od_fxsave64_sequence_end();
    fn od_fxsave64_instruction();
}

core::arch::global_asm!(
    ".text",
    ".globl od_fxsave64_sequence",
    ".globl od_fxsave64_instruction",
    ".type od_fxsave64_sequence,@function",
    "od_fxsave64_sequence:",
    // Preserve every SysV XMM register and MXCSR around this oracle call.
    "movdqu [rdi + 0], xmm0",
    "movdqu [rdi + 16], xmm1",
    "movdqu [rdi + 32], xmm2",
    "movdqu [rdi + 48], xmm3",
    "movdqu [rdi + 64], xmm4",
    "movdqu [rdi + 80], xmm5",
    "movdqu [rdi + 96], xmm6",
    "movdqu [rdi + 112], xmm7",
    "movdqu [rdi + 128], xmm8",
    "movdqu [rdi + 144], xmm9",
    "movdqu [rdi + 160], xmm10",
    "movdqu [rdi + 176], xmm11",
    "movdqu [rdi + 192], xmm12",
    "movdqu [rdi + 208], xmm13",
    "movdqu [rdi + 224], xmm14",
    "movdqu [rdi + 240], xmm15",
    "stmxcsr [rdi + 256]",
    "ldmxcsr [rdi + 260]",
    "movdqu xmm0, [rdi + 264]",
    "movdqu xmm1, [rdi + 280]",
    "movdqu xmm2, [rdi + 296]",
    "movdqu xmm3, [rdi + 312]",
    "movdqu xmm4, [rdi + 328]",
    "movdqu xmm5, [rdi + 344]",
    "movdqu xmm6, [rdi + 360]",
    "movdqu xmm7, [rdi + 376]",
    "movdqu xmm8, [rdi + 392]",
    "movdqu xmm9, [rdi + 408]",
    "movdqu xmm10, [rdi + 424]",
    "movdqu xmm11, [rdi + 440]",
    "movdqu xmm12, [rdi + 456]",
    "movdqu xmm13, [rdi + 472]",
    "movdqu xmm14, [rdi + 488]",
    "movdqu xmm15, [rdi + 504]",
    "movdqu [rdi + 520], xmm0",
    "movdqu [rdi + 536], xmm1",
    "movdqu [rdi + 552], xmm2",
    "movdqu [rdi + 568], xmm3",
    "movdqu [rdi + 584], xmm4",
    "movdqu [rdi + 600], xmm5",
    "movdqu [rdi + 616], xmm6",
    "movdqu [rdi + 632], xmm7",
    "movdqu [rdi + 648], xmm8",
    "movdqu [rdi + 664], xmm9",
    "movdqu [rdi + 680], xmm10",
    "movdqu [rdi + 696], xmm11",
    "movdqu [rdi + 712], xmm12",
    "movdqu [rdi + 728], xmm13",
    "movdqu [rdi + 744], xmm14",
    "movdqu [rdi + 760], xmm15",
    "stmxcsr [rdi + 1032]",
    "lea rax, [rip + od_fxsave64_instruction]",
    "mov [rdi + 1040], rax",
    "mov rax, rsi",
    "sub rax, 64",
    "od_fxsave64_instruction:",
    ".byte 0x48, 0x0f, 0xae, 0x40, 0x40",
    "od_fxsave64_marker:",
    ".byte 0xcc",
    "movdqu [rdi + 776], xmm0",
    "movdqu [rdi + 792], xmm1",
    "movdqu [rdi + 808], xmm2",
    "movdqu [rdi + 824], xmm3",
    "movdqu [rdi + 840], xmm4",
    "movdqu [rdi + 856], xmm5",
    "movdqu [rdi + 872], xmm6",
    "movdqu [rdi + 888], xmm7",
    "movdqu [rdi + 904], xmm8",
    "movdqu [rdi + 920], xmm9",
    "movdqu [rdi + 936], xmm10",
    "movdqu [rdi + 952], xmm11",
    "movdqu [rdi + 968], xmm12",
    "movdqu [rdi + 984], xmm13",
    "movdqu [rdi + 1000], xmm14",
    "movdqu [rdi + 1016], xmm15",
    "stmxcsr [rdi + 1036]",
    "movdqu xmm0, [rdi + 0]",
    "movdqu xmm1, [rdi + 16]",
    "movdqu xmm2, [rdi + 32]",
    "movdqu xmm3, [rdi + 48]",
    "movdqu xmm4, [rdi + 64]",
    "movdqu xmm5, [rdi + 80]",
    "movdqu xmm6, [rdi + 96]",
    "movdqu xmm7, [rdi + 112]",
    "movdqu xmm8, [rdi + 128]",
    "movdqu xmm9, [rdi + 144]",
    "movdqu xmm10, [rdi + 160]",
    "movdqu xmm11, [rdi + 176]",
    "movdqu xmm12, [rdi + 192]",
    "movdqu xmm13, [rdi + 208]",
    "movdqu xmm14, [rdi + 224]",
    "movdqu xmm15, [rdi + 240]",
    "ldmxcsr [rdi + 256]",
    "ret",
    "od_fxsave64_sequence_end:",
    ".size od_fxsave64_sequence, .-od_fxsave64_sequence",
);

const A_XMM: [[u8; 16]; 16] = [
    [
        0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0a, 0x0b, 0x0c, 0x0d, 0x0e,
        0x0f,
    ],
    [
        0x10, 0x11, 0x12, 0x13, 0x14, 0x15, 0x16, 0x17, 0x18, 0x19, 0x1a, 0x1b, 0x1c, 0x1d, 0x1e,
        0x1f,
    ],
    [
        0x20, 0x21, 0x22, 0x23, 0x24, 0x25, 0x26, 0x27, 0x28, 0x29, 0x2a, 0x2b, 0x2c, 0x2d, 0x2e,
        0x2f,
    ],
    [
        0x30, 0x31, 0x32, 0x33, 0x34, 0x35, 0x36, 0x37, 0x38, 0x39, 0x3a, 0x3b, 0x3c, 0x3d, 0x3e,
        0x3f,
    ],
    [
        0x40, 0x41, 0x42, 0x43, 0x44, 0x45, 0x46, 0x47, 0x48, 0x49, 0x4a, 0x4b, 0x4c, 0x4d, 0x4e,
        0x4f,
    ],
    [
        0x50, 0x51, 0x52, 0x53, 0x54, 0x55, 0x56, 0x57, 0x58, 0x59, 0x5a, 0x5b, 0x5c, 0x5d, 0x5e,
        0x5f,
    ],
    [
        0x60, 0x61, 0x62, 0x63, 0x64, 0x65, 0x66, 0x67, 0x68, 0x69, 0x6a, 0x6b, 0x6c, 0x6d, 0x6e,
        0x6f,
    ],
    [
        0x70, 0x71, 0x72, 0x73, 0x74, 0x75, 0x76, 0x77, 0x78, 0x79, 0x7a, 0x7b, 0x7c, 0x7d, 0x7e,
        0x7f,
    ],
    [
        0x80, 0x81, 0x82, 0x83, 0x84, 0x85, 0x86, 0x87, 0x88, 0x89, 0x8a, 0x8b, 0x8c, 0x8d, 0x8e,
        0x8f,
    ],
    [
        0x90, 0x91, 0x92, 0x93, 0x94, 0x95, 0x96, 0x97, 0x98, 0x99, 0x9a, 0x9b, 0x9c, 0x9d, 0x9e,
        0x9f,
    ],
    [
        0xa0, 0xa1, 0xa2, 0xa3, 0xa4, 0xa5, 0xa6, 0xa7, 0xa8, 0xa9, 0xaa, 0xab, 0xac, 0xad, 0xae,
        0xaf,
    ],
    [
        0xb0, 0xb1, 0xb2, 0xb3, 0xb4, 0xb5, 0xb6, 0xb7, 0xb8, 0xb9, 0xba, 0xbb, 0xbc, 0xbd, 0xbe,
        0xbf,
    ],
    [
        0xc0, 0xc1, 0xc2, 0xc3, 0xc4, 0xc5, 0xc6, 0xc7, 0xc8, 0xc9, 0xca, 0xcb, 0xcc, 0xcd, 0xce,
        0xcf,
    ],
    [
        0xd0, 0xd1, 0xd2, 0xd3, 0xd4, 0xd5, 0xd6, 0xd7, 0xd8, 0xd9, 0xda, 0xdb, 0xdc, 0xdd, 0xde,
        0xdf,
    ],
    [
        0xe0, 0xe1, 0xe2, 0xe3, 0xe4, 0xe5, 0xe6, 0xe7, 0xe8, 0xe9, 0xea, 0xeb, 0xec, 0xed, 0xee,
        0xef,
    ],
    [
        0xf0, 0xf1, 0xf2, 0xf3, 0xf4, 0xf5, 0xf6, 0xf7, 0xf8, 0xf9, 0xfa, 0xfb, 0xfc, 0xfd, 0xfe,
        0xff,
    ],
];
const B_XMM: [[u8; 16]; 16] = [
    [
        0xff, 0xfe, 0xfd, 0xfc, 0xfb, 0xfa, 0xf9, 0xf8, 0xf7, 0xf6, 0xf5, 0xf4, 0xf3, 0xf2, 0xf1,
        0xf0,
    ],
    [
        0xef, 0xee, 0xed, 0xec, 0xeb, 0xea, 0xe9, 0xe8, 0xe7, 0xe6, 0xe5, 0xe4, 0xe3, 0xe2, 0xe1,
        0xe0,
    ],
    [
        0xdf, 0xde, 0xdd, 0xdc, 0xdb, 0xda, 0xd9, 0xd8, 0xd7, 0xd6, 0xd5, 0xd4, 0xd3, 0xd2, 0xd1,
        0xd0,
    ],
    [
        0xcf, 0xce, 0xcd, 0xcc, 0xcb, 0xca, 0xc9, 0xc8, 0xc7, 0xc6, 0xc5, 0xc4, 0xc3, 0xc2, 0xc1,
        0xc0,
    ],
    [
        0xbf, 0xbe, 0xbd, 0xbc, 0xbb, 0xba, 0xb9, 0xb8, 0xb7, 0xb6, 0xb5, 0xb4, 0xb3, 0xb2, 0xb1,
        0xb0,
    ],
    [
        0xaf, 0xae, 0xad, 0xac, 0xab, 0xaa, 0xa9, 0xa8, 0xa7, 0xa6, 0xa5, 0xa4, 0xa3, 0xa2, 0xa1,
        0xa0,
    ],
    [
        0x9f, 0x9e, 0x9d, 0x9c, 0x9b, 0x9a, 0x99, 0x98, 0x97, 0x96, 0x95, 0x94, 0x93, 0x92, 0x91,
        0x90,
    ],
    [
        0x8f, 0x8e, 0x8d, 0x8c, 0x8b, 0x8a, 0x89, 0x88, 0x87, 0x86, 0x85, 0x84, 0x83, 0x82, 0x81,
        0x80,
    ],
    [
        0x7f, 0x7e, 0x7d, 0x7c, 0x7b, 0x7a, 0x79, 0x78, 0x77, 0x76, 0x75, 0x74, 0x73, 0x72, 0x71,
        0x70,
    ],
    [
        0x6f, 0x6e, 0x6d, 0x6c, 0x6b, 0x6a, 0x69, 0x68, 0x67, 0x66, 0x65, 0x64, 0x63, 0x62, 0x61,
        0x60,
    ],
    [
        0x5f, 0x5e, 0x5d, 0x5c, 0x5b, 0x5a, 0x59, 0x58, 0x57, 0x56, 0x55, 0x54, 0x53, 0x52, 0x51,
        0x50,
    ],
    [
        0x4f, 0x4e, 0x4d, 0x4c, 0x4b, 0x4a, 0x49, 0x48, 0x47, 0x46, 0x45, 0x44, 0x43, 0x42, 0x41,
        0x40,
    ],
    [
        0x3f, 0x3e, 0x3d, 0x3c, 0x3b, 0x3a, 0x39, 0x38, 0x37, 0x36, 0x35, 0x34, 0x33, 0x32, 0x31,
        0x30,
    ],
    [
        0x2f, 0x2e, 0x2d, 0x2c, 0x2b, 0x2a, 0x29, 0x28, 0x27, 0x26, 0x25, 0x24, 0x23, 0x22, 0x21,
        0x20,
    ],
    [
        0x1f, 0x1e, 0x1d, 0x1c, 0x1b, 0x1a, 0x19, 0x18, 0x17, 0x16, 0x15, 0x14, 0x13, 0x12, 0x11,
        0x10,
    ],
    [
        0x0f, 0x0e, 0x0d, 0x0c, 0x0b, 0x0a, 0x09, 0x08, 0x07, 0x06, 0x05, 0x04, 0x03, 0x02, 0x01,
        0x00,
    ],
];

fn xmm_literal(case: Fxsave64Case, register: usize) -> [u8; 16] {
    match case {
        Fxsave64Case::A => A_XMM[register],
        Fxsave64Case::B => B_XMM[register],
    }
}

pub fn run_cli(args: &[String]) -> ExitCode {
    if !args.is_empty() {
        eprintln!("usage: od-x86-runner --fxsave64-oracle");
        return ExitCode::from(3);
    }
    match run_oracle() {
        Ok(mut evidence) => {
            let comparison = compare_fxsave64(&evidence);
            if !comparison.passed && evidence.status == "observed" {
                evidence.status = "rejected".into();
                evidence.failure_reason = Some(comparison.errors.join("; "));
            }
            let output_dir = std::path::Path::new("work/stage2/s2-39k-fxsave64");
            if let Err(error) = std::fs::create_dir_all(output_dir) {
                eprintln!("could not create ignored raw evidence directory: {error}");
                return ExitCode::from(5);
            }
            let stamp = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|duration| duration.as_nanos())
                .unwrap_or_default();
            let output_path = output_dir.join(format!("fxsave64-{stamp}.json"));
            let bytes = match serde_json::to_vec_pretty(&evidence) {
                Ok(bytes) => bytes,
                Err(error) => {
                    eprintln!("could not serialize raw evidence: {error}");
                    return ExitCode::from(5);
                }
            };
            if let Err(error) = std::fs::write(&output_path, bytes) {
                eprintln!("could not persist raw evidence: {error}");
                return ExitCode::from(5);
            }
            println!("raw evidence: {}", output_path.display());
            if comparison.passed {
                ExitCode::SUCCESS
            } else {
                for error in comparison.errors {
                    eprintln!("oracle comparison failed: {error}");
                }
                ExitCode::from(5)
            }
        }
        Err(error) => {
            eprintln!("FXSAVE64 oracle unavailable/not_run: {error}");
            ExitCode::from(4)
        }
    }
}

fn run_oracle() -> Result<Fxsave64Evidence, String> {
    let allowed = allowed_cpus()?;
    let cpu = allowed
        .first()
        .copied()
        .ok_or("no CPU available for affinity pinning")?;
    let _affinity_guard = pin_current_cpu(cpu)?;
    let cpuid0 = std::arch::x86_64::__cpuid(0);
    let cpuid1 = std::arch::x86_64::__cpuid(1);
    let fxsr = cpuid1.edx & (1 << 24) != 0;
    let sse = cpuid1.edx & (1 << 25) != 0;
    if !fxsr || !sse {
        return Err(format!(
            "host lacks required FXSR/SSE (FXSR={fxsr}, SSE={sse})"
        ));
    }
    let rustc = Command::new("rustc")
        .arg("--version")
        .output()
        .map_err(|error| format!("could not record rustc version: {error}"))?;
    let toolchain = String::from_utf8_lossy(&rustc.stdout).trim().to_owned();
    let executable = std::env::current_exe().map_err(|error| error.to_string())?;
    let executable_bytes = std::fs::read(&executable).map_err(|error| error.to_string())?;
    let source_revision = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .output()
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_owned())
        .unwrap_or_else(|_| "unknown".into());
    let uts = uname_strings()?;
    let literal_bytes = literal_table_bytes();
    let emitted_code_address = od_fxsave64_sequence as *const () as usize;
    let emitted_end = od_fxsave64_sequence_end as *const () as usize;
    if emitted_end <= emitted_code_address {
        return Err("invalid emitted sequence bounds".into());
    }
    let emitted_bytes = unsafe {
        std::slice::from_raw_parts(
            emitted_code_address as *const u8,
            emitted_end - emitted_code_address,
        )
        .to_vec()
    };
    let test_instruction_offset = (od_fxsave64_instruction as *const () as usize as u64)
        .checked_sub(emitted_code_address as u64)
        .ok_or("test entry precedes emitted function")? as usize;
    let mut runs = Vec::with_capacity(4);
    let mut failure_reason = None;
    for (index, case) in [
        Fxsave64Case::A,
        Fxsave64Case::B,
        Fxsave64Case::A,
        Fxsave64Case::B,
    ]
    .into_iter()
    .enumerate()
    {
        let run = match execute_case(case, index as u8, cpu) {
            Ok(run) => run,
            Err(error) => {
                if runs.is_empty() {
                    return Err(error);
                }
                failure_reason = Some(format!(
                    "run {index}: could not complete tracee setup: {error}"
                ));
                break;
            }
        };
        if let Some(error) = &run.execution_error {
            failure_reason = Some(format!("run {index}: {error}"));
            runs.push(run);
            break;
        }
        runs.push(run);
    }
    Ok(Fxsave64Evidence {
        schema_version: EVIDENCE_VERSION,
        status: if failure_reason.is_some() { "rejected" } else { "observed" }.into(),
        failure_reason,
        source_revision,
        hashes: Fxsave64Hashes {
            harness_source_sha256: sha256_hex(include_bytes!("fxsave64.rs")),
            literal_table_sha256: sha256_hex(&literal_bytes),
            emitted_bytes_sha256: sha256_hex(&emitted_bytes),
            executable_sha256: sha256_hex(&executable_bytes),
            comparator_sha256: sha256_hex(include_bytes!("../../../crates/od-testkit/src/fxsave64.rs")),
            protocol_sha256: sha256_hex(include_bytes!("../../../docs/testing/S2_39J_FXSAVE64_ORACLE.md")),
        },
        environment: Fxsave64Environment {
            cpu_vendor: cpuid_vendor(cpuid0),
            cpu_family: ((cpuid1.eax >> 8) & 0xf) + ((cpuid1.eax >> 20) & 0xff),
            cpu_model: ((cpuid1.eax >> 4) & 0xf) | ((cpuid1.eax >> 12) & 0xf0),
            cpu_stepping: cpuid1.eax & 0xf,
            cpuid_fxsr: fxsr,
            cpuid_sse: sse,
            cpuid_leaf1_eax: cpuid1.eax,
            cpuid_leaf1_edx: cpuid1.edx,
            os: uts.0,
            kernel: uts.1,
            virtualization: if cpuid1.ecx & (1 << 31) != 0 { "hypervisor-present" } else { "no-hypervisor-bit" }.into(),
            execution_kind: "unknown".into(),
            affinity_cpu: cpu as u32,
            toolchain,
            build_flags: format!(
                "profile={}; target=x86_64-unknown-linux-gnu; rustflags={}",
                if cfg!(debug_assertions) { "debug" } else { "release" },
                option_env!("CARGO_ENCODED_RUSTFLAGS").unwrap_or("none")
            ),
        },
        instruction_bytes: INSTRUCTION.to_vec(),
        instruction_length: 5,
        emitted_code_address: emitted_code_address as u64,
        emitted_bytes,
        test_instruction_offset,
        comparison_ranges: COMPARISON_RANGES
            .iter()
            .map(|range| (*range).into())
            .collect(),
        stop_mechanism: "PTRACE_TRACEME child; INT3 at E+5; raw PTRACE_GETREGS RIP must be E+6; marker adjustment=1; PTRACE_GETSIGINFO validates marker signal source".into(),
        retirement_counter_claimed: false,
        runs,
    })
}

fn execute_case(case: Fxsave64Case, repetition: u8, cpu: usize) -> Result<Fxsave64Run, String> {
    let length = size_of::<Shared>();
    let ptr = unsafe {
        libc::mmap(
            ptr::null_mut(),
            length,
            libc::PROT_READ | libc::PROT_WRITE,
            libc::MAP_SHARED | libc::MAP_ANON,
            -1,
            0,
        )
    };
    if ptr == libc::MAP_FAILED {
        return Err("mmap shared evidence buffer failed".into());
    }
    let shared = ptr.cast::<Shared>();
    unsafe {
        ptr::write_bytes(shared.cast::<u8>(), 0, length);
    }
    unsafe {
        (*shared).left_guard.fill(case.left_guard());
        (*shared).image.fill(case.image_canary());
        (*shared).right_guard.fill(case.right_guard());
        (*shared).context.target_mxcsr = u32::from_le_bytes(case.mxcsr());
        for register in 0..16 {
            (&mut (*shared).context.input_xmm)[register * 16..(register + 1) * 16]
                .copy_from_slice(&xmm_literal(case, register));
        }
    }
    let initial_image = unsafe { (*shared).image.to_vec() };
    let left_guard_before = unsafe { (*shared).left_guard.to_vec() };
    let right_guard_before = unsafe { (*shared).right_guard.to_vec() };
    let image_ptr = unsafe { ptr::addr_of_mut!((*shared).image).cast::<u8>() };
    let context_ptr = unsafe { ptr::addr_of_mut!((*shared).context) };
    let child = unsafe { libc::fork() };
    if child < 0 {
        unsafe {
            libc::munmap(ptr, length);
        }
        return Err("fork failed".into());
    }
    if child == 0 {
        let mut mask: libc::cpu_set_t = unsafe { std::mem::zeroed() };
        unsafe {
            libc::CPU_ZERO(&mut mask);
            libc::CPU_SET(cpu, &mut mask);
        }
        if unsafe { libc::sched_setaffinity(0, size_of::<libc::cpu_set_t>(), &mask) } != 0 {
            unsafe {
                libc::_exit(90);
            }
        }
        if unsafe {
            libc::ptrace(
                libc::PTRACE_TRACEME,
                0,
                ptr::null_mut::<libc::c_void>(),
                ptr::null_mut::<libc::c_void>(),
            )
        } < 0
        {
            unsafe {
                libc::_exit(91);
            }
        }
        unsafe {
            libc::raise(libc::SIGSTOP);
            od_fxsave64_sequence(context_ptr, image_ptr);
            libc::_exit(0);
        }
    }
    let mut status = 0;
    let mut raw_rip = None;
    let mut raw_rax = None;
    let mut signal_code = None;
    let mut marker_valid = false;
    let mut failure = None;
    let mut child_finished = false;
    let mut status_valid = false;
    if unsafe { libc::waitpid(child, &mut status, 0) } != child {
        failure = Some("waitpid failed before initial tracee stop".to_string());
    } else if failure.is_none() {
        status_valid = true;
    }
    if failure.is_none() && (!libc::WIFSTOPPED(status) || libc::WSTOPSIG(status) != libc::SIGSTOP) {
        failure = Some("tracee did not reach initial SIGSTOP".into());
    } else if failure.is_none()
        && unsafe {
            libc::ptrace(
                libc::PTRACE_CONT,
                child,
                ptr::null_mut::<libc::c_void>(),
                ptr::null_mut::<libc::c_void>(),
            )
        } < 0
    {
        failure = Some("PTRACE_CONT before instruction failed".into());
    } else if failure.is_none() && unsafe { libc::waitpid(child, &mut status, 0) } != child {
        failure = Some("waitpid failed while awaiting completion marker".into());
    } else if failure.is_none() {
        status_valid = true;
    }
    if failure.is_none() && libc::WIFEXITED(status) {
        child_finished = true;
        failure = Some(format!(
            "tracee exited before completion marker with status {}",
            libc::WEXITSTATUS(status)
        ));
    } else if failure.is_none() && libc::WIFSIGNALED(status) {
        child_finished = true;
        failure = Some(format!(
            "tracee was terminated before completion marker by signal {}",
            libc::WTERMSIG(status)
        ));
    } else if failure.is_none() && !libc::WIFSTOPPED(status) {
        failure = Some("tracee wait status was neither stopped nor exited".into());
    } else if failure.is_none() {
        let stop_signal = libc::WSTOPSIG(status);
        let mut regs = MaybeUninit::<libc::user_regs_struct>::zeroed();
        if unsafe {
            libc::ptrace(
                libc::PTRACE_GETREGS,
                child,
                ptr::null_mut::<libc::c_void>(),
                regs.as_mut_ptr().cast::<libc::c_void>(),
            )
        } == 0
        {
            let regs = unsafe { regs.assume_init() };
            raw_rip = Some(regs.rip);
            raw_rax = Some(regs.rax);
        }
        if stop_signal != libc::SIGTRAP {
            failure = Some(format!(
                "tracee stopped before completion marker with signal {stop_signal}"
            ));
        } else if raw_rip.is_none() {
            failure = Some("PTRACE_GETREGS failed at completion stop".into());
        } else {
            let mut info = MaybeUninit::<libc::siginfo_t>::zeroed();
            if unsafe {
                libc::ptrace(
                    libc::PTRACE_GETSIGINFO,
                    child,
                    ptr::null_mut::<libc::c_void>(),
                    info.as_mut_ptr().cast::<libc::c_void>(),
                )
            } < 0
            {
                failure = Some("PTRACE_GETSIGINFO failed at completion stop".into());
            } else {
                let info = unsafe { info.assume_init() };
                signal_code = Some(info.si_code);
                let entry = unsafe { (*context_ptr).entry };
                let expected_rax = image_ptr as u64 - 0x40;
                if info.si_signo != libc::SIGTRAP || ![0, 1, 0x80].contains(&info.si_code) {
                    failure = Some(format!(
                        "unexpected stop signal source/code {}/{}",
                        info.si_signo, info.si_code
                    ));
                } else if raw_rax != Some(expected_rax) {
                    failure = Some(format!("raw RAX {:?} != D-0x40 {expected_rax:#x}", raw_rax));
                } else if raw_rip != Some(entry + 6) {
                    failure = Some(format!(
                        "raw stop RIP {:?} != E+6 {:#x}",
                        raw_rip,
                        entry + 6
                    ));
                } else {
                    let emitted = unsafe { std::slice::from_raw_parts(entry as *const u8, 6) };
                    if emitted != [INSTRUCTION.as_slice(), &[0xcc]].concat() {
                        failure = Some("emitted test/marker bytes differ from protocol".into());
                    } else if unsafe {
                        libc::ptrace(
                            libc::PTRACE_CONT,
                            child,
                            ptr::null_mut::<libc::c_void>(),
                            ptr::null_mut::<libc::c_void>(),
                        )
                    } < 0
                    {
                        failure = Some("PTRACE_CONT after marker failed".into());
                    } else if unsafe { libc::waitpid(child, &mut status, 0) } != child {
                        failure = Some("waitpid failed after completion marker".into());
                    } else {
                        status_valid = true;
                        if libc::WIFEXITED(status) && libc::WEXITSTATUS(status) == 0 {
                            child_finished = true;
                            marker_valid = true;
                        } else {
                            child_finished = libc::WIFEXITED(status) || libc::WIFSIGNALED(status);
                            failure = Some(
                                "tracee had unexpected exit, stop, or signal after observations"
                                    .into(),
                            );
                        }
                    }
                }
            }
        }
    }
    if failure.is_some() && status_valid && libc::WIFSTOPPED(status) {
        let mut regs = MaybeUninit::<libc::user_regs_struct>::zeroed();
        if unsafe {
            libc::ptrace(
                libc::PTRACE_GETREGS,
                child,
                ptr::null_mut::<libc::c_void>(),
                regs.as_mut_ptr().cast::<libc::c_void>(),
            )
        } == 0
        {
            let regs = unsafe { regs.assume_init() };
            raw_rip = Some(regs.rip);
            raw_rax = Some(regs.rax);
        }
        let mut info = MaybeUninit::<libc::siginfo_t>::zeroed();
        if unsafe {
            libc::ptrace(
                libc::PTRACE_GETSIGINFO,
                child,
                ptr::null_mut::<libc::c_void>(),
                info.as_mut_ptr().cast::<libc::c_void>(),
            )
        } == 0
        {
            let info = unsafe { info.assume_init() };
            signal_code = Some(info.si_code);
        }
    }
    if failure.is_some() && !child_finished {
        unsafe {
            libc::kill(child, libc::SIGKILL);
            libc::waitpid(child, ptr::null_mut(), 0);
        }
    }
    let shared = unsafe { &*shared };
    let d = image_ptr as u64;
    let code_entry = if shared.context.entry == 0 {
        od_fxsave64_instruction as *const () as usize as u64
    } else {
        shared.context.entry
    };
    let code_permissions = mapping_permissions(code_entry).unwrap_or_else(|error| {
        failure.get_or_insert_with(|| format!("could not record code mapping: {error}"));
        "unknown".into()
    });
    let data_permissions = mapping_permissions(context_ptr as u64).unwrap_or_else(|error| {
        failure.get_or_insert_with(|| format!("could not record data mapping: {error}"));
        "unknown".into()
    });
    let run = Fxsave64Run {
        case,
        repetition,
        code_address: code_entry,
        input_address: context_ptr as u64 + 260,
        pre_observation_address: context_ptr as u64 + 520,
        pre_mxcsr_address: context_ptr as u64 + 1032,
        post_observation_address: context_ptr as u64 + 776,
        post_mxcsr_address: context_ptr as u64 + 1036,
        image_address: d,
        left_guard_address: d - 32,
        right_guard_address: d + 512,
        rax: raw_rax.unwrap_or(0),
        code_permissions: code_permissions.clone(),
        input_permissions: data_permissions.clone(),
        image_permissions: data_permissions.clone(),
        guard_permissions: data_permissions,
        entry_rip: code_entry,
        raw_stop_rip: raw_rip,
        stop_rip_adjustment: 1,
        observed_rip: if marker_valid {
            raw_rip.and_then(|rip| rip.checked_sub(1))
        } else {
            None
        },
        completion_marker: marker_valid,
        execution_error: failure.clone(),
        exit_status: if status_valid && libc::WIFEXITED(status) {
            Some(libc::WEXITSTATUS(status))
        } else {
            None
        },
        signal: if status_valid && libc::WIFSIGNALED(status) {
            Some(libc::WTERMSIG(status))
        } else if status_valid && libc::WIFSTOPPED(status) && !marker_valid {
            Some(libc::WSTOPSIG(status))
        } else {
            None
        },
        signal_code,
        initial_image,
        image: shared.image.to_vec(),
        left_guard_before,
        left_guard_after: shared.left_guard.to_vec(),
        right_guard_before,
        right_guard_after: shared.right_guard.to_vec(),
        xmm_before: shared.context.xmm_before.to_vec(),
        xmm_after: shared.context.xmm_after.to_vec(),
        mxcsr_before: shared.context.mxcsr_before.to_vec(),
        mxcsr_after: shared.context.mxcsr_after.to_vec(),
        mxcsr_mask_raw: shared.image[28..32].to_vec(),
    };
    unsafe {
        libc::munmap(ptr, length);
    }
    Ok(run)
}

fn allowed_cpus() -> Result<Vec<usize>, String> {
    let mut set: libc::cpu_set_t = unsafe { std::mem::zeroed() };
    if unsafe { libc::sched_getaffinity(0, size_of::<libc::cpu_set_t>(), &mut set) } != 0 {
        return Err("sched_getaffinity failed".into());
    }
    Ok((0..libc::CPU_SETSIZE as usize)
        .filter(|cpu| unsafe { libc::CPU_ISSET(*cpu, &set) })
        .collect())
}

struct AffinityGuard {
    original: libc::cpu_set_t,
}

impl Drop for AffinityGuard {
    fn drop(&mut self) {
        unsafe {
            libc::sched_setaffinity(0, size_of::<libc::cpu_set_t>(), &self.original);
        }
    }
}

fn pin_current_cpu(cpu: usize) -> Result<AffinityGuard, String> {
    let mut original: libc::cpu_set_t = unsafe { std::mem::zeroed() };
    if unsafe { libc::sched_getaffinity(0, size_of::<libc::cpu_set_t>(), &mut original) } != 0 {
        return Err("sched_getaffinity before pinning failed".into());
    }
    let mut selected: libc::cpu_set_t = unsafe { std::mem::zeroed() };
    unsafe {
        libc::CPU_ZERO(&mut selected);
        libc::CPU_SET(cpu, &mut selected);
    }
    if unsafe { libc::sched_setaffinity(0, size_of::<libc::cpu_set_t>(), &selected) } != 0 {
        return Err("could not pin runner to selected CPU".into());
    }
    Ok(AffinityGuard { original })
}

fn mapping_permissions(address: u64) -> Result<String, String> {
    let maps = std::fs::read_to_string("/proc/self/maps")
        .map_err(|error| format!("could not record process mappings: {error}"))?;
    for line in maps.lines() {
        let mut fields = line.split_whitespace();
        let Some(range) = fields.next() else { continue };
        let Some(permissions) = fields.next() else {
            continue;
        };
        let Some((start, end)) = range.split_once('-') else {
            continue;
        };
        let start = u64::from_str_radix(start, 16).map_err(|error| error.to_string())?;
        let end = u64::from_str_radix(end, 16).map_err(|error| error.to_string())?;
        if (start..end).contains(&address) {
            return Ok(permissions.to_owned());
        }
    }
    Err(format!(
        "address {address:#x} is absent from /proc/self/maps"
    ))
}

fn uname_strings() -> Result<(String, String), String> {
    let mut uts = MaybeUninit::<libc::utsname>::zeroed();
    if unsafe { libc::uname(uts.as_mut_ptr()) } != 0 {
        return Err("uname failed".into());
    }
    let uts = unsafe { uts.assume_init() };
    Ok(unsafe {
        (
            CStr::from_ptr(uts.sysname.as_ptr())
                .to_string_lossy()
                .into_owned(),
            CStr::from_ptr(uts.release.as_ptr())
                .to_string_lossy()
                .into_owned(),
        )
    })
}

fn cpuid_vendor(leaf: std::arch::x86_64::CpuidResult) -> String {
    let mut bytes = Vec::with_capacity(12);
    bytes.extend_from_slice(&leaf.ebx.to_le_bytes());
    bytes.extend_from_slice(&leaf.edx.to_le_bytes());
    bytes.extend_from_slice(&leaf.ecx.to_le_bytes());
    String::from_utf8_lossy(&bytes).into_owned()
}

fn literal_table_bytes() -> Vec<u8> {
    let mut bytes = Vec::with_capacity(520);
    for value in A_XMM.iter().chain(B_XMM.iter()) {
        bytes.extend_from_slice(value);
    }
    bytes.extend_from_slice(&Fxsave64Case::A.mxcsr());
    bytes.extend_from_slice(&Fxsave64Case::B.mxcsr());
    bytes
}
