#!/bin/sh
set -eu

if [ "$#" -lt 3 ] || [ "$#" -gt 5 ]; then
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
