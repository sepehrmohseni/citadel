type Child = Node | string | number | null | undefined | false | Child[];
type Handler = (ev: Event) => void;
export type Props = Record<string, unknown> & {
  class?: string;
  style?: Partial<CSSStyleDeclaration> | string;
  dataset?: Record<string, string>;
};

const SVG_NS = "http://www.w3.org/2000/svg";
const SVG_TAGS = new Set([
  "svg", "g", "path", "circle", "rect", "line", "polyline", "polygon", "text", "defs", "marker", "linearGradient", "stop", "title", "tspan",
]);

function append(el: Element, child: Child): void {
  if (child === null || child === undefined || child === false) return;
  if (Array.isArray(child)) {
    for (const c of child) append(el, c);
    return;
  }
  el.append(child instanceof Node ? child : document.createTextNode(String(child)));
}

export function h<K extends keyof HTMLElementTagNameMap>(tag: K, props?: Props | null, ...children: Child[]): HTMLElementTagNameMap[K];
export function h(tag: string, props?: Props | null, ...children: Child[]): Element;
export function h(tag: string, props?: Props | null, ...children: Child[]): Element {
  const svg = SVG_TAGS.has(tag);
  const el = svg ? document.createElementNS(SVG_NS, tag) : document.createElement(tag);
  for (const [k, v] of Object.entries(props ?? {})) {
    if (v === undefined || v === null || v === false) continue;
    if (k === "class") el.setAttribute("class", String(v));
    else if (k === "style" && typeof v === "object") Object.assign((el as HTMLElement).style, v);
    else if (k === "dataset" && typeof v === "object") Object.assign((el as HTMLElement).dataset, v);
    else if (k.startsWith("on") && typeof v === "function") el.addEventListener(k.slice(2).toLowerCase(), v as Handler);
    else if (!svg && k in el && typeof v !== "string") (el as unknown as Record<string, unknown>)[k] = v;
    else el.setAttribute(k, v === true ? "" : String(v));
  }
  for (const c of children) append(el, c);
  return el;
}

export function clear(el: Element): void {
  while (el.firstChild) el.firstChild.remove();
}

export function mount(el: Element, ...children: Child[]): void {
  clear(el);
  for (const c of children) append(el, c);
}

const ICONS: Record<string, string> = {
  hosts: "M4 5h16v5H4zM4 14h16v5H4zM7 7.5h.01M7 16.5h.01",
  tunnel: "M3 12h4m10 0h4M7 12a5 5 0 0 1 10 0M7 12a5 5 0 0 0 10 0M12 7v10",
  key: "M15 7a4 4 0 1 1-3.9 4.9L4 19v-3h3v-3h3l1.1-1.1A4 4 0 0 1 15 7zm1.5 2.5h.01",
  snippet: "M8 7l-5 5 5 5M16 7l5 5-5 5M13.5 5l-3 14",
  settings: "M12 15a3 3 0 1 0 0-6 3 3 0 0 0 0 6zM19.4 13a7.6 7.6 0 0 0 0-2l2-1.6-2-3.4-2.4 1a7.4 7.4 0 0 0-1.7-1L15 3.5h-4l-.4 2.5a7.4 7.4 0 0 0-1.7 1l-2.4-1-2 3.4 2 1.6a7.6 7.6 0 0 0 0 2l-2 1.6 2 3.4 2.4-1a7.4 7.4 0 0 0 1.7 1l.4 2.5h4l.4-2.5a7.4 7.4 0 0 0 1.7-1l2.4 1 2-3.4z",
  lock: "M6 11h12v9H6zM8.5 11V8a3.5 3.5 0 0 1 7 0v3",
  terminal: "M4 5h16v14H4zM7.5 10l2.5 2-2.5 2M12 15h4",
  folder: "M3 7a2 2 0 0 1 2-2h4l2 2h8a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z",
  file: "M6 3h8l4 4v14H6zM14 3v4h4",
  plus: "M12 5v14M5 12h14",
  close: "M6 6l12 12M18 6L6 18",
  search: "M11 18a7 7 0 1 0 0-14 7 7 0 0 0 0 14zM20 20l-4-4",
  play: "M8 5l11 7-11 7z",
  stop: "M7 7h10v10H7z",
  edit: "M4 20h4l10-10-4-4L4 16zM13.5 6.5l4 4",
  trash: "M5 7h14M10 11v6M14 11v6M6 7l1 13h10l1-13M9 7V4h6v3",
  upload: "M12 16V4M7 9l5-5 5 5M4 20h16",
  download: "M12 4v12M7 11l5 5 5-5M4 20h16",
  refresh: "M20 11a8 8 0 1 0-2.3 5.7M20 4v7h-7",
  copy: "M9 9h11v11H9zM5 15V4h11",
  chart: "M4 19h16M7 16v-5M12 16V7M17 16v-8",
  shield: "M12 3l8 3v6c0 5-3.5 8-8 9-4.5-1-8-4-8-9V6z",
  bolt: "M13 3L5 14h6l-1 7 8-11h-6z",
  link: "M10 14a4 4 0 0 0 5.7 0l3-3a4 4 0 0 0-5.7-5.7l-1 1M14 10a4 4 0 0 0-5.7 0l-3 3a4 4 0 0 0 5.7 5.7l1-1",
  chevron: "M9 6l6 6-6 6",
  up: "M12 19V5M5 12l7-7 7 7",
  cloud: "M7 18a5 5 0 1 1 .9-9.9A6 6 0 0 1 19 10a4 4 0 0 1-1 8z",
  broadcast: "M4.9 19.1a10 10 0 0 1 0-14.2M19.1 4.9a10 10 0 0 1 0 14.2M7.8 16.2a6 6 0 0 1 0-8.4M16.2 7.8a6 6 0 0 1 0 8.4M12 12h.01",
  dots: "M5 12h.01M12 12h.01M19 12h.01",
  check: "M5 12.5l4.5 4.5L19 7",
  warn: "M12 4l9 16H3zM12 10v4M12 17h.01",
  import: "M12 3v12M7 10l5 5 5-5M5 21h14",
};

export function icon(name: keyof typeof ICONS | string, size = 16): SVGSVGElement {
  const svg = document.createElementNS(SVG_NS, "svg");
  svg.setAttribute("viewBox", "0 0 24 24");
  svg.setAttribute("width", String(size));
  svg.setAttribute("height", String(size));
  svg.setAttribute("fill", "none");
  svg.setAttribute("stroke", "currentColor");
  svg.setAttribute("stroke-width", "1.7");
  svg.setAttribute("stroke-linecap", "round");
  svg.setAttribute("stroke-linejoin", "round");
  svg.setAttribute("aria-hidden", "true");
  svg.classList.add("icon");
  const p = document.createElementNS(SVG_NS, "path");
  p.setAttribute("d", ICONS[name] ?? ICONS.dots ?? "");
  svg.append(p);
  return svg;
}

export function bytes(n: number, digits = 1): string {
  if (!Number.isFinite(n) || n < 0) return "–";
  const units = ["B", "KiB", "MiB", "GiB", "TiB"];
  let v = n;
  let i = 0;
  while (v >= 1024 && i < units.length - 1) {
    v /= 1024;
    i++;
  }
  return `${i === 0 ? v.toFixed(0) : v.toFixed(digits)} ${units[i]}`;
}

export function rate(bps: number): string {
  return `${bytes(bps)}/s`;
}

export function duration(secs: number): string {
  const d = Math.floor(secs / 86400);
  const hrs = Math.floor((secs % 86400) / 3600);
  const m = Math.floor((secs % 3600) / 60);
  if (d > 0) return `${d}d ${hrs}h`;
  if (hrs > 0) return `${hrs}h ${m}m`;
  return `${m}m`;
}

export function ago(ms: number): string {
  const s = Math.max(0, Math.round((Date.now() - ms) / 1000));
  if (s < 60) return "just now";
  if (s < 3600) return `${Math.floor(s / 60)}m ago`;
  if (s < 86400) return `${Math.floor(s / 3600)}h ago`;
  return `${Math.floor(s / 86400)}d ago`;
}

export function fuzzy(query: string, text: string): number {
  const q = query.toLowerCase().trim();
  if (!q) return 1;
  const t = text.toLowerCase();
  const direct = t.indexOf(q);
  if (direct >= 0) return 1000 - direct * 2 - (t.length - q.length) * 0.1;
  let score = 0;
  let ti = 0;
  let streak = 0;
  for (const ch of q) {
    const found = t.indexOf(ch, ti);
    if (found < 0) return -1;
    streak = found === ti ? streak + 1 : 0;
    score += 10 + streak * 5 - (found - ti);
    ti = found + 1;
  }
  return score;
}

export function coalesce(fn: () => void): () => void {
  let queued = false;
  return () => {
    if (queued) return;
    queued = true;
    requestAnimationFrame(() => {
      queued = false;
      fn();
    });
  };
}

export function debounce<A extends unknown[]>(fn: (...args: A) => void, ms: number): (...args: A) => void {
  let t: ReturnType<typeof setTimeout> | undefined;
  return (...args: A) => {
    if (t) clearTimeout(t);
    t = setTimeout(() => fn(...args), ms);
  };
}

const US_KEYS: Record<string, readonly [string, string]> = {
  Backquote: ["`", "~"],
  Digit1: ["1", "!"],
  Digit2: ["2", "@"],
  Digit3: ["3", "#"],
  Digit4: ["4", "$"],
  Digit5: ["5", "%"],
  Digit6: ["6", "^"],
  Digit7: ["7", "&"],
  Digit8: ["8", "*"],
  Digit9: ["9", "("],
  Digit0: ["0", ")"],
  Minus: ["-", "_"],
  Equal: ["=", "+"],
  BracketLeft: ["[", "{"],
  BracketRight: ["]", "}"],
  Backslash: ["\\", "|"],
  IntlBackslash: ["\\", "|"],
  Semicolon: [";", ":"],
  Quote: ["'", '"'],
  Comma: [",", "<"],
  Period: [".", ">"],
  Slash: ["/", "?"],
  Space: [" ", " "],
};

export function foreignKey(e: KeyboardEvent): boolean {
  const cps = [...e.key];
  return cps.length === 1 && (cps[0]?.codePointAt(0) ?? 0) > 0x7f;
}

export function keyOf(e: KeyboardEvent): string {
  if (!foreignKey(e)) return e.key;
  const letter = /^Key([A-Z])$/.exec(e.code)?.[1];
  if (letter) return e.shiftKey ? letter : letter.toLowerCase();
  const us = US_KEYS[e.code];
  return us ? us[e.shiftKey ? 1 : 0] : e.key;
}

const CTRL_PUNCT: Record<string, string> = {
  "@": "\x00",
  " ": "\x00",
  "2": "\x00",
  "[": "\x1b",
  "3": "\x1b",
  "\\": "\x1c",
  "4": "\x1c",
  "]": "\x1d",
  "5": "\x1d",
  "^": "\x1e",
  "6": "\x1e",
  "_": "\x1f",
  "/": "\x1f",
  "7": "\x1f",
  "8": "\x7f",
};

export function terminalChord(e: KeyboardEvent, optionIsMeta: boolean): string | null {
  if (!foreignKey(e) || e.metaKey || e.isComposing) return null;
  const k = keyOf(e);
  if (k.length !== 1) return null;
  if (e.ctrlKey && !e.altKey && !e.shiftKey) {
    if (/^[a-z]$/.test(k)) return String.fromCharCode(k.charCodeAt(0) - 96);
    return CTRL_PUNCT[k] ?? null;
  }
  if (e.altKey && !e.ctrlKey && (optionIsMeta || !isMac)) return `\x1b${k}`;
  return null;
}

export const isMac = /Mac|iPhone|iPad/.test(navigator.platform || navigator.userAgent);
export const modKey = isMac ? "⌘" : "Ctrl";

export function kbd(...keys: string[]): HTMLElement {
  return h("span", { class: "kbd-group" }, keys.map((k) => h("kbd", null, k)));
}
