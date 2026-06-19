declare function gc(): void;

function maybeGc(): void {
  if (typeof gc === "function") gc();
}

const payload = { marker: "payload", nested: { count: 23 } };
const target: any = {};
const handler: any = {
  prefix: "trap",
  get(t: any, key: string) {
    maybeGc();
    const value = t[key];
    return value && value.nested ? `${this.prefix}:${value.marker}:${value.nested.count}` : value;
  },
  set(t: any, key: string, value: any) {
    maybeGc();
    t[key] = value;
    return true;
  },
  has(t: any, key: string) {
    maybeGc();
    return key in t;
  },
  deleteProperty(t: any, key: string) {
    maybeGc();
    return delete t[key];
  },
  defineProperty(t: any, key: string, descriptor: PropertyDescriptor) {
    maybeGc();
    return Reflect.defineProperty(t, key, descriptor);
  },
  isExtensible(t: any) {
    maybeGc();
    return Object.isExtensible(t);
  },
  preventExtensions(t: any) {
    maybeGc();
    Object.preventExtensions(t);
    return true;
  },
};

const proxy: any = new Proxy(target, handler);

proxy.slot = payload;
console.log("proxyTrapRoots:get", proxy.slot);
console.log("proxyTrapRoots:has", "slot" in proxy);
console.log(
  "proxyTrapRoots:define",
  Reflect.defineProperty(proxy, "extra", {
    value: { marker: "extra", nested: { count: 5 } },
    configurable: true,
  }),
);
console.log("proxyTrapRoots:extra", proxy.extra);
console.log("proxyTrapRoots:isExtensible", Reflect.isExtensible(proxy));
console.log("proxyTrapRoots:prevent", Reflect.preventExtensions(proxy));
console.log("proxyTrapRoots:isExtensibleAfter", Reflect.isExtensible(proxy));
console.log("proxyTrapRoots:delete", delete proxy.extra, "extra" in proxy);

class Box {
  label: string;
  constructor(label: any) {
    maybeGc();
    this.label = typeof label === "string" ? label : label.marker;
  }
}

const AliasBox: any = Box;
const aliasConstructed = new AliasBox({ marker: "alias", nested: { count: 17 } });
console.log("proxyTrapRoots:aliasConstruct", aliasConstructed.label);

const DefaultConstructProxy: any = new Proxy(Box, {});
const defaultConstructed = new DefaultConstructProxy({ marker: "default", nested: { count: 19 } });
console.log("proxyTrapRoots:defaultConstruct", defaultConstructed.label);

const spreadArgs: any[] = [{ marker: "spread", nested: { count: 21 } }];
console.log("proxyTrapRoots:classSpread", new Box(...spreadArgs).label);
console.log("proxyTrapRoots:aliasSpread", new AliasBox(...spreadArgs).label);
console.log("proxyTrapRoots:proxySpread", new DefaultConstructProxy(...spreadArgs).label);
console.log("proxyTrapRoots:reflectConstructAlias", Reflect.construct(AliasBox, spreadArgs).label);
try {
  new AliasBox(...(undefined as any));
  console.log("proxyTrapRoots:badSpread", "missing");
} catch (error: any) {
  console.log("proxyTrapRoots:badSpread", error.name);
}

const constructHandler: any = {
  prefix: "construct",
  construct(target: any, args: any[]) {
    maybeGc();
    if (args[0] === "boom") {
      throw new Error("construct trap boom");
    }
    const input = args[0];
    console.log("proxyTrapRoots:constructTrap", `${this.prefix}:${input.marker}:${input.nested.count}`);
    return { label: `override:${input.marker}:${input.nested.count}` };
  },
};

const ConstructProxy: any = new Proxy(Box, constructHandler);
const constructed = new ConstructProxy(payload);
console.log("proxyTrapRoots:construct", constructed.label);
try {
  new ConstructProxy("boom");
  console.log("proxyTrapRoots:constructThrow", "missing");
} catch (error: any) {
  console.log("proxyTrapRoots:constructThrow", error.message);
}
const constructedAfterThrow = new ConstructProxy({ marker: "after", nested: { count: 24 } });
console.log("proxyTrapRoots:constructAfterThrow", constructedAfterThrow.label);
