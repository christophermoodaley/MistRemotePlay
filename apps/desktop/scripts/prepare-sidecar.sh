#!/usr/bin/env sh
# Builds the `mist` CLI in release mode and copies it to the location Tauri
# expects for `bundle.externalBin` (binaries/mist-<target-triple>).
set -eu
cd "$(dirname "$0")/../../.."
cargo build --release -p mist-app-core --bin mist
triple="$(rustc -vV | sed -n 's/^host: //p')"
mkdir -p apps/desktop/src-tauri/binaries
cp target/release/mist "apps/desktop/src-tauri/binaries/mist-${triple}"
echo "sidecar: apps/desktop/src-tauri/binaries/mist-${triple}"
