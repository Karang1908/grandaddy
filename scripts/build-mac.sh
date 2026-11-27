#!/bin/sh
# Build Grandaddy.app (with the grandaddy CLI inside it) and a .dmg.
# Needs: Rust, and the Tauri bundler (cargo install tauri-cli --version "^2" --locked).
set -eu
cd "$(dirname "$0")/.."
triple=$(rustc -vV | sed -n 's/^host: //p')
cargo build -p grandaddy --release
mkdir -p app/src-tauri/binaries
cp target/release/grandaddy "app/src-tauri/binaries/grandaddy-$triple"
(cd app/src-tauri && cargo tauri build --bundles app)

# A plain disk image: the app next to an Applications shortcut. Made with hdiutil rather than
# the bundler's Finder-scripted one, which needs Automation access and times out without it.
app=target/release/bundle/macos/Grandaddy.app
version=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
dmg="target/release/bundle/Grandaddy_${version}_${triple%%-*}.dmg"
stage=$(mktemp -d)
cp -R "$app" "$stage/"
ln -s /Applications "$stage/Applications"
rm -f "$dmg"
hdiutil create -quiet -volname Grandaddy -srcfolder "$stage" -ov -format UDZO "$dmg"
rm -rf "$stage"
echo
echo "Built:"
echo "  $app"
echo "  $dmg"
