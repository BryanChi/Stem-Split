#!/bin/bash
# Build Stem Split (CLAP + VST3, universal arm64 + x86_64) and install it for this user.
# Usage: ./build_mac.sh            build + install
#        ./build_mac.sh --no-install
set -euo pipefail
cd "$(dirname "$0")"
rustup target add aarch64-apple-darwin x86_64-apple-darwin >/dev/null 2>&1 || true
cargo xtask bundle-universal stem_split_plugin --release
[ "${1:-}" = "--no-install" ] && exit 0
CLAP="$HOME/Library/Audio/Plug-Ins/CLAP"
VST3="$HOME/Library/Audio/Plug-Ins/VST3"
mkdir -p "$CLAP" "$VST3"
for b in "Stem Split.clap" "Stem Split.vst3"; do
  dest="$CLAP"; [[ "$b" == *.vst3 ]] && dest="$VST3"
  rm -rf "$dest/$b"
  cp -R "target/bundled/$b" "$dest/"
  codesign --force --deep -s - "$dest/$b" >/dev/null 2>&1 || true
  xattr -dr com.apple.quarantine "$dest/$b" 2>/dev/null || true
  echo "installed $dest/$b"
done
