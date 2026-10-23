import { isMac, keyOf } from "./dom";

export type Scope = "app" | "terminal" | "hosts";

export interface Command {
  id: string;
  label: string;
  group: string;
  scope: Scope;
  mac: string[];
  other: string[];
}

const both = (...chords: string[]) => ({ mac: chords.map((c) => c.replace(/\bMod\b/g, "Meta")), other: chords.map((c) => c.replace(/\bMod\b/g, "Ctrl")) });

function cmd(id: string, label: string, group: string, scope: Scope, keys: { mac: string[]; other: string[] }): Command {
  return { id, label, group, scope, ...keys };
}

export const COMMANDS: Command[] = [
  cmd("palette", "Command palette", "General", "app", { mac: ["Meta+K"], other: ["Ctrl+K", "Ctrl+Shift+K"] }),
  cmd("help", "Keyboard sheet", "General", "app", both("Shift+/")),
  cmd("lock", "Lock vault", "General", "app", both("Mod+Shift+L")),
  cmd("zoom.in", "Larger terminal text", "General", "app", { mac: ["Meta+="], other: ["Ctrl+=", "Ctrl+Shift+="] }),
  cmd("zoom.out", "Smaller terminal text", "General", "app", { mac: ["Meta+-"], other: ["Ctrl+-", "Ctrl+Shift+-"] }),
  cmd("zoom.reset", "Reset terminal text size", "General", "app", both("Mod+0")),
  cmd("go.hosts", "Go to hosts", "Navigate", "app", both("G H")),
  cmd("go.fleet", "Go to fleet run", "Navigate", "app", both("G R")),
  cmd("go.tunnels", "Go to tunnels", "Navigate", "app", both("G T")),
  cmd("go.keys", "Go to keys", "Navigate", "app", both("G K")),
  cmd("go.snippets", "Go to snippets", "Navigate", "app", both("G S")),
  cmd("go.audit", "Go to audit", "Navigate", "app", both("G A")),
  cmd("go.settings", "Go to settings", "Navigate", "app", both("G ,")),
  cmd("tab.next", "Next tab", "Tabs", "app", both("Ctrl+Tab")),
  cmd("tab.prev", "Previous tab", "Tabs", "app", both("Ctrl+Shift+Tab")),
  cmd("tab.close", "Close pane or tab", "Tabs", "app", { mac: ["Meta+W"], other: ["Ctrl+W", "Ctrl+Shift+W"] }),
  cmd("tab.reopen", "Reopen closed terminal", "Tabs", "app", both("Mod+Shift+T")),
  ...Array.from({ length: 9 }, (_, i) => cmd(`tab.${i + 1}`, `Go to tab ${i + 1}`, "Tabs", "app", both(`Mod+${i + 1}`))),
  cmd("term.copy", "Copy selection", "Terminal", "terminal", { mac: ["Meta+C"], other: ["Ctrl+Shift+C", "Ctrl+Insert"] }),
  cmd("term.paste", "Paste (guarded)", "Terminal", "terminal", { mac: ["Meta+V"], other: ["Ctrl+Shift+V", "Shift+Insert"] }),
  cmd("term.find", "Find in scrollback", "Terminal", "terminal", { mac: ["Meta+F"], other: ["Ctrl+Shift+F"] }),
  cmd("term.selectAll", "Select all", "Terminal", "terminal", { mac: ["Meta+A"], other: ["Ctrl+Shift+A"] }),
  cmd("term.clear", "Clear scrollback", "Terminal", "terminal", { mac: ["Meta+Shift+K"], other: [] }),
  cmd("term.broadcast", "Broadcast input", "Terminal", "terminal", { mac: ["Meta+Shift+B"], other: ["Ctrl+Shift+B"] }),
  cmd("term.record", "Record session", "Terminal", "terminal", { mac: ["Meta+Shift+R"], other: ["Ctrl+Shift+R"] }),
  cmd("term.telemetry", "Host telemetry panel", "Terminal", "terminal", { mac: ["Meta+Shift+M"], other: ["Ctrl+Shift+M"] }),
  cmd("term.ports", "Remote ports panel", "Terminal", "terminal", { mac: ["Meta+Shift+O"], other: ["Ctrl+Shift+O"] }),
  cmd("term.files", "Open files for this host", "Terminal", "terminal", { mac: ["Meta+Shift+E"], other: ["Ctrl+Shift+U"] }),
  cmd("pane.right", "Split right", "Panes", "terminal", { mac: ["Meta+D"], other: ["Ctrl+Shift+D"] }),
  cmd("pane.down", "Split down", "Panes", "terminal", { mac: ["Meta+Shift+D"], other: ["Ctrl+Shift+E"] }),
  cmd("pane.next", "Next pane", "Panes", "terminal", { mac: ["Meta+]"], other: ["Ctrl+Shift+]"] }),
  cmd("pane.prev", "Previous pane", "Panes", "terminal", { mac: ["Meta+["], other: ["Ctrl+Shift+["] }),
  cmd("host.down", "Move down", "Hosts list", "hosts", both("J", "Down")),
  cmd("host.up", "Move up", "Hosts list", "hosts", both("K", "Up")),
  cmd("host.connect", "Connect", "Hosts list", "hosts", both("Enter")),
  cmd("host.files", "Open files", "Hosts list", "hosts", both("F")),
  cmd("host.edit", "Edit host", "Hosts list", "hosts", both("E")),
  cmd("host.tunnel", "New tunnel", "Hosts list", "hosts", both("T")),
  cmd("host.delete", "Delete host", "Hosts list", "hosts", both("D")),
  cmd("host.new", "New host", "Hosts list", "hosts", both("N")),
  cmd("host.import", "Import ~/.ssh/config", "Hosts list", "hosts", both("I")),
  cmd("host.filter", "Filter", "Hosts list", "hosts", both("/")),
  cmd("host.fleet", "Run on selected hosts", "Hosts list", "hosts", both("R")),
];

const byId = new Map(COMMANDS.map((c) => [c.id, c]));

let overrides: Record<string, string[]> = {};
const listeners = new Set<() => void>();

export function setOverrides(next: Record<string, string[]> | undefined): void {
  overrides = { ...(next ?? {}) };
  for (const fn of [...listeners]) fn();
}

export function getOverrides(): Record<string, string[]> {
  return { ...overrides };
}

export function onKeymapChange(fn: () => void): () => void {
  listeners.add(fn);
  return () => listeners.delete(fn);
}

export function defaults(id: string): string[] {
  const c = byId.get(id);
  if (!c) return [];
  return isMac ? c.mac : c.other;
}

export function bindings(id: string): string[] {
  return overrides[id] ?? defaults(id);
}

export function isDefault(id: string): boolean {
  return !(id in overrides);
}

const MODS = ["Ctrl", "Alt", "Shift", "Meta"] as const;
const PUNCT_CODES: Record<string, string> = {
  Backquote: "`",
  Minus: "-",
  Equal: "=",
  BracketLeft: "[",
  BracketRight: "]",
  Backslash: "\\",
  IntlBackslash: "\\",
  Semicolon: ";",
  Quote: "'",
  Comma: ",",
  Period: ".",
  Slash: "/",
};
const NAMED: Record<string, string> = {
  " ": "Space",
  ArrowUp: "Up",
  ArrowDown: "Down",
  ArrowLeft: "Left",
  ArrowRight: "Right",
  Esc: "Escape",
};
const TYPABLE = new Set(["Space", "Enter", "Tab", "Backspace", "Delete", "Up", "Down", "Left", "Right", "Home", "End", "PageUp", "PageDown", "Escape"]);
const SHIFTED: Record<string, string> = { "/": "?", "=": "+", "-": "_", ";": ":", "'": '"', ",": "<", ".": ">", "[": "{", "]": "}", "\\": "|", "`": "~" };

export function baseKey(e: KeyboardEvent): string | null {
  if (["Control", "Shift", "Alt", "Meta", "AltGraph", "CapsLock", "OS", "Fn", "Dead", "Unidentified", "Process"].includes(e.key)) return null;
  const letter = /^Key([A-Z])$/.exec(e.code)?.[1];
  if (letter) {
    const k = keyOf(e);
    return /^[a-z]$/i.test(k) ? k.toUpperCase() : letter;
  }
  const digit = /^(?:Digit|Numpad)([0-9])$/.exec(e.code)?.[1];
  if (digit) return digit;
  const punct = PUNCT_CODES[e.code];
  if (punct) return punct;
  if (/^F([1-9]|1[0-9]|2[0-4])$/.test(e.key)) return e.key;
  const named = NAMED[e.key] ?? e.key;
  if (named.length === 1) return named.toUpperCase();
  return named;
}

const UNSHIFT: Record<string, string> = Object.fromEntries(Object.entries(SHIFTED).map(([k, v]) => [v, k]));

export function chordOf(e: KeyboardEvent): string | null {
  let key = baseKey(e);
  if (!key) return null;
  let shift = e.shiftKey;
  const plain = !e.code ? UNSHIFT[key] : undefined;
  if (plain) {
    key = plain;
    shift = true;
  }
  const mods = MODS.filter((m) => (m === "Ctrl" ? e.ctrlKey : m === "Alt" ? e.altKey : m === "Shift" ? shift : e.metaKey));
  return [...mods, key].join("+");
}

const MOD_ALIASES: Record<string, (typeof MODS)[number]> = { ctrl: "Ctrl", control: "Ctrl", alt: "Alt", option: "Alt", shift: "Shift", meta: "Meta", cmd: "Meta", command: "Meta", super: "Meta" };

export function normalize(chord: string): string {
  const parts = chord.split("+").map((p) => p.trim()).filter(Boolean);
  const key = parts.pop() ?? "";
  const named = new Set(parts.map((p) => MOD_ALIASES[p.toLowerCase()]).filter(Boolean));
  const mods = MODS.filter((m) => named.has(m));
  return [...mods, key.length === 1 ? key.toUpperCase() : key].join("+");
}

function split(chord: string): { mods: Set<string>; key: string } {
  const parts = chord.split("+");
  const key = parts.pop() ?? "";
  return { mods: new Set(parts), key };
}

export function pretty(chord: string): string[] {
  return chord.split(" ").map((step) => {
    const { mods, key } = split(step);
    if (mods.size === 1 && mods.has("Shift") && SHIFTED[key]) return SHIFTED[key] as string;
    const names = [...mods].map((m) => (isMac ? { Ctrl: "⌃", Alt: "⌥", Shift: "⇧", Meta: "⌘" }[m] ?? m : m === "Meta" ? "Super" : m));
    const k = { Up: "↑", Down: "↓", Left: "←", Right: "→", Enter: "⏎", Space: "Space" }[key] ?? key;
    return isMac ? [...names, k].join("") : [...names, k].join("+");
  });
}

interface Context {
  terminal: boolean;
  typing: boolean;
  buttonish: boolean;
}

function context(): Context {
  const a = document.activeElement as HTMLElement | null;
  const terminal = !!a?.closest(".xterm");
  const typing = !terminal && !!a && (a.tagName === "INPUT" || a.tagName === "TEXTAREA" || a.tagName === "SELECT" || a.isContentEditable || !!a.closest("[data-typeahead]"));
  const buttonish = !!a && !!a.closest("button, a, summary, [role=button], [role=tab], [role=option], [role=menuitem], [role=checkbox]");
  return { terminal, typing, buttonish };
}

export function allowed(chord: string, scope: Scope, ctx: Context): boolean {
  const { mods, key } = split(chord);
  const commandish = mods.has("Ctrl") || mods.has("Alt") || mods.has("Meta");
  const printable = key.length === 1 || TYPABLE.has(key);
  if (!commandish && printable) {
    if (ctx.typing || ctx.terminal) return false;
    if ((key === "Enter" || key === "Space") && ctx.buttonish) return false;
  }
  if (ctx.terminal) {
    const reservedKey = /^[A-Z]$/.test(key) || ["[", "]", "\\", "/", "-", "Space"].includes(key);
    if (!isMac && reservedKey && !mods.has("Shift") && (mods.has("Ctrl") || mods.has("Alt"))) return false;
    if (!isMac && mods.has("Ctrl") && mods.has("Alt")) return false;
    if (isMac && mods.has("Alt") && !mods.has("Meta") && !mods.has("Ctrl")) return false;
  }
  if (scope === "terminal" && !ctx.terminal) return false;
  return true;
}

type Handler = () => boolean | void;
const handlers = new Map<string, Handler[]>();

export function bind(id: string, fn: Handler): () => void {
  const list = handlers.get(id) ?? [];
  list.push(fn);
  handlers.set(id, list);
  return () => {
    const cur = handlers.get(id) ?? [];
    handlers.set(
      id,
      cur.filter((h) => h !== fn),
    );
  };
}

export function run(id: string): boolean {
  const list = handlers.get(id) ?? [];
  for (let i = list.length - 1; i >= 0; i--) {
    const fn = list[i];
    if (fn && fn() !== false) return true;
  }
  return false;
}

let suspended = 0;

export function suspend(): () => void {
  suspended++;
  let done = false;
  return () => {
    if (!done) {
      done = true;
      suspended--;
    }
  };
}

let pending: { steps: string[]; at: number } | null = null;
const SEQUENCE_MS = 1200;

export function conflicts(chord: string, except: string): Command[] {
  const n = normalize(chord);
  return COMMANDS.filter((c) => c.id !== except && bindings(c.id).some((b) => b === n || b.startsWith(`${n} `) || n.startsWith(`${b} `)));
}

export function install(blocked: () => boolean): () => void {
  const onKey = (e: KeyboardEvent) => {
    if (suspended || e.defaultPrevented || e.isComposing) return;
    if (blocked()) return;
    const chord = chordOf(e);
    if (!chord) return;
    const ctx = context();
    const live = pending && Date.now() - pending.at < SEQUENCE_MS ? pending.steps : [];
    const attempt = (steps: string[]): "done" | "wait" | "none" => {
      const seq = steps.join(" ");
      let prefix = false;
      for (const c of COMMANDS) {
        for (const b of bindings(c.id)) {
          const first = b.split(" ")[0] ?? b;
          if (!allowed(first, c.scope, ctx)) continue;
          if (b === seq) {
            if (run(c.id)) return "done";
          } else if (b.startsWith(`${seq} `)) {
            prefix = true;
          }
        }
      }
      return prefix ? "wait" : "none";
    };
    let steps = [...live, chord];
    let outcome = attempt(steps);
    if (outcome === "none" && live.length) {
      steps = [chord];
      outcome = attempt(steps);
    }
    if (outcome === "none") {
      pending = null;
      return;
    }
    pending = outcome === "wait" ? { steps, at: Date.now() } : null;
    e.preventDefault();
    e.stopPropagation();
  };
  window.addEventListener("keydown", onKey, true);
  return () => window.removeEventListener("keydown", onKey, true);
}
