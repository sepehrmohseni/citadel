import { api, type AuditReport, type Finding, type Severity } from "../api";
import { h } from "../dom";
import { go, type Page } from "../shell";
import { hostById, hosts, identities, tunnels } from "../store";
import { btn, empty, fail } from "../ui";
import { hostForm } from "./hosts";
import "./audit.css";

const GLYPHS: Record<string, string[]> = {
  A: [" ███ ", "█   █", "█████", "█   █", "█   █"],
  B: ["████ ", "█   █", "████ ", "█   █", "████ "],
  C: [" ████", "█    ", "█    ", "█    ", " ████"],
  D: ["████ ", "█   █", "█   █", "█   █", "████ "],
  F: ["█████", "█    ", "████ ", "█    ", "█    "],
};

const ORDER: Severity[] = ["critical", "high", "medium", "low", "info"];
const TAG: Record<Severity, string> = { critical: "CRIT", high: "HIGH", medium: "MED", low: "LOW", info: "INFO" };

function tone(grade: string): string {
  return grade === "A" ? "a" : grade === "B" ? "b" : grade === "C" ? "c" : "f";
}

function fix(f: Finding): (() => void) | null {
  if (!f.subject) return f.area === "vault" ? () => go("settings") : null;
  const host = hostById(f.subject);
  if (host) return () => hostForm(host);
  if (identities.get().some((k) => k.id === f.subject)) return () => go("keys");
  if (tunnels.get().some((t) => t.id === f.subject)) return () => go("tunnels");
  if (f.area === "sync") return () => go("settings");
  return null;
}

function row(f: Finding): HTMLElement {
  const action = fix(f);
  return h(
    "div",
    { class: `au-row sev-${f.severity}` },
    h("span", { class: "au-tag" }, TAG[f.severity]),
    h("div", { class: "grow" }, h("div", { class: "au-title" }, h("span", { class: "au-area" }, `${f.area}/`), f.title), h("div", { class: "au-detail" }, f.detail)),
    action ? btn("fix →", { small: true, onClick: action }) : null,
  );
}

function render(r: AuditReport): HTMLElement {
  const glyph = GLYPHS[r.grade] ?? GLYPHS.F ?? [];
  const counts = ORDER.map((s) => [s, r.findings.filter((f) => f.severity === s).length] as const);
  const total = Math.max(1, r.findings.length);
  return h(
    "div",
    { class: "au-layout" },
    h(
      "div",
      { class: `au-card ticks grade-${tone(r.grade)}` },
      h("div", { class: "au-card-head" }, "posture"),
      h("pre", { class: "au-grade", "aria-label": `Grade ${r.grade}` }, glyph.join("\n")),
      h("div", { class: "au-score" }, h("b", null, String(r.score)), h("span", null, "/100")),
      h("div", { class: "au-meta" }, `${r.checked} items checked`),
      h(
        "div",
        { class: "au-dist", title: counts.map(([s, n]) => `${s}: ${n}`).join("  ") },
        ...counts.filter(([, n]) => n > 0).map(([s, n]) => h("span", { class: `sev-${s}`, style: { flexGrow: String(n / total) } })),
      ),
      h(
        "div",
        { class: "au-legend" },
        ...counts.map(([s, n]) => h("span", { class: `sev-${s}` }, h("i"), `${TAG[s].toLowerCase()} ${n}`)),
      ),
    ),
    h(
      "div",
      { class: "au-findings" },
      r.findings.length
        ? h("div", { class: "list" }, r.findings.map(row))
        : empty("Nothing to report", "Every host uses a modern key, nothing is exposed beyond loopback, and the vault locks itself. Keep it that way."),
    ),
  );
}

export function auditPage(): Page {
  return {
    id: "audit",
    title: "Audit",
    icon: "shield",
    render(root) {
      const body = h("div", null, h("div", { class: "muted" }, h("span", { class: "spinner" }), " scanning vault"));
      const run = async () => {
        try {
          body.replaceChildren(render(await api.vaultAudit()));
        } catch (e) {
          fail(e, "Audit failed");
        }
      };
      root.append(
        h(
          "div",
          { class: "page-inner" },
          h(
            "div",
            { class: "page-head" },
            h("div", null, h("h1", null, "Audit"), h("div", { class: "sub" }, "Posture of everything in the vault, scored locally. Nothing leaves this machine.")),
            h("div", { class: "spacer" }),
            btn("rescan", { icon: "refresh", onClick: () => void run() }),
          ),
          body,
        ),
      );
      void run();
      const unsubs = [hosts.subscribe(() => void run(), false), identities.subscribe(() => void run(), false), tunnels.subscribe(() => void run(), false)];
      return () => {
        for (const u of unsubs) u();
      };
    },
  };
}
