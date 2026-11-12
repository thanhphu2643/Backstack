#!/bin/bash
# Builds build/Backstack.app.
set -euo pipefail
cd "$(dirname "$0")"
export MACOSX_DEPLOYMENT_TARGET=14.0 # match Package.swift for the Rust static lib

cargo test --quiet --manifest-path core/Cargo.toml
cargo build --release --manifest-path core/Cargo.toml
swift build -c release -Xlinker -L"$PWD/core/target/release"

app=build/Backstack.app
rm -rf "$app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
cp .build/release/Backstack "$app/Contents/MacOS/"
cp Resources/Info.plist "$app/Contents/"
cp Resources/AppIcon.icns "$app/Contents/Resources/"

# A stable certificate keeps the Downloads folder permission across rebuilds;
# an ad-hoc signature changes every build, so macOS would ask again.
identity=$(security find-identity -v -p codesigning | awk '/^ *1\)/ {print $2}')
codesign --force --sign "${identity:--}" "$app"
echo "Built $app"
