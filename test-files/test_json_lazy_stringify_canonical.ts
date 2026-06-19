// Lazy JSON.parse must not stringify by copying the original source bytes.
// Whitespace, escape spellings, and number spellings are not canonical
// JSON.stringify output.

const parsed = JSON.parse(
  '[ 1, 2, { "a" : "\\u0062", "n" : 1.2300e+2 }, [ true, null ] ]',
);

const out = JSON.stringify(parsed);
console.log("canonical:" + out);
console.log("has-space:" + (out.indexOf(" ") >= 0));
console.log("has-unicode-escape:" + (out.indexOf("\\u0062") >= 0));
console.log("has-source-number:" + (out.indexOf("1.2300e+2") >= 0));
