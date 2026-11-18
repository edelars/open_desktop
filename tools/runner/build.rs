use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;

const SOURCE_PATHS: &[&str] = &[
    "Cargo.toml",
    "Cargo.lock",
    "tools/runner/Cargo.toml",
    "tools/runner/build.rs",
    "tools/runner/src",
    "crates/od-isa/Cargo.toml",
    "crates/od-isa/src",
    "crates/od-jit/Cargo.toml",
    "crates/od-jit/src",
    "crates/od-machine/Cargo.toml",
    "crates/od-machine/src",
    "crates/od-storage/Cargo.toml",
    "crates/od-storage/src",
    "crates/od-testkit/Cargo.toml",
    "crates/od-testkit/src",
    "crates/od-host/Cargo.toml",
    "crates/od-host/src",
];

fn git_text(root: &Path, args: &[&str]) -> Option<String> {
    let output = Command::new("git")
        .current_dir(root)
        .args(args)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    Some(String::from_utf8(output.stdout).ok()?.trim().to_owned())
}

fn valid_revision(revision: &str) -> bool {
    matches!(revision.len(), 40 | 64) && revision.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn emit_rerun_paths(root: &Path) {
    for source in SOURCE_PATHS {
        println!("cargo:rerun-if-changed={}", root.join(source).display());
    }

    for git_path in ["HEAD", "index", "packed-refs"] {
        if let Some(path) = git_text(root, &["rev-parse", "--git-path", git_path]) {
            let path = PathBuf::from(path);
            let path = if path.is_absolute() {
                path
            } else {
                root.join(path)
            };
            println!("cargo:rerun-if-changed={}", path.display());
        }
    }

    if let Some(reference) = git_text(root, &["symbolic-ref", "--quiet", "HEAD"])
        && let Some(path) = git_text(root, &["rev-parse", "--git-path", &reference])
    {
        let path = PathBuf::from(path);
        let path = if path.is_absolute() {
            path
        } else {
            root.join(path)
        };
        println!("cargo:rerun-if-changed={}", path.display());
    }
}

fn main() {
    let manifest_dir =
        PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("Cargo sets CARGO_MANIFEST_DIR"));
    let root = manifest_dir
        .join("../..")
        .canonicalize()
        .unwrap_or_else(|_| manifest_dir.join("../.."));

    emit_rerun_paths(&root);

    let revision = git_text(&root, &["rev-parse", "--verify", "HEAD"])
        .filter(|revision| valid_revision(revision))
        .unwrap_or_else(|| "unknown".into());
    println!("cargo:rustc-env=OD_SOURCE_REVISION={revision}");

    let mut status_args = vec!["status", "--porcelain", "--untracked-files=all", "--"];
    status_args.extend_from_slice(SOURCE_PATHS);
    let dirty_state = match git_text(&root, &status_args) {
        Some(status) if status.is_empty() => "false",
        Some(_) => "true",
        None => "unknown",
    };
    println!("cargo:rustc-env=OD_SOURCE_DIRTY={dirty_state}");
}
