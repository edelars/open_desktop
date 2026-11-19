//! Build script (host `std` is fine here: it is not firmware code).
//!
//! For the freestanding target it assembles `src/start.S` with the system `clang` and passes
//! the object file and the own linker script to the linker. For any other target (host
//! `cargo test`) it does nothing, so the host test run needs neither `clang` nor a linker script.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::{env, fs};

fn main() {
    println!("cargo:rerun-if-changed=src/start.S");
    println!("cargo:rerun-if-changed=linker.ld");
    println!("cargo:rerun-if-changed=acpi_blob.ld");
    println!("cargo:rerun-if-env-changed=OD_FW_CLANG");
    println!("cargo:rerun-if-env-changed=OD_FW_FV_ARCHIVE");

    let target = env::var("TARGET").unwrap_or_default();
    if target == "x86_64-unknown-uefi" {
        println!("cargo:rustc-link-arg-bin=od-runtime=/subsystem:efi_runtime_driver");
        println!("cargo:rustc-link-arg-bin=od-runtime=/Brepro");
        println!("cargo:rustc-link-arg-bin=od-runtime=/debug:none");
        return;
    }
    if target != "x86_64-unknown-none" {
        return;
    }

    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    let out_dir = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR"));
    write_firmware_volume(&out_dir);
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

fn write_firmware_volume(out_dir: &Path) {
    let destination = out_dir.join("od-fv.bin");
    if let Some(source) = env::var_os("OD_FW_FV_ARCHIVE") {
        let path = PathBuf::from(source);
        println!("cargo:rerun-if-changed={}", path.display());
        fs::copy(&path, destination).unwrap_or_else(|error| {
            panic!("could not copy firmware volume {}: {error}", path.display())
        });
    } else {
        // Canonical version 1 empty archive with its CRC-32 at offset 20.
        const EMPTY: &[u8] =
            b"ODFVARC\0\x01\x00\x18\x00\x18\x00\x00\x00\x00\x00\x00\x00\x2b\x6f\xe4\x49";
        fs::write(destination, EMPTY).expect("could not write the empty firmware volume");
    }
}
