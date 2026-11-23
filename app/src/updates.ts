import { api, events, type UpdateState } from "./api";
import { ago } from "./dom";
import { isLocked, whenUnlocked } from "./shell";
import { Signal, prefs, updatePrefs } from "./store";
import { connectedSessions } from "./terminal";
import { actionToast, confirmBox, fail, toast } from "./ui";

/** The update state reported by the Rust side; Settings › About renders it. */
export const update = new Signal<UpdateState>({ state: "idle" });

const announced = new Set<string>();

export function initUpdates(): void {
  void api
    .updateStatus()
    .then((s) => update.set(s))
    .catch(() => undefined);
  void events.update((s) => {
    update.set(s);
    void announce(s);
  });
}

/** One line describing the state, for Settings. */
export function describeUpdate(s: UpdateState): string {
  switch (s.state) {
    case "idle":
      return "Citadel has not checked for updates yet.";
    case "checking":
      return "Checking for updates…";
    case "up_to_date":
      return `Up to date. Checked ${ago(s.checked_ms)}.`;
    case "available":
      if (prefs.get()?.skip_version === s.version) return `Citadel ${s.version} is available. You chose to skip it.`;
      return s.install === "manual" ? `Citadel ${s.version} is available to download.` : `Citadel ${s.version} is available.`;
    case "downloading": {
      const pct = s.total ? ` ${Math.min(100, Math.floor((s.done / s.total) * 100))}%` : "";
      return `Downloading Citadel ${s.version}…${pct}`;
    }
    case "ready":
      if (s.install === "password") return `Citadel ${s.version} is downloaded. Installing it asks for your system password.`;
      return `Citadel ${s.version} is ready. It installs when you quit or restart Citadel.`;
    case "installing":
      return `Installing Citadel ${s.version}…`;
    case "failed":
      return `The last update check failed (${ago(s.checked_ms)}): ${s.error}`;
  }
}

/** Installs a downloaded update after the user confirms, then Citadel restarts. */
export async function installUpdate(): Promise<void> {
  const s = update.get();
  if (s.state !== "ready") return;
  const open = connectedSessions();
  const sessions = open ? ` ${open} connected session${open === 1 ? "" : "s"} will be closed.` : "";
  const tabs = (prefs.get()?.restore_tabs ?? true) ? " Your tabs come back after the restart." : "";
  const password = s.install === "password" ? " Your system asks for your password to install it." : "";
  const ok = await confirmBox(`Update to Citadel ${s.version}`, `Citadel restarts to finish the update.${password}${sessions}${tabs}`, {
    confirm: "Restart and update",
  });
  if (!ok) return;
  try {
    await api.updateInstall();
  } catch (e) {
    fail(e, "Update not installed");
  }
}

export async function checkForUpdates(): Promise<void> {
  try {
    const s = await api.updateCheck();
    if (s.state === "up_to_date") toast("Citadel is up to date.", "ok", 3000);
    else if (s.state === "failed") toast(describeUpdate(s), "warn", 6000);
  } catch (e) {
    fail(e, "Could not check for updates");
  }
}

export function skipVersion(version: string): void {
  void updatePrefs({ skip_version: version })
    .then(() => toast(`Citadel ${version} skipped. Settings › About can still install it.`, "info", 5000))
    .catch((e) => fail(e));
}

/** Tells the user about a new version once, without interrupting: a toast with the next step. */
async function announce(s: UpdateState): Promise<void> {
  if (s.state !== "available" && s.state !== "ready") return;
  const p = prefs.get();
  if (p?.skip_version === s.version) return;
  // In automatic mode a downloadable update says nothing until it is ready to install.
  if (s.state === "available" && (p?.updates ?? "auto") === "auto" && s.install !== "manual") return;
  const key = `${s.state}:${s.version}`;
  if (announced.has(key)) return;
  announced.add(key);
  if (isLocked()) await whenUnlocked();
  const skip = { label: "skip this version", run: () => skipVersion(s.version) };
  if (s.state === "available" && s.install === "manual") {
    actionToast(`Citadel ${s.version} is available.`, [{ label: "download", run: () => void api.updateOpen().catch((e) => fail(e)) }, skip], 20000);
  } else if (s.state === "available") {
    actionToast(`Citadel ${s.version} is available.`, [{ label: "download and install", run: () => void api.updateDownload().catch((e) => fail(e)) }, skip], 20000);
  } else if (s.install === "password") {
    actionToast(`Citadel ${s.version} is downloaded. Installing it asks for your system password.`, [{ label: "install", run: () => void installUpdate() }], 20000);
  } else {
    actionToast(`Citadel ${s.version} is ready. It installs when you quit Citadel.`, [{ label: "restart now", run: () => void installUpdate() }], 20000);
  }
}
