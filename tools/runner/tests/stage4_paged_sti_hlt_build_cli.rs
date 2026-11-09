use serde_json::{Value, json};
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
};
const MAIN: u64 = 0x40_1000;
const HANDLER: u64 = 0x40_2000;
const RSP0: u64 = 0x70_0ff8;
const FRAME: u64 = 0x70_0fc8;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf()
}
fn build(root: &Path, name: &str) -> PathBuf {
    let out = Command::new("python3")
        .current_dir(root)
        .args([
            "tools/build/build_stage4_paged_sti_hlt_probe.py",
            "--output-dir",
            &format!("work/stage4/{name}"),
        ])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    root.join(format!("work/stage4/{name}/paged_sti_hlt_probe.bin"))
}
fn run(image: &Path, budget: u16, inject: Option<&str>) -> Output {
    let mut c = Command::new(env!("CARGO_BIN_EXE_od-runner"));
    c.arg("--stage4-paged-sti-hlt-probe")
        .arg(image)
        .args(["--budget", &budget.to_string()]);
    if let Some(flag) = inject {
        c.arg(flag);
    }
    c.output().unwrap()
}
fn json(o: &Output) -> Value {
    serde_json::from_slice(&o.stdout).unwrap()
}
fn hex(v: u64) -> String {
    format!("0x{v:016x}")
}
fn names(x: &Value) -> Vec<String> {
    x["event_trace"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| {
            e["name"]
                .as_str()
                .unwrap()
                .split(':')
                .next()
                .unwrap()
                .to_string()
        })
        .collect()
}

#[test]
fn real_paged_sti_hlt_artifact_is_reproducible_and_guest_trace_is_exact() {
    let r = root();
    let tag = format!("s4-27i4a-{}", std::process::id());
    let a = build(&r, &format!("{tag}-a"));
    let a2 = build(&r, &format!("{tag}-a2"));
    let bytes = fs::read(&a).unwrap();
    let manifest: Value = serde_json::from_slice(
        &fs::read(r.join(format!("work/stage4/{tag}-a/build_manifest.json"))).unwrap(),
    )
    .unwrap();
    assert_eq!(manifest["profile"], "Stage4PagedStiHltProbeV1");
    assert_eq!(manifest["layout"]["main_code"]["bytes"], "fbf4f4");
    assert_eq!(manifest["layout"]["handler_code"]["bytes"], "890348cf");
    assert_eq!(manifest["layout"]["idt"]["gate_vector"], 0x41);
    assert_eq!(
        manifest["layout"]["scenarios"]["wake_from_hlt"]["frame_low_to_high"][0],
        "rip=0x401002"
    );
    assert_eq!(bytes, fs::read(a2).unwrap());

    // (rip, rsp, rflags, halted, irr, isr) after each prefix of the seven steps.
    let expected = [
        (MAIN, RSP0, "0x2", false, false, false),
        (MAIN + 1, RSP0, "0x202", false, false, false),
        (MAIN + 2, RSP0, "0x202", true, false, false),
        (MAIN + 2, RSP0, "0x202", true, false, false),
        (HANDLER, FRAME, "0x2", false, false, true),
        (HANDLER + 2, FRAME, "0x2", false, false, false),
        (MAIN + 2, RSP0, "0x202", false, false, false),
        (MAIN + 3, RSP0, "0x202", true, false, false),
    ];
    for budget in 0..=7_u16 {
        let out = run(&a, budget, None);
        let x = json(&out);
        let (rip, rsp, rflags, halted, irr, isr) = expected[budget as usize];
        assert_eq!(x["profile"], "Stage4PagedStiHltProbeV1");
        assert_eq!(x["steps"], budget);
        assert_eq!(x["event_trace"].as_array().unwrap().len(), budget as usize);
        let g = &x["guest_state"];
        assert_eq!(g["rip"], hex(rip), "budget {budget}");
        assert_eq!(g["rsp"], hex(rsp), "budget {budget}");
        assert_eq!(g["rflags"], rflags, "budget {budget}");
        assert_eq!(g["halted"], halted, "budget {budget}");
        assert_eq!(g["irr"], irr, "budget {budget}");
        assert_eq!(g["isr"], isr, "budget {budget}");
        assert_eq!(x["accepted"], budget == 7);
        if budget == 7 {
            assert!(out.status.success());
            assert_eq!(x["stop_reason"], "case_complete");
            assert_eq!(x["status"], "completed");
            assert_eq!(x["retired"], 5);
            assert_eq!(x["vector"], "0x41");
            assert!(x["failed_checks"].as_array().unwrap().is_empty());
            assert_eq!(
                names(&x),
                ["sti", "hlt", "idle poll", "wake", "eoi", "IRETQ", "resume"]
            );
            let t = &x["event_trace"];
            assert_eq!(t[0]["shadow_after"], true);
            assert_eq!(t[1]["halted_after"], true);
            assert_eq!(t[1]["shadow_after"], false);
            assert_eq!(t[2]["exit"], "idle");
            assert_eq!(t[3]["exit"], "entered");
            assert_eq!(t[3]["if_after"], false);
            assert_eq!(t[3]["isr_after"], true);
            assert_eq!(
                t[3]["frame_low_to_high"],
                json!(["0x401002", "0x8", "0x202", "0x700ff8", "0x10"])
            );
            assert_eq!(t[3]["ptes_after"]["stack"], "0xa063");
            assert_eq!(t[3]["ptes_after"]["idt"], "0x8023");
            assert_eq!(t[3]["ptes_after"]["gdt"], "0x9023");
            assert_eq!(t[4]["ptes_after"]["lapic"], "0xfee00063");
            assert_eq!(t[4]["isr_after"], false);
            assert_eq!(t[5]["rsp_after"], hex(RSP0));
            assert_eq!(t[5]["if_after"], true);
            assert_eq!(x["ptes"]["main"], "0x6023");
            assert_eq!(x["ptes"]["handler"], "0x7023");
            assert_eq!(x["ptes"]["lapic"], "0xfee00063");
        } else {
            assert_eq!(out.status.code(), Some(5));
            assert_eq!(x["stop_reason"], "budget_exhausted");
        }
    }

    // Injection 1: gate P=0 -> refused before any commit, the CPU stays halted, IRR is kept.
    let o = run(&a, 16, Some("--inject-idt-not-present"));
    let x = json(&o);
    assert_eq!(o.status.code(), Some(5));
    assert_eq!(x["injection"], "idt_not_present");
    assert_eq!(x["stop_reason"], "delivery_outside_profile");
    assert_eq!(x["stop_detail"], "InvalidGate");
    assert_eq!(
        (x["steps"].clone(), x["retired"].clone()),
        (json!(4), json!(2))
    );
    let g = &x["guest_state"];
    assert_eq!(g["rip"], hex(MAIN + 2));
    assert_eq!(g["rsp"], hex(RSP0));
    assert_eq!(g["halted"], true);
    assert_eq!(
        (g["irr"].clone(), g["isr"].clone()),
        (json!(true), json!(false))
    );
    assert_eq!(x["ptes"]["idt"], "0x8023");
    assert_eq!(x["ptes"]["gdt"], "0x9003");
    assert_eq!(x["ptes"]["stack"], "0xa003");
    assert_eq!(x["ptes"]["handler"], "0x7003");
    assert!(x["event_trace"][3]["frame_low_to_high"].is_null());

    // Injection 2: read-only stack -> the first frame store aborts the wake with an architectural #PF.
    let o = run(&a, 16, Some("--inject-stack-read-only"));
    let x = json(&o);
    assert_eq!(o.status.code(), Some(5));
    assert_eq!(x["injection"], "stack_read_only");
    assert_eq!(x["stop_reason"], "delivery_aborted");
    assert!(x["stop_detail"].as_str().unwrap().contains("StackWrite"));
    assert!(x["stop_detail"].as_str().unwrap().contains("OldSs"));
    assert_eq!(
        (x["steps"].clone(), x["retired"].clone()),
        (json!(4), json!(2))
    );
    let pf = &x["event_trace"][3]["page_fault"];
    assert_eq!(
        (
            pf["vector"].clone(),
            pf["error_code"].clone(),
            pf["linear_address"].clone()
        ),
        (json!(14), json!(3), json!("0x700fe8"))
    );
    let g = &x["guest_state"];
    assert!(g["cr2"].is_null());
    assert_eq!(g["rsp"], hex(RSP0));
    assert_eq!(g["rip"], hex(MAIN + 2));
    assert_eq!(g["halted"], true);
    assert_eq!(
        (g["irr"].clone(), g["isr"].clone()),
        (json!(true), json!(false))
    );
    assert_eq!(x["ptes"]["stack"], "0xa021");
    assert_eq!(x["ptes"]["gdt"], "0x9023");

    // Argument and image validation.
    let bad = Command::new(env!("CARGO_BIN_EXE_od-runner"))
        .args(["--stage4-paged-sti-hlt-probe"])
        .arg(&a)
        .args(["--inject-stack-read-only", "--inject-idt-not-present"])
        .output()
        .unwrap();
    assert_eq!(bad.status.code(), Some(3));
    let short = r.join(format!("work/stage4/{tag}-a/short.bin"));
    fs::write(&short, [0u8; 16]).unwrap();
    assert_eq!(run(&short, 4, None).status.code(), Some(3));
    let mut foreign = bytes.clone();
    foreign[0x5000 - 0x1000 + 0x800] = 0x01; // stack PTE read-only from the start
    let foreign_path = r.join(format!("work/stage4/{tag}-a/foreign.bin"));
    fs::write(&foreign_path, foreign).unwrap();
    let out = run(&foreign_path, 4, None);
    assert_eq!(out.status.code(), Some(4));
    assert_eq!(json(&out)["error"]["kind"], "loader");
    let mut no_sti = bytes.clone();
    no_sti[0x5000] = 0x90; // STI replaced by NOP: not the fixed profile
    let no_sti_path = r.join(format!("work/stage4/{tag}-a/no_sti.bin"));
    fs::write(&no_sti_path, no_sti).unwrap();
    assert_eq!(run(&no_sti_path, 4, None).status.code(), Some(4));
    for n in ["-a", "-a2"] {
        let _ = fs::remove_dir_all(r.join(format!("work/stage4/{tag}{n}")));
    }
}
