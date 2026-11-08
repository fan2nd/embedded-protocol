#!/bin/sh
set -eu
cd "$(dirname "$0")/.."
mkdir -p output
(cd firmware && cargo build --release --locked)
rustc --edition=2021 -O tools/elf2uf2.rs -o output/elf2uf2
cp firmware/target/thumbv8m.main-none-eabihf/release/pc-monitor output/pc-monitor.elf
output/elf2uf2 output/pc-monitor.elf output/pc-monitor.uf2
cargo build --release --locked --manifest-path host/Cargo.toml
cp host/target/release/pc-monitor-host output/pc-monitor-host
