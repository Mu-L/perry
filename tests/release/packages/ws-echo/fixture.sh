#!/usr/bin/env bash
set -uo pipefail
cd "$(dirname "$0")"
. "$(dirname "$0")/../_fixture_lib.sh"

# Avoid colliding with another release sweep on the same host.
export PERRY_WS_PORT="${PERRY_WS_PORT:-$((20000 + RANDOM % 20000))}"

fixture_setup "ws-echo" || exit 1
fixture_compile_run_diff "ws-echo"
