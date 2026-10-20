#!/usr/bin/env sh
# Builds the Linux .deb on the machine it runs on: x86_64, or arm64 on a
# Raspberry Pi 4/5 with Raspberry Pi OS (64-bit). Output goes to
# target/release/bundle/deb/. Build packages: README, "Linux and Raspberry Pi".
set -eu
cd "$(dirname "$0")/../.."

# Tauri bundles the `mist` CLI (the streaming window) as a sidecar named
# after the target triple.
cargo build --release -p mist-core --bin mist
triple="$(rustc -vV | sed -n 's/^host: //p')"
mkdir -p app/src-tauri/binaries
cp target/release/mist "app/src-tauri/binaries/mist-${triple}"

cd app
npm ci
npx tauri build --bundles deb
ls -1 ../target/release/bundle/deb/*.deb
