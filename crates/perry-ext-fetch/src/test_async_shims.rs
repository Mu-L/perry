use perry_ffi::Promise;
use std::ffi::c_void;

const TAG_UNDEFINED: u64 = 0x7FFC_0000_0000_0001;

#[no_mangle]
pub extern "C" fn perry_ffi_promise_new() -> *mut Promise {
    perry_runtime::promise::js_promise_new() as *mut Promise
}

#[no_mangle]
pub extern "C" fn perry_ffi_promise_resolve_bits(promise: *mut Promise, bits: u64) {
    perry_runtime::promise::js_promise_resolve(
        promise as *mut perry_runtime::Promise,
        f64::from_bits(bits),
    );
}

#[no_mangle]
pub extern "C" fn perry_ffi_promise_reject_bits(promise: *mut Promise, bits: u64) {
    perry_runtime::promise::js_promise_reject(
        promise as *mut perry_runtime::Promise,
        f64::from_bits(bits),
    );
}

#[no_mangle]
pub extern "C" fn perry_ffi_spawn_blocking(ctx: *mut c_void, invoke: extern "C" fn(*mut c_void)) {
    invoke(ctx);
}

// `perry-ext-fetch` enables `perry-runtime/external-fetch-symbols` in its test
// graph so the runtime's no-op fetch stubs cannot shadow this crate's real
// exports. The complete application link also contains perry-stdlib, which
// supplies these constructor/abort symbols; the isolated lib-test link does
// not. Keep the test-only providers here so `cargo test -p perry-ext-fetch`
// continues to exercise this crate without pulling in a second, conflicting
// implementation of the Fetch API.
#[no_mangle]
pub extern "C" fn js_blob_new(_parts: f64, _content_type: f64) -> f64 {
    f64::from_bits(TAG_UNDEFINED)
}

#[no_mangle]
pub extern "C" fn js_file_new(
    _parts: f64,
    _name: f64,
    _content_type: f64,
    _last_modified: f64,
) -> f64 {
    f64::from_bits(TAG_UNDEFINED)
}

#[no_mangle]
pub extern "C" fn js_headers_init_from_value(_handle: f64, _init: f64) -> f64 {
    f64::from_bits(TAG_UNDEFINED)
}

#[no_mangle]
pub extern "C" fn js_fetch_notify_signal_aborted(_signal_ptr: i64) {}
