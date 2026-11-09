//! S4-27i4b: the paged `Stage4InterruptPumpPagedProbeV1` guest driven by the event
//! pump through the runner. The image is assembled here from the S4-27h layout
//! (independently of the loader; the constants mirror
//! `tools/build/build_stage4_paged_sti_hlt_probe.py` with the main code
//! `STI; HLT; HLT; HLT`). Expected evidence comes from the queue rules of S4-27i3 and
//! the Intel SDM (interrupt gate frame, A/D on use), not from the runner's output.

use serde_json::{Value, json};
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
};

const BASE: usize = 0x1000;
const IMAGE_BYTES: usize = 0xa000;
const FUTURE_NS: u64 = 250_000;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf()
}

/// Page tables, `STI; HLT; HLT; HLT`, the handler `MOV [RBX],EAX; IRETQ`, the gate of
/// vector 0x41 (target 0x402000, CS 8, type 0x8e) and the GDT.
fn image() -> Vec<u8> {
    let mut image = vec![0; IMAGE_BYTES];
    let mut put = |gpa: usize, bytes: &[u8]| {
        image[gpa - BASE..gpa - BASE + bytes.len()].copy_from_slice(bytes)
    };
    for (gpa, value) in [
        (0x1000_u64, 0x2023_u64),
        (0x2000, 0x3023),
        (0x3010, 0x4023),
        (0x3018, 0x5023),
        (0x4008, 0x6003),
        (0x4010, 0x7003),
        (0x5000, 0x8003),
        (0x5008, 0x9003),
        (0x5010, 0xfee0_0003),
        (0x5800, 0xa003),
    ] {
        put(gpa as usize, &value.to_le_bytes());
    }
    put(0x6000, &[0xfb, 0xf4, 0xf4, 0xf4]);
    put(0x7000, &[0x89, 0x03, 0x48, 0xcf]);
    let mut gate = Vec::new();
    gate.extend(0x2000_u16.to_le_bytes());
    gate.extend(8_u16.to_le_bytes());
    gate.extend([0, 0x8e]);
    gate.extend(0x0040_u16.to_le_bytes());
    gate.extend(0_u32.to_le_bytes());
    gate.extend([0; 4]);
    put(0x8000 + 0x41 * 16, &gate);
    let mut gdt = vec![0; 8];
    gdt.extend([0xff, 0xff, 0, 0, 0, 0x9b, 0xaf, 0]);
    gdt.extend([0xff, 0xff, 0, 0, 0, 0x93, 0xcf, 0]);
    put(0x9000, &gdt);
    image
}

fn write_image(root: &Path, name: &str, bytes: &[u8]) -> PathBuf {
    let dir = root.join(format!("work/stage4/{name}"));
    fs::create_dir_all(&dir).unwrap();
    let path = dir.join("probe.bin");
    fs::write(&path, bytes).unwrap();
    path
}

fn run(image: &Path, extra: &[&str]) -> (Output, Value) {
    let out = Command::new(env!("CARGO_BIN_EXE_od-runner"))
        .arg("--stage4-interrupt-pump-paged-probe")
        .arg(image)
        .args(extra)
        .output()
        .unwrap();
    let json = serde_json::from_slice(&out.stdout)
        .unwrap_or_else(|e| panic!("{e}: {}", String::from_utf8_lossy(&out.stdout)));
    (out, json)
}

fn names(x: &Value) -> Vec<String> {
    x["trace"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["event"].as_str().unwrap().to_string())
        .collect()
}

fn frame(rip: u64) -> Value {
    json!([format!("0x{rip:x}"), "0x8", "0x202", "0x700ff8", "0x10"])
}

/// What every successful run ends with: halted at an HLT with the handler returned.
fn assert_idle(x: &Value, final_rip: &str) {
    assert_eq!(x["profile"], "Stage4InterruptPumpPagedProbeV1");
    assert_eq!(x["status"], "guest_idle");
    assert_eq!(x["exit"], "idle_halt");
    assert_eq!(x["waiting"], true);
    assert_eq!(x["activity"], "Halted");
    assert_eq!(
        x["rip"],
        format!("0x{:016x}", u64::from_str_radix(final_rip, 16).unwrap())
    );
    assert_eq!(x["rflags"], "0x0000000000000202");
    assert_eq!(x["rsp"], "0x0000000000700ff8");
    assert_eq!(x["vectors"]["0x41"], json!({"irr": false, "isr": false}));
    assert_eq!(x["queue_len"], 0);
}

#[test]
fn queue_schedule_delivers_the_due_and_the_future_event() {
    let r = root();
    let path = write_image(&r, &format!("s4-27i4b-a-{}", std::process::id()), &image());
    let (out, x) = run(&path, &[]);
    assert!(out.status.success(), "{x}");
    assert_idle(&x, "401004");
    assert_eq!(x["mode"], "queue_schedule");
    // Phase 1: the request of the due event fails, the event stays, the guest did not move.
    assert_eq!(x["phase1"]["exit"], "rejected");
    assert!(
        x["phase1"]["error"]
            .as_str()
            .unwrap()
            .starts_with("QueueRequest(UnsupportedConfiguration")
    );
    assert_eq!(x["phase1"]["retired"], 0);
    assert_eq!(x["phase1"]["queue_len"], 2);
    assert_eq!(x["phase1"]["rip"], "0x0000000000401000");
    // Two wake-ups: the due event at 0 ns and the future one at its deadline.
    assert_eq!(x["wakeups"], 2);
    assert_eq!(
        (x["handler_returns"].as_u64(), x["eois"].as_u64()),
        (Some(2), Some(2))
    );
    assert_eq!(
        (x["time_ns"].as_u64(), x["coalesced"].as_u64()),
        (Some(FUTURE_NS), Some(0))
    );
    let entries = x["entries"].as_array().unwrap();
    for (entry, (time, rip)) in entries
        .iter()
        .zip([(0, 0x401002_u64), (FUTURE_NS, 0x401003)])
    {
        assert_eq!(entry["source"], "halt_wake");
        assert_eq!(entry["time_ns"], time);
        assert_eq!(entry["rip_after_entry"], "0x0000000000402000");
        // Gate frame: 0x700ff8 aligned down to 16 minus five qwords.
        assert_eq!(entry["rsp_after_entry"], "0x0000000000700fc8");
        assert_eq!(entry["rflags_after_entry"], "0x0000000000000002");
        assert_eq!(entry["frame_low_to_high"], frame(rip));
    }
    assert_eq!(
        names(&x),
        [
            "irr", "sti", "hlt", "entry", "eoi", "iretq", "hlt", "irr", "entry", "eoi", "iretq",
            "hlt"
        ]
    );
    // A/D (SDM Vol. 3A 4.8): A on use, D on the first write (frame, EOI store).
    assert_eq!(
        x["ptes_main_handler_idt_gdt_lapic_stack"],
        json!([
            "0x6023",
            "0x7023",
            "0x8023",
            "0x9023",
            "0xfee00063",
            "0xa063"
        ])
    );
}

#[test]
fn the_pop_before_request_injection_loses_the_due_event() {
    let r = root();
    let path = write_image(&r, &format!("s4-27i4b-i-{}", std::process::id()), &image());
    let (out, accepted) = run(&path, &[]);
    assert!(out.status.success());
    let (out, injected) = run(&path, &["--inject-pop-before-request"]);
    // The injected pump is idle too (a successful exit) but lost the due event.
    assert!(out.status.success(), "{injected}");
    assert_eq!(injected["injection"], "PopBeforeRequest");
    assert_idle(&injected, "401003");
    assert_eq!(injected["phase1"]["exit"], "rejected");
    assert_eq!(injected["phase1"]["queue_len"], 1);
    assert_eq!(injected["wakeups"], 1);
    assert_eq!(injected["handler_returns"], 1);
    assert_eq!(injected["time_ns"], FUTURE_NS);
    assert_eq!(
        names(&injected),
        ["sti", "hlt", "irr", "entry", "eoi", "iretq", "hlt"]
    );
    assert_ne!(
        (
            accepted["wakeups"].clone(),
            accepted["phase1"]["queue_len"].clone()
        ),
        (
            injected["wakeups"].clone(),
            injected["phase1"]["queue_len"].clone()
        )
    );
}

#[test]
fn caller_requests_wake_the_halted_cpu_and_a_late_request_after_idle() {
    let r = root();
    let path = write_image(&r, &format!("s4-27i4b-c-{}", std::process::id()), &image());
    // Raised before HLT retires: STI (IF=1, shadow), HLT in the shadow halts, the wake follows.
    for request in ["0x41@0", "0x41@1"] {
        let (out, x) = run(&path, &["--request", request]);
        assert!(out.status.success(), "{request}: {x}");
        assert_idle(&x, "401003");
        assert_eq!(x["mode"], "caller_requests");
        assert_eq!(x["phase1"], Value::Null);
        assert_eq!(
            (x["wakeups"].as_u64(), x["time_ns"].as_u64()),
            (Some(1), Some(0))
        );
        assert_eq!(x["entries"][0]["source"], "halt_wake");
        assert_eq!(x["entries"][0]["frame_low_to_high"], frame(0x401002));
        assert_eq!(names(&x).iter().filter(|n| *n == "entry").count(), 1);
    }
    // A request after the guest went idle is raised then and the run continues.
    let (out, x) = run(&path, &["--request", "0x41@50"]);
    assert!(out.status.success(), "{x}");
    assert_idle(&x, "401003");
    assert_eq!(x["wakeups"], 1);
    assert_eq!(x["requests"], json!([{"vector": 0x41, "after_quanta": 50}]));
}

#[test]
fn budget_and_input_errors_are_reported() {
    let r = root();
    let tag = format!("s4-27i4b-b-{}", std::process::id());
    let path = write_image(&r, &tag, &image());
    // Budget 3: phase 1 fails at once (no quantum), phase 2 runs three quanta.
    let (out, x) = run(&path, &["--budget", "3"]);
    assert_eq!(out.status.code(), Some(5));
    assert_eq!(x["status"], "work_limit");
    assert_eq!(x["quanta"], 3);

    for extra in [
        vec!["--request", "0x41"],
        vec!["--request", "0x41@1", "--inject-pop-before-request"],
        vec!["--budget", "1000000"],
        vec!["--unknown", "1"],
    ] {
        let (out, x) = run(&path, &extra);
        assert_eq!(out.status.code(), Some(3), "{extra:?}");
        assert_eq!(x["status"], "invalid_input");
    }

    let (out, x) = run(&r.join("work/stage4/does-not-exist.bin"), &[]);
    assert_eq!(out.status.code(), Some(3));
    assert_eq!(x["error"]["kind"], "file");

    // A raw image of the wrong size and one with a corrupt GDT code descriptor.
    let short = write_image(&r, &format!("{tag}-short"), &image()[..IMAGE_BYTES - 1]);
    let (out, x) = run(&short, &[]);
    assert_eq!(out.status.code(), Some(3));
    assert_eq!(x["error"]["kind"], "file");
    let mut bytes = image();
    bytes[0x9008 - BASE] ^= 1;
    let bad = write_image(&r, &format!("{tag}-bad"), &bytes);
    let (out, x) = run(&bad, &[]);
    assert_eq!(out.status.code(), Some(4));
    assert_eq!(x["status"], "loader_error");
}
