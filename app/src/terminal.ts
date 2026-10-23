import "@xterm/xterm/css/xterm.css";
import { FitAddon } from "@xterm/addon-fit";
import { SearchAddon } from "@xterm/addon-search";
import { Unicode11Addon } from "@xterm/addon-unicode11";
import { WebLinksAddon } from "@xterm/addon-web-links";
import { WebglAddon } from "@xterm/addon-webgl";
import { Terminal } from "@xterm/xterm";
import { save } from "@tauri-apps/plugin-dialog";

import { Channel, api, errCode, errText, type Id, type Prefs, type PtyStatus, type Snapshot, type TelemetryMsg } from "./api";
import { bytes, duration, h, icon, isMac, keyOf, rate, terminalChord } from "./dom";
import { currentTab, openFiles, retitleTab, type Tab, type TabTone } from "./shell";
import { hostById, links, prefs, refreshHosts } from "./store";
import { themeFor } from "./themes";
import { btn, confirmBox, contextMenu, copyText, fail, input, readClipboard, toast, writeClipboard } from "./ui";

const encoder = new TextEncoder();
const HISTORY = 90;
const ACK_STEP = 256 * 1024;
const BLINK_IDLE_MS = 10_000;
const RTL = "\u0590-\u08FF\uFB1D-\uFDFF\uFE70-\uFEFC";
const HAS_RTL = new RegExp(`[${RTL}]`);
const RTL_RUN = new RegExp(`[${RTL}](?:[${RTL}\u200C\u200D\u0660-\u0669\u06F0-\u06F9 .,:;!?()«»-]*[${RTL}])?`, "g");
const PERSIAN_FALLBACK = '"Vazirmatn Variable"';

export function rtlRuns(text: string): [number, number][] {
  if (!HAS_RTL.test(text)) return [];
  const out: [number, number][] = [];
  for (const m of text.matchAll(RTL_RUN)) if (m[0].length > 1) out.push([m.index, m.index + m[0].length]);
  return out;
}

export function withFallback(family: string): string {
  return family.includes("Vazirmatn") ? family : family.replace(/,?\s*monospace\s*$/, "") + `, ${PERSIAN_FALLBACK}, monospace`;
}

const fontsReady: Promise<unknown> = document.fonts
  ? Promise.all([document.fonts.load('14px "JetBrains Mono Variable"'), document.fonts.load(`14px ${PERSIAN_FALLBACK}`, "سلام")]).catch(() => undefined)
  : Promise.resolve();

type GlState = "unknown" | "hardware" | "software" | "none";
let glState: GlState = "unknown";

function probeGl(): GlState {
  if (glState !== "unknown") return glState;
  try {
    const c = document.createElement("canvas");
    const gl = c.getContext("webgl2");
    if (!gl) return (glState = "none");
    const info = gl.getExtension("WEBGL_debug_renderer_info");
    const name = String(info ? gl.getParameter(info.UNMASKED_RENDERER_WEBGL) : gl.getParameter(gl.RENDERER));
    gl.getExtension("WEBGL_lose_context")?.loseContext();
    glState = /llvmpipe|softpipe|swiftshader|software|basic render/i.test(name) ? "software" : "hardware";
  } catch {
    glState = "none";
  }
  return glState;
}

function wantsGl(): boolean {
  const mode = prefs.get()?.renderer ?? "auto";
  if (mode === "dom") return false;
  const state = probeGl();
  if (state === "none") return false;
  return mode === "webgl" || state === "hardware";
}

interface Live {
  key: string;
  host: Id;
  title: string;
  send(text: string): void;
  focus(): void;
  text(): string;
  renderer(): string;
}

const live = new Map<string, Live>();
const broadcast = new Set<string>();
const bcastListeners = new Set<() => void>();
let bcastWarned = false;
let seq = 0;

function bcastChanged(): void {
  for (const fn of [...bcastListeners]) fn();
}

async function toggleBroadcast(key: string): Promise<void> {
  if (broadcast.has(key)) {
    broadcast.delete(key);
  } else {
    if (!bcastWarned) {
      const ok = await confirmBox(
        "Broadcast input",
        "Keystrokes typed in any broadcasting terminal are sent to every broadcasting terminal. Enable it on each session you want in the group.",
        { confirm: "Join broadcast" },
      );
      if (!ok) return;
      bcastWarned = true;
    }
    broadcast.add(key);
  }
  bcastChanged();
}

interface Recording {
  start: number;
  cols: number;
  rows: number;
  events: string[];
  decoder: TextDecoder;
}

if (import.meta.env.DEV || import.meta.env.MODE === "e2e") {
  Object.assign(window, {
    __citadel: {
      terminals: () => [...live.keys()],
      text: (key: string) => live.get(key)?.text() ?? "",
      type: (key: string, data: string) => live.get(key)?.send(data),
      renderer: (key: string) => live.get(key)?.renderer() ?? null,
    },
  });
}

export function liveTerminals(): { key: string; title: string; host: string }[] {
  return [...live.values()].map(({ key, title, host }) => ({ key, title, host }));
}

export function sendText(key: string, text: string): void {
  live.get(key)?.send(text);
}

export function activeTerminalKey(): string | null {
  const t = currentTab();
  return t && live.has(t.key) ? t.key : null;
}

function concat(parts: Uint8Array[]): Uint8Array {
  if (parts.length === 1 && parts[0]) return parts[0];
  const total = parts.reduce((n, p) => n + p.length, 0);
  const out = new Uint8Array(total);
  let at = 0;
  for (const p of parts) {
    out.set(p, at);
    at += p.length;
  }
  return out;
}

function spark(values: number[], max: number, cls = ""): SVGSVGElement {
  const w = 268;
  const hgt = 46;
  const svg = h("svg", { class: `spark ${cls}`, viewBox: `0 0 ${w} ${hgt}`, preserveAspectRatio: "none" }) as SVGSVGElement;
  if (values.length < 2) return svg;
  const top = Math.max(max, 1e-9);
  const step = w / (HISTORY - 1);
  const pts = values.map((v, i) => [w - (values.length - 1 - i) * step, hgt - 2 - (Math.min(v, top) / top) * (hgt - 6)] as const);
  const line = pts.map(([x, y], i) => `${i ? "L" : "M"}${x.toFixed(1)},${y.toFixed(1)}`).join("");
  const first = pts[0];
  const last = pts[pts.length - 1];
  if (!first || !last) return svg;
  const area = `${line}L${last[0].toFixed(1)},${hgt}L${first[0].toFixed(1)},${hgt}Z`;
  svg.append(
    h("path", { d: area, fill: "currentColor", opacity: "0.12" }),
    h("path", { d: line, fill: "none", stroke: "currentColor", "stroke-width": "1.5", "vector-effect": "non-scaling-stroke" }),
  );
  return svg;
}

function meter(pct: number): HTMLElement {
  const cls = pct >= 90 ? "bar crit" : pct >= 75 ? "bar hot" : "bar";
  return h("div", { class: cls }, h("span", { style: { width: `${Math.max(0, Math.min(100, pct)).toFixed(1)}%` } }));
}

class Telemetry {
  el: HTMLElement;
  #host: Id;
  #id: number | null = null;
  #cpu: number[] = [];
  #rx: number[] = [];
  #tx: number[] = [];
  #starting = false;

  constructor(host: Id) {
    this.#host = host;
    this.el = h("aside", { class: "telemetry", hidden: true }, h("div", { class: "muted" }, "Waiting for first sample…"));
  }

  get open(): boolean {
    return !this.el.hidden;
  }

  async toggle(): Promise<void> {
    if (this.open) this.stop();
    else await this.start();
  }

  async start(): Promise<void> {
    this.el.hidden = false;
    if (this.#id !== null || this.#starting) return;
    this.#starting = true;
    const feed = new Channel<TelemetryMsg>();
    feed.onmessage = (m) => this.#onMsg(m);
    try {
      this.#id = await api.telemetryStart(this.#host, feed);
    } catch (e) {
      this.el.replaceChildren(h("div", { class: "alert" }, icon("warn", 15), h("span", null, errText(e))));
    } finally {
      this.#starting = false;
    }
  }

  stop(): void {
    this.el.hidden = true;
    if (this.#id !== null) {
      void api.telemetryStop(this.#id).catch(() => undefined);
      this.#id = null;
    }
  }

  #onMsg(m: TelemetryMsg): void {
    if (m.kind === "sample") this.#render(m);
    else if (m.kind === "error") {
      this.el.replaceChildren(h("div", { class: "alert" }, icon("warn", 15), h("span", null, m.message)));
      this.#id = null;
    } else this.#id = null;
  }

  #render(s: Snapshot): void {
    const push = (arr: number[], v: number) => {
      arr.push(v);
      if (arr.length > HISTORY) arr.shift();
    };
    const rx = s.net.reduce((n, i) => n + i.rx_bps, 0);
    const tx = s.net.reduce((n, i) => n + i.tx_bps, 0);
    push(this.#cpu, s.cpu.busy_pct);
    push(this.#rx, rx);
    push(this.#tx, tx);
    const netMax = Math.max(...this.#rx, ...this.#tx, 1024);
    const memUsed = s.mem.total_kib - s.mem.available_kib;
    const docker =
      s.docker.state === "available"
        ? h(
            "div",
            { class: "tm-block" },
            h("div", { class: "tm-label" }, "Containers", h("b", null, String(s.docker.containers.length))),
            h(
              "div",
              null,
              s.docker.containers.slice(0, 40).map((c) =>
                h(
                  "div",
                  { class: "ctr", title: `${c.image}\n${c.status}` },
                  h("span", { class: `dot ${c.state === "running" ? "ok" : c.state === "restarting" ? "warn" : c.state === "exited" ? "idle" : "err"}` }),
                  h("span", { class: "grow" }, c.name),
                  h("span", { class: "dim mono" }, c.state),
                ),
              ),
            ),
          )
        : s.docker.state === "unavailable"
          ? h("div", { class: "tm-row" }, h("span", { class: "dim" }, "Docker"), h("span", null, s.docker.reason))
          : null;
    this.el.replaceChildren(
      h(
        "div",
        { class: "tm-head" },
        h("span", { class: "host" }, s.host.hostname || "remote"),
        h("span", { class: "dim" }, `${s.host.kernel} · up ${duration(s.host.uptime_secs)} · ${s.host.cores} cores`),
      ),
      h(
        "div",
        { class: "tm-block", style: { color: "var(--accent)" } },
        h("div", { class: "tm-label" }, "CPU", h("b", null, `${s.cpu.busy_pct.toFixed(1)}%`)),
        spark(this.#cpu, 100),
        h(
          "div",
          { class: "cores", title: "Per-core utilisation" },
          s.cpu.per_core.map((p) => h("span", { title: `${p.toFixed(0)}%` }, h("i", { style: { height: `${Math.min(100, p)}%` } }))),
        ),
        h("div", { class: "tm-row" }, h("span", { class: "dim" }, "Load"), h("span", null, s.cpu.load.map((l) => l.toFixed(2)).join("  "))),
      ),
      h(
        "div",
        { class: "tm-block" },
        h("div", { class: "tm-label" }, "Memory", h("b", null, `${s.mem.used_pct.toFixed(1)}%`)),
        meter(s.mem.used_pct),
        h("div", { class: "tm-row" }, h("span", { class: "dim" }, "Used"), h("span", null, `${bytes(memUsed * 1024)} / ${bytes(s.mem.total_kib * 1024)}`)),
        s.mem.swap_total_kib > 0
          ? h("div", { class: "tm-row" }, h("span", { class: "dim" }, "Swap"), h("span", null, `${bytes(s.mem.swap_used_kib * 1024)} / ${bytes(s.mem.swap_total_kib * 1024)}`))
          : null,
      ),
      h(
        "div",
        { class: "tm-block", style: { color: "var(--info)" } },
        h("div", { class: "tm-label" }, "Network", h("b", null, `↓ ${rate(rx)}  ↑ ${rate(tx)}`)),
        h("div", { style: { position: "relative" } }, spark(this.#rx, netMax), h("div", { style: { position: "absolute", inset: "0", color: "var(--violet)" } }, spark(this.#tx, netMax))),
        s.net
          .filter((n) => n.rx_bps + n.tx_bps > 0 || s.net.length <= 4)
          .slice(0, 6)
          .map((n) => h("div", { class: "tm-row" }, h("span", { class: "dim" }, n.name), h("span", null, `↓ ${rate(n.rx_bps)}  ↑ ${rate(n.tx_bps)}`))),
      ),
      h(
        "div",
        { class: "tm-block" },
        h("div", { class: "tm-label" }, "Disks"),
        s.disks.slice(0, 8).map((d) =>
          h(
            "div",
            { class: "stack", style: { gap: "4px" }, title: d.fs },
            h("div", { class: "tm-row" }, h("span", null, d.mount), h("span", null, `${bytes(d.used_kib * 1024, 0)} / ${bytes(d.total_kib * 1024, 0)}`)),
            meter(d.used_pct),
          ),
        ),
      ),
      ...(docker ? [docker] : []),
    );
  }
}

export function terminalTab(host: Id): Tab {
  const meta = hostById(host);
  const label = meta?.label ?? "host";
  const key = `term:${host}:${++seq}`;
  const p: Prefs | null = prefs.get();
  const term = new Terminal({
    allowProposedApi: true,
    allowTransparency: false,
    cursorBlink: p?.cursor_blink ?? true,
    cursorStyle: (p?.cursor_style as "block" | "bar" | "underline") ?? "bar",
    fontFamily: withFallback(p?.font_family ?? "monospace"),
    fontSize: p?.font_size ?? 13,
    lineHeight: p?.line_height ?? 1.2,
    scrollback: p?.scrollback ?? 10000,
    macOptionIsMeta: true,
    rightClickSelectsWord: false,
    smoothScrollDuration: 0,
    theme: themeFor(p?.theme).theme,
    minimumContrastRatio: 1,
    drawBoldTextInBrightColors: false,
  });
  const fit = new FitAddon();
  const search = new SearchAddon();
  term.loadAddon(fit);
  term.loadAddon(search);
  term.loadAddon(new Unicode11Addon());
  term.unicode.activeVersion = "11";
  term.loadAddon(
    new WebLinksAddon((ev, uri) => {
      if (ev.metaKey || ev.ctrlKey) void copyText(uri, "Link copied to clipboard");
    }),
  );

  const hostEl = h("div", { class: "term-host" });
  const tele = new Telemetry(host);
  const toneDot = h("span", { class: "dot idle" });
  const findInput = input({ placeholder: "Find in scrollback", "aria-label": "Find" });
  const findBox = h(
    "div",
    { class: "term-find", hidden: true },
    findInput,
    btn("", { icon: "up", small: true, ghost: true, title: "Previous", onClick: () => search.findPrevious(findInput.value) }),
    btn("", { icon: "chevron", small: true, ghost: true, title: "Next", onClick: () => search.findNext(findInput.value) }),
    btn("", { icon: "close", small: true, ghost: true, title: "Close", onClick: () => closeFind() }),
  );
  const overlay = h("div", { class: "term-overlay", hidden: true });
  const bcastEl = h("span", { class: "bcast", hidden: true });
  const recEl = h("span", { class: "rec", hidden: true });
  let rec: Recording | null = null;
  let recTimer: ReturnType<typeof setInterval> | undefined;
  const renderBcast = () => {
    const on = broadcast.has(key);
    bcastEl.hidden = !on;
    bcastEl.textContent = `⇶ broadcast ${broadcast.size}`;
  };
  bcastListeners.add(renderBcast);
  const startRec = () => {
    rec = { start: performance.now(), cols: term.cols, rows: term.rows, events: [], decoder: new TextDecoder() };
    recEl.hidden = false;
    const tick = () => {
      if (!rec) return;
      const s = Math.floor((performance.now() - rec.start) / 1000);
      recEl.textContent = `● rec ${String(Math.floor(s / 60)).padStart(2, "0")}:${String(s % 60).padStart(2, "0")}`;
    };
    tick();
    recTimer = setInterval(tick, 1000);
    toast("Recording output. Keystrokes are never recorded.", "info", 3000);
  };
  const stopRec = async () => {
    const r = rec;
    rec = null;
    recEl.hidden = true;
    if (recTimer) clearInterval(recTimer);
    if (!r || !r.events.length) return;
    const header = JSON.stringify({ version: 2, width: r.cols, height: r.rows, timestamp: Math.floor(Date.now() / 1000), title: label, env: { TERM: "xterm-256color" } });
    const stamp = new Date().toISOString().slice(0, 19).replace(/[:T]/g, "-");
    const path = await save({ defaultPath: `${label.replace(/[^\w.-]+/g, "_")}-${stamp}.cast`, filters: [{ name: "asciicast", extensions: ["cast"] }] });
    if (!path) return;
    try {
      const size = await api.recordingSave(path, `${header}\n${r.events.join("\n")}\n`);
      toast(`Saved ${bytes(size)} recording. Replay with: asciinema play`, "ok", 6000);
    } catch (e) {
      fail(e, "Recording not saved");
    }
  };
  const toggleRec = () => (rec ? void stopRec() : startRec());
  const bar = h(
    "div",
    { class: "term-bar" },
    toneDot,
    h("span", { class: "who" }, meta ? `${meta.user}@${meta.hostname}${meta.port !== 22 ? `:${meta.port}` : ""}` : label),
    meta && meta.jump.length
      ? h("span", { class: "chain" }, icon("link", 12), meta.jump.map((j) => hostById(j)?.label ?? "?").join(" → "))
      : null,
    h("span", { class: "spacer" }),
    btn("", { icon: "search", small: true, ghost: true, title: `Find (${isMac ? "⌘" : "Ctrl+Shift+"}F)`, onClick: () => openFind() }),
    btn("", { icon: "folder", small: true, ghost: true, title: "Open files", onClick: () => openFiles(host) }),
    btn("", { icon: "chart", small: true, ghost: true, title: "Host telemetry", onClick: () => void tele.toggle().then(() => refit()) }),
    recEl,
    bcastEl,
    btn("", { icon: "broadcast", small: true, ghost: true, title: `Broadcast input (${isMac ? "⌘" : "Ctrl+Shift+"}B)`, onClick: () => void toggleBroadcast(key) }),
    btn("", { icon: "stop", small: true, ghost: true, title: `Record session (${isMac ? "⌘" : "Ctrl+Shift+"}R)`, onClick: toggleRec }),
    btn("", { icon: "refresh", small: true, ghost: true, title: "Reconnect", onClick: () => void reconnect() }),
  );
  const main = h("div", { class: "term-main" }, bar, hostEl, overlay, findBox);
  const el = h("div", { class: "term-pane", style: { background: themeFor(p?.theme).theme.background ?? "" } }, main, tele.el);

  let gl: WebglAddon | null = null;
  let glTimer: ReturnType<typeof setTimeout> | undefined;
  const attachGl = () => {
    if (glTimer) clearTimeout(glTimer);
    if (gl || disposed || !wantsGl()) return;
    try {
      const addon = new WebglAddon();
      addon.onContextLoss(() => {
        if (gl === addon) gl = null;
        addon.dispose();
      });
      term.loadAddon(addon);
      gl = addon;
    } catch {
      gl = null;
    }
  };
  const releaseGl = () => {
    if (glTimer) clearTimeout(glTimer);
    glTimer = setTimeout(() => {
      if (!gl || !el.hidden) return;
      gl.dispose();
      gl = null;
    }, 30000);
  };

  let pid: number | null = null;
  let opened = false;
  let disposed = false;
  let exitCode: number | null = null;
  let outbox: Uint8Array[] = [];
  let flushing = false;
  let resizeTimer: ReturnType<typeof setTimeout> | undefined;
  let tone: TabTone = "idle";

  const setTone = (t: TabTone) => {
    tone = t;
    toneDot.className = `dot ${t === "ok" ? "ok" : t === "warn" ? "warn pulse" : t === "err" ? "err" : "idle"}`;
    retitleTab(key, label, t);
  };

  const flush = async () => {
    flushing = true;
    while (outbox.length && pid !== null) {
      const chunk = concat(outbox);
      outbox = [];
      try {
        await api.ptyWrite(pid, chunk);
      } catch (e) {
        if (errCode(e) !== "not_found") fail(e, "Write failed");
        outbox = [];
        break;
      }
    }
    flushing = false;
  };

  const send = (data: Uint8Array) => {
    if (pid === null || data.length === 0) return;
    outbox.push(data);
    if (!flushing) void flush();
  };

  const sendString = (s: string) => send(encoder.encode(s));

  const showOverlay = (body: Node[]) => {
    overlay.replaceChildren(h("div", { class: "term-overlay-card" }, ...body));
    overlay.hidden = false;
  };

  const hideOverlay = () => {
    overlay.hidden = true;
    overlay.replaceChildren();
  };

  const refit = () => {
    if (el.hidden || !opened) return;
    try {
      fit.fit();
    } catch {
      return;
    }
  };

  const connect = async () => {
    if (disposed) return;
    exitCode = null;
    setTone("warn");
    showOverlay([h("div", { class: "spinner" }), h("strong", null, `Connecting to ${label}`), h("span", { class: "muted" }, "Negotiating keys and authenticating…")]);
    const data = new Channel<ArrayBuffer>();
    let processed = 0;
    let acked = 0;
    let ackFor: number | null = null;
    const ack = () => {
      if (ackFor === null || processed === acked) return;
      acked = processed;
      void api.ptyAck(ackFor, processed).catch(() => undefined);
    };
    data.onmessage = (buf) => {
      const chunk = new Uint8Array(buf);
      term.write(chunk, () => {
        processed += chunk.length;
        if (processed - acked >= ACK_STEP) ack();
      });
      if (rec) rec.events.push(JSON.stringify([Number(((performance.now() - rec.start) / 1000).toFixed(6)), "o", rec.decoder.decode(chunk, { stream: true })]));
    };
    const status = new Channel<PtyStatus>();
    status.onmessage = (s) => {
      if (s.kind === "exit") exitCode = s.code;
      else if (s.kind === "signal") term.write(`\r\n\x1b[2m[terminated by ${s.name}]\x1b[0m\r\n`);
      else ended();
    };
    try {
      refit();
      pid = await api.ptyOpen(host, Math.max(term.cols, 2), Math.max(term.rows, 2), data, status);
      if (disposed) {
        void api.ptyClose(pid);
        return;
      }
      ackFor = pid;
      ack();
      hideOverlay();
      setTone("ok");
      term.focus();
    } catch (e) {
      pid = null;
      setTone("err");
      const code = errCode(e);
      const fixes: HTMLElement[] = [];
      if (code === "legacy") {
        fixes.push(
          btn("enable legacy algorithms and retry", {
            primary: true,
            icon: "shield",
            onClick: async () => {
              const cur = hostById(host);
              if (!cur) return;
              const { asInput } = await import("./views/hosts");
              try {
                await api.hostSave({ ...asInput(cur), legacy_crypto: true });
                await refreshHosts();
                void connect();
              } catch (err) {
                fail(err);
              }
            },
          }),
        );
      }
      if (code === "auth" || code === "key_encrypted") {
        fixes.push(
          btn("edit host", {
            icon: "edit",
            onClick: () =>
              void import("./views/hosts").then(({ hostForm }) => {
                const cur = hostById(host);
                if (cur) hostForm(cur);
              }),
          }),
        );
      }
      showOverlay([
        icon("warn", 22),
        h("strong", null, "Connection failed"),
        h("span", { class: "muted err-detail" }, errText(e)),
        h("div", { class: "row" }, ...fixes, btn("Retry", { primary: fixes.length === 0, icon: "refresh", onClick: () => void connect() })),
      ]);
    }
  };

  const ended = () => {
    pid = null;
    outbox = [];
    if (disposed) return;
    setTone("idle");
    showOverlay([
      h("strong", null, exitCode === null ? "Session closed" : `Session ended · exit ${exitCode}`),
      h("span", { class: "muted" }, "Press Enter to reconnect."),
      h("div", { class: "row" }, btn("Reconnect", { primary: true, icon: "refresh", onClick: () => void connect() })),
    ]);
  };

  const reconnect = async () => {
    if (pid !== null) {
      const ok = await confirmBox("Reconnect session", "The current shell will be closed and a new one opened.", { confirm: "Reconnect" });
      if (!ok) return;
      const old = pid;
      pid = null;
      await api.ptyClose(old).catch(() => undefined);
    }
    await connect();
  };

  const openFind = () => {
    findBox.hidden = false;
    findInput.select();
    findInput.focus();
  };

  const closeFind = () => {
    findBox.hidden = true;
    search.clearDecorations();
    term.focus();
  };

  findInput.addEventListener("keydown", (e) => {
    if (e.key === "Enter") {
      e.preventDefault();
      if (e.shiftKey) search.findPrevious(findInput.value);
      else search.findNext(findInput.value);
    } else if (e.key === "Escape") closeFind();
  });

  const pasteGuard = async (text: string) => {
    const limit = prefs.get()?.paste_confirm_lines ?? 5;
    const lines = text.split(/\r\n|\r|\n/).length;
    if (limit > 0 && lines > limit) {
      const ok = await confirmBox(
        "Paste multiple lines",
        `You are about to paste ${lines} lines into ${label}. Each line may execute as a command.`,
        { confirm: "Paste", danger: true },
      );
      if (!ok) return;
    }
    term.paste(text);
  };

  let blinkTimer: ReturnType<typeof setTimeout> | undefined;
  const wakeCursor = () => {
    if (!(prefs.get()?.cursor_blink ?? true)) return;
    if (!term.options.cursorBlink) term.options.cursorBlink = true;
    if (blinkTimer) clearTimeout(blinkTimer);
    blinkTimer = setTimeout(() => {
      term.options.cursorBlink = false;
    }, BLINK_IDLE_MS);
  };
  term.onData((d) => {
    wakeCursor();
    sendString(d);
    if (broadcast.has(key)) {
      for (const k of broadcast) if (k !== key) live.get(k)?.send(d);
    }
  });
  term.onBinary((d) => {
    const buf = new Uint8Array(d.length);
    for (let i = 0; i < d.length; i++) buf[i] = d.charCodeAt(i) & 0xff;
    send(buf);
  });
  term.onResize(({ cols, rows }) => {
    if (rec) rec.events.push(JSON.stringify([Number(((performance.now() - rec.start) / 1000).toFixed(6)), "r", `${cols}x${rows}`]));
    if (resizeTimer) clearTimeout(resizeTimer);
    resizeTimer = setTimeout(() => {
      if (pid !== null) void api.ptyResize(pid, cols, rows).catch(() => undefined);
    }, 60);
  });
  term.onSelectionChange(() => {
    if (prefs.get()?.copy_on_select && term.hasSelection()) void writeClipboard(term.getSelection()).catch(() => undefined);
  });
  const copySelection = () => {
    if (term.hasSelection()) void copyText(term.getSelection(), "Copied selection");
  };
  const pasteClipboard = () => {
    void readClipboard().then((text) => (text ? pasteGuard(text) : undefined), (err) => fail(err, "Clipboard unavailable"));
  };
  term.attachCustomKeyEventHandler((e) => {
    if (e.type !== "keydown") return true;
    const mod = isMac ? e.metaKey && !e.ctrlKey && !e.altKey : e.ctrlKey && e.shiftKey && !e.altKey && !e.metaKey;
    const k = keyOf(e).toLowerCase();
    const handled = () => {
      e.preventDefault();
      e.stopPropagation();
      return false;
    };
    if ((mod && k === "c") || (e.key === "Insert" && e.ctrlKey && !e.shiftKey)) {
      copySelection();
      return handled();
    }
    if ((mod && k === "v") || (e.key === "Insert" && e.shiftKey && !e.ctrlKey)) {
      pasteClipboard();
      return handled();
    }
    if (mod && k === "b") {
      void toggleBroadcast(key);
      return handled();
    }
    if (mod && k === "r") {
      toggleRec();
      return handled();
    }
    if (mod && k === "f") {
      openFind();
      return handled();
    }
    if (isMac && e.metaKey && k === "k") return false;
    if (!pid && !overlay.hidden && e.key === "Enter") {
      void connect();
      return handled();
    }
    const chord = terminalChord(e, true);
    if (chord !== null) {
      term.input(chord);
      return handled();
    }
    return true;
  });
  hostEl.addEventListener("contextmenu", (e) => {
    e.preventDefault();
    const sc = isMac ? "⌘" : "Ctrl+Shift+";
    contextMenu(e.clientX, e.clientY, [
      { label: "Copy", icon: "copy", hint: `${sc}C`, disabled: !term.hasSelection(), run: copySelection },
      { label: "Paste", icon: "download", hint: `${sc}V`, run: pasteClipboard },
      { label: "Select all", icon: "check", run: () => term.selectAll() },
      null,
      { label: "Find", icon: "search", hint: `${sc}F`, run: openFind },
      { label: "Clear scrollback", icon: "trash", run: () => term.clear() },
    ]);
  });

  const ro = new ResizeObserver(() => requestAnimationFrame(refit));
  const unsubLink = links.subscribe((m) => {
    const s = m.get(host);
    if (!s || pid === null) return;
    if (s.state === "backoff" || s.state === "connecting") setTone("warn");
    else if (s.state === "up" && tone !== "ok") setTone("ok");
  }, false);
  const unsubPrefs = prefs.subscribe((np) => {
    if (!np || !opened) return;
    term.options.fontSize = np.font_size;
    term.options.lineHeight = np.line_height;
    term.options.fontFamily = withFallback(np.font_family);
    term.options.cursorBlink = np.cursor_blink;
    wakeCursor();
    term.options.cursorStyle = np.cursor_style as "block" | "bar" | "underline";
    term.options.scrollback = np.scrollback;
    const t = themeFor(np.theme).theme;
    if (term.options.theme?.background !== t.background || term.options.theme?.foreground !== t.foreground) {
      term.options.theme = { ...t };
      term.clearTextureAtlas();
      term.refresh(0, term.rows - 1);
    }
    el.style.background = t.background ?? "";
    refit();
  }, false);

  const open = () => {
    if (opened) return;
    opened = true;
    term.open(hostEl);
    term.textarea?.addEventListener(
      "paste",
      (e) => {
        e.preventDefault();
        e.stopImmediatePropagation();
        const text = e.clipboardData?.getData("text/plain") ?? "";
        if (text) void pasteGuard(text);
      },
      true,
    );
    attachGl();
    term.registerCharacterJoiner(rtlRuns);
    term.textarea?.addEventListener("focus", wakeCursor);
    wakeCursor();
    ro.observe(hostEl);
    refit();
    void fontsReady.then(() => {
      if (disposed) return;
      term.options.fontFamily = withFallback(prefs.get()?.font_family ?? "monospace");
      term.clearTextureAtlas();
      refit();
    });
    void connect();
  };

  live.set(key, {
    key,
    host,
    title: label,
    send: (text) => sendString(text),
    focus: () => term.focus(),
    renderer: () => (gl ? "webgl" : "dom"),
    text: () => {
      const b = term.buffer.active;
      const out: string[] = [];
      for (let i = 0; i < b.length; i++) out.push(b.getLine(i)?.translateToString(true) ?? "");
      return out.join("\n");
    },
  });

  return {
    key,
    title: label,
    icon: "terminal",
    host,
    el,
    tone: "warn",
    onShow() {
      open();
      attachGl();
      refit();
      term.focus();
    },
    onHide() {
      releaseGl();
    },
    dispose() {
      if (rec) void stopRec();
      broadcast.delete(key);
      bcastListeners.delete(renderBcast);
      bcastChanged();
      disposed = true;
      live.delete(key);
      unsubLink();
      unsubPrefs();
      ro.disconnect();
      tele.stop();
      if (glTimer) clearTimeout(glTimer);
      if (blinkTimer) clearTimeout(blinkTimer);
      if (pid !== null) void api.ptyClose(pid).catch(() => undefined);
      pid = null;
      term.dispose();
    },
  };
}
