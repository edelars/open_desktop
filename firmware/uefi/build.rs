//! Build script (host `std` is fine here: it is not firmware code).
//!
//! For the freestanding target it assembles `src/start.S` with the system `clang` and passes
//! the object file and the own linker script to the linker. For any other target (host
//! `cargo test`) it does nothing, so the host test run needs neither `clang` nor a linker script.

use std::env;
use std::path::PathBuf;
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=src/start.S");
    println!("cargo:rerun-if-changed=linker.ld");
    println!("cargo:rerun-if-changed=acpi_blob.ld");
    println!("cargo:rerun-if-env-changed=OD_FW_CLANG");

    let target = env::var("TARGET").unwrap_or_default();
    if target != "x86_64-unknown-none" {
        return;
    }

    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    let out_dir = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR"));
    let clang = env::var("OD_FW_CLANG").unwrap_or_else(|_| "clang".to_string());
    let object = out_dir.join("start.o");

    let status = Command::new(&clang)
        .arg("--target=x86_64-unknown-none-elf")
        .arg("-c")
        .arg(manifest_dir.join("src/start.S"))
        .arg("-o")
        .arg(&object)
        .status()
        .unwrap_or_else(|error| {
            panic!(
                "could not run `{clang}` to assemble src/start.S: {error}; \
                 install clang or set OD_FW_CLANG to its path"
            )
        });
    if !status.success() {
        panic!("`{clang}` failed to assemble src/start.S ({status})");
    }

    // Link arguments are per binary: `od-firmware` is the flat firmware image, `od-acpi-blob`
    // (S4-31d) only carries the `.acpi_blob` section and has its own linker script.
    println!("cargo:rustc-link-arg-bin=od-firmware={}", object.display());
    println!(
        "cargo:rustc-link-arg-bin=od-firmware=-T{}",
        manifest_dir.join("linker.ld").display()
    );
    println!("cargo:rustc-link-arg-bin=od-firmware=-zmax-page-size=4096");
    println!("cargo:rustc-link-arg-bin=od-firmware=--build-id=none");
    println!(
        "cargo:rustc-link-arg-bin=od-acpi-blob=-T{}",
        manifest_dir.join("acpi_blob.ld").display()
    );
    println!("cargo:rustc-link-arg-bin=od-acpi-blob=-zmax-page-size=4096");
    println!("cargo:rustc-link-arg-bin=od-acpi-blob=--build-id=none");
}
