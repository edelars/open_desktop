#!/usr/bin/env python3
"""Build OpenDesktop.app with VMHost inside (S11-05b, ADR-0006 solution (a)).

Steps: `cargo build -p od-host` (the staticlib that only `od-vmhost` links), `swift build` of the products
`OpenDesktop` and `od-vmhost` from `apps/`, then the bundle layout, `Info.plist`, and ad-hoc signing:

    OpenDesktop.app/Contents/Info.plist
    OpenDesktop.app/Contents/MacOS/OpenDesktop      (no engine, no JIT entitlement)
    OpenDesktop.app/Contents/Helpers/od-vmhost      (allow-jit, apps/vmhost/Configuration/vmhost.entitlements)

The helper is signed first, then the application bundle (the same order as tests/system/vmhost_app_bundle_smoke.sh).
Both use `--options runtime`; the application has no JIT entitlement and no App Sandbox (ADR-0006 (b)).
Ad-hoc signing only: Developer ID and notarization belong to stage 12.

The last line of stdout is `app_bundle_path=<absolute path>`. Python 3.9+, macOS only.
"""

from __future__ import annotations

import argparse
import shutil
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
APPS = ROOT / "apps"
INFO_PLIST = APPS / "macos" / "Resources" / "Info.plist"
APP_ENTITLEMENTS = APPS / "macos" / "Configuration" / "OpenDesktop.entitlements"
VMHOST_ENTITLEMENTS = APPS / "vmhost" / "Configuration" / "vmhost.entitlements"
APP_EXECUTABLE = "OpenDesktop"
HELPER_RELATIVE = Path("Contents") / "Helpers" / "od-vmhost"
# Keep in sync with VMHostLocation.bundledRelativePath in apps/macos/Sources/App/AppOptions.swift.
DEFAULT_OUTPUT = ROOT / "target" / "app-bundle"


class BuildError(Exception):
    pass


def run(command: list[str], cwd: Path | None = None) -> str:
    print("+ " + " ".join(command), file=sys.stderr, flush=True)
    result = subprocess.run(command, cwd=cwd, stdout=subprocess.PIPE, text=True)
    if result.returncode != 0:
        if result.stdout:
            print(result.stdout, file=sys.stderr, end="")
        raise BuildError(f"command failed with exit code {result.returncode}: {' '.join(command)}")
    return result.stdout


def build_products(configuration: str) -> Path:
    """Build the Rust staticlib and the two Swift products; return the SwiftPM binary directory."""
    cargo = ["cargo", "build", "-p", "od-host", "--offline"]
    if configuration == "release":
        cargo.append("--release")
    run(cargo, cwd=ROOT)
    swift = ["swift", "build", "-c", configuration, "--package-path", str(APPS)]
    run(swift + ["--product", "OpenDesktop"])
    run(swift + ["--product", "od-vmhost"])
    bin_path = Path(run(swift + ["--show-bin-path"]).strip())
    for name in (APP_EXECUTABLE, "od-vmhost"):
        if not (bin_path / name).is_file():
            raise BuildError(f"missing build product: {bin_path / name}")
    return bin_path


def assemble(bin_path: Path, output: Path, codesign: str) -> Path:
    app = output / "OpenDesktop.app"
    if app.is_symlink():
        app.unlink()
    elif app.exists():
        shutil.rmtree(app)
    macos_dir = app / "Contents" / "MacOS"
    helper = app / HELPER_RELATIVE
    macos_dir.mkdir(parents=True)
    helper.parent.mkdir(parents=True)
    shutil.copy2(bin_path / APP_EXECUTABLE, macos_dir / APP_EXECUTABLE)
    shutil.copy2(bin_path / "od-vmhost", helper)
    shutil.copy2(INFO_PLIST, app / "Contents" / "Info.plist")
    run(["plutil", "-lint", str(app / "Contents" / "Info.plist")])
    run([codesign, "--force", "--sign", "-", "--options", "runtime", "--entitlements", str(VMHOST_ENTITLEMENTS), str(helper)])
    run([codesign, "--force", "--sign", "-", "--options", "runtime", "--entitlements", str(APP_ENTITLEMENTS), str(app)])
    run([codesign, "--verify", "--strict", "--verbose=2", str(helper)])
    run([codesign, "--verify", "--strict", "--deep", "--verbose=2", str(app)])
    return app


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(description="Build OpenDesktop.app with VMHost inside (ad-hoc signed).")
    parser.add_argument("--configuration", choices=("debug", "release"), default="release", help="SwiftPM/Cargo profile (default: release)")
    parser.add_argument("--output", type=Path, default=DEFAULT_OUTPUT, help=f"directory that receives OpenDesktop.app (default: {DEFAULT_OUTPUT})")
    parser.add_argument("--skip-build", action="store_true", help="reuse the products of a previous `swift build` of the same configuration")
    parser.add_argument("--codesign", default="codesign", help="codesign tool (default: codesign)")
    args = parser.parse_args(argv)

    if sys.platform != "darwin":
        print("error: macOS only", file=sys.stderr)
        return 1
    for required in (INFO_PLIST, APP_ENTITLEMENTS, VMHOST_ENTITLEMENTS):
        if not required.is_file():
            print(f"error: missing {required}", file=sys.stderr)
            return 1
    try:
        if args.skip_build:
            swift = ["swift", "build", "-c", args.configuration, "--package-path", str(APPS), "--show-bin-path"]
            bin_path = Path(run(swift).strip())
            for name in (APP_EXECUTABLE, "od-vmhost"):
                if not (bin_path / name).is_file():
                    raise BuildError(f"--skip-build: missing {bin_path / name}")
        else:
            bin_path = build_products(args.configuration)
        app = assemble(bin_path, args.output.resolve(), args.codesign)
    except BuildError as error:
        print(f"error: {error}", file=sys.stderr)
        return 1
    print(f"app_bundle_path={app}")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
