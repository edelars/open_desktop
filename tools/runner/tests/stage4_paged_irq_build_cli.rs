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
            "tools/build/build_stage4_paged_irq_probe.py",
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
    root.join(format!("work/stage4/{name}/paged_irq_probe.bin"))
}
fn run(image: &Path, budget: u16, inject: Option<&str>) -> Output {
    let mut c = Command::new(env!("CARGO_BIN_EXE_od-runner"));
    c.arg("--stage4-paged-irq-probe")
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
fn real_paged_irq_artifact_is_reproducible_and_guest_trace_is_exact() {
    let r = root();
    let tag = format!("s4-27h-{}", std::process::id());
    let a = build(&r, &format!("{tag}-a"));
    let a2 = build(&r, &format!("{tag}-a2"));
    let bytes = fs::read(&a).unwrap();
    let manifest: Value = serde_json::from_slice(
        &fs::read(r.join(format!("work/stage4/{tag}-a/build_manifest.json"))).unwrap(),
    )
    .unwrap();
    assert_eq!(manifest["profile"], "Stage4PagedIrqProbeV1");
    assert_eq!(manifest["layout"]["main_code"]["bytes"], "0f013b0f013b");
    assert_eq!(manifest["layout"]["handler_code"]["bytes"], "890348cf");
    assert_eq!(manifest["layout"]["idt"]["gate_vector"], 0x41);
    assert_eq!(manifest["layout"]["eoi"]["physical"], "0xfee000b0");
    assert_eq!(
        manifest["layout"]["expected_ptes"]["lapic"]["final"],
        "0xfee00063"
    );
    assert_eq!(bytes, fs::read(a2).unwrap());

    // (rip, rsp, rflags, irr, isr) after each prefix of the five steps.
    let expected = [
        (MAIN, RSP0, "0x202", false, false),
        (MAIN + 3, RSP0, "0x202", false, false),
        (HANDLER, FRAME, "0x2", false, true),
        (HANDLER + 2, FRAME, "0x2", false, false),
        (MAIN + 3, RSP0, "0x202", false, false),
        (MAIN + 6, RSP0, "0x202", false, false),
    ];
    for budget in 0..=5_u16 {
        let out = run(&a, budget, None);
        let x = json(&out);
        let (rip, rsp, rflags, irr, isr) = expected[budget as usize];
        assert_eq!(x["profile"], "Stage4PagedIrqProbeV1");
        assert_eq!(x["steps"], budget);
        assert_eq!(x["event_trace"].as_array().unwrap().len(), budget as usize);
        assert_eq!(x["guest_state"]["rip"], hex(rip), "budget {budget}");
        assert_eq!(x["guest_state"]["rsp"], hex(rsp), "budget {budget}");
        assert_eq!(x["guest_state"]["rflags"], rflags, "budget {budget}");
        assert_eq!(x["guest_state"]["irr"], irr, "budget {budget}");
        assert_eq!(x["guest_state"]["isr"], isr, "budget {budget}");
        assert_eq!(x["accepted"], budget == 5);
        if budget == 5 {
            assert!(out.status.success());
            assert_eq!(x["stop_reason"], "case_complete");
            assert_eq!(x["status"], "completed");
            assert_eq!(x["retired"], 4);
            assert_eq!(x["vector"], "0x41");
            assert!(x["failed_checks"].as_array().unwrap().is_empty());
            assert_eq!(names(&x), ["main", "delivery", "eoi", "IRETQ", "resume"]);
            let t = &x["event_trace"];
            assert_eq!(t[1]["exit"], "delivered");
            assert_eq!(t[1]["if_after"], false);
            assert_eq!(t[1]["isr_after"], true);
            assert_eq!(
                t[1]["frame_low_to_high"],
                json!(["0x401003", "0x8", "0x202", "0x700ff8", "0x10"])
            );
            assert_eq!(t[1]["ptes_after"]["stack"], "0xa063");
            assert_eq!(t[1]["ptes_after"]["idt"], "0x8023");
            assert_eq!(t[1]["ptes_after"]["gdt"], "0x9023");
            assert_eq!(t[2]["ptes_after"]["lapic"], "0xfee00063");
            assert_eq!(t[2]["isr_after"], false);
            assert_eq!(t[3]["rsp_after"], hex(RSP0));
            assert_eq!(t[3]["if_after"], true);
            assert_eq!(x["ptes"]["main"], "0x6023");
            assert_eq!(x["ptes"]["handler"], "0x7023");
            assert_eq!(x["ptes"]["lapic"], "0xfee00063");
        } else {
            assert_eq!(out.status.code(), Some(5));
            assert_eq!(x["stop_reason"], "budget_exhausted");
        }
    }

    // Injection 1: gate P=0 -> refused before any commit, handler never runs.
    let o = run(&a, 16, Some("--inject-idt-gate-not-present"));
    let x = json(&o);
    assert_eq!(o.status.code(), Some(5));
    assert_eq!(x["injection"], "idt_gate_not_present");
    assert_eq!(x["stop_reason"], "delivery_outside_profile");
    assert_eq!(x["stop_detail"], "InvalidGate");
    assert_eq!(
        (x["steps"].clone(), x["retired"].clone()),
        (json!(2), json!(1))
    );
    assert_eq!(x["guest_state"]["rip"], hex(MAIN + 3));
    assert_eq!(x["guest_state"]["rsp"], hex(RSP0));
    assert_eq!(
        (
            x["guest_state"]["irr"].clone(),
            x["guest_state"]["isr"].clone()
        ),
        (json!(true), json!(false))
    );
    assert_eq!(x["ptes"]["idt"], "0x8023");
    assert_eq!(x["ptes"]["gdt"], "0x9003");
    assert_eq!(x["ptes"]["handler"], "0x7003");
    assert!(x["event_trace"][1]["frame_low_to_high"].is_null());

    // Injection 2: read-only stack -> first frame store aborts delivery with an architectural #PF.
    let o = run(&a, 16, Some("--inject-stack-readonly"));
    let x = json(&o);
    assert_eq!(o.status.code(), Some(5));
    assert_eq!(x["injection"], "stack_readonly");
    assert_eq!(x["stop_reason"], "delivery_aborted");
    assert!(x["stop_detail"].as_str().unwrap().contains("StackWrite"));
    assert!(x["stop_detail"].as_str().unwrap().contains("OldSs"));
    assert_eq!(
        (x["steps"].clone(), x["retired"].clone()),
        (json!(2), json!(1))
    );
    let pf = &x["event_trace"][1]["page_fault"];
    assert_eq!(
        (
            pf["vector"].clone(),
            pf["error_code"].clone(),
            pf["linear_address"].clone()
        ),
        (json!(14), json!(3), json!("0x700fe8"))
    );
    assert!(x["guest_state"]["cr2"].is_null());
    assert_eq!(x["guest_state"]["rsp"], hex(RSP0));
    assert_eq!(
        (
            x["guest_state"]["irr"].clone(),
            x["guest_state"]["isr"].clone()
        ),
        (json!(true), json!(false))
    );
    assert_eq!(x["ptes"]["stack"], "0xa021");

    // Injection 3: no EOI -> ISR stays set, a second request of the vector is not eligible.
    let o = run(&a, 16, Some("--inject-handler-without-eoi"));
    let x = json(&o);
    assert_eq!(o.status.code(), Some(5));
    assert_eq!(x["injection"], "handler_without_eoi");
    assert_eq!(x["stop_reason"], "second_delivery_blocked");
    assert_eq!(
        (x["steps"].clone(), x["retired"].clone()),
        (json!(5), json!(3))
    );
    assert_eq!(
        (
            x["guest_state"]["irr"].clone(),
            x["guest_state"]["isr"].clone()
        ),
        (json!(true), json!(true))
    );
    assert_eq!(x["guest_state"]["rip"], hex(MAIN + 6));
    assert!(x["guest_state"]["eligible"].is_null());
    assert_eq!(x["ptes"]["lapic"], "0xfee00003");
    let failed = x["failed_checks"].as_array().unwrap();
    for c in ["eoi_cleared_isr", "irq_state_clear"] {
        assert!(failed.contains(&json!(c)), "{c}");
    }

    // Argument and image validation.
    let bad = Command::new(env!("CARGO_BIN_EXE_od-runner"))
        .args(["--stage4-paged-irq-probe"])
        .arg(&a)
        .args(["--inject-stack-readonly", "--inject-handler-without-eoi"])
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
    for n in ["-a", "-a2"] {
        let _ = fs::remove_dir_all(r.join(format!("work/stage4/{tag}{n}")));
    }
}
