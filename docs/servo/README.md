# In-process Servo web engine (experimental)

`perry-ui-macos` can optionally use [Servo](https://servo.org) as an alternative
web engine to the system WKWebView, selected at runtime via `PERRY_WEBVIEW=servo`.
Enabled by the `servo-webview` cargo feature (off by default — pulls the full
Servo stack incl. SpiderMonkey).

## Dependency conflicts (and how they're resolved)

Embedding Servo in-process means unifying its ~835-crate tree with perry's. Two
hard version conflicts had to be resolved:

1. **`libsqlite3-sys`** (`links = "sqlite3"` → one version per workspace).
   Servo pulls `rusqlite 0.37 → libsqlite3-sys 0.35`; perry pinned older.
   Fixed by upgrading perry's sqlite stack: `rusqlite 0.32→0.37` and
   `sqlx 0.8.6→0.9.0` (see the `chore/align-libsqlite3-for-servo` PR). Clean,
   stable upgrade — landed independently of Servo.

2. **`ml-kem`** (post-quantum KEM, used by both perry's WebCrypto and Servo's).
   Perry needs `ml-kem 0.3.2` + the `pkcs8` feature (stable `kem 0.3.0`); Servo's
   `servo-script` needs `ml-kem 0.2.x`, which pins a *pre-release* `kem
   =0.3.0-pre.0` and lacks `pkcs8`. Irreconcilable in one workspace.
   Fixed by **forking `servo-script`** and migrating its WebCrypto ML-KEM code
   from ml-kem 0.2 → 0.3.2 (so both sides share 0.3.2). The migration is captured
   in `servo-script-ml-kem-0.3.2.patch` (332 lines, 3 files) and applied via a
   local `[patch.crates-io]`.

   `ml-dsa` (the sibling ML-DSA/Dilithium crate Servo also uses) is **not** a
   conflict — perry doesn't depend on it.

## Reproducing the fork

```sh
# 1. Vendor servo-script from the registry into a sibling dir:
cp -R ~/.cargo/registry/src/*/servo-script-0.1.0 ../servo-forks/servo-script
chmod -R u+w ../servo-forks/servo-script
# 2. Apply the ml-kem 0.3.2 migration:
cd ../servo-forks/servo-script && patch -p0 < .../docs/servo/servo-script-ml-kem-0.3.2.patch
```

The workspace `Cargo.toml` then carries:
```toml
[patch.crates-io]
servo-script = { path = "../servo-forks/servo-script" }
```

For a landable PR the fork needs a permanent home (git fork/submodule or in-tree
vendor) rather than the local path above.

## ML-KEM migration: correctness note

The migration is **type-correct** (the full Servo stack compiles). Behavioral
equivalence was reasoned per call-site (`from_seed` ≡ `generate_deterministic`,
`ExpandedKeyEncoding::to_expanded_bytes` ≡ the old `EncodedSizeUser::as_bytes`,
no-arg `encapsulate()` OS entropy ≡ `OsRng`) but **not** verified against Servo's
WebCrypto ML-KEM test vectors. ML-KEM WebCrypto is a niche surface; page
rendering is unaffected. Verify with Servo's WPT WebCrypto suite before relying
on it.
