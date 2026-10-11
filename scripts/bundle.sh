#!/bin/sh
# Builds a release Duitrack.app into dist/. Pure Rust binary, no webview.
set -eu
cd "$(dirname "$0")/.."
cargo build --release -p duitrack
APP=dist/Duitrack.app
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp target/release/duitrack "$APP/Contents/MacOS/Duitrack"

if [ ! -f assets/Duitrack.icns ]; then
  ICONSET=$(mktemp -d)/Duitrack.iconset
  mkdir -p "$ICONSET"
  swift scripts/icon.swift assets/icon-1024.png
  for s in 16 32 128 256 512; do
    sips -z $s $s assets/icon-1024.png --out "$ICONSET/icon_${s}x${s}.png" >/dev/null
    d=$((s * 2))
    sips -z $d $d assets/icon-1024.png --out "$ICONSET/icon_${s}x${s}@2x.png" >/dev/null
  done
  iconutil -c icns "$ICONSET" -o assets/Duitrack.icns
fi
cp assets/Duitrack.icns "$APP/Contents/Resources/Duitrack.icns"

cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key><string>Duitrack</string>
  <key>CFBundleDisplayName</key><string>Duitrack</string>
  <key>CFBundleIdentifier</key><string>id.adefebrian.duitrack</string>
  <key>CFBundleVersion</key><string>0.1.0</string>
  <key>CFBundleShortVersionString</key><string>0.1.0</string>
  <key>CFBundleExecutable</key><string>Duitrack</string>
  <key>CFBundleIconFile</key><string>Duitrack</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>LSMinimumSystemVersion</key><string>12.0</string>
  <key>NSHighResolutionCapable</key><true/>
  <key>LSApplicationCategoryType</key><string>public.app-category.finance</string>
  <key>NSHumanReadableCopyright</key><string>Dibuat oleh Brian. adefebrian.com, github.com/adefebrian</string>
</dict>
</plist>
PLIST
codesign --force --sign - "$APP" 2>/dev/null || true
du -sh "$APP"
