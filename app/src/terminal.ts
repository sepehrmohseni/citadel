import "@xterm/xterm/css/xterm.css";
import { FitAddon } from "@xterm/addon-fit";
import { SearchAddon } from "@xterm/addon-search";
import { Unicode11Addon } from "@xterm/addon-unicode11";
import { WebLinksAddon } from "@xterm/addon-web-links";
import { WebglAddon } from "@xterm/addon-webgl";
import { Terminal } from "@xterm/xterm";

import { Channel, api, errCode, errText, events, type Id, type Listener, type Prefs, type PtyStatus } from "./api";
import { bytes, h, icon, terminalChord } from "./dom";
import { bind, bindings, pretty } from "./keymap";
import { forwardPort, PortsPanel } from "./ports";
import { currentTab, notifyLayout, openFiles, retitleTab, type Tab, type TabTone } from "./shell";
import { hostById, links, prefs, refreshHosts, updatePrefs } from "./store";
import { Telemetry } from "./telemetry";
import { themeFor } from "./themes";
import { actionToast, btn, confirmBox, contextMenu, copyText, fail, input, readClipboard, toast, writeClipboard } from "./ui";

const encoder = new TextEncoder();
const ACK_STEP = 256 * 1024;
const BLINK_IDLE_MS = 10_000;
const PORT_WATCH_MS = 15_000;
const RTL = "֐-ࣿיִ-﷿ﹰ-ﻼ";
const HAS_RTL = new RegExp(`[${RTL}]`);
const RTL_RUN = new RegExp(`[${RTL}](?:[${RTL}‌‍٠-٩۰-۹ .,:;!?()«»-]*[${RTL}])?`, "g");
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

function hint(id: string): string {
  const b = bindings(id)[0];
  return b ? ` (${pretty(b).join(" ")})` : "";
}

interface Recording {
  start: number;
  cols: number;
  rows: number;
  events: string[];
  decoder: TextDecoder;
}

export interface Pane {
  key: string;
  host: Id;
  label: string;
  el: HTMLElement;
  tone(): TabTone;
  connecting(): boolean;
  banner(text: string): void;
  focus(): void;
  show(): void;
  hide(): void;
  refit(): void;
  dispose(): void;
  copy(): void;
  paste(): void;
  find(): void;
  selectAll(): void;
  clear(): void;
  broadcast(): void;
  record(): void;
  telemetry(): void;
  ports(): void;
  files(): void;
  send(text: string): void;
  text(): string;
  renderer(): string;
}

const live = new Map<string, Pane>();
const byEl = new WeakMap<Element, Pane>();
const broadcast = new Set<string>();
const bcastListeners = new Set<() => void>();
let bcastWarned = false;
let seq = 0;
let lastPaneKey: string | null = null;
const announced = new Set<string>();

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
  return [...live.values()].map(({ key, label, host }) => ({ key, title: label, host }));
}

export function sendText(key: string, text: string): void {
  live.get(key)?.send(text);
}

export function activeTerminalKey(): string | null {
  const focused = paneOf(document.activeElement);
  if (focused) return focused.key;
  if (lastPaneKey && live.has(lastPaneKey)) return lastPaneKey;
  return null;
}

function paneOf(node: Element | null): Pane | undefined {
  let el: Element | null = node;
  while (el) {
    const p = byEl.get(el);
    if (p) return p;
    el = el.parentElement;
  }
  return undefined;
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

function createPane(host: Id, changed: () => void): Pane {
  const meta = hostById(host);
  const label = meta?.label ?? "host";
  const key = `pane:${host}:${++seq}`;
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
  const portsPanel = new PortsPanel(host, label);
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
    try {
      const target = await api.pickSave("Save recording", `${label.replace(/[^\w.-]+/g, "_")}-${stamp}.cast`, [{ name: "asciicast", extensions: ["cast"] }]);
      if (!target) return;
      const size = await api.recordingSave(target.token, `${header}\n${r.events.join("\n")}\n`);
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
    btn("", { icon: "search", small: true, ghost: true, title: `Find${hint("term.find")}`, onClick: () => openFind() }),
    btn("", { icon: "folder", small: true, ghost: true, title: `Open files${hint("term.files")}`, onClick: () => openFiles(host) }),
    btn("", { icon: "plug", small: true, ghost: true, title: `Remote ports${hint("term.ports")}`, onClick: () => togglePorts() }),
    btn("", { icon: "chart", small: true, ghost: true, title: `Host telemetry${hint("term.telemetry")}`, onClick: () => toggleTelemetry() }),
    recEl,
    bcastEl,
    btn("", { icon: "broadcast", small: true, ghost: true, title: `Broadcast input${hint("term.broadcast")}`, onClick: () => void toggleBroadcast(key) }),
    btn("", { icon: "stop", small: true, ghost: true, title: `Record session${hint("term.record")}`, onClick: toggleRec }),
    btn("", { icon: "split", small: true, ghost: true, title: `Split right${hint("pane.right")}`, onClick: () => splitFrom(api_, "row") }),
    btn("", { icon: "refresh", small: true, ghost: true, title: "Reconnect", onClick: () => void reconnect() }),
  );
  const main = h("div", { class: "term-main" }, bar, hostEl, overlay, findBox);
  const el = h("div", { class: "term-pane", style: { background: themeFor(p?.theme).theme.background ?? "" } }, main, portsPanel.el, tele.el);

  let gl: WebglAddon | null = null;
  let glTimer: ReturnType<typeof setTimeout> | undefined;
  let visible = false;
  let pid: number | null = null;
  let opened = false;
  let disposed = false;
  let exitCode: number | null = null;
  let outbox: Uint8Array[] = [];
  let flushing = false;
  let resizeTimer: ReturnType<typeof setTimeout> | undefined;
  let tone: TabTone = "idle";
  let connecting = false;
  let attempt = 0;
  let userTurn = false;
  let blinkTimer: ReturnType<typeof setTimeout> | undefined;
  let watchTimer: ReturnType<typeof setInterval> | undefined;
  let knownPorts: Set<number> | null = null;

  const attachGl = () => {
    if (glTimer) clearTimeout(glTimer);
    if (gl || disposed || !opened || !wantsGl()) return;
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
      if (!gl || visible) return;
      gl.dispose();
      gl = null;
    }, 30000);
  };

  const setTone = (t: TabTone) => {
    tone = t;
    toneDot.className = `dot ${t === "ok" ? "ok" : t === "warn" ? "warn pulse" : t === "err" ? "err" : "idle"}`;
    changed();
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
    if (!visible || !opened) return;
    try {
      fit.fit();
    } catch {
      return;
    }
  };

  const failed = (e: unknown) => {
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
  };

  const connect = async () => {
    if (disposed || connecting) return;
    connecting = true;
    const mine = ++attempt;
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
      if (mine !== attempt) return;
      const chunk = new Uint8Array(buf);
      term.write(chunk, () => {
        processed += chunk.length;
        if (processed - acked >= ACK_STEP) ack();
      });
      if (rec) rec.events.push(JSON.stringify([Number(((performance.now() - rec.start) / 1000).toFixed(6)), "o", rec.decoder.decode(chunk, { stream: true })]));
    };
    const status = new Channel<PtyStatus>();
    status.onmessage = (s) => {
      if (mine !== attempt) return;
      if (s.kind === "exit") exitCode = s.code;
      else if (s.kind === "signal") term.write(`\r\n\x1b[2m[terminated by ${s.name}]\x1b[0m\r\n`);
      else ended();
    };
    try {
      refit();
      const opened_ = await api.ptyOpen(host, Math.max(term.cols, 2), Math.max(term.rows, 2), data, status);
      if (disposed || mine !== attempt) {
        void api.ptyClose(opened_).catch(() => undefined);
        return;
      }
      pid = opened_;
      ackFor = pid;
      ack();
      hideOverlay();
      setTone("ok");
      if (visible) term.focus();
      armPortWatch();
    } catch (e) {
      if (mine !== attempt) return;
      pid = null;
      setTone("err");
      failed(e);
    } finally {
      if (mine === attempt) connecting = false;
    }
  };

  const ended = () => {
    pid = null;
    outbox = [];
    stopPortWatch();
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
      attempt++;
      connecting = false;
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

  const asUser = (fn: () => void) => {
    userTurn = true;
    try {
      fn();
    } finally {
      userTurn = false;
    }
  };

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
    asUser(() => term.paste(text));
    term.focus();
  };

  const wakeCursor = () => {
    if (!(prefs.get()?.cursor_blink ?? true)) return;
    if (!term.options.cursorBlink) term.options.cursorBlink = true;
    if (blinkTimer) clearTimeout(blinkTimer);
    blinkTimer = setTimeout(() => {
      term.options.cursorBlink = false;
    }, BLINK_IDLE_MS);
  };

  const stopPortWatch = () => {
    if (watchTimer) clearInterval(watchTimer);
    watchTimer = undefined;
  };

  const watchPorts = async () => {
    if (!visible || pid === null || document.hidden || portsPanel.open) return;
    let list: Listener[];
    try {
      list = await api.ports(host);
    } catch {
      stopPortWatch();
      return;
    }
    const seen = new Set(list.map((l) => l.port));
    if (knownPorts) {
      for (const l of list) {
        if (knownPorts.has(l.port) || l.port < 1024 || l.scope === "address") continue;
        const tag = `${host}:${l.port}:${l.pid ?? ""}`;
        if (announced.has(tag)) continue;
        announced.add(tag);
        actionToast(`port ${l.port}${l.process ? ` (${l.process})` : ""} opened on ${label}`, [
          { label: "forward & open", run: () => void forwardPort(host, label, l, true).catch((e) => fail(e, "Forward failed")) },
        ]);
      }
    }
    knownPorts = seen;
  };

  const armPortWatch = () => {
    stopPortWatch();
    knownPorts = null;
    if (!(prefs.get()?.detect_ports ?? true)) return;
    void watchPorts();
    watchTimer = setInterval(() => void watchPorts(), PORT_WATCH_MS);
  };

  const toggleTelemetry = () => {
    void tele.toggle().then(() => refit());
  };

  const togglePorts = () => {
    portsPanel.toggle();
    requestAnimationFrame(refit);
  };

  const copySelection = () => {
    if (term.hasSelection()) void copyText(term.getSelection(), "Copied selection");
  };

  const pasteClipboard = () => {
    void readClipboard().then((text) => (text ? pasteGuard(text) : undefined), (err) => fail(err, "Clipboard unavailable"));
  };

  const latch = { ctrl: false, alt: false };
  const latchEls: HTMLElement[] = [];
  const applyLatch = (d: string): string => {
    if ((!latch.ctrl && !latch.alt) || d.startsWith("\x1b")) return d;
    let out = d;
    if (latch.ctrl && d.length === 1) {
      const c = d.toLowerCase().charCodeAt(0);
      if (c >= 97 && c <= 122) out = String.fromCharCode(c - 96);
      else if (d === " " || d === "@") out = "\x00";
      else if ("[\\]^_".includes(d)) out = String.fromCharCode(d.charCodeAt(0) - 64);
    }
    if (latch.alt) out = `\x1b${out}`;
    latch.ctrl = latch.alt = false;
    for (const el of latchEls) el.classList.remove("on");
    return out;
  };
  const arrow = (k: string) => () => (term.modes.applicationCursorKeysMode ? `\x1bO${k}` : `\x1b[${k}`);
  const touchKeys: [string, string | (() => string)][] = [
    ["esc", "\x1b"],
    ["tab", "\t"],
    ["ctrl", "ctrl"],
    ["alt", "alt"],
    ["←", arrow("D")],
    ["↑", arrow("A")],
    ["↓", arrow("B")],
    ["→", arrow("C")],
    ["|", "|"],
    ["~", "~"],
    ["/", "/"],
    ["-", "-"],
  ];
  const keysEl = h(
    "div",
    { class: "term-keys", role: "toolbar", "aria-label": "Terminal keys" },
    ...touchKeys.map(([label, out]) => {
      const b = h("button", { class: "term-key", type: "button", tabindex: "-1", dataset: { key: label } }, label);
      b.addEventListener("pointerdown", (e) => {
        e.preventDefault();
        if (out === "ctrl" || out === "alt") {
          latch[out] = !latch[out];
          b.classList.toggle("on", latch[out]);
        } else {
          const seq = typeof out === "function" ? out() : out;
          asUser(() => term.input(seq, true));
        }
        term.focus();
      });
      if (out === "ctrl" || out === "alt") latchEls.push(b);
      return b;
    }),
  );
  main.append(keysEl);

  term.onData((raw) => {
    const d = applyLatch(raw);
    wakeCursor();
    sendString(d);
    if (userTurn && broadcast.has(key)) {
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
  term.attachCustomKeyEventHandler((e) => {
    if (e.type !== "keydown") return true;
    if (!pid && !overlay.hidden && e.key === "Enter") {
      e.preventDefault();
      void connect();
      return false;
    }
    const chord = terminalChord(e, true);
    if (chord !== null) {
      e.preventDefault();
      asUser(() => term.input(chord));
      return false;
    }
    return true;
  });
  hostEl.addEventListener(
    "keydown",
    () => {
      userTurn = true;
      queueMicrotask(() => {
        userTurn = false;
      });
    },
    true,
  );
  for (const ev of ["compositionend", "input"]) {
    hostEl.addEventListener(
      ev,
      () => {
        userTurn = true;
        queueMicrotask(() => {
          userTurn = false;
        });
      },
      true,
    );
  }
  hostEl.addEventListener("contextmenu", (e) => {
    e.preventDefault();
    contextMenu(e.clientX, e.clientY, [
      { label: "Copy", icon: "copy", hint: pretty(bindings("term.copy")[0] ?? "")[0], disabled: !term.hasSelection(), run: copySelection },
      { label: "Paste", icon: "download", hint: pretty(bindings("term.paste")[0] ?? "")[0], run: pasteClipboard },
      { label: "Select all", icon: "check", run: () => term.selectAll() },
      null,
      { label: "Split right", icon: "split", hint: pretty(bindings("pane.right")[0] ?? "")[0], run: () => splitFrom(api_, "row") },
      { label: "Split down", icon: "split", hint: pretty(bindings("pane.down")[0] ?? "")[0], run: () => splitFrom(api_, "col") },
      null,
      { label: "Find", icon: "search", hint: pretty(bindings("term.find")[0] ?? "")[0], run: openFind },
      { label: "Clear scrollback", icon: "trash", run: () => term.clear() },
    ]);
  });
  el.addEventListener("focusin", () => {
    lastPaneKey = key;
    changed();
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
    if (!np.detect_ports) stopPortWatch();
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

  const api_: Pane = {
    key,
    host,
    label,
    el,
    tone: () => tone,
    connecting: () => connecting,
    banner: (text) => {
      const clean = text.replace(/[\x00-\x09\x0b-\x1f\x7f]/g, "").replace(/\n/g, "\r\n");
      term.write(`\x1b[2m${clean}\x1b[0m\r\n`);
    },
    focus: () => term.focus(),
    show: () => {
      visible = true;
      open();
      attachGl();
      refit();
      if (pid !== null && !watchTimer) armPortWatch();
    },
    hide: () => {
      visible = false;
      releaseGl();
    },
    refit,
    dispose: () => {
      if (rec) void stopRec();
      broadcast.delete(key);
      bcastListeners.delete(renderBcast);
      bcastChanged();
      disposed = true;
      attempt++;
      live.delete(key);
      unsubLink();
      unsubPrefs();
      ro.disconnect();
      tele.dispose();
      portsPanel.dispose();
      stopPortWatch();
      if (glTimer) clearTimeout(glTimer);
      if (blinkTimer) clearTimeout(blinkTimer);
      if (pid !== null) void api.ptyClose(pid).catch(() => undefined);
      pid = null;
      term.dispose();
    },
    copy: copySelection,
    paste: pasteClipboard,
    find: openFind,
    selectAll: () => term.selectAll(),
    clear: () => term.clear(),
    broadcast: () => void toggleBroadcast(key),
    record: toggleRec,
    telemetry: toggleTelemetry,
    ports: togglePorts,
    files: () => openFiles(host),
    send: (text) => sendString(text),
    text: () => {
      const b = term.buffer.active;
      const out: string[] = [];
      for (let i = 0; i < b.length; i++) out.push(b.getLine(i)?.translateToString(true) ?? "");
      return out.join("\n");
    },
    renderer: () => (gl ? "webgl" : "dom"),
  };
  byEl.set(el, api_);
  live.set(key, api_);
  return api_;
}

export type LayoutSpec = { pane: Id } | { dir: "row" | "col"; ratio: number; a: LayoutSpec; b: LayoutSpec };

type Tree = { kind: "leaf"; pane: Pane } | { kind: "split"; dir: "row" | "col"; ratio: number; a: Tree; b: Tree };

interface Layout {
  split(from: Pane, dir: "row" | "col"): void;
  closePane(p: Pane): boolean;
  cycle(from: Pane, step: number): void;
}

const layouts = new WeakMap<Pane, Layout>();

function splitFrom(p: Pane, dir: "row" | "col"): void {
  layouts.get(p)?.split(p, dir);
}

function leaves(n: Tree): Pane[] {
  return n.kind === "leaf" ? [n.pane] : [...leaves(n.a), ...leaves(n.b)];
}

function validSpec(spec: unknown, depth = 0): spec is LayoutSpec {
  if (!spec || typeof spec !== "object" || depth > 6) return false;
  const s = spec as Record<string, unknown>;
  if (typeof s.pane === "string") return true;
  return (s.dir === "row" || s.dir === "col") && typeof s.ratio === "number" && validSpec(s.a, depth + 1) && validSpec(s.b, depth + 1);
}

export function layoutTab(spec: LayoutSpec): Tab | null {
  if (!validSpec(spec)) return null;
  const key = `term:${++seq}`;
  const el = h("div", { class: "term-layout" });
  let visible = false;
  let focused: Pane | null = null;
  const changed = () => {
    const all = leaves(root);
    const now = all.find((x) => x.key === lastPaneKey);
    if (now) focused = now;
    for (const x of all) x.el.classList.toggle("focused", all.length > 1 && x === focused);
    const tones = all.map((x) => x.tone());
    const tone: TabTone = tones.includes("err") ? "err" : tones.includes("warn") ? "warn" : tones.includes("ok") ? "ok" : "idle";
    const head = focused ?? all[0];
    const extra = all.length > 1 ? ` +${all.length - 1}` : "";
    retitleTab(key, `${head?.label ?? "host"}${extra}`, tone);
  };
  const build = (s: LayoutSpec): Tree | null => {
    if ("pane" in s) {
      if (!hostById(s.pane)) return null;
      const pane = createPane(s.pane, () => changed());
      layouts.set(pane, layout);
      return { kind: "leaf", pane };
    }
    const a = build(s.a);
    const b = build(s.b);
    if (a && b) return { kind: "split", dir: s.dir, ratio: Math.min(0.85, Math.max(0.15, s.ratio)), a, b };
    return a ?? b;
  };
  const toSpec = (n: Tree): LayoutSpec => (n.kind === "leaf" ? { pane: n.pane.host } : { dir: n.dir, ratio: Math.round(n.ratio * 100) / 100, a: toSpec(n.a), b: toSpec(n.b) });
  const refitAll = () => requestAnimationFrame(() => leaves(root).forEach((x) => x.refit()));
  const render = (n: Tree): HTMLElement => {
    if (n.kind === "leaf") return n.pane.el;
    const a = render(n.a);
    const b = render(n.b);
    const wrapA = h("div", { class: "split-cell" }, a);
    const wrapB = h("div", { class: "split-cell" }, b);
    const apply = () => {
      wrapA.style.flex = `${n.ratio} 1 0`;
      wrapB.style.flex = `${1 - n.ratio} 1 0`;
    };
    apply();
    const box = h("div", { class: `split split-${n.dir}` });
    const divider = h("div", {
      class: "split-divider",
      role: "separator",
      "aria-orientation": n.dir === "row" ? "vertical" : "horizontal",
      onPointerdown: (e: PointerEvent) => {
        e.preventDefault();
        divider.setPointerCapture(e.pointerId);
        const rect = box.getBoundingClientRect();
        const move = (ev: PointerEvent) => {
          const r = n.dir === "row" ? (ev.clientX - rect.left) / rect.width : (ev.clientY - rect.top) / rect.height;
          n.ratio = Math.min(0.85, Math.max(0.15, r));
          apply();
          refitAll();
        };
        const up = () => {
          divider.removeEventListener("pointermove", move);
          divider.removeEventListener("pointerup", up);
          notifyLayout();
        };
        divider.addEventListener("pointermove", move);
        divider.addEventListener("pointerup", up);
      },
    });
    box.append(wrapA, divider, wrapB);
    return box;
  };
  const redraw = () => {
    el.replaceChildren(render(root));
    if (visible) for (const x of leaves(root)) x.show();
    changed();
    refitAll();
  };
  const replace = (n: Tree, target: Pane, make: (leaf: Tree) => Tree | null): Tree | null => {
    if (n.kind === "leaf") return n.pane === target ? make(n) : n;
    const a = replace(n.a, target, make);
    const b = replace(n.b, target, make);
    if (a && b) return { ...n, a, b };
    return a ?? b;
  };
  const layout: Layout = {
    split(from, dir) {
      const pane = createPane(from.host, () => changed());
      layouts.set(pane, layout);
      const next = replace(root, from, (leaf) => ({ kind: "split", dir, ratio: 0.5, a: leaf, b: { kind: "leaf", pane } }));
      if (!next) return;
      root = next;
      lastPaneKey = pane.key;
      redraw();
      requestAnimationFrame(() => pane.focus());
      notifyLayout();
    },
    closePane(target) {
      if (root.kind === "leaf") return false;
      const next = replace(root, target, () => null);
      if (!next) return false;
      target.dispose();
      root = next;
      const first = leaves(root)[0];
      if (first) lastPaneKey = first.key;
      redraw();
      requestAnimationFrame(() => first?.focus());
      notifyLayout();
      return true;
    },
    cycle(from, step) {
      const all = leaves(root);
      const i = all.indexOf(from);
      const next = all[(i + step + all.length) % all.length];
      next?.focus();
    },
  };
  const built = build(spec);
  if (!built) return null;
  let root: Tree = built;
  el.append(render(root));
  const first = leaves(root)[0];
  focused = first ?? null;
  return {
    key,
    title: first?.label ?? "host",
    icon: "terminal",
    host: first?.host,
    el,
    tone: "warn",
    snapshot: () => ({ kind: "term", layout: toSpec(root) }),
    closePane: () => {
      const target = leaves(root).find((x) => x.key === lastPaneKey && x.el.contains(document.activeElement)) ?? null;
      return target ? layout.closePane(target) : false;
    },
    onShow() {
      visible = true;
      for (const x of leaves(root)) x.show();
      refitAll();
      requestAnimationFrame(() => (focused ?? leaves(root)[0])?.focus());
    },
    onHide() {
      visible = false;
      for (const x of leaves(root)) x.hide();
    },
    dispose() {
      for (const x of leaves(root)) x.dispose();
    },
  };
}

export function terminalTab(host: Id): Tab {
  const tab = layoutTab({ pane: host });
  if (tab) return tab;
  const el = h("div", { class: "term-layout" });
  return { key: `term:missing:${++seq}`, title: "missing host", icon: "terminal", el, dispose: () => undefined };
}

function focusedPane(): Pane | undefined {
  return paneOf(document.activeElement);
}

const onPane = (fn: (p: Pane) => void) => () => {
  const p = focusedPane();
  if (!p) return false;
  fn(p);
  return true;
};

bind("term.copy", onPane((p) => p.copy()));
bind("term.paste", onPane((p) => p.paste()));
bind("term.find", onPane((p) => p.find()));
bind("term.selectAll", onPane((p) => p.selectAll()));
bind("term.clear", onPane((p) => p.clear()));
bind("term.broadcast", onPane((p) => p.broadcast()));
bind("term.record", onPane((p) => p.record()));
bind("term.telemetry", onPane((p) => p.telemetry()));
bind("term.ports", onPane((p) => p.ports()));
bind("term.files", onPane((p) => p.files()));
bind("pane.right", onPane((p) => splitFrom(p, "row")));
bind("pane.down", onPane((p) => splitFrom(p, "col")));
bind("pane.next", onPane((p) => layouts.get(p)?.cycle(p, 1)));
bind("pane.prev", onPane((p) => layouts.get(p)?.cycle(p, -1)));

const zoom = (delta: number) => () => {
  const cur = prefs.get();
  if (!cur || !currentTab()?.key.startsWith("term:")) return false;
  const size = delta === 0 ? 13 : Math.min(32, Math.max(8, cur.font_size + delta));
  void updatePrefs({ font_size: size }).catch((e) => fail(e));
  return true;
};
bind("zoom.in", zoom(1));
bind("zoom.out", zoom(-1));
bind("zoom.reset", zoom(0));

void events.banner((label, text) => {
  for (const p of live.values()) if (p.label === label && p.connecting()) p.banner(text);
});
