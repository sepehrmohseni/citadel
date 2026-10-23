import { api, type AgentKey, type AuthInput, type Host, type HostInput, type Id, type LinkState } from "../api";
import { coalesce, fuzzy, h, icon, kbd, keyOf } from "../dom";
import { go, openFiles, openTerminal, type Page } from "../shell";
import { hostById, hosts, identities, links, refreshHosts, refreshIdentities } from "../store";
import { btn, confirmBox, empty, fail, field, input, modal, segmented, select, toast, toggle } from "../ui";
import { parseSsh } from "../target";
import { openTunnelForm } from "./tunnels";

const RTT_POINTS = 24;
const rttHistory = new Map<Id, number[]>();

links.subscribe((m) => {
  for (const [id, s] of m) {
    if (s.state !== "up" || typeof s.rtt_ms !== "number") continue;
    const hist = rttHistory.get(id) ?? [];
    if (hist[hist.length - 1] !== s.rtt_ms || hist.length === 0) hist.push(s.rtt_ms);
    if (hist.length > RTT_POINTS) hist.shift();
    rttHistory.set(id, hist);
  }
}, false);

function authLabel(hst: Host): HTMLElement {
  switch (hst.auth.kind) {
    case "identity": {
      const k = identities.get().find((i) => i.id === (hst.auth as { identity: Id }).identity);
      return h("span", { class: "badge accent", title: k?.fingerprint ?? "" }, `key:${k ? k.label : "?"}`);
    }
    case "agent":
      return h("span", { class: "badge info" }, hst.auth.fingerprint ? "agent:pinned" : "agent:any");
    case "password":
      return h("span", { class: "badge warn" }, "password");
    default:
      return h("span", { class: "badge" }, "interactive");
  }
}

function glyph(s: LinkState | undefined): HTMLElement {
  if (!s) return h("span", { class: "state-glyph", title: "not connected" }, "○");
  if (s.state === "up") return h("span", { class: "state-glyph up", title: "connected" }, "●");
  if (s.state === "backoff") return h("span", { class: "state-glyph healing", title: `reconnecting: ${s.error}` }, "◐");
  if (s.state === "connecting") return h("span", { class: "state-glyph healing", title: "connecting" }, "◐");
  return h("span", { class: "state-glyph" }, "○");
}

function sparkline(values: number[]): SVGSVGElement {
  const svg = h("svg", { viewBox: "0 0 44 16", preserveAspectRatio: "none" }) as SVGSVGElement;
  if (values.length < 2) return svg;
  const max = Math.max(...values, 1);
  const step = 44 / (RTT_POINTS - 1);
  const pts = values.map((v, i) => `${(44 - (values.length - 1 - i) * step).toFixed(1)},${(15 - (v / max) * 13).toFixed(1)}`);
  svg.append(h("polyline", { points: pts.join(" "), fill: "none", stroke: "currentColor", "stroke-width": "1.4", "vector-effect": "non-scaling-stroke" }));
  return svg;
}

export function hostForm(existing?: Host, opts: { connect?: boolean } = {}): void {
  const isEdit = Boolean(existing);
  const label = input({ value: existing?.label ?? "", placeholder: "prod-db-1" });
  const hostname = input({ value: existing?.hostname ?? "", placeholder: "10.0.4.12 or db.internal", class: "input mono" });
  const port = input({ type: "number", min: 1, max: 65535, value: String(existing?.port ?? 22), class: "input mono" });
  const user = input({ value: existing?.user ?? "", placeholder: "root", class: "input mono" });
  type Kind = AuthInput["kind"];
  let kind: Kind = existing?.auth.kind ?? (identities.get().length ? "identity" : "agent");
  const authBody = h("div", { class: "stack" });
  const identitySel = select(
    identities.get().map((i) => ({ value: i.id, label: `${i.label} · ${i.algorithm}` })),
    existing?.auth.kind === "identity" ? existing.auth.identity : identities.get()[0]?.id ?? "",
  );
  const agentSel = select([{ value: "", label: "Agent keys, then ~/.ssh/id_ed25519, id_ecdsa, id_rsa" }], existing?.auth.kind === "agent" ? existing.auth.fingerprint ?? "" : "");
  const password = input({
    type: "password",
    placeholder: existing?.auth.kind === "password" ? "Stored in vault · leave blank to keep" : "Password",
  });
  let agentLoaded = false;
  const loadAgent = async () => {
    if (agentLoaded) return;
    agentLoaded = true;
    try {
      const keys: AgentKey[] = await api.agentKeys();
      for (const k of keys) agentSel.append(h("option", { value: k.fingerprint }, `${k.comment || k.algorithm} · ${k.fingerprint.slice(0, 22)}…`));
      if (existing?.auth.kind === "agent" && existing.auth.fingerprint) agentSel.value = existing.auth.fingerprint;
    } catch (e) {
      agentSel.append(h("option", { disabled: true }, `Agent unavailable: ${String((e as { message?: string }).message ?? e)}`));
    }
  };
  const genKey = btn("Generate Ed25519 key", {
    small: true,
    icon: "plus",
    onClick: async () => {
      try {
        const k = await api.identityGenerate(label.value.trim() || "citadel", "ed25519");
        await refreshIdentities();
        identitySel.append(h("option", { value: k.id }, `${k.label} · ${k.algorithm}`));
        identitySel.value = k.id;
        toast("Key generated. Copy the public key from the Keys page to the server.", "ok");
      } catch (e) {
        fail(e);
      }
    },
  });
  const renderAuth = () => {
    if (kind === "identity") {
      authBody.replaceChildren(
        identities.get().length ? field("Identity", identitySel, "Only this key is offered to the server.") : h("p", { class: "muted" }, "No keys in the vault yet."),
        h("div", null, genKey),
      );
    } else if (kind === "agent") {
      void loadAgent();
      authBody.replaceChildren(field("Key source", agentSel, "Works like the ssh command. Pin one agent key to stop the others from being offered."));
    } else if (kind === "password") {
      authBody.replaceChildren(field("Password", password, "Encrypted in the vault with XChaCha20-Poly1305."));
    } else {
      authBody.replaceChildren(h("p", { class: "muted" }, "You will be prompted for keyboard-interactive answers (OTP, password) on each connect."));
    }
  };
  renderAuth();

  const others = hosts.get().filter((x) => x.id !== existing?.id);
  let jump: Id[] = [...(existing?.jump ?? [])];
  const chainEl = h("div", { class: "chips" });
  const addJump = select(
    [{ value: "", label: "Add bastion…" }, ...others.map((o) => ({ value: o.id, label: `${o.label} (${o.hostname})` }))],
    "",
  );
  const renderChain = () => {
    chainEl.replaceChildren(
      ...(jump.length
        ? jump.map((id, i) =>
            h(
              "span",
              { class: "chip" },
              h("span", { class: "num" }, String(i + 1)),
              hostById(id)?.label ?? "missing host",
              i > 0
                ? h("button", { type: "button", title: "Move earlier", onClick: () => { const a = jump[i - 1]; const b = jump[i]; if (a && b) { jump[i - 1] = b; jump[i] = a; renderChain(); } } }, icon("up", 11))
                : null,
              h("button", { type: "button", title: "Remove", onClick: () => { jump = jump.filter((_, j) => j !== i); renderChain(); } }, icon("close", 11)),
            ),
          )
        : [h("span", { class: "dim" }, "Direct connection")]),
    );
  };
  renderChain();
  addJump.addEventListener("change", () => {
    if (addJump.value && !jump.includes(addJump.value)) jump.push(addJump.value);
    addJump.value = "";
    renderChain();
  });

  const workspaces = [...new Set(hosts.get().map((x) => x.workspace).filter((w): w is string => Boolean(w)))];
  const wsList = h("datalist", { id: "ws-list" }, workspaces.map((w) => h("option", { value: w })));
  const workspace = input({ value: existing?.workspace ?? "", placeholder: "production", list: "ws-list" });
  const tags = input({ value: existing?.tags.join(", ") ?? "", placeholder: "postgres, eu-central" });
  const keepalive = input({ type: "number", min: 0, max: 600, value: String(existing?.keepalive_secs ?? 15), class: "input mono" });
  const alias = input({ value: existing?.ssh_alias ?? "", placeholder: "Host alias in ~/.ssh/config", class: "input mono" });
  let agentForward = existing?.agent_forward ?? false;
  let legacy = existing?.legacy_crypto ?? false;
  const paste = input({ placeholder: "ssh -p 2222 admin@10.0.0.5", class: "input mono", "aria-label": "Paste an ssh command" });
  paste.addEventListener("input", () => {
    const t = parseSsh(paste.value);
    if (!t) return;
    hostname.value = t.host;
    port.value = String(t.port);
    if (t.user) user.value = t.user;
    if (!label.value.trim()) label.value = t.host;
  });
  const advancedOpen = Boolean(existing && (existing.workspace || existing.tags.length || existing.jump.length || existing.legacy_crypto || existing.agent_forward || existing.ssh_alias));

  const body = h(
    "div",
    { class: "stack" },
    existing ? null : field("Paste an ssh command", paste, "Fills the fields below. Or type them yourself."),
    h("div", { class: "grid-3" }, field("Hostname", hostname), field("Port", port), field("User", user)),
    field("Label", label),
    field(
      "Authentication",
      segmented<Kind>(
        [
          { value: "identity", label: "Key" },
          { value: "agent", label: "System" },
          { value: "password", label: "Password" },
          { value: "interactive", label: "Interactive" },
        ],
        kind,
        (v) => {
          kind = v;
          renderAuth();
        },
      ),
    ),
    authBody,
    h(
      "details",
      { class: "advanced", open: advancedOpen },
      h("summary", { class: "muted", style: { cursor: "pointer" } }, "Advanced"),
      h(
        "div",
        { class: "stack", style: { marginTop: "12px" } },
        h("div", { class: "grid-2" }, field("Workspace", h("div", null, workspace, wsList)), field("Tags", tags)),
        field("Bastion chain", h("div", { class: "stack", style: { gap: "8px" } }, chainEl, others.length ? addJump : null), "Hops are traversed in order through direct-tcpip channels."),
        h("div", { class: "grid-2" }, field("Keepalive (seconds)", keepalive, "0 disables heartbeats."), field("SSH config alias", alias)),
        toggle(agentForward, "Forward agent (exposes only this host's key)", (v) => {
          agentForward = v;
        }),
        toggle(legacy, "Legacy algorithms (SHA-1 key exchange, CBC ciphers) for old routers and appliances", (v) => {
          legacy = v;
        }),
      ),
    ),
  );

  const collect = (): HostInput => {
    let auth: AuthInput;
    if (kind === "identity") auth = { kind, identity: identitySel.value };
    else if (kind === "agent") auth = { kind, fingerprint: agentSel.value || null };
    else if (kind === "password") auth = { kind, password: password.value || null };
    else auth = { kind: "interactive" };
    return {
      id: existing?.id ?? null,
      label: label.value.trim() || hostname.value.trim(),
      hostname: hostname.value.trim(),
      port: Number(port.value) || 22,
      user: user.value.trim(),
      auth,
      jump,
      workspace: workspace.value.trim() || null,
      tags: tags.value.split(",").map((t) => t.trim()).filter(Boolean),
      keepalive_secs: Math.max(0, Math.min(600, Number(keepalive.value) || 0)),
      agent_forward: agentForward,
      legacy_crypto: legacy,
      ssh_alias: alias.value.trim() || null,
    };
  };

  const save = async (connect: boolean) => {
    if (kind === "identity" && !identitySel.value) throw { message: "Select or generate a key first" };
    const saved = await api.hostSave(collect());
    await refreshHosts();
    toast(isEdit ? "Host updated" : "Host saved", "ok", 2000);
    if (connect) openTerminal(saved.id);
  };

  modal({
    title: isEdit ? `Edit ${existing?.label}` : "New host",
    body,
    wide: true,
    actions: [
      { label: "Cancel", kind: "ghost", run: () => undefined },
      { label: "Save", run: () => save(false) },
      { label: isEdit ? "Save and connect" : "Save and connect", kind: "primary", run: () => save(opts.connect ?? true) },
    ],
  });
}

export function asInput(hst: Host): HostInput {
  const auth: AuthInput =
    hst.auth.kind === "password"
      ? { kind: "password", password: null }
      : hst.auth.kind === "identity"
        ? { kind: "identity", identity: hst.auth.identity }
        : hst.auth.kind === "agent"
          ? { kind: "agent", fingerprint: hst.auth.fingerprint }
          : { kind: "interactive" };
  return {
    id: hst.id,
    label: hst.label,
    hostname: hst.hostname,
    port: hst.port,
    user: hst.user,
    auth,
    jump: [...hst.jump],
    workspace: hst.workspace,
    tags: [...hst.tags],
    keepalive_secs: hst.keepalive_secs,
    agent_forward: hst.agent_forward,
    legacy_crypto: hst.legacy_crypto,
    ssh_alias: hst.ssh_alias,
  };
}

async function removeHost(hst: Host): Promise<void> {
  const ok = await confirmBox("Delete host", `Delete ${hst.label}? Its tunnels are removed and it is dropped from bastion chains.`, {
    danger: true,
    confirm: "Delete",
  });
  if (!ok) return;
  try {
    await api.hostDelete(hst.id);
    await refreshHosts();
    toast(`${hst.label} deleted`, "ok", 2000);
  } catch (e) {
    fail(e);
  }
}

function row(hst: Host, selected: boolean, onSelect: () => void): HTMLElement {
  const s = links.get().get(hst.id);
  const chain = hst.jump.map((j) => hostById(j)?.label ?? "?");
  const rtt = s?.state === "up" && typeof s.rtt_ms === "number" ? s.rtt_ms : null;
  const stop = (fn: () => void) => (e: MouseEvent) => {
    e.stopPropagation();
    fn();
  };
  return h(
    "div",
    {
      class: `fleet-row${selected ? " sel" : ""}`,
      role: "row",
      "aria-selected": String(selected),
      dataset: { id: hst.id },
      onClick: onSelect,
      onDblclick: () => openTerminal(hst.id),
    },
    glyph(s),
    h("span", { class: "name", title: hst.label }, hst.label),
    h("span", { class: "addr", title: `${hst.user}@${hst.hostname}:${hst.port}` }, `${hst.user}@${hst.hostname}${hst.port !== 22 ? `:${hst.port}` : ""}`),
    h("span", null, authLabel(hst)),
    h("span", { class: "cell-dim", title: chain.join(" → ") }, chain.length ? `via ${chain.join(" → ")}` : "direct"),
    h("span", { class: "cell-dim" }, hst.tags.join(" ") || "–"),
    h("span", { class: "rtt" }, rtt !== null ? (rtt < 1 ? "<1ms" : `${rtt}ms`) : s?.state === "up" ? "up" : "", sparkline(rttHistory.get(hst.id) ?? [])),
    h(
      "div",
      { class: "fleet-actions" },
      btn("connect", { small: true, onClick: stop(() => openTerminal(hst.id)) }),
      btn("files", { small: true, onClick: stop(() => openFiles(hst.id)) }),
      btn("edit", { small: true, onClick: stop(() => hostForm(hst)) }),
    ),
  );
}

export function hostsPage(): Page {
  return {
    id: "hosts",
    title: "Hosts",
    icon: "hosts",
    render(root) {
      let query = "";
      let selected: Id | null = null;
      let ordered: Host[] = [];
      const searchInput = input({ placeholder: "filter by name, address, tag, workspace", "aria-label": "Filter hosts" });
      const sub = h("div", { class: "sub" });
      const content = h("div");
      const render = () => {
        const all = hosts.get();
        const up = all.filter((x) => links.get().get(x.id)?.state === "up").length;
        sub.textContent = `${all.length} host${all.length === 1 ? "" : "s"} · ${up} connected · ${new Set(all.map((x) => x.workspace ?? "")).size} workspace${new Set(all.map((x) => x.workspace ?? "")).size === 1 ? "" : "s"}`;
        if (!all.length) {
          ordered = [];
          content.replaceChildren(
            empty(
              "The keep is empty",
              "Add your first host, or import what you already have in ~/.ssh/config. Hostnames, users and keys are encrypted on this machine before they touch disk.",
              h("div", { class: "row" }, btn("new host", { primary: true, icon: "plus", onClick: () => hostForm() }), btn("import ~/.ssh/config", { icon: "import", onClick: () => go("settings") })),
            ),
          );
          return;
        }
        const scored = all
          .map((x) => ({ x, s: fuzzy(query, `${x.label} ${x.hostname} ${x.user} ${x.tags.join(" ")} ${x.workspace ?? ""}`) }))
          .filter((r) => r.s >= 0)
          .sort((a, b) => (query ? b.s - a.s : a.x.label.localeCompare(b.x.label)));
        if (!scored.length) {
          ordered = [];
          content.replaceChildren(h("div", { class: "empty" }, h("p", null, `nothing matches "${query}"`)));
          return;
        }
        const groups = new Map<string, Host[]>();
        for (const { x } of scored) groups.set(x.workspace ?? "", [...(groups.get(x.workspace ?? "") ?? []), x]);
        const sortedGroups = [...groups.entries()].sort(([a], [b]) => (a === "" ? 1 : b === "" ? -1 : a.localeCompare(b)));
        ordered = sortedGroups.flatMap(([, list]) => list);
        if (!selected || !ordered.some((x) => x.id === selected)) selected = ordered[0]?.id ?? null;
        const table = h(
          "div",
          { class: "fleet ticks", role: "grid", "aria-label": "Hosts" },
          h("div", { class: "fleet-head", role: "row" }, h("span"), h("span", null, "name"), h("span", null, "address"), h("span", null, "auth"), h("span", null, "route"), h("span", null, "tags"), h("span", { style: { textAlign: "right" } }, "rtt")),
          ...sortedGroups.flatMap(([ws, list]) => [
            ...(sortedGroups.length > 1 || ws ? [h("div", { class: "fleet-group" }, ws || "unsorted", h("span", { class: "n" }, String(list.length)))] : []),
            ...list.map((x) => row(x, x.id === selected, () => select_(x.id))),
          ]),
        );
        content.replaceChildren(table);
      };
      const select_ = (id: Id | null) => {
        selected = id;
        for (const el of content.querySelectorAll<HTMLElement>(".fleet-row")) {
          const on = el.dataset.id === id;
          el.classList.toggle("sel", on);
          el.setAttribute("aria-selected", String(on));
          if (on) el.scrollIntoView({ block: "nearest" });
        }
      };
      const move = (d: number) => {
        if (!ordered.length) return;
        const i = Math.max(0, ordered.findIndex((x) => x.id === selected));
        select_(ordered[Math.max(0, Math.min(ordered.length - 1, i + d))]?.id ?? null);
      };
      const current = () => (selected ? hostById(selected) : undefined);
      searchInput.addEventListener("input", () => {
        query = searchInput.value;
        render();
      });
      searchInput.addEventListener("keydown", (e) => {
        if (e.key === "Enter") {
          e.preventDefault();
          const x = current();
          if (x) openTerminal(x.id);
        } else if (e.key === "ArrowDown") {
          e.preventDefault();
          move(1);
        } else if (e.key === "ArrowUp") {
          e.preventDefault();
          move(-1);
        } else if (e.key === "Escape") {
          searchInput.value = "";
          query = "";
          render();
          searchInput.blur();
        }
      });
      const onKey = (e: KeyboardEvent) => {
        if (root.hidden || root.closest("[hidden]") || document.querySelector(".scrim")) return;
        const a = document.activeElement as HTMLElement | null;
        if (a && (a.tagName === "INPUT" || a.tagName === "TEXTAREA" || a.tagName === "SELECT" || a.closest(".xterm"))) return;
        if (e.ctrlKey || e.metaKey || e.altKey) return;
        const x = current();
        const act: Record<string, () => void> = {
          j: () => move(1),
          ArrowDown: () => move(1),
          k: () => move(-1),
          ArrowUp: () => move(-1),
          Enter: () => x && openTerminal(x.id),
          f: () => x && openFiles(x.id),
          e: () => x && hostForm(x),
          t: () => x && openTunnelForm(undefined, x.id),
          d: () => x && void removeHost(x),
          n: () => hostForm(),
          i: () => go("settings"),
          "/": () => searchInput.focus(),
        };
        const fn = act[keyOf(e)];
        if (fn) {
          e.preventDefault();
          fn();
        }
      };
      window.addEventListener("keydown", onKey);
      root.append(
        h(
          "div",
          { class: "page-inner" },
          h(
            "div",
            { class: "page-head" },
            h("div", null, h("h1", null, "Hosts"), sub),
            h("div", { class: "spacer" }),
            btn("import", { icon: "import", onClick: () => go("settings") }),
            btn("new host", { icon: "plus", primary: true, onClick: () => hostForm() }),
          ),
          h(
            "div",
            { class: "host-tools" },
            h("div", { class: "search" }, h("span", { class: "prompt" }, "/"), searchInput),
            h(
              "div",
              { class: "hints" },
              h("span", null, kbd("j"), kbd("k"), "move"),
              h("span", null, kbd("⏎"), "connect"),
              h("span", null, kbd("f"), "files"),
              h("span", null, kbd("t"), "tunnel"),
              h("span", null, kbd("e"), "edit"),
              h("span", null, kbd("n"), "new"),
            ),
          ),
          content,
        ),
      );
      let stale = false;
      const later = coalesce(() => {
        if (root.closest("[hidden]")) stale = true;
        else render();
      });
      const onShow = () => {
        if (!stale) return;
        stale = false;
        render();
      };
      root.addEventListener("page:show", onShow);
      const unsubs = [hosts.subscribe(render), links.subscribe(later, false), identities.subscribe(later, false), () => root.removeEventListener("page:show", onShow)];
      void refreshHosts().catch((e) => fail(e));
      return () => {
        window.removeEventListener("keydown", onKey);
        for (const u of unsubs) u();
      };
    },
  };
}
