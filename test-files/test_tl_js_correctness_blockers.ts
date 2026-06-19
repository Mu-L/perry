function outcome(label: string, fn: () => unknown) {
  try {
    console.log(label + ": ok " + String(fn()));
  } catch (e) {
    console.log(label + ": throw " + (e as Error).name);
  }
}

function shape(a: unknown[]) {
  return a.length + ":" + JSON.stringify(a) + ":" + Object.keys(a).join("|");
}

outcome("push frozen", () => {
  const a = [1];
  Object.freeze(a);
  return a.push(2);
});

outcome("push sealed", () => {
  const a = [1];
  Object.seal(a);
  return a.push(2);
});

outcome("push nonextensible", () => {
  const a = [1];
  Object.preventExtensions(a);
  return a.push(2);
});

outcome("push readonly length", () => {
  const a = [1];
  Object.defineProperty(a, "length", { writable: false });
  return a.push(2);
});

outcome("pop sealed nonempty", () => {
  const a = [1];
  Object.seal(a);
  return a.pop();
});

outcome("pop nonextensible nonempty", () => {
  const a = [1];
  Object.preventExtensions(a);
  return String(a.pop()) + ":" + shape(a);
});

outcome("pop readonly length", () => {
  const a = [1];
  Object.defineProperty(a, "length", { writable: false });
  return a.pop();
});

outcome("shift sealed nonempty", () => {
  const a = [1];
  Object.seal(a);
  return a.shift();
});

outcome("shift nonextensible nonempty", () => {
  const a = [1];
  Object.preventExtensions(a);
  return String(a.shift()) + ":" + shape(a);
});

outcome("shift readonly length", () => {
  const a = [1];
  Object.defineProperty(a, "length", { writable: false });
  return a.shift();
});

outcome("unshift sealed", () => {
  const a = [1];
  Object.seal(a);
  return a.unshift(0);
});

outcome("unshift readonly length", () => {
  const a = [1];
  Object.defineProperty(a, "length", { writable: false });
  return a.unshift(0);
});

outcome("unshift zero sealed", () => {
  const a: number[] = [];
  Object.seal(a);
  return a.unshift() + ":" + shape(a);
});

outcome("negative bounded index", () => {
  const a = [10, 20, 30];
  for (let i = -1; i < a.length; i++) {
    a[i] = i + 100;
  }
  return String(a[-1]) + ":" + shape(a);
});

outcome("strict proxy assignment false", () => {
  "use strict";
  const p = new Proxy({}, { set() { return false; } });
  (p as { x?: number }).x = 1;
  return "assigned";
});

outcome("strict proxy assignment truthy", () => {
  "use strict";
  const target: { x?: number } = {};
  const p = new Proxy(target, { set(t, k, v) { (t as any)[k] = v; return "yes" as any; } });
  p.x = 2;
  return target.x;
});

outcome("reflect proxy set false", () => {
  const target: { x?: number } = {};
  const p = new Proxy(target, { set() { return 0 as any; } });
  return String(Reflect.set(p, "x", 3)) + ":" + String("x" in target);
});

outcome("reflect proxy set truthy", () => {
  const target: { x?: number } = {};
  const p = new Proxy(target, { set(t, k, v) { (t as any)[k] = v; return "yes" as any; } });
  return String(Reflect.set(p, "x", 4)) + ":" + String(target.x);
});

outcome("proxy array push false set", () => {
  const target: number[] = [];
  const p = new Proxy(target, { set() { return false; } });
  return Array.prototype.push.call(p, 1);
});

outcome("proxy array push truthy set", () => {
  const target: number[] = [];
  const p = new Proxy(target, { set(t, k, v) { (t as any)[k] = v; return "yes" as any; } });
  return String(Array.prototype.push.call(p, 5)) + ":" + String(target.length) + ":" + String(target[0]);
});

outcome("proxy source push spread values", () => {
  const src = new Proxy([2, 3], {});
  const target = [1];
  target.push(...src);
  return target.join(",");
});

outcome("proxy target empty push spread length set", () => {
  const events: string[] = [];
  const target: number[] = [];
  const p = new Proxy(target, {
    set(t, k, v, r) {
      events.push(String(k) + "=" + String(v));
      return Reflect.set(t, k, v, r);
    }
  });
  p.push(...[]);
  return events.join("|") + ":" + String(target.length);
});

outcome("json stringify undefined length", () => {
  const s: string | void = JSON.stringify(undefined);
  return s.length;
});
