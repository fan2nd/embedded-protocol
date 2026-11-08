#!/bin/sh
# Optional Apple Silicon cross-build of the three bundled executables.
# Prerequisites: Rust targets x86_64-pc-windows-gnu, x86_64-unknown-linux-musl,
# aarch64-apple-darwin; Zig 0.13.0; Python 3; Xcode Command Line Tools.
set -eu
cd "$(dirname "$0")/.."
root=$(pwd)
: "${PCMON_ZIG:?Set PCMON_ZIG to the absolute path of Zig 0.13.0}"
export PCMON_ZIG
export ZIG_GLOBAL_CACHE_DIR="$root/target/zig-cache"
export CARGO_PROFILE_RELEASE_DEBUG=0
sh tools/build-host.sh aarch64-apple-darwin
sysroot=$(rustc --print sysroot)
host=$(rustc -vV | sed -n 's/^host: //p')
CARGO_TARGET_X86_64_UNKNOWN_LINUX_MUSL_LINKER="$sysroot/lib/rustlib/$host/bin/rust-lld" \
    CARGO_PROFILE_RELEASE_STRIP=symbols sh tools/build-host.sh x86_64-unknown-linux-musl
CARGO_TARGET_X86_64_PC_WINDOWS_GNU_LINKER="$root/tools/zig-linker.sh" \
    CARGO_TARGET_X86_64_PC_WINDOWS_GNU_RUSTFLAGS="-C dlltool=$root/tools/zig-dlltool.sh -C link-self-contained=no" \
    CARGO_PROFILE_RELEASE_STRIP=symbols CARGO_PROFILE_RELEASE_PANIC=abort \
    sh tools/build-host.sh x86_64-pc-windows-gnu
python3 examples/rp2350/tools/make_disk.py
