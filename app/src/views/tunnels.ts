import { api, type Endpoint, type Forward, type Host, type Id, type Tunnel } from "../api";
import { bytes, clear, h, icon, rate as fmtRate } from "../dom";
import type { Page } from "../shell";
import { hostById, hosts, links, refreshTunnels, tunnels } from "../store";
import { btn, confirmBox, copyText, empty, fail, field, input, modal, segmented, select, toast, toggle } from "../ui";
import "./tunnels.css";

type Tone = "ok" | "warn" | "err" | "idle";
type Mode = "graph" | "list";
type Kind = Forward["kind"];

interface Pos {
  x: number;
  y: number;
}

interface RateMark {
  at: number;
  bytes: number;
  bps: number;
}

interface NodeModel {
  key: string;
  kind: "host" | "left" | "right";
  x: number;
  y: number;
  w: number;
  h: number;
  title: string;
  sub: string;
  tone: Tone;
  chain: string[];
  selected: boolean;
  dim: boolean;
  tunnel: Id | null;
  host: Id | null;
}

interface EdgeModel {
  key: string;
  tunnel: Id;
  d: string;
  flow: boolean;
  speed: number;
  tone: Tone;
  selected: boolean;
  arrow: boolean;
}

interface LabelModel {
  key: string;
  tunnel: Id;
  x: number;
  y: number;
  text: string;
}

const SVG_NS = "http://www.w3.org/2000/svg";
const ROW = 78;
const COL = 340;
const GAP = 46;
const HOST_W = 236;
const HOST_H = 62;
const END_W = 214;
const END_H = 44;
const RATE_WINDOW = 900;

let mode: Mode = "graph";
const marks = new Map<Id, RateMark>();

function track(list: Tunnel[]): void {
  const now = performance.now();
  for (const t of list) {
    const total = (t.stats?.tx ?? 0) + (t.stats?.rx ?? 0);
    const prev = marks.get(t.id);
    if (!t.running || !t.stats) {
      marks.set(t.id, { at: now, bytes: total, bps: 0 });
      continue;
    }
    if (!prev) {
      marks.set(t.id, { at: now, bytes: total, bps: 0 });
      continue;
    }
    const dt = now - prev.at;
    if (dt < RATE_WINDOW) continue;
    const delta = Math.max(0, total - prev.bytes);
    marks.set(t.id, { at: now, bytes: total, bps: (delta / dt) * 1000 });
  }
}

function bps(id: Id): number {
  return marks.get(id)?.bps ?? 0;
}

function svg<K extends keyof SVGElementTagNameMap>(tag: K, attrs: Record<string, string | number> = {}): SVGElementTagNameMap[K] {
  const el = document.createElementNS(SVG_NS, tag);
  for (const [k, v] of Object.entries(attrs)) el.setAttribute(k, String(v));
  return el;
}

function fit(text: string, max: number): string {
  return text.length > max ? `${text.slice(0, Math.max(1, max - 1))}…` : text;
}

function ep(e: Endpoint): string {
  const host = e.host.includes(":") && !e.host.startsWith("[") ? `[${e.host}]` : e.host;
  return `${host}:${e.port === 0 ? "auto" : e.port}`;
}

function exposed(host: string): boolean {
  const v = host.trim();
  return v === "0.0.0.0" || v === "::" || v === "*" || v === "[::]" || v === "";
}

function kindLetter(k: Kind): string {
  return k === "local" ? "L" : k === "remote" ? "R" : "D";
}

function kindName(k: Kind): string {
  return k === "local" ? "Local" : k === "remote" ? "Remote" : "Dynamic";
}

function listenAddr(t: Tunnel): string | null {
  if (t.state?.state === "listening") return t.state.addr;
  if (t.forward.kind === "local" || t.forward.kind === "dynamic") return ep(t.forward.bind);
  return null;
}

function sides(t: Tunnel): { left: string; leftSub: string; right: string; rightSub: string; reverse: boolean } {
  const live = t.state?.state === "listening" ? t.state.addr : null;
  switch (t.forward.kind) {
    case "local":
      return { left: live ?? ep(t.forward.bind), leftSub: "local listener", right: ep(t.forward.dest), rightSub: "destination", reverse: false };
    case "remote": {
      const port = t.state?.state === "forwarding" ? t.state.remote_port : t.forward.bind.port;
      return {
        left: ep(t.forward.dest),
        leftSub: "local target",
        right: ep({ host: t.forward.bind.host, port }),
        rightSub: "remote listener",
        reverse: true,
      };
    }
    case "dynamic":
      return { left: `SOCKS5 ${live ?? ep(t.forward.bind)}`, leftSub: "dynamic proxy", right: "any destination", rightSub: "resolved by the remote host", reverse: false };
  }
}

function stateOf(t: Tunnel): { tone: Tone; text: string } {
  if (!t.running) return { tone: "idle", text: "Stopped" };
  const s = t.state;
  if (!s) return { tone: "warn", text: "Starting" };
  switch (s.state) {
    case "listening":
      return { tone: "ok", text: `Listening on ${s.addr}` };
    case "forwarding":
      return { tone: "ok", text: `Forwarding remote :${s.remote_port}` };
    case "waiting":
      return { tone: "warn", text: `Waiting · ${s.reason}` };
    case "failed":
      return { tone: "err", text: `Failed · ${s.error}` };
    case "stopped":
      return { tone: "idle", text: "Stopped" };
  }
}

function linkTone(host: Id): Tone {
  const s = links.get().get(host);
  if (!s) return "idle";
  if (s.state === "up") return "ok";
  if (s.state === "connecting" || s.state === "backoff") return "warn";
  return "idle";
}

function chainOf(host: Host): string[] {
  return host.jump.map((j) => hostById(j)?.label ?? "unknown");
}

function viaText(host: Host | undefined): string {
  if (!host) return "unknown host";
  return [...chainOf(host), host.label].join(" → ");
}

function routeText(t: Tunnel): string {
  const s = sides(t);
  const host = hostById(t.host);
  return s.reverse ? `${s.right} → ${s.left} via ${host?.label ?? "?"}` : `${s.left} → ${s.right} via ${host?.label ?? "?"}`;
}

function speedFor(v: number): number {
  if (v <= 0) return 0;
  const levels = [2.2, 1.5, 1.0, 0.65, 0.4];
  const idx = Math.min(levels.length - 1, Math.max(0, Math.floor(Math.log10(v + 1) - 2)));
  return levels[idx] ?? 1.2;
}

function patchTunnel(t: Tunnel): void {
  tunnels.update((list) => {
    const has = list.some((x) => x.id === t.id);
    return has ? list.map((x) => (x.id === t.id ? t : x)) : [...list, t];
  });
}

async function startStop(t: Tunnel): Promise<void> {
  try {
    if (t.running) {
      await api.tunnelStop(t.id);
      patchTunnel({ ...t, running: false, state: null, stats: null });
      toast(`${t.label} stopped`, "info", 2200);
    } else {
      patchTunnel(await api.tunnelStart(t.id));
      toast(`${t.label} started`, "ok", 2200);
    }
  } catch (e) {
    fail(e, t.running ? "Stop failed" : "Start failed");
  }
}

async function remove(t: Tunnel): Promise<boolean> {
  const ok = await confirmBox("Delete tunnel", `Delete "${t.label}"? A running forward is stopped first.`, { danger: true, confirm: "Delete" });
  if (!ok) return false;
  try {
    await api.tunnelDelete(t.id);
    tunnels.update((list) => list.filter((x) => x.id !== t.id));
    marks.delete(t.id);
    toast("Tunnel deleted", "ok", 2000);
    return true;
  } catch (e) {
    fail(e);
    return false;
  }
}

async function setAutostart(t: Tunnel, on: boolean): Promise<void> {
  try {
    const saved = await api.tunnelSave({ id: t.id, label: t.label, host: t.host, forward: t.forward, autostart: on });
    patchTunnel({ ...saved, running: t.running, state: t.state, stats: t.stats });
  } catch (e) {
    fail(e);
  }
}

export function openTunnelForm(existing?: Tunnel, presetHost?: Id): void {
  const allHosts = hosts.get();
  if (!allHosts.length) {
    toast("Add a host before creating a tunnel", "warn");
    return;
  }
  let kind: Kind = existing?.forward.kind ?? "local";
  const f = existing?.forward;
  const label = input({ value: existing?.label ?? "", placeholder: "postgres on db-1" });
  const hostSel = select(
    allHosts.map((x) => ({ value: x.id, label: `${x.label}  ·  ${x.user}@${x.hostname}` })),
    existing?.host ?? presetHost ?? allHosts[0]?.id ?? "",
  );
  const bindHost = input({ class: "input mono", value: f ? f.bind.host : "127.0.0.1" });
  const bindPort = input({ class: "input mono", type: "number", min: "0", max: "65535", value: f ? String(f.bind.port) : "", placeholder: "15432" });
  const destHost = input({ class: "input mono", value: f && f.kind !== "dynamic" ? f.dest.host : "", placeholder: "10.0.0.5" });
  const destPort = input({ class: "input mono", type: "number", min: "1", max: "65535", value: f && f.kind !== "dynamic" ? String(f.dest.port) : "", placeholder: "5432" });
  let autostart = existing?.autostart ?? false;
  let startNow = !existing;
  const bindLabel = h("span", { class: "field-label" });
  const destLabel = h("span", { class: "field-label" });
  const bindHint = h("span", { class: "field-hint" });
  const destRow = h("div", { class: "grid-3" });
  const warn = h("div", { class: "alert", hidden: true });
  const preview = h("div", { class: "tf-preview" });
  const kindHelp = h("p", { class: "muted tf-kindhelp" });

  destRow.append(
    h("label", { class: "field" }, destLabel, destHost),
    h("label", { class: "field" }, h("span", { class: "field-label" }, "Port"), destPort),
    h("div"),
  );

  const parsePort = (el: HTMLInputElement): number => {
    const n = Number(el.value.trim());
    return Number.isInteger(n) ? n : -1;
  };

  const refresh = () => {
    const host = hostById(hostSel.value);
    const via = viaText(host);
    const bh = bindHost.value.trim() || "127.0.0.1";
    const bp = bindPort.value.trim() || "?";
    const dh = destHost.value.trim() || "?";
    const dp = destPort.value.trim() || "?";
    destRow.hidden = kind === "dynamic";
    if (kind === "local") {
      bindLabel.textContent = "Listen on (this machine)";
      destLabel.textContent = "Destination (as seen from the host)";
      bindHint.textContent = "Port 0 picks any free port.";
      kindHelp.textContent = "Expose a service that lives behind the host on a port of this machine.";
      preview.replaceChildren(
        "Connections to ",
        h("code", null, `${bh}:${bp}`),
        " on this machine are forwarded through ",
        h("b", null, via),
        " to ",
        h("code", null, `${dh}:${dp}`),
        ".",
      );
    } else if (kind === "remote") {
      bindLabel.textContent = "Listen on (remote host)";
      destLabel.textContent = "Destination (reachable from this machine)";
      bindHint.textContent = "Loopback keeps the port private to the remote host.";
      kindHelp.textContent = "Publish a service from this machine on a port of the remote host.";
      preview.replaceChildren(
        "Connections to ",
        h("code", null, `${bh}:${bp}`),
        " on ",
        h("b", null, host?.label ?? "the host"),
        " are carried back over ",
        h("b", null, via),
        " to ",
        h("code", null, `${dh}:${dp}`),
        " on this machine.",
      );
    } else {
      bindLabel.textContent = "SOCKS5 listener (this machine)";
      bindHint.textContent = "Point a browser or tool at this address as a SOCKS5 proxy.";
      kindHelp.textContent = "A SOCKS5 proxy that resolves and connects from the remote host.";
      preview.replaceChildren(
        "Applications using the SOCKS5 proxy at ",
        h("code", null, `${bh}:${bp}`),
        " reach any destination through ",
        h("b", null, via),
        ".",
      );
    }
    const open = exposed(bindHost.value);
    warn.hidden = !open;
    if (open) {
      warn.replaceChildren(
        icon("warn", 15),
        h(
          "span",
          null,
          kind === "remote"
            ? "This binds on every interface of the remote host. Anyone who can reach it can use the tunnel, provided the server allows GatewayPorts."
            : "This binds on every interface of this machine. Anyone on your network can connect to the forwarded port.",
        ),
      );
    }
  };

  for (const el of [bindHost, bindPort, destHost, destPort]) el.addEventListener("input", refresh);
  hostSel.addEventListener("change", refresh);

  const body = h(
    "div",
    { class: "stack tunnel-form" },
    h("div", { class: "grid-2" }, field("Label", label), field("Host", hostSel)),
    h(
      "div",
      { class: "field" },
      h("span", { class: "field-label" }, "Type"),
      segmented<Kind>(
        [
          { value: "local", label: "Local  -L" },
          { value: "remote", label: "Remote  -R" },
          { value: "dynamic", label: "Dynamic  -D" },
        ],
        kind,
        (v) => {
          kind = v;
          refresh();
        },
      ),
      kindHelp,
    ),
    h(
      "div",
      { class: "grid-3" },
      h("label", { class: "field" }, bindLabel, bindHost, bindHint),
      h("label", { class: "field" }, h("span", { class: "field-label" }, "Port"), bindPort),
      h("div"),
    ),
    destRow,
    warn,
    preview,
    h(
      "div",
      { class: "row tf-toggles" },
      toggle(autostart, "Start when the vault unlocks", (v) => {
        autostart = v;
      }),
      toggle(startNow, existing ? "Restart now" : "Start now", (v) => {
        startNow = v;
      }),
    ),
  );
  refresh();

  modal({
    title: existing ? "Edit tunnel" : "New tunnel",
    body,
    wide: true,
    actions: [
      { label: "Cancel", kind: "ghost", run: () => undefined },
      {
        label: existing ? "Save" : "Create tunnel",
        kind: "primary",
        run: async () => {
          const bp = parsePort(bindPort);
          const minBind = kind === "remote" ? 1 : 0;
          if (bp < minBind || bp > 65535) throw { message: `Listen port must be between ${minBind} and 65535` };
          const bh = bindHost.value.trim();
          if (!bh) throw { message: "Listen address is required" };
          let forward: Forward;
          if (kind === "dynamic") {
            forward = { kind, bind: { host: bh, port: bp } };
          } else {
            const dp = parsePort(destPort);
            const dh = destHost.value.trim();
            if (!dh) throw { message: "Destination host is required" };
            if (dp < 1 || dp > 65535) throw { message: "Destination port must be between 1 and 65535" };
            forward = { kind, bind: { host: bh, port: bp }, dest: { host: dh, port: dp } };
          }
          const host = hostById(hostSel.value);
          const name = label.value.trim() || `${host?.label ?? "tunnel"} :${bp === 0 ? "auto" : bp}`;
          const saved = await api.tunnelSave({ id: existing?.id ?? null, label: name, host: hostSel.value, forward, autostart });
          patchTunnel(saved);
          if (startNow && !saved.running) {
            try {
              patchTunnel(await api.tunnelStart(saved.id));
            } catch (e) {
              fail(e, "Saved, but the tunnel did not start");
              return;
            }
          }
          toast(existing ? "Tunnel updated" : "Tunnel created", "ok", 2200);
        },
      },
    ],
  });
}

interface NodeView {
  g: SVGGElement;
  update(n: NodeModel): void;
}

function makeNode(kind: NodeModel["kind"]): NodeView {
  const g = svg("g", { class: `tg-node tg-${kind}`, tabindex: 0 });
  const w = kind === "host" ? HOST_W : END_W;
  const hh = kind === "host" ? HOST_H : END_H;
  const ring = svg("rect", { class: "tg-ring", x: -3, y: -3, width: w + 6, height: hh + 6, rx: 3 });
  const bg = svg("rect", { class: "tg-bg", width: w, height: hh, rx: 2 });
  const dot = svg("circle", { class: "tg-dot", cx: 16, cy: hh / 2, r: 4 });
  const title = svg("text", { class: "tg-title", x: 30, y: kind === "host" ? 26 : 19 });
  const sub = svg("text", { class: "tg-sub", x: 30, y: kind === "host" ? 44 : 33 });
  const chain = svg("g", { class: "tg-chain" });
  g.append(ring, bg, dot, title, sub, chain);
  let chainKey = "";
  return {
    g,
    update(n) {
      g.setAttribute("transform", `translate(${n.x - n.w / 2},${n.y - n.h / 2})`);
      g.setAttribute("class", `tg-node tg-${n.kind} tone-${n.tone}${n.selected ? " sel" : ""}${n.dim ? " dim" : ""}`);
      const max = n.kind === "host" ? 27 : 24;
      title.textContent = fit(n.title, max);
      sub.textContent = fit(n.sub, n.kind === "host" ? 32 : 28);
      const key = n.chain.join("\u0000");
      if (key !== chainKey) {
        chainKey = key;
        while (chain.firstChild) chain.firstChild.remove();
        let x = 0;
        n.chain.forEach((c, i) => {
          const text = fit(c, 14);
          const width = text.length * 6.4 + 16;
          const pill = svg("g", { transform: `translate(${x},-24)` });
          pill.append(svg("rect", { class: "tg-pill", width, height: 18, rx: 1 }));
          const t = svg("text", { class: "tg-pill-text", x: width / 2, y: 12.5, "text-anchor": "middle" });
          t.textContent = text;
          pill.append(t);
          chain.append(pill);
          x += width + 4;
          const arrow = svg("text", { class: "tg-pill-arrow", x: x - 1, y: -11 });
          arrow.textContent = i === n.chain.length - 1 ? "→" : "›";
          chain.append(arrow);
          x += 10;
        });
      }
    },
  };
}

interface EdgeView {
  vis: SVGPathElement;
  hit: SVGPathElement;
}

class Graph {
  readonly el: HTMLElement;
  readonly svg: SVGSVGElement;
  readonly world: SVGGElement;
  readonly edgesG: SVGGElement;
  readonly nodesG: SVGGElement;
  readonly labelsG: SVGGElement;
  readonly heads: SVGGElement;
  k = 1;
  tx = 0;
  ty = 0;
  selected: Id | null = null;
  dragging = false;
  pending = false;
  fitted = false;
  overrides = new Map<string, Pos>();
  nodes = new Map<string, NodeView>();
  edges = new Map<string, EdgeView>();
  labels = new Map<string, { g: SVGGElement; rect: SVGRectElement; text: SVGTextElement }>();
  models = new Map<string, NodeModel>();
  onSelect: (id: Id | null) => void = () => undefined;
  #resize: ResizeObserver;

  constructor() {
    this.svg = svg("svg", { class: "tg-svg" });
    const defs = svg("defs");
    const marker = svg("marker", { id: "tg-arrow", viewBox: "0 0 10 10", refX: 9, refY: 5, markerWidth: 7, markerHeight: 7, orient: "auto-start-reverse" });
    marker.append(svg("path", { d: "M0 0 L10 5 L0 10 z", class: "tg-arrowhead" }));
    const markerOn = svg("marker", { id: "tg-arrow-on", viewBox: "0 0 10 10", refX: 9, refY: 5, markerWidth: 7, markerHeight: 7, orient: "auto-start-reverse" });
    markerOn.append(svg("path", { d: "M0 0 L10 5 L0 10 z", class: "tg-arrowhead on" }));
    const grid = svg("pattern", { id: "tg-grid", width: 24, height: 24, patternUnits: "userSpaceOnUse" });
    grid.append(svg("circle", { cx: 1, cy: 1, r: 1, class: "tg-griddot" }));
    defs.append(marker, markerOn, grid);
    const bg = svg("rect", { class: "tg-bgrect", x: -20000, y: -20000, width: 40000, height: 40000, fill: "url(#tg-grid)" });
    this.world = svg("g");
    this.heads = svg("g", { class: "tg-heads" });
    this.edgesG = svg("g");
    this.labelsG = svg("g");
    this.nodesG = svg("g");
    this.world.append(bg, this.heads, this.edgesG, this.labelsG, this.nodesG);
    this.svg.append(defs, this.world);
    this.el = h("div", { class: "tg-canvas" }, this.svg);
    this.#wire();
    this.#resize = new ResizeObserver(() => {
      if (!this.fitted && this.svg.clientWidth > 0) {
        this.fit();
      }
    });
    this.#resize.observe(this.el);
  }

  dispose(): void {
    this.#resize.disconnect();
  }

  apply(): void {
    this.world.setAttribute("transform", `translate(${this.tx},${this.ty}) scale(${this.k})`);
  }

  zoomAt(factor: number, cx: number, cy: number): void {
    const k = Math.min(2, Math.max(0.5, this.k * factor));
    const f = k / this.k;
    this.tx = cx - (cx - this.tx) * f;
    this.ty = cy - (cy - this.ty) * f;
    this.k = k;
    this.apply();
  }

  fit(): void {
    const W = this.svg.clientWidth;
    const H = this.svg.clientHeight;
    if (!W || !H) return;
    if (!this.models.size) {
      this.k = 1;
      this.tx = W / 2;
      this.ty = H / 2;
      this.apply();
      return;
    }
    let x0 = Infinity;
    let y0 = Infinity;
    let x1 = -Infinity;
    let y1 = -Infinity;
    for (const m of this.models.values()) {
      x0 = Math.min(x0, m.x - m.w / 2);
      x1 = Math.max(x1, m.x + m.w / 2);
      y0 = Math.min(y0, m.y - m.h / 2 - (m.chain.length ? 30 : 0));
      y1 = Math.max(y1, m.y + m.h / 2);
    }
    const pad = 70;
    const k = Math.min(1.15, Math.max(0.5, Math.min((W - pad * 2) / (x1 - x0), (H - pad * 2) / (y1 - y0))));
    this.k = k;
    this.tx = W / 2 - ((x0 + x1) / 2) * k;
    this.ty = H / 2 - ((y0 + y1) / 2) * k;
    this.fitted = true;
    this.apply();
  }

  #local(e: PointerEvent | WheelEvent): Pos {
    const r = this.svg.getBoundingClientRect();
    return { x: e.clientX - r.left, y: e.clientY - r.top };
  }

  #wire(): void {
    this.svg.addEventListener(
      "wheel",
      (e) => {
        e.preventDefault();
        const p = this.#local(e);
        const scale = e.ctrlKey ? 0.012 : 0.0016;
        this.zoomAt(Math.exp(-e.deltaY * scale), p.x, p.y);
      },
      { passive: false },
    );
    let pan: { x: number; y: number; tx: number; ty: number } | null = null;
    this.svg.addEventListener("pointerdown", (e) => {
      const target = e.target as Element;
      if (e.button !== 0) return;
      if (target === this.svg || target.classList.contains("tg-bgrect")) {
        pan = { x: e.clientX, y: e.clientY, tx: this.tx, ty: this.ty };
        this.svg.setPointerCapture(e.pointerId);
        this.svg.classList.add("panning");
      }
    });
    this.svg.addEventListener("pointermove", (e) => {
      if (!pan) return;
      this.tx = pan.tx + (e.clientX - pan.x);
      this.ty = pan.ty + (e.clientY - pan.y);
      this.apply();
    });
    const endPan = (e: PointerEvent) => {
      if (!pan) return;
      const moved = Math.abs(e.clientX - pan.x) + Math.abs(e.clientY - pan.y);
      pan = null;
      this.svg.classList.remove("panning");
      if (moved < 3) this.onSelect(null);
    };
    this.svg.addEventListener("pointerup", endPan);
    this.svg.addEventListener("pointercancel", endPan);
  }

  #dragNode(view: NodeView, key: string): void {
    view.g.addEventListener("pointerdown", (e) => {
      if (e.button !== 0) return;
      e.stopPropagation();
      const m = this.models.get(key);
      if (!m) return;
      const start = { x: e.clientX, y: e.clientY };
      const origin = { x: m.x, y: m.y };
      let moved = false;
      view.g.setPointerCapture(e.pointerId);
      const move = (ev: PointerEvent) => {
        const dx = (ev.clientX - start.x) / this.k;
        const dy = (ev.clientY - start.y) / this.k;
        if (!moved && Math.abs(dx) + Math.abs(dy) < 3) return;
        moved = true;
        this.dragging = true;
        view.g.classList.add("dragging");
        this.#placeDragged(key, { x: origin.x + dx, y: origin.y + dy });
      };
      const up = () => {
        view.g.removeEventListener("pointermove", move);
        view.g.removeEventListener("pointerup", up);
        view.g.removeEventListener("pointercancel", up);
        view.g.classList.remove("dragging");
        this.dragging = false;
        const cur = this.models.get(key);
        if (moved && cur) {
          void this.#persist(cur);
        } else if (cur) {
          this.onSelect(cur.tunnel ?? this.selected);
        }
        if (this.pending) {
          this.pending = false;
          this.render(tunnels.get());
        }
      };
      view.g.addEventListener("pointermove", move);
      view.g.addEventListener("pointerup", up);
      view.g.addEventListener("pointercancel", up);
    });
    view.g.addEventListener("keydown", (e) => {
      const m = this.models.get(key);
      if (!m) return;
      if (e.key === "Enter" || e.key === " ") {
        e.preventDefault();
        this.onSelect(m.tunnel ?? this.selected);
      }
    });
  }

  #placeDragged(key: string, p: Pos): void {
    const m = this.models.get(key);
    if (!m) return;
    if (m.kind === "host" && m.host) {
      this.overrides.set(`h:${m.host}`, p);
    } else if (m.tunnel) {
      const hostModel = this.models.get(`h:${this.#hostOfTunnel(m.tunnel)}`);
      const hx = hostModel?.x ?? 0;
      const left = m.kind === "left" ? p : { x: 2 * hx - p.x, y: p.y };
      this.overrides.set(`t:${m.tunnel}`, left);
    }
    this.render(tunnels.get(), true);
  }

  #hostOfTunnel(id: Id): Id {
    return tunnels.get().find((t) => t.id === id)?.host ?? "";
  }

  async #persist(m: NodeModel): Promise<void> {
    try {
      if (m.kind === "host" && m.host) {
        const p = this.overrides.get(`h:${m.host}`);
        if (!p) return;
        await api.canvasMove(m.host, p.x, p.y);
        const id = m.host;
        hosts.update((list) => list.map((x) => (x.id === id ? { ...x, canvas: { x: p.x, y: p.y } } : x)));
        this.overrides.delete(`h:${id}`);
      } else if (m.tunnel) {
        const p = this.overrides.get(`t:${m.tunnel}`);
        if (!p) return;
        await api.canvasMove(m.tunnel, p.x, p.y);
        const id = m.tunnel;
        tunnels.update((list) => list.map((x) => (x.id === id ? { ...x, canvas: { x: p.x, y: p.y } } : x)));
        this.overrides.delete(`t:${id}`);
      }
    } catch (e) {
      fail(e, "Could not save layout");
    }
  }

  layout(list: Tunnel[]): { nodes: NodeModel[]; edges: EdgeModel[]; labels: LabelModel[] } {
    const byHost = new Map<Id, Tunnel[]>();
    for (const t of list) {
      const arr = byHost.get(t.host) ?? [];
      arr.push(t);
      byHost.set(t.host, arr);
    }
    const hostIds = [...byHost.keys()].sort((a, b) => (hostById(a)?.label ?? "").localeCompare(hostById(b)?.label ?? ""));
    const nodes: NodeModel[] = [];
    const edges: EdgeModel[] = [];
    const labels: LabelModel[] = [];
    let cursor = 0;
    const sel = this.selected;
    for (const hid of hostIds) {
      const ts = (byHost.get(hid) ?? []).slice().sort((a, b) => a.label.localeCompare(b.label));
      const host = hostById(hid);
      const rows = Math.max(1, ts.length);
      const block = rows * ROW;
      const def = { x: 0, y: cursor + block / 2 - ROW / 2 };
      cursor += block + GAP;
      const hp = this.overrides.get(`h:${hid}`) ?? host?.canvas ?? def;
      const hostSelected = sel !== null && ts.some((t) => t.id === sel);
      nodes.push({
        key: `h:${hid}`,
        kind: "host",
        x: hp.x,
        y: hp.y,
        w: HOST_W,
        h: HOST_H,
        title: host?.label ?? "Unknown host",
        sub: host ? `${host.user}@${host.hostname}${host.port === 22 ? "" : `:${host.port}`}` : hid,
        tone: linkTone(hid),
        chain: host ? chainOf(host) : [],
        selected: hostSelected,
        dim: sel !== null && !hostSelected,
        tunnel: null,
        host: hid,
      });
      ts.forEach((t, i) => {
        const lp = this.overrides.get(`t:${t.id}`) ?? t.canvas ?? { x: hp.x - COL, y: hp.y + (i - (ts.length - 1) / 2) * ROW };
        const rp = { x: 2 * hp.x - lp.x, y: lp.y };
        const s = sides(t);
        const st = stateOf(t);
        const on = sel === t.id;
        const dim = sel !== null && !on;
        nodes.push(
          { key: `l:${t.id}`, kind: "left", x: lp.x, y: lp.y, w: END_W, h: END_H, title: s.left, sub: `${kindLetter(t.forward.kind)} · ${s.leftSub}`, tone: st.tone, chain: [], selected: on, dim, tunnel: t.id, host: null },
          { key: `r:${t.id}`, kind: "right", x: rp.x, y: rp.y, w: END_W, h: END_H, title: s.right, sub: s.rightSub, tone: t.forward.kind === "dynamic" ? "idle" : st.tone, chain: [], selected: on, dim, tunnel: t.id, host: null },
        );
        const v = bps(t.id);
        const flow = t.running && st.tone === "ok" && v > 0;
        const speed = speedFor(v);
        const a = { x: lp.x + END_W / 2, y: lp.y };
        const b = { x: hp.x - HOST_W / 2, y: hp.y };
        const c = { x: hp.x + HOST_W / 2, y: hp.y };
        const d = { x: rp.x - END_W / 2, y: rp.y };
        const curve = (p: Pos, q: Pos) => {
          const dx = Math.max(40, Math.abs(q.x - p.x) / 2) * Math.sign(q.x - p.x || 1);
          return `M${p.x},${p.y} C${p.x + dx},${p.y} ${q.x - dx},${q.y} ${q.x},${q.y}`;
        };
        const first = s.reverse ? curve(d, c) : curve(a, b);
        const second = s.reverse ? curve(b, a) : curve(c, d);
        edges.push(
          { key: `e1:${t.id}`, tunnel: t.id, d: first, flow, speed, tone: st.tone, selected: on, arrow: false },
          { key: `e2:${t.id}`, tunnel: t.id, d: second, flow, speed, tone: st.tone, selected: on, arrow: true },
        );
        if (t.running) {
          const active = t.stats?.active ?? 0;
          const text = `${v > 0 ? fmtRate(v) : "idle"} · ${active} conn`;
          labels.push({ key: `lb:${t.id}`, tunnel: t.id, x: (a.x + b.x) / 2, y: (a.y + b.y) / 2 - 12, text });
        }
      });
    }
    return { nodes, edges, labels };
  }

  render(list: Tunnel[], geometryOnly = false): void {
    if (this.dragging && !geometryOnly) {
      this.pending = true;
      return;
    }
    const { nodes, edges, labels } = this.layout(list);
    const seenN = new Set<string>();
    this.models.clear();
    for (const n of nodes) {
      seenN.add(n.key);
      this.models.set(n.key, n);
      let view = this.nodes.get(n.key);
      if (!view) {
        view = makeNode(n.kind);
        this.nodes.set(n.key, view);
        this.nodesG.append(view.g);
        this.#dragNode(view, n.key);
      }
      view.update(n);
    }
    for (const [k, v] of this.nodes) {
      if (!seenN.has(k)) {
        v.g.remove();
        this.nodes.delete(k);
      }
    }
    const seenE = new Set<string>();
    for (const e of edges) {
      seenE.add(e.key);
      let view = this.edges.get(e.key);
      if (!view) {
        const vis = svg("path", { class: "tg-edge" });
        const hit = svg("path", { class: "tg-hit" });
        const tid = e.tunnel;
        hit.addEventListener("pointerdown", (ev) => ev.stopPropagation());
        hit.addEventListener("click", () => this.onSelect(tid));
        this.edgesG.append(vis, hit);
        view = { vis, hit };
        this.edges.set(e.key, view);
      }
      view.vis.setAttribute("d", e.d);
      view.hit.setAttribute("d", e.d);
      view.vis.setAttribute("class", `tg-edge tone-${e.tone}${e.flow ? " flow" : ""}${e.selected ? " sel" : ""}${this.selected && !e.selected ? " dim" : ""}`);
      if (e.flow) view.vis.style.setProperty("--flow", `${e.speed}s`);
      if (e.arrow) view.vis.setAttribute("marker-end", e.selected || e.flow ? "url(#tg-arrow-on)" : "url(#tg-arrow)");
    }
    for (const [k, v] of this.edges) {
      if (!seenE.has(k)) {
        v.vis.remove();
        v.hit.remove();
        this.edges.delete(k);
      }
    }
    const seenL = new Set<string>();
    for (const l of labels) {
      seenL.add(l.key);
      let view = this.labels.get(l.key);
      if (!view) {
        const g = svg("g", { class: "tg-label" });
        const rect = svg("rect", { rx: 1, height: 18 });
        const text = svg("text", { y: 12.5, "text-anchor": "middle" });
        g.append(rect, text);
        this.labelsG.append(g);
        view = { g, rect, text };
        this.labels.set(l.key, view);
      }
      const w = l.text.length * 6.3 + 16;
      view.rect.setAttribute("width", String(w));
      view.rect.setAttribute("x", String(-w / 2));
      view.text.textContent = l.text;
      view.g.setAttribute("transform", `translate(${l.x},${l.y - 9})`);
      view.g.classList.toggle("dim", this.selected !== null && this.selected !== l.tunnel);
    }
    for (const [k, v] of this.labels) {
      if (!seenL.has(k)) {
        v.g.remove();
        this.labels.delete(k);
      }
    }
    if (!this.fitted && this.svg.clientWidth > 0) this.fit();
  }
}

function stateBadge(t: Tunnel): HTMLElement {
  const s = stateOf(t);
  const cls = s.tone === "ok" ? "accent" : s.tone === "warn" ? "warn" : s.tone === "err" ? "danger" : "";
  return h("span", { class: `badge ${cls} tn-state`, title: s.text }, h("span", { class: `dot ${s.tone}${s.tone === "warn" ? " pulse" : ""}` }), fit(s.text, 64));
}

function inspector(t: Tunnel, close: () => void): HTMLElement {
  const host = hostById(t.host);
  const s = sides(t);
  const st = t.stats;
  const addr = listenAddr(t);
  const kv = (k: string, v: Node | string) => h("div", { class: "ti-kv" }, h("span", null, k), h("span", null, v));
  return h(
    "aside",
    { class: "tn-inspector" },
    h(
      "header",
      null,
      h("span", { class: `tn-kind k-${t.forward.kind}` }, kindLetter(t.forward.kind)),
      h("div", { class: "grow" }, h("h3", null, t.label), h("div", { class: "muted" }, `${kindName(t.forward.kind)} forward`)),
      btn("", { icon: "close", ghost: true, small: true, title: "Close inspector", onClick: close }),
    ),
    stateBadge(t),
    t.state?.state === "failed" ? h("div", { class: "alert danger" }, icon("warn", 15), h("span", null, t.state.error)) : null,
    t.state?.state === "waiting" ? h("div", { class: "alert" }, icon("warn", 15), h("span", null, t.state.reason)) : null,
    h(
      "div",
      { class: "ti-block" },
      kv("Route", h("code", null, s.reverse ? `${s.right} → ${s.left}` : `${s.left} → ${s.right}`)),
      kv("Via", viaText(host)),
      addr ? kv("Address", h("code", null, addr)) : null,
    ),
    h(
      "div",
      { class: "ti-stats" },
      h("div", null, h("b", null, String(st?.active ?? 0)), h("span", null, "active")),
      h("div", null, h("b", null, String(st?.accepted ?? 0)), h("span", null, "accepted")),
      h("div", { class: (st?.failed ?? 0) > 0 ? "bad" : "" }, h("b", null, String(st?.failed ?? 0)), h("span", null, "failed")),
    ),
    h(
      "div",
      { class: "ti-block" },
      kv("Rate", t.running ? fmtRate(bps(t.id)) : "–"),
      kv("Sent", bytes(st?.tx ?? 0)),
      kv("Received", bytes(st?.rx ?? 0)),
    ),
    toggle(t.autostart, "Start when the vault unlocks", (v) => void setAutostart(t, v)),
    h(
      "div",
      { class: "ti-actions" },
      btn(t.running ? "Stop" : "Start", { icon: t.running ? "stop" : "play", primary: !t.running, onClick: () => void startStop(t) }),
      btn("Edit", { icon: "edit", onClick: () => openTunnelForm(t) }),
      addr ? btn("", { icon: "copy", title: "Copy address", onClick: () => void copyText(addr, "Address copied") }) : null,
      btn("", {
        icon: "trash",
        danger: true,
        title: "Delete tunnel",
        onClick: async () => {
          if (await remove(t)) close();
        },
      }),
    ),
  );
}

function listView(list: Tunnel[]): HTMLElement {
  return h(
    "div",
    { class: "list tn-list" },
    list.map((t) => {
      const st = stateOf(t);
      const v = bps(t.id);
      return h(
        "div",
        { class: "list-row" },
        h("span", { class: `tn-kind k-${t.forward.kind}`, title: `${kindName(t.forward.kind)} forward` }, kindLetter(t.forward.kind)),
        h(
          "div",
          { class: "grow" },
          h("div", { class: "title" }, t.label, t.autostart ? h("span", { class: "badge tn-auto" }, "autostart") : null),
          h("div", { class: "meta mono" }, routeText(t)),
        ),
        h("div", { class: "tn-status" }, h("span", { class: `dot ${st.tone}${st.tone === "warn" ? " pulse" : ""}` }), h("span", { title: st.text }, fit(st.text, 40))),
        h("div", { class: "tn-rate mono" }, t.running ? `${v > 0 ? fmtRate(v) : "idle"} · ${t.stats?.active ?? 0}` : "–"),
        h(
          "div",
          { class: "actions" },
          btn(t.running ? "Stop" : "Start", { icon: t.running ? "stop" : "play", small: true, primary: !t.running, onClick: () => void startStop(t) }),
          btn("", { icon: "edit", small: true, ghost: true, title: "Edit", onClick: () => openTunnelForm(t) }),
          btn("", { icon: "trash", small: true, ghost: true, title: "Delete", onClick: () => void remove(t) }),
        ),
      );
    }),
  );
}

export function tunnelsPage(): Page {
  return {
    id: "tunnels",
    title: "Tunnels",
    icon: "tunnel",
    render(root) {
      const sub = h("div", { class: "sub" });
      const body = h("div", { class: "tn-body" });
      const graph = new Graph();
      const insp = h("div", { class: "tn-inspector-slot" });
      const graphWrap = h(
        "div",
        { class: "tn-graph" },
        graph.el,
        h(
          "div",
          { class: "tg-controls" },
          btn("", { icon: "plus", small: true, ghost: true, title: "Zoom in", onClick: () => graph.zoomAt(1.2, graph.svg.clientWidth / 2, graph.svg.clientHeight / 2) }),
          h("button", { class: "btn small ghost icon-only tg-minus", title: "Zoom out", "aria-label": "Zoom out", onClick: () => graph.zoomAt(1 / 1.2, graph.svg.clientWidth / 2, graph.svg.clientHeight / 2) }, h("span", null, "−")),
          btn("Fit", { small: true, ghost: true, title: "Fit to view", onClick: () => graph.fit() }),
        ),
        h("div", { class: "tg-legend" }, h("span", null, "This machine"), h("span", null, "Through"), h("span", null, "Remote network")),
        insp,
      );
      const switcher = segmented<Mode>(
        [
          { value: "graph", label: "Graph" },
          { value: "list", label: "List" },
        ],
        mode,
        (v) => {
          mode = v;
          paint();
        },
      );
      const head = h(
        "div",
        { class: "page-head tn-head" },
        h("div", null, h("h1", null, "Tunnels"), sub),
        h("div", { class: "spacer" }),
        switcher,
        btn("New tunnel", { icon: "plus", primary: true, onClick: () => openTunnelForm() }),
      );
      const wrap = h("div", { class: "tn-page" }, head, body);
      root.append(wrap);

      const select_ = (id: Id | null) => {
        graph.selected = id;
        paintInspector();
        graph.render(tunnels.get());
      };
      graph.onSelect = select_;

      const paintInspector = () => {
        clear(insp);
        const t = tunnels.get().find((x) => x.id === graph.selected);
        if (!t) {
          graph.selected = null;
          insp.hidden = true;
          return;
        }
        insp.hidden = false;
        insp.append(inspector(t, () => select_(null)));
      };

      let lastMode: Mode | null = null;
      const paint = () => {
        const list = tunnels.get();
        const running = list.filter((t) => t.running).length;
        const active = list.reduce((n, t) => n + (t.stats?.active ?? 0), 0);
        sub.textContent = list.length
          ? `${running} of ${list.length} running · ${active} active connection${active === 1 ? "" : "s"}`
          : "Local, remote and SOCKS5 forwards that survive reconnects";
        if (!list.length) {
          lastMode = null;
          body.replaceChildren(
            empty(
              "No tunnels yet",
              "Forward a remote database to localhost, publish a local service on a server, or open a SOCKS5 proxy. Listeners stay bound while the link heals.",
              btn("Create a tunnel", { icon: "plus", primary: true, onClick: () => openTunnelForm() }),
            ),
          );
          return;
        }
        if (mode === "graph") {
          if (lastMode !== "graph") {
            body.replaceChildren(graphWrap);
            lastMode = "graph";
            graph.fitted = false;
          }
          graph.render(list);
          paintInspector();
        } else {
          lastMode = "list";
          body.replaceChildren(listView(list));
        }
      };

      let queued = false;
      const schedule = () => {
        if (queued) return;
        queued = true;
        requestAnimationFrame(() => {
          queued = false;
          paint();
        });
      };

      const unsubs = [
        tunnels.subscribe((list) => {
          track(list);
          schedule();
        }),
        hosts.subscribe(schedule, false),
        links.subscribe(schedule, false),
      ];
      refreshTunnels().catch((e) => fail(e, "Could not load tunnels"));
      return () => {
        for (const u of unsubs) u();
        graph.dispose();
      };
    },
  };
}
