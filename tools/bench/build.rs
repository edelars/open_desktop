use std::env;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

fn main() {
    let manifest_dir = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("manifest dir"));
    let root = manifest_dir
        .join("../..")
        .canonicalize()
        .expect("workspace root");

    for path in [
        "Cargo.toml",
        "Cargo.lock",
        "crates/od-isa/src",
        "crates/od-jit/src",
        "crates/od-machine/src",
        "crates/od-machine/Cargo.toml",
        "crates/od-host/src",
        "crates/od-testkit/src",
        "tools/bench/src",
        "tools/bench/build.rs",
    ] {
        println!("cargo:rerun-if-changed={}", root.join(path).display());
    }
    for path in ["HEAD", "index"] {
        let git_path = git_text(&root, &["rev-parse", "--git-path", path]);
        if git_path != "unavailable" {
            let path = PathBuf::from(git_path);
            let path = if path.is_absolute() {
                path
            } else {
                root.join(path)
            };
            println!("cargo:rerun-if-changed={}", path.display());
        }
    }
    println!("cargo:rerun-if-env-changed=RUSTC");

    let revision = git_text(&root, &["rev-parse", "--verify", "HEAD"]);
    let dirty = git_text(&root, &["status", "--porcelain"]);
    let source_fingerprint = source_fingerprint(&root, &revision);
    let rustc = env::var_os("RUSTC")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("rustc"));
    let rustc_version = command_text(&rustc, &["--version"]);
    let profile = env::var("PROFILE").unwrap_or_else(|_| "unknown".into());
    let target = env::var("TARGET").unwrap_or_else(|_| "unknown".into());

    emit("OD_BENCH_BUILD_RUSTC", &rustc_version);
    emit("OD_BENCH_BUILD_SOURCE_REVISION", &revision);
    emit("OD_BENCH_BUILD_SOURCE_SHA256", &source_fingerprint);
    emit(
        "OD_BENCH_BUILD_TREE_DIRTY",
        if dirty == "unavailable" {
            "unknown"
        } else if dirty.is_empty() {
            "false"
        } else {
            "true"
        },
    );
    emit("OD_BENCH_BUILD_PROFILE", &profile);
    emit("OD_BENCH_BUILD_TARGET", &target);
}

fn emit(name: &str, value: &str) {
    println!("cargo:rustc-env={name}={value}");
}

fn git_text(root: &Path, args: &[&str]) -> String {
    Command::new("git")
        .current_dir(root)
        .args(args)
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_owned())
        .unwrap_or_else(|| "unavailable".into())
}

fn command_text(program: &Path, args: &[&str]) -> String {
    Command::new(program)
        .args(args)
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_owned())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "unavailable".into())
}

fn source_fingerprint(root: &Path, revision: &str) -> String {
    if revision == "unavailable" {
        return "unavailable".into();
    }
    let mut material = format!("revision={revision}\0").into_bytes();
    let diff = Command::new("git")
        .current_dir(root)
        .args(["diff", "--binary", "HEAD"])
        .output();
    let Ok(diff) = diff else {
        return "unavailable".into();
    };
    if !diff.status.success() {
        return "unavailable".into();
    }
    material.extend_from_slice(&diff.stdout);

    let untracked = Command::new("git")
        .current_dir(root)
        .args(["ls-files", "--others", "--exclude-standard", "-z"])
        .output();
    let Ok(untracked) = untracked else {
        return "unavailable".into();
    };
    if !untracked.status.success() {
        return "unavailable".into();
    }
    for path in untracked
        .stdout
        .split(|byte| *byte == 0)
        .filter(|path| !path.is_empty())
    {
        let path = String::from_utf8_lossy(path);
        let full_path = root.join(path.as_ref());
        let Ok(contents) = std::fs::read(full_path) else {
            return "unavailable".into();
        };
        material.extend_from_slice(path.as_bytes());
        material.push(0);
        material.extend_from_slice(&contents);
        material.push(0);
    }
    hash_sha256(&material).unwrap_or_else(|| "unavailable".into())
}

fn hash_sha256(bytes: &[u8]) -> Option<String> {
    for (program, args) in [("shasum", vec!["-a", "256"]), ("sha256sum", vec![])] {
        let Ok(mut child) = Command::new(program)
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
        else {
            continue;
        };
        let Some(mut stdin) = child.stdin.take() else {
            let _ = child.wait();
            continue;
        };
        if stdin.write_all(bytes).is_err() {
            let _ = child.wait();
            continue;
        }
        drop(stdin);
        let Ok(output) = child.wait_with_output() else {
            continue;
        };
        if output.status.success() {
            let text = String::from_utf8_lossy(&output.stdout);
            if let Some(hash) = text.split_whitespace().next() {
                if hash.len() == 64 && hash.bytes().all(|byte| byte.is_ascii_hexdigit()) {
                    return Some(hash.into());
                }
            }
        }
    }
    None
}

use std::io::Write;
