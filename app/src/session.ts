import { api } from "./api";
import { debounce } from "./dom";
import { currentTab, focusTab, listTabs, onTabsChanged, openSnapshot, type TabSnapshot } from "./shell";
import { prefs } from "./store";
import { toast } from "./ui";

const MAX_TABS = 24;
let restoring = false;

const save = debounce(() => {
  if (restoring) return;
  const all = listTabs();
  const tabs = all.map((t) => t.snapshot?.()).filter((s): s is TabSnapshot => !!s);
  const cur = currentTab();
  const active = cur ? all.indexOf(cur) : -1;
  void api.sessionSave(JSON.stringify({ v: 1, tabs, active })).catch(() => undefined);
}, 800);

export function trackSession(): () => void {
  return onTabsChanged(save);
}

function snapshots(raw: unknown): { tabs: TabSnapshot[]; active: number } | null {
  if (!raw || typeof raw !== "object") return null;
  const r = raw as { v?: unknown; tabs?: unknown; active?: unknown };
  if (r.v !== 1 || !Array.isArray(r.tabs)) return null;
  const tabs: TabSnapshot[] = [];
  for (const t of r.tabs.slice(0, MAX_TABS)) {
    if (!t || typeof t !== "object") continue;
    const s = t as { kind?: unknown; host?: unknown; layout?: unknown };
    if (s.kind === "files" && typeof s.host === "string") tabs.push({ kind: "files", host: s.host });
    else if (s.kind === "term" && s.layout && typeof s.layout === "object") tabs.push({ kind: "term", layout: s.layout });
  }
  return { tabs, active: typeof r.active === "number" ? r.active : -1 };
}

export async function restoreSession(): Promise<number> {
  if (!(prefs.get()?.restore_tabs ?? true) || listTabs().length) return 0;
  const raw = await api.sessionLoad().catch(() => null);
  if (!raw) return 0;
  let parsed: unknown;
  try {
    parsed = JSON.parse(raw);
  } catch {
    return 0;
  }
  const s = snapshots(parsed);
  if (!s?.tabs.length) return 0;
  restoring = true;
  let n = 0;
  try {
    for (const t of s.tabs) if (openSnapshot(t)) n++;
  } finally {
    restoring = false;
  }
  const want = listTabs()[s.active];
  if (want) focusTab(want.key);
  if (n) toast(`Reopened ${n} tab${n === 1 ? "" : "s"} from the last session`, "info", 3000);
  save();
  return n;
}
