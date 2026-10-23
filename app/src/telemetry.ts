import { Channel, api, errText, type Id, type Snapshot, type TelemetryMsg } from "./api";
import { bytes, duration, h, icon, rate } from "./dom";

const HISTORY = 90;

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

export class Telemetry {
  el: HTMLElement;
  #host: Id;
  #id: number | null = null;
  #cpu: number[] = [];
  #rx: number[] = [];
  #tx: number[] = [];
  #starting = false;
  #disposed = false;

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
      const id = await api.telemetryStart(this.#host, feed);
      if (this.#disposed || this.el.hidden) {
        void api.telemetryStop(id).catch(() => undefined);
      } else {
        this.#id = id;
      }
    } catch (e) {
      this.el.replaceChildren(h("div", { class: "alert" }, icon("warn", 15), h("span", null, errText(e))));
    } finally {
      this.#starting = false;
    }
  }

  dispose(): void {
    this.#disposed = true;
    this.stop();
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
