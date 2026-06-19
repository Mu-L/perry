declare function gc(): void;

function maybeGc(): void {
  if (typeof gc === "function") gc();
}

const value = { marker: "defined", nested: { count: 11 } };
const descriptor: any = {};

Object.defineProperty(descriptor, "value", {
  enumerable: true,
  get() {
    maybeGc();
    return value;
  },
});

Object.defineProperty(descriptor, "writable", {
  enumerable: true,
  get() {
    maybeGc();
    return true;
  },
});

Object.defineProperty(descriptor, "enumerable", {
  enumerable: true,
  get() {
    maybeGc();
    return true;
  },
});

const bag: any = {};
Object.defineProperty(bag, "slot", {
  enumerable: true,
  get() {
    maybeGc();
    return descriptor;
  },
});

const target: any = {};
Object.defineProperties(target, bag);
maybeGc();

console.log("definePropertiesRoots:", target.slot.marker, target.slot.nested.count);
