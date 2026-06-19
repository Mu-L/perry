declare function gc(): void;

function maybeGc(): void {
  if (typeof gc === "function") gc();
}

const payload = { marker: "payload", nested: { count: 7 } };
const receiver = { tag: "ctx" };

const args = {
  length: 2,
  get 0() {
    maybeGc();
    return payload;
  },
  get 1() {
    maybeGc();
    return "tail";
  },
};

function target(this: any, first: any, second: string): string {
  maybeGc();
  return `${this.tag}:${first.marker}:${first.nested.count}:${second}`;
}

const proxied = new Proxy(target, {
  apply(fn, thisArg, argArray) {
    maybeGc();
    return Reflect.apply(fn, thisArg, argArray);
  },
});

console.log("reflectApplyRoots:", Reflect.apply(target, receiver, args));
console.log("proxyApplyRoots:", Reflect.apply(proxied, receiver, [payload, "tail"]));
