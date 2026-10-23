import "./fleet.css";
import { Channel, api, type FleetEvent, type Host, type Id } from "../api";
import { coalesce, fuzzy, h, icon, kbd } from "../dom";
import { bindings, pretty } from "../keymap";
import type { Page } from "../shell";
import { hosts, links, snippets } from "../store";
import { btn, confirmBox, copyText, fail, input, select, textarea, toast, toggle } from "../ui";

type State = "queued" | "running" | "ok" | "fail" | "error";

interface Result {
  host: Id;
  label: string;
  state: State;
  status: number | null;
  ms: number;
  out: string;
  truncated: boolean;
  error: string | null;
}

const DANGER = /\brm\s+-[a-zA-Z]*[rf]|\bmkfs\b|\bdd\s+if=|\bshutdown\b|\breboot\b|\bpoweroff\b|\bhalt\b|\binit\s+[06]\b|:\(\)\s*\{|\bchmod\s+-R\s+777\b|>\s*\/dev\/[sh]d|\bwipefs\b|\bdrop\s+database\b/i;
const MAX_OUT = 256 * 1024;
const CONFIRM_OVER = 10;

let preselected: Id[] | null = null;
let selection = new Set<Id>();
let lastCommand = "";
const results = new Map<Id, Result>();
let runId: number | null = null;
let started = 0;
let finished = 0;
let repaint: ((done: boolean) => void) | null = null;

function onEvent(ev: FleetEvent): void {
  if (ev.kind === "finished") {
    if (runId === null && finished) return;
    finished = Date.now() - started;
    runId = null;
    const bad = [...results.values()].filter((r) => r.state !== "ok").length;
    toast(bad ? `Fleet run finished: ${bad} host${bad === 1 ? "" : "s"} need attention` : "Fleet run finished cleanly", bad ? "warn" : "ok", 3500);
    repaint?.(true);
    return;
  }
  const r = results.get(ev.host);
  if (!r) return;
  if (ev.kind === "started") r.state = "running";
  else if (ev.kind === "output") {
    if (r.out.length < MAX_OUT) r.out += ev.text;
  } else if (ev.kind === "truncated") r.truncated = true;
  else if (ev.kind === "done") {
    r.status = ev.status;
    r.ms = ev.ms;
    r.state = ev.status === 0 ? "ok" : "fail";
  } else if (ev.kind === "failed") {
    r.error = ev.error;
    r.ms = ev.ms;
    r.state = "error";
  }
  repaint?.(false);
}

export function preselectFleet(ids: Id[]): void {
  preselected = [...ids];
}

function glyph(s: State): string {
  return s === "ok" ? "✓" : s === "fail" ? "✗" : s === "error" ? "!" : s === "running" ? "⧗" : "·";
}

function secs(ms: number): string {
  return ms < 1000 ? `${ms} ms` : `${(ms / 1000).toFixed(ms < 10_000 ? 2 : 1)} s`;
}

export function fleetPage(): Page {
  return {
    id: "fleet",
    title: "Fleet",
    icon: "fleet",
    render(root) {
      if (preselected) {
        selection = new Set(preselected);
        preselected = null;
      }
      let query = "";
      let grouped = true;
      let filter: "all" | "failed" | "ok" = "all";

      const search = input({ placeholder: "filter hosts, tags, workspaces", "aria-label": "Filter hosts" });
      const list = h("div", { class: "fleet-hosts", role: "listbox", "aria-multiselectable": "true" });
      const chips = h("div", { class: "fleet-chips" });
      const count = h("span", { class: "dim" });
      const cmd = textarea({ class: "input mono fleet-cmd", rows: 3, placeholder: "uptime && df -h / | tail -1", "aria-label": "Command" });
      cmd.value = lastCommand;
      const snippetSel = select([{ value: "", label: "insert a snippet…" }, ...snippets.get().map((s) => ({ value: s.id, label: s.label }))], "");
      const timeout = select(
        [
          { value: "10", label: "timeout 10 s" },
          { value: "30", label: "timeout 30 s" },
          { value: "60", label: "timeout 1 min" },
          { value: "300", label: "timeout 5 min" },
          { value: "900", label: "timeout 15 min" },
        ],
        "60",
      );
      const runBtn = btn("run", { primary: true, icon: "play", onClick: () => void run() });
      const cancelBtn = btn("cancel", { icon: "stop", onClick: () => void cancel() });
      runBtn.hidden = runId !== null;
      cancelBtn.hidden = runId === null;
      const summary = h("div", { class: "fleet-summary" });
      const out = h("div", { class: "fleet-results" });
      const filterBar = h("div", { class: "fleet-filter" });

      const visibleHosts = (): Host[] =>
        hosts
          .get()
          .map((x) => ({ x, s: fuzzy(query, `${x.label} ${x.hostname} ${x.user} ${x.tags.join(" ")} ${x.workspace ?? ""}`) }))
          .filter((r) => r.s >= 0)
          .sort((a, b) => (query ? b.s - a.s : a.x.label.localeCompare(b.x.label)))
          .map((r) => r.x);

      const renderHosts = () => {
        const all = hosts.get();
        for (const id of [...selection]) if (!all.some((x) => x.id === id)) selection.delete(id);
        const shown = visibleHosts();
        list.replaceChildren(
          ...(shown.length
            ? shown.map((x) => {
                const on = selection.has(x.id);
                const st = links.get().get(x.id)?.state;
                return h(
                  "button",
                  {
                    type: "button",
                    class: `fleet-host${on ? " on" : ""}`,
                    role: "option",
                    "aria-selected": String(on),
                    onClick: () => {
                      if (selection.has(x.id)) selection.delete(x.id);
                      else selection.add(x.id);
                      renderHosts();
                    },
                  },
                  h("span", { class: "box" }, on ? "[■]" : "[ ]"),
                  h("span", { class: `dot ${st === "up" ? "ok" : st ? "warn" : "idle"}` }),
                  h("span", { class: "name" }, x.label),
                  h("span", { class: "dim addr" }, `${x.user}@${x.hostname}`),
                );
              })
            : [h("div", { class: "muted" }, hosts.get().length ? `nothing matches "${query}"` : "Add hosts first.")]),
        );
        const tags = new Map<string, Id[]>();
        for (const x of all) {
          for (const t of x.tags) tags.set(`#${t}`, [...(tags.get(`#${t}`) ?? []), x.id]);
          if (x.workspace) tags.set(x.workspace, [...(tags.get(x.workspace) ?? []), x.id]);
        }
        chips.replaceChildren(
          h("button", { type: "button", class: "chip-btn", onClick: () => { for (const x of shown) selection.add(x.id); renderHosts(); } }, "all shown"),
          h("button", { type: "button", class: "chip-btn", onClick: () => { selection.clear(); renderHosts(); } }, "none"),
          h(
            "button",
            {
              type: "button",
              class: "chip-btn",
              onClick: () => {
                selection = new Set(all.filter((x) => links.get().get(x.id)?.state === "up").map((x) => x.id));
                renderHosts();
              },
            },
            "connected",
          ),
          ...[...tags.entries()].slice(0, 16).map(([t, ids]) =>
            h(
              "button",
              {
                type: "button",
                class: "chip-btn",
                onClick: () => {
                  for (const id of ids) selection.add(id);
                  renderHosts();
                },
              },
              t,
              h("span", { class: "n" }, String(ids.length)),
            ),
          ),
        );
        count.textContent = `${selection.size} selected`;
        runBtn.querySelector("span")?.replaceChildren(selection.size ? `run on ${selection.size} host${selection.size === 1 ? "" : "s"}` : "run");
        runBtn.disabled = runId !== null || !selection.size;
      };

      const renderResults = () => {
        const all = [...results.values()];
        const ok = all.filter((r) => r.state === "ok").length;
        const bad = all.filter((r) => r.state === "fail" || r.state === "error").length;
        const busy = all.filter((r) => r.state === "running" || r.state === "queued").length;
        const elapsed = runId !== null ? Date.now() - started : finished;
        summary.replaceChildren(
          ...(all.length
            ? [
                h("span", { class: "s-ok" }, `✓ ${ok} ok`),
                h("span", { class: "s-bad" }, `✗ ${bad} failed`),
                h("span", { class: "s-run", hidden: !busy }, `⧗ ${busy} running`),
                h("span", { class: "dim" }, secs(elapsed)),
                h("span", { class: "spacer" }),
                btn("copy report", {
                  small: true,
                  icon: "copy",
                  onClick: () =>
                    void copyText(
                      all.map((r) => `## ${r.label} — ${r.state}${r.status !== null ? ` exit ${r.status}` : ""} (${secs(r.ms)})\n${r.error ?? r.out}`).join("\n\n"),
                      "Report copied",
                    ),
                }),
              ]
            : [h("span", { class: "muted" }, "Pick hosts on the left, type a command, run it everywhere at once.")]),
        );
        filterBar.replaceChildren(
          ...(["all", "failed", "ok"] as const).map((f) =>
            h("button", { type: "button", class: `chip-btn${filter === f ? " on" : ""}`, onClick: () => { filter = f; renderResults(); } }, f),
          ),
          toggle(grouped, "group identical output", (v) => {
            grouped = v;
            renderResults();
          }),
        );
        filterBar.hidden = !all.length;
        const keep = all.filter((r) => filter === "all" || (filter === "ok" ? r.state === "ok" : r.state === "fail" || r.state === "error"));
        const buckets: { key: string; items: Result[] }[] = [];
        for (const r of keep) {
          const done = r.state !== "running" && r.state !== "queued";
          const key = grouped && done ? `${r.state}|${r.status}|${r.error ?? ""}|${r.out}` : r.host;
          const b = buckets.find((x) => x.key === key);
          if (b) b.items.push(r);
          else buckets.push({ key, items: [r] });
        }
        buckets.sort((a, b) => {
          const rank = (r: Result) => (r.state === "error" ? 0 : r.state === "fail" ? 1 : r.state === "running" ? 2 : r.state === "queued" ? 3 : 4);
          return rank(a.items[0] as Result) - rank(b.items[0] as Result) || b.items.length - a.items.length;
        });
        out.replaceChildren(
          ...buckets.map(({ items }) => {
            const r = items[0] as Result;
            const names = items.map((x) => x.label);
            const text = r.error ?? r.out;
            return h(
              "section",
              { class: `fleet-card st-${r.state}` },
              h(
                "header",
                null,
                h("span", { class: "g" }, glyph(r.state)),
                h("span", { class: "who", title: names.join(", ") }, names.length > 3 ? `${names.slice(0, 3).join(", ")} +${names.length - 3}` : names.join(", ")),
                items.length > 1 ? h("span", { class: "badge" }, `${items.length} hosts`) : null,
                h("span", { class: "spacer" }),
                r.status !== null ? h("span", { class: "dim" }, `exit ${r.status}`) : null,
                r.state !== "queued" ? h("span", { class: "dim" }, secs(items.length > 1 ? Math.max(...items.map((x) => x.ms)) : r.ms)) : null,
              ),
              text ? h("pre", { class: "fleet-out" }, text.length > 20000 ? `${text.slice(0, 20000)}\n…` : text) : r.state === "ok" ? h("div", { class: "dim fleet-empty" }, "no output") : null,
              r.truncated ? h("div", { class: "dim fleet-empty" }, "output truncated at 256 KiB") : null,
            );
          }),
        );
      };
      const later = coalesce(renderResults);
      let ticker: ReturnType<typeof setInterval> | undefined;

      repaint = (done) => {
        if (done) {
          runBtn.hidden = false;
          cancelBtn.hidden = true;
          if (ticker) clearInterval(ticker);
          ticker = undefined;
          renderHosts();
          renderResults();
        } else later();
      };
      if (runId !== null) ticker = setInterval(later, 500);

      const run = async () => {
        const command = cmd.value.trim();
        if (!command || !selection.size || runId !== null) return;
        const targets = hosts.get().filter((x) => selection.has(x.id));
        const risky = DANGER.test(command);
        if (risky || targets.length > CONFIRM_OVER) {
          const ok = await confirmBox(
            risky ? "Destructive command" : "Run on many hosts",
            `${risky ? "This command looks destructive. " : ""}It will run on ${targets.length} hosts at once: ${targets
              .slice(0, 6)
              .map((x) => x.label)
              .join(", ")}${targets.length > 6 ? ` and ${targets.length - 6} more` : ""}.`,
            { confirm: `run on ${targets.length}`, danger: risky },
          );
          if (!ok) return;
        }
        lastCommand = command;
        finished = 0;
        results.clear();
        for (const x of targets) results.set(x.id, { host: x.id, label: x.label, state: "queued", status: null, ms: 0, out: "", truncated: false, error: null });
        const feed = new Channel<FleetEvent>();
        feed.onmessage = onEvent;
        started = Date.now();
        try {
          runId = await api.fleetRun(
            targets.map((x) => x.id),
            command,
            Number(timeout.value),
            feed,
          );
          runBtn.hidden = true;
          cancelBtn.hidden = false;
          ticker = setInterval(later, 500);
        } catch (e) {
          runId = null;
          fail(e, "Fleet run");
        }
        renderHosts();
        renderResults();
      };

      const cancel = async () => {
        if (runId === null) return;
        const id = runId;
        await api.fleetCancel(id).catch(() => undefined);
        for (const r of results.values()) if (r.state === "running" || r.state === "queued") {
          r.state = "error";
          r.error = "cancelled";
        }
        onEvent({ kind: "finished" });
      };

      search.addEventListener("input", () => {
        query = search.value;
        renderHosts();
      });
      cmd.addEventListener("keydown", (e) => {
        if (e.key === "Enter" && (e.ctrlKey || e.metaKey)) {
          e.preventDefault();
          void run();
        }
      });
      snippetSel.addEventListener("change", () => {
        const s = snippets.get().find((x) => x.id === snippetSel.value);
        if (s) cmd.value = s.body;
        snippetSel.value = "";
        cmd.focus();
      });

      root.append(
        h(
          "div",
          { class: "page-inner fleet-page" },
          h(
            "div",
            { class: "page-head" },
            h("div", null, h("h1", null, "Fleet run"), h("div", { class: "sub" }, "one command, many hosts, every exit code")),
            h("div", { class: "spacer" }),
            h("div", { class: "hints" }, h("span", null, kbd(...pretty(bindings("go.fleet")[0] ?? "")), " open"), h("span", null, kbd(isMacKey(), "⏎"), " run")),
          ),
          h(
            "div",
            { class: "fleet-grid" },
            h(
              "aside",
              { class: "fleet-pick ticks" },
              h("div", { class: "search" }, h("span", { class: "prompt" }, "/"), search),
              chips,
              list,
              h("div", { class: "fleet-count" }, icon("fleet", 14), count),
            ),
            h(
              "div",
              { class: "fleet-main" },
              h("div", { class: "fleet-compose ticks" }, h("div", { class: "fleet-prompt" }, h("span", { class: "prompt" }, "$"), cmd), h("div", { class: "row" }, snippetSel, timeout, h("span", { class: "spacer" }), cancelBtn, runBtn)),
              summary,
              filterBar,
              out,
            ),
          ),
        ),
      );
      const unsubs = [hosts.subscribe(renderHosts), links.subscribe(coalesce(renderHosts), false)];
      renderResults();
      requestAnimationFrame(() => cmd.focus());
      return () => {
        if (ticker) clearInterval(ticker);
        repaint = null;
        for (const u of unsubs) u();
      };
    },
  };
}

function isMacKey(): string {
  return /Mac|iPhone|iPad/.test(navigator.platform || navigator.userAgent) ? "⌘" : "Ctrl";
}
