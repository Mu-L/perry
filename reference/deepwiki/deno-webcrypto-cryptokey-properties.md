# DeepWiki Response

- Repository: `denoland/deno`
- Question: In Deno WebCrypto, how are CryptoKey objects represented so that algorithm, type, extractable, and usages property reads come from internal key metadata rather than the raw key material? Identify implementation structure, invariants, edge cases, and focused tests relevant to generated or imported asymmetric key material.
- DeepWiki search: https://deepwiki.com/search/in-deno-webcrypto-how-are-cryp_f54789f6-6118-48b3-affc-d27e870f7662

In Deno WebCrypto, `CryptoKey` objects store their metadata (algorithm, type, extractable, and usages) as private symbols directly on the JavaScript object, separate from the raw key material. [[1](#ref-1)] The raw key material is managed in a `WeakMap` called `KEY_STORE` and associated with a handle. [[2](#ref-2)]

## Implementation Structure

The `CryptoKey` class in `ext/crypto/00_crypto.js` defines the structure of a cryptographic key object. [[3](#ref-3)] It uses several private `Symbol` properties to store key metadata:
*   `_type`: Stores the key type (e.g., "secret", "public", "private"). [[4](#ref-4)]
*   `_extractable`: A boolean indicating if the key can be extracted. [[5](#ref-5)]
*   `_algorithm`: An object describing the key's algorithm. [[6](#ref-6)]
*   `_usages`: An array of strings defining the allowed usages for the key. [[7](#ref-7)]
*   `_handle`: A reference to an internal object that points to the actual key material stored in `KEY_STORE`. [[8](#ref-8)]

The `CryptoKey` class provides getter methods for `type`, `extractable`, `usages`, and `algorithm` that directly return the values from these private symbols. [[9](#ref-9)] This ensures that these properties are read from the metadata and not the raw key material. [[9](#ref-9)]

The `constructKey` function is responsible for creating new `CryptoKey` instances. [[10](#ref-10)] It initializes these private symbol properties with the provided metadata and associates a `handle` with the key material in `KEY_STORE`. [[10](#ref-10)] The `KEY_STORE` is a `WeakMap` that maps the `handle` object to the actual key data, which is typically a `Uint8Array` or an object containing key components. [[2](#ref-2)]

## Invariants

*   **Separation of Metadata and Material**: The `CryptoKey` object itself holds only metadata, while the sensitive raw key material is stored separately in `KEY_STORE` and accessed via a `handle`. [[1](#ref-1)] [[2](#ref-2)]
*   **Immutability of Metadata**: Once a `CryptoKey` is constructed, its `type`, `extractable`, `algorithm`, and `usages` properties are read-only and cannot be directly modified. [[9](#ref-9)]
*   **Usage Intersection**: When generating or importing keys, the actual `usages` assigned to the `CryptoKey` are an intersection of the requested usages and the recognized usages for the specific algorithm and key type. [[11](#ref-11)] This is handled by the `usageIntersection` function. [[11](#ref-11)]

## Edge Cases

*   **Non-extractable Keys**: If a key is generated or imported with `extractable` set to `false`, attempts to export it will result in a `DOMException`. [[12](#ref-12)] The `extractable` property on the `CryptoKey` object accurately reflects this state. [[13](#ref-13)]
*   **Invalid Key Usages**: Generating or importing a key with invalid or unsupported usages for a given algorithm will throw a `DOMException` with a "SyntaxError" or "DataError". [[14](#ref-14)] [[15](#ref-15)]
*   **JWK `ext` property**: When importing a key in JWK format, if the `ext` property is `false` but `extractable` is `true`, a `DOMException` is thrown. [[16](#ref-16)]

## Focused Tests

The `tests/unit/webcrypto_test.ts` file contains several tests that verify the correct behavior of `CryptoKey` properties for generated and imported asymmetric key material. [[17](#ref-17)]

*   **`testAesKeyGen`**: This test generates an AES key and asserts that its `type`, `extractable`, `usages`, and `algorithm` properties are correctly set. [[18](#ref-18)]
*   **`testUnwrapKey`**: This test involves wrapping and unwrapping an AES key using an RSA key pair. [[19](#ref-19)] It verifies that the unwrapped `CryptoKey` has the expected `type`, `extractable`, and `usages` properties. [[20](#ref-20)]
*   **`testImportExportEcDsaJwk`**: This test imports and exports ECDSA keys in JWK format, asserting that the `CryptoKey` properties match the expected values after import. [[21](#ref-21)]
*   **`rsaExport`**: This test generates RSA key pairs and exports them in PKCS8 and SPKI formats, implicitly verifying that the `extractable` property is correctly handled. [[22](#ref-22)]
*   **`structuredClone CryptoKey`**: This test verifies that `CryptoKey` objects can be structured cloned and that the cloned object retains the correct `type`, `extractable`, `algorithm`, and `usages` properties. [[23](#ref-23)] It also includes tests for non-extractable keys and various asymmetric key types like EC and Ed25519. [[24](#ref-24)]

## Notes

The `CryptoKey` interface is also defined in TypeScript declaration files like `cli/tsc/dts/lib.webworker.d.ts` and `cli/tsc/dts/lib.deno_crypto.d.ts`, which provide type definitions for these properties. [[25](#ref-25)] [[26](#ref-26)] These definitions align with the Web Crypto API specification. [[25](#ref-25)]

Wiki pages you might want to explore:
- [TypeScript Integration and Type Checking (denoland/deno)](/wiki/denoland/deno#4.2)
- [Web Standards APIs (denoland/deno)](/wiki/denoland/deno#6)

## References

<a id="ref-1"></a>
[1] [`Repo denoland/deno: ext/crypto/00_crypto.js`](https://github.com/denoland/deno/blob/d6212d40/ext/crypto/00_crypto.js#L353-L357) lines 353-357

```js
const _handle = Symbol("[[handle]]");
const _algorithm = Symbol("[[algorithm]]");
const _extractable = Symbol("[[extractable]]");
const _usages = Symbol("[[usages]]");
const _type = Symbol("[[type]]");
```

<a id="ref-2"></a>
[2] [`Repo denoland/deno: ext/crypto/00_crypto.js`](https://github.com/denoland/deno/blob/d6212d40/ext/crypto/00_crypto.js#L482-L483) lines 482-483

```js
/** @type {WeakMap<object, object>} */
const KEY_STORE = new SafeWeakMap();
```

<a id="ref-3"></a>
[3] [`Repo denoland/deno: ext/crypto/00_crypto.js`](https://github.com/denoland/deno/blob/d6212d40/ext/crypto/00_crypto.js#L359-L372) lines 359-372

```js
class CryptoKey {
  /** @type {string} */
  [_type];
  /** @type {boolean} */
  [_extractable];
  /** @type {object} */
  [_algorithm];
  /** @type {string[]} */
  [_usages];
  /** @type {object} */
  [_handle];
  /** @type {object} */
  [kKeyObject];

```

<a id="ref-4"></a>
[4] [`Repo denoland/deno: ext/crypto/00_crypto.js`](https://github.com/denoland/deno/blob/d6212d40/ext/crypto/00_crypto.js#L357-L357) lines 357-357

```js
const _type = Symbol("[[type]]");
```

<a id="ref-5"></a>
[5] [`Repo denoland/deno: ext/crypto/00_crypto.js`](https://github.com/denoland/deno/blob/d6212d40/ext/crypto/00_crypto.js#L355-L355) lines 355-355

```js
const _extractable = Symbol("[[extractable]]");
```

<a id="ref-6"></a>
[6] [`Repo denoland/deno: ext/crypto/00_crypto.js`](https://github.com/denoland/deno/blob/d6212d40/ext/crypto/00_crypto.js#L354-L354) lines 354-354

```js
const _algorithm = Symbol("[[algorithm]]");
```

<a id="ref-7"></a>
[7] [`Repo denoland/deno: ext/crypto/00_crypto.js`](https://github.com/denoland/deno/blob/d6212d40/ext/crypto/00_crypto.js#L356-L356) lines 356-356

```js
const _usages = Symbol("[[usages]]");
```

<a id="ref-8"></a>
[8] [`Repo denoland/deno: ext/crypto/00_crypto.js`](https://github.com/denoland/deno/blob/d6212d40/ext/crypto/00_crypto.js#L353-L353) lines 353-353

```js
const _handle = Symbol("[[handle]]");
```

<a id="ref-9"></a>
[9] [`Repo denoland/deno: ext/crypto/00_crypto.js`](https://github.com/denoland/deno/blob/d6212d40/ext/crypto/00_crypto.js#L377-L401) lines 377-401

```js
  /** @returns {string} */
  get type() {
    webidl.assertBranded(this, CryptoKeyPrototype);
    return this[_type];
  }

  /** @returns {boolean} */
  get extractable() {
    webidl.assertBranded(this, CryptoKeyPrototype);
    return this[_extractable];
  }

  /** @returns {string[]} */
  get usages() {
    webidl.assertBranded(this, CryptoKeyPrototype);
    // TODO(lucacasonato): return a SameObject copy
    return this[_usages];
  }

  /** @returns {object} */
  get algorithm() {
    webidl.assertBranded(this, CryptoKeyPrototype);
    // TODO(lucacasonato): return a SameObject copy
    return this[_algorithm];
  }
```

<a id="ref-10"></a>
[10] [`Repo denoland/deno: ext/crypto/00_crypto.js`](https://github.com/denoland/deno/blob/d6212d40/ext/crypto/00_crypto.js#L431-L437) lines 431-437

```js
function constructKey(type, extractable, usages, algorithm, handle) {
  const key = webidl.createBranded(CryptoKey);
  key[_type] = type;
  key[_extractable] = extractable;
  key[_usages] = usages;
  key[_algorithm] = algorithm;
  key[_handle] = handle;
```

<a id="ref-11"></a>
[11] [`Repo denoland/deno: ext/crypto/00_crypto.js`](https://github.com/denoland/deno/blob/d6212d40/ext/crypto/00_crypto.js#L474-L478) lines 474-478

```js
function usageIntersection(a, b) {
  return ArrayPrototypeFilter(
    a,
    (i) => ArrayPrototypeIncludes(b, i),
  );
```

<a id="ref-12"></a>
[12] [`Repo denoland/deno: tests/unit/webcrypto_test.ts`](https://github.com/denoland/deno/blob/d6212d40/tests/unit/webcrypto_test.ts#L1964-L1981) lines 1964-1981

```ts
Deno.test(async function exportKeyNotExtractable() {
  const key = await crypto.subtle.generateKey(
    {
      name: "HMAC",
      hash: "SHA-512",
    },
    false,
    ["sign", "verify"],
  );

  assert(key);
  assertEquals(key.extractable, false);

  await assertRejects(async () => {
    // Should fail
    await crypto.subtle.exportKey("raw", key);
  }, DOMException);
});
```

<a id="ref-13"></a>
[13] [`Repo denoland/deno: tests/unit/webcrypto_test.ts`](https://github.com/denoland/deno/blob/d6212d40/tests/unit/webcrypto_test.ts#L1975-L1975) lines 1975-1975

```ts
  assertEquals(key.extractable, false);
```

<a id="ref-14"></a>
[14] [`Repo denoland/deno: ext/crypto/00_crypto.js`](https://github.com/denoland/deno/blob/d6212d40/ext/crypto/00_crypto.js#L1741-L1749) lines 1741-1749

```js
      // 1.
      if (
        ArrayPrototypeFind(
          usages,
          (u) => !ArrayPrototypeIncludes(["sign", "verify"], u),
        ) !== undefined
      ) {
        throw new DOMException("Invalid key usage", "SyntaxError");
      }
```

<a id="ref-15"></a>
[15] [`Repo denoland/deno: ext/crypto/00_crypto.js`](https://github.com/denoland/deno/blob/d6212d40/ext/crypto/00_crypto.js#L3464-L3474) lines 3464-3474

```js
          ArrayPrototypeFind(
            keyUsages,
            (u) =>
              !ArrayPrototypeIncludes(
                SUPPORTED_KEY_USAGES[normalizedAlgorithm.name].public,
                u,
              ),
          ) !== undefined
        ) {
          throw new DOMException("Invalid key usage", "SyntaxError");
        }
```

<a id="ref-16"></a>
[16] [`Repo denoland/deno: ext/crypto/00_crypto.js`](https://github.com/denoland/deno/blob/d6212d40/ext/crypto/00_crypto.js#L3989-L3994) lines 3989-3994

```js
      if (jwk.ext === false && extractable === true) {
        throw new DOMException(
          "'ext' property of JsonWebKey must not be false if extractable is true",
          "DataError",
        );
      }
```

<a id="ref-17"></a>
[17] [`Repo denoland/deno: tests/unit/webcrypto_test.ts`](https://github.com/denoland/deno/blob/d6212d40/tests/unit/webcrypto_test.ts#L1-L2000) lines 1-2000

```ts
// Copyright 2018-2026 the Deno authors. MIT license.

import {
  assert,
  assertEquals,
  assertNotEquals,
  assertRejects,
  assertThrows,
} from "./test_util.ts";

// https://github.com/denoland/deno/issues/11664
Deno.test(async function testImportArrayBufferKey() {
  const subtle = globalThis.crypto.subtle;
  assert(subtle);

  // deno-fmt-ignore
  const key = new Uint8Array([1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16]);

  const cryptoKey = await subtle.importKey(
    "raw",
    key.buffer,
    { name: "HMAC", hash: "SHA-1" },
    true,
    ["sign"],
  );
  assert(cryptoKey);

  // Test key usage
  await subtle.sign({ name: "HMAC" }, cryptoKey, new Uint8Array(8));
});

Deno.test(async function testSignVerify() {
  const subtle = globalThis.crypto.subtle;
  assert(subtle);
  for (const algorithm of ["RSA-PSS", "RSASSA-PKCS1-v1_5"]) {
    for (
      const hash of [
        "SHA-1",
        "SHA-256",
        "SHA-384",
        "SHA-512",
      ]
    ) {
      const keyPair = await subtle.generateKey(
        {
          name: algorithm,
          modulusLength: 2048,
          publicExponent: new Uint8Array([1, 0, 1]),
          hash,
        },
        true,
        ["sign", "verify"],
      );

      const data = new Uint8Array([1, 2, 3]);

      const signAlgorithm = { name: algorithm, saltLength: 32 };

      const signature = await subtle.sign(
        signAlgorithm,
        keyPair.privateKey,
        data,
      );

      assert(signature);
      assert(signature.byteLength > 0);
      assert(signature.byteLength % 8 == 0);
      assert(signature instanceof ArrayBuffer);

      const verified = await subtle.verify(
        signAlgorithm,
        keyPair.publicKey,
        signature,
        data,
      );
      assert(verified);
    }
  }
});

// deno-fmt-ignore
const plainText = new Uint8Array([95, 77, 186, 79, 50, 12, 12, 232, 118, 114, 90, 252, 229, 251, 210, 91, 248, 62, 90, 113, 37, 160, 140, 175, 231, 60, 62, 186, 196, 33, 119, 157, 249, 213, 93, 24, 12, 58, 233, 148, 38, 69, 225, 216, 47, 238, 140, 157, 41, 75, 60, 177, 160, 138, 153, 49, 32, 27, 60, 14, 129, 252, 71, 202, 207, 131, 21, 162, 175, 102, 50, 65, 19, 195, 182, 98, 48, 195, 70, 8, 196, 244, 89, 54, 52, 206, 2, 178, 103, 54, 34, 119, 240, 168, 64, 202, 116, 188, 61, 26, 98, 54, 149, 44, 94, 215, 170, 248, 168, 254, 203, 221, 250, 117, 132, 230, 151, 140, 234, 93, 42, 91, 159, 183, 241, 180, 140, 139, 11, 229, 138, 48, 82, 2, 117, 77, 131, 118, 16, 115, 116, 121, 60, 240, 38, 170, 238, 83, 0, 114, 125, 131, 108, 215, 30, 113, 179, 69, 221, 178, 228, 68, 70, 255, 197, 185, 1, 99, 8 ... (truncated)

// Passing
const hashPlainTextVector = [
  {
    hash: "SHA-1",
    plainText: plainText.slice(0, 214),
  },
  {
    hash: "SHA-256",
    plainText: plainText.slice(0, 190),
  },
  {
    hash: "SHA-384",
    plainText: plainText.slice(0, 158),
  },
  {
    hash: "SHA-512",
    plainText: plainText.slice(0, 126),
  },
];

Deno.test(async function testEncryptDecrypt() {
  const subtle = globalThis.crypto.subtle;
  assert(subtle);
  for (
    const { hash, plainText } of hashPlainTextVector
  ) {
    const keyPair = await subtle.generateKey(
      {
        name: "RSA-OAEP",
        modulusLength: 2048,
        publicExponent: new Uint8Array([1, 0, 1]),
        hash,
      },
      true,
      ["encrypt", "decrypt"],
    );

    const encryptAlgorithm = { name: "RSA-OAEP" };
    const cipherText = await subtle.encrypt(
      encryptAlgorithm,
      keyPair.publicKey,
      plainText,
    );

    assert(cipherText);
    assert(cipherText.byteLength > 0);
    assertEquals(cipherText.byteLength * 8, 2048);
    assert(cipherText instanceof ArrayBuffer);

    const decrypted = await subtle.decrypt(
      encryptAlgorithm,
      keyPair.privateKey,
      cipherText,
    );
    assert(decrypted);
    assert(decrypted instanceof ArrayBuffer);
    assertEquals(new Uint8Array(decrypted), plainText);

    const badPlainText = new Uint8Array(plainText.byteLength + 1);
    badPlainText.set(plainText, 0);
    badPlainText.set(new Uint8Array([32]), plainText.byteLength);
    await assertRejects(async () => {
      // Should fail
      await subtle.encrypt(
        encryptAlgorithm,
        keyPair.publicKey,
        badPlainText,
      );
      throw new TypeError("unreachable");
    }, DOMException);
  }
});

Deno.test(async function testGenerateRSAKey() {
  const subtle = globalThis.crypto.subtle;
  assert(subtle);

  const keyPair = await subtle.generateKey(
    {
      name: "RSA-PSS",
      modulusLength: 2048,
      publicExponent: new Uint8Array([1, 0, 1]),
      hash: "SHA-256",
    },
    true,
    ["sign", "verify"],
  );

  assert(keyPair.privateKey);
  assert(keyPair.publicKey);
  assertEquals(keyPair.privateKey.extractable, true);
  assert(keyPair.privateKey.usages.includes("sign"));
});

Deno.test(async function testGenerateHMACKey() {
  const key = await globalThis.crypto.subtle.generateKey(
    {
      name: "HMAC",
      hash: "SHA-512",
    },
    true,
    ["sign", "verify"],
  );

  assert(key);
  assertEquals(key.extractable, true);
  assert(key.usages.includes("sign"));
});

Deno.test(async function testECDSASignVerify() {
  const key = await globalThis.crypto.subtle.generateKey(
    {
      name: "ECDSA",
      namedCurve: "P-384",
    },
    true,
    ["sign", "verify"],
  );

  const encoder = new TextEncoder();
  const encoded = encoder.encode("Hello, World!");
  const signature = await globalThis.crypto.subtle.sign(
    { name: "ECDSA", hash: "SHA-384" },
    key.privateKey,
    encoded,
  );

  assert(signature);
  assert(signature instanceof ArrayBuffer);

  const verified = await globalThis.crypto.subtle.verify(
    { hash: { name: "SHA-384" }, name: "ECDSA" },
    key.publicKey,
    signature,
    encoded,
  );
  assert(verified);
});

// Tests the "bad paths" as a temporary replacement for sign_verify/ecdsa WPT.
Deno.test(async function testECDSASignVerifyFail() {
  const key = await globalThis.crypto.subtle.generateKey(
    {
      name: "ECDSA",
      namedCurve: "P-384",
    },
    true,
    ["sign", "verify"],
  );

  const encoded = new Uint8Array([1]);
  // Signing with a public key (InvalidAccessError)
  await assertRejects(async () => {
    await globalThis.crypto.subtle.sign(
      { name: "ECDSA", hash: "SHA-384" },
      key.publicKey,
      new Uint8Array([1]),
    );
    throw new TypeError("unreachable");
  }, DOMException);

  // Do a valid sign for later verifying.
  const signature = await globalThis.crypto.subtle.sign(
    { name: "ECDSA", hash: "SHA-384" },
    key.privateKey,
    encoded,
  );

  // Verifying with a private key (InvalidAccessError)
  await assertRejects(async () => {
    await globalThis.crypto.subtle.verify(
      { hash: { name: "SHA-384" }, name: "ECDSA" },
      key.privateKey,
      signature,
      encoded,
    );
    throw new TypeError("unreachable");
  }, DOMException);
});

// https://github.com/denoland/deno/issues/11313
Deno.test(async function testSignRSASSAKey() {
  const subtle = globalThis.crypto.subtle;
  assert(subtle);

  const keyPair = await subtle.generateKey(
    {
      name: "RSASSA-PKCS1-v1_5",
      modulusLength: 2048,
      publicExponent: new Uint8Array([1, 0, 1]),
      hash: "SHA-256",
    },
    true,
    ["sign", "verify"],
  );

  assert(keyPair.privateKey);
  assert(keyPair.publicKey);
  assertEquals(keyPair.privateKey.extractable, true);
  assert(keyPair.privateKey.usages.includes("sign"));

  const encoder = new TextEncoder();
  const encoded = encoder.encode("Hello, World!");

  const signature = await globalThis.crypto.subtle.sign(
    { name: "RSASSA-PKCS1-v1_5" },
    keyPair.privateKey,
    encoded,
  );

  assert(signature);
});

// deno-fmt-ignore
const rawKey = new Uint8Array([
  1, 2, 3, 4, 5, 6, 7, 8,
  9, 10, 11, 12, 13, 14, 15, 16
]);

const jwk: JsonWebKey = {
  kty: "oct",
  // unpadded base64 for rawKey.
  k: "AQIDBAUGBwgJCgsMDQ4PEA",
  alg: "HS256",
  ext: true,
  "key_ops": ["sign"],
};

Deno.test(async function subtleCryptoHmacImportExport() {
  const key1 = await crypto.subtle.importKey(
    "raw",
    rawKey,
    { name: "HMAC", hash: "SHA-256" },
    true,
    ["sign"],
  );
  const key2 = await crypto.subtle.importKey(
    "jwk",
    jwk,
    { name: "HMAC", hash: "SHA-256" },
    true,
    ["sign"],
  );
  const actual1 = await crypto.subtle.sign(
    { name: "HMAC" },
    key1,
    new Uint8Array([1, 2, 3, 4]),
  );

  const actual2 = await crypto.subtle.sign(
    { name: "HMAC" },
    key2,
    new Uint8Array([1, 2, 3, 4]),
  );
  // deno-fmt-ignore
  const expected = new Uint8Array([
    59, 170, 255, 216, 51, 141, 51, 194,
    213, 48, 41, 191, 184, 40, 216, 47,
    130, 165, 203, 26, 163, 43, 38, 71,
    23, 122, 222, 1, 146, 46, 182, 87,
  ]);
  assertEquals(
    new Uint8Array(actual1),
    expected,
  );
  assertEquals(
    new Uint8Array(actual2),
    expected,
  );

  const exportedKey1 = await crypto.subtle.exportKey("raw", key1);
  assertEquals(new Uint8Array(exportedKey1), rawKey);

  const exportedKey2 = await crypto.subtle.exportKey("jwk", key2);
  assertEquals(exportedKey2, jwk);
});

// https://github.com/denoland/deno/issues/12085
Deno.test(async function generateImportHmacJwk() {
  const key = await crypto.subtle.generateKey(
    {
      name: "HMAC",
      hash: "SHA-512",
    },
    true,
    ["sign"],
  );
  assert(key);
  assertEquals(key.type, "secret");
  assertEquals(key.extractable, true);
  assertEquals(key.usages, ["sign"]);

  const exportedKey = await crypto.subtle.exportKey("jwk", key);
  assertEquals(exportedKey.kty, "oct");
  assertEquals(exportedKey.alg, "HS512");
  assertEquals(exportedKey.key_ops, ["sign"]);
  assertEquals(exportedKey.ext, true);
  assert(typeof exportedKey.k == "string");
  assertEquals(exportedKey.k.length, 171);
});

// 2048-bits publicExponent=65537
const pkcs8TestVectors = [
  // rsaEncryption
  { pem: "tests/testdata/webcrypto/id_rsaEncryption.pem", hash: "SHA-256" },
];

Deno.test({ permissions: { read: true } }, async function importRsaPkcs8() {
  const pemHeader = "-----BEGIN PRIVATE KEY-----";
  const pemFooter = "-----END PRIVATE KEY-----";
  for (const { pem, hash } of pkcs8TestVectors) {
    const keyFile = await Deno.readTextFile(pem);
    const pemContents = keyFile.substring(
      pemHeader.length,
      keyFile.length - pemFooter.length,
    );
    const binaryDerString = atob(pemContents);
    const binaryDer = new Uint8Array(binaryDerString.length);
    for (let i = 0; i < binaryDerString.length; i++) {
      binaryDer[i] = binaryDerString.charCodeAt(i);
    }

    const key = await crypto.subtle.importKey(
      "pkcs8",
      binaryDer,
      { name: "RSA-PSS", hash },
      true,
      ["sign"],
    );

    assert(key);
    assertEquals(key.type, "private");
    assertEquals(key.extractable, true);
    assertEquals(key.usages, ["sign"]);
    const algorithm = key.algorithm as RsaHashedKeyAlgorithm;
    assertEquals(algorithm.name, "RSA-PSS");
    assertEquals(algorithm.hash.name, hash);
    assertEquals(algorithm.modulusLength, 2048);
    assertEquals(algorithm.publicExponent, new Uint8Array([1, 0, 1]));
  }
});

const nonInteroperableVectors = [
  // id-RSASSA-PSS (sha256)
  // `openssl genpkey -algorithm rsa-pss -pkeyopt rsa_pss_keygen_md:sha256 -out id_rsassaPss.pem`
  { pem: "tests/testdata/webcrypto/id_rsassaPss.pem", hash: "SHA-256" },
  // id-RSASSA-PSS (default parameters)
  // `openssl genpkey -algorithm rsa-pss -out id_rsassaPss.pem`
  {
    pem: "tests/testdata/webcrypto/id_rsassaPss_default.pem",
    hash: "SHA-1",
  },
  // id-RSASSA-PSS (default hash)
  // `openssl genpkey -algorithm rsa-pss -pkeyopt rsa_pss_keygen_saltlen:30 -out rsaPss_saltLen_30.pem`
  {
    pem: "tests/testdata/webcrypto/id_rsassaPss_saltLen_30.pem",
    hash: "SHA-1",
  },
];

Deno.test(
  { permissions: { read: true } },
  async function importNonInteroperableRsaPkcs8() {
    const pemHeader = "-----BEGIN PRIVATE KEY-----";
    const pemFooter = "-----END PRIVATE KEY-----";
    for (const { pem, hash } of nonInteroperableVectors) {
      const keyFile = await Deno.readTextFile(pem);
      const pemContents = keyFile.substring(
        pemHeader.length,
        keyFile.length - pemFooter.length,
      );
      const binaryDerString = atob(pemContents);
      const binaryDer = new Uint8Array(binaryDerString.length);
      for (let i = 0; i < binaryDerString.length; i++) {
        binaryDer[i] = binaryDerString.charCodeAt(i);
      }

      await assertRejects(
        () =>
          crypto.subtle.importKey(
            "pkcs8",
            binaryDer,
            { name: "RSA-PSS", hash },
            true,
            ["sign"],
          ),
        DOMException,
        "unsupported algorithm",
      );
    }
  },
);

// deno-fmt-ignore
const asn1AlgorithmIdentifier = new Uint8Array([
  0x02, 0x01, 0x00, // INTEGER
  0x30, 0x0d, // SEQUENCE (2 elements)
  0x06, 0x09, // OBJECT IDENTIFIER
  0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x01, 0x01, // 1.2.840.113549.1.1.1 (rsaEncryption)
  0x05, 0x00, // NULL
]);

Deno.test(async function rsaExport() {
  for (const algorithm of ["RSASSA-PKCS1-v1_5", "RSA-PSS", "RSA-OAEP"]) {
    const keyPair = await crypto.subtle.generateKey(
      {
        name: algorithm,
        modulusLength: 2048,
        publicExponent: new Uint8Array([1, 0, 1]),
        hash: "SHA-256",
      },
      true,
      algorithm !== "RSA-OAEP" ? ["sign", "verify"] : ["encrypt", "decrypt"],
    );

    assert(keyPair.privateKey);
    assert(keyPair.publicKey);
    assertEquals(keyPair.privateKey.extractable, true);

    const exportedPrivateKey = await crypto.subtle.exportKey(
      "pkcs8",
      keyPair.privateKey,
    );

    assert(exportedPrivateKey);
    assert(exportedPrivateKey instanceof ArrayBuffer);

    const pkcs8 = new Uint8Array(exportedPrivateKey);
    assert(pkcs8.length > 0);

    assertEquals(
      pkcs8.slice(4, asn1AlgorithmIdentifier.byteLength + 4),
      asn1AlgorithmIdentifier,
    );

    const exportedPublicKey = await crypto.subtle.exportKey(
      "spki",
      keyPair.publicKey,
    );

    const spki = new Uint8Array(exportedPublicKey);
    assert(spki.length > 0);

    assertEquals(
      spki.slice(4, asn1AlgorithmIdentifier.byteLength + 1),
      asn1AlgorithmIdentifier.slice(3),
    );
  }
});

Deno.test(async function testHkdfDeriveBits() {
  const rawKey = crypto.getRandomValues(new Uint8Array(16));
  const key = await crypto.subtle.importKey(
    "raw",
    rawKey,
    { name: "HKDF", hash: "SHA-256" },
    false,
    ["deriveBits"],
  );
  const salt = crypto.getRandomValues(new Uint8Array(16));
  const info = crypto.getRandomValues(new Uint8Array(16));
  const result = await crypto.subtle.deriveBits(
    {
      name: "HKDF",
      hash: "SHA-256",
      salt: salt,
      info: info,
    },
    key,
    128,
  );
  assertEquals(result.byteLength, 128 / 8);
});

Deno.test(async function testHkdfDeriveBitsWithLargeKeySize() {
  const key = await crypto.subtle.importKey(
    "raw",
    new Uint8Array([0x00]),
    "HKDF",
    false,
    ["deriveBits"],
  );
  await assertRejects(
    () =>
      crypto.subtle.deriveBits(
        {
          name: "HKDF",
          hash: "SHA-1",
          salt: new Uint8Array(),
          info: new Uint8Array(),
        },
        key,
        ((20 * 255) << 3) + 8,
      ),
    DOMException,
    "The length provided for HKDF is too large",
  );
});

Deno.test(async function testEcdhDeriveBitsWithShorterLength() {
  const keypair = await crypto.subtle.generateKey(
    {
      name: "ECDH",
      namedCurve: "P-384",
    },
    true,
    ["deriveBits", "deriveKey"],
  );
  const result = await crypto.subtle.deriveBits(
    {
      name: "ECDH",
      public: keypair.publicKey,
    },
    keypair.privateKey,
    256,
  );
  assertEquals(result.byteLength * 8, 256);
});

Deno.test(async function testEcdhDeriveBitsWithLongerLength() {
  const keypair = await crypto.subtle.generateKey(
    {
      name: "ECDH",
      namedCurve: "P-384",
    },
    true,
    ["deriveBits", "deriveKey"],
  );
  await assertRejects(
    () =>
      crypto.subtle.deriveBits(
        {
          name: "ECDH",
          public: keypair.publicKey,
        },
        keypair.privateKey,
        512,
      ),
    DOMException,
    "Invalid length",
  );
});

Deno.test(async function testEcdhDeriveBitsWithNullLength() {
  const keypair = await crypto.subtle.generateKey(
    {
      name: "ECDH",
      namedCurve: "P-384",
    },
    true,
    ["deriveBits", "deriveKey"],
  );
  const result = await crypto.subtle.deriveBits(
    {
      name: "ECDH",
      public: keypair.publicKey,
    },
    keypair.privateKey,
    // @ts-ignore: necessary until .d.ts file allows passing null (see https://github.com/microsoft/TypeScript-DOM-lib-generator/pull/1416)
    null,
  );
  assertEquals(result.byteLength * 8, 384);
});

Deno.test(async function testDeriveKey() {
  // Test deriveKey
  const rawKey = crypto.getRandomValues(new Uint8Array(16));
  const key = await crypto.subtle.importKey(
    "raw",
    rawKey,
    "PBKDF2",
    false,
    ["deriveKey", "deriveBits"],
  );

  const salt = crypto.getRandomValues(new Uint8Array(16));
  const derivedKey = await crypto.subtle.deriveKey(
    {
      name: "PBKDF2",
      salt,
      iterations: 1000,
      hash: "SHA-256",
    },
    key,
    { name: "HMAC", hash: "SHA-256" },
    true,
    ["sign"],
  );

  assert(derivedKey instanceof CryptoKey);
  assertEquals(derivedKey.type, "secret");
  assertEquals(derivedKey.extractable, true);
  assertEquals(derivedKey.usages, ["sign"]);

  const algorithm = derivedKey.algorithm as HmacKeyAlgorithm;
  assertEquals(algorithm.name, "HMAC");
  assertEquals(algorithm.hash.name, "SHA-256");
  assertEquals(algorithm.length, 512);
});

Deno.test(async function testAesCbcEncryptDecrypt() {
  const key = await crypto.subtle.generateKey(
    { name: "AES-CBC", length: 128 },
    true,
    ["encrypt", "decrypt"],
  );

  const iv = crypto.getRandomValues(new Uint8Array(16));
  const encrypted = await crypto.subtle.encrypt(
    {
      name: "AES-CBC",
      iv,
    },
    key as CryptoKey,
    new Uint8Array([1, 2, 3, 4, 5, 6]),
  );

  assert(encrypted instanceof ArrayBuffer);
  assertEquals(encrypted.byteLength, 16);

  const decrypted = await crypto.subtle.decrypt(
    {
      name: "AES-CBC",
      iv,
    },
    key as CryptoKey,
    encrypted,
  );

  assert(decrypted instanceof ArrayBuffer);
  assertEquals(decrypted.byteLength, 6);
  assertEquals(new Uint8Array(decrypted), new Uint8Array([1, 2, 3, 4, 5, 6]));
});

Deno.test(async function testAesCtrEncryptDecrypt() {
  async function aesCtrRoundTrip(
    key: CryptoKey,
    counter: Uint8Array<ArrayBuffer>,
    length: number,
    plainText: Uint8Array<ArrayBuffer>,
  ) {
    const cipherText = await crypto.subtle.encrypt(
      {
        name: "AES-CTR",
        counter,
        length,
      },
      key,
      plainText,
    );

    assert(cipherText instanceof ArrayBuffer);
    assertEquals(cipherText.byteLength, plainText.byteLength);
    assertNotEquals(new Uint8Array(cipherText), plainText);

    const decryptedText = await crypto.subtle.decrypt(
      {
        name: "AES-CTR",
        counter,
        length,
      },
      key,
      cipherText,
    );

    assert(decryptedText instanceof ArrayBuffer);
    assertEquals(decryptedText.byteLength, plainText.byteLength);
    assertEquals(new Uint8Array(decryptedText), plainText);
  }
  for (const keySize of [128, 192, 256]) {
    const key = await crypto.subtle.generateKey(
      { name: "AES-CTR", length: keySize },
      true,
      ["encrypt", "decrypt"],
    ) as CryptoKey;

    // test normal operation
    for (const length of [128 /*, 64, 128 */]) {
      const counter = crypto.getRandomValues(new Uint8Array(16));

      await aesCtrRoundTrip(
        key,
        counter,
        length,
        new Uint8Array([1, 2, 3, 4, 5, 6]),
      );
    }

    // test counter-wrapping
    for (const length of [32, 64, 128]) {
      const plaintext1 = crypto.getRandomValues(new Uint8Array(32));
      const counter = new Uint8Array(16);

      // fixed upper part
      for (let off = 0; off < 16 - (length / 8); ++off) {
        counter[off] = off;
      }
      const ciphertext1 = await crypto.subtle.encrypt(
        {
          name: "AES-CTR",
          counter,
          length,
        },
        key,
        plaintext1,
      );

      // Set lower [length] counter bits to all '1's
      for (let off = 16 - (length / 8); off < 16; ++off) {
        counter[off] = 0xff;
      }

      // = [ 1 block of 0x00 + plaintext1 ]
      const plaintext2 = new Uint8Array(48);
      plaintext2.set(plaintext1, 16);

      const ciphertext2 = await crypto.subtle.encrypt(
        {
          name: "AES-CTR",
          counter,
          length,
        },
        key,
        plaintext2,
      );

      // If counter wrapped, 2nd block of ciphertext2 should be equal to 1st block of ciphertext1
      // since ciphertext1 used counter = 0x00...00
      // and ciphertext2 used counter = 0xFF..FF which should wrap to 0x00..00 without affecting
      // higher bits
      assertEquals(
        new Uint8Array(ciphertext1),
        new Uint8Array(ciphertext2).slice(16),
      );
    }
  }
});

Deno.test(async function testECDH() {
  for (const keySize of [256, 384]) {
    const keyPair = await crypto.subtle.generateKey(
      {
        name: "ECDH",
        namedCurve: "P-" + keySize,
      },
      true,
      ["deriveBits"],
    );

    const derivedKey = await crypto.subtle.deriveBits(
      {
        name: "ECDH",
        public: keyPair.publicKey,
      },
      keyPair.privateKey,
      keySize,
    );

    assert(derivedKey instanceof ArrayBuffer);
    assertEquals(derivedKey.byteLength, keySize / 8);
  }
});

Deno.test(async function testWrapKey() {
  // Test wrapKey
  const key = await crypto.subtle.generateKey(
    {
      name: "RSA-OAEP",
      modulusLength: 4096,
      publicExponent: new Uint8Array([1, 0, 1]),
      hash: "SHA-256",
    },
    true,
    ["wrapKey", "unwrapKey"],
  );

  const hmacKey = await crypto.subtle.generateKey(
    {
      name: "HMAC",
      hash: "SHA-256",
      length: 128,
    },
    true,
    ["sign"],
  );

  const wrappedKey = await crypto.subtle.wrapKey(
    "raw",
    hmacKey,
    key.publicKey,
    {
      name: "RSA-OAEP",
      label: new Uint8Array(8),
    },
  );

  assert(wrappedKey instanceof ArrayBuffer);
  assertEquals(wrappedKey.byteLength, 512);
});

// Doesn't need to cover all cases.
// Only for testing types.
Deno.test(async function testAesKeyGen() {
  const key = await crypto.subtle.generateKey(
    {
      name: "AES-GCM",
      length: 256,
    },
    true,
    ["encrypt", "decrypt"],
  );

  assert(key);
  assertEquals(key.type, "secret");
  assertEquals(key.extractable, true);
  assertEquals(key.usages, ["encrypt", "decrypt"]);
  const algorithm = key.algorithm as AesKeyAlgorithm;
  assertEquals(algorithm.name, "AES-GCM");
  assertEquals(algorithm.length, 256);
});

Deno.test(async function testUnwrapKey() {
  const subtle = crypto.subtle;

  const AES_KEY: AesKeyAlgorithm & AesCbcParams = {
    name: "AES-CBC",
    length: 128,
    iv: new Uint8Array(16),
  };

  const RSA_KEY: RsaHashedKeyGenParams & RsaOaepParams = {
    name: "RSA-OAEP",
    modulusLength: 2048,
    publicExponent: new Uint8Array([1, 0, 1]),
    hash: "SHA-1",
  };

  const aesKey = await subtle.generateKey(AES_KEY, true, [
    "encrypt",
    "decrypt",
  ]);

  const rsaKeyPair = await subtle.generateKey(
    {
      name: "RSA-OAEP",
      hash: "SHA-1",
      publicExponent: new Uint8Array([1, 0, 1]),
      modulusLength: 2048,
    },
    false,
    ["wrapKey", "encrypt", "unwrapKey", "decrypt"],
  );

  const enc = await subtle.wrapKey(
    "raw",
    aesKey,
    rsaKeyPair.publicKey,
    RSA_KEY,
  );
  const unwrappedKey = await subtle.unwrapKey(
    "raw",
    enc,
    rsaKeyPair.privateKey,
    RSA_KEY,
    AES_KEY,
    false,
    ["encrypt", "decrypt"],
  );

  assert(unwrappedKey instanceof CryptoKey);
  assertEquals(unwrappedKey.type, "secret");
  assertEquals(unwrappedKey.extractable, false);
  assertEquals(unwrappedKey.usages, ["encrypt", "decrypt"]);
});

Deno.test(async function testDecryptWithInvalidIntializationVector() {
  // deno-fmt-ignore
  const data = new Uint8Array([42,42,42,42,42,42,42,42,42,42,42,42,42,42,42]);
  const key = await crypto.subtle.importKey(
    "raw",
    new Uint8Array(16),
    { name: "AES-CBC", length: 256 },
    true,
    ["encrypt", "decrypt"],
  );
  // deno-fmt-ignore
  const initVector = new Uint8Array([0,1,2,3,4,5,6,7,8,9,10,11,12,13,14,15]);
  const encrypted = await crypto.subtle.encrypt(
    { name: "AES-CBC", iv: initVector },
    key,
    data,
  );
  // deno-fmt-ignore
  const initVector2 = new Uint8Array([15,14,13,12,11,10,9,8,7,6,5,4,3,2,1,0]);
  await assertRejects(async () => {
    await crypto.subtle.decrypt(
      { name: "AES-CBC", iv: initVector2 },
      key,
      encrypted,
    );
  }, DOMException);
});

const jwtRSAKeys = {
  "1024": {
    size: 1024,
    publicJWK: {
      kty: "RSA",
      n: "zZn4sRGfjQos56yL_Qy1R9NI-THMnFynn94g5RxA6wGrJh4BJT3x6I9x0IbpS3q-d4ORA6R2vuDMh8dDFRr9RDH6XY-gUScc9U5Jz3UA2KmVfsCbnUPvcAmMV_ENA7_TF0ivVjuIFodyDTx7EKHNVTrHHSlrbt7spbmcivs23Zc",
      e: "AQAB",
    },
    privateJWK: {
      kty: "RSA",
      n: "zZn4sRGfjQos56yL_Qy1R9NI-THMnFynn94g5RxA6wGrJh4BJT3x6I9x0IbpS3q-d4ORA6R2vuDMh8dDFRr9RDH6XY-gUScc9U5Jz3UA2KmVfsCbnUPvcAmMV_ENA7_TF0ivVjuIFodyDTx7EKHNVTrHHSlrbt7spbmcivs23Zc",
      e: "AQAB",
      d: "YqIK_GdH85F-GWZdgfgmv15NE78gOaL5h2g4v7DeM9-JC7A5PHSLKNYn87HFGcC4vv0PBIBRtyCA_mJJfEaGWORVCOXSBpWNepMYpio52n3w5uj5UZEsBnbtZc0EtWhVF2Auqa7VbiKrWcQUEgEI8V0gE5D4tyBg8GXv9975dQE",
      p: "9BrAg5L1zfqGPuWJDuDCBX-TmtZdrOI3Ys4ZaN-yMPlTjwWSEPO0qnfjEZcw2VgXHgJJmbVco6TxckJCmEYqeQ",
      q: "157jDJ1Ya5nmQvTPbhKAPAeMWogxCyaQTkBrp30pEKd6mGSB385hqr4BIk8s3f7MdXpM-USpaZgUoT4o_2VEjw",
      dp:
        "qdd_QUzcaB-6jkKo1Ug-1xKIAgDLFsIjJUUfWt_iHL8ti2Kl2dOnTcCypgebPm5TT1bqHN-agGYAdK5zpX2UiQ",
      dq:
        "hNRfwOSplNfhLvxLUN7a2qA3yYm-1MSz_1DWQP7srlLORlUcYPht2FZmsnEeDcAqynBGPQUcbG2Av_hgHz2OZw",
      qi:
        "zbpJQAhinrxSbVKxBQ2EZGFUD2e3WCXbAJRYpk8HVQ5AA52OhKTicOye2hEHnrgpFKzC8iznTsCG3FMkvwcj4Q",
    },
  },

  "2048": {
    size: 2048,
    publicJWK: {
      kty: "RSA",
      // unpadded base64 for rawKey.
      n: "09eVwAhT9SPBxdEN-74BBeEANGaVGwqH-YglIc4VV7jfhR2by5ivzVq8NCeQ1_ACDIlTDY8CTMQ5E1c1SEXmo_T7q84XUGXf8U9mx6uRg46sV7fF-hkwJR80BFVsvWxp4ahPlVJYj__94ft7rIVvchb5tyalOjrYFCJoFnSgq-i3ZjU06csI9XnO5klINucD_Qq0vUhO23_Add2HSYoRjab8YiJJR_Eths7Pq6HHd2RSXmwYp5foRnwe0_U75XmesHWDJlJUHYbwCZo0kP9G8g4QbucwU-MSNBkZOO2x2ZtZNexpHd0ThkATbnNlpVG_z2AGNORp_Ve3rlXwrGIXXw",
      e: "AQAB",
    },
    privateJWK: {
      kty: "RSA",
      // unpadded base64 for rawKey.
      n: "09eVwAhT9SPBxdEN-74BBeEANGaVGwqH-YglIc4VV7jfhR2by5ivzVq8NCeQ1_ACDIlTDY8CTMQ5E1c1SEXmo_T7q84XUGXf8U9mx6uRg46sV7fF-hkwJR80BFVsvWxp4ahPlVJYj__94ft7rIVvchb5tyalOjrYFCJoFnSgq-i3ZjU06csI9XnO5klINucD_Qq0vUhO23_Add2HSYoRjab8YiJJR_Eths7Pq6HHd2RSXmwYp5foRnwe0_U75XmesHWDJlJUHYbwCZo0kP9G8g4QbucwU-MSNBkZOO2x2ZtZNexpHd0ThkATbnNlpVG_z2AGNORp_Ve3rlXwrGIXXw",
      e: "AQAB",
      d: "H4xboN2co0VP9kXL71G8lUOM5EDis8Q9u8uqu_4U75t4rjpamVeD1vFMVfgOehokM_m_hKVnkkcmuNqj9L90ObaiRFPM5QxG7YkFpXbHlPAKeoXD1hsqMF0VQg_2wb8DhberInHA_rEA_kaVhHvavQLu7Xez45gf1d_J4I4931vjlCB6cupbLL0H5hHsxbMsX_5nnmAJdL_U3gD-U7ZdQheUPhDBJR2KeGzvnTm3KVKpOnwn-1Cd45MU4-KDdP0FcBVEuBsSrsQHliTaciBgkbyj__BangPj3edDxTkb-fKkEvhkXRjAoJs1ixt8nfSGDce9cM_GqAX9XGb4s2QkAQ",
      dp:
        "mM82RBwzGzi9LAqjGbi-badLtHRRBoH9sfMrJuOtzxRnmwBFccg_lwy-qAhUTqnN9kvD0H1FzXWzoFPFJbyi-AOmumYGpWm_PvzQGldne5CPJ02pYaeg-t1BePsT3OpIq0Am8E2Kjf9polpRJwIjO7Kx8UJKkhg5bISnsy0V8wE",
      dq:
        "ZlM4AvrWIpXwqsH_5Q-6BsLJdbnN_GypFCXoT9VXniXncSBZIWCkgDndBdWkSzyzIN65NiMRBfZaf9yduTFj4kvOPwb3ch3J0OxGJk0Ary4OGSlS1zNwMl93ALGal1FzpWUuiia9L9RraGqXAUr13L7TIIMRobRjpAV-z7M-ruM",
      p: "7VwGt_tJcAFQHrmDw5dM1EBru6fidM45NDv6VVOEbxKuD5Sh2EfAHfm5c6oouA1gZqwvKH0sn_XpB1NsyYyHEQd3sBVdK0zRjTo-E9mRP-1s-LMd5YDXVq6HE339nxpXsmO25slQEF6zBrj1bSNNXBFc7fgDnlq-HIeleMvsY_E",
      q: "5HqMHLzb4IgXhUl4pLz7E4kjY8PH2YGzaQfK805zJMbOXzmlZK0hizKo34Qqd2nB9xos7QgzOYQrNfSWheARwVsSQzAE0vGvw3zHIPP_lTtChBlCTPctQcURjw4dXcnK1oQ-IT321FNOW3EO-YTsyGcypJqJujlZrLbxYjOjQE8",
      qi:
        "OQXzi9gypDnpdHatIi0FaUGP8LSzfVH0AUugURJXs4BTJpvA9y4hcpBQLrcl7H_vq6kbGmvC49V-9I5HNVX_AuxGIXKuLZr5WOxPq8gLTqHV7X5ZJDtWIP_nq2NNgCQQyNNRrxebiWlwGK9GnX_unewT6jopI_oFhwp0Q13rBR0",
    },
  },
  "4096": {
    size: 4096,
    publicJWK: {
      kty: "RSA",
      n: "2qr2TL2c2JmbsN0OLIRnaAB_ZKb1-Gh9H0qb4lrBuDaqkW_eFPwT-JIsvnNJvDT7BLJ57tTMIj56ZMtv6efSSTWSk9MOoW2J1K_iEretZ2cegB_aRX7qQVjnoFsz9U02BKfAIUT0o_K7b9G08d1rrAUohi_SVQhwObodg7BddMbKUmz70QNIS487LN44WUVnn9OgE9atTYUARNukT0DuQb3J-K20ksTuVujXbSelohDmLobqlGoi5sY_548Qs9BtFmQ2nGuEHNB2zdlZ5EvEqbUFVZ2QboG6jXdoos6qcwdgUvAhj1Hz10Ngic_RFqL7bNDoIOzNp66hdA35uxbwuaygZ16ikxoPj7eTYud1hrkyQCgeGw2YhCiKIE6eos_U5dL7WHRD5aSkkzsgXtnF8pVmStsuf0QcdAoC-eeCex0tSTgRw9AtGTz8Yr1tGQD9l_580zAXnE6jmrwRRQ68EEA7vohGov3tnG8pGyg_zcxeADLtPlfTc1tEwmh3SGrioDClioYCipm1JvkweEgP9eMPpEC8SgRU1VNDSVe1SF4uNsH8vA7PHFKfg6juqJEc5ht-l10FYER-Qq6bZXsU2oNcfE5SLDeLTWmxiHmxK00M8ABMFIV5gUkPoMiWcl87O6XwzA2chsIERp7Vb-Vn2O-EELiXzv7lPhc6fTGQ0Nc",
      e: "AQAB",
    },
    privateJWK: {
      kty: "RSA",
      n: "2qr2TL2c2JmbsN0OLIRnaAB_ZKb1-Gh9H0qb4lrBuDaqkW_eFPwT-JIsvnNJvDT7BLJ57tTMIj56ZMtv6efSSTWSk9MOoW2J1K_iEretZ2cegB_aRX7qQVjnoFsz9U02BKfAIUT0o_K7b9G08d1rrAUohi_SVQhwObodg7BddMbKUmz70QNIS487LN44WUVnn9OgE9atTYUARNukT0DuQb3J-K20ksTuVujXbSelohDmLobqlGoi5sY_548Qs9BtFmQ2nGuEHNB2zdlZ5EvEqbUFVZ2QboG6jXdoos6qcwdgUvAhj1Hz10Ngic_RFqL7bNDoIOzNp66hdA35uxbwuaygZ16ikxoPj7eTYud1hrkyQCgeGw2YhCiKIE6eos_U5dL7WHRD5aSkkzsgXtnF8pVmStsuf0QcdAoC-eeCex0tSTgRw9AtGTz8Yr1tGQD9l_580zAXnE6jmrwRRQ68EEA7vohGov3tnG8pGyg_zcxeADLtPlfTc1tEwmh3SGrioDClioYCipm1JvkweEgP9eMPpEC8SgRU1VNDSVe1SF4uNsH8vA7PHFKfg6juqJEc5ht-l10FYER-Qq6bZXsU2oNcfE5SLDeLTWmxiHmxK00M8ABMFIV5gUkPoMiWcl87O6XwzA2chsIERp7Vb-Vn2O-EELiXzv7lPhc6fTGQ0Nc",
      e: "AQAB",
      d: "uXPRXBhcE5-DWabBRKQuhxgU8ype5gTISWefeYP7U96ZHqu_sBByZ5ihdgyU9pgAZGVx4Ep9rnVKnH2lNr2zrP9Qhyqy99nM0aMxmypIWLAuP__DwLj4t99M4sU29c48CAq1egHfccSFjzpNuetOTCA71EJuokt70pm0OmGzgTyvjuR7VTLxd5PMXitBowSn8_cphmnFpT8tkTiuy8CH0R3DU7MOuINomDD1s8-yPBcVAVTPUnwJiauNuzestLQKMLlhT5wn-cAbYk36XRKdgkjSc2AkhHRl4WDqT1nzWYdh_DVIYSLiKSktkPO9ovMrRYiPtozfhl0m9SR9Ll0wXtcnnDlWXc_MSGpw18vmUBSJ4PIhkiFsvLn-db3wUkA8uve-iqqfk0sxlGWughWx03kGmZDmprWbXugCBHfsI4X93w4exznXH_tapxPnmjbhVUQR6p41MvO2lcHWPLwGJgLIoejBHpnn3TmMN0UjFZki7q9B_dJ3fXh0mX9DzAlC0sil1NgCPhMPq02393_giinQquMknrBvgKxGSfGUrDKuflCx611ZZlRM3R7YMX2OIy1g4DyhPzBVjxRMtm8PnIs3m3Hi-O-C_PHF93w9J8Wqd0yIw7SpavDqZXLPC6Cqi8K7MBZyVECXHtRj1bBqT-h_xZmFCDjSU0NqfOdgApE",
      p: "9NrXwq4kY9kBBOwLoFZVQc4kJI_NbKa_W9FLdQdRIbMsZZHXJ3XDUR9vJAcaaR75WwIC7X6N55nVtWTq28Bys9flJ9RrCTfciOntHEphBhYaL5ZTUl-6khYmsOf_psff2VaOOCvHGff5ejuOmBQxkw2E-cv7knRgWFHoLWpku2NJIMuGHt9ks7OAUfIZVYl9YJnw4FYUzhgaxemknjLeZ8XTkGW2zckzF-d95YI9i8zD80Umubsw-YxriSfqFQ0rGHBsbQ8ZOTd_KJju42BWnXIjNDYmjFUqdzVjI4XQ8EGrCEf_8_iwphGyXD7LOJ4fqd97B3bYpoRTPnCgY_SEHQ",
      q: "5J758_NeKr1XPZiLxXohYQQnh0Lb4QtGZ1xzCgjhBQLcIBeTOG_tYjCues9tmLt93LpJfypSJ-SjDLwkR2s069_IByYGpxyeGtV-ulqYhSw1nD2CXKMDGyO5jXDs9tJrS_UhfobXKQH03CRdFugyPkSNmXY-AafFynG7xLr7oYBC05FnhUXPm3VBTPt9K-BpqwYd_h9vkAWeprSPo83UlwcLMupSJY9LaHxhRdz2yi0ZKNwXXHRwcszGjDBvvzUcCYbqWqjzbEvFY6KtH8Jh4LhM46rHaoEOTernJsDF6a6W8Df88RthqTExcwnaQf0O_dlbjSxEIPfbxx8t1EQugw",
      dp:
        "4Y7Hu5tYAnLhMXuQqj9dgqU3PkcKYdCp7xc6f7Ah2P2JJHfYz4z4RD7Ez1eLyNKzulZ8A_PVHUjlSZiRkaYTBAEaJDrV70P6cFWuC6WpA0ZREQ1V7EgrQnANbGILa8QsPbYyhSQu4YlB1IwQq5_OmzyVBtgWA7AZIMMzMsMT0FuB_if-gWohBjmRN-vh0p45VUf6UW568-_YmgDFmMYbg1UFs7s_TwrNenPR0h7MO4CB8hP9vJLoZrooRczzIjljPbwy5bRG9CJfjTJ0vhj9MUT3kR1hHV1HJVGU5iBbfTfBKnvJGSI6-IDM4ZUm-B0R5hbs6s9cfOjhFmACIJIbMQ",
      dq:
        "gT4iPbfyHyVEwWyQb4X4grjvg7bXSKSwG1SXMDAOzV9tg7LwJjKYNy8gJAtJgNNVdsfVLs-E_Epzpoph1AIWO9YZZXkov6Yc9zyEVONMX9S7ReU74hTBd8E9b2lMfMg9ogYk9jtSPTt-6kigW4fOh4cHqZ6_tP3cgfLD3JZ8FDPHE4WaySvLDq49yUBO5dQKyIU_xV6OGhQjOUjP_yEoMmzn9tOittsIHTxbXTxqQ6c1FvU9O6YTv8Jl5_Cl66khfX1I1RG38xvurcHULyUbYgeuZ_Iuo9XreT73h9_owo9RguGT29XH4vcNZmRGf5GIvRb4e5lvtleIZkwJA3u78w",
      qi:
        "JHmVKb1zwW5iRR6RCeexYnh2fmY-3DrPSdM8Dxhr0F8dayi-tlRqEdnG0hvp45n8gLUskWWcB9EXlUJObZGKDfGuxgMa3g_xeLA2vmFQ12MxPsyH4iCNZvsgmGxx7TuOHrnDh5EBVnM4_de63crEJON2sYI8Ozi-xp2OEmAr2seWKq4sxkFni6exLhqb-NE4m9HMKlng1EtQh2rLBFG1VYD3SYYpMLc5fxzqGvSxn3Fa-Xgg-IZPY3ubrcm52KYgmLUGmnYStfVqGSWSdhDXHlNgI5pdAA0FzpyBk3ZX-JsxhwcnneKrYBBweq06kRMGWgvdbdAQ-7wSeGqqj5VPwA",
    },
  },
};

Deno.test(async function testImportRsaJwk() {
  const subtle = globalThis.crypto.subtle;
  assert(subtle);

  for (const [_key, jwkData] of Object.entries(jwtRSAKeys)) {
    const { size, publicJWK, privateJWK } = jwkData;
    if (size < 2048) {
      continue;
    }

    // 1. Test import PSS
    for (const hash of ["SHA-1", "SHA-256", "SHA-384", "SHA-512"]) {
      const hashMapPSS: Record<string, string> = {
        "SHA-1": "PS1",
        "SHA-256": "PS256",
        "SHA-384": "PS384",
        "SHA-512": "PS512",
      };

      if (size == 1024 && hash == "SHA-512") {
        continue;
      }

      const privateKeyPSS = await crypto.subtle.importKey(
        "jwk",
        {
          alg: hashMapPSS[hash],
          ...privateJWK,
          ext: true,
          "key_ops": ["sign"],
        },
        { name: "RSA-PSS", hash },
        true,
        ["sign"],
      );

      const publicKeyPSS = await crypto.subtle.importKey(
        "jwk",
        {
          alg: hashMapPSS[hash],
          ...publicJWK,
          ext: true,
          "key_ops": ["verify"],
        },
        { name: "RSA-PSS", hash },
        true,
        ["verify"],
      );

      const signaturePSS = await crypto.subtle.sign(
        { name: "RSA-PSS", saltLength: 32 },
        privateKeyPSS,
        new Uint8Array([1, 2, 3, 4]),
      );

      const verifyPSS = await crypto.subtle.verify(
        { name: "RSA-PSS", saltLength: 32 },
        publicKeyPSS,
        signaturePSS,
        new Uint8Array([1, 2, 3, 4]),
      );
      assert(verifyPSS);
    }

    // 2. Test import PKCS1
    for (const hash of ["SHA-1", "SHA-256", "SHA-384", "SHA-512"]) {
      const hashMapPKCS1: Record<string, string> = {
        "SHA-1": "RS1",
        "SHA-256": "RS256",
        "SHA-384": "RS384",
        "SHA-512": "RS512",
      };

      if (size == 1024 && hash == "SHA-512") {
        continue;
      }

      const privateKeyPKCS1 = await crypto.subtle.importKey(
        "jwk",
        {
          alg: hashMapPKCS1[hash],
          ...privateJWK,
          ext: true,
          "key_ops": ["sign"],
        },
        { name: "RSASSA-PKCS1-v1_5", hash },
        true,
        ["sign"],
      );

      const publicKeyPKCS1 = await crypto.subtle.importKey(
        "jwk",
        {
          alg: hashMapPKCS1[hash],
          ...publicJWK,
          ext: true,
          "key_ops": ["verify"],
        },
        { name: "RSASSA-PKCS1-v1_5", hash },
        true,
        ["verify"],
      );

      const signaturePKCS1 = await crypto.subtle.sign(
        { name: "RSASSA-PKCS1-v1_5", saltLength: 32 },
        privateKeyPKCS1,
        new Uint8Array([1, 2, 3, 4]),
      );

      const verifyPKCS1 = await crypto.subtle.verify(
        { name: "RSASSA-PKCS1-v1_5", saltLength: 32 },
        publicKeyPKCS1,
        signaturePKCS1,
        new Uint8Array([1, 2, 3, 4]),
      );
      assert(verifyPKCS1);
    }

    // 3. Test import OAEP
    for (
      const { hash, plainText } of hashPlainTextVector
    ) {
      const hashMapOAEP: Record<string, string> = {
        "SHA-1": "RSA-OAEP",
        "SHA-256": "RSA-OAEP-256",
        "SHA-384": "RSA-OAEP-384",
        "SHA-512": "RSA-OAEP-512",
      };

      if (size == 1024 && hash == "SHA-512") {
        continue;
      }

      const encryptAlgorithm = { name: "RSA-OAEP" };

      const privateKeyOAEP = await crypto.subtle.importKey(
        "jwk",
        {
          alg: hashMapOAEP[hash],
          ...privateJWK,
          ext: true,
          "key_ops": ["decrypt"],
        },
        { ...encryptAlgorithm, hash },
        true,
        ["decrypt"],
      );

      const publicKeyOAEP = await crypto.subtle.importKey(
        "jwk",
        {
          alg: hashMapOAEP[hash],
          ...publicJWK,
          ext: true,
          "key_ops": ["encrypt"],
        },
        { ...encryptAlgorithm, hash },
        true,
        ["encrypt"],
      );
      const cipherText = await subtle.encrypt(
        encryptAlgorithm,
        publicKeyOAEP,
        plainText,
      );

      assert(cipherText);
      assert(cipherText.byteLength > 0);
      assertEquals(cipherText.byteLength * 8, size);
      assert(cipherText instanceof ArrayBuffer);

      const decrypted = await subtle.decrypt(
        encryptAlgorithm,
        privateKeyOAEP,
        cipherText,
      );
      assert(decrypted);
      assert(decrypted instanceof ArrayBuffer);
      assertEquals(new Uint8Array(decrypted), plainText);
    }
  }
});

const jwtECKeys = {
  "256": {
    size: 256,
    algo: "ES256",
    publicJWK: {
      kty: "EC",
      crv: "P-256",
      x: "0hCwpvnZ8BKGgFi0P6T0cQGFQ7ugDJJQ35JXwqyuXdE",
      y: "zgN1UtSBRQzjm00QlXAbF1v6s0uObAmeGPHBmDWDYeg",
    },
    privateJWK: {
      kty: "EC",
      crv: "P-256",
      x: "0hCwpvnZ8BKGgFi0P6T0cQGFQ7ugDJJQ35JXwqyuXdE",
      y: "zgN1UtSBRQzjm00QlXAbF1v6s0uObAmeGPHBmDWDYeg",
      d: "E9M6LVq_nPnrsh_4YNSu_m5W53eQ9N7ptAiE69M1ROo",
    },
  },
  "384": {
    size: 384,
    algo: "ES384",
    publicJWK: {
      kty: "EC",
      crv: "P-384",
      x: "IZwU1mYXs27G2IVrOFtzp000T9iude8EZDXdpU47RL1fvevR0I3Wni19wdwhjLQ1",
      y: "vSgTjMd4M3qEL2vWGyQOdCSfJGZ8KlgQp2v8KOAzX4imUB3sAZdtqFr7AIactqzo",
    },
    privateJWK: {
      kty: "EC",
      crv: "P-384",
      x: "IZwU1mYXs27G2IVrOFtzp000T9iude8EZDXdpU47RL1fvevR0I3Wni19wdwhjLQ1",
      y: "vSgTjMd4M3qEL2vWGyQOdCSfJGZ8KlgQp2v8KOAzX4imUB3sAZdtqFr7AIactqzo",
      d: "RTe1mQeE08LSLpao-S-hqkku6HPldqQVguFEGDyYiNEOa560ztSyzEAS5KxeqEBz",
    },
  },
};

type JWK = Record<string, string>;

function equalJwk(expected: JWK, got: JWK): boolean {
  const fields = Object.keys(expected);

  for (let i = 0; i < fields.length; i++) {
    const fieldName = fields[i];

    if (!(fieldName in got)) {
      return false;
    }
    if (expected[fieldName] !== got[fieldName]) {
      return false;
    }
  }

  return true;
}

Deno.test(async function testImportExportEcDsaJwk() {
  const subtle = crypto.subtle;
  assert(subtle);

  for (
    const [_key, keyData] of Object.entries(jwtECKeys)
  ) {
    const { publicJWK, privateJWK, algo } = keyData;

    // 1. Test import EcDsa
    const privateKeyECDSA = await subtle.importKey(
      "jwk",
      {
        alg: algo,
        ...privateJWK,
        ext: true,
        "key_ops": ["sign"],
      },
      { name: "ECDSA", namedCurve: privateJWK.crv },
      true,
      ["sign"],
    );
    const expPrivateKeyJWK = await subtle.exportKey(
      "jwk",
      privateKeyECDSA,
    );
    assert(equalJwk(privateJWK, expPrivateKeyJWK as JWK));

    const publicKeyECDSA = await subtle.importKey(
      "jwk",
      {
        alg: algo,
        ...publicJWK,
        ext: true,
        "key_ops": ["verify"],
      },
      { name: "ECDSA", namedCurve: publicJWK.crv },
      true,
      ["verify"],
    );

    const expPublicKeyJWK = await subtle.exportKey(
      "jwk",
      publicKeyECDSA,
    );

    assert(equalJwk(publicJWK, expPublicKeyJWK as JWK));

    const signatureECDSA = await subtle.sign(
      { name: "ECDSA", hash: `SHA-${keyData.size}` },
      privateKeyECDSA,
      new Uint8Array([1, 2, 3, 4]),
    );

    const verifyECDSA = await subtle.verify(
      { name: "ECDSA", hash: `SHA-${keyData.size}` },
      publicKeyECDSA,
      signatureECDSA,
      new Uint8Array([1, 2, 3, 4]),
    );
    assert(verifyECDSA);
  }
});

Deno.test(async function testEcdsaCrossHashAlgorithms() {
  // Ensure that we can sign and verify with hash algorithms that
  // don't match the curve's nominal size. For example, sign using
  // P-384 with a SHA-256 digest and verify using the same params.
  const subtle = crypto.subtle;
  assert(subtle);
  const ec384 = jwtECKeys["384"];
  const data = new Uint8Array([5, 6, 7, 8]);
  const privateKey = await subtle.importKey(
    "jwk",
    {
      alg: ec384.algo,
      ...ec384.privateJWK,
      ext: true,
      "key_ops": ["sign"],
    },
    { name: "ECDSA", namedCurve: ec384.privateJWK.crv },
    true,
    ["sign"],
  );
  const publicKey = await subtle.importKey(
    "jwk",
    {
      alg: ec384.algo,
      ...ec384.publicJWK,
      ext: true,
      "key_ops": ["verify"],
    },
    { name: "ECDSA", namedCurve: ec384.publicJWK.crv },
    true,
    ["verify"],
  );
  // Use SHA-256 instead of SHA-384 with P-384 keys.
  const signature = await subtle.sign(
    { name: "ECDSA", hash: "SHA-256" },
    privateKey,
    data,
  );
  const verified = await subtle.verify(
    { name: "ECDSA", hash: "SHA-256" },
    publicKey,
    signature,
    data,
  );
  assert(verified);
});

Deno.test(async function testImportEcDhJwk() {
  const subtle = crypto.subtle;
  assert(subtle);

  for (
    const [_key, jwkData] of Object.entries(jwtECKeys)
  ) {
    const { size, publicJWK, privateJWK } = jwkData;

    // 1. Test import EcDsa
    const privateKeyECDH = await subtle.importKey(
      "jwk",
      {
        ...privateJWK,
        ext: true,
        "key_ops": ["deriveBits"],
      },
      { name: "ECDH", namedCurve: privateJWK.crv },
      true,
      ["deriveBits"],
    );

    const expPrivateKeyJWK = await subtle.exportKey(
      "jwk",
      privateKeyECDH,
    );
    assert(equalJwk(privateJWK, expPrivateKeyJWK as JWK));

    const publicKeyECDH = await subtle.importKey(
      "jwk",
      {
        ...publicJWK,
        ext: true,
        "key_ops": [],
      },
      { name: "ECDH", namedCurve: publicJWK.crv },
      true,
      [],
    );
    const expPublicKeyJWK = await subtle.exportKey(
      "jwk",
      publicKeyECDH,
    );
    assert(equalJwk(publicJWK, expPublicKeyJWK as JWK));

    const derivedKey = await subtle.deriveBits(
      {
        name: "ECDH",
        public: publicKeyECDH,
      },
      privateKeyECDH,
      size,
    );

    assert(derivedKey instanceof ArrayBuffer);
    assertEquals(derivedKey.byteLength, size / 8);
  }
});

const ecTestKeys = [
  {
    size: 256,
    namedCurve: "P-256",
    signatureLength: 64,
    // deno-fmt-ignore
    raw: new Uint8Array([
      4, 210, 16, 176, 166, 249, 217, 240, 18, 134, 128, 88, 180, 63, 164, 244,
      113, 1, 133, 67, 187, 160, 12, 146, 80, 223, 146, 87, 194, 172, 174, 93,
      209, 206, 3, 117, 82, 212, 129, 69, 12, 227, 155, 77, 16, 149, 112, 27,
      23, 91, 250, 179, 75, 142, 108, 9, 158, 24, 241, 193, 152, 53, 131, 97,
      232,
    ]),
    // deno-fmt-ignore
    spki: new Uint8Array([
      48, 89, 48, 19, 6, 7, 42, 134, 72, 206, 61, 2, 1, 6, 8, 42, 134, 72, 206,
      61, 3, 1, 7, 3, 66, 0, 4, 210, 16, 176, 166, 249, 217, 240, 18, 134, 128,
      88, 180, 63, 164, 244, 113, 1, 133, 67, 187, 160, 12, 146, 80, 223, 146,
      87, 194, 172, 174, 93, 209, 206, 3, 117, 82, 212, 129, 69, 12, 227, 155,
      77, 16, 149, 112, 27, 23, 91, 250, 179, 75, 142, 108, 9, 158, 24, 241,
      193, 152, 53, 131, 97, 232,
    ]),
    // deno-fmt-ignore
    pkcs8: new Uint8Array([
      48, 129, 135, 2, 1, 0, 48, 19, 6, 7, 42, 134, 72, 206, 61, 2, 1, 6, 8, 42,
      134, 72, 206, 61, 3, 1, 7, 4, 109, 48, 107, 2, 1, 1, 4, 32, 19, 211, 58,
      45, 90, 191, 156, 249, 235, 178, 31, 248, 96, 212, 174, 254, 110, 86, 231,
      119, 144, 244, 222, 233, 180, 8, 132, 235, 211, 53, 68, 234, 161, 68, 3,
      66, 0, 4, 210, 16, 176, 166, 249, 217, 240, 18, 134, 128, 88, 180, 63,
      164, 244, 113, 1, 133, 67, 187, 160, 12, 146, 80, 223, 146, 87, 194, 172,
      174, 93, 209, 206, 3, 117, 82, 212, 129, 69, 12, 227, 155, 77, 16, 149,
      112, 27, 23, 91, 250, 179, 75, 142, 108, 9, 158, 24, 241, 193, 152, 53,
      131, 97, 232,
    ]),
  },
  {
    size: 384,
    namedCurve: "P-384",
    signatureLength: 96,
    // deno-fmt-ignore
    raw: new Uint8Array([
      4, 118, 64, 176, 165, 100, 177, 112, 49, 254, 58, 53, 158, 63, 73, 200,
      148, 248, 242, 216, 186, 80, 92, 160, 53, 64, 232, 157, 19, 1, 12, 226,
      115, 51, 42, 143, 98, 206, 55, 220, 108, 78, 24, 71, 157, 21, 120, 126,
      104, 157, 86, 48, 226, 110, 96, 52, 48, 77, 170, 9, 231, 159, 26, 165,
      200, 26, 164, 99, 46, 227, 169, 105, 172, 225, 60, 102, 141, 145, 139,
      165, 47, 72, 53, 17, 17, 246, 161, 220, 26, 21, 23, 219, 1, 107, 185,
      163, 215,
    ]),
    // deno-fmt-ignore
    spki: new Uint8Array([
      48, 118, 48, 16, 6, 7, 42, 134, 72, 206, 61, 2, 1, 6, 5, 43, 129, 4, 0,
      34, 3, 98, 0, 4, 118, 64, 176, 165, 100, 177, 112, 49, 254, 58, 53, 158,
      63, 73, 200, 148, 248, 242, 216, 186, 80, 92, 160, 53, 64, 232, 157, 19,
      1, 12, 226, 115, 51, 42, 143, 98, 206, 55, 220, 108, 78, 24, 71, 157, 21,
      120, 126, 104, 157, 86, 48, 226, 110, 96, 52, 48, 77, 170, 9, 231, 159,
      26, 165, 200, 26, 164, 99, 46, 227, 169, 105, 172, 225, 60, 102, 141,
      145, 139, 165, 47, 72, 53, 17, 17, 246, 161, 220, 26, 21, 23, 219, 1,
      107, 185, 163, 215,
    ]),
    // deno-fmt-ignore
    pkcs8: new Uint8Array([
      48, 129, 182, 2, 1, 0, 48, 16, 6, 7, 42, 134, 72, 206, 61, 2, 1, 6, 5, 43,
      129, 4, 0, 34, 4, 129, 158, 48, 129, 155, 2, 1, 1, 4, 48, 202, 7, 195,
      169, 124, 170, 81, 169, 253, 127, 56, 28, 98, 90, 255, 165, 72, 142, 133,
      138, 237, 200, 176, 92, 179, 192, 83, 28, 47, 118, 157, 152, 47, 65, 133,
      140, 50, 83, 182, 191, 224, 96, 216, 179, 59, 150, 15, 233, 161, 100, 3,
      98, 0, 4, 118, 64, 176, 165, 100, 177, 112, 49, 254, 58, 53, 158, 63, 73,
      200, 148, 248, 242, 216, 186, 80, 92, 160, 53, 64, 232, 157, 19, 1, 12,
      226, 115, 51, 42, 143, 98, 206, 55, 220, 108, 78, 24, 71, 157, 21, 120,
      126, 104, 157, 86, 48, 226, 110, 96, 52, 48, 77, 170, 9, 231, 159, 26,
      165, 200, 26, 164, 99, 46, 227, 169, 105, 172, 225, 60, 102, 141, 145,
      139, 165, 47, 72, 53, 17, 17, 246, 161, 220, 26, 21, 23, 219, 1, 107,
      185, 163, 215,
    ]),
  },
];

Deno.test(async function testImportEcSpkiPkcs8() {
  const subtle = globalThis.crypto.subtle;
  assert(subtle);

  for (
    const { namedCurve, raw, spki, pkcs8 } of ecTestKeys
  ) {
    const rawPublicKeyECDSA = await subtle.importKey(
      "raw",
      raw,
      { name: "ECDSA", namedCurve },
      true,
      ["verify"],
    );

    const expPublicKeyRaw = await subtle.exportKey(
      "raw",
      rawPublicKeyECDSA,
    );

    assertEquals(new Uint8Array(expPublicKeyRaw), raw);

    const privateKeyECDSA = await subtle.importKey(
      "pkcs8",
      pkcs8,
      { name: "ECDSA", namedCurve },
      true,
      ["sign"],
    );

    const expPrivateKeyPKCS8 = await subtle.exportKey(
      "pkcs8",
      privateKeyECDSA,
    );

    assertEquals(new Uint8Array(expPrivateKeyPKCS8), pkcs8);

    const expPrivateKeyJWK = await subtle.exportKey(
      "jwk",
      privateKeyECDSA,
    );

    assertEquals(expPrivateKeyJWK.crv, namedCurve);

    const publicKeyECDSA = await subtle.importKey(
      "spki",
      spki,
      { name: "ECDSA", namedCurve },
      true,
      ["verify"],
    );

    const expPublicKeySPKI = await subtle.exportKey(
      "spki",
      publicKeyECDSA,
    );

    assertEquals(new Uint8Array(expPublicKeySPKI), spki);

    const expPublicKeyJWK = await subtle.exportKey(
      "jwk",
      publicKeyECDSA,
    );

    assertEquals(expPublicKeyJWK.crv, namedCurve);

    for (
      const hash of ["SHA-1", "SHA-256", "SHA-384", "SHA-512"]
    ) {
      if (
        (hash == "SHA-256" && namedCurve == "P-256") ||
        (hash == "SHA-384" && namedCurve == "P-384") ||
        (hash == "SHA-512" && namedCurve == "P-521")
      ) {
        const signatureECDSA = await subtle.sign(
          { name: "ECDSA", hash },
          privateKeyECDSA,
          new Uint8Array([1, 2, 3, 4]),
        );

        const verifyECDSA = await subtle.verify(
          { name: "ECDSA", hash },
          publicKeyECDSA,
          signatureECDSA,
          new Uint8Array([1, 2, 3, 4]),
        );
        assert(verifyECDSA);
      }
    }
  }
});

Deno.test(async function testAesGcmEncrypt() {
  const key = await crypto.subtle.importKey(
    "raw",
    new Uint8Array(16),
    { name: "AES-GCM", length: 256 },
    true,
    ["encrypt", "decrypt"],
  );

  const nonces = [{
    iv: new Uint8Array([0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11]),
    ciphertext: new Uint8Array([
      50,
      223,
      112,
      178,
      166,
      156,
      255,
      110,
      125,
      138,
      95,
      141,
      82,
      47,
      14,
      164,
      134,
      247,
      22,
    ]),
  }, {
    iv: new Uint8Array([0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15]),
    ciphertext: new Uint8Array([
      210,
      101,
      81,
      216,
      151,
      9,
      192,
      197,
      62,
      254,
      28,
      132,
      89,
      106,
      40,
      29,
      175,
      232,
      201,
    ]),
  }];
  for (const { iv, ciphertext: fixture } of nonces) {
    const data = new Uint8Array([1, 2, 3]);

    const cipherText = await crypto.subtle.encrypt(
      { name: "AES-GCM", iv },
      key,
      data,
    );

    assert(cipherText instanceof ArrayBuffer);
    assertEquals(cipherText.byteLength, 19);
    assertEquals(
      new Uint8Array(cipherText),
      fixture,
    );

    const plainText = await crypto.subtle.decrypt(
      { name: "AES-GCM", iv },
      key,
      cipherText,
    );
    assert(plainText instanceof ArrayBuffer);
    assertEquals(plainText.byteLength, 3);
    assertEquals(new Uint8Array(plainText), data);
  }
});

async function roundTripSecretJwk(
  jwk: JsonWebKey,
  algId: AlgorithmIdentifier | HmacImportParams,
  ops: KeyUsage[],
  validateKeys: (
    key: CryptoKey,
    originalJwk: JsonWebKey,
    exportedJwk: JsonWebKey,
  ) => void,
) {
  const key = await crypto.subtle.importKey(
    "jwk",
    jwk,
    algId,
    true,
    ops,
  );

  assert(key instanceof CryptoKey);
  assertEquals(key.type, "secret");

  const exportedKey = await crypto.subtle.exportKey("jwk", key);

  validateKeys(key, jwk, exportedKey);
}

Deno.test(async function testSecretJwkBase64Url() {
  // Test 16bits with "overflow" in 3rd pos of 'quartet', no padding
  const keyData = `{
      "kty": "oct",
      "k": "xxx",
      "alg": "HS512",
      "key_ops": ["sign", "verify"],
      "ext": true
    }`;

  await roundTripSecretJwk(
    JSON.parse(keyData),
    { name: "HMAC", hash: "SHA-512" },
    ["sign", "verify"],
    (key, _orig, exp) => {
      assertEquals((key.algorithm as HmacKeyAlgorithm).length, 16);

      assertEquals(exp.k, "xxw");
    },
  );

  // HMAC 128bits with base64url characters (-_)
  await roundTripSecretJwk(
    {
      kty: "oct",
      k: "HnZXRyDKn-_G5Fx4JWR1YA",
      alg: "HS256",
      "key_ops": ["sign", "verify"],
      ext: true,
    },
    { name: "HMAC", hash: "SHA-256" },
    ["sign", "verify"],
    (key, orig, exp) => {
      assertEquals((key.algorithm as HmacKeyAlgorithm).length, 128);

      assertEquals(orig.k, exp.k);
    },
  );

  // HMAC 104bits/(12+1) bytes with base64url characters (-_), padding and overflow in 2rd pos of "quartet"
  await roundTripSecretJwk(
    {
      kty: "oct",
      k: "a-_AlFa-2-OmEGa_-z==",
      alg: "HS384",
      "key_ops": ["sign", "verify"],
      ext: true,
    },
    { name: "HMAC", hash: "SHA-384" },
    ["sign", "verify"],
    (key, _orig, exp) => {
      assertEquals((key.algorithm as HmacKeyAlgorithm).length, 104);

      assertEquals("a-_AlFa-2-OmEGa_-w", exp.k);
    },
  );

  // AES-CBC 128bits with base64url characters (-_) no padding
  await roundTripSecretJwk(
    {
      kty: "oct",
      k: "_u3K_gEjRWf-7cr-ASNFZw",
      alg: "A128CBC",
      "key_ops": ["encrypt", "decrypt"],
      ext: true,
    },
    { name: "AES-CBC" },
    ["encrypt", "decrypt"],
    (_key, orig, exp) => {
      assertEquals(orig.k, exp.k);
    },
  );

  // AES-CBC 128bits of '1' with padding chars
  await roundTripSecretJwk(
    {
      kty: "oct",
      k: "_____________________w==",
      alg: "A128CBC",
      "key_ops": ["encrypt", "decrypt"],
      ext: true,
    },
    { name: "AES-CBC" },
    ["encrypt", "decrypt"],
    (_key, _orig, exp) => {
      assertEquals(exp.k, "_____________________w");
    },
  );
});

Deno.test(async function testAESWrapKey() {
  const key = await crypto.subtle.generateKey(
    {
      name: "AES-KW",
      length: 128,
    },
    true,
    ["wrapKey", "unwrapKey"],
  );

  const hmacKey = await crypto.subtle.generateKey(
    {
      name: "HMAC",
      hash: "SHA-256",
      length: 128,
    },
    true,
    ["sign"],
  );

  //round-trip
  // wrap-unwrap-export compare
  const wrappedKey = await crypto.subtle.wrapKey(
    "raw",
    hmacKey,
    key,
    {
      name: "AES-KW",
    },
  );

  assert(wrappedKey instanceof ArrayBuffer);
  assertEquals(wrappedKey.byteLength, 16 + 8); // 8 = 'auth tag'

  const unwrappedKey = await crypto.subtle.unwrapKey(
    "raw",
    wrappedKey,
    key,
    {
      name: "AES-KW",
    },
    {
      name: "HMAC",
      hash: "SHA-256",
    },
    true,
    ["sign"],
  );

  assert(unwrappedKey instanceof CryptoKey);
  assertEquals((unwrappedKey.algorithm as HmacKeyAlgorithm).length, 128);

  const hmacKeyBytes = await crypto.subtle.exportKey("raw", hmacKey);
  const unwrappedKeyBytes = await crypto.subtle.exportKey("raw", unwrappedKey);

  assertEquals(new Uint8Array(hmacKeyBytes), new Uint8Array(unwrappedKeyBytes));
});

// https://github.com/denoland/deno/issues/13534
Deno.test(async function testAesGcmTagLength() {
  const key = await crypto.subtle.importKey(
    "raw",
    new Uint8Array(32),
    "AES-GCM",
    false,
    ["encrypt", "decrypt"],
  );

  const iv = crypto.getRandomValues(new Uint8Array(12));

  // encrypt won't fail, it will simply truncate the tag
  // as expected.
  const encrypted = await crypto.subtle.encrypt(
    { name: "AES-GCM", iv, tagLength: 96 },
    key,
    new Uint8Array(32),
  );

  await assertRejects(async () => {
    await crypto.subtle.decrypt(
      { name: "AES-GCM", iv, tagLength: 96 },
      key,
      encrypted,
    );
  });
});

Deno.test(async function ecPrivateKeyMaterialExportSpki() {
  // `generateKey` generates a key pair internally stored as "private" key.
  const keys = await crypto.subtle.generateKey(
    { name: "ECDSA", namedCurve: "P-256" },
    true,
    ["sign", "verify"],
  );

  assert(keys.privateKey instanceof CryptoKey);
  assert(keys.publicKey instanceof CryptoKey);

  // `exportKey` should be able to perform necessary conversion to export spki.
  const spki = await crypto.subtle.exportKey("spki", keys.publicKey);
  assert(spki instanceof ArrayBuffer);
});

// https://github.com/denoland/deno/issues/13911
Deno.test(async function importJwkWithUse() {
  const jwk = {
    "kty": "EC",
    "use": "sig",
    "crv": "P-256",
    "x": "FWZ9rSkLt6Dx9E3pxLybhdM6xgR5obGsj5_pqmnz5J4",
    "y": "_n8G69C-A2Xl4xUW2lF0i8ZGZnk_KPYrhv4GbTGu5G4",
  };

  const algorithm = { name: "ECDSA", namedCurve: "P-256" };

  const key = await crypto.subtle.importKey(
    "jwk",
    jwk,
    algorithm,
    true,
    ["verify"],
  );

  assert(key instanceof CryptoKey);
});

// https://github.com/denoland/deno/issues/14215
Deno.test(async function exportKeyNotExtractable() {
  const key = await crypto.subtle.generateKey(
    {
      name: "HMAC",
      hash: "SHA-512",
    },
    false,
    ["sign", "verify"],
  );

  assert(key);
  assertEquals(key.extractable, false);

  await assertRejects(async () => {
    // Should fail
    await crypto.subtle.exportKey("raw", key);
  }, DOMException);
});

// https://github.com/denoland/deno/issues/15126
Deno.test(async function testImportLeadingZeroesKey() {
  const alg = { name: "ECDSA", namedCurve: "P-256" };

  const jwk = {
    kty: "EC",
    crv: "P-256",
    alg: "ES256",
    x: "EvidcdFB1xC6tgfakqZsU9aIURxAJkcX62zHe1Nt6xU",
    y: "AHsk6BioGM7MZWeXOE_49AGmtuaXFT3Ill3DYtz9uYg",
    d: "WDeYo4o1heCF9l_2VIaClRyIeO16zsMlN8UG6Le9dU8",
    "key_ops": ["sign"],
    ext: true,
  };

  const key = await crypto.subtle.importKey(
    "jwk",
    jwk,
```

<a id="ref-18"></a>
[18] [`Repo denoland/deno: tests/unit/webcrypto_test.ts`](https://github.com/denoland/deno/blob/d6212d40/tests/unit/webcrypto_test.ts#L887-L902) lines 887-902

```ts
  const key = await crypto.subtle.generateKey(
    {
      name: "AES-GCM",
      length: 256,
    },
    true,
    ["encrypt", "decrypt"],
  );

  assert(key);
  assertEquals(key.type, "secret");
  assertEquals(key.extractable, true);
  assertEquals(key.usages, ["encrypt", "decrypt"]);
  const algorithm = key.algorithm as AesKeyAlgorithm;
  assertEquals(algorithm.name, "AES-GCM");
  assertEquals(algorithm.length, 256);
```

<a id="ref-19"></a>
[19] [`Repo denoland/deno: tests/unit/webcrypto_test.ts`](https://github.com/denoland/deno/blob/d6212d40/tests/unit/webcrypto_test.ts#L905-L957) lines 905-957

```ts
Deno.test(async function testUnwrapKey() {
  const subtle = crypto.subtle;

  const AES_KEY: AesKeyAlgorithm & AesCbcParams = {
    name: "AES-CBC",
    length: 128,
    iv: new Uint8Array(16),
  };

  const RSA_KEY: RsaHashedKeyGenParams & RsaOaepParams = {
    name: "RSA-OAEP",
    modulusLength: 2048,
    publicExponent: new Uint8Array([1, 0, 1]),
    hash: "SHA-1",
  };

  const aesKey = await subtle.generateKey(AES_KEY, true, [
    "encrypt",
    "decrypt",
  ]);

  const rsaKeyPair = await subtle.generateKey(
    {
      name: "RSA-OAEP",
      hash: "SHA-1",
      publicExponent: new Uint8Array([1, 0, 1]),
      modulusLength: 2048,
    },
    false,
    ["wrapKey", "encrypt", "unwrapKey", "decrypt"],
  );

  const enc = await subtle.wrapKey(
    "raw",
    aesKey,
    rsaKeyPair.publicKey,
    RSA_KEY,
  );
  const unwrappedKey = await subtle.unwrapKey(
    "raw",
    enc,
    rsaKeyPair.privateKey,
    RSA_KEY,
    AES_KEY,
    false,
    ["encrypt", "decrypt"],
  );

  assert(unwrappedKey instanceof CryptoKey);
  assertEquals(unwrappedKey.type, "secret");
  assertEquals(unwrappedKey.extractable, false);
  assertEquals(unwrappedKey.usages, ["encrypt", "decrypt"]);
});
```

<a id="ref-20"></a>
[20] [`Repo denoland/deno: tests/unit/webcrypto_test.ts`](https://github.com/denoland/deno/blob/d6212d40/tests/unit/webcrypto_test.ts#L953-L956) lines 953-956

```ts
  assert(unwrappedKey instanceof CryptoKey);
  assertEquals(unwrappedKey.type, "secret");
  assertEquals(unwrappedKey.extractable, false);
  assertEquals(unwrappedKey.usages, ["encrypt", "decrypt"]);
```

<a id="ref-21"></a>
[21] [`Repo denoland/deno: tests/unit/webcrypto_test.ts`](https://github.com/denoland/deno/blob/d6212d40/tests/unit/webcrypto_test.ts#L1298-L1344) lines 1298-1344

```ts
Deno.test(async function testImportExportEcDsaJwk() {
  const subtle = crypto.subtle;
  assert(subtle);

  for (
    const [_key, keyData] of Object.entries(jwtECKeys)
  ) {
    const { publicJWK, privateJWK, algo } = keyData;

    // 1. Test import EcDsa
    const privateKeyECDSA = await subtle.importKey(
      "jwk",
      {
        alg: algo,
        ...privateJWK,
        ext: true,
        "key_ops": ["sign"],
      },
      { name: "ECDSA", namedCurve: privateJWK.crv },
      true,
      ["sign"],
    );
    const expPrivateKeyJWK = await subtle.exportKey(
      "jwk",
      privateKeyECDSA,
    );
    assert(equalJwk(privateJWK, expPrivateKeyJWK as JWK));

    const publicKeyECDSA = await subtle.importKey(
      "jwk",
      {
        alg: algo,
        ...publicJWK,
        ext: true,
        "key_ops": ["verify"],
      },
      { name: "ECDSA", namedCurve: publicJWK.crv },
      true,
      ["verify"],
    );

    const expPublicKeyJWK = await subtle.exportKey(
      "jwk",
      publicKeyECDSA,
    );

    assert(equalJwk(publicJWK, expPublicKeyJWK as JWK));
```

<a id="ref-22"></a>
[22] [`Repo denoland/deno: tests/unit/webcrypto_test.ts`](https://github.com/denoland/deno/blob/d6212d40/tests/unit/webcrypto_test.ts#L486-L531) lines 486-531

```ts
Deno.test(async function rsaExport() {
  for (const algorithm of ["RSASSA-PKCS1-v1_5", "RSA-PSS", "RSA-OAEP"]) {
    const keyPair = await crypto.subtle.generateKey(
      {
        name: algorithm,
        modulusLength: 2048,
        publicExponent: new Uint8Array([1, 0, 1]),
        hash: "SHA-256",
      },
      true,
      algorithm !== "RSA-OAEP" ? ["sign", "verify"] : ["encrypt", "decrypt"],
    );

    assert(keyPair.privateKey);
    assert(keyPair.publicKey);
    assertEquals(keyPair.privateKey.extractable, true);

    const exportedPrivateKey = await crypto.subtle.exportKey(
      "pkcs8",
      keyPair.privateKey,
    );

    assert(exportedPrivateKey);
    assert(exportedPrivateKey instanceof ArrayBuffer);

    const pkcs8 = new Uint8Array(exportedPrivateKey);
    assert(pkcs8.length > 0);

    assertEquals(
      pkcs8.slice(4, asn1AlgorithmIdentifier.byteLength + 4),
      asn1AlgorithmIdentifier,
    );

    const exportedPublicKey = await crypto.subtle.exportKey(
      "spki",
      keyPair.publicKey,
    );

    const spki = new Uint8Array(exportedPublicKey);
    assert(spki.length > 0);

    assertEquals(
      spki.slice(4, asn1AlgorithmIdentifier.byteLength + 1),
      asn1AlgorithmIdentifier.slice(3),
    );
  }
```

<a id="ref-23"></a>
[23] [`Repo denoland/deno: tests/unit/structured_clone_test.ts`](https://github.com/denoland/deno/blob/d6212d40/tests/unit/structured_clone_test.ts#L63-L75) lines 63-75

```ts
Deno.test("structuredClone CryptoKey", async () => {
  // AES key
  const aesKey = await crypto.subtle.generateKey(
    { name: "AES-GCM", length: 256 },
    true,
    ["encrypt", "decrypt"],
  );
  const aesClone = structuredClone(aesKey);
  assert(aesKey !== aesClone);
  assertEquals(aesClone.type, aesKey.type);
  assertEquals(aesClone.extractable, aesKey.extractable);
  assertEquals(aesClone.algorithm, aesKey.algorithm);
  assertEquals([...aesClone.usages], [...aesKey.usages]);
```

<a id="ref-24"></a>
[24] [`Repo denoland/deno: tests/unit/structured_clone_test.ts`](https://github.com/denoland/deno/blob/d6212d40/tests/unit/structured_clone_test.ts#L92-L133) lines 92-133

```ts
  // Non-extractable key can be cloned
  const nonExtractable = await crypto.subtle.generateKey(
    { name: "AES-GCM", length: 256 },
    false,
    ["encrypt", "decrypt"],
  );
  const nonExtractableClone = structuredClone(nonExtractable);
  assertEquals(nonExtractableClone.extractable, false);
  assertEquals(nonExtractableClone.algorithm, nonExtractable.algorithm);

  // HMAC key
  const hmacKey = await crypto.subtle.generateKey(
    { name: "HMAC", hash: "SHA-256" },
    true,
    ["sign", "verify"],
  );
  const hmacClone = structuredClone(hmacKey);
  assertEquals(hmacClone.type, hmacKey.type);
  assertEquals(hmacClone.algorithm, hmacKey.algorithm);
  assertEquals([...hmacClone.usages], [...hmacKey.usages]);

  // EC key pair
  const ecKeyPair = await crypto.subtle.generateKey(
    { name: "ECDSA", namedCurve: "P-256" },
    true,
    ["sign", "verify"],
  ) as CryptoKeyPair;
  const ecPrivateClone = structuredClone(ecKeyPair.privateKey);
  const ecPublicClone = structuredClone(ecKeyPair.publicKey);
  assertEquals(ecPrivateClone.type, "private");
  assertEquals(ecPublicClone.type, "public");

  // Ed25519 key pair
  const edKeyPair = await crypto.subtle.generateKey(
    "Ed25519",
    true,
    ["sign", "verify"],
  ) as CryptoKeyPair;
  const edClone = structuredClone(edKeyPair.privateKey);
  assertEquals(edClone.type, "private");
  assertEquals(edClone.algorithm.name, "Ed25519");
});
```

<a id="ref-25"></a>
[25] [`Repo denoland/deno: cli/tsc/dts/lib.webworker.d.ts`](https://github.com/denoland/deno/blob/d6212d40/cli/tsc/dts/lib.webworker.d.ts#L3207-L3231) lines 3207-3231

```ts
interface CryptoKey {
    /**
     * The read-only **`algorithm`** property of the CryptoKey interface returns an object describing the algorithm for which this key can be used, and any associated extra parameters.
     *
     * [MDN Reference](https://developer.mozilla.org/docs/Web/API/CryptoKey/algorithm)
     */
    readonly algorithm: KeyAlgorithm;
    /**
     * The read-only **`extractable`** property of the CryptoKey interface indicates whether or not the key may be extracted using SubtleCrypto.exportKey() or SubtleCrypto.wrapKey().
     *
     * [MDN Reference](https://developer.mozilla.org/docs/Web/API/CryptoKey/extractable)
     */
    readonly extractable: boolean;
    /**
     * The read-only **`type`** property of the CryptoKey interface indicates which kind of key is represented by the object. It can have the following values:
     *
     * [MDN Reference](https://developer.mozilla.org/docs/Web/API/CryptoKey/type)
     */
    readonly type: KeyType;
    /**
     * The read-only **`usages`** property of the CryptoKey interface indicates what can be done with the key.
     *
     * [MDN Reference](https://developer.mozilla.org/docs/Web/API/CryptoKey/usages)
     */
    readonly usages: KeyUsage[];
```

<a id="ref-26"></a>
[26] [`Repo denoland/deno: cli/tsc/dts/lib.deno_crypto.d.ts`](https://github.com/denoland/deno/blob/d6212d40/cli/tsc/dts/lib.deno_crypto.d.ts#L206-L210) lines 206-210

```ts
interface CryptoKey {
  readonly algorithm: KeyAlgorithm;
  readonly extractable: boolean;
  readonly type: KeyType;
  readonly usages: KeyUsage[];
```
