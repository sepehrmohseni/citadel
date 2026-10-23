import { api, type Id } from "./api";
import { clear, coalesce, h, icon, isMac, kbd, keyOf, modKey } from "./dom";
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
  dispose(): void;
}

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
let leaderAt = 0;
const tabListeners = new Set<() => void>();

const HOTKEYS: Record<string, string> = { hosts: "h", tunnels: "t", keys: "k", snippets: "s", audit: "a", settings: "," };

export const factories: { terminal?: TabFactory; files?: TabFactory; palette?: () => void } = {};

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
        { class: `rail-btn${on ? " on" : ""}`, title: `${p.title} (g ${HOTKEYS[p.id] ?? ""})`, "aria-label": p.title, dataset: { page: p.id }, onClick: () => go(p.id) },
        h("span", { class: "rail-key" }, HOTKEYS[p.id] ?? "·"),
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
        title: `${t.title}${i < 9 ? ` (${modKey}+${i + 1})` : ""}`,
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
    h("span", { class: "status-item" }, kbd("?"), " keys"),
    h("span", { class: "status-item" }, kbd(modKey, "K"), " palette"),
    h("span", { class: "status-item" }, h("b", null, hhmm())),
  );
  renderRail();
}

function typing(): boolean {
  const a = document.activeElement as HTMLElement | null;
  if (!a) return false;
  return a.tagName === "INPUT" || a.tagName === "TEXTAREA" || a.tagName === "SELECT" || a.isContentEditable || !!a.closest(".xterm");
}

export function showHelp(): void {
  const row = (keys: HTMLElement, what: string) => h("div", { class: "help-row" }, h("span", null, what), keys);
  modal({
    title: "Keyboard",
    wide: true,
    body: h(
      "div",
      { class: "help-grid" },
      h("h4", null, "Anywhere"),
      row(kbd(modKey, "K"), "command palette"),
      row(kbd("g", "h"), "go to hosts"),
      row(kbd("g", "t"), "go to tunnels"),
      row(kbd("g", "k"), "go to keys"),
      row(kbd("g", "s"), "go to snippets"),
      row(kbd("g", "a"), "go to audit"),
      row(kbd("g", ","), "go to settings"),
      row(kbd(modKey, "1-9"), "switch tab"),
      row(kbd(modKey, "W"), "close tab"),
      row(kbd("Ctrl", "Tab"), "next tab"),
      row(kbd("?"), "this sheet"),
      h("h4", null, "Hosts"),
      row(kbd("j", "k"), "move selection"),
      row(kbd("⏎"), "connect"),
      row(kbd("f"), "open files"),
      row(kbd("e"), "edit host"),
      row(kbd("n"), "new host"),
      row(kbd("d"), "delete host"),
      row(kbd("/"), "filter"),
      row(kbd("i"), "import ~/.ssh/config"),
      h("h4", null, "Terminal"),
      row(kbd(isMac ? "⌘" : "Ctrl+Shift", "F"), "find in scrollback"),
      row(kbd(isMac ? "⌘" : "Ctrl+Shift", "C"), "copy selection"),
      row(kbd(isMac ? "⌘" : "Ctrl+Shift", "V"), "paste (guarded)"),
      row(kbd(isMac ? "⌘" : "Ctrl+Shift", "B"), "broadcast input"),
      row(kbd(isMac ? "⌘" : "Ctrl+Shift", "R"), "record session"),
      row(kbd("⏎"), "reconnect closed session"),
    ),
  });
}

function onKey(e: KeyboardEvent): void {
  if (document.querySelector(".scrim")) return;
  const key = keyOf(e);
  const mod = isMac ? e.metaKey : e.ctrlKey;
  if (mod && !e.shiftKey && !e.altKey && key.toLowerCase() === "k") {
    e.preventDefault();
    factories.palette?.();
    return;
  }
  if (mod && !e.shiftKey && /^[1-9]$/.test(key)) {
    const t = tabs[Number(key) - 1];
    if (t) {
      e.preventDefault();
      focusTab(t.key);
    }
    return;
  }
  if (mod && key.toLowerCase() === "w" && !e.shiftKey && activeTab) {
    e.preventDefault();
    closeTab(activeTab);
    return;
  }
  if (e.ctrlKey && e.key === "Tab" && tabs.length) {
    e.preventDefault();
    const i = tabs.findIndex((t) => t.key === activeTab);
    const next = tabs[(i + (e.shiftKey ? tabs.length - 1 : 1) + tabs.length) % tabs.length];
    if (next) focusTab(next.key);
    return;
  }
  if (e.ctrlKey || e.metaKey || e.altKey || typing()) return;
  if (key === "?") {
    e.preventDefault();
    showHelp();
    return;
  }
  if (key === "g") {
    leaderAt = Date.now();
    return;
  }
  if (Date.now() - leaderAt < 1200) {
    const target = Object.entries(HOTKEYS).find(([, k]) => k === key)?.[0];
    leaderAt = 0;
    if (target) {
      e.preventDefault();
      go(target);
    }
  }
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
    kbd(modKey, "K"),
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
  window.addEventListener("keydown", onKey, true);
  const first = pages[0];
  if (first) go(first.id);
  return () => {
    window.removeEventListener("keydown", onKey, true);
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
