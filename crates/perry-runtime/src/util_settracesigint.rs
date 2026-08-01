//! `util.setTraceSigInt(enable)` (#2514) — toggle printing a JS stack trace on
//! SIGINT. Current Node releases accept values outside the documented boolean
//! type without throwing; the call returns `undefined` for every input.
//!
//! Perry does not install a SIGINT stack-trace handler, so this is otherwise a
//! no-op matching Node's observable behavior.

use crate::value::TAG_UNDEFINED;

#[no_mangle]
pub extern "C" fn js_util_set_trace_sig_int(_enable: f64) -> f64 {
    f64::from_bits(TAG_UNDEFINED)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::value::TAG_TRUE;

    #[test]
    fn accepts_boolean_and_non_boolean_inputs() {
        for input in [f64::from_bits(TAG_TRUE), 1.0, f64::from_bits(TAG_UNDEFINED)] {
            assert_eq!(js_util_set_trace_sig_int(input).to_bits(), TAG_UNDEFINED);
        }
    }
}
