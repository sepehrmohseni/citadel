import { open } from "@tauri-apps/plugin-dialog";

import { api, errCode, errText, type BackupInfo } from "../api";
import { ago, foreignKey, h } from "../dom";
import { input } from "../ui";

const CREST = ["█▀▀ █ ▀█▀ ▄▀█ █▀▄ █▀▀ █  ", "█▄▄ █  █  █▀█ █▄▀ ██▄ █▄▄"].join("\n");
const CRENEL = "▄▄  ".repeat(16);

function score(pw: string): number {
  if (!pw) return 0;
  let classes = 0;
  if (/[a-z]/.test(pw)) classes++;
  if (/[A-Z]/.test(pw)) classes++;
  if (/\d/.test(pw)) classes++;
  if (/[^A-Za-z0-9]/.test(pw)) classes++;
  const entropy = pw.length * Math.log2([0, 26, 52, 62, 94][classes] ?? 26);
  if (entropy >= 90) return 4;
  if (entropy >= 65) return 3;
  if (entropy >= 45) return 2;
  return 1;
}

const LABELS = ["", "weak", "fair", "strong", "excellent"];

export function lockScreen(initialized: boolean, onUnlocked: () => void): HTMLElement {
  const pw = input({ type: "password", placeholder: initialized ? "" : "8+ characters", autocomplete: "current-password", "aria-label": "Master passphrase" });
  const confirm = input({ type: "password", "aria-label": "Confirm passphrase" });
  const msg = h("div", { class: "lock-msg", role: "alert" });
  const hint = h("div", { class: "lock-hint", "aria-live": "polite" });
  let restoring: string | null = null;
  const lede = h(
    "p",
    { class: "lock-lede" },
    initialized
      ? "Vault sealed. Enter your passphrase to derive the key; nothing is decrypted until you do."
      : "New vault. Pick a passphrase. It never leaves this machine and cannot be recovered, so keep it somewhere safe.",
  );
  const restoreBox = h("div", { class: "lock-restore", hidden: true });
  const meter = h("div", { class: "strength", dataset: { score: "0" } }, h("span"), h("span"), h("span"), h("span"));
  const meterLabel = h("span", { class: "dim" });
  const verb = initialized ? "unlock" : "create vault";
  const submit = h("button", { type: "submit", class: "btn primary" }, h("span", null, verb), h("span", { class: "hk" }, "⏎"));
  const card = h(
    "form",
    { class: "lock-card ticks", onSubmit: (e: Event) => { e.preventDefault(); void go(); } },
    h("pre", { class: "crenel", "aria-hidden": "true" }, CRENEL),
    h("pre", { class: "crest", "aria-label": "Citadel" }, CREST),
    h("div", { class: "lock-tagline" }, "local-first ssh · tunnels · encrypted vault"),
    lede,
    h("label", { class: "prompt-line" }, h("span", { class: "who" }, "passphrase"), pw),
    hint,
    initialized ? null : h("div", { class: "prompt-line create-only" }, h("span"), h("div", { class: "row", style: { gap: "12px" } }, h("div", { style: { flex: "1" } }, meter), meterLabel)),
    initialized ? null : h("label", { class: "prompt-line create-only" }, h("span", { class: "who" }, "confirm"), confirm),
    h("div", { class: "derive", "aria-live": "polite" }, h("span", { class: "bar" }), h("span", null, "deriving key · argon2id 64 MiB")),
    msg,
    submit,
    restoreBox,
    h(
      "div",
      { class: "lock-specs" },
      h("span", null, "argon2id m=64MiB t=3 p=4"),
      h("span", null, "xchacha20-poly1305"),
      h("span", null, "zero-knowledge"),
    ),
  );
  const watchKeys = (e: KeyboardEvent) => {
    const notes: string[] = [];
    if (e.getModifierState?.("CapsLock")) notes.push("caps lock is on");
    if (foreignKey(e)) notes.push(`non-latin keyboard layout active (${e.key})`);
    hint.textContent = notes.join(" · ");
  };
  pw.addEventListener("keydown", watchKeys);
  confirm.addEventListener("keydown", watchKeys);
  const enterRestore = (path: string, label: string) => {
    restoring = path;
    card.classList.add("restoring");
    lede.textContent = `Restoring ${label}. Enter the passphrase the vault had when this backup was taken.`;
    const verbEl = submit.querySelector("span");
    if (verbEl) verbEl.textContent = "restore";
    msg.textContent = "";
    pw.focus();
  };
  const offerBackups = (list: BackupInfo[]) => {
    const latest = list[0];
    if (!latest) return;
    restoreBox.hidden = false;
    restoreBox.replaceChildren(
      h("div", { class: "dim" }, `found ${list.length} encrypted backup${list.length === 1 ? "" : "s"} · newest ${ago(latest.created_ms)}`),
      h(
        "div",
        { class: "row" },
        h("button", { type: "button", class: "btn small", onClick: () => enterRestore(latest.path, "the newest backup") }, "restore newest"),
        h(
          "button",
          {
            type: "button",
            class: "btn small ghost",
            onClick: async () => {
              const picked = await open({ multiple: false, filters: [{ name: "Citadel backup", extensions: ["ctdl"] }] });
              if (typeof picked === "string") enterRestore(picked, picked.split(/[\\/]/).pop() ?? "backup");
            },
          },
          "choose file…",
        ),
      ),
    );
  };
  if (!initialized) {
    void api.backups().then(offerBackups, () => undefined);
  }
  pw.addEventListener("input", () => {
    msg.textContent = "";
    if (!initialized) {
      const s = score(pw.value);
      meter.dataset.score = String(s);
      meterLabel.textContent = LABELS[s] ?? "";
    }
  });
  const busy = (on: boolean) => {
    card.classList.toggle("busy", on);
    submit.disabled = on;
    pw.disabled = on;
    confirm.disabled = on;
  };
  const shake = () => {
    card.classList.remove("shake");
    void card.offsetWidth;
    card.classList.add("shake");
  };
  const go = async () => {
    const value = pw.value;
    if (restoring) {
      busy(true);
      try {
        await api.backupRestore(restoring, value, false);
        pw.value = "";
        onUnlocked();
      } catch (e) {
        msg.textContent = errCode(e) === "bad_password" ? "That passphrase does not open this backup." : errText(e);
        shake();
        pw.select();
      } finally {
        busy(false);
        pw.focus();
      }
      return;
    }
    if (!initialized) {
      if (value.length < 8) {
        msg.textContent = "Use at least 8 characters.";
        shake();
        return;
      }
      if (value !== confirm.value) {
        msg.textContent = "Passphrases do not match.";
        shake();
        return;
      }
    }
    busy(true);
    try {
      if (initialized) await api.vaultUnlock(value);
      else await api.vaultCreate(value);
      pw.value = "";
      confirm.value = "";
      onUnlocked();
    } catch (e) {
      msg.textContent = errCode(e) === "bad_password" ? "Wrong passphrase." : errText(e);
      shake();
      pw.select();
    } finally {
      busy(false);
      pw.focus();
    }
  };
  const root = h("div", { class: "lock" }, card);
  requestAnimationFrame(() => pw.focus());
  return root;
}
