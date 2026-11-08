#!/bin/sh
# Optional Windows GNU cross-linker. Zig provides MinGW CRT and import libraries.
set -eu
: "${PCMON_ZIG:?Set PCMON_ZIG to the zig executable}"
exec python3 - "$@" <<'PY'
import os
import sys
from pathlib import Path
# Zig supplies compiler-rt; linking Rust's copy as well duplicates stack probes.
args = [a for a in sys.argv[1:] if not Path(a).name.startswith('libcompiler_builtins-') and a not in (
    '-Wl,--disable-auto-image-base', '-lgcc', '-lgcc_eh', '-l:libpthread.a',
    '-Wl,-Bdynamic', '-Wl,-Bstatic', '-lmsvcrt', '-nodefaultlibs',
)]
os.execv(os.environ['PCMON_ZIG'], [os.environ['PCMON_ZIG'], 'cc', '-lunwind', '-target', 'x86_64-windows-gnu', *args])
PY
