use std::env;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

const SOURCE_PATHS: &[&str] = &[
    "Cargo.toml",
    "Cargo.lock",
    "tools/x86-runner/Cargo.toml",
    "tools/x86-runner/build.rs",
    "tools/x86-runner/src",
    "crates/od-isa/Cargo.toml",
    "crates/od-isa/src",
    "crates/od-testkit/Cargo.toml",
    "crates/od-testkit/src",
];

fn run_text(program: &Path, args: &[&str], cwd: &Path) -> Result<String, String> {
    let output = Command::new(program)
        .current_dir(cwd)
        .args(args)
        .output()
        .map_err(|error| format!("could not run {}: {error}", program.display()))?;
    if !output.status.success() {
        return Err(format!(
            "{} exited with status {}",
            program.display(),
            output.status
        ));
    }
    // Preserve successful empty output: clean `git status --porcelain` is empty.
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

fn git_text(root: &Path, args: &[&str]) -> Result<String, String> {
    run_text(Path::new("git"), args, root)
}

fn rustc_version(root: &Path) -> String {
    let rustc = env::var_os("RUSTC")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("rustc"));
    run_text(&rustc, &["--version", "--verbose"], root)
        .unwrap_or_else(|error| format!("unknown ({error})"))
}

fn dirty_metadata(status: Result<String, String>) -> (String, String) {
    match status {
        Ok(status) if status.is_empty() => ("false".into(), String::new()),
        Ok(status) => ("true".into(), status.replace('\n', "; ")),
        Err(_) => ("unknown".into(), "unknown".into()),
    }
}

fn extra_rustflags() -> (String, String) {
    if let Ok(value) = env::var("CARGO_ENCODED_RUSTFLAGS")
        && !value.is_empty()
    {
        return ("CARGO_ENCODED_RUSTFLAGS".into(), value);
    }
    if let Ok(value) = env::var("RUSTFLAGS")
        && !value.is_empty()
    {
        return ("RUSTFLAGS".into(), value);
    }
    (
        "none".into(),
        "<no extra rustflags; Cargo profile defaults apply>".into(),
    )
}

fn rerun_paths(root: &Path, git_paths: &[PathBuf]) -> Vec<PathBuf> {
    let mut paths = SOURCE_PATHS
        .iter()
        .map(|path| root.join(path))
        .collect::<Vec<_>>();
    paths.extend(git_paths.iter().cloned());
    paths.sort();
    paths.dedup();
    paths
}

fn source_fingerprint(root: &Path, revision: &str) -> String {
    if revision.is_empty() {
        return "unknown".into();
    }
    let mut material = format!("revision={revision}\0").into_bytes();
    let mut diff_args = vec!["diff", "--binary", "HEAD", "--"];
    diff_args.extend_from_slice(SOURCE_PATHS);
    let Ok(diff) = Command::new("git")
        .current_dir(root)
        .args(&diff_args)
        .output()
    else {
        return "unknown".into();
    };
    if !diff.status.success() {
        return "unknown".into();
    }
    material.extend_from_slice(&diff.stdout);

    let mut untracked_args = vec!["ls-files", "--others", "--exclude-standard", "-z", "--"];
    untracked_args.extend_from_slice(SOURCE_PATHS);
    let Ok(untracked) = Command::new("git")
        .current_dir(root)
        .args(&untracked_args)
        .output()
    else {
        return "unknown".into();
    };
    if !untracked.status.success() {
        return "unknown".into();
    }
    for relative in untracked
        .stdout
        .split(|byte| *byte == 0)
        .filter(|path| !path.is_empty())
    {
        let relative = PathBuf::from(String::from_utf8_lossy(relative).into_owned());
        let Ok(contents) = std::fs::read(root.join(&relative)) else {
            return "unknown".into();
        };
        material.extend_from_slice(relative.to_string_lossy().as_bytes());
        material.push(0);
        material.extend_from_slice(&contents);
        material.push(0);
    }
    hash_sha256(&material).unwrap_or_else(|| "unknown".into())
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
            if let Some(hash) = text.split_whitespace().next()
                && hash.len() == 64
                && hash.bytes().all(|byte| byte.is_ascii_hexdigit())
            {
                return Some(hash.into());
            }
        }
    }
    None
}

fn emit(name: &str, value: &str) {
    assert!(!value.contains('\n') && !value.contains('\r'));
    // Cargo splits rustc-env directives at the first `=` and does not decode
    // percent escapes. Arbitrary metadata is emitted as hex by emit_hex.
    println!("cargo:rustc-env={name}={value}");
}

fn hex_encode(value: &str) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(value.len() * 2);
    for byte in value.as_bytes() {
        encoded.push(char::from(HEX[usize::from(byte >> 4)]));
        encoded.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    encoded
}

fn emit_hex(name: &str, value: &str) {
    emit(name, &hex_encode(value));
}

fn main() {
    let manifest = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("manifest dir"));
    let root = manifest
        .join("../..")
        .canonicalize()
        .expect("workspace root");
    let head = git_text(&root, &["rev-parse", "--git-path", "HEAD"])
        .ok()
        .map(|path| absolute_git_path(&root, path));
    let index = git_text(&root, &["rev-parse", "--git-path", "index"])
        .ok()
        .map(|path| absolute_git_path(&root, path));
    let packed_refs = git_text(&root, &["rev-parse", "--git-path", "packed-refs"])
        .ok()
        .map(|path| absolute_git_path(&root, path));
    let symbolic_ref = git_text(&root, &["symbolic-ref", "-q", "HEAD"])
        .ok()
        .and_then(|reference| git_text(&root, &["rev-parse", "--git-path", &reference]).ok())
        .map(|path| absolute_git_path(&root, path));
    let git_paths = [head, index, packed_refs, symbolic_ref]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>();

    for path in rerun_paths(&root, &git_paths) {
        println!("cargo:rerun-if-changed={}", path.display());
    }
    for key in ["RUSTC", "RUSTFLAGS", "CARGO_ENCODED_RUSTFLAGS"] {
        println!("cargo:rerun-if-env-changed={key}");
    }

    let revision = git_text(&root, &["rev-parse", "--verify", "HEAD"]);
    let dirty = git_text(
        &root,
        &[
            "status",
            "--porcelain",
            "--untracked-files=all",
            "--",
            "tools/x86-runner",
            "crates/od-isa",
            "crates/od-testkit",
            "Cargo.toml",
            "Cargo.lock",
        ],
    );
    let (dirty_value, dirty_paths) = dirty_metadata(dirty);
    let (flag_source, flags) = extra_rustflags();
    let revision_value = revision.unwrap_or_else(|_| "unknown".into());

    emit_hex("OD_X86_BUILD_RUSTC_HEX", &rustc_version(&root));
    emit("OD_X86_BUILD_SOURCE_REVISION", &revision_value);
    emit(
        "OD_X86_BUILD_SOURCE_SHA256",
        &source_fingerprint(&root, &revision_value),
    );
    emit("OD_X86_BUILD_SOURCE_DIRTY", &dirty_value);
    emit_hex("OD_X86_BUILD_DIRTY_PATHS_HEX", &dirty_paths);
    emit("OD_X86_BUILD_RUSTFLAGS_SOURCE", &flag_source);
    emit_hex("OD_X86_BUILD_RUSTFLAGS_HEX", &flags);
    emit(
        "OD_X86_BUILD_PROFILE",
        &env::var("PROFILE").unwrap_or_else(|_| "unknown".into()),
    );
    emit(
        "OD_X86_BUILD_TARGET",
        &env::var("TARGET").unwrap_or_else(|_| "unknown".into()),
    );
}

fn absolute_git_path(root: &Path, path: String) -> PathBuf {
    let path = PathBuf::from(path);
    if path.is_absolute() {
        path
    } else {
        root.join(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clean_and_dirty_git_status_are_distinguished_from_failure() {
        assert_eq!(
            dirty_metadata(Ok(String::new())),
            ("false".into(), String::new())
        );
        assert_eq!(
            dirty_metadata(Ok(" M tools/x86-runner/build.rs".into())),
            ("true".into(), " M tools/x86-runner/build.rs".into())
        );
        assert_eq!(
            dirty_metadata(Err("git unavailable".into())),
            ("unknown".into(), "unknown".into())
        );
    }

    #[test]
    fn successful_empty_command_output_is_preserved_and_failure_is_separate() {
        assert_eq!(
            run_text(Path::new("/bin/sh"), &["-c", "printf ''"], Path::new(".")).unwrap(),
            ""
        );
        assert!(run_text(Path::new("/bin/sh"), &["-c", "exit 7"], Path::new(".")).is_err());
        assert!(run_text(Path::new("/definitely/not/a/command"), &[], Path::new(".")).is_err());
    }

    #[test]
    fn flags_report_override_source_or_explicit_cargo_default() {
        let (source, flags) = extra_rustflags();
        if source == "none" {
            assert_eq!(flags, "<no extra rustflags; Cargo profile defaults apply>");
        } else {
            assert!(matches!(
                source.as_str(),
                "RUSTFLAGS" | "CARGO_ENCODED_RUSTFLAGS"
            ));
            assert!(!flags.is_empty());
        }
    }

    #[test]
    fn hex_metadata_roundtrips_flags_with_equals_and_newlines() {
        for value in [
            "--cfg=feature=fast",
            "--cfg=feature=fast\n--check-cfg=cfg(feature, values(\"fast\"))",
        ] {
            let encoded = hex_encode(value);
            assert!(!encoded.contains('=') && !encoded.contains('\n'));
            let bytes = encoded
                .as_bytes()
                .chunks_exact(2)
                .map(|pair| {
                    let high = (pair[0] as char).to_digit(16).unwrap();
                    let low = (pair[1] as char).to_digit(16).unwrap();
                    ((high << 4) | low) as u8
                })
                .collect::<Vec<_>>();
            assert_eq!(String::from_utf8(bytes).unwrap(), value);
        }
    }

    #[test]
    fn rerun_paths_include_source_trees_and_git_head_index_refs() {
        let root = Path::new("/work/tree");
        let git_paths = vec![
            PathBuf::from("/work/tree/.git/worktrees/codex/HEAD"),
            PathBuf::from("/work/tree/.git/index"),
            PathBuf::from("/work/tree/.git/refs/heads/topic"),
        ];
        let paths = rerun_paths(root, &git_paths);
        assert!(paths.contains(&root.join("tools/x86-runner/src")));
        assert!(paths.contains(&root.join("crates/od-isa/src")));
        assert!(paths.contains(&git_paths[0]));
        assert!(paths.contains(&git_paths[1]));
        assert!(paths.contains(&git_paths[2]));
    }

    #[test]
    fn relevant_source_and_head_changes_refresh_fingerprint() {
        let root = env::temp_dir().join(format!("od-x86-build-provenance-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("tools/x86-runner/src")).unwrap();
        std::fs::create_dir_all(root.join("crates/od-isa/src")).unwrap();
        std::fs::create_dir_all(root.join("crates/od-testkit/src")).unwrap();
        std::fs::write(root.join("tools/x86-runner/src/main.rs"), "fn main() {}\n").unwrap();
        let git = |args: &[&str]| {
            let output = Command::new("git")
                .current_dir(&root)
                .args(args)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "git {args:?}: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        };
        git(&["init", "-q"]);
        git(&["config", "user.email", "build-test@example.invalid"]);
        git(&["config", "user.name", "Build Script Test"]);
        git(&["add", "."]);
        git(&["commit", "-qm", "initial"]);
        let first_revision = git_text(&root, &["rev-parse", "HEAD"]).unwrap();
        let first = source_fingerprint(&root, &first_revision);
        assert_ne!(first, "unknown");

        std::fs::write(
            root.join("tools/x86-runner/src/main.rs"),
            "fn main() { let _ = 1; }\n",
        )
        .unwrap();
        let dirty = source_fingerprint(&root, &first_revision);
        assert_ne!(
            first, dirty,
            "dirty source edit must change embedded source fingerprint"
        );
        git(&["add", "tools/x86-runner/src/main.rs"]);
        git(&["commit", "-qm", "source edit"]);
        let second_revision = git_text(&root, &["rev-parse", "HEAD"]).unwrap();
        let committed = source_fingerprint(&root, &second_revision);
        assert_ne!(
            dirty, committed,
            "HEAD change must change embedded source fingerprint"
        );
        let _ = std::fs::remove_dir_all(root);
    }
}
