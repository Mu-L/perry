// electron — Electron-compatible app shell for Perry.
//
// Existing Electron app code (`import { app, BrowserWindow, ipcMain } from
// 'electron'`) runs unmodified. App logic compiles natively; the view runs in
// the OS-native webview. Internals are the Tauri model (system webview, single
// native process); the public surface is Electron's.
//
// Main process (this file, compiled native): app, BrowserWindow, ipcMain, Menu,
// dialog, webContents. Renderer/preload (ipcRenderer, contextBridge) is the JS
// in ./preload-runtime.ts injected into each webview.

import { EventEmitter } from "events";
import {
  Window,
  WebView,
  webviewLoadUrl,
  webviewEvaluateJs,
  webviewSetOnMessage,
  webviewSetOnLoaded,
  webviewAddUserScript,
  appRequestLoop,
  appQuit,
} from "perry/ui";
import * as os from "os";
import * as path from "path";
import * as fs from "fs";
import * as childProcess from "child_process";
import { PRELOAD_RUNTIME } from "./preload-runtime";

// ---------------------------------------------------------------------------
// Small helpers for marshalling values across the webview boundary.
// ---------------------------------------------------------------------------

function jsStringLiteral(s: string): string {
  // Produce a JS source string literal for embedding in an evaluateJs payload.
  return JSON.stringify(s);
}

// Build `window.__perryResolve(id, ok, payload)` source. `payload` is the
// result re-encoded as a JS string literal of its JSON (double-encoded), which
// the renderer JSON.parses. undefined results pass the bare `undefined` token.
function resolveJs(id: number, ok: boolean, value: unknown): string {
  let arg3: string;
  if (value === undefined) {
    arg3 = "undefined";
  } else {
    arg3 = jsStringLiteral(JSON.stringify(value));
  }
  return "window.__perryResolve(" + id + "," + (ok ? "true" : "false") + "," + arg3 + ")";
}

function deliverJs(channel: string, args: unknown[]): string {
  const argsJson = JSON.stringify(args);
  return "window.__perryDeliver(" + jsStringLiteral(channel) + "," + jsStringLiteral(argsJson) + ")";
}

// ---------------------------------------------------------------------------
// ipcMain
// ---------------------------------------------------------------------------

type IpcInvokeHandler = (event: IpcMainInvokeEvent, ...args: any[]) => any;

interface IpcMainInvokeEvent {
  sender: WebContents;
  frameId: number;
  processId: number;
}

interface IpcMainEvent {
  sender: WebContents;
  reply: (channel: string, ...args: any[]) => void;
  frameId: number;
  processId: number;
  returnValue?: any;
}

class IpcMain extends EventEmitter {
  // channel -> invoke handler (for ipcRenderer.invoke / ipcMain.handle).
  // Initialized in the constructor (not as a field initializer): Perry does not
  // reliably run field initializers on EventEmitter subclasses.
  private handlers: { [channel: string]: IpcInvokeHandler };

  constructor() {
    super();
    this.handlers = {};
  }

  handle(channel: string, listener: IpcInvokeHandler): void {
    this.handlers[channel] = listener;
  }
  handleOnce(channel: string, listener: IpcInvokeHandler): void {
    const self = this;
    this.handlers[channel] = function (event: IpcMainInvokeEvent, ...args: any[]) {
      delete self.handlers[channel];
      return listener(event, ...args);
    };
  }
  removeHandler(channel: string): void {
    delete this.handlers[channel];
  }

  // Internal: dispatch a parsed message coming from a webview.
  _dispatch(win: BrowserWindow, msg: { kind: string; id: number; channel: string; args: any[] }): void {
    const wc = win.webContents;
    if (msg.kind === "console") {
      // Renderer console / errors forwarded over the bridge.
      const text = msg.args && msg.args.length > 0 ? String(msg.args[0]) : "";
      if (msg.channel === "error") console.error("[renderer-console] " + text);
      else console.log("[renderer-console] " + text);
      return;
    }
    if (msg.kind === "invoke") {
      const handler = this.handlers[msg.channel];
      const invokeEvent: IpcMainInvokeEvent = { sender: wc, frameId: 0, processId: 0 };
      if (!handler) {
        wc._eval(resolveJs(msg.id, false, { message: "No handler registered for '" + msg.channel + "'" }));
        return;
      }
      try {
        const result = handler(invokeEvent, ...msg.args);
        Promise.resolve(result).then(
          (value) => wc._eval(resolveJs(msg.id, true, value)),
          (err) => wc._eval(resolveJs(msg.id, false, { message: errMessage(err) }))
        );
      } catch (err) {
        wc._eval(resolveJs(msg.id, false, { message: errMessage(err) }));
      }
    } else if (msg.kind === "send") {
      const event: IpcMainEvent = {
        sender: wc,
        reply: (channel: string, ...args: any[]) => wc.send(channel, ...args),
        frameId: 0,
        processId: 0,
      };
      this.emit(msg.channel, event, ...msg.args);
    }
  }
}

function errMessage(err: unknown): string {
  if (err && typeof err === "object" && "message" in (err as any)) return String((err as any).message);
  return String(err);
}

export const ipcMain = new IpcMain();

// ---------------------------------------------------------------------------
// WebContents — the per-window bridge to its webview.
// ---------------------------------------------------------------------------

class WebContents extends EventEmitter {
  // The perry/ui WebView widget handle for this window.
  _wv: any;
  id: number;

  constructor(id: number) {
    super();
    this._wv = 0;
    this.id = id;
  }

  _eval(js: string): void {
    if (this._wv) {
      webviewEvaluateJs(this._wv, js, (_result: string) => {});
    }
  }

  // main -> renderer push (ipcRenderer.on receives it)
  send(channel: string, ...args: any[]): void {
    this._eval(deliverJs(channel, args));
  }

  executeJavaScript(code: string): Promise<any> {
    return new Promise((resolve) => {
      if (!this._wv) {
        resolve(undefined);
        return;
      }
      webviewEvaluateJs(this._wv, code, (result: string) => resolve(result));
    });
  }

  openDevTools(): void {
    /* no-op: system webview devtools are opened via the OS inspector */
  }
  closeDevTools(): void {}
  setWindowOpenHandler(_handler: (details: any) => any): void {}
  get session(): any {
    return { clearStorageData: () => Promise.resolve() };
  }
}

// ---------------------------------------------------------------------------
// BrowserWindow
// ---------------------------------------------------------------------------

interface BrowserWindowOptions {
  width?: number;
  height?: number;
  title?: string;
  show?: boolean;
  webPreferences?: { preload?: string; [k: string]: any };
  [k: string]: any;
}

let nextWebContentsId = 1;
const openWindows: BrowserWindow[] = [];

// Keeps the app-ready closure passed to appRequestLoop alive from module init
// until the native loop fires it after main top-level (GC root).
let rootedOnReady: (() => void) | null = null;

class BrowserWindow extends EventEmitter {
  // perry/ui window operations, captured as closures over the LOCAL `win` in
  // the constructor. This is required: perry/ui instance-method dispatch
  // (setBody/show/…) only fires on a local flow-tracked from the `Window()`
  // call — calling on a property receiver (`this.win.show()`) compiles to a
  // generic no-op, so the window would never composite or respond. A closure
  // capturing the local preserves the dispatch.
  private _show: () => void;
  private _hide: () => void;
  private _close: () => void;
  private _setSize: (w: number, h: number) => void;
  webContents: WebContents;
  private destroyed: boolean;
  private preloadPath: string | undefined;

  constructor(options?: BrowserWindowOptions) {
    super();
    this.destroyed = false;
    const opts = options || {};
    const width = opts.width || 800;
    const height = opts.height || 600;
    const title = opts.title || "";
    this.preloadPath = opts.webPreferences && opts.webPreferences.preload;

    const win = Window(title, width, height);
    this._show = () => win.show();
    this._hide = () => win.hide();
    this._close = () => win.close();
    this._setSize = (w: number, h: number) => win.setSize(w, h);
    this.webContents = new WebContents(nextWebContentsId++);

    // Create the webview that fills the window. Start blank; load via
    // loadFile/loadURL. Persistent data store (ephemeral=0) so apps keep state.
    const wv = WebView({ url: "about:blank", width, height });
    this.webContents._wv = wv;

    // Inject the IPC bridge runtime + the app's preload (document-start),
    // before any page loads. Order matters: bridge first so `require('electron')`
    // and `contextBridge` exist when the app's preload runs.
    webviewAddUserScript(wv, PRELOAD_RUNTIME);
    if (this.preloadPath) {
      const preloadSrc = tryReadFile(this.preloadPath);
      if (preloadSrc) webviewAddUserScript(wv, preloadSrc);
    }

    // Fire webContents 'did-finish-load' / 'dom-ready' when the page loads, so
    // apps that push to the renderer after load (a very common pattern) work.
    const self = this;
    // NOTE: wires webContents 'did-finish-load'/'dom-ready' from the webview's
    // onLoaded delegate. The native onLoaded currently doesn't fire for file://
    // loads (tracked gap) so apps that push to the renderer on load don't yet
    // get the event; renderer→main and main→renderer IPC otherwise work.
    webviewSetOnLoaded(wv, (_url: string) => {
      self.webContents.emit("did-finish-load");
      self.webContents.emit("dom-ready");
    });

    // Route inbound IPC from this window's renderer to ipcMain.
    webviewSetOnMessage(wv, (json: string) => {
      let msg: any;
      try {
        msg = JSON.parse(json);
      } catch (e) {
        return;
      }
      ipcMain._dispatch(self, msg);
    });

    win.setBody(wv);
    if (opts.show !== false) {
      win.show();
    }
    openWindows.push(this);
  }

  loadURL(url: string): Promise<void> {
    webviewLoadUrl(this.webContents._wv, url);
    return Promise.resolve();
  }

  loadFile(filePath: string, _options?: any): Promise<void> {
    // Resolve relative to cwd; Electron resolves relative to the app dir.
    const abs = path.isAbsolute(filePath) ? filePath : path.join(process.cwd(), filePath);
    webviewLoadUrl(this.webContents._wv, "file://" + abs);
    return Promise.resolve();
  }

  show(): void {
    this._show();
  }
  hide(): void {
    this._hide();
  }
  close(): void {
    if (this.destroyed) return;
    this.emit("close");
    this._close();
    this.destroyed = true;
    const idx = openWindows.indexOf(this);
    if (idx >= 0) openWindows.splice(idx, 1);
    this.emit("closed");
    if (openWindows.length === 0) {
      app.emit("window-all-closed");
    }
  }
  destroy(): void {
    this.close();
  }
  isDestroyed(): boolean {
    return this.destroyed;
  }
  setSize(width: number, height: number): void {
    this._setSize(width, height);
  }
  setTitle(_title: string): void {
    /* perry/ui window title is set at create; runtime retitle is a follow-up */
  }
  focus(): void {
    this._show();
  }

  static getAllWindows(): BrowserWindow[] {
    return openWindows.slice();
  }
  static getFocusedWindow(): BrowserWindow | null {
    return openWindows.length > 0 ? openWindows[openWindows.length - 1] : null;
  }
}

function tryReadFile(p: string): string | null {
  try {
    const abs = path.isAbsolute(p) ? p : path.join(process.cwd(), p);
    return fs.readFileSync(abs, "utf8");
  } catch (e) {
    console.error("[electron] failed to read preload '" + p + "': " + errMessage(e));
    return null;
  }
}

// ---------------------------------------------------------------------------
// app
// ---------------------------------------------------------------------------

class App extends EventEmitter {
  private ready: boolean;
  private readyResolvers: Array<() => void>;
  private loopScheduled: boolean;
  private appName: string;
  dock: { hide: () => void; show: () => void; setIcon: (icon: any) => void };

  constructor() {
    super();
    this.ready = false;
    this.readyResolvers = [];
    this.loopScheduled = false;
    this.appName = "Perry App";
    this.dock = { hide: () => {}, show: () => {}, setIcon: (_icon: any) => {} };
    // Register the native UI event loop. This does NOT block — the generated
    // `main` enters the loop at the top level AFTER the user's `main` top-level
    // code runs (registering ipcMain/whenReady handlers). Entering the loop at
    // the top level (rather than from a microtask) is what lets windows created
    // in `whenReady().then(...)` actually composite on screen.
    this.startLoop();
  }

  private startLoop(): void {
    if (this.loopScheduled) return;
    this.loopScheduled = true;
    const self = this;
    // onReady fires when the loop takes over (top level), resolving whenReady();
    // the `.then(createWindow)` chain then runs on the first pump tick. Held in
    // a module-level slot (rootedOnReady) so GC can't collect it before then.
    rootedOnReady = () => {
      self.ready = true;
      self.emit("ready");
      const resolvers = self.readyResolvers;
      self.readyResolvers = [];
      for (let i = 0; i < resolvers.length; i++) resolvers[i]();
    };
    appRequestLoop(rootedOnReady);
  }

  whenReady(): Promise<void> {
    if (this.ready) return Promise.resolve();
    const self = this;
    return new Promise<void>((resolve) => {
      self.readyResolvers.push(resolve);
    });
  }

  isReady(): boolean {
    return this.ready;
  }

  quit(): void {
    this.emit("before-quit");
    this.emit("will-quit");
    appQuit();
  }
  exit(_code?: number): void {
    appQuit();
  }

  getName(): string {
    return this.appName;
  }
  setName(name: string): void {
    this.appName = name;
  }
  getVersion(): string {
    return "0.0.0";
  }
  getAppPath(): string {
    return process.cwd();
  }
  getPath(name: string): string {
    const home = os.homedir();
    switch (name) {
      case "home":
        return home;
      case "appData":
        return path.join(home, "Library", "Application Support");
      case "userData":
        return path.join(home, "Library", "Application Support", this.appName);
      case "temp":
        return os.tmpdir();
      case "desktop":
        return path.join(home, "Desktop");
      case "documents":
        return path.join(home, "Documents");
      case "downloads":
        return path.join(home, "Downloads");
      case "exe":
        return process.execPath || process.cwd();
      default:
        return home;
    }
  }
  requestSingleInstanceLock(): boolean {
    return true;
  }
  setActivationPolicy(_policy: string): void {}
}

export const app = new App();

// ---------------------------------------------------------------------------
// Menu / MenuItem  (v1: template is accepted; native menubar wiring is a
// follow-up. setApplicationMenu(null) hides — accepted as a no-op so apps run.)
// ---------------------------------------------------------------------------

class MenuItem {
  label: string;
  click?: () => void;
  submenu?: any;
  role?: string;
  type?: string;
  accelerator?: string;
  constructor(opts: any) {
    this.label = opts.label || "";
    this.click = opts.click;
    this.submenu = opts.submenu;
    this.role = opts.role;
    this.type = opts.type;
    this.accelerator = opts.accelerator;
  }
}

class Menu {
  items: MenuItem[];
  constructor() {
    this.items = [];
  }
  append(item: MenuItem): void {
    this.items.push(item);
  }
  popup(_options?: any): void {}
  static buildFromTemplate(template: any[]): Menu {
    const menu = new Menu();
    for (let i = 0; i < template.length; i++) {
      menu.append(new MenuItem(template[i]));
    }
    return menu;
  }
  static setApplicationMenu(_menu: Menu | null): void {
    /* v1: native menubar already provides standard Cmd+Q etc. */
  }
  static getApplicationMenu(): Menu | null {
    return null;
  }
}

export { Menu, MenuItem, BrowserWindow, WebContents };

// ---------------------------------------------------------------------------
// dialog  (v1: returns canceled; native NSOpenPanel/NSSavePanel wiring is a
// follow-up. Apps that gate on `canceled` keep running.)
// ---------------------------------------------------------------------------

export const dialog = {
  showOpenDialog(_window?: any, _options?: any): Promise<{ canceled: boolean; filePaths: string[] }> {
    console.warn("[electron] dialog.showOpenDialog is a v1 stub (returns canceled)");
    return Promise.resolve({ canceled: true, filePaths: [] });
  },
  showSaveDialog(_window?: any, _options?: any): Promise<{ canceled: boolean; filePath: string }> {
    console.warn("[electron] dialog.showSaveDialog is a v1 stub (returns canceled)");
    return Promise.resolve({ canceled: true, filePath: "" });
  },
  showMessageBox(_window?: any, _options?: any): Promise<{ response: number; checkboxChecked: boolean }> {
    return Promise.resolve({ response: 0, checkboxChecked: false });
  },
  showErrorBox(title: string, content: string): void {
    console.error("[electron] " + title + ": " + content);
  },
};

// ---------------------------------------------------------------------------
// shell
// ---------------------------------------------------------------------------

export const shell = {
  openExternal(url: string): Promise<void> {
    try {
      childProcess.spawn("open", [url]);
    } catch (e) {
      console.error("[electron] shell.openExternal failed: " + errMessage(e));
    }
    return Promise.resolve();
  },
  openPath(p: string): Promise<string> {
    try {
      childProcess.spawn("open", [p]);
    } catch (e) {}
    return Promise.resolve("");
  },
  showItemInFolder(_p: string): void {},
  beep(): void {},
};

// ---------------------------------------------------------------------------
// ipcRenderer / contextBridge — renderer-only in Electron. Provided here so a
// main-process `import { ipcRenderer } from 'electron'` doesn't crash; the real
// implementations run in the renderer (./preload-runtime.ts).
// ---------------------------------------------------------------------------

export const ipcRenderer = {
  invoke(_channel: string, ..._args: any[]): Promise<any> {
    return Promise.reject(new Error("ipcRenderer is only available in the renderer process"));
  },
  send(_channel: string, ..._args: any[]): void {},
  on(_channel: string, _listener: any): any {
    return ipcRenderer;
  },
  once(_channel: string, _listener: any): any {
    return ipcRenderer;
  },
  removeListener(_channel: string, _listener: any): any {
    return ipcRenderer;
  },
  removeAllListeners(_channel?: string): any {
    return ipcRenderer;
  },
};

export const contextBridge = {
  exposeInMainWorld(_key: string, _api: any): void {
    /* renderer-only; see preload-runtime.ts */
  },
};

export const nativeTheme = {
  shouldUseDarkColors: false,
  themeSource: "system",
  on: (_event: string, _cb: any) => {},
};

// Default export mirrors `const electron = require('electron')`.
export default {
  app,
  BrowserWindow,
  ipcMain,
  ipcRenderer,
  contextBridge,
  Menu,
  MenuItem,
  dialog,
  shell,
  nativeTheme,
  WebContents,
};
