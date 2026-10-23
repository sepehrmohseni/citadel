import { api } from "./api";
import { fuzzy, h, icon, kbd } from "./dom";
import { focusTab, go, listTabs, openFiles, openTerminal } from "./shell";
import { hosts, refreshHosts, snippets } from "./store";
import { parseSsh } from "./target";
import { activeTerminalKey, sendText } from "./terminal";
import { fail, toast } from "./ui";
import { hostForm } from "./views/hosts";
import { openSettings } from "./views/settings";

interface Item {
  group: string;
  title: string;
  hint?: string;
  icon: string;
  keywords?: string;
  run: () => void | Promise<void>;
}

export { parseQuick, parseSsh } from "./target";

function quickConnect(q: string): Item | null {
  const target = parseSsh(q);
  if (!target) return null;
  return {
    group: "Quick connect",
    title: `Connect to ${q.trim()}`,
    hint: "saved to the vault, authenticates like the ssh command (agent, then ~/.ssh keys)",
    icon: "bolt",
    run: async () => {
      const saved = await api.hostSave({
        id: null,
        label: q.trim(),
        hostname: target.host,
        port: target.port,
        user: target.user ?? "root",
        auth: { kind: "agent", fingerprint: null },
        jump: [],
        workspace: null,
        tags: ["quick"],
        keepalive_secs: 15,
        agent_forward: false,
        legacy_crypto: false,
        ssh_alias: null,
      });
      await refreshHosts();
      openTerminal(saved.id);
    },
  };
}

function items(): Item[] {
  const out: Item[] = [];
  for (const t of listTabs()) out.push({ group: "Open tabs", title: t.title, icon: t.icon, hint: "switch", run: () => focusTab(t.key) });
  for (const x of hosts.get()) {
    const addr = `${x.user}@${x.hostname}${x.port !== 22 ? `:${x.port}` : ""}`;
    out.push({ group: "Hosts", title: x.label, hint: addr, icon: "terminal", keywords: `${addr} ${x.tags.join(" ")} ${x.workspace ?? ""} ssh connect`, run: () => openTerminal(x.id) });
    out.push({ group: "Files", title: `${x.label} files`, hint: addr, icon: "folder", keywords: `${addr} sftp files browse`, run: () => openFiles(x.id) });
  }
  const term = activeTerminalKey();
  for (const s of snippets.get()) {
    out.push({
      group: "Snippets",
      title: s.label,
      hint: s.body.split("\n")[0]?.slice(0, 60),
      icon: "snippet",
      keywords: s.body,
      run: () => {
        if (!term) {
          toast("Open a terminal first", "warn");
          return;
        }
        const body = s.body.replace(/\r?\n/g, "\r");
        sendText(term, body.endsWith("\r") ? body : `${body}\r`);
      },
    });
  }
  const actions: Item[] = [
    { group: "Actions", title: "New host", icon: "plus", run: () => hostForm() },
    { group: "Actions", title: "Fleet run", icon: "fleet", keywords: "run command many hosts parallel exec", run: () => go("fleet") },
    { group: "Actions", title: "Keyboard shortcuts", icon: "keyboard", keywords: "keybindings hotkeys customize global", run: () => openSettings("shortcuts") },
    { group: "Actions", title: "Tunnels", icon: "tunnel", keywords: "port forward socks", run: () => go("tunnels") },
    { group: "Actions", title: "Keys", icon: "key", keywords: "identity generate ssh key", run: () => go("keys") },
    { group: "Actions", title: "Snippets", icon: "snippet", run: () => go("snippets") },
    { group: "Actions", title: "Settings", icon: "settings", keywords: "preferences sync export import", run: () => go("settings") },
    { group: "Actions", title: "Hosts", icon: "hosts", run: () => go("hosts") },
    { group: "Actions", title: "Lock vault", icon: "lock", keywords: "lock secure", run: () => api.vaultLock() },
  ];
  out.push(...actions);
  return out;
}

let openNow = false;

export function openPalette(): void {
  if (openNow) return;
  openNow = true;
  const field = h("input", { placeholder: "Type a host, action or user@host:port", spellcheck: false, autocomplete: "off", "aria-label": "Command palette" }) as HTMLInputElement;
  const list = h("div", { class: "palette-list", role: "listbox" });
  let sel = 0;
  let shown: Item[] = [];
  const all = items();
  const render = () => {
    const q = field.value;
    const scored = all
      .map((it) => ({ it, s: Math.max(fuzzy(q, it.title), fuzzy(q, `${it.title} ${it.hint ?? ""} ${it.keywords ?? ""}`) - 40) }))
      .filter((r) => r.s >= 0);
    if (q) scored.sort((a, b) => b.s - a.s);
    const ranked = scored.map((r) => r.it);
    const order: string[] = [];
    for (const it of ranked) if (!order.includes(it.group)) order.push(it.group);
    shown = order.flatMap((g) => ranked.filter((it) => it.group === g)).slice(0, 60);
    const quick = quickConnect(q);
    if (quick && !shown.some((s) => s.hint === q.trim())) shown.unshift(quick);
    sel = Math.min(sel, Math.max(0, shown.length - 1));
    list.replaceChildren();
    let group = "";
    shown.forEach((it, i) => {
      if (it.group !== group) {
        group = it.group;
        list.append(h("div", { class: "palette-group" }, group));
      }
      list.append(
        h(
          "div",
          {
            class: `palette-item${i === sel ? " on" : ""}`,
            role: "option",
            "aria-selected": String(i === sel),
            onMousemove: () => {
              if (sel !== i) {
                sel = i;
                render();
              }
            },
            onClick: () => choose(i),
          },
          icon(it.icon, 16),
          h("div", { class: "grow" }, h("div", null, it.title), it.hint ? h("div", { class: "hint" }, it.hint) : null),
        ),
      );
    });
    if (!shown.length) list.append(h("div", { class: "palette-group" }, "No matches"));
    list.querySelector(".palette-item.on")?.scrollIntoView({ block: "nearest" });
  };
  const close = () => {
    scrim.classList.remove("in");
    openNow = false;
    setTimeout(() => scrim.remove(), 150);
  };
  const choose = (i: number) => {
    const it = shown[i];
    if (!it) return;
    close();
    Promise.resolve()
      .then(() => it.run())
      .catch((e) => fail(e));
  };
  field.addEventListener("input", () => {
    sel = 0;
    render();
  });
  field.addEventListener("keydown", (e) => {
    if (e.key === "ArrowDown") {
      e.preventDefault();
      sel = Math.min(shown.length - 1, sel + 1);
      render();
    } else if (e.key === "ArrowUp") {
      e.preventDefault();
      sel = Math.max(0, sel - 1);
      render();
    } else if (e.key === "Enter") {
      e.preventDefault();
      choose(sel);
    } else if (e.key === "Escape") {
      e.preventDefault();
      close();
    }
  });
  const scrim = h(
    "div",
    { class: "scrim palette-scrim", onMousedown: (e: MouseEvent) => { if (e.target === scrim) close(); } },
    h(
      "div",
      { class: "palette", role: "dialog", "aria-label": "Command palette" },
      h("div", { class: "palette-input" }, h("span", { class: "prompt" }, "›"), field),
      list,
      h("div", { class: "palette-foot" }, h("span", null, kbd("↑", "↓"), " navigate"), h("span", null, kbd("Enter"), " run"), h("span", null, kbd("Esc"), " close"), h("span", null, "user@host:port quick-connects")),
    ),
  );
  document.body.append(scrim);
  render();
  requestAnimationFrame(() => {
    scrim.classList.add("in");
    field.focus();
  });
}
