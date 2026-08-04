### `gc-native-roots`: make the statepoint arm able to pass, and restore ELF coverage

The gate has been red on `main` since it landed — five consecutive runs — and
neither cause was a code regression.

**It could not pass.** `native-roots-aarch64` runs on `macos-14`, whose image
ships **Apple clang 15**, which does not register the `statepoint-example` GC
strategy. Every run died with `fatal error: error in backend: unsupported GC:
statepoint-example` before a single probe executed. That is CLAUDE.md's
failure mode inverted: a gate that cannot *pass* is as useless as one that
cannot fail, and it looked like a real regression for a day. The job now pins
Homebrew LLVM, as the RS4GC job beside it already did, and asserts the pinned
toolchain registers the strategy so a future image change fails with a clear
message rather than inside a probe compile. Newer Apple clang does register it
(21 does), so this is a property of the runner image, not of Apple clang.

**It stopped testing ELF.** Both GC arms run on macOS, so nothing exercised the
object format the compact map had to be reworked for. Every bug that reached
`main` in that area was ELF-only and invisible on Mach-O: the section needed
`SHF_GNU_RETAIN` or `--gc-sections` discarded it; it needed `SHF_WRITE` too, or
the relocated addresses forced a `DT_TEXTREL` in a PIE; and `eh_walker`'s asm
used the Mach-O underscore convention, so aarch64-Linux could not link at all.
A `native-roots-elf-aarch64` arm runs the same matrix on `ubuntu-24.04-arm`
with both liveness asserts (`.perry_gcmap` present **and** `.llvm_stackmaps`
absent). It needs no toolchain install — stock Ubuntu clang registers the
strategy, verified on 18.1.3.
