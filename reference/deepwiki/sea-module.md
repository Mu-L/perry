# DeepWiki Response

- Repository: `PerryTS/perry`
- Question: In PerryTS/perry, how are small Node builtin modules wired into the API manifest, native module shape, and runtime dispatch? For a node:sea module with top-level functions isSea, getAsset, getAssetAsBlob, getRawAsset, and getAssetKeys, which existing files and patterns should be followed?
- DeepWiki search: https://deepwiki.com/search/in-perrytsperry-how-are-small_c1ab8747-f236-414a-b781-9fd2207190e3

In PerryTS/perry, small Node.js builtin modules are integrated through a combination of the API manifest, native module definitions, and runtime dispatch mechanisms. To add a new `node:sea` module with top-level functions like `isSea`, `getAsset`, `getAssetAsBlob`, `getRawAsset`, and `getAssetKeys`, you would primarily interact with the `perry-api-manifest` crate to define the module's API surface, and the `perry-runtime` crate for its runtime implementation.

### API Manifest Integration

The `perry-api-manifest` crate serves as the source of truth for all implemented stdlib and native APIs in Perry [[1](#ref-1)].

1.  **Declare the Module**: First, the `node:sea` module needs to be added to the `NATIVE_MODULES` constant in `crates/perry-api-manifest/src/entries.rs` [[2](#ref-2)]. This list identifies modules that Perry recognizes as native and can resolve without falling back to a V8 runtime [[3](#ref-3)].
2.  **Define API Entries**: For each function (`isSea`, `getAsset`, etc.), an `ApiEntry` needs to be added to the `API_MANIFEST` constant in `crates/perry-api-manifest/src/entries.rs` [[4](#ref-4)]. These entries specify the module, function name, whether it has a receiver, and its parameter and return types [[5](#ref-5)]. The `method_sig` helper function can be used for methods with declared parameters and return types [[5](#ref-5)].
    *   The `perry-hir` crate uses `module_has_symbol` to validate API calls against this manifest during lowering, ensuring that only implemented APIs are used [[6](#ref-6)] [[7](#ref-7)].
    *   The `perry-codegen` crate also uses this manifest to ensure its native dispatch table is aligned [[8](#ref-8)].

### Native Module Shape and Runtime Dispatch

The runtime dispatch for Node.js submodules is handled within the `perry-runtime` crate, specifically in `crates/perry-runtime/src/node_submodules/mod.rs` [[9](#ref-9)].

1.  **Create a new submodule file**: You would create a new file, for example, `crates/perry-runtime/src/node_submodules/sea.rs`, to house the implementations of your `node:sea` functions.
2.  **Implement functions**: Each top-level function (`isSea`, `getAsset`, etc.) would be implemented as an `extern "C"` function in `sea.rs`. These functions will interact with Perry's internal runtime values (NaN-boxed values) [[10](#ref-10)].
3.  **Register functions**: In `crates/perry-runtime/src/node_submodules/mod.rs`, you would import your new `sea.rs` module [[11](#ref-11)]. Then, you would register these functions to be exposed as part of the `node:sea` namespace. This typically involves creating "thunk" functions that handle the FFI boundary and then registering them in a dispatch table. The existing pattern for other submodules like `stream/promises` or `timers/promises` can be followed [[12](#ref-12)].
    *   The `perry-codegen` crate uses `iter_native_method_signatures` to walk the native-module dispatch table and ensure consistency with the API manifest [[13](#ref-13)].

### Example Pattern to Follow

For the `node:sea` module, you would follow the pattern established by other Node.js submodules. For instance, `node:stream/promises` functions like `pipeline` and `finished` are implemented as `thunk_streamP_pipeline` and `thunk_streamP_finished` respectively [[14](#ref-14)]. These thunks are then exposed through the `node_submodules` system.

You would need to:
1.  Add `"sea"` to `NODE_SUBMODULES` in `crates/perry-api-manifest/src/entries.rs` [[15](#ref-15)].
2.  Add `ApiEntry` definitions for `isSea`, `getAsset`, `getAssetAsBlob`, `getRawAsset`, and `getAssetKeys` to `API_MANIFEST` in `crates/perry-api-manifest/src/entries.rs`.
3.  Create `crates/perry-runtime/src/node_submodules/sea.rs` and implement the `extern "C"` functions for each of your `node:sea` functions.
4.  In `crates/perry-runtime/src/node_submodules/mod.rs`, import `sea.rs` and register your functions, similar to how `consumers` functions are handled [[16](#ref-16)].

## Notes

The `perry-api-manifest` crate is crucial for defining the public API surface of native modules, which is then used by `perry-hir` for compile-time checks and by `perry-codegen` for generating dispatch metadata [[17](#ref-17)]. The `perry-runtime` crate handles the actual implementation and runtime dispatch of these native functions [[9](#ref-9)]. The `docs/src/native-libraries/manifest-v1.md` provides a detailed specification for the `perry.nativeLibrary` manifest, which is relevant for external native bindings but the internal `perry-api-manifest` follows a similar structure [[18](#ref-18)].

Wiki pages you might want to explore:
- [Language Support & TypeScript Compatibility (PerryTS/perry)](/wiki/PerryTS/perry#1.3)
- [Glossary (PerryTS/perry)](/wiki/PerryTS/perry#14)
- [Internationalization (i18n) (PerryTS/perry)](/wiki/PerryTS/perry#7)

## References

<a id="ref-1"></a>
[1] [`Repo PerryTS/perry: crates/perry-api-manifest/src/lib.rs`](https://github.com/PerryTS/perry/blob/c720d2a4/crates/perry-api-manifest/src/lib.rs#L1-L1) lines 1-1

```rs
//! Source-of-truth manifest of stdlib / native APIs Perry implements.
```

<a id="ref-2"></a>
[2] [`Repo PerryTS/perry: crates/perry-api-manifest/src/entries.rs`](https://github.com/PerryTS/perry/blob/c720d2a4/crates/perry-api-manifest/src/entries.rs#L30-L121) lines 30-121

```rs
pub const NATIVE_MODULES: &[&str] = &[
    "mysql2",
    "mysql2/promise",
    "pg",
    "uuid",
    "bcrypt",
    "argon2",
    "ioredis",
    "axios",
    "node-fetch",
    "ws",
    "zlib",
    "crypto",
    "dotenv",
    "dotenv/config",
    "jsonwebtoken",
    "nanoid",
    "slugify",
    "validator",
    "ethers",
    "mongodb",
    "better-sqlite3",
    "tursodb",
    "iroh",
    "node-cron",
    "nodemailer",
    "http",
    "https",
    "http2",
    "events",
    "os",
    "buffer",
    "assert",
    "assert/strict",
    "child_process",
    "net",
    "tls",
    "stream",
    "streams",
    "fs",
    "path",
    "console",
    "util",
    "util/types",
    "url",
    "lru-cache",
    "commander",
    "decimal.js",
    "bignumber.js",
    "exponential-backoff",
    "lodash",
    "dayjs",
    "date-fns",
    "moment",
    "sharp",
    "cheerio",
    "cron",
    "fastify",
    "async_hooks",
    "readline",
    "string_decoder",
    "querystring",
    "cluster",
    "tty",
    "perf_hooks",
    "process",
    "perry/tui",
    "perry/ui",
    "perry/system",
    "perry/plugin",
    "perry/widget",
    "perry/i18n",
    "worker_threads",
    "perry/thread",
    "perry/updater",
    "perry/media",
    "perry/background",
    "redis",
    "rate-limiter-flexible",
    "fetch",
    // `@perryts/pdf` — official PDF creation package (#516).
    // Bundled wrapper lives in `crates/perry-ext-pdf`; the producer
    // side companion to the existing PdfView widget. d.ts at
    // `types/perry/pdf/index.d.ts`.
    "@perryts/pdf",
    // `perry/ads` — official in-app advertising package (#867).
    // MVP scaffold: bundled wrapper at `crates/perry-ext-ads`
    // returns structured `{ error: "no-sdk-linked" }` placeholders
    // until real Google Mobile Ads SDK integration lands. d.ts at
    // `types/perry/ads/index.d.ts`.
    "perry/ads",
];
```

<a id="ref-3"></a>
[3] [`Repo PerryTS/perry: crates/perry-api-manifest/src/entries.rs`](https://github.com/PerryTS/perry/blob/c720d2a4/crates/perry-api-manifest/src/entries.rs#L25-L29) lines 25-29

```rs
/// Module specifiers Perry recognizes as native (i.e. resolvable
/// without going through the V8 fallback). Migrated from
/// `crates/perry-hir/src/ir.rs::NATIVE_MODULES` so the manifest can
/// answer module-resolution questions without depending on
/// `perry-hir`. Order matches the original list to keep diffs minimal.
```

<a id="ref-4"></a>
[4] [`Repo PerryTS/perry: crates/perry-api-manifest/src/entries.rs`](https://github.com/PerryTS/perry/blob/c720d2a4/crates/perry-api-manifest/src/entries.rs#L164-L177) lines 164-177

```rs
    ApiEntry {
        module,
        name,
        kind: ApiKind::Method {
            has_receiver,
            class_filter,
        },
        source: ApiSource::Stdlib,
        stub: false,
        abi_version: None,
        params: &[],
        returns: TypeSpec::Any,
    }
}
```

<a id="ref-5"></a>
[5] [`Repo PerryTS/perry: crates/perry-api-manifest/src/entries.rs`](https://github.com/PerryTS/perry/blob/c720d2a4/crates/perry-api-manifest/src/entries.rs#L182-L202) lines 182-202

```rs
const fn method_sig(
    module: &'static str,
    name: &'static str,
    has_receiver: bool,
    class_filter: Option<&'static str>,
    params: &'static [ParamSpec],
    returns: TypeSpec,
) -> ApiEntry {
    ApiEntry {
        module,
        name,
        kind: ApiKind::Method {
            has_receiver,
            class_filter,
        },
        source: ApiSource::Stdlib,
        stub: false,
        abi_version: None,
        params,
        returns,
    }
```

<a id="ref-6"></a>
[6] [`Repo PerryTS/perry: crates/perry-api-manifest/src/lib.rs`](https://github.com/PerryTS/perry/blob/c720d2a4/crates/perry-api-manifest/src/lib.rs#L5-L6) lines 5-6

```rs
//! - **perry-hir** consults [`module_has_symbol`] during HIR lowering to
//!   reject references to unimplemented APIs at compile time (#463).
```

<a id="ref-7"></a>
[7] [`Repo PerryTS/perry: crates/perry-api-manifest/src/lib.rs`](https://github.com/PerryTS/perry/blob/c720d2a4/crates/perry-api-manifest/src/lib.rs#L177-L197) lines 177-197

```rs
pub fn module_has_symbol(module: &str, name: &str) -> Option<&'static ApiEntry> {
    let module = module.strip_prefix("node:").unwrap_or(module);
    // Match either:
    //  - a top-level export by name (`ethers.parseEther` → entry.name = parseEther)
    //  - any method whose class_filter is the requested name (`ethers.Wallet`
    //    → some entry has Method { class_filter: Some("Wallet") }). Without
    //    this branch, `ethers.Wallet.createRandom()` failed the #463
    //    unimplemented gate even though `createRandom` was registered with
    //    class_filter=Wallet.
    API_MANIFEST.iter().find(|e| {
        if e.module != module {
            return false;
        }
        if e.name == name {
            return true;
        }
        matches!(
            e.kind,
            ApiKind::Method { class_filter: Some(c), .. } if c == name
        )
    })
```

<a id="ref-8"></a>
[8] [`Repo PerryTS/perry: crates/perry-api-manifest/src/lib.rs`](https://github.com/PerryTS/perry/blob/c720d2a4/crates/perry-api-manifest/src/lib.rs#L7-L9) lines 7-9

```rs
//! - **perry-codegen** keeps its native dispatch table aligned with this
//!   manifest via a CI test (`tests/manifest_consistency.rs`) — the
//!   manifest is the entry list, codegen owns the dispatch metadata.
```

<a id="ref-9"></a>
[9] [`Repo PerryTS/perry: crates/perry-runtime/src/node_submodules/mod.rs`](https://github.com/PerryTS/perry/blob/c720d2a4/crates/perry-runtime/src/node_submodules/mod.rs#L1-L22) lines 1-22

```rs
//! Issue #841 — wire up named exports + namespace imports for five
//! Node.js submodules that Perry's manifest had registered but whose
//! FFI export tables defaulted to a `TAG_TRUE` sentinel cell:
//!
//!   - `node:timers/promises` (setTimeout / setImmediate / setInterval / scheduler.*)
//!   - `node:readline/promises` (createInterface, Interface, Readline)
//!   - `node:stream/promises` (pipeline, finished)
//!   - `node:stream/consumers` (text, json, buffer, arrayBuffer, bytes, blob)
//!   - `node:sys` (deprecated alias for node:util — re-exports format, inspect, etc.)
//!
//! Pre-fix `import { setTimeout } from "node:timers/promises"; typeof setTimeout`
//! reported `"boolean"` (the value was literally `true`) and `import * as ns
//! from "node:..."` errored at compile time with the "switch to named imports"
//! diagnostic. This module ships per-export function singletons whose `typeof`
//! is `"function"`, plus per-submodule namespace stubs whose properties point
//! at the same singletons.
//!
//! Most thunks are deliberately minimal — they throw `Error("<api> is not yet
//! implemented in Perry")` when invoked. `node:stream/consumers` is the first
//! submodule here with concrete behavior, so consuming code can import and use
//! its helpers while the broader #793 Node compatibility roadmap continues.

```

<a id="ref-10"></a>
[10] [`Repo PerryTS/perry: crates/perry-runtime/src/node_submodules/mod.rs`](https://github.com/PerryTS/perry/blob/c720d2a4/crates/perry-runtime/src/node_submodules/mod.rs#L36-L36) lines 36-36

```rs
use crate::value::JSValue;
```

<a id="ref-11"></a>
[11] [`Repo PerryTS/perry: crates/perry-runtime/src/node_submodules/mod.rs`](https://github.com/PerryTS/perry/blob/c720d2a4/crates/perry-runtime/src/node_submodules/mod.rs#L107-L112) lines 107-112

```rs
mod blob;
mod consumers;
mod fs_promises;
mod hono_jsx;
mod stream_promises;
mod timers;
```

<a id="ref-12"></a>
[12] [`Repo PerryTS/perry: crates/perry-runtime/src/node_submodules/mod.rs`](https://github.com/PerryTS/perry/blob/c720d2a4/crates/perry-runtime/src/node_submodules/mod.rs#L143-L149) lines 143-149

```rs
use stream_promises::{thunk_streamP_finished, thunk_streamP_pipeline, value_from_ptr};
use timers::{
    timers_ns_clear_immediate, timers_ns_clear_interval, timers_ns_clear_timeout,
    timers_ns_set_immediate, timers_ns_set_interval, timers_ns_set_timeout,
    timers_promises_scheduler, timers_promises_scheduler_wait, timers_promises_scheduler_yield,
    timers_promises_set_immediate, timers_promises_set_interval, timers_promises_set_timeout,
};
```

<a id="ref-13"></a>
[13] [`Repo PerryTS/perry: crates/perry-codegen/src/lib.rs`](https://github.com/PerryTS/perry/blob/c720d2a4/crates/perry-codegen/src/lib.rs#L69-L83) lines 69-83

```rs
/// Walk every entry in the native-module dispatch table.
/// `perry-api-manifest`'s consistency test consumes this to verify
/// the manifest is in sync with the dispatch table. Stable iteration
/// order — declaration order in `lower_call.rs::NATIVE_MODULE_TABLE`.
pub fn iter_native_method_signatures() -> impl Iterator<Item = NativeMethodRef> {
    lower_call::iter_native_module_table().map(
        |(module, has_receiver, method, class_filter, arg_kinds, ret_kind)| NativeMethodRef {
            module,
            has_receiver,
            method,
            class_filter,
            arg_kinds,
            ret_kind,
        },
    )
```

<a id="ref-14"></a>
[14] [`Repo PerryTS/perry: crates/perry-runtime/src/node_submodules/mod.rs`](https://github.com/PerryTS/perry/blob/c720d2a4/crates/perry-runtime/src/node_submodules/mod.rs#L143-L143) lines 143-143

```rs
use stream_promises::{thunk_streamP_finished, thunk_streamP_pipeline, value_from_ptr};
```

<a id="ref-15"></a>
[15] [`Repo PerryTS/perry: crates/perry-api-manifest/src/entries.rs`](https://github.com/PerryTS/perry/blob/c720d2a4/crates/perry-api-manifest/src/entries.rs#L127-L127) lines 127-127

```rs
pub const NODE_SUBMODULES: &[&str] = &["stream/promises"];
```

<a id="ref-16"></a>
[16] [`Repo PerryTS/perry: crates/perry-runtime/src/node_submodules/mod.rs`](https://github.com/PerryTS/perry/blob/c720d2a4/crates/perry-runtime/src/node_submodules/mod.rs#L122-L125) lines 122-125

```rs
use consumers::{
    thunk_consumers_arrayBuffer, thunk_consumers_blob, thunk_consumers_buffer,
    thunk_consumers_bytes, thunk_consumers_json, thunk_consumers_text,
};
```

<a id="ref-17"></a>
[17] [`Repo PerryTS/perry: crates/perry-api-manifest/src/lib.rs`](https://github.com/PerryTS/perry/blob/c720d2a4/crates/perry-api-manifest/src/lib.rs#L1-L16) lines 1-16

```rs
//! Source-of-truth manifest of stdlib / native APIs Perry implements.
//!
//! Three consumers:
//!
//! - **perry-hir** consults [`module_has_symbol`] during HIR lowering to
//!   reject references to unimplemented APIs at compile time (#463).
//! - **perry-codegen** keeps its native dispatch table aligned with this
//!   manifest via a CI test (`tests/manifest_consistency.rs`) — the
//!   manifest is the entry list, codegen owns the dispatch metadata.
//! - **perry's docs / .d.ts emit** iterates entries to produce an
//!   external view of the supported surface (#465).
//!
//! The schema is also the foundation for #466 Phase 2's external
//! `perry.nativeLibrary` manifest spec — third-party native bindings
//! will declare entries with the same shape, just `source: External`
//! instead of `Stdlib`.
```

<a id="ref-18"></a>
[18] [`Repo PerryTS/perry: docs/src/native-libraries/manifest-v1.md`](https://github.com/PerryTS/perry/blob/c720d2a4/docs/src/native-libraries/manifest-v1.md#L1-L10) lines 1-10

```md
# `perry.nativeLibrary` manifest — spec v1

> **New here?** Start with [Native Bindings — Overview](overview.md)
> for the architectural picture and the
> [Authoring Guide](authoring-guide.md) for a step-by-step that uses
> this manifest. This page is reference-grade detail.

This page is the authoritative spec for the `perry.nativeLibrary`
field a native-bindings package declares in its `package.json`. The
Perry compiler reads this manifest at resolve time and uses it to:
```
