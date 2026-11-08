#!/bin/sh
set -eu
cd "$(dirname "$0")/../.."
out=output/examples/rp2350
mkdir -p "$out"
cargo build --release --locked -p rp2350-monitor --target thumbv8m.main-none-eabihf
rustc --edition=2021 -O examples/rp2350/tools/elf2uf2.rs -o "$out/elf2uf2"
cp target/thumbv8m.main-none-eabihf/release/rp2350-monitor "$out/pc-monitor.elf"
"$out/elf2uf2" "$out/pc-monitor.elf" "$out/pc-monitor.uf2"
