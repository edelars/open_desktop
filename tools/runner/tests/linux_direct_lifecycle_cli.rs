//! S4-41: process-level lifecycle checks of `od-runner linux-direct`: an invalid configuration
//! ends the process with a stable exit code and evidence that says the guest was not started. The
//! cases need no valid kernel, so they cost nothing; the runs of a real board are covered by the
//! in-process tests of `linux_direct.rs`.

use serde_json::Value;
use std::path::PathBuf;
use std::process::{Command, Output};

struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let dir =
            std::env::temp_dir().join(format!("od-runner-s441-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }

    fn path(&self, file: &str) -> String {
        self.0.join(file).to_string_lossy().into_owned()
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn invoke(scratch: &Scratch, vmlinuz: &str, extra: &[&str]) -> Output {
    let zeros = "0".repeat(64);
    Command::new(env!("CARGO_BIN_EXE_od-runner"))
        .args(["linux-direct", "--vmlinuz", vmlinuz])
        .args(["--initramfs", &scratch.path("initramfs")])
        .args(["--acpi-blob", &scratch.path("acpi")])
        .args(["--nonce", "n1", "--ram-mib", "128"])
        .args(["--expected-vmlinuz-sha256", &zeros])
        .args(["--expected-initramfs-sha256", &zeros])
        .args(["--expected-acpi-manifest-hash", &zeros])
        .args(["--out", &scratch.path("out.json")])
        .args(extra)
        .output()
        .unwrap()
}

fn stdout_json(output: &Output) -> Value {
    serde_json::from_slice(&output.stdout).expect("stdout is one JSON document")
}

#[test]
fn bad_arguments_exit_3_with_invalid_input_and_write_no_evidence_file() {
    let scratch = Scratch::new("args");
    let run = ["--run", "--budget", "1", "--set-cpl0"];
    for extra in [
        [&run[..], &["--cpuid-profile", "nope"]].concat(),
        [&run[..], &["--cpuid-profile", "diagnostic", "--user-mode"]].concat(),
        vec!["--run", "--budget", "1", "--user-mode"],
        vec!["--run"],
    ] {
        let output = invoke(&scratch, &scratch.path("vmlinuz"), &extra);
        assert_eq!(output.status.code(), Some(3), "{extra:?}");
        let value = stdout_json(&output);
        assert_eq!(value["status"], "refused", "{extra:?}");
        assert_eq!(value["error"]["code"], "invalid_input", "{extra:?}");
        assert_eq!(value["guest_started"], false, "{extra:?}");
        assert_eq!(value["retired"], 0, "{extra:?}");
    }
    assert!(!std::path::Path::new(&scratch.path("out.json")).exists());
}

#[test]
fn missing_and_foreign_inputs_never_start_the_guest_and_repeat_identically() {
    let scratch = Scratch::new("inputs");
    let run = ["--run", "--budget", "1000", "--set-cpl0"];

    // A missing kernel: unreadable input, exit 3, evidence written.
    let output = invoke(&scratch, &scratch.path("missing"), &run);
    assert_eq!(output.status.code(), Some(3));
    let value = stdout_json(&output);
    assert_eq!(value["error"]["code"], "invalid_input");
    assert_eq!(value["error"]["stage"], "vmlinuz_file");
    assert_eq!(
        (&value["guest_started"], &value["retired"]),
        (&false.into(), &0.into())
    );
    assert!(value.get("stop").is_none());
    // Unreadable input is still a result of `evaluate`: the evidence file is written and is the
    // document that was printed (only bad arguments leave no file, see the test above).
    let written: Value =
        serde_json::from_str(&std::fs::read_to_string(scratch.path("out.json")).unwrap()).unwrap();
    assert_eq!(written, value);

    // A kernel whose hash is not the pinned one: refused, exit 4, still no guest.
    std::fs::write(scratch.path("vmlinuz"), b"not a kernel").unwrap();
    let first = invoke(&scratch, &scratch.path("vmlinuz"), &run);
    assert_eq!(first.status.code(), Some(4));
    let value = stdout_json(&first);
    assert_eq!(value["status"], "refused");
    assert_eq!(value["error"]["code"], "vmlinuz_sha256_mismatch");
    assert_eq!(
        (&value["guest_started"], &value["retired"]),
        (&false.into(), &0.into())
    );
    assert!(value.get("stop").is_none());
    // The evidence file is the same document that was printed.
    let written: Value =
        serde_json::from_str(&std::fs::read_to_string(scratch.path("out.json")).unwrap()).unwrap();
    assert_eq!(written, value);

    // A second session in a fresh process: identical result (no state outside the process).
    let second = invoke(&scratch, &scratch.path("vmlinuz"), &run);
    assert_eq!(second.status.code(), first.status.code());
    assert_eq!(second.stdout, first.stdout);
}
