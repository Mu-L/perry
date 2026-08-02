#!/usr/bin/env bash
# Run UI doc-examples on an Android emulator via adb.
#
# This is the Android counterpart to scripts/run_simctl_tests.sh. Each UI
# example whose banner targets line includes `android` gets compiled with
# `perry compile --target android`, installed via `adb install`, launched
# with the PERRY_UI_TEST_MODE intent extra, and observed via adb logcat
# for the perry-ui-android exit-after-first-frame signal.
#
# Required: ANDROID_HOME (or ANDROID_SDK_ROOT), the `emulator` binary, an
# AVD configured (any), `adb`. Tier 10 of release_sweep.sh detects missing
# preconditions BEFORE running this script — but this script also checks
# them so it can be invoked standalone.
#
# Env:
#   ANDROID_AVD_NAME   — AVD to boot (default: first AVD listed by avdmanager)
#   PERRY_BIN          — path to perry (default: target/release/perry)
#   BOOT_TIMEOUT       — seconds to wait for boot complete (default: 180)
#   LAUNCH_TIMEOUT     — seconds per example (default: 60)
#   PERRY_TEST_SUMMARY_OUT — release_sweep.sh hook
#   KEEP_BOOTED        — if "1", don't shut down the emulator after run

set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
cd "$REPO_ROOT"

PERRY_BIN="${PERRY_BIN:-$REPO_ROOT/target/release/perry}"
BOOT_TIMEOUT="${BOOT_TIMEOUT:-180}"
LAUNCH_TIMEOUT="${LAUNCH_TIMEOUT:-60}"

# Resolve SDK
SDK="${ANDROID_HOME:-${ANDROID_SDK_ROOT:-}}"
if [[ -z "$SDK" ]]; then
    echo "android-emu: ANDROID_HOME / ANDROID_SDK_ROOT not set" >&2
    exit 2
fi

# Perry's Android auto-optimized runtime build consumes ANDROID_NDK_HOME to
# configure cc-rs and rustc with the API-24 clang wrappers. Android Studio's
# normal side-by-side NDK install only sets ANDROID_HOME, so discover the
# newest installed NDK when neither of the explicit NDK variables is set.
NDK="${ANDROID_NDK_HOME:-${ANDROID_NDK_ROOT:-}}"
if [[ -z "$NDK" && -d "$SDK/ndk" ]]; then
    NDK="$(find "$SDK/ndk" -mindepth 1 -maxdepth 1 -type d -print \
        | sort -V | tail -1)"
fi
if [[ -z "$NDK" && -d "$SDK/ndk-bundle" ]]; then
    NDK="$SDK/ndk-bundle"
fi
if [[ -z "$NDK" || ! -d "$NDK" ]]; then
    echo "android-emu: Android NDK not found (set ANDROID_NDK_HOME or install an SDK side-by-side NDK)" >&2
    exit 2
fi

case "$(uname -s)" in
    Darwin) NDK_HOST_TAG="darwin-x86_64" ;;
    Linux) NDK_HOST_TAG="linux-x86_64" ;;
    *)
        echo "android-emu: unsupported NDK host for this runner: $(uname -s)" >&2
        exit 2
        ;;
esac
NDK_CLANG="$NDK/toolchains/llvm/prebuilt/$NDK_HOST_TAG/bin/aarch64-linux-android24-clang"
if [[ ! -x "$NDK_CLANG" ]]; then
    echo "android-emu: NDK API-24 clang not found at $NDK_CLANG" >&2
    exit 2
fi
export ANDROID_NDK_HOME="$NDK"
NDK_BIN="$NDK/toolchains/llvm/prebuilt/$NDK_HOST_TAG/bin"
export CC_aarch64_linux_android="$NDK_CLANG"
export CXX_aarch64_linux_android="$NDK_BIN/aarch64-linux-android24-clang++"
export AR_aarch64_linux_android="$NDK_BIN/llvm-ar"
export CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER="$NDK_CLANG"

EMULATOR_BIN="$SDK/emulator/emulator"
[[ -x "$EMULATOR_BIN" ]] || EMULATOR_BIN="$(command -v emulator 2>/dev/null || true)"
ADB_BIN="$SDK/platform-tools/adb"
[[ -x "$ADB_BIN" ]] || ADB_BIN="$(command -v adb 2>/dev/null || true)"
AVDMANAGER_BIN="$SDK/cmdline-tools/latest/bin/avdmanager"
[[ -x "$AVDMANAGER_BIN" ]] || AVDMANAGER_BIN="$(command -v avdmanager 2>/dev/null || true)"

if [[ ! -x "$EMULATOR_BIN" ]] || [[ ! -x "$ADB_BIN" ]] || [[ ! -x "$AVDMANAGER_BIN" ]]; then
    echo "android-emu: required binary missing" >&2
    echo "  emulator:    ${EMULATOR_BIN:-(not found)}" >&2
    echo "  adb:         ${ADB_BIN:-(not found)}" >&2
    echo "  avdmanager:  ${AVDMANAGER_BIN:-(not found)}" >&2
    exit 2
fi

if [[ ! -x "$PERRY_BIN" ]]; then
    echo "android-emu: perry binary not found at $PERRY_BIN" >&2
    exit 2
fi

# Android UI bundles link a target-specific backend archive in addition to
# the auto-optimized runtime/stdlib pair. Build it once before starting the
# emulator so every example sees a current libperry_ui_android.a and a build
# failure does not leave an emulator idling while the fixture loop continues.
echo "android-emu: building Android UI backend..."
android_rustflags="${RUSTFLAGS:+$RUSTFLAGS }-Z tls-model=global-dynamic"
if ! RUSTC_BOOTSTRAP=1 RUSTFLAGS="$android_rustflags" \
        cargo build --release -p perry-ui-android --target aarch64-linux-android; then
    echo "android-emu: failed to build Android UI backend" >&2
    exit 1
fi

# Pick an AVD to boot
AVD="${ANDROID_AVD_NAME:-}"
if [[ -z "$AVD" ]]; then
    AVD="$("$AVDMANAGER_BIN" list avd | sed -nE 's/^[[:space:]]*Name:[[:space:]]+(.*)$/\1/p' | head -1)"
fi
if [[ -z "$AVD" ]]; then
    echo "android-emu: no AVD configured. Create one with avdmanager / Android Studio." >&2
    exit 2
fi

OUT_DIR="$REPO_ROOT/target/perry-android-tests"
mkdir -p "$OUT_DIR"

# `perry compile --target android` produces the native ARM64 shared object,
# not an installable APK. Package that object with the same Gradle template
# used by `perry run android`; keep one project warm across all examples so
# Gradle only has to replace libperry_app.so after the first build.
ANDROID_TEMPLATE="$REPO_ROOT/crates/perry-ui-android/template"
ANDROID_WRAPPER_SOURCE="$REPO_ROOT/android-build"
ANDROID_PACKAGE_DIR="$OUT_DIR/android-package"
if [[ ! -d "$ANDROID_TEMPLATE" || ! -x "$ANDROID_WRAPPER_SOURCE/gradlew" || \
      ! -d "$ANDROID_WRAPPER_SOURCE/gradle/wrapper" ]]; then
    echo "android-emu: Android Gradle template/wrapper missing" >&2
    exit 2
fi
if [[ -d "$ANDROID_PACKAGE_DIR" ]]; then
    find "$ANDROID_PACKAGE_DIR" -depth -delete
fi
mkdir -p "$ANDROID_PACKAGE_DIR/gradle/wrapper"
cp -R "$ANDROID_TEMPLATE/." "$ANDROID_PACKAGE_DIR/"
cp "$ANDROID_WRAPPER_SOURCE/gradlew" "$ANDROID_PACKAGE_DIR/gradlew"
cp "$ANDROID_WRAPPER_SOURCE/gradlew.bat" "$ANDROID_PACKAGE_DIR/gradlew.bat"
cp -R "$ANDROID_WRAPPER_SOURCE/gradle/wrapper/." "$ANDROID_PACKAGE_DIR/gradle/wrapper/"
chmod +x "$ANDROID_PACKAGE_DIR/gradlew"
ANDROID_JNI_DIR="$ANDROID_PACKAGE_DIR/app/src/main/jniLibs/arm64-v8a"
ANDROID_GRADLE_APK="$ANDROID_PACKAGE_DIR/app/build/outputs/apk/debug/app-debug.apk"
mkdir -p "$ANDROID_JNI_DIR"

echo "android-emu: AVD=$AVD"

# Boot emulator in background
"$EMULATOR_BIN" -avd "$AVD" -no-snapshot -no-audio -no-window -gpu swiftshader_indirect \
    > "$OUT_DIR/emulator.log" 2>&1 &
EMU_PID=$!

cleanup() {
    if [[ "${KEEP_BOOTED:-0}" != "1" ]]; then
        echo "android-emu: shutting down emulator..."
        "$ADB_BIN" emu kill >/dev/null 2>&1 || true
        kill "$EMU_PID" 2>/dev/null || true
        wait "$EMU_PID" 2>/dev/null || true
    fi
}
trap cleanup EXIT

# Wait for boot complete
echo "android-emu: waiting for boot complete (timeout ${BOOT_TIMEOUT}s)..."
deadline=$(( $(date +%s) + BOOT_TIMEOUT ))
while [[ $(date +%s) -lt $deadline ]]; do
    if "$ADB_BIN" shell getprop sys.boot_completed 2>/dev/null | grep -q 1; then
        echo "android-emu: boot complete"
        break
    fi
    sleep 2
done
if ! "$ADB_BIN" shell getprop sys.boot_completed 2>/dev/null | grep -q 1; then
    echo "android-emu: boot did not complete within ${BOOT_TIMEOUT}s" >&2
    exit 1
fi

# Iterate UI examples whose banner includes android
TOTAL=0
PASS=0
FAIL=0
FAILURES=()

while IFS= read -r -d '' src; do
    rel="${src#$REPO_ROOT/}"
    if ! head -15 "$src" | grep -qE "^// *targets:.*android"; then continue; fi

    TOTAL=$((TOTAL+1))
    stem="$(basename "${src%.ts}")"
    native_so="$OUT_DIR/${stem}.so"
    apk="$OUT_DIR/${stem}.apk"
    # The shared warm Gradle project deliberately retains the template's
    # applicationId. Every iteration uninstalls it before the next fixture.
    pkg_id="com.perry.template"

    echo "=== $rel ==="
    echo "  [+] perry compile --target android"
    if ! "$PERRY_BIN" compile --target android --app-bundle-id "$pkg_id" "$src" -o "$native_so" \
            > "$OUT_DIR/$stem.compile.log" 2>&1; then
        echo "  COMPILE_FAIL"
        FAIL=$((FAIL+1)); FAILURES+=("$rel COMPILE_FAIL")
        continue
    fi
    if [[ ! -s "$native_so" ]]; then
        echo "  NO_SHARED_OBJECT"
        FAIL=$((FAIL+1)); FAILURES+=("$rel NO_SHARED_OBJECT")
        continue
    fi

    echo "  [+] Gradle package"
    if ! cp "$native_so" "$ANDROID_JNI_DIR/libperry_app.so" ||
       ! (cd "$ANDROID_PACKAGE_DIR" && ./gradlew --console=plain :app:assembleDebug) \
            > "$OUT_DIR/$stem.package.log" 2>&1 ||
       [[ ! -s "$ANDROID_GRADLE_APK" ]] ||
       ! cp "$ANDROID_GRADLE_APK" "$apk"; then
        echo "  PACKAGE_FAIL"
        FAIL=$((FAIL+1)); FAILURES+=("$rel PACKAGE_FAIL")
        continue
    fi

    echo "  [+] adb install"
    if ! "$ADB_BIN" install -r "$apk" > "$OUT_DIR/$stem.install.log" 2>&1; then
        echo "  INSTALL_FAIL"
        FAIL=$((FAIL+1)); FAILURES+=("$rel INSTALL_FAIL")
        continue
    fi

    echo "  [+] adb shell am start (PERRY_UI_TEST_MODE)"
    "$ADB_BIN" logcat -c >/dev/null 2>&1 || true
    if ! "$ADB_BIN" shell am start \
            --es PERRY_UI_TEST_MODE 1 \
            --ei PERRY_UI_TEST_EXIT_AFTER_MS 500 \
            -n "${pkg_id}/com.perry.app.PerryActivity" \
            > "$OUT_DIR/$stem.run.log" 2>&1; then
        echo "  LAUNCH_FAIL"
        FAIL=$((FAIL+1)); FAILURES+=("$rel LAUNCH_FAIL")
        "$ADB_BIN" uninstall "$pkg_id" >/dev/null 2>&1 || true
        continue
    fi

    # Watch logcat for clean exit signal or crash
    deadline=$(( $(date +%s) + LAUNCH_TIMEOUT ))
    saw_exit=0
    while [[ $(date +%s) -lt $deadline ]]; do
        if "$ADB_BIN" logcat -d -s PerryUI:I 2>/dev/null | grep -qE "test-mode exit"; then
            saw_exit=1
            break
        fi
        if "$ADB_BIN" logcat -d 2>/dev/null | grep -qE "FATAL EXCEPTION|FORCE_FINISHING|Process: $pkg_id" | grep -qE "FATAL|FORCE"; then
            break
        fi
        sleep 1
    done

    # Preserve the complete device log before uninstalling the package. This
    # makes native loader/JNI crashes diagnosable from release-sweep artifacts
    # instead of losing the only evidence when the emulator shuts down.
    "$ADB_BIN" logcat -d > "$OUT_DIR/$stem.logcat.log" 2>&1 || true
    "$ADB_BIN" uninstall "$pkg_id" >/dev/null 2>&1 || true

    if [[ "$saw_exit" -eq 1 ]]; then
        echo "  PASS"
        PASS=$((PASS+1))
    else
        echo "  TIMEOUT_OR_CRASH"
        FAIL=$((FAIL+1)); FAILURES+=("$rel TIMEOUT_OR_CRASH")
    fi
done < <(find "$REPO_ROOT/docs/examples" -name "*.ts" -print0)

echo
echo "android-emu: $PASS/$TOTAL passed, $FAIL failed"
[[ $FAIL -gt 0 ]] && printf '  %s\n' "${FAILURES[@]+${FAILURES[@]}}"

if [[ -n "${PERRY_TEST_SUMMARY_OUT:-}" ]]; then
    cat > "$PERRY_TEST_SUMMARY_OUT" <<EOF
{"script": "run_android_emu_tests.sh", "passed": $PASS, "failed": $FAIL, "skipped": 0, "total": $TOTAL, "platform": "android", "avd": "$AVD"}
EOF
fi

[[ $FAIL -eq 0 ]]
