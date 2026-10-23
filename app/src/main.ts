import "./style.css";

import { api, events, type LockState } from "./api";
import { h } from "./dom";
import { openPalette } from "./palette";
import { initPrompts } from "./prompts";
import { restoreSession, trackSession } from "./session";
import { factories, mountShell, registerPage, setLocked } from "./shell";
import { clearSensitive, prefs, refreshAll, wireEvents } from "./store";
import { setOverrides } from "./keymap";
import { layoutTab, terminalTab, type LayoutSpec } from "./terminal";
import { fail } from "./ui";
import { filesTab } from "./views/files";
import { auditPage } from "./views/audit";
import { fleetPage } from "./views/fleet";
import { hostsPage } from "./views/hosts";
import { keysPage } from "./views/keys";
import { lockScreen } from "./views/lock";
import { settingsPage } from "./views/settings";
import { snippetsPage } from "./views/snippets";
import { tunnelsPage } from "./views/tunnels";

registerPage(hostsPage());
registerPage(fleetPage());
registerPage(tunnelsPage());
registerPage(keysPage());
registerPage(snippetsPage());
registerPage(auditPage());
registerPage(settingsPage());
factories.terminal = terminalTab;
factories.layout = (spec) => layoutTab(spec as LayoutSpec);
prefs.subscribe((p) => setOverrides(p?.keybindings));
factories.files = filesTab;
factories.palette = openPalette;

const touch = matchMedia("(pointer: coarse)").matches || /Android|iPhone|iPad/i.test(navigator.userAgent);
document.documentElement.classList.toggle("touch", touch);

const syncVisibility = () => document.documentElement.classList.toggle("app-hidden", document.hidden);
document.addEventListener("visibilitychange", syncVisibility);
syncVisibility();

const root = document.getElementById("root") ?? document.body;
const shellHost = h("div", { style: { height: "100%" } });
const lockHost = h("div", { class: "lock-layer" });
root.append(shellHost, lockHost);

let shellMounted = false;

async function unlocked(): Promise<void> {
  lockHost.replaceChildren();
  lockHost.hidden = true;
  shellHost.inert = false;
  for (const s of document.querySelectorAll<HTMLElement>(".scrim")) s.inert = false;
  setLocked(false);
  try {
    await refreshAll();
  } catch (e) {
    fail(e);
  }
  if (!shellMounted) {
    mountShell(shellHost);
    shellMounted = true;
    trackSession();
    void restoreSession().catch(() => undefined);
  }
}

function locked(state: LockState): void {
  setLocked(true);
  shellHost.inert = true;
  for (const s of document.querySelectorAll<HTMLElement>(".scrim")) s.inert = true;
  clearSensitive();
  lockHost.hidden = false;
  lockHost.replaceChildren(lockScreen(state !== "uninitialized", () => void unlocked()));
}

async function boot(): Promise<void> {
  document.addEventListener("contextmenu", (e) => {
    const t = e.target as HTMLElement | null;
    if (!t?.closest("input, textarea, .xterm, [data-context]")) e.preventDefault();
  });
  wireEvents();
  initPrompts();
  const status = await api.vaultStatus();
  void events.vault((state) => {
    if (state === "unlocked") {
      if (lockHost.hidden === false && lockHost.firstChild) void unlocked();
    } else locked(state);
  });
  if (status.state === "unlocked") await unlocked();
  else locked(status.state);
}

void boot().catch((e) => {
  root.replaceChildren(h("div", { class: "lock" }, h("div", { class: "lock-card" }, h("h1", null, "Citadel failed to start"), h("p", { class: "muted" }, String((e as { message?: string }).message ?? e)))));
});
