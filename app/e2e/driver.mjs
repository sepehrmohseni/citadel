import { execFileSync, spawn } from "node:child_process";
import { mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { dirname } from "node:path";
import { deflateSync } from "node:zlib";

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

const CRC = new Uint32Array(256).map((_, n) => {
  let c = n;
  for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
  return c >>> 0;
});

function crc32(buf) {
  let c = 0xffffffff;
  for (const b of buf) c = CRC[(c ^ b) & 0xff] ^ (c >>> 8);
  return (c ^ 0xffffffff) >>> 0;
}

function chunk(type, data) {
  const len = Buffer.alloc(4);
  len.writeUInt32BE(data.length);
  const td = Buffer.concat([Buffer.from(type, "ascii"), data]);
  const crc = Buffer.alloc(4);
  crc.writeUInt32BE(crc32(td));
  return Buffer.concat([len, td, crc]);
}

export function xwdToPng(xwd) {
  const f = (i) => xwd.readUInt32BE(i * 4);
  const [hsize, w, h, order, bpl, ncolors] = [f(0), f(4), f(5), f(7), f(12), f(19)];
  const pixels = xwd.subarray(hsize + ncolors * 12);
  const raw = Buffer.alloc((w * 3 + 1) * h);
  for (let y = 0; y < h; y++) {
    const row = y * (w * 3 + 1);
    for (let x = 0; x < w; x++) {
      const p = y * bpl + x * 4;
      const [r, g, b] = order === 0 ? [pixels[p + 2], pixels[p + 1], pixels[p]] : [pixels[p + 1], pixels[p + 2], pixels[p + 3]];
      raw[row + 1 + x * 3] = r;
      raw[row + 2 + x * 3] = g;
      raw[row + 3 + x * 3] = b;
    }
  }
  const ihdr = Buffer.alloc(13);
  ihdr.writeUInt32BE(w, 0);
  ihdr.writeUInt32BE(h, 4);
  ihdr.set([8, 2, 0, 0, 0], 8);
  return Buffer.concat([
    Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
    chunk("IHDR", ihdr),
    chunk("IDAT", deflateSync(raw, { level: 6 })),
    chunk("IEND", Buffer.alloc(0)),
  ]);
}

export class App {
  #ws = null;
  #target = null;
  #seq = 1000;
  #pending = new Map();
  #targetWaiters = [];
  errors = [];

  constructor(child, port, tools) {
    this.child = child;
    this.port = port;
    this.tools = tools;
  }

  static async launch({ bin, home, port = 9222, env = {}, tools = {} }) {
    mkdirSync(home, { recursive: true });
    const child = spawn(bin, [], {
      env: {
        ...process.env,
        HOME: home,
        XDG_DATA_HOME: `${home}/.local/share`,
        XDG_CONFIG_HOME: `${home}/.config`,
        XDG_CACHE_HOME: `${home}/.cache`,
        WEBKIT_INSPECTOR_HTTP_SERVER: `127.0.0.1:${port}`,
        GDK_BACKEND: "x11",
        ...env,
      },
      stdio: ["ignore", "ignore", "pipe"],
    });
    child.stderr.on("data", () => undefined);
    const app = new App(child, port, tools);
    const deadline = Date.now() + 30000;
    for (;;) {
      try {
        const html = await (await fetch(`http://127.0.0.1:${port}/`)).text();
        const m = /socket\/(\d+)\/(\d+)\/WebPage/.exec(html);
        if (m) {
          await app.#connect(`ws://127.0.0.1:${port}/${m[0]}`);
          break;
        }
      } catch {
        if (Date.now() > deadline) throw new Error("inspector did not come up");
      }
      if (Date.now() > deadline) throw new Error("no inspectable page");
      await sleep(250);
    }
    await app.waitFor("document.readyState === 'complete'");
    return app;
  }

  #connect(url) {
    return new Promise((resolve, reject) => {
      const ws = new WebSocket(url);
      this.#ws = ws;
      ws.onerror = () => reject(new Error("inspector websocket failed"));
      ws.onopen = () => ws.send(JSON.stringify({ id: 1, method: "Target.setPauseOnStart", params: { pauseOnStart: false } }));
      ws.onmessage = (ev) => {
        const m = JSON.parse(ev.data);
        if (m.method === "Target.targetCreated") {
          this.#target = m.params.targetInfo.targetId;
          void this.#raw("Console.enable").catch(() => undefined);
          for (const w of this.#targetWaiters.splice(0)) w();
          resolve();
        } else if (m.method === "Target.dispatchMessageFromTarget") {
          const inner = JSON.parse(m.params.message);
          if (inner.id !== undefined && this.#pending.has(inner.id)) {
            const { ok, fail } = this.#pending.get(inner.id);
            this.#pending.delete(inner.id);
            if (inner.error) fail(new Error(inner.error.message));
            else ok(inner.result);
          } else if (inner.method === "Console.messageAdded") {
            const c = inner.params.message;
            if (c.level === "error") this.errors.push(`${c.text} ${c.url ?? ""}:${c.line ?? ""}`);
          }
        }
      };
    });
  }

  #raw(method, params = {}) {
    const id = ++this.#seq;
    return new Promise((ok, fail) => {
      this.#pending.set(id, { ok, fail });
      this.#ws.send(
        JSON.stringify({
          id: ++this.#seq,
          method: "Target.sendMessageToTarget",
          params: { targetId: this.#target, message: JSON.stringify({ id, method, params }) },
        }),
      );
      setTimeout(() => {
        if (this.#pending.delete(id)) fail(new Error(`${method} timed out`));
      }, 20000);
    });
  }

  async eval(expr) {
    const wrapped = `Promise.resolve((async () => (${expr}))())`;
    const r = await this.#raw("Runtime.evaluate", { expression: wrapped, returnByValue: false });
    if (r.wasThrown) throw new Error(r.result?.description ?? "evaluation threw");
    const done = await this.#raw("Runtime.awaitPromise", { promiseObjectId: r.result.objectId, returnByValue: true });
    if (done.wasThrown) throw new Error(done.result?.description ?? "promise rejected");
    return done.result?.value;
  }

  async waitFor(expr, timeout = 15000, label = expr) {
    const deadline = Date.now() + timeout;
    let last;
    while (Date.now() < deadline) {
      try {
        last = await this.eval(expr);
        if (last) return last;
      } catch (e) {
        last = String(e);
      }
      await sleep(150);
    }
    throw new Error(`waitFor timed out: ${label} (last=${JSON.stringify(last)})`);
  }

  async click(selector) {
    await this.waitFor(`!!document.querySelector(${JSON.stringify(selector)})`, 10000, `click target ${selector}`);
    await this.eval(`(() => { const el = document.querySelector(${JSON.stringify(selector)}); el.scrollIntoView({ block: "center" }); el.click(); return true; })()`);
  }

  async clickText(selector, text) {
    const q = `[...document.querySelectorAll(${JSON.stringify(selector)})].find(e => e.textContent.trim() === ${JSON.stringify(text)})`;
    await this.waitFor(`!!${q}`, 10000, `element ${selector} with text ${text}`);
    await this.eval(`(() => { const el = ${q}; el.scrollIntoView({ block: "center" }); el.click(); return true; })()`);
  }

  async fill(selector, value) {
    await this.waitFor(`!!document.querySelector(${JSON.stringify(selector)})`, 10000, `fill target ${selector}`);
    await this.eval(
      `(() => { const el = document.querySelector(${JSON.stringify(selector)}); el.focus(); el.value = ${JSON.stringify(value)}; el.dispatchEvent(new Event("input", { bubbles: true })); el.dispatchEvent(new Event("change", { bubbles: true })); return true; })()`,
    );
  }

  async key(selector, key, mods = {}) {
    await this.eval(
      `(() => { const el = ${selector ? `document.querySelector(${JSON.stringify(selector)})` : "document.activeElement || document.body"}; el.dispatchEvent(new KeyboardEvent("keydown", { key: ${JSON.stringify(key)}, bubbles: true, cancelable: true, ctrlKey: ${!!mods.ctrl}, metaKey: ${!!mods.meta}, shiftKey: ${!!mods.shift} })); return true; })()`,
    );
  }

  windowId() {
    const { xwininfo, xprop, env } = this.tools;
    const tree = execFileSync(xwininfo, ["-root", "-tree"], { env }).toString();
    for (const m of tree.matchAll(/^\s+(0x[0-9a-f]+) "Citadel"/gm)) {
      try {
        const pid = execFileSync(xprop, ["-id", m[1], "_NET_WM_PID"], { env }).toString();
        if (pid.includes(`= ${this.child.pid}`)) return m[1];
      } catch {
        continue;
      }
    }
    throw new Error("app window not found");
  }

  async screenshot(file) {
    const { xwd, env } = this.tools;
    await sleep(350);
    const raw = execFileSync(xwd, ["-silent", "-id", this.windowId()], { env, maxBuffer: 64 << 20 });
    mkdirSync(dirname(file), { recursive: true });
    writeFileSync(file, xwdToPng(raw));
    return file;
  }

  async close() {
    try {
      this.#ws?.close();
    } catch {
      return;
    } finally {
      this.child.kill("SIGTERM");
      await sleep(400);
    }
  }
}

export function readText(path) {
  return readFileSync(path, "utf8");
}
