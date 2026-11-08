#!/bin/sh
set -eu
cd "$(dirname "$0")/.."
target=${1:-$(rustc -vV | sed -n 's/^host: //p')}
cargo build --release --locked --manifest-path host/Cargo.toml --target "$target"
name=pc-monitor-host
case "$target" in *windows*) name=pc-monitor-host.exe ;; esac
out="output/host/$target"
mkdir -p "$out"
cp "host/target/$target/release/$name" "$out/"
cp README.md "$out/"
case "$target" in *linux*) cp packaging/linux/70-pc-monitor.rules "$out/" ;; esac
tar -czf "output/pc-monitor-host-$target.tar.gz" -C "$out" .
printf 'Built %s/%s\n' "$out" "$name"
