import "./pages.css";
import { api, errCode, errText, type AgentKey, type Identity, type KeyKind, type KnownHost } from "../api";
import { clear, h, icon } from "../dom";
import type { Page } from "../shell";
import { hosts, identities, refreshIdentities } from "../store";
import { btn, confirmBox, copyText, empty, fail, field, input, modal, segmented, textarea, toast } from "../ui";

function usage(id: string): number {
  return hosts.get().filter((x) => x.auth.kind === "identity" && x.auth.identity === id).length;
}

function algoLabel(alg: string): string {
  if (alg.includes("ed25519")) return "Ed25519";
  if (alg.includes("nistp256")) return "ECDSA P-256";
  if (alg.includes("nistp384")) return "ECDSA P-384";
  if (alg.includes("nistp521")) return "ECDSA P-521";
  if (alg.includes("rsa")) return "RSA";
  return alg;
}

function loading(text: string): HTMLElement {
  return h("div", { class: "loading" }, h("span", { class: "spinner" }), text);
}

export function showPublicKey(k: Identity): void {
  modal({
    title: `Public key · ${k.label}`,
    wide: true,
    body: h(
      "div",
      { class: "stack" },
      h("code", { class: "pubkey" }, k.public_key),
      h("div", { class: "row" }, h("span", { class: "badge" }, algoLabel(k.algorithm)), h("span", { class: "key-fp" }, k.fingerprint)),
      h("div", { class: "note" }, icon("shield", 15), h("span", null, "Append this line to ~/.ssh/authorized_keys on the server. The private half never leaves the vault.")),
    ),
    actions: [
      { label: "Close", kind: "ghost", run: () => undefined },
      { label: "Copy public key", kind: "primary", keep: true, run: () => copyText(k.public_key, "Public key copied") },
    ],
  });
}

function generateModal(): void {
  const label = input({ placeholder: "deploy-laptop" });
  let kind: KeyKind = "ed25519";
  modal({
    title: "Generate key",
    body: h(
      "div",
      { class: "stack" },
      field("Label", label, "Used in the comment of the public key."),
      field(
        "Algorithm",
        segmented<KeyKind>(
          [
            { value: "ed25519", label: "Ed25519" },
            { value: "ecdsa_p256", label: "ECDSA P-256" },
          ],
          kind,
          (v) => {
            kind = v;
          },
        ),
        "Ed25519 is recommended. ECDSA P-256 is accepted by hardware-constrained or FIPS-restricted servers.",
      ),
    ),
    actions: [
      { label: "Cancel", kind: "ghost", run: () => undefined },
      {
        label: "Generate",
        kind: "primary",
        run: async () => {
          const name = label.value.trim();
          if (!name) {
            toast("A label is required", "warn");
            label.focus();
            return false;
          }
          const k = await api.identityGenerate(name, kind);
          await refreshIdentities();
          showPublicKey(k);
          return true;
        },
      },
    ],
  });
}

function importModal(): void {
  let mode: "paste" | "file" = "paste";
  let path: string | null = null;
  const label = input({ placeholder: "production" });
  const pem = textarea({ placeholder: "-----BEGIN OPENSSH PRIVATE KEY-----", rows: 9 });
  const pass = input({ type: "password", placeholder: "Leave empty for unencrypted keys" });
  const pathEl = h("span", { class: "path" }, "No file selected");
  const choose = btn("Choose file", {
    icon: "folder",
    onClick: async () => {
      try {
        const [picked] = await api.pickOpen("Select a private key", []);
        if (picked) {
          path = picked.token;
          pathEl.textContent = picked.path;
          pathEl.classList.add("set");
          if (!label.value.trim()) label.value = picked.name;
        }
      } catch (e) {
        fail(e);
      }
    },
  });
  const pasteBox = field("Private key", pem, "OpenSSH, PuTTY .ppk, PKCS#8 and PEM keys (Ed25519, ECDSA, RSA). Indentation and line endings mangled by copy-paste are repaired.");
  const fileBox = field("Key file", h("div", { class: "file-pick" }, pathEl, choose), "The file is read once and stored encrypted in the vault.");
  fileBox.hidden = true;
  modal({
    title: "Import key",
    wide: true,
    body: h(
      "div",
      { class: "stack" },
      segmented<"paste" | "file">(
        [
          { value: "paste", label: "Paste" },
          { value: "file", label: "File" },
        ],
        mode,
        (v) => {
          mode = v;
          pasteBox.hidden = v !== "paste";
          fileBox.hidden = v !== "file";
        },
      ),
      field("Label", label),
      pasteBox,
      fileBox,
      field("Passphrase", pass, "Stored encrypted alongside the key so connections do not prompt."),
    ),
    actions: [
      { label: "Cancel", kind: "ghost", run: () => undefined },
      {
        label: "Import",
        kind: "primary",
        run: async () => {
          const name = label.value.trim();
          if (!name) {
            toast("A label is required", "warn");
            return false;
          }
          const passphrase = pass.value ? pass.value : null;
          if (mode === "paste" && !pem.value.trim()) {
            toast("Paste a private key first", "warn");
            return false;
          }
          const file = path;
          if (mode === "file" && !file) {
            toast("Choose a key file first", "warn");
            return false;
          }
          try {
            if (mode === "paste" || !file) await api.identityImport(name, pem.value, passphrase);
            else await api.identityImportFile(name, file, passphrase);
          } catch (e) {
            if (errCode(e) !== "key_encrypted") throw e;
            toast(passphrase ? "That passphrase does not unlock this key" : "This key is protected by a passphrase. Enter it below.", "warn", 4000);
            pass.focus();
            pass.select();
            return false;
          }
          pem.value = "";
          pass.value = "";
          await refreshIdentities();
          toast(`Imported ${name}`, "ok");
          return true;
        },
      },
    ],
  });
}

function agentModal(): void {
  let chosen: AgentKey | null = null;
  const label = input({ placeholder: "yubikey" });
  const listEl = h("div", null, loading("Querying the SSH agent"));
  const known = new Set(identities.get().map((i) => i.fingerprint));
  const draw = (keys: AgentKey[]) => {
    if (!keys.length) {
      listEl.replaceChildren(h("div", { class: "note" }, icon("warn", 15), h("span", null, "The agent holds no keys. Add one with ssh-add or unlock it in your password manager.")));
      return;
    }
    const rows = keys.map((k) => {
      const taken = known.has(k.fingerprint);
      const radio = h("input", { type: "radio", name: "agent-key", disabled: taken }) as HTMLInputElement;
      const row = h(
        "label",
        { class: `pick-row${taken ? " disabled" : ""}` },
        radio,
        h(
          "div",
          { class: "grow" },
          h("div", { class: "title" }, k.comment || "(no comment)"),
          h("div", { class: "meta" }, k.fingerprint),
        ),
        h("span", { class: "badge" }, algoLabel(k.algorithm)),
        taken ? h("span", { class: "badge accent" }, "in vault") : null,
      );
      radio.addEventListener("change", () => {
        chosen = k;
        for (const r of listEl.querySelectorAll(".pick-row")) r.classList.remove("on");
        row.classList.add("on");
        if (!label.value.trim()) label.value = k.comment || algoLabel(k.algorithm);
      });
      return row;
    });
    listEl.replaceChildren(h("div", { class: "pick-list" }, rows));
  };
  api
    .agentKeys()
    .then(draw)
    .catch((e) => listEl.replaceChildren(h("div", { class: "alert danger" }, icon("warn", 15), h("span", null, `Agent unavailable: ${errText(e)}`))));
  modal({
    title: "Use a key from the SSH agent",
    wide: true,
    body: h(
      "div",
      { class: "stack" },
      h("div", { class: "note" }, icon("shield", 15), h("span", null, "The private key stays inside the agent (OpenSSH, 1Password, Bitwarden). Citadel stores only the public key and asks the agent to sign, offering this key alone to hosts that use it.")),
      listEl,
      field("Label", label),
    ),
    actions: [
      { label: "Cancel", kind: "ghost", run: () => undefined },
      {
        label: "Add key",
        kind: "primary",
        run: async () => {
          if (!chosen) {
            toast("Select a key", "warn");
            return false;
          }
          const name = label.value.trim() || chosen.comment || "agent key";
          await api.identityAgent(name, chosen.public_key);
          await refreshIdentities();
          toast(`Added ${name}`, "ok");
          return true;
        },
      },
    ],
  });
}

function identityRow(k: Identity): HTMLElement {
  const n = usage(k.id);
  return h(
    "div",
    { class: "list-row" },
    h("div", { class: `key-icon ${k.source}` }, icon("key", 17)),
    h(
      "div",
      { class: "grow" },
      h(
        "div",
        { class: "key-head" },
        h("span", { class: "title" }, k.label),
        h("span", { class: "badge" }, algoLabel(k.algorithm)),
        k.source === "agent" ? h("span", { class: "badge info" }, "agent") : h("span", { class: "badge accent" }, "vault"),
        k.encrypted ? h("span", { class: "badge" }, "passphrase") : null,
      ),
      h("div", { class: "key-fp", title: k.fingerprint }, k.fingerprint),
    ),
    h("span", { class: "key-usage" }, n === 0 ? "unused" : `${n} host${n === 1 ? "" : "s"}`),
    h(
      "div",
      { class: "actions" },
      btn("", { icon: "copy", ghost: true, small: true, title: "Copy public key", onClick: () => void copyText(k.public_key, "Public key copied") }),
      btn("", { icon: "file", ghost: true, small: true, title: "Show public key", onClick: () => showPublicKey(k) }),
      btn("", {
        icon: "trash",
        ghost: true,
        small: true,
        title: "Delete key",
        onClick: async () => {
          const ok = await confirmBox(
            "Delete key",
            k.source === "vault"
              ? `"${k.label}" will be erased from the vault. Servers that only trust this key will become unreachable from Citadel.`
              : `Citadel will forget "${k.label}". The key itself stays in the agent.`,
            { danger: true, confirm: "Delete" },
          );
          if (!ok) return;
          try {
            await api.identityDelete(k.id);
            await refreshIdentities();
            toast(`Deleted ${k.label}`, "ok");
          } catch (e) {
            fail(e, "Cannot delete");
          }
        },
      }),
    ),
  );
}

function knownHostsSection(): { el: HTMLElement; reload: () => Promise<void> } {
  const body = h("div", null, loading("Loading trusted host keys"));
  const reload = async () => {
    let list: KnownHost[];
    try {
      list = await api.knownHosts();
    } catch (e) {
      body.replaceChildren(h("div", { class: "alert danger" }, icon("warn", 15), h("span", null, errText(e))));
      return;
    }
    if (!list.length) {
      body.replaceChildren(
        h("div", { class: "note" }, icon("shield", 15), h("span", null, "No host keys trusted yet. Citadel asks before trusting a new server and stores its key here, encrypted.")),
      );
      return;
    }
    const rows = list.map((k) =>
      h(
        "tr",
        null,
        h("td", { class: "mono" }, k.port === 22 ? k.host : `${k.host}:${k.port}`),
        h("td", null, h("span", { class: "badge" }, algoLabel(k.algorithm))),
        h("td", { class: "mono fp", title: k.fingerprint }, k.fingerprint),
        h(
          "td",
          { class: "act" },
          btn("", {
            icon: "trash",
            ghost: true,
            small: true,
            title: "Forget host key",
            onClick: async () => {
              const ok = await confirmBox("Forget host key", `Citadel will ask again on the next connection to ${k.host}:${k.port}.`, {
                danger: true,
                confirm: "Forget",
              });
              if (!ok) return;
              try {
                await api.knownHostDelete(k.id);
                await reload();
              } catch (e) {
                fail(e);
              }
            },
          }),
        ),
      ),
    );
    body.replaceChildren(
      h(
        "div",
        { class: "list" },
        h(
          "table",
          { class: "kh-table" },
          h("thead", null, h("tr", null, h("th", null, "Host"), h("th", null, "Algorithm"), h("th", null, "Fingerprint"), h("th", null, ""))),
          h("tbody", null, rows),
        ),
      ),
    );
  };
  const el = h(
    "div",
    { class: "section" },
    h("div", { class: "section-title" }, icon("shield", 13), "Trusted host keys", h("span", { class: "spacer", style: { flex: "1" } }), btn("", { icon: "refresh", ghost: true, small: true, title: "Reload", onClick: () => void reload() })),
    body,
  );
  return { el, reload };
}

export function keysPage(): Page {
  return {
    id: "keys",
    title: "Keys",
    icon: "key",
    render(root: HTMLElement) {
      const listEl = h("div");
      const draw = () => {
        clear(listEl);
        const keys = identities.get();
        if (!keys.length) {
          listEl.append(
            empty(
              "No identities yet",
              "Generate an Ed25519 key, import an existing private key, or reference a key held by your SSH agent. Each host can be pinned to exactly one identity.",
              h("div", { class: "toolbar" }, btn("Generate key", { icon: "plus", primary: true, onClick: generateModal }), btn("Import key", { icon: "import", onClick: importModal })),
            ),
          );
          return;
        }
        listEl.append(h("div", { class: "list keys-list" }, keys.map(identityRow)));
      };
      const kh = knownHostsSection();
      const inner = h(
        "div",
        { class: "page-inner" },
        h(
          "div",
          { class: "page-head" },
          h("div", null, h("h1", null, "Keys"), h("div", { class: "sub" }, "Identities used for public-key authentication, and the host keys you have trusted.")),
          h("div", { class: "spacer" }),
          h(
            "div",
            { class: "toolbar" },
            btn("Use agent key", { icon: "link", onClick: agentModal }),
            btn("Import", { icon: "import", onClick: importModal }),
            btn("Generate key", { icon: "plus", primary: true, onClick: generateModal }),
          ),
        ),
        h("div", { class: "section-title" }, icon("key", 13), "Identities"),
        listEl,
        kh.el,
      );
      root.append(inner);
      const unsubs = [identities.subscribe(draw), hosts.subscribe(draw, false)];
      void refreshIdentities().catch((e) => fail(e));
      void kh.reload();
      return () => {
        for (const u of unsubs) u();
      };
    },
  };
}
