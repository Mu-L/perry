declare function gc(): void;

function forceGC() {
  if (typeof gc === "function") {
    gc();
  }
}

const nested = {
  first: {
    label: "first",
    toJSON() {
      forceGC();
      return { label: this.label, made: { n: 1 } };
    },
  },
  second: { label: "second", made: { n: 2 } },
  third: [{ label: "third-0" }, { label: "third-1" }],
};

console.log(
  JSON.stringify(
    nested,
    (key, value) => {
      if (key === "first" || key === "0") {
        forceGC();
      }
      return value;
    },
    2,
  ),
);

const prettyOnly = {
  a: {
    toJSON() {
      forceGC();
      return { value: "A" };
    },
  },
  b: { value: "B" },
  c: { value: "C" },
};

console.log(JSON.stringify(prettyOnly, null, 2));

const arraySiblings = [
  {
    toJSON() {
      forceGC();
      return { value: 1 };
    },
  },
  { value: 2 },
  { value: 3 },
];

console.log(
  JSON.stringify(
    arraySiblings,
    (key, value) => {
      if (key === "1") {
        forceGC();
      }
      return value;
    },
    2,
  ),
);
