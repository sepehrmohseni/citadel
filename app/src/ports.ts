import { api, type Id, type Listener, type Tunnel } from "./api";
import { h, icon } from "./dom";
import { refreshTunnels, tunnels } from "./store";
import { btn, fail, toast } from "./ui";

const REFRESH_MS = 5000;

function forwardOf(host: Id, port: number): Tunnel | undefined {
  return tunnels.get().find((t) => t.host === host && t.forward.kind === "local" && t.forward.dest.port === port);
}

export async function forwardPort(host: Id, label: string, l: Listener, open: boolean): Promise<void> {
  let t = forwardOf(host, l.port);
  if (!t) {
    const local = await api.portFree(l.port);
    t = await api.tunnelSave({
      id: null,
      label: `${label} :${l.port}${l.process ? ` ${l.process}` : ""}`,
      host,
      autostart: false,
      forward: { kind: "local", bind: { host: "127.0.0.1", port: local }, dest: { host: l.address, port: l.port } },
    });
  }
  if (!t.running) t = await api.tunnelStart(t.id);
  await refreshTunnels();
  const bind = t.forward.kind === "local" ? t.forward.bind.port : l.port;
  if (open) await api.openLocal(bind);
  else toast(`localhost:${bind} now reaches ${label}:${l.port}`, "ok", 3500);
}

export class PortsPanel {
  el: HTMLElement;
  #host: Id;
  #label: string;
  #timer: ReturnType<typeof setInterval> | undefined;
  #list = h("div", { class: "ports-list" });
  #busy = false;
  #unsub: (() => void) | null = null;
  #last: Listener[] = [];

  constructor(host: Id, label: string) {
    this.#host = host;
    this.#label = label;
    this.el = h(
      "aside",
      { class: "ports-panel", hidden: true },
      h("div", { class: "tm-head" }, h("span", { class: "host" }, "listening ports"), h("span", { class: "dim" }, "forward to localhost in one click")),
      this.#list,
    );
  }

  get open(): boolean {
    return !this.el.hidden;
  }

  toggle(): void {
    if (this.open) this.stop();
    else this.start();
  }

  start(): void {
    this.el.hidden = false;
    this.#list.replaceChildren(h("div", { class: "muted" }, "Asking the server…"));
    this.#unsub ??= tunnels.subscribe(() => this.#render(), false);
    void this.refresh();
    this.#timer ??= setInterval(() => void this.refresh(), REFRESH_MS);
  }

  stop(): void {
    this.el.hidden = true;
    if (this.#timer) clearInterval(this.#timer);
    this.#timer = undefined;
    this.#unsub?.();
    this.#unsub = null;
  }

  dispose(): void {
    this.stop();
  }

  async refresh(): Promise<void> {
    if (this.#busy || document.hidden) return;
    this.#busy = true;
    try {
      this.#last = await api.ports(this.#host);
      this.#render();
    } catch (e) {
      this.#list.replaceChildren(h("div", { class: "alert" }, icon("warn", 15), h("span", null, String((e as { message?: string })?.message ?? e))));
    } finally {
      this.#busy = false;
    }
  }

  #render(): void {
    if (!this.open) return;
    if (!this.#last.length) {
      this.#list.replaceChildren(h("div", { class: "muted" }, "No TCP listeners reported (needs ss or netstat on the server)."));
      return;
    }
    this.#list.replaceChildren(
      ...this.#last.map((l) => {
        const t = forwardOf(this.#host, l.port);
        const live = t?.running && t.forward.kind === "local";
        return h(
          "div",
          { class: `port-row${live ? " live" : ""}` },
          h("span", { class: "port" }, String(l.port)),
          h("span", { class: "proc", title: l.pid ? `pid ${l.pid}` : "" }, l.process ?? "–"),
          h("span", { class: `scope ${l.scope}` }, l.scope === "any" ? "all interfaces" : l.scope === "loopback" ? "loopback" : l.address),
          h(
            "span",
            { class: "acts" },
            live && t.forward.kind === "local" ? h("span", { class: "dim" }, `→ :${t.forward.bind.port}`) : null,
            btn(live ? "open" : "forward", {
              small: true,
              onClick: () => void forwardPort(this.#host, this.#label, l, true).catch((e) => fail(e, "Forward failed")),
            }),
            live && t ? btn("stop", { small: true, ghost: true, onClick: () => void api.tunnelStop(t.id).then(refreshTunnels).catch((e) => fail(e)) }) : null,
          ),
        );
      }),
    );
  }
}
