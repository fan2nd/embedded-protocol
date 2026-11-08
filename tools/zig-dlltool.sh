#!/bin/sh
set -eu
: "${PCMON_ZIG:?Set PCMON_ZIG to the zig executable}"
exec "$PCMON_ZIG" dlltool "$@"
