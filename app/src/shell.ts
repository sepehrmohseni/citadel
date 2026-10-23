import { api, type Id } from "./api";
import { clear, coalesce, h, icon, isMac, kbd } from "./dom";
import { COMMANDS, bind, bindings, install, onKeymapChange, pretty } from "./keymap";
import { hosts, identities, links, snippets, tunnels } from "./store";
import { fail, modal } from "./ui";

export interface Page {
  id: string;
  title: string;
  icon: string;
  render(root: HTMLElement): void | (() => void);
}

export type TabTone = "idle" | "ok" | "warn" | "err";

export interface Tab {
  key: string;
  title: string;
  icon: string;
  host?: Id;
  el: HTMLElement;
  tone?: TabTone;
  onShow?(): void;
  onHide?(): void;
  snapshot?(): TabSnapshot;
  closePane?(): boolean;
  dispose(): void;
}

export type TabSnapshot = { kind: "term"; layout: unknown } | { kind: "files"; host: Id };

type TabFactory = (host: Id) => Tab;

const pages: Page[] = [];
const tabs: Tab[] = [];
let activePage: string | null = null;
let activeTab: string | null = null;
let disposePage: (() => void) | null = null;

let stage!: HTMLElement;
let pageRoot!: HTMLElement;
let tabStrip!: HTMLElement;
let railNav!: HTMLElement;
let railFoot!: HTMLElement;
let statusLeft!: HTMLElement;
let statusRight!: HTMLElement;
let clock: ReturnType<typeof setInterval> | undefined;
let locked = false;
const tabListeners = new Set<() => void>();
const closed: TabSnapshot[] = [];

export const factories: {
  terminal?: TabFactory;
  files?: TabFactory;
  palette?: () => void;
  layout?: (spec: unknown) => Tab | null;
} = {};

const unlockWaiters: (() => void)[] = [];

export function setLocked(on: boolean): void {
  locked = on;
  if (!on) for (const fn of unlockWaiters.splice(0)) fn();
}

export function whenUnlocked(): Promise<void> {
  return locked ? new Promise((resolve) => unlockWaiters.push(resolve)) : Promise.resolve();
}

export function isLocked(): boolean {
  return locked;
}

export function notifyLayout(): void {
  emitTabs();
}

function railKey(page: string): string {
  const b = bindings(`go.${page}`)[0];
  const steps = b?.split(" ") ?? [];
  return (steps.length === 2 ? steps[1] : "")?.toLowerCase() ?? "";
}

export function registerPage(p: Page): void {
  pages.push(p);
}

export function onTabsChanged(fn: () => void): () => void {
  tabListeners.add(fn);
  return () => tabListeners.delete(fn);
}

function emitTabs(): void {
  for (const fn of [...tabListeners]) fn();
}

export function listTabs(): readonly Tab[] {
  return tabs;
}

export function currentTab(): Tab | undefined {
  return tabs.find((t) => t.key === activeTab);
}

export function go(id: string): void {
  const page = pages.find((p) => p.id === id);
  if (!page) return;
  hideActiveTab();
  activeTab = null;
  if (activePage !== id || !pageRoot.firstChild) {
    disposePage?.();
    disposePage = null;
    clear(pageRoot);
    activePage = id;
    const d = page.render(pageRoot);
    disposePage = typeof d === "function" ? d : null;
  }
  pageRoot.hidden = false;
  pageRoot.dispatchEvent(new Event("page:show"));
  renderRail();
  renderTabs();
}

function hideActiveTab(): void {
  const t = currentTab();
  if (t) {
    t.el.hidden = true;
    t.onHide?.();
  }
}

export function openTab(tab: Tab): void {
  const existing = tabs.find((t) => t.key === tab.key);
  if (existing) {
    tab.dispose();
    focusTab(existing.key);
    return;
  }
  tabs.push(tab);
  tab.el.hidden = true;
  tab.el.classList.add("tab-pane");
  stage.append(tab.el);
  focusTab(tab.key);
  emitTabs();
}

export function focusTab(key: string): void {
  const t = tabs.find((x) => x.key === key);
  if (!t) return;
  hideActiveTab();
  pageRoot.hidden = true;
  activeTab = key;
  t.el.hidden = false;
  renderRail();
  renderTabs();
  requestAnimationFrame(() => t.onShow?.());
}

export function closeTab(key: string): void {
  const i = tabs.findIndex((t) => t.key === key);
  if (i < 0) return;
  const [t] = tabs.splice(i, 1);
  if (!t) return;
  const snap = t.snapshot?.();
  if (snap) {
    closed.push(snap);
    if (closed.length > 10) closed.shift();
  }
  t.dispose();
  t.el.remove();
  if (activeTab === key) {
    activeTab = null;
    const next = tabs[Math.min(i, tabs.length - 1)];
    if (next) focusTab(next.key);
    else go(activePage ?? "hosts");
  }
  renderTabs();
  emitTabs();
}

export function retitleTab(key: string, title: string, tone?: TabTone): void {
  const t = tabs.find((x) => x.key === key);
  if (!t) return;
  t.title = title;
  if (tone) t.tone = tone;
  renderTabs();
}

export function openTerminal(host: Id): void {
  if (factories.terminal) openTab(factories.terminal(host));
}

export function openFiles(host: Id): void {
  if (factories.files) openTab(factories.files(host));
}

export function openSnapshot(snap: TabSnapshot): boolean {
  if (snap.kind === "files") {
    if (!hosts.get().some((x) => x.id === snap.host)) return false;
    openFiles(snap.host);
    return true;
  }
  const tab = factories.layout?.(snap.layout);
  if (!tab) return false;
  openTab(tab);
  return true;
}

function count(id: string): string {
  switch (id) {
    case "hosts":
      return String(hosts.get().length);
    case "tunnels": {
      const all = tunnels.get();
      const up = all.filter((t) => t.running).length;
      return all.length ? `${up}/${all.length}` : "0";
    }
    case "keys":
      return String(identities.get().length);
    case "snippets":
      return String(snippets.get().length);
    default:
      return "";
  }
}

function renderRail(): void {
  clear(railNav);
  for (const p of pages) {
    const on = activeTab === null && activePage === p.id;
    railNav.append(
      h(
        "button",
        { class: `rail-btn${on ? " on" : ""}`, title: `${p.title}${railKey(p.id) ? ` (g ${railKey(p.id)})` : ""}`, "aria-label": p.title, dataset: { page: p.id }, onClick: () => go(p.id) },
        h("span", { class: "rail-key" }, railKey(p.id) || "·"),
        h("span", { class: "rail-text" }, p.title.toLowerCase()),
        h("span", { class: "rail-count" }, count(p.id)),
      ),
    );
  }
  const all = links.get();
  const up = [...all.values()].filter((s) => s.state === "up").length;
  railFoot.replaceChildren(
    h("div", null, "sessions ", h("b", null, String(up))),
    h("div", null, "terminals ", h("b", null, String(tabs.filter((t) => t.key.startsWith("term:")).length))),
    h("div", null, "press ", h("b", null, "?"), " for keys"),
  );
}

let dragKey: string | null = null;

function renderTabs(): void {
  clear(tabStrip);
  tabs.forEach((t, i) => {
    const el = h(
      "div",
      {
        class: `tab${t.key === activeTab ? " on" : ""} tone-${t.tone ?? "idle"}`,
        draggable: "true",
        role: "tab",
        tabindex: 0,
        title: `${t.title}${i < 9 && bindings(`tab.${i + 1}`)[0] ? ` (${pretty(bindings(`tab.${i + 1}`)[0] ?? "").join(" ")})` : ""}`,
        onMousedown: (e: MouseEvent) => {
          if (e.button === 1) {
            e.preventDefault();
            closeTab(t.key);
          }
        },
        onClick: () => focusTab(t.key),
        onDragstart: () => {
          dragKey = t.key;
        },
        onDragover: (e: DragEvent) => e.preventDefault(),
        onDrop: (e: DragEvent) => {
          e.preventDefault();
          if (!dragKey || dragKey === t.key) return;
          const from = tabs.findIndex((x) => x.key === dragKey);
          const to = tabs.findIndex((x) => x.key === t.key);
          if (from < 0 || to < 0) return;
          const [moved] = tabs.splice(from, 1);
          if (moved) tabs.splice(to, 0, moved);
          dragKey = null;
          renderTabs();
          emitTabs();
        },
      },
      h("span", { class: "tab-num" }, `${i + 1}`),
      h("span", { class: "tab-dot" }),
      h("span", { class: "tab-title" }, `${t.icon === "folder" ? "sftp:" : ""}${t.title.replace(/ · files$/, "")}`),
      h(
        "button",
        {
          class: "tab-x",
          title: "Close tab",
          "aria-label": "Close tab",
          onClick: (e: MouseEvent) => {
            e.stopPropagation();
            closeTab(t.key);
          },
        },
        icon("close", 12),
      ),
    );
    tabStrip.append(el);
  });
}

function hhmm(): string {
  const d = new Date();
  return `${String(d.getHours()).padStart(2, "0")}:${String(d.getMinutes()).padStart(2, "0")}`;
}

function renderStatus(): void {
  const all = links.get();
  const states = [...all.values()];
  const up = states.filter((s) => s.state === "up").length;
  const healing = states.filter((s) => s.state === "backoff" || s.state === "connecting").length;
  const running = tunnels.get().filter((t) => t.running).length;
  const active = tunnels.get().reduce((n, t) => n + (t.stats?.active ?? 0), 0);
  const rtts = states.flatMap((s) => (s.state === "up" && typeof s.rtt_ms === "number" ? [s.rtt_ms] : []));
  const rtt = rtts.length ? Math.round(rtts.reduce((a, b) => a + b, 0) / rtts.length) : null;
  statusLeft.replaceChildren(
    h("span", { class: "status-item mark" }, "◆ VAULT OPEN"),
    h("span", { class: "status-item" }, h("span", { class: `dot ${up ? "ok" : "idle"}` }), "links ", h("b", null, `${up}/${states.length}`)),
    ...(healing ? [h("span", { class: "status-item" }, h("span", { class: "dot warn pulse" }), h("b", null, String(healing)), " healing")] : []),
    h("span", { class: "status-item" }, "tunnels ", h("b", null, String(running)), " · conns ", h("b", null, String(active))),
    rtt !== null ? h("span", { class: "status-item" }, "rtt ", h("b", null, rtt < 1 ? "<1ms" : `${rtt}ms`)) : h("span"),
  );
  statusRight.replaceChildren(
    h("span", { class: "status-item" }, kbd(...pretty(bindings("help")[0] ?? "?")), " keys"),
    h("span", { class: "status-item" }, kbd(...pretty(bindings("palette")[0] ?? "")), " palette"),
    h("span", { class: "status-item" }, h("b", null, hhmm())),
  );
  renderRail();
}

export function showHelp(): void {
  const groups = new Map<string, HTMLElement[]>();
  for (const c of COMMANDS) {
    const b = bindings(c.id);
    if (!b.length) continue;
    const list = groups.get(c.group) ?? [];
    list.push(h("div", { class: "help-row" }, h("span", null, c.label), h("span", { class: "help-keys" }, b.slice(0, 2).map((x) => kbd(...pretty(x))))));
    groups.set(c.group, list);
  }
  modal({
    title: "Keyboard",
    wide: true,
    body: h(
      "div",
      { class: "help-grid" },
      [...groups.entries()].flatMap(([g, rows]) => [h("h4", null, g), ...rows]),
      h("p", { class: "muted help-foot" }, "Change any of these in Settings → Shortcuts."),
    ),
  });
}

function bindAppCommands(): (() => void)[] {
  const offs = [
    bind("palette", () => {
      factories.palette?.();
    }),
    bind("help", () => showHelp()),
    bind("lock", () => {
      void api.vaultLock().catch((e) => fail(e));
    }),
    bind("tab.next", () => {
      if (!tabs.length) return false;
      const i = tabs.findIndex((t) => t.key === activeTab);
      const next = tabs[(i + 1 + tabs.length) % tabs.length];
      if (next) focusTab(next.key);
    }),
    bind("tab.prev", () => {
      if (!tabs.length) return false;
      const i = tabs.findIndex((t) => t.key === activeTab);
      const next = tabs[(Math.max(i, 0) - 1 + tabs.length) % tabs.length];
      if (next) focusTab(next.key);
    }),
    bind("tab.close", () => {
      const t = currentTab();
      if (!t) return false;
      if (t.closePane?.()) return;
      closeTab(t.key);
    }),
    bind("tab.reopen", () => {
      while (closed.length) {
        const snap = closed.pop();
        if (snap && openSnapshot(snap)) return;
      }
      return false;
    }),
  ];
  for (let n = 1; n <= 9; n++) {
    offs.push(
      bind(`tab.${n}`, () => {
        const t = tabs[n - 1];
        if (!t) return false;
        focusTab(t.key);
      }),
    );
  }
  for (const page of ["hosts", "fleet", "tunnels", "keys", "snippets", "audit", "settings"]) {
    offs.push(
      bind(`go.${page}`, () => {
        if (!pages.some((p) => p.id === page)) return false;
        go(page);
      }),
    );
  }
  return offs;
}

export function mountShell(root: HTMLElement): () => void {
  tabStrip = h("div", { class: "tabs", role: "tablist" });
  railNav = h("nav", { class: "rail-nav" });
  railFoot = h("div", { class: "rail-foot" });
  pageRoot = h("section", { class: "page" });
  stage = h("main", { class: "stage" }, pageRoot);
  statusLeft = h("div", { class: "status-left" });
  statusRight = h("div", { class: "status-right" });
  const lockBtn = h(
    "button",
    { class: "btn small ghost", title: "Lock vault", onClick: () => api.vaultLock().catch((e) => fail(e)) },
    icon("lock", 14),
    h("span", null, "lock"),
  );
  const paletteBtn = h(
    "button",
    { class: "palette-trigger", onClick: () => factories.palette?.() },
    h("span", { class: "prompt" }, "›"),
    h("span", { class: "label" }, "jump to host, action, snippet"),
    kbd(...pretty(bindings("palette")[0] ?? "")),
  );
  const shell = h(
    "div",
    { class: `shell${isMac ? " mac" : ""}` },
    h(
      "header",
      { class: "titlebar", "data-tauri-drag-region": true },
      h("div", { class: "brand", "data-tauri-drag-region": true }, h("span", { class: "brand-glyph" }, "▙▟"), h("span", { class: "brand-word" }, "CITADEL")),
      tabStrip,
      h("div", { class: "title-actions" }, paletteBtn, lockBtn),
    ),
    h("aside", { class: "rail" }, h("div", { class: "rail-head" }, "navigate"), railNav, railFoot),
    stage,
    h("footer", { class: "statusbar" }, statusLeft, statusRight),
  );
  root.replaceChildren(shell);
  const statusLater = coalesce(renderStatus);
  const unsubs = [
    links.subscribe(statusLater),
    tunnels.subscribe(statusLater),
    hosts.subscribe(statusLater),
    identities.subscribe(renderRail, false),
    snippets.subscribe(renderRail, false),
    onTabsChanged(renderRail),
  ];
  clock = setInterval(renderStatus, 15000);
  const uninstall = install(() => locked || !!document.querySelector(".scrim.in"));
  const unbind = bindAppCommands();
  const unwatchKeys = onKeymapChange(() => {
    renderRail();
    renderTabs();
    renderStatus();
  });
  const first = pages[0];
  if (first) go(first.id);
  return () => {
    uninstall();
    for (const off of unbind) off();
    unwatchKeys();
    if (clock) clearInterval(clock);
    for (const u of unsubs) u();
    for (const t of tabs.splice(0)) {
      t.dispose();
      t.el.remove();
    }
    disposePage?.();
    disposePage = null;
    activePage = null;
    activeTab = null;
  };
}
