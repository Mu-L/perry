#!/usr/bin/env bash
# Tier 1 — cargo_workspace
#
# Runs every host-compatible package's release tests. Packages are deliberately
# tested one Cargo invocation at a time: a multi-package workspace invocation
# unifies perry-runtime features, so perry-ext-fetch's
# `external-fetch-symbols` leaks into unrelated test binaries that do not link
# the fetch implementation and they fail with undefined `js_fetch_*` symbols.
# This mirrors the full cargo-test CI job's package isolation rule.

set -uo pipefail
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
. "$SCRIPT_DIR/../release_sweep_lib.sh"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"

OUT="${PERRY_RELEASE_SWEEP_OUTPUT:?PERRY_RELEASE_SWEEP_OUTPUT not set}"
TIER_DIR="$(sweep_tier_dir "$OUT" 1)"
LOG="$TIER_DIR/cargo_workspace.log"
SUMMARY="$TIER_DIR/summary.json"

host="$(sweep_host_detect)"

EXCLUDES_COMMON=(
    perry-ui-ios
    perry-ui-tvos
    perry-ui-watchos
    perry-ui-visionos
    perry-ui-android
)
case "$host" in
    macos)   EXCLUDES=("${EXCLUDES_COMMON[@]}" perry-ui-windows perry-ui-gtk4) ;;
    linux)   EXCLUDES=("${EXCLUDES_COMMON[@]}" perry-ui-macos perry-ui-windows) ;;
    windows) EXCLUDES=("${EXCLUDES_COMMON[@]}" perry-ui-macos perry-ui-gtk4) ;;
    *)       EXCLUDES=("${EXCLUDES_COMMON[@]}") ;;
esac

is_excluded() {
    local candidate="$1"
    local excluded
    for excluded in "${EXCLUDES[@]}"; do
        [[ "$candidate" == "$excluded" ]] && return 0
    done
    return 1
}

start="$(date +%s)"
{
    echo "tier 1 cargo_workspace — host=$host"
    echo "command: cargo test --release -p <package> (serial, excluding: ${EXCLUDES[*]})"
    echo
} > "$LOG"

# Keep metadata failure on the structured-result path even if this script is
# invoked by a caller that enabled errexit before sourcing it.
case "$-" in
    *e*) had_errexit=1 ;;
    *) had_errexit=0 ;;
esac
set +e
package_list="$(
    cd "$REPO_ROOT" && cargo metadata --no-deps --format-version 1 |
        python3 -c 'import json, sys; print("\n".join(sorted(p["name"] for p in json.load(sys.stdin)["packages"])))'
)"
metadata_rc=$?

rc=$metadata_rc
failed_packages=()
if [[ "$rc" -eq 0 ]]; then
    # Perry integration tests compile with PERRY_NO_AUTO_OPTIMIZE=1 and link
    # these archives directly. Build the wrappers separately so the standalone
    # runtime archive retains its runtime-only feature set.
    (cd "$REPO_ROOT" && cargo build --release -p perry-runtime-static) >> "$LOG" 2>&1
    rc=$?
    if [[ "$rc" -eq 0 ]]; then
        (cd "$REPO_ROOT" && cargo build --release -p perry-stdlib-static) >> "$LOG" 2>&1
        rc=$?
    fi
fi
if [[ "$rc" -eq 0 ]]; then
    tested_packages=0
    while IFS= read -r package; do
        [[ -z "$package" ]] && continue
        is_excluded "$package" && continue
        tested_packages=$((tested_packages + 1))
        echo "=== cargo test --release -p $package ===" >> "$LOG"
        if [[ "$package" == "perry-runtime" ]]; then
            (cd "$REPO_ROOT" && RUST_TEST_THREADS=1 cargo test --release -p "$package") >> "$LOG" 2>&1
        else
            (cd "$REPO_ROOT" && cargo test --release -p "$package") >> "$LOG" 2>&1
        fi
        package_rc=$?
        if [[ "$package_rc" -ne 0 ]]; then
            rc=$package_rc
            failed_packages+=("$package")
        fi
        # Release test executables are large and are not inputs to later package
        # builds. Keep libraries/proc macros, prune only completed executables.
        find "$REPO_ROOT/target/release/deps" -maxdepth 1 -type f -perm -111 \
            ! -name '*.dylib' ! -name '*.so' ! -name '*.dll' \
            -delete 2>/dev/null || true
    done <<< "$package_list"
    if [[ "$tested_packages" -eq 0 ]]; then
        echo "no packages were tested; package_list was empty or fully excluded" >> "$LOG"
        rc=1
        failed_packages+=("no-packages-tested")
    fi
fi
if [[ "$had_errexit" -eq 1 ]]; then
    set -e
else
    set +e
fi

# Try to extract per-crate test counts from the log.
# `cargo test` prints lines like "test result: ok. 12 passed; 0 failed; 0 ignored ..."
# at the end of each crate's run. We sum those.
#
# Defensive parsing: `grep -c PATTERN` exits 1 (and prints "0") on no match,
# so the naive `$(grep -c ... || echo 0)` produces multi-line output ("0\n0")
# that breaks downstream arithmetic. Capture, then validate integer.
total_passed=$(grep -cE 'test result: ok\.' "$LOG" 2>/dev/null || true)
total_failed=$(grep -cE 'test result: FAILED' "$LOG" 2>/dev/null || true)
[[ "$total_passed" =~ ^[0-9]+$ ]] || total_passed=0
[[ "$total_failed" =~ ^[0-9]+$ ]] || total_failed=0

end="$(date +%s)"
dur="$((end - start))"

cat > "$SUMMARY" <<EOF
{"script": "tier01_cargo_workspace.sh", "passed": $total_passed, "failed": $total_failed, "skipped": 0, "host": "$host", "exit_code": $rc}
EOF

if [[ "$rc" -eq 0 ]]; then
    sweep_tier_emit "$OUT" 1 "cargo_workspace" "PASS" "$dur" "$total_passed crate-suites passed"
else
    failed_list="${failed_packages[*]:-setup}"
    sweep_tier_emit "$OUT" 1 "cargo_workspace" "FAIL" "$dur" \
        "cargo test exited $rc ($total_failed crate-suites failed; packages: $failed_list)"
fi
