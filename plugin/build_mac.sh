#!/bin/bash
# Build Stem Split for macOS (universal arm64 + x86_64) and install it for this user:
#   - CLAP + VST3 plugins  -> ~/Library/Audio/Plug-Ins/{CLAP,VST3}
#   - Stem Split.app       -> /Applications (or ~/Applications if that isn't writable)
# Usage: ./build_mac.sh [--no-install]
set -euo pipefail
cd "$(dirname "$0")"
rustup target add aarch64-apple-darwin x86_64-apple-darwin >/dev/null 2>&1 || true

# plugins
cargo xtask bundle-universal stem_split_plugin --release

# standalone app
for t in aarch64-apple-darwin x86_64-apple-darwin; do
  cargo build --release --features standalone --bin stem_split_standalone --target "$t"
done
APP="target/bundled/Stem Split.app"
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
lipo -create -output "$APP/Contents/MacOS/Stem Split" \
  target/aarch64-apple-darwin/release/stem_split_standalone \
  target/x86_64-apple-darwin/release/stem_split_standalone
VERSION=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key><string>Stem Split</string>
  <key>CFBundleDisplayName</key><string>Stem Split</string>
  <key>CFBundleIdentifier</key><string>com.bryanchi.stem-split.app</string>
  <key>CFBundleExecutable</key><string>Stem Split</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>${VERSION}</string>
  <key>CFBundleVersion</key><string>${VERSION}</string>
  <key>LSMinimumSystemVersion</key><string>11.0</string>
  <key>NSHighResolutionCapable</key><true/>
</dict>
</plist>
PLIST
codesign --force --deep -s - "$APP" >/dev/null 2>&1 || true

[ "${1:-}" = "--no-install" ] && exit 0
CLAP="$HOME/Library/Audio/Plug-Ins/CLAP"
VST3="$HOME/Library/Audio/Plug-Ins/VST3"
APPS="/Applications"; [ -w "$APPS" ] || APPS="$HOME/Applications"
mkdir -p "$CLAP" "$VST3" "$APPS"
for b in "Stem Split.clap" "Stem Split.vst3" "Stem Split.app"; do
  case "$b" in *.clap) dest="$CLAP" ;; *.vst3) dest="$VST3" ;; *) dest="$APPS" ;; esac
  rm -rf "$dest/$b"
  cp -R "target/bundled/$b" "$dest/"
  codesign --force --deep -s - "$dest/$b" >/dev/null 2>&1 || true
  xattr -dr com.apple.quarantine "$dest/$b" 2>/dev/null || true
  echo "installed $dest/$b"
done
