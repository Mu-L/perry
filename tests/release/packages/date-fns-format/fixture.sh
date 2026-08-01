#!/usr/bin/env bash
set -uo pipefail
cd "$(dirname "$0")"
. "$(dirname "$0")/../_fixture_lib.sh"

NAME="date-fns-format"

if [[ "${1:-}" == "--__did-skip-marker" ]]; then
    exit 1
fi

fixture_setup "$NAME" || exit 1
fixture_compile_run_diff "$NAME"
