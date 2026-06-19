# DeepWiki Response

- Repository: `denoland/deno`
- Question: For Deno's Node compatibility implementation of node:child_process, how are exec, execFile, and spawnSync option validation plus deterministic options such as input, maxBuffer, and timeout structured? Please outline relevant modules, invariants, edge cases, and test surface for command/file/args validation and sync buffered process execution.
- DeepWiki search: https://deepwiki.com/search/for-denos-node-compatibility-i_5a08fe87-3e7d-47f4-8f90-c32696d978b6

Deno's Node compatibility layer implements `child_process.exec`, `child_process.execFile`, and `child_process.spawnSync` with specific option validation and handling for deterministic options like `input`, `maxBuffer`, and `timeout` [[1](#ref-1)] [[2](#ref-2)] [[3](#ref-3)]. These functions leverage internal validation utilities and a shared `spawnSync` mechanism for synchronous buffered process execution [[1](#ref-1)] [[4](#ref-4)] [[5](#ref-5)].

## Option Validation and Structure

The validation for `exec`, `execFile`, and `spawnSync` options is primarily handled within `ext/node/polyfills/child_process.ts` and `ext/node/polyfills/internal/child_process.ts` [[6](#ref-6)] [[7](#ref-7)] [[8](#ref-8)].

### `spawnSync` Options

The `spawnSync` function in `ext/node/polyfills/child_process.ts` explicitly validates `timeout` and `maxBuffer` [[9](#ref-9)].
*   `timeout`: Validated using `validateTimeout`, which ensures it's an integer greater than or equal to 0 [[10](#ref-10)].
*   `maxBuffer`: Validated using `validateMaxBuffer`, ensuring it's a number greater than or equal to 0 [[11](#ref-11)]. The default `maxBuffer` is `1024 * 1024` (1MB) [[12](#ref-12)] [[13](#ref-13)].
*   `killSignal`: Sanitized by `sanitizeKillSignal` to convert string or number signals to a valid signal name [[14](#ref-14)].

The `spawnSync` implementation in `ext/node/polyfills/internal/child_process.ts` further processes these options, including `input`, `cwd`, `env`, `uid`, `gid`, and `windowsVerbatimArguments` [[15](#ref-15)]. The `input` option is normalized to a `Buffer` if provided as a string, `TypedArray`, or `DataView` [[16](#ref-16)].

### `exec` and `execFile` Options

`exec` and `execFile` internally normalize their arguments and options before calling `spawnSync` [[17](#ref-17)] [[18](#ref-18)].
*   `execFile` sets default values for `encoding`, `timeout`, `maxBuffer`, `killSignal`, and `shell` [[19](#ref-19)]. It then validates `timeout` and `maxBuffer` [[8](#ref-8)].
*   `execSync` and `execFileSync` use `normalizeExecArgs` and `normalizeExecFileArgs` respectively to parse arguments and options [[20](#ref-20)] [[21](#ref-21)].

## Command/File/Args Validation

Command, file, and argument validation involves several steps:
*   `fork` validates `modulePath` and `options.execPath` as strings and checks for null bytes [[22](#ref-22)] [[23](#ref-23)]. It also validates `args` and `options.execArgv` for null bytes [[24](#ref-24)] [[25](#ref-25)].
*   `spawn` uses `normalizeSpawnArguments` to process command and arguments [[26](#ref-26)].
*   `normalizeSpawnArguments` (in `ext/node/polyfills/internal/child_process.ts`) validates `options.shell`, `options.argv0`, `options.windowsHide`, `options.windowsVerbatimArguments`, and `options.serialization` [[27](#ref-27)]. It also handles escaping arguments if `options.shell` is true [[28](#ref-28)].
*   The `ChildProcess` class's `spawn` method validates `options.envPairs`, `options.args`, and `options.file` [[29](#ref-29)].

## Sync Buffered Process Execution

Synchronous buffered process execution is handled by `spawnSync` in `ext/node/polyfills/internal/child_process.ts` [[30](#ref-30)]. This function calls `nodeSpawnSyncChild` (from `ext/process/40_process.js`) which in turn invokes the Rust `op_spawn_sync` [[31](#ref-31)] [[32](#ref-32)] [[33](#ref-33)].

*   `nodeSpawnSyncChild` prepares the arguments for `op_spawn_sync`, including `cmd`, `args`, `cwd`, `env`, `uid`, `gid`, `stdin`, `stdout`, `stderr`, `extraStdio`, `windowsRawArguments`, `needsNpmProcessState`, `input`, `timeout`, and `killSignal` [[34](#ref-34)].
*   After `op_spawn_sync` returns, `spawnSync` processes the output, including `stdout`, `stderr`, `status`, `signal`, `pid`, and `killedByTimeout` [[35](#ref-35)].
*   It checks if `stdout` or `stderr` exceed `maxBuffer` and throws an `ENOBUFS` error if they do [[36](#ref-36)].
*   If the process was killed by a timeout, it sets the `error` to `ETIMEDOUT` [[37](#ref-37)].
*   Errors during execution, such as command not found, are caught and converted to Node.js-compatible errors like `ENOENT` [[38](#ref-38)].

## Test Surface

The Node.js compatibility layer is tested using both Deno-specific unit tests and official Node.js compatibility tests [[39](#ref-39)].
*   Unit tests in `tests/unit_node/child_process_test.ts` cover scenarios like exceeding `maxBuffer` limits [[40](#ref-40)] and `input` option support for `spawnSync` [[41](#ref-41)].
*   The `tests/node_compat/config.jsonc` file lists numerous Node.js official tests related to `child_process`, including tests for `exec`, `execFile`, and `spawnSync` options like `maxBuffer`, `timeout`, `input`, and `killSignal` [[42](#ref-42)] [[43](#ref-43)]. These tests help ensure compliance with Node.js behavior.

## Invariants and Edge Cases

*   **Null Bytes**: Arguments and paths are validated to ensure they do not contain null bytes, which could lead to security vulnerabilities [[44](#ref-44)] [[45](#ref-45)] [[46](#ref-46)] [[47](#ref-47)].
*   **`maxBuffer` Exceeded**: If the output (stdout or stderr) exceeds `maxBuffer`, the process is terminated, and an `ERR_CHILD_PROCESS_STDIO_MAXBUFFER` error (mapped to `ENOBUFS` internally) is thrown [[36](#ref-36)] [[48](#ref-48)].
*   **Timeout**: If a `timeout` is specified and the process does not exit within that time, it is killed with `killSignal` (defaulting to `SIGTERM`), and an `ETIMEDOUT` error is reported [[49](#ref-49)] [[37](#ref-37)].
*   **`shell` Option**: When `options.shell` is true and arguments are provided, a deprecation warning is emitted because arguments are concatenated rather than escaped, posing a security risk [[50](#ref-50)].
*   **`input` Type Validation**: The `input` option for `spawnSync` must be a string, `Buffer`, `TypedArray`, or `DataView`; otherwise, an `ERR_INVALID_ARG_TYPE` is thrown [[51](#ref-51)] [[52](#ref-52)].

## Relevant Modules

*   `ext/node/polyfills/child_process.ts`: Contains the main `exec`, `execFile`, `spawn`, and `spawnSync` functions, including argument parsing and initial option validation [[53](#ref-53)] [[54](#ref-54)] [[55](#ref-55)] [[19](#ref-19)].
*   `ext/node/polyfills/internal/child_process.ts`: Provides internal helper functions, the `ChildProcess` class, and the core `spawnSync` implementation that interacts with Deno's Rust ops [[56](#ref-56)] [[30](#ref-30)]. It also includes `normalizeSpawnArguments` for detailed option validation [[57](#ref-57)].
*   `ext/process/40_process.js`: Contains `nodeSpawnSyncChild`, which acts as a bridge between the JavaScript `spawnSync` implementation and the underlying Rust `op_spawn_sync` [[32](#ref-32)].
*   `ext:deno_node/internal/validators.mjs`: Provides utility functions like `validateInteger`, `validateNumber`, and `validateString` used for option validation [[58](#ref-58)].

## Notes
The `Child Process and IPC` wiki page provides a

Wiki pages you might want to explore:
- [Node.js Compatibility Layer (denoland/deno)](/wiki/denoland/deno#7)
- [Child Process and IPC (denoland/deno)](/wiki/denoland/deno#7.9)

## References

<a id="ref-1"></a>
[1] [`Repo denoland/deno: ext/node/polyfills/child_process.ts`](https://github.com/denoland/deno/blob/d6212d40/ext/node/polyfills/child_process.ts#L300-L325) lines 300-325

```ts
export function spawnSync(
  command: string,
  argsOrOptions?: string[] | SpawnSyncOptions,
  maybeOptions?: SpawnSyncOptions,
): SpawnSyncResult {
  const args = Array.isArray(argsOrOptions) ? argsOrOptions : [];
  let options = !Array.isArray(argsOrOptions) && argsOrOptions
    ? argsOrOptions
    : maybeOptions as SpawnSyncOptions;

  options = {
    __proto__: null,
    maxBuffer: MAX_BUFFER,
    ...normalizeSpawnArguments(command, args, options),
  } as typeof options;

  // Validate the timeout, if present.
  validateTimeout(options.timeout);

  // Validate maxBuffer, if present.
  validateMaxBuffer(options.maxBuffer);

  // Validate and translate the kill signal, if present.
  options.killSignal = sanitizeKillSignal(options.killSignal);

  return internalChildProcess.spawnSync(options);
```

<a id="ref-2"></a>
[2] [`Repo denoland/deno: ext/node/polyfills/child_process.ts`](https://github.com/denoland/deno/blob/d6212d40/ext/node/polyfills/child_process.ts#L539-L555) lines 539-555

```ts
  const execOptions = {
    __proto__: null,
    encoding: "utf8",
    timeout: 0,
    maxBuffer: MAX_BUFFER,
    killSignal: "SIGTERM",
    shell: false,
    ...options,
  };
  validateTimeout(execOptions.timeout);
  if (execOptions.maxBuffer < 0) {
    throw new ERR_OUT_OF_RANGE(
      "options.maxBuffer",
      "a positive number",
      execOptions.maxBuffer,
    );
  }
```

<a id="ref-3"></a>
[3] [`Repo denoland/deno: ext/node/polyfills/internal/child_process.ts`](https://github.com/denoland/deno/blob/d6212d40/ext/node/polyfills/internal/child_process.ts#L1579-L1592) lines 1579-1592

```ts
function spawnSync(
  options,
) {
  const {
    env = Deno.env.toObject(),
    input,
    stdio = ["pipe", "pipe", "pipe"],
    cwd,
    encoding,
    uid,
    gid,
    maxBuffer,
    timeout,
    killSignal,
```

<a id="ref-4"></a>
[4] [`Repo denoland/deno: ext/node/polyfills/child_process.ts`](https://github.com/denoland/deno/blob/d6212d40/ext/node/polyfills/child_process.ts#L823-L827) lines 823-827

```ts
export function execSync(command: string, options: ExecSyncOptions) {
  const opts = normalizeExecArgs(command, options);
  const inheritStderr = !(opts.options as ExecSyncOptions).stdio;

  const ret = spawnSync(opts.file, opts.options as SpawnSyncOptions);
```

<a id="ref-5"></a>
[5] [`Repo denoland/deno: ext/node/polyfills/child_process.ts`](https://github.com/denoland/deno/blob/d6212d40/ext/node/polyfills/child_process.ts#L911-L913) lines 911-913

```ts

  const inheritStderr = !options.stdio;
  const ret = spawnSync(file, args, options);
```

<a id="ref-6"></a>
[6] [`Repo denoland/deno: ext/node/polyfills/child_process.ts`](https://github.com/denoland/deno/blob/d6212d40/ext/node/polyfills/child_process.ts#L276-L298) lines 276-298

```ts
function validateTimeout(timeout?: number) {
  if (timeout != null) {
    validateInteger(timeout, "timeout", 0);
  }
}

function validateMaxBuffer(maxBuffer?: number) {
  if (maxBuffer != null) {
    validateNumber(maxBuffer, "options.maxBuffer", 0);
  }
}

function sanitizeKillSignal(killSignal?: string | number) {
  if (typeof killSignal === "string" || typeof killSignal === "number") {
    return convertToValidSignal(killSignal);
  } else if (killSignal != null) {
    throw new ERR_INVALID_ARG_TYPE(
      "options.killSignal",
      ["string", "number"],
      killSignal,
    );
  }
}
```

<a id="ref-7"></a>
[7] [`Repo denoland/deno: ext/node/polyfills/child_process.ts`](https://github.com/denoland/deno/blob/d6212d40/ext/node/polyfills/child_process.ts#L316-L324) lines 316-324

```ts
  // Validate the timeout, if present.
  validateTimeout(options.timeout);

  // Validate maxBuffer, if present.
  validateMaxBuffer(options.maxBuffer);

  // Validate and translate the kill signal, if present.
  options.killSignal = sanitizeKillSignal(options.killSignal);

```

<a id="ref-8"></a>
[8] [`Repo denoland/deno: ext/node/polyfills/child_process.ts`](https://github.com/denoland/deno/blob/d6212d40/ext/node/polyfills/child_process.ts#L548-L555) lines 548-555

```ts
  validateTimeout(execOptions.timeout);
  if (execOptions.maxBuffer < 0) {
    throw new ERR_OUT_OF_RANGE(
      "options.maxBuffer",
      "a positive number",
      execOptions.maxBuffer,
    );
  }
```

<a id="ref-9"></a>
[9] [`Repo denoland/deno: ext/node/polyfills/child_process.ts`](https://github.com/denoland/deno/blob/d6212d40/ext/node/polyfills/child_process.ts#L316-L320) lines 316-320

```ts
  // Validate the timeout, if present.
  validateTimeout(options.timeout);

  // Validate maxBuffer, if present.
  validateMaxBuffer(options.maxBuffer);
```

<a id="ref-10"></a>
[10] [`Repo denoland/deno: ext/node/polyfills/child_process.ts`](https://github.com/denoland/deno/blob/d6212d40/ext/node/polyfills/child_process.ts#L276-L279) lines 276-279

```ts
function validateTimeout(timeout?: number) {
  if (timeout != null) {
    validateInteger(timeout, "timeout", 0);
  }
```

<a id="ref-11"></a>
[11] [`Repo denoland/deno: ext/node/polyfills/child_process.ts`](https://github.com/denoland/deno/blob/d6212d40/ext/node/polyfills/child_process.ts#L282-L285) lines 282-285

```ts
function validateMaxBuffer(maxBuffer?: number) {
  if (maxBuffer != null) {
    validateNumber(maxBuffer, "options.maxBuffer", 0);
  }
```

<a id="ref-12"></a>
[12] [`Repo denoland/deno: ext/node/polyfills/child_process.ts`](https://github.com/denoland/deno/blob/d6212d40/ext/node/polyfills/child_process.ts#L64-L64) lines 64-64

```ts
const MAX_BUFFER = 1024 * 1024;
```

<a id="ref-13"></a>
[13] [`Repo denoland/deno: ext/node/polyfills/child_process.ts`](https://github.com/denoland/deno/blob/d6212d40/ext/node/polyfills/child_process.ts#L312-L312) lines 312-312

```ts
    maxBuffer: MAX_BUFFER,
```

<a id="ref-14"></a>
[14] [`Repo denoland/deno: ext/node/polyfills/child_process.ts`](https://github.com/denoland/deno/blob/d6212d40/ext/node/polyfills/child_process.ts#L288-L298) lines 288-298

```ts
function sanitizeKillSignal(killSignal?: string | number) {
  if (typeof killSignal === "string" || typeof killSignal === "number") {
    return convertToValidSignal(killSignal);
  } else if (killSignal != null) {
    throw new ERR_INVALID_ARG_TYPE(
      "options.killSignal",
      ["string", "number"],
      killSignal,
    );
  }
}
```

<a id="ref-15"></a>
[15] [`Repo denoland/deno: ext/node/polyfills/internal/child_process.ts`](https://github.com/denoland/deno/blob/d6212d40/ext/node/polyfills/internal/child_process.ts#L1582-L1594) lines 1582-1594

```ts
  const {
    env = Deno.env.toObject(),
    input,
    stdio = ["pipe", "pipe", "pipe"],
    cwd,
    encoding,
    uid,
    gid,
    maxBuffer,
    timeout,
    killSignal,
    windowsVerbatimArguments = false,
  } = options;
```

<a id="ref-16"></a>
[16] [`Repo denoland/deno: ext/node/polyfills/internal/child_process.ts`](https://github.com/denoland/deno/blob/d6212d40/ext/node/polyfills/internal/child_process.ts#L1561-L1577) lines 1561-1577

```ts
function normalizeInput(input) {
  if (input == null) {
    return null;
  }
  if (typeof input === "string") {
    return Buffer.from(input);
  }
  if (ArrayBuffer.isView(input)) {
    return Buffer.from(input.buffer, input.byteOffset, input.byteLength);
  }
  throw new ERR_INVALID_ARG_TYPE("input", [
    "string",
    "Buffer",
    "TypedArray",
    "DataView",
  ], input);
}
```

<a id="ref-17"></a>
[17] [`Repo denoland/deno: ext/node/polyfills/child_process.ts`](https://github.com/denoland/deno/blob/d6212d40/ext/node/polyfills/child_process.ts#L358-L381) lines 358-381

```ts
function normalizeExecArgs(
  command: string,
  optionsOrCallback?: ExecOptions | ExecSyncOptions | ExecCallback,
  maybeCallback?: ExecCallback,
) {
  let callback: ExecFileCallback | undefined = maybeCallback;

  if (typeof optionsOrCallback === "function") {
    callback = optionsOrCallback;
    optionsOrCallback = undefined;
  }

  // Make a shallow copy so we don't clobber the user's options object.
  const options: ExecOptions | ExecSyncOptions = {
    __proto__: null,
    ...optionsOrCallback,
  } as ExecOptions | ExecSyncOptions;
  options.shell = typeof options.shell === "string" ? options.shell : true;

  return {
    file: command,
    options: options!,
    callback: callback!,
  };
```

<a id="ref-18"></a>
[18] [`Repo denoland/deno: ext/node/polyfills/child_process.ts`](https://github.com/denoland/deno/blob/d6212d40/ext/node/polyfills/child_process.ts#L842-L892) lines 842-892

```ts
function normalizeExecFileArgs(
  file: string,
  args?: string[] | null | ExecFileSyncOptions | ExecFileCallback,
  options?: ExecFileSyncOptions | null | ExecFileCallback,
  callback?: ExecFileCallback,
): {
  file: string;
  args: string[];
  options: ExecFileSyncOptions;
  callback?: ExecFileCallback;
} {
  if (ArrayIsArray(args)) {
    args = ArrayPrototypeSlice(args);
  } else if (args != null && typeof args === "object") {
    callback = options as ExecFileCallback;
    options = args as ExecFileSyncOptions;
    args = null;
  } else if (typeof args === "function") {
    callback = args;
    options = null;
    args = null;
  }

  if (args == null) {
    args = [];
  }

  if (typeof options === "function") {
    callback = options as ExecFileCallback;
  } else if (options != null) {
    validateObject(options, "options");
  }

  if (options == null) {
    options = kEmptyObject;
  }

  args = args as string[];
  options = options as ExecFileSyncOptions;

  if (callback != null) {
    validateFunction(callback, "callback");
  }

  // Validate argv0, if present.
  if (options.argv0 != null) {
    validateString(options.argv0, "options.argv0");
  }

  return { file, args, options, callback };
}
```

<a id="ref-19"></a>
[19] [`Repo denoland/deno: ext/node/polyfills/child_process.ts`](https://github.com/denoland/deno/blob/d6212d40/ext/node/polyfills/child_process.ts#L539-L547) lines 539-547

```ts
  const execOptions = {
    __proto__: null,
    encoding: "utf8",
    timeout: 0,
    maxBuffer: MAX_BUFFER,
    killSignal: "SIGTERM",
    shell: false,
    ...options,
  };
```

<a id="ref-20"></a>
[20] [`Repo denoland/deno: ext/node/polyfills/child_process.ts`](https://github.com/denoland/deno/blob/d6212d40/ext/node/polyfills/child_process.ts#L823-L824) lines 823-824

```ts
export function execSync(command: string, options: ExecSyncOptions) {
  const opts = normalizeExecArgs(command, options);
```

<a id="ref-21"></a>
[21] [`Repo denoland/deno: ext/node/polyfills/child_process.ts`](https://github.com/denoland/deno/blob/d6212d40/ext/node/polyfills/child_process.ts#L911-L911) lines 911-911

<a id="ref-22"></a>
[22] [`Repo denoland/deno: ext/node/polyfills/child_process.ts`](https://github.com/denoland/deno/blob/d6212d40/ext/node/polyfills/child_process.ts#L81-L82) lines 81-82

```ts
  validateString(modulePath, "modulePath");
  validateNullByteNotInArg(modulePath, "modulePath");
```

<a id="ref-23"></a>
[23] [`Repo denoland/deno: ext/node/polyfills/child_process.ts`](https://github.com/denoland/deno/blob/d6212d40/ext/node/polyfills/child_process.ts#L121-L123) lines 121-123

```ts
  if (options.execPath != null) {
    validateString(options.execPath, "options.execPath");
    validateNullByteNotInArg(options.execPath, "options.execPath");
```

<a id="ref-24"></a>
[24] [`Repo denoland/deno: ext/node/polyfills/child_process.ts`](https://github.com/denoland/deno/blob/d6212d40/ext/node/polyfills/child_process.ts#L113-L118) lines 113-118

```ts
  // Validate null bytes in args
  for (let i = 0; i < args.length; i++) {
    if (typeof args[i] === "string") {
      validateNullByteNotInArg(args[i], `args[${i}]`);
    }
  }
```

<a id="ref-25"></a>
[25] [`Repo denoland/deno: ext/node/polyfills/child_process.ts`](https://github.com/denoland/deno/blob/d6212d40/ext/node/polyfills/child_process.ts#L127-L132) lines 127-132

```ts
  if (options.execArgv != null && Array.isArray(options.execArgv)) {
    for (let i = 0; i < options.execArgv.length; i++) {
      if (typeof options.execArgv[i] === "string") {
        validateNullByteNotInArg(options.execArgv[i], `options.execArgv[${i}]`);
      }
    }
```

<a id="ref-26"></a>
[26] [`Repo denoland/deno: ext/node/polyfills/child_process.ts`](https://github.com/denoland/deno/blob/d6212d40/ext/node/polyfills/child_process.ts#L249-L249) lines 249-249

```ts
  options = normalizeSpawnArguments(command, args, options);
```

<a id="ref-27"></a>
[27] [`Repo denoland/deno: ext/node/polyfills/internal/child_process.ts`](https://github.com/denoland/deno/blob/d6212d40/ext/node/polyfills/internal/child_process.ts#L1036-L1075) lines 1036-1075

```ts
  if (
    options.shell != null &&
    typeof options.shell !== "boolean" &&
    typeof options.shell !== "string"
  ) {
    throw new ERR_INVALID_ARG_TYPE(
      "options.shell",
      ["boolean", "string"],
      options.shell,
    );
  }
  if (typeof options.shell === "string") {
    validateNullByteNotInArg(options.shell, "options.shell");
  }

  // Validate argv0, if present.
  if (options.argv0 != null) {
    validateString(options.argv0, "options.argv0");
    validateNullByteNotInArg(options.argv0, "options.argv0");
  }

  // Validate windowsHide, if present.
  if (options.windowsHide != null) {
    validateBoolean(options.windowsHide, "options.windowsHide");
  }

  // Validate windowsVerbatimArguments, if present.
  let { windowsVerbatimArguments } = options;
  if (windowsVerbatimArguments != null) {
    validateBoolean(
      windowsVerbatimArguments,
      "options.windowsVerbatimArguments",
    );
  }

  validateOneOf(options.serialization, "options.serialization", [
    undefined,
    "json",
    "advanced",
  ]);
```

<a id="ref-28"></a>
[28] [`Repo denoland/deno: ext/node/polyfills/internal/child_process.ts`](https://github.com/denoland/deno/blob/d6212d40/ext/node/polyfills/internal/child_process.ts#L1077-L1093) lines 1077-1093

```ts
  if (options.shell) {
    // When args are provided, escape them to prevent shell injection.
    // When no args are provided (just a string command), the user intends
    // for shell interpretation, so don't escape.
    let command;
    if (args.length > 0) {
      if (!emittedShellDeprecation) {
        process.emitWarning(
          "Passing args to a child process with shell option true can lead to security " +
            "vulnerabilities, as the arguments are not escaped, only concatenated.",
          "DeprecationWarning",
          "DEP0190",
        );
        emittedShellDeprecation = true;
      }
      const escapedParts = [escapeShellArg(file), ...args.map(escapeShellArg)];
      command = ArrayPrototypeJoin(escapedParts, " ");
```

<a id="ref-29"></a>
[29] [`Repo denoland/deno: ext/node/polyfills/internal/child_process.ts`](https://github.com/denoland/deno/blob/d6212d40/ext/node/polyfills/internal/child_process.ts#L249-L269) lines 249-269

```ts
    if (options == null || typeof options !== "object") {
      throw new ERR_INVALID_ARG_TYPE("options", "object", options);
    }

    // Validate envPairs before file (Node.js validation order)
    const { envPairs } = options;
    if (envPairs !== undefined && !ArrayIsArray(envPairs)) {
      throw new ERR_INVALID_ARG_TYPE("options.envPairs", "Array", envPairs);
    }

    // Validate args
    const { args } = options;
    if (args !== undefined && !ArrayIsArray(args)) {
      throw new ERR_INVALID_ARG_TYPE("options.args", "Array", args);
    }

    // Validate file
    const { file } = options;
    if (file == null || typeof file !== "string") {
      throw new ERR_INVALID_ARG_TYPE("options.file", "string", file);
    }
```

<a id="ref-30"></a>
[30] [`Repo denoland/deno: ext/node/polyfills/internal/child_process.ts`](https://github.com/denoland/deno/blob/d6212d40/ext/node/polyfills/internal/child_process.ts#L1579-L1690) lines 1579-1690

```ts
function spawnSync(
  options,
) {
  const {
    env = Deno.env.toObject(),
    input,
    stdio = ["pipe", "pipe", "pipe"],
    cwd,
    encoding,
    uid,
    gid,
    maxBuffer,
    timeout,
    killSignal,
    windowsVerbatimArguments = false,
  } = options;
  let command = options.file || "";
  let args = options.args || [];
  const [
    stdin_ = "pipe",
    stdout_ = "pipe",
    stderr_ = "pipe",
    ...extraStdio_
  ] = normalizeStdioOption(stdio);

  const extraStdioNormalized = [];
  for (let i = 0; i < extraStdio_.length; i++) {
    const val = extraStdio_[i];
    const fd = i + 3; // extra stdio starts at FD 3
    // null/undefined means "don't pass this fd"
    if (val == null) {
      extraStdioNormalized.push("null");
    } else if (val === "inherit") {
      // "inherit" for extra FDs means pass the parent's FD at this index
      extraStdioNormalized.push(fd);
    } else {
      extraStdioNormalized.push(toDenoStdio(val));
    }
  }

  let includeNpmProcessState = false;
  // args[0] is argv0 (prepended by normalizeSpawnArguments). Capture it
  // before slicing so we can pass it via kArgv0 for OS-level argv[0].
  const argv0 = args && args.length > 0 ? args[0] : command;
  const argsToProcess = args && args.length > 0 ? args.slice(1) : [];
  [command, args, includeNpmProcessState] = buildCommand(
    command,
    argsToProcess,
    env,
  );
  const input_ = normalizeInput(input);

  const result = {};
  try {
    const output = nodeSpawnSyncChild({
      args: [command, ...args],
      cwd,
      env: mapValues(env, (value) => value.toString()),
      argv0: argv0 !== command ? argv0 : undefined,
      stdout: toDenoStdio(stdout_),
      stderr: toDenoStdio(stderr_),
      stdin: stdin_ == "inherit" ? "inherit" : "null",
      uid,
      gid,
      clearEnv: false,
      extraStdio: extraStdioNormalized,
      windowsRawArguments: windowsVerbatimArguments,
      needsNpmProcessState: options[kNeedsNpmProcessState] ||
        includeNpmProcessState,
      input: input_,
      timeout,
      killSignal,
    });

    const status = output.signal ? null : output.code;
    let stdout = output.stdout ? Buffer.from(output.stdout) : null;
    let stderr = output.stderr ? Buffer.from(output.stderr) : null;

    if (
      (stdout && stdout.length > maxBuffer) ||
      (stderr && stderr.length > maxBuffer)
    ) {
      result.error = _createSpawnError("ENOBUFS", command, args, true);
    }

    if (output.killedByTimeout) {
      result.error = _createSpawnError("ETIMEDOUT", command, args, true);
    }

    if (encoding && encoding !== "buffer") {
      stdout = stdout && stdout.toString(encoding);
      stderr = stderr && stderr.toString(encoding);
    }

    result.pid = output.pid;
    // When killed by timeout, report the killSignal (matching Node.js behavior).
    // On Windows there are no real Unix signals, but Node still reports the
    // configured killSignal so callers can detect the timeout.
    result.status = output.killedByTimeout ? null : status;
    result.signal = output.killedByTimeout
      ? _resolveKillSignalName(killSignal)
      : output.signal;
    result.stdout = stdout;
    result.stderr = stderr;
    result.output = [output.signal, stdout, stderr];
  } catch (err) {
    if (err instanceof Deno.errors.NotFound) {
      result.error = _createSpawnError("ENOENT", command, args, true);
    }
  }
  return result;
}
```

<a id="ref-31"></a>
[31] [`Repo denoland/deno: ext/node/polyfills/internal/child_process.ts`](https://github.com/denoland/deno/blob/d6212d40/ext/node/polyfills/internal/child_process.ts#L1633-L1633) lines 1633-1633

```ts
    const output = nodeSpawnSyncChild({
```

<a id="ref-32"></a>
[32] [`Repo denoland/deno: ext/process/40_process.js`](https://github.com/denoland/deno/blob/d6212d40/ext/process/40_process.js#L358-L375) lines 358-375

```js
function nodeSpawnSyncChild({
  args,
  cwd,
  clearEnv,
  argv0,
  env,
  uid,
  gid,
  stdin,
  stdout,
  stderr,
  extraStdio = [],
  windowsRawArguments,
  needsNpmProcessState,
  input,
  timeout,
  killSignal,
}) {
```

<a id="ref-33"></a>
[33] [`Repo denoland/deno: ext/process/40_process.js`](https://github.com/denoland/deno/blob/d6212d40/ext/process/40_process.js#L400-L400) lines 400-400

```js
  const result = op_spawn_sync(spawnArgs);
```

<a id="ref-34"></a>
[34] [`Repo denoland/deno: ext/process/40_process.js`](https://github.com/denoland/deno/blob/d6212d40/ext/process/40_process.js#L377-L393) lines 377-393

```js
    cmd: pathFromURL(args[0]),
    args: ArrayPrototypeMap(ArrayPrototypeSlice(args, 1), String),
    cwd: pathFromURL(cwd),
    clearEnv,
    env: ObjectEntries(env),
    uid,
    gid,
    stdin,
    stdout,
    stderr,
    windowsRawArguments,
    extraStdio,
    detached: false,
    needsNpmProcessState,
    input,
    argv0,
  };
```

<a id="ref-35"></a>
[35] [`Repo denoland/deno: ext/node/polyfills/internal/child_process.ts`](https://github.com/denoland/deno/blob/d6212d40/ext/node/polyfills/internal/child_process.ts#L1652-L1683) lines 1652-1683

```ts

    const status = output.signal ? null : output.code;
    let stdout = output.stdout ? Buffer.from(output.stdout) : null;
    let stderr = output.stderr ? Buffer.from(output.stderr) : null;

    if (
      (stdout && stdout.length > maxBuffer) ||
      (stderr && stderr.length > maxBuffer)
    ) {
      result.error = _createSpawnError("ENOBUFS", command, args, true);
    }

    if (output.killedByTimeout) {
      result.error = _createSpawnError("ETIMEDOUT", command, args, true);
    }

    if (encoding && encoding !== "buffer") {
      stdout = stdout && stdout.toString(encoding);
      stderr = stderr && stderr.toString(encoding);
    }

    result.pid = output.pid;
    // When killed by timeout, report the killSignal (matching Node.js behavior).
    // On Windows there are no real Unix signals, but Node still reports the
    // configured killSignal so callers can detect the timeout.
    result.status = output.killedByTimeout ? null : status;
    result.signal = output.killedByTimeout
      ? _resolveKillSignalName(killSignal)
      : output.signal;
    result.stdout = stdout;
    result.stderr = stderr;
    result.output = [output.signal, stdout, stderr];
```

<a id="ref-36"></a>
[36] [`Repo denoland/deno: ext/node/polyfills/internal/child_process.ts`](https://github.com/denoland/deno/blob/d6212d40/ext/node/polyfills/internal/child_process.ts#L1657-L1662) lines 1657-1662

```ts
    if (
      (stdout && stdout.length > maxBuffer) ||
      (stderr && stderr.length > maxBuffer)
    ) {
      result.error = _createSpawnError("ENOBUFS", command, args, true);
    }
```

<a id="ref-37"></a>
[37] [`Repo denoland/deno: ext/node/polyfills/internal/child_process.ts`](https://github.com/denoland/deno/blob/d6212d40/ext/node/polyfills/internal/child_process.ts#L1664-L1666) lines 1664-1666

```ts
    if (output.killedByTimeout) {
      result.error = _createSpawnError("ETIMEDOUT", command, args, true);
    }
```

<a id="ref-38"></a>
[38] [`Repo denoland/deno: ext/node/polyfills/internal/child_process.ts`](https://github.com/denoland/deno/blob/d6212d40/ext/node/polyfills/internal/child_process.ts#L1685-L1687) lines 1685-1687

```ts
    if (err instanceof Deno.errors.NotFound) {
      result.error = _createSpawnError("ENOENT", command, args, true);
    }
```

<a id="ref-39"></a>
[39] [`Repo denoland/deno: wiki/Node.js Compatibility Layer`](https://github.com/denoland/deno/blob/d6212d40/wiki/Node.js Compatibility Layer#L190-L190) lines 190-190

<a id="ref-40"></a>
[40] [`Repo denoland/deno: tests/unit_node/child_process_test.ts`](https://github.com/denoland/deno/blob/d6212d40/tests/unit_node/child_process_test.ts#L454-L492) lines 454-492

```ts
Deno.test({
  name: "[node/child_process execFile] Exceed given maxBuffer limit",
  async fn() {
    let child: unknown;
    const script = path.join(
      path.dirname(path.fromFileUrl(import.meta.url)),
      "./testdata/exec_file_text_error.js",
    );
    const promise = new Promise<
      { err: Error | null; stderr?: string | Buffer }
    >((resolve) => {
      child = execFile(Deno.execPath(), ["run", script], {
        encoding: "buffer",
        maxBuffer: 3,
      }, (err, _, stderr) => {
        resolve({ err, stderr });
      });
    });
    try {
      const { err, stderr } = await promise;
      if (child instanceof ChildProcess) {
        assert(err);
        assertEquals(
          // deno-lint-ignore no-explicit-any
          (err as any).code,
          "ERR_CHILD_PROCESS_STDIO_MAXBUFFER",
        );
        assertEquals(err.message, "stderr maxBuffer length exceeded");
        assertEquals((stderr as Buffer).toString("utf8"), "yik");
      } else {
        throw err;
      }
    } finally {
      if (child instanceof ChildProcess) {
        child.kill();
      }
    }
  },
});
```

<a id="ref-41"></a>
[41] [`Repo denoland/deno: tests/unit_node/child_process_test.ts`](https://github.com/denoland/deno/blob/d6212d40/tests/unit_node/child_process_test.ts#L1222-L1264) lines 1222-1264

```ts
Deno.test({
  name: "[node/child_process] spawnSync supports input option",
  fn() {
    const text = "  console.log('hello')";
    const expected = `console.log("hello");\n`;
    {
      const { stdout } = spawnSync(Deno.execPath(), ["fmt", "-"], {
        input: text,
      });
      assertEquals(stdout.toString(), expected);
    }
    {
      const { stdout } = spawnSync(Deno.execPath(), ["fmt", "-"], {
        input: Buffer.from(text),
      });
      assertEquals(stdout.toString(), expected);
    }
    {
      const { stdout } = spawnSync(Deno.execPath(), ["fmt", "-"], {
        input: new TextEncoder().encode(text),
      });
      assertEquals(stdout.toString(), expected);
    }
    {
      const b = Buffer.from(text);
      const { stdout } = spawnSync(Deno.execPath(), ["fmt", "-"], {
        input: new DataView(b.buffer, b.byteOffset, b.byteLength),
      });
      assertEquals(stdout.toString(), expected);
    }

    assertThrows(
      () => {
        spawnSync(Deno.execPath(), ["fmt", "-"], {
          // deno-lint-ignore no-explicit-any
          input: {} as any,
        });
      },
      Error,
      'The "input" argument must be of type string or an instance of Buffer, TypedArray, or DataView. Received an instance of Object',
    );
  },
});
```

<a id="ref-42"></a>
[42] [`Repo denoland/deno: tests/node_compat/config.jsonc`](https://github.com/denoland/deno/blob/d6212d40/tests/node_compat/config.jsonc#L309-L318) lines 309-318

```jsonc
    "parallel/test-child-process-exec-maxbuf.js": {},
    "parallel/test-child-process-exec-std-encoding.js": {},
    "parallel/test-child-process-exec-stdout-stderr-data-string.js": {},
    "parallel/test-child-process-exec-timeout-expire.js": {},
    "parallel/test-child-process-exec-timeout-kill.js": {},
    "parallel/test-child-process-exec-timeout-not-expired.js": {},
    "parallel/test-child-process-execFile-promisified-abortController.js": {},
    "parallel/test-child-process-execfile-maxbuf.js": {},
    "parallel/test-child-process-execfilesync-maxbuf.js": {},
    "parallel/test-child-process-execsync-maxbuf.js": {},
```

<a id="ref-43"></a>
[43] [`Repo denoland/deno: tests/node_compat/config.jsonc`](https://github.com/denoland/deno/blob/d6212d40/tests/node_compat/config.jsonc#L377-L387) lines 377-387

```jsonc
    "parallel/test-child-process-spawnsync-args.js": {},
    "parallel/test-child-process-spawnsync-env.js": {},
    "parallel/test-child-process-spawnsync-input.js": {},
    "parallel/test-child-process-spawnsync-kill-signal.js": {},
    "parallel/test-child-process-spawnsync-maxbuf.js": {},
    "parallel/test-child-process-spawnsync-shell.js": {
      "ignore": true,
      "reason": "Requires monkey-patching internal/child_process.spawnSync, which doesn't work with ESM static bindings"
    },
    "parallel/test-child-process-spawnsync-timeout.js": {},
    "parallel/test-child-process-spawnsync-validation-errors.js": {},
```

<a id="ref-44"></a>
[44] [`Repo denoland/deno: ext/node/polyfills/child_process.ts`](https://github.com/denoland/deno/blob/d6212d40/ext/node/polyfills/child_process.ts#L82-L82) lines 82-82

```ts
  validateNullByteNotInArg(modulePath, "modulePath");
```

<a id="ref-45"></a>
[45] [`Repo denoland/deno: ext/node/polyfills/child_process.ts`](https://github.com/denoland/deno/blob/d6212d40/ext/node/polyfills/child_process.ts#L116-L116) lines 116-116

```ts
      validateNullByteNotInArg(args[i], `args[${i}]`);
```

<a id="ref-46"></a>
[46] [`Repo denoland/deno: ext/node/polyfills/child_process.ts`](https://github.com/denoland/deno/blob/d6212d40/ext/node/polyfills/child_process.ts#L123-L123) lines 123-123

```ts
    validateNullByteNotInArg(options.execPath, "options.execPath");
```

<a id="ref-47"></a>
[47] [`Repo denoland/deno: ext/node/polyfills/internal/child_process.ts`](https://github.com/denoland/deno/blob/d6212d40/ext/node/polyfills/internal/child_process.ts#L1048-L1048) lines 1048-1048

```ts
    validateNullByteNotInArg(options.shell, "options.shell");
```

<a id="ref-48"></a>
[48] [`Repo denoland/deno: tests/unit_node/child_process_test.ts`](https://github.com/denoland/deno/blob/d6212d40/tests/unit_node/child_process_test.ts#L477-L480) lines 477-480

```ts
          // deno-lint-ignore no-explicit-any
          (err as any).code,
          "ERR_CHILD_PROCESS_STDIO_MAXBUFFER",
        );
```

<a id="ref-49"></a>
[49] [`Repo denoland/deno: ext/node/polyfills/child_process.ts`](https://github.com/denoland/deno/blob/d6212d40/ext/node/polyfills/child_process.ts#L257-L263) lines 257-263

```ts
  const timeout = options?.timeout;
  if (timeout != null && timeout > 0) {
    const killSignal = options?.killSignal ?? "SIGTERM";
    let timeoutId: ReturnType<typeof setTimeout> | null = setTimeout(() => {
      timeoutId = null;
      child.kill(killSignal as string);
    }, timeout);
```

<a id="ref-50"></a>
[50] [`Repo denoland/deno: ext/node/polyfills/internal/child_process.ts`](https://github.com/denoland/deno/blob/d6212d40/ext/node/polyfills/internal/child_process.ts#L1083-L1093) lines 1083-1093

```ts
      if (!emittedShellDeprecation) {
        process.emitWarning(
          "Passing args to a child process with shell option true can lead to security " +
            "vulnerabilities, as the arguments are not escaped, only concatenated.",
          "DeprecationWarning",
          "DEP0190",
        );
        emittedShellDeprecation = true;
      }
      const escapedParts = [escapeShellArg(file), ...args.map(escapeShellArg)];
      command = ArrayPrototypeJoin(escapedParts, " ");
```

<a id="ref-51"></a>
[51] [`Repo denoland/deno: ext/node/polyfills/internal/child_process.ts`](https://github.com/denoland/deno/blob/d6212d40/ext/node/polyfills/internal/child_process.ts#L1571-L1577) lines 1571-1577

```ts
  throw new ERR_INVALID_ARG_TYPE("input", [
    "string",
    "Buffer",
    "TypedArray",
    "DataView",
  ], input);
}
```

<a id="ref-52"></a>
[52] [`Repo denoland/deno: tests/unit_node/child_process_test.ts`](https://github.com/denoland/deno/blob/d6212d40/tests/unit_node/child_process_test.ts#L1253-L1262) lines 1253-1262

```ts
    assertThrows(
      () => {
        spawnSync(Deno.execPath(), ["fmt", "-"], {
          // deno-lint-ignore no-explicit-any
          input: {} as any,
        });
      },
      Error,
      'The "input" argument must be of type string or an instance of Buffer, TypedArray, or DataView. Received an instance of Object',
    );
```

<a id="ref-53"></a>
[53] [`Repo denoland/deno: ext/node/polyfills/child_process.ts`](https://github.com/denoland/deno/blob/d6212d40/ext/node/polyfills/child_process.ts#L75-L79) lines 75-79

```ts
export function fork(
  modulePath: string | URL,
  _args?: string[],
  _options?: ForkOptions,
) {
```

<a id="ref-54"></a>
[54] [`Repo denoland/deno: ext/node/polyfills/child_process.ts`](https://github.com/denoland/deno/blob/d6212d40/ext/node/polyfills/child_process.ts#L239-L243) lines 239-243

```ts
export function spawn(
  command: string,
  argsOrOptions?: string[] | SpawnOptions,
  maybeOptions?: SpawnOptions,
): ChildProcess {
```

<a id="ref-55"></a>
[55] [`Repo denoland/deno: ext/node/polyfills/child_process.ts`](https://github.com/denoland/deno/blob/d6212d40/ext/node/polyfills/child_process.ts#L300-L304) lines 300-304

```ts
export function spawnSync(
  command: string,
  argsOrOptions?: string[] | SpawnSyncOptions,
  maybeOptions?: SpawnSyncOptions,
): SpawnSyncResult {
```

<a id="ref-56"></a>
[56] [`Repo denoland/deno: ext/node/polyfills/internal/child_process.ts`](https://github.com/denoland/deno/blob/d6212d40/ext/node/polyfills/internal/child_process.ts#L247-L272) lines 247-272

```ts
  spawn(options) {
    // Validate options
    if (options == null || typeof options !== "object") {
      throw new ERR_INVALID_ARG_TYPE("options", "object", options);
    }

    // Validate envPairs before file (Node.js validation order)
    const { envPairs } = options;
    if (envPairs !== undefined && !ArrayIsArray(envPairs)) {
      throw new ERR_INVALID_ARG_TYPE("options.envPairs", "Array", envPairs);
    }

    // Validate args
    const { args } = options;
    if (args !== undefined && !ArrayIsArray(args)) {
      throw new ERR_INVALID_ARG_TYPE("options.args", "Array", args);
    }

    // Validate file
    const { file } = options;
    if (file == null || typeof file !== "string") {
      throw new ERR_INVALID_ARG_TYPE("options.file", "string", file);
    }

    this.#spawnInternal(file, args || [], options);
  }
```

<a id="ref-57"></a>
[57] [`Repo denoland/deno: ext/node/polyfills/internal/child_process.ts`](https://github.com/denoland/deno/blob/d6212d40/ext/node/polyfills/internal/child_process.ts#L1034-L1093) lines 1034-1093

```ts

  // Validate the shell, if present.
  if (
    options.shell != null &&
    typeof options.shell !== "boolean" &&
    typeof options.shell !== "string"
  ) {
    throw new ERR_INVALID_ARG_TYPE(
      "options.shell",
      ["boolean", "string"],
      options.shell,
    );
  }
  if (typeof options.shell === "string") {
    validateNullByteNotInArg(options.shell, "options.shell");
  }

  // Validate argv0, if present.
  if (options.argv0 != null) {
    validateString(options.argv0, "options.argv0");
    validateNullByteNotInArg(options.argv0, "options.argv0");
  }

  // Validate windowsHide, if present.
  if (options.windowsHide != null) {
    validateBoolean(options.windowsHide, "options.windowsHide");
  }

  // Validate windowsVerbatimArguments, if present.
  let { windowsVerbatimArguments } = options;
  if (windowsVerbatimArguments != null) {
    validateBoolean(
      windowsVerbatimArguments,
      "options.windowsVerbatimArguments",
    );
  }

  validateOneOf(options.serialization, "options.serialization", [
    undefined,
    "json",
    "advanced",
  ]);

  if (options.shell) {
    // When args are provided, escape them to prevent shell injection.
    // When no args are provided (just a string command), the user intends
    // for shell interpretation, so don't escape.
    let command;
    if (args.length > 0) {
      if (!emittedShellDeprecation) {
        process.emitWarning(
          "Passing args to a child process with shell option true can lead to security " +
            "vulnerabilities, as the arguments are not escaped, only concatenated.",
          "DeprecationWarning",
          "DEP0190",
        );
        emittedShellDeprecation = true;
      }
      const escapedParts = [escapeShellArg(file), ...args.map(escapeShellArg)];
      command = ArrayPrototypeJoin(escapedParts, " ");
```

<a id="ref-58"></a>
[58] [`Repo denoland/deno: ext/node/polyfills/child_process.ts`](https://github.com/denoland/deno/blob/d6212d40/ext/node/polyfills/child_process.ts#L28-L32) lines 28-32

```ts
  validateInteger,
  validateNumber,
  validateObject,
  validateString,
} = core.loadExtScript("ext:deno_node/internal/validators.mjs");
```
