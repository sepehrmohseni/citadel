import "./pages.css";
import { THEMES, themeFor } from "../themes";
import { getVersion } from "@tauri-apps/api/app";
import { open, save } from "@tauri-apps/plugin-dialog";
import {
  api,
  errCode,
  errText,
  type AgentKey,
  type AgentSocket,
  type BackupInfo,
  type ConfigEntry,
  type Prefs,
  type SyncBackendInput,
  type SyncReport,
  type SyncTarget,
} from "../api";
import { ago, bytes, clear, debounce, h, icon } from "../dom";
import type { Page } from "../shell";
import { hosts, prefs, refreshAll } from "../store";
import { askText, btn, confirmBox, copyText, fail, field, input, modal, segmented, select, toast, toggle } from "../ui";

type SectionId = "vault" | "terminal" | "agent" | "openssh" | "sync" | "about";

interface Section {
  el: HTMLElement;
  dispose?: () => void;
}

const SECTIONS: { id: SectionId; label: string; icon: string }[] = [
  { id: "vault", label: "Vault", icon: "lock" },
  { id: "terminal", label: "Terminal", icon: "terminal" },
  { id: "agent", label: "SSH agent", icon: "key" },
  { id: "openssh", label: "OpenSSH config", icon: "file" },
  { id: "sync", label: "Sync", icon: "cloud" },
  { id: "about", label: "About", icon: "shield" },
];

const REPO = "https://github.com/sepehrmohseni/citadel";

async function currentPrefs(): Promise<Prefs> {
  const p = prefs.get();
  if (p) return p;
  const fetched = await api.prefsGet();
  prefs.set(fetched);
  return fetched;
}

async function savePrefs(patch: Partial<Prefs>): Promise<Prefs> {
  const next = await api.prefsSet({ ...(await currentPrefs()), ...patch });
  prefs.set(next);
  return next;
}

let pending: Partial<Prefs> = {};
const flush = debounce(() => {
  const patch = pending;
  pending = {};
  savePrefs(patch).catch((e) => fail(e, "Settings not saved"));
}, 350);

function queuePrefs(patch: Partial<Prefs>): void {
  pending = { ...pending, ...patch };
  flush();
}

function score(pw: string): number {
  if (!pw) return 0;
  const classes = [/[a-z]/, /[A-Z]/, /[0-9]/, /[^A-Za-z0-9]/].filter((r) => r.test(pw)).length;
  let s = 0;
  if (pw.length >= 8) s++;
  if (pw.length >= 12) s++;
  if (pw.length >= 16) s++;
  if (classes >= 3) s++;
  return Math.max(1, Math.min(4, s));
}

function strength(target: HTMLInputElement): HTMLElement {
  const meter = h("div", { class: "strength", "data-score": "0" }, h("span"), h("span"), h("span"), h("span"));
  target.addEventListener("input", () => meter.setAttribute("data-score", String(score(target.value))));
  return meter;
}

function card(title: string | null, ...rows: (HTMLElement | null)[]): HTMLElement {
  return h("div", { class: "set-card" }, title ? h("div", { class: "set-card-head" }, title) : null, rows);
}

function row(name: string, hint: string | null, ...ctl: (Node | null)[]): HTMLElement {
  return h(
    "div",
    { class: "set-row" },
    h("div", { class: "desc" }, h("div", { class: "name" }, name), hint ? h("div", { class: "hint" }, hint) : null),
    h("div", { class: "ctl" }, ctl),
  );
}

function head(title: string, lede: string): HTMLElement[] {
  return [h("h2", { class: "set-title" }, title), h("p", { class: "set-lede" }, lede)];
}

function changePasswordModal(): void {
  const cur = input({ type: "password", autocomplete: "current-password" });
  const next = input({ type: "password", autocomplete: "new-password" });
  const again = input({ type: "password", autocomplete: "new-password" });
  modal({
    title: "Change master password",
    body: h(
      "div",
      { class: "stack" },
      field("Current password", cur),
      field("New password", h("div", { class: "stack", style: { gap: "8px" } }, next, strength(next)), "At least 8 characters. A passphrase of several words is easier to type and harder to guess."),
      field("Confirm new password", again),
      h("div", { class: "note" }, icon("shield", 15), h("span", null, "Only the wrapped data key is re-encrypted; records are untouched. Devices that sync this vault adopt the new password on their next sync.")),
    ),
    actions: [
      { label: "Cancel", kind: "ghost", run: () => undefined },
      {
        label: "Change password",
        kind: "primary",
        run: async () => {
          if ([...next.value].length < 8) {
            toast("The new password must be at least 8 characters", "warn");
            return false;
          }
          if (next.value !== again.value) {
            toast("The new passwords do not match", "warn");
            return false;
          }
          await api.vaultChangePassword(cur.value, next.value);
          cur.value = next.value = again.value = "";
          toast("Master password changed", "ok");
          return true;
        },
      },
    ],
  });
}

function exportModal(): void {
  const pw = input({ type: "password", autocomplete: "new-password" });
  const again = input({ type: "password", autocomplete: "new-password" });
  modal({
    title: "Export encrypted bundle",
    body: h(
      "div",
      { class: "stack" },
      h("p", { class: "muted" }, "Writes every host, identity, tunnel, snippet and trusted host key into a single .ctdl file, sealed with its own password. Use it for backups or to move to a machine without sync."),
      field("Bundle password", h("div", { class: "stack", style: { gap: "8px" } }, pw, strength(pw)), "Independent of the master password. At least 8 characters."),
      field("Confirm password", again),
    ),
    actions: [
      { label: "Cancel", kind: "ghost", run: () => undefined },
      {
        label: "Choose location",
        kind: "primary",
        run: async () => {
          if ([...pw.value].length < 8) {
            toast("The bundle password must be at least 8 characters", "warn");
            return false;
          }
          if (pw.value !== again.value) {
            toast("The passwords do not match", "warn");
            return false;
          }
          const stamp = new Date().toISOString().slice(0, 10);
          const path = await save({
            title: "Export Citadel bundle",
            defaultPath: `citadel-${stamp}.ctdl`,
            filters: [{ name: "Citadel bundle", extensions: ["ctdl"] }],
          });
          if (!path) return false;
          const size = await api.vaultExport(path, pw.value);
          pw.value = again.value = "";
          toast(`Exported ${bytes(size)} to ${path}`, "ok", 6000);
          return true;
        },
      },
    ],
  });
}

function importModal(): void {
  let path: string | null = null;
  let mode: "merge" | "replace" = "merge";
  const pw = input({ type: "password" });
  const pathEl = h("span", { class: "path" }, "No file selected");
  const danger = h(
    "div",
    { class: "alert danger" },
    icon("warn", 15),
    h("span", null, "Replace erases every record in this vault and substitutes the bundle's contents. This cannot be undone."),
  );
  danger.hidden = true;
  modal({
    title: "Import bundle",
    body: h(
      "div",
      { class: "stack" },
      field(
        "Bundle",
        h(
          "div",
          { class: "file-pick" },
          pathEl,
          btn("Choose file", {
            icon: "folder",
            onClick: async () => {
              try {
                const picked = await open({ multiple: false, directory: false, filters: [{ name: "Citadel bundle", extensions: ["ctdl"] }] });
                if (typeof picked === "string") {
                  path = picked;
                  pathEl.textContent = picked;
                  pathEl.classList.add("set");
                }
              } catch (e) {
                fail(e);
              }
            },
          }),
        ),
      ),
      field("Bundle password", pw),
      field(
        "Mode",
        segmented<"merge" | "replace">(
          [
            { value: "merge", label: "Merge" },
            { value: "replace", label: "Replace" },
          ],
          mode,
          (v) => {
            mode = v;
            danger.hidden = v !== "replace";
          },
        ),
        "Merge keeps the newer version of each record, including deletions.",
      ),
      danger,
    ),
    actions: [
      { label: "Cancel", kind: "ghost", run: () => undefined },
      {
        label: "Import",
        kind: "primary",
        run: async () => {
          if (!path) {
            toast("Choose a bundle first", "warn");
            return false;
          }
          if (mode === "replace" && !(await confirmBox("Replace vault contents", "Every record in this vault will be replaced by the bundle.", { danger: true, confirm: "Replace" }))) {
            return false;
          }
          const r = await api.vaultImport(path, pw.value, mode === "replace");
          pw.value = "";
          await refreshAll();
          const m = r.report;
          toast(`Imported: ${m.added} added, ${m.updated} updated, ${m.deleted} deleted, ${m.unchanged} unchanged`, "ok", 6000);
          const n = r.neutralized;
          const parts = [
            n.autostart ? `autostart disabled on ${n.autostart} tunnel${n.autostart === 1 ? "" : "s"}` : "",
            n.agent_forward ? `agent forwarding disabled on ${n.agent_forward} host${n.agent_forward === 1 ? "" : "s"}` : "",
            n.host_keys ? `${n.host_keys} trusted host key${n.host_keys === 1 ? "" : "s"} not imported (verified on first connect)` : "",
            n.rejected ? `${n.rejected} malformed record${n.rejected === 1 ? "" : "s"} rejected` : "",
          ].filter(Boolean);
          if (parts.length) toast(`Imported data is quarantined: ${parts.join("; ")}. Re-enable what you trust.`, "warn", 12000);
          return true;
        },
      },
    ],
  });
}

function restoreModal(b: BackupInfo, done: () => void): void {
  const pw = input({ type: "password", placeholder: "passphrase at the time of the backup", autocomplete: "off" });
  let replace = false;
  modal({
    title: "Restore backup",
    body: h(
      "div",
      { class: "stack" },
      h("p", { class: "muted" }, `Snapshot from ${new Date(b.created_ms).toLocaleString()} (${bytes(b.bytes)}). A fresh snapshot of the current vault is taken first, so this can be undone.`),
      field("Passphrase", pw, "Only needed if the master password changed since this backup."),
      field(
        "Mode",
        segmented(
          [
            { value: "merge", label: "Merge" },
            { value: "replace", label: "Replace everything" },
          ],
          "merge",
          (v) => {
            replace = v === "replace";
          },
        ),
        "Merge brings back missing records and keeps newer edits. Replace makes the vault identical to the backup.",
      ),
    ),
    actions: [
      { label: "Cancel", kind: "ghost", run: () => undefined },
      {
        label: "Restore",
        kind: "primary",
        run: async () => {
          const r = await api.backupRestore(b.path, pw.value, replace);
          await refreshAll();
          toast(`Restored: ${r.added} added, ${r.updated} updated`, "ok", 4000);
          done();
        },
      },
    ],
  });
}

function backupsCard(): HTMLElement {
  const list = h("div", { class: "backup-list" }, h("div", { class: "muted" }, "Loading…"));
  const render = async () => {
    const items = await api.backups();
    list.replaceChildren(
      ...(items.length
        ? items.slice(0, 8).map((b) =>
            h(
              "div",
              { class: "backup-row" },
              h("span", { class: "when" }, new Date(b.created_ms).toLocaleString()),
              h("span", { class: "dim" }, ago(b.created_ms)),
              h("span", { class: "dim" }, bytes(b.bytes)),
              btn("restore", { small: true, onClick: () => restoreModal(b, () => void render()) }),
            ),
          )
        : [h("div", { class: "muted" }, "No snapshots yet. One is taken after the first change.")]),
    );
  };
  void render().catch((e) => fail(e));
  return card(
    "Automatic backups",
    row(
      "Snapshots",
      "Encrypted copies of the vault, taken on unlock and every 20 minutes when something changed. The newest 20 are kept next to the database. Updates also keep a raw copy of the previous version's file.",
      btn("back up now", {
        icon: "download",
        onClick: () =>
          void api
            .backupNow()
            .then(() => render())
            .then(() => toast("Snapshot saved", "ok", 2000))
            .catch((e) => fail(e)),
      }),
    ),
    h("div", { class: "set-row" }, list),
  );
}

function vaultSection(): Section {
  const pathEl = h("div", { class: "set-path" }, "…");
  const stateEl = h("span", { class: "badge accent" }, "unlocked");
  const warn = h("div", { class: "alert", hidden: true });
  api
    .vaultStatus()
    .then((s) => {
      pathEl.textContent = s.path;
      stateEl.textContent = s.state;
      if (s.unreadable > 0) {
        warn.hidden = false;
        warn.replaceChildren(icon("warn", 15), h("span", null, `${s.unreadable} record${s.unreadable === 1 ? "" : "s"} were written by a newer Citadel and are hidden. They are kept untouched; update Citadel to see them.`));
      }
    })
    .catch((e) => fail(e));
  const options = [0, 1, 5, 10, 15, 30, 60];
  const current = prefs.get()?.autolock_minutes ?? 10;
  if (!options.includes(current)) options.push(current);
  const autolock = select(
    options.sort((a, b) => a - b).map((m) => ({ value: String(m), label: m === 0 ? "Never" : `${m} minute${m === 1 ? "" : "s"}` })),
    String(current),
  );
  autolock.addEventListener("change", () => {
    savePrefs({ autolock_minutes: Number(autolock.value) })
      .then(() => toast(Number(autolock.value) === 0 ? "Auto-lock disabled" : `Vault locks after ${autolock.value} idle minutes`, "ok", 2400))
      .catch((e) => fail(e));
  });
  const el = h(
    "div",
    null,
    head("Vault", "All hosts, keys, passwords and tunnels live in a local encrypted database. Nothing is readable without the master password."),
    warn,
    card(
      null,
      h(
        "div",
        { class: "set-row" },
        h("div", { class: "desc" }, h("div", { class: "name" }, "Database"), pathEl),
        h("div", { class: "ctl" }, stateEl),
      ),
      h(
        "div",
        { class: "set-row" },
        h(
          "div",
          { class: "desc" },
          h("div", { class: "name" }, "Cryptography"),
          h("div", { class: "spec-line", style: { marginTop: "6px" } }, h("span", { class: "badge" }, "Argon2id 64 MiB · t=3 · p=4"), h("span", { class: "badge" }, "XChaCha20-Poly1305"), h("span", { class: "badge" }, "256-bit data key")),
        ),
      ),
      row("Lock now", "Drops the cipher and zeroes key material. Open sessions and tunnels stay up.", btn("Lock", { icon: "lock", onClick: () => api.vaultLock().catch((e) => fail(e)) })),
      row("Auto-lock", "Locks after this long without vault access or terminal input.", autolock),
    ),
    card(
      "Credentials",
      row("Master password", "Re-wraps the data key with a new password.", btn("Change", { onClick: changePasswordModal })),
    ),
    backupsCard(),
    card(
      "Transfer",
      row("Export bundle", "Password-protected .ctdl file with every record, for another machine.", btn("Export", { icon: "download", onClick: exportModal })),
      row("Import bundle", "Merge or replace from a .ctdl file. A snapshot is taken first.", btn("Import", { icon: "upload", onClick: importModal })),
    ),
  );
  return { el };
}

function terminalSection(): Section {
  const body = h("div");
  const build = (p: Prefs) => {
    const preview = h("pre", { class: "term-preview" });
    const paint = (q: Prefs) => {
      preview.style.fontFamily = q.font_family;
      preview.style.fontSize = `${q.font_size}px`;
      preview.style.lineHeight = String(q.line_height);
      const cursor = h("span", { class: `c ${q.cursor_style}` });
      const th = themeFor(q.theme).theme;
      preview.style.background = th.background ?? "";
      preview.style.color = th.foreground ?? "";
      preview.style.setProperty("--prompt", th.green ?? "");
      preview.style.setProperty("--cur", th.cursor ?? "");
      preview.replaceChildren(
        h("span", { class: "p" }, "ops@edge-01"),
        ":~$ uptime\n 14:02:11 up 41 days,  3:17,  2 users,  load average: 0.12, 0.09, 0.05\n",
        h("span", { class: "p" }, "ops@edge-01"),
        ":~$ ",
        cursor,
      );
    };
    let local = { ...p };
    const change = (patch: Partial<Prefs>) => {
      local = { ...local, ...patch };
      paint(local);
      queuePrefs(patch);
    };
    const num = (value: number, min: number, max: number, step: number, key: keyof Prefs, cls = "narrow") => {
      const el = input({ type: "number", min: String(min), max: String(max), step: String(step), value: String(value), class: `input ${cls}` });
      el.addEventListener("change", () => {
        const v = Number(el.value);
        if (!Number.isFinite(v)) return;
        const clamped = Math.min(max, Math.max(min, v));
        el.value = String(clamped);
        change({ [key]: clamped } as Partial<Prefs>);
      });
      return el;
    };
    const family = input({ value: p.font_family, class: "input wide mono" });
    family.addEventListener("change", () => change({ font_family: family.value.trim() || p.font_family }));
    const telemetry = select(
      [1000, 2000, 5000, 10000].map((ms) => ({ value: String(ms), label: `${ms / 1000} s` })),
      String(p.telemetry_ms),
    );
    telemetry.addEventListener("change", () => change({ telemetry_ms: Number(telemetry.value) }));
    paint(p);
    const tiles: HTMLButtonElement[] = THEMES.map((t) => {
      const c = t.theme;
      const tile = h(
        "button",
        {
          type: "button",
          class: `theme-tile${t.id === themeFor(p.theme).id ? " on" : ""}`,
          style: { background: c.background ?? "", color: c.foreground ?? "" },
          "aria-pressed": String(t.id === themeFor(p.theme).id),
          onClick: () => {
            for (const x of tiles) {
              x.classList.toggle("on", x === tile);
              x.setAttribute("aria-pressed", String(x === tile));
            }
            change({ theme: t.id });
          },
        },
        h(
          "pre",
          { class: "tt-lines" },
          h("span", { style: { color: c.green ?? "" } }, "ops@db"),
          ":~$ ls\n",
          h("span", { style: { color: c.blue ?? "" } }, "etc"),
          " ",
          h("span", { style: { color: c.red ?? "" } }, "logs"),
          " ",
          h("span", { style: { color: c.magenta ?? "" } }, "srv"),
          " ",
          h("span", { style: { color: c.yellow ?? "" } }, "tmp"),
          "\n$ ",
          h("span", { class: "tt-cur", style: { background: c.cursor ?? "" } }),
        ),
        h("div", { class: "tt-name" }, h("b", null, t.name), h("span", null, t.blurb)),
      ) as HTMLButtonElement;
      return tile;
    });
    body.replaceChildren(
      card("Theme", h("div", { class: "theme-grid" }, tiles)),
      card(null, h("div", { class: "set-row" }, h("div", { class: "desc" }, preview))),
      card(
        "Typography",
        row("Font family", "CSS font stack. The first installed font is used.", family),
        row("Font size", "8 to 32 px.", num(p.font_size, 8, 32, 1, "font_size")),
        row("Line height", "1.0 to 2.0.", num(p.line_height, 1, 2, 0.05, "line_height")),
      ),
      card(
        "Cursor",
        row(
          "Style",
          null,
          segmented<string>(
            [
              { value: "block", label: "Block" },
              { value: "bar", label: "Bar" },
              { value: "underline", label: "Underline" },
            ],
            p.cursor_style,
            (v) => change({ cursor_style: v }),
          ),
        ),
        row("Blink", null, toggle(p.cursor_blink, "", (v) => change({ cursor_blink: v }))),
      ),
      card(
        "Behaviour",
        row("Scrollback", "Lines kept per terminal, 500 to 200 000.", num(p.scrollback, 500, 200000, 500, "scrollback", "input")),
        row("Copy on select", "Selecting text copies it to the clipboard.", toggle(p.copy_on_select, "", (v) => change({ copy_on_select: v }))),
        row("Paste confirmation", "Ask before pasting more than this many lines. 0 disables the check.", num(p.paste_confirm_lines, 0, 1000, 1, "paste_confirm_lines")),
        row("Telemetry interval", "How often host metrics refresh in the terminal side panel.", telemetry),
        row(
          "Renderer",
          "Auto uses the GPU when one is present and falls back to the lighter DOM renderer on software OpenGL. Applies to newly opened terminals.",
          segmented(
            [
              { value: "auto", label: "Auto" },
              { value: "webgl", label: "GPU" },
              { value: "dom", label: "DOM" },
            ],
            p.renderer,
            (v) => change({ renderer: v }),
          ),
        ),
      ),
    );
  };
  body.append(h("div", { class: "loading" }, h("span", { class: "spinner" }), "Loading preferences"));
  currentPrefs().then(build).catch((e) => fail(e));
  const el = h("div", null, head("Terminal", "Rendering and input behaviour for every terminal tab. Changes apply to new and open terminals."), body);
  return { el };
}

function agentKind(k: AgentSocket["kind"]): string {
  switch (k) {
    case "native":
      return "OpenSSH agent";
    case "one_password":
      return "1Password";
    case "bitwarden":
      return "Bitwarden";
    case "custom":
      return "Custom";
  }
}

function agentKeysModal(): void {
  const body = h("div", null, h("div", { class: "loading" }, h("span", { class: "spinner" }), "Querying the agent"));
  api
    .agentKeys()
    .then((keys: AgentKey[]) => {
      if (!keys.length) {
        body.replaceChildren(h("div", { class: "note" }, icon("warn", 15), h("span", null, "The agent is reachable but holds no keys.")));
        return;
      }
      body.replaceChildren(
        h(
          "div",
          { class: "pick-list" },
          keys.map((k) =>
            h(
              "div",
              { class: "pick-row" },
              icon("key", 15),
              h("div", { class: "grow" }, h("div", { class: "title" }, k.comment || "(no comment)"), h("div", { class: "meta" }, k.fingerprint)),
              h("span", { class: "badge" }, k.algorithm),
            ),
          ),
        ),
      );
    })
    .catch((e) => body.replaceChildren(h("div", { class: "alert danger" }, icon("warn", 15), h("span", null, errText(e)))));
  modal({ title: "Keys offered by the agent", wide: true, body, actions: [{ label: "Close", kind: "ghost", run: () => undefined }] });
}

function agentSection(): Section {
  const list = h("div", { class: "radio-list" }, h("div", { class: "loading", style: { padding: "16px" } }, h("span", { class: "spinner" }), "Discovering agent sockets"));
  const draw = (sockets: AgentSocket[], chosen: string | null) => {
    const name = "agent-socket";
    const known = new Set(sockets.map((s) => s.path));
    const custom = input({ placeholder: "/path/to/agent.sock or \\\\.\\pipe\\name", class: "input mono", value: chosen && !known.has(chosen) ? chosen : "" });
    const pick = (value: string | null) => {
      savePrefs({ agent: value })
        .then(() => toast(value ? `Using agent at ${value}` : "Using SSH_AUTH_SOCK", "ok", 2400))
        .catch((e) => fail(e));
    };
    const radio = (checked: boolean, value: string | null) => {
      const r = h("input", { type: "radio", name, checked }) as HTMLInputElement;
      r.addEventListener("change", () => {
        if (r.checked) pick(value);
      });
      return r;
    };
    const customRadio = h("input", { type: "radio", name, checked: !!chosen && !known.has(chosen) }) as HTMLInputElement;
    const useCustom = btn("Use", {
      small: true,
      onClick: () => {
        const v = custom.value.trim();
        if (!v) {
          toast("Enter a socket path", "warn");
          return;
        }
        customRadio.checked = true;
        pick(v);
      },
    });
    customRadio.addEventListener("change", () => {
      if (customRadio.checked) custom.focus();
    });
    list.replaceChildren(
      h(
        "label",
        { class: "radio-row" },
        radio(chosen === null, null),
        h("div", { class: "grow" }, h("div", { class: "name" }, "Environment default"), h("div", { class: "path" }, "SSH_AUTH_SOCK, or the OpenSSH pipe on Windows")),
      ),
      ...sockets.map((s) =>
        h(
          "label",
          { class: "radio-row" },
          radio(chosen === s.path, s.path),
          h("div", { class: "grow" }, h("div", { class: "name" }, agentKind(s.kind)), h("div", { class: "path", title: s.path }, s.path)),
          s.available ? h("span", { class: "badge accent" }, "reachable") : h("span", { class: "badge" }, "not found"),
        ),
      ),
      h(
        "label",
        { class: "radio-row" },
        customRadio,
        h("div", { class: "grow" }, h("div", { class: "name" }, "Custom socket"), h("div", { class: "custom" }, custom, useCustom)),
      ),
    );
  };
  Promise.all([api.agents(), currentPrefs()])
    .then(([sockets, p]) => draw(sockets, p.agent))
    .catch((e) => list.replaceChildren(h("div", { class: "alert danger", style: { margin: "12px" } }, icon("warn", 15), h("span", null, errText(e)))));
  const el = h(
    "div",
    null,
    head("SSH agent", "Agent-backed identities and agent-authenticated hosts use this socket. Keys in the agent never enter the vault."),
    card("Socket", list),
    card(null, row("Inspect agent", "Lists public keys the selected agent offers.", btn("List keys", { icon: "key", onClick: agentKeysModal }))),
    h(
      "div",
      { class: "note" },
      icon("shield", 15),
      h("span", null, "With agent forwarding enabled on a host, Citadel answers the remote agent protocol itself and exposes only that host's assigned key. Other keys in your agent are never visible to the server."),
    ),
  );
  return { el };
}

function describeEntry(e: ConfigEntry): string {
  const x = e.host;
  const target = `${x.user ? `${x.user}@` : ""}${x.hostname ?? x.alias}${x.port && x.port !== 22 ? `:${x.port}` : ""}`;
  return target;
}

function importConfigModal(): void {
  const body = h("div", null, h("div", { class: "loading" }, h("span", { class: "spinner" }), "Reading ~/.ssh/config"));
  const picks = new Map<string, HTMLInputElement>();
  const all = h("input", { type: "checkbox" }) as HTMLInputElement;
  const sync = () => {
    const enabled = [...picks.values()];
    const n = enabled.filter((c) => c.checked).length;
    all.checked = n > 0 && n === enabled.length;
    all.indeterminate = n > 0 && n < enabled.length;
  };
  all.addEventListener("change", () => {
    for (const c of picks.values()) c.checked = all.checked;
  });
  api
    .sshConfigHosts()
    .then((entries) => {
      if (!entries.length) {
        body.replaceChildren(h("div", { class: "note" }, icon("file", 15), h("span", null, "No concrete Host entries found. Wildcard patterns and Match blocks are not imported.")));
        return;
      }
      const rows = entries.map((e) => {
        const done = e.imported !== null;
        const cb = h("input", { type: "checkbox", disabled: done, checked: !done }) as HTMLInputElement;
        if (!done) {
          picks.set(e.host.alias, cb);
          cb.addEventListener("change", sync);
        }
        const extras = [
          e.host.proxy_jump.length ? `via ${e.host.proxy_jump.join(" → ")}` : null,
          e.host.identity_files.length ? e.host.identity_files.join(", ") : null,
        ].filter((x): x is string => x !== null);
        return h(
          "label",
          { class: `pick-row${done ? " disabled" : ""}` },
          cb,
          h(
            "div",
            { class: "grow" },
            h("div", { class: "title" }, e.host.alias),
            h("div", { class: "meta" }, describeEntry(e), extras.length ? `  ·  ${extras.join("  ·  ")}` : ""),
          ),
          done ? h("span", { class: "badge accent" }, "imported") : null,
        );
      });
      body.replaceChildren(
        h("div", { class: "pick-head" }, h("span", null, `${entries.length} host${entries.length === 1 ? "" : "s"} found`), h("label", null, all, "Select all")),
        h("div", { class: "pick-list" }, rows),
      );
      sync();
    })
    .catch((e) => body.replaceChildren(h("div", { class: "alert danger" }, icon("warn", 15), h("span", null, errText(e)))));
  modal({
    title: "Import from ~/.ssh/config",
    wide: true,
    body: h(
      "div",
      { class: "stack" },
      body,
      h(
        "div",
        { class: "note" },
        icon("shield", 15),
        h("span", null, "Unencrypted IdentityFile keys are copied into the vault. Passphrase-protected keys are linked to your SSH agent instead. ProxyJump chains become bastion hops."),
      ),
    ),
    actions: [
      { label: "Cancel", kind: "ghost", run: () => undefined },
      {
        label: "Import selected",
        kind: "primary",
        run: async () => {
          const aliases = [...picks.entries()].filter(([, c]) => c.checked).map(([a]) => a);
          if (!aliases.length) {
            toast("Nothing selected", "warn");
            return false;
          }
          const r = await api.sshConfigImport(aliases);
          await refreshAll();
          toast(`Imported ${r.hosts} host${r.hosts === 1 ? "" : "s"} and ${r.keys} key${r.keys === 1 ? "" : "s"}`, "ok", 5000);
          if (r.skipped.length) toast(`Skipped: ${r.skipped.join("; ")}`, "warn", 9000);
          return true;
        },
      },
    ],
  });
}

function exportConfigModal(): void {
  const list = hosts.get();
  if (!list.length) {
    toast("The vault has no hosts to export", "warn");
    return;
  }
  const boxes = list.map((x) => ({ x, cb: h("input", { type: "checkbox", checked: true }) as HTMLInputElement }));
  const all = h("input", { type: "checkbox", checked: true }) as HTMLInputElement;
  const sync = () => {
    const n = boxes.filter((b) => b.cb.checked).length;
    all.checked = n === boxes.length;
    all.indeterminate = n > 0 && n < boxes.length;
  };
  all.addEventListener("change", () => {
    for (const b of boxes) b.cb.checked = all.checked;
  });
  const rows = boxes.map(({ x, cb }) => {
    cb.addEventListener("change", sync);
    return h(
      "label",
      { class: "pick-row" },
      cb,
      h(
        "div",
        { class: "grow" },
        h("div", { class: "title" }, x.label),
        h("div", { class: "meta" }, `${x.user}@${x.hostname}${x.port !== 22 ? `:${x.port}` : ""}`, x.ssh_alias ? `  ·  Host ${x.ssh_alias}` : ""),
      ),
      x.jump.length ? h("span", { class: "badge" }, `${x.jump.length} hop${x.jump.length === 1 ? "" : "s"}`) : null,
    );
  });
  modal({
    title: "Export to ~/.ssh/config",
    wide: true,
    body: h(
      "div",
      { class: "stack" },
      h("div", null, h("div", { class: "pick-head" }, h("span", null, "Hosts"), h("label", null, all, "Select all")), h("div", { class: "pick-list" }, rows)),
      h(
        "div",
        { class: "note" },
        icon("shield", 15),
        h(
          "span",
          null,
          "Writes Host blocks with HostName, User, Port, ProxyJump and ForwardAgent. Existing entries, comments and ordering are preserved, a backup is written to config.citadel.bak, and private keys are never written to disk.",
        ),
      ),
    ),
    actions: [
      { label: "Cancel", kind: "ghost", run: () => undefined },
      {
        label: "Write config",
        kind: "primary",
        run: async () => {
          const ids = boxes.filter((b) => b.cb.checked).map((b) => b.x.id);
          if (!ids.length) {
            toast("Nothing selected", "warn");
            return false;
          }
          const path = await api.sshConfigExport(ids);
          await refreshAll();
          toast(`Wrote ${ids.length} host${ids.length === 1 ? "" : "s"} to ${path}`, "ok", 5000);
          return true;
        },
      },
    ],
  });
}

function opensshSection(): Section {
  const el = h(
    "div",
    null,
    head("OpenSSH config", "Two-way, non-destructive exchange with ~/.ssh/config so command-line ssh and Citadel agree on aliases and bastions."),
    card(
      null,
      row("Import", "Create vault hosts from concrete Host entries, including ProxyJump chains and identity files.", btn("Import…", { icon: "import", onClick: importConfigModal })),
      row("Export", "Write or update Host blocks for selected vault hosts.", btn("Export…", { icon: "upload", onClick: exportConfigModal })),
    ),
  );
  return { el };
}

function kindLabel(t: SyncTarget): string {
  return t.backend.kind === "s3" ? "S3" : t.backend.kind === "web_dav" ? "WebDAV" : "Git";
}

function detail(t: SyncTarget): string {
  const b = t.backend;
  if (b.kind === "s3") return `${b.endpoint} · ${b.bucket}/${b.object}`;
  if (b.kind === "web_dav") return `${b.user}@${b.url}`;
  return `${b.remote} · ${b.branch}:${b.path}`;
}

function reportText(r: SyncReport): string {
  const parts: string[] = [];
  if (r.pulled) {
    const m = r.pulled.report;
    parts.push(`pulled ${m.added} added, ${m.updated} updated, ${m.deleted} deleted`);
  } else {
    parts.push("remote was empty");
  }
  parts.push(r.pushed ? "pushed" : "nothing pushed");
  if (r.rounds > 1) parts.push(`${r.rounds} rounds`);
  return parts.join(" · ");
}

function syncModal(existing: SyncTarget | null, done: () => void): void {
  type Kind = "s3" | "web_dav" | "git";
  let kind: Kind = existing?.backend.kind ?? "s3";
  const b = existing?.backend;
  const label = input({ value: existing?.label ?? "", placeholder: "Personal bucket" });
  const s3 = {
    endpoint: input({ value: b?.kind === "s3" ? b.endpoint : "", placeholder: "https://s3.eu-central-1.amazonaws.com", class: "input mono" }),
    region: input({ value: b?.kind === "s3" ? b.region : "", placeholder: "eu-central-1" }),
    bucket: input({ value: b?.kind === "s3" ? b.bucket : "", placeholder: "my-citadel" }),
    object: input({ value: b?.kind === "s3" ? b.object : "citadel/vault.ctdl", class: "input mono" }),
    access: input({ value: b?.kind === "s3" ? b.access_key : "", class: "input mono" }),
    secret: input({ type: "password", placeholder: existing && b?.kind === "s3" ? "Unchanged" : "" }),
    pathStyle: b?.kind === "s3" ? b.path_style : false,
  };
  const dav = {
    url: input({ value: b?.kind === "web_dav" ? b.url : "", placeholder: "https://cloud.example.com/remote.php/dav/files/me/citadel.ctdl", class: "input mono" }),
    user: input({ value: b?.kind === "web_dav" ? b.user : "" }),
    password: input({ type: "password", placeholder: existing && b?.kind === "web_dav" ? "Unchanged" : "" }),
  };
  const git = {
    remote: input({ value: b?.kind === "git" ? b.remote : "", placeholder: "git@github.com:me/citadel-vault.git", class: "input mono" }),
    branch: input({ value: b?.kind === "git" ? b.branch : "main" }),
    path: input({ value: b?.kind === "git" ? b.path : "citadel.ctdl", class: "input mono" }),
  };
  const panes: Record<Kind, HTMLElement> = {
    s3: h(
      "div",
      { class: "stack" },
      field("Endpoint", s3.endpoint, "AWS, MinIO, Cloudflare R2, Backblaze B2 or any S3-compatible service."),
      h("div", { class: "grid-2" }, field("Region", s3.region), field("Bucket", s3.bucket)),
      field("Object key", s3.object),
      h("div", { class: "grid-2" }, field("Access key", s3.access), field("Secret key", s3.secret)),
      toggle(s3.pathStyle, "Path-style addressing (MinIO and most self-hosted services)", (v) => {
        s3.pathStyle = v;
      }),
    ),
    web_dav: h(
      "div",
      { class: "stack" },
      field("File URL", dav.url, "Full URL of the blob file. Parent collections are created if missing."),
      h("div", { class: "grid-2" }, field("User", dav.user), field("Password", dav.password)),
    ),
    git: h(
      "div",
      { class: "stack" },
      field("Remote", git.remote, "Uses the system git binary and your existing SSH or credential helper setup."),
      h("div", { class: "grid-2" }, field("Branch", git.branch), field("File path", git.path)),
    ),
  };
  const holder = h("div");
  const show = (k: Kind) => {
    kind = k;
    holder.replaceChildren(panes[k]);
  };
  show(kind);
  const nul = (v: string) => (v ? v : null);
  modal({
    title: existing ? `Edit ${existing.label}` : "Add sync target",
    wide: true,
    body: h(
      "div",
      { class: "stack" },
      field("Label", label),
      segmented<Kind>(
        [
          { value: "s3", label: "S3" },
          { value: "web_dav", label: "WebDAV" },
          { value: "git", label: "Git" },
        ],
        kind,
        show,
      ),
      holder,
      h(
        "div",
        { class: "note" },
        icon("shield", 15),
        h("span", null, "Only the encrypted vault blob leaves this machine. The storage provider sees ciphertext and its size, never hostnames, keys or passwords. Credentials for this target are stored in the vault and sync with it."),
      ),
    ),
    actions: [
      { label: "Cancel", kind: "ghost", run: () => undefined },
      {
        label: existing ? "Save" : "Add target",
        kind: "primary",
        run: async () => {
          let backend: SyncBackendInput;
          if (kind === "s3") {
            backend = {
              kind: "s3",
              endpoint: s3.endpoint.value.trim(),
              region: s3.region.value.trim(),
              bucket: s3.bucket.value.trim(),
              object: s3.object.value.trim(),
              access_key: s3.access.value.trim(),
              secret_key: nul(s3.secret.value),
              path_style: s3.pathStyle,
            };
          } else if (kind === "web_dav") {
            backend = { kind: "web_dav", url: dav.url.value.trim(), user: dav.user.value.trim(), password: nul(dav.password.value) };
          } else {
            backend = { kind: "git", remote: git.remote.value.trim(), branch: git.branch.value.trim(), path: git.path.value.trim() };
          }
          await api.syncTargetSave({ id: existing?.id ?? null, label: label.value.trim(), backend });
          s3.secret.value = dav.password.value = "";
          toast(existing ? "Sync target updated" : "Sync target added", "ok");
          done();
          return true;
        },
      },
    ],
  });
}

async function runSync(t: SyncTarget, button: HTMLButtonElement): Promise<void> {
  const original = [...button.childNodes];
  button.disabled = true;
  button.replaceChildren(h("span", { class: "spinner" }), h("span", null, "Syncing"));
  try {
    let report: SyncReport;
    try {
      report = await api.syncRun(t.id, null);
    } catch (e) {
      if (errCode(e) !== "needs_password") throw e;
      const pw = await askText("Remote vault password", "Password of the vault stored on this target", {
        secret: true,
        hint: "The remote blob was created by another vault. Joining it re-encrypts this vault under the remote key; afterwards this vault unlocks with the remote password.",
        confirm: "Join",
      });
      if (!pw) return;
      report = await api.syncRun(t.id, pw);
      toast("Joined the remote vault. From now on, unlock with the remote vault's password.", "warn", 9000);
    }
    await refreshAll();
    if (report.pulled?.header_adopted && !report.pulled.rekeyed) {
      toast("The master password was changed on another device and has been adopted here.", "warn", 9000);
    }
    toast(`${t.label}: ${reportText(report)}`, "ok", 6000);
  } catch (e) {
    fail(e, `Sync with ${t.label} failed`);
  } finally {
    button.disabled = false;
    button.replaceChildren(...original);
  }
}

function syncSection(): Section {
  const listEl = h("div", null, h("div", { class: "loading" }, h("span", { class: "spinner" }), "Loading sync targets"));
  const reload = async () => {
    let targets: SyncTarget[];
    try {
      targets = await api.syncTargets();
    } catch (e) {
      listEl.replaceChildren(h("div", { class: "alert danger" }, icon("warn", 15), h("span", null, errText(e))));
      return;
    }
    if (!targets.length) {
      listEl.replaceChildren(
        h(
          "div",
          { class: "empty" },
          h("div", { class: "empty-mark" }, icon("cloud", 26)),
          h("h3", null, "No sync targets"),
          h("p", null, "Bring your own storage. Citadel runs no servers and keeps no accounts."),
          btn("Add target", { icon: "plus", primary: true, onClick: () => syncModal(null, () => void reload()) }),
        ),
      );
      return;
    }
    listEl.replaceChildren(
      h(
        "div",
        { class: "list" },
        targets.map((t) => {
          const run = btn("Sync now", { icon: "refresh", small: true, primary: true });
          run.addEventListener("click", () => void runSync(t, run));
          return h(
            "div",
            { class: "list-row" },
            h("div", { class: "key-icon", style: { display: "grid", placeItems: "center", width: "34px", height: "34px", borderRadius: "9px", background: "var(--bg-4)" } }, icon("cloud", 17)),
            h("div", { class: "grow" }, h("div", { class: "key-head" }, h("span", { class: "title" }, t.label), h("span", { class: "badge info" }, kindLabel(t))), h("div", { class: "sync-detail", title: detail(t) }, detail(t))),
            h(
              "div",
              { class: "actions", style: { opacity: "1" } },
              run,
              btn("", { icon: "edit", ghost: true, small: true, title: "Edit", onClick: () => syncModal(t, () => void reload()) }),
              btn("", {
                icon: "trash",
                ghost: true,
                small: true,
                title: "Delete",
                onClick: async () => {
                  if (!(await confirmBox("Delete sync target", `"${t.label}" and its stored credentials will be removed. The remote blob is left in place.`, { danger: true, confirm: "Delete" }))) return;
                  try {
                    await api.syncTargetDelete(t.id);
                    await reload();
                  } catch (e) {
                    fail(e);
                  }
                },
              }),
            ),
          );
        }),
      ),
    );
  };
  void reload();
  const el = h(
    "div",
    null,
    head("Sync", "Replicate the encrypted vault through storage you control: an S3-compatible bucket, a WebDAV share or a private Git repository."),
    h("div", { class: "toolbar", style: { marginBottom: "12px" } }, btn("Add target", { icon: "plus", onClick: () => syncModal(null, () => void reload()) })),
    listEl,
    h(
      "div",
      { class: "note", style: { marginTop: "14px" } },
      icon("shield", 15),
      h(
        "span",
        null,
        "Each sync fetches the remote blob, merges it record by record (newest write wins, deletions included) and pushes the result with an ETag or fast-forward precondition. Concurrent writers are detected and retried, never overwritten.",
      ),
    ),
  );
  return { el };
}

function aboutSection(): Section {
  const ver = h("span", { class: "badge accent" }, "…");
  getVersion()
    .then((v) => {
      ver.textContent = `v${v}`;
    })
    .catch(() => {
      ver.textContent = "unknown";
    });
  const specs: [string, string][] = [
    ["Vault cipher", "XChaCha20-Poly1305 with a random 256-bit data key and a fresh 192-bit nonce per record"],
    ["Key derivation", "Argon2id, 64 MiB, 3 passes, 4 lanes, 32-byte salt; the 64 MiB arena is zeroed after use"],
    ["Record binding", "Each record's AAD binds its table and identifier, so ciphertext cannot be moved or swapped"],
    ["Bundle format", ".ctdl: authenticated header, wrapped key, zstd-compressed MessagePack payload"],
    ["Auto-lock", "Idle watchdog drops the cipher and zeroes key buffers; secrets use zeroize-on-drop types"],
    ["Agent isolation", "Forwarded agent requests are answered by Citadel and expose only the host's assigned key"],
    ["Host keys", "Unknown keys require confirmation; changed keys are flagged as a possible interception"],
    ["Transport", "russh with the ring backend; no OpenSSL or libssh2"],
  ];
  const el = h(
    "div",
    null,
    head("About", "Local-first SSH, tunnel and infrastructure workstation."),
    card(
      null,
      row("Citadel", "Licensed under MIT OR Apache-2.0.", ver),
      row("Source", REPO, btn("Copy URL", { icon: "copy", small: true, onClick: () => void copyText(REPO, "Repository URL copied") })),
    ),
    card("Security model", h("table", { class: "spec-table" }, h("tbody", null, specs.map(([k, v]) => h("tr", null, h("td", null, k), h("td", null, v)))))),
  );
  return { el };
}

const BUILDERS: Record<SectionId, () => Section> = {
  vault: vaultSection,
  terminal: terminalSection,
  agent: agentSection,
  openssh: opensshSection,
  sync: syncSection,
  about: aboutSection,
};

let lastSection: SectionId = "vault";

export function settingsPage(): Page {
  return {
    id: "settings",
    title: "Settings",
    icon: "settings",
    render(root: HTMLElement) {
      const nav = h("nav", { class: "set-nav", "aria-label": "Settings sections" }, h("h1", null, "Settings"));
      const content = h("div", { class: "set-body" });
      let active: Section | null = null;
      const buttons = new Map<SectionId, HTMLButtonElement>();
      const show = (id: SectionId) => {
        active?.dispose?.();
        lastSection = id;
        active = BUILDERS[id]();
        clear(content);
        content.append(active.el);
        for (const [k, b] of buttons) {
          b.classList.toggle("on", k === id);
          b.setAttribute("aria-current", k === id ? "page" : "false");
        }
        root.scrollTop = 0;
      };
      for (const s of SECTIONS) {
        const b = h("button", { type: "button", onClick: () => show(s.id) }, icon(s.icon, 15), s.label) as HTMLButtonElement;
        buttons.set(s.id, b);
        nav.append(b);
      }
      nav.addEventListener("keydown", (e) => {
        if (e.key !== "ArrowDown" && e.key !== "ArrowUp") return;
        e.preventDefault();
        const i = SECTIONS.findIndex((s) => s.id === lastSection);
        const next = SECTIONS[(i + (e.key === "ArrowDown" ? 1 : SECTIONS.length - 1)) % SECTIONS.length];
        if (next) {
          show(next.id);
          buttons.get(next.id)?.focus();
        }
      });
      root.append(h("div", { class: "settings" }, nav, content));
      show(lastSection);
      return () => {
        active?.dispose?.();
        active = null;
      };
    },
  };
}
