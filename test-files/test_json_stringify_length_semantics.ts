// JSON.stringify can return undefined at the top level. A following `.length`
// is then a property read on undefined and must throw, not silently return 0.

function observe(label: string, thunk: () => unknown) {
  try {
    console.log(label + ":value:" + thunk());
  } catch (e) {
    const err = e as Error;
    console.log(
      label +
        ":throw:" +
        (e instanceof TypeError) +
        ":" +
        String(err.message).includes("length"),
    );
  }
}

observe("undefined-direct", () => (JSON.stringify(undefined) as any).length);

const undefinedResult = JSON.stringify(undefined);
observe("undefined-local", () => (undefinedResult as any).length);

const fn = function stringifyLengthFn() {
  return 1;
};
observe("function-direct", () => (JSON.stringify(fn) as any).length);

const functionResult = JSON.stringify(fn);
observe("function-local", () => (functionResult as any).length);

observe("symbol-direct", () => (JSON.stringify(Symbol("s")) as any).length);

const symbolResult = JSON.stringify(Symbol.for("registered"));
observe("symbol-local", () => (symbolResult as any).length);

const toJSONUndefined = {
  toJSON: function () {
    return undefined;
  },
};
observe("tojson-direct", () => (JSON.stringify(toJSONUndefined) as any).length);

const toJSONResult = JSON.stringify(toJSONUndefined);
observe("tojson-local", () => (toJSONResult as any).length);

function rootUndefinedReplacer(key: string, value: unknown) {
  if (key === "") {
    return undefined;
  }
  return value;
}

observe(
  "replacer-direct",
  () => (JSON.stringify({ a: 1 }, rootUndefinedReplacer) as any).length,
);

const replacerResult = JSON.stringify({ a: 1 }, rootUndefinedReplacer);
observe("replacer-local", () => (replacerResult as any).length);

observe("string-result", () => JSON.stringify({ a: 1 }).length);
