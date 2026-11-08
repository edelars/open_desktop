#!/bin/sh
# Smoke of the OpenDesktop.app bundle (S11-05c): JIT execution of a fragment by the VMHost helper inside the bundle.
#
# Usage: sh tests/system/vmhost_app_bundle_smoke.sh [APP_BUNDLE [LAUNCHER]]
#   APP_BUNDLE  default: the bundle built by `python3 tools/build/build_app_bundle.py` (target/app-bundle/OpenDesktop.app)
#   LAUNCHER    default: apps/.build/debug/open-desktop-launcher-smoke (built here with `swift build --product ...`)
# Checks: plutil -lint of Info.plist, codesign --verify --strict --deep of the application and --strict of the helper
# Contents/Helpers/od-vmhost, then the launcher runs a fragment through that helper (exit status of the launcher).
#
# Legacy interface (3 to 5 arguments: LAUNCHER VMHOST ENTITLEMENTS [PRESERVED_APP_BUNDLE [CODESIGN_TOOL]]) is kept only for
# tests/cpu/stage3_harness.py (frozen Stage 3 evidence); it is not normative. Without it that harness cannot run.
set -eu

root=$(cd "$(dirname "$0")/../.." && pwd)

if [ "$#" -ge 3 ]; then
if [ "$#" -gt 5 ]; then
    echo "usage: $0 LAUNCHER VMHOST ENTITLEMENTS [PRESERVED_APP_BUNDLE [CODESIGN_TOOL]]" >&2
    exit 2
fi

launcher=$(cd "$(dirname "$1")" && pwd)/$(basename "$1")
vmhost=$(cd "$(dirname "$2")" && pwd)/$(basename "$2")
entitlements=$(cd "$(dirname "$3")" && pwd)/$(basename "$3")
codesign_tool=${5:-codesign}
temporary=$(mktemp -d /private/tmp/open-desktop-stage0-app.XXXXXX)
app="$temporary/OpenDesktop-Stage0-Smoke.app"
trap 'rm -rf "$temporary"' EXIT HUP INT TERM

if [ "$#" -ge 4 ]; then
    preserved_app=$4
    if [ -e "$preserved_app" ] || [ -L "$preserved_app" ]; then
        echo "refusing to overwrite preserved bundle path: $preserved_app" >&2
        exit 1
    fi
    mkdir -p "$(dirname "$preserved_app")"
fi

mkdir -p "$app/Contents/MacOS"
cp "$launcher" "$app/Contents/MacOS/OpenDesktopLauncher"
cp "$vmhost" "$app/Contents/MacOS/od-vmhost-smoke"
cat > "$app/Contents/Info.plist" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleExecutable</key><string>OpenDesktopLauncher</string>
  <key>CFBundleIdentifier</key><string>dev.opendesk.stage0.smoke</string>
  <key>CFBundleName</key><string>OpenDesktop Stage 0 Smoke</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleVersion</key><string>0.1</string>
  <key>LSMinimumSystemVersion</key><string>14.0</string>
</dict>
</plist>
PLIST

"$codesign_tool" --force --sign - --options runtime --entitlements "$entitlements" \
    "$app/Contents/MacOS/od-vmhost-smoke"
"$codesign_tool" --force --sign - --options runtime \
    "$app/Contents/MacOS/OpenDesktopLauncher"
"$codesign_tool" --force --sign - --options runtime "$app"
plutil -lint "$app/Contents/Info.plist"
"$codesign_tool" --verify --strict --deep --verbose=2 "$app"
if [ "$#" -ge 4 ]; then
    cp -R "$app" "$preserved_app"
    app=$preserved_app
    echo "vmhost_app_bundle_path=$app"
fi
"$app/Contents/MacOS/OpenDesktopLauncher" \
    --vmhost "$app/Contents/MacOS/od-vmhost-smoke" \
    --hex '48 2b c1 75 fb' --rax 3 --rcx 1 --budget 32
if [ "$#" -ge 4 ]; then
    echo "vmhost_app_bundle_status=passed"
fi
exit 0
fi

if [ "$#" -gt 2 ]; then
    echo "usage: $0 [APP_BUNDLE [LAUNCHER]]" >&2
    exit 2
fi
codesign_tool=codesign

if [ "$#" -ge 1 ]; then
    app=$(cd "$(dirname "$1")" && pwd)/$(basename "$1")
else
    build_output=$(python3 "$root/tools/build/build_app_bundle.py")
    printf '%s\n' "$build_output"
    app=$(printf '%s\n' "$build_output" | sed -n 's/^app_bundle_path=//p' | tail -n 1)
    if [ -z "$app" ]; then
        echo "build_app_bundle.py did not report app_bundle_path" >&2
        exit 1
    fi
fi
if [ "$#" -ge 2 ]; then
    launcher=$(cd "$(dirname "$2")" && pwd)/$(basename "$2")
else
    (cd "$root/apps" && swift build --product open-desktop-launcher-smoke)
    launcher=$root/apps/.build/debug/open-desktop-launcher-smoke
fi
helper=$app/Contents/Helpers/od-vmhost

plutil -lint "$app/Contents/Info.plist"
"$codesign_tool" --verify --strict --verbose=2 "$helper"
"$codesign_tool" --verify --strict --deep --verbose=2 "$app"
"$launcher" --vmhost "$helper" --hex '48 2b c1 75 fb' --rax 3 --rcx 1 --budget 32
