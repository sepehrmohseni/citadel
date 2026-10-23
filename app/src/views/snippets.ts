import "./pages.css";
import { api, type Snippet } from "../api";
import { clear, h, icon, kbd, modKey } from "../dom";
import { onTabsChanged, type Page } from "../shell";
import { refreshSnippets, snippets } from "../store";
import { activeTerminalKey, liveTerminals, sendText } from "../terminal";
import { btn, confirmBox, empty, fail, field, input, modal, textarea, toast } from "../ui";

function toPty(body: string): string {
  const s = body.replace(/\r\n/g, "\n").replace(/\n/g, "\r");
  return s.endsWith("\r") ? s : `${s}\r`;
}

function lineCount(body: string): number {
  return body.replace(/\s+$/, "").split("\n").length;
}

function editModal(s: Snippet | null): void {
  const label = input({ placeholder: "Tail nginx errors", value: s?.label ?? "" });
  const body = textarea({ placeholder: "sudo tail -f /var/log/nginx/error.log", rows: 10, value: s?.body ?? "" });
  body.value = s?.body ?? "";
  body.addEventListener("keydown", (e) => {
    if (e.key === "Tab" && !e.shiftKey) {
      e.preventDefault();
      const at = body.selectionStart;
      body.setRangeText("  ", at, body.selectionEnd, "end");
    }
  });
  modal({
    title: s ? "Edit snippet" : "New snippet",
    wide: true,
    body: h(
      "div",
      { class: "stack" },
      field("Label", label),
      field("Command", body, "Sent verbatim to the shell. Multi-line snippets run line by line; a trailing return is added if missing."),
    ),
    actions: [
      { label: "Cancel", kind: "ghost", run: () => undefined },
      {
        label: s ? "Save" : "Create",
        kind: "primary",
        run: async () => {
          if (!label.value.trim()) {
            toast("A label is required", "warn");
            return false;
          }
          if (!body.value.trim()) {
            toast("The command is empty", "warn");
            return false;
          }
          await api.snippetSave(s?.id ?? null, label.value.trim(), body.value);
          await refreshSnippets();
          return true;
        },
      },
    ],
  });
}

function runActive(s: Snippet): void {
  const key = activeTerminalKey();
  if (!key) {
    toast("Open a terminal first", "warn");
    return;
  }
  sendText(key, toPty(s.body));
  const t = liveTerminals().find((x) => x.key === key);
  toast(`Sent "${s.label}" to ${t?.title ?? "terminal"}`, "ok", 2200);
}

function broadcastModal(s: Snippet): void {
  const terms = liveTerminals();
  if (!terms.length) {
    toast("No terminals are open", "warn");
    return;
  }
  const active = activeTerminalKey();
  const boxes = terms.map((t) => ({ t, cb: h("input", { type: "checkbox", checked: t.key === active || terms.length === 1 }) as HTMLInputElement }));
  const all = h("input", { type: "checkbox" }) as HTMLInputElement;
  const warn = h("div", { class: "alert" }, icon("warn", 15), h("span"));
  const count = () => boxes.filter((b) => b.cb.checked).length;
  const sync = () => {
    const n = count();
    all.checked = n === boxes.length;
    all.indeterminate = n > 0 && n < boxes.length;
    const hostsHit = new Set(boxes.filter((b) => b.cb.checked).map((b) => b.t.host)).size;
    const text = warn.lastElementChild;
    if (text) {
      text.textContent =
        n === 0
          ? "Select at least one terminal."
          : `This sends the command to ${n} terminal${n === 1 ? "" : "s"} on ${hostsHit} host${hostsHit === 1 ? "" : "s"} and executes it immediately.`;
    }
    warn.classList.toggle("danger", n > 1);
  };
  all.addEventListener("change", () => {
    for (const b of boxes) b.cb.checked = all.checked;
    sync();
  });
  const rows = boxes.map(({ t, cb }) => {
    cb.addEventListener("change", sync);
    return h("label", { class: "pick-row" }, cb, h("div", { class: "grow" }, h("div", { class: "title" }, t.title)), icon("terminal", 14));
  });
  sync();
  modal({
    title: `Broadcast · ${s.label}`,
    wide: true,
    body: h(
      "div",
      { class: "stack" },
      h("pre", { class: "snip-pre" }, s.body),
      h("div", null, h("div", { class: "pick-head" }, h("span", null, "Target terminals"), h("label", null, all, "Select all")), h("div", { class: "pick-list" }, rows)),
      warn,
    ),
    actions: [
      { label: "Cancel", kind: "ghost", run: () => undefined },
      {
        label: "Send",
        kind: "danger",
        run: () => {
          const targets = boxes.filter((b) => b.cb.checked);
          if (!targets.length) {
            toast("Select at least one terminal", "warn");
            return false;
          }
          const text = toPty(s.body);
          for (const b of targets) sendText(b.t.key, text);
          toast(`Sent to ${targets.length} terminal${targets.length === 1 ? "" : "s"}`, "ok");
          return true;
        },
      },
    ],
  });
}

function card(s: Snippet, live: number): HTMLElement {
  const lines = lineCount(s.body);
  return h(
    "div",
    { class: "snip-card" },
    h("div", { class: "snip-title" }, icon("snippet", 15), h("span", { title: s.label }, s.label), lines > 1 ? h("span", { class: "badge" }, `${lines} lines`) : null),
    h("pre", { class: "snip-pre" }, s.body),
    h(
      "div",
      { class: "snip-actions" },
      btn("Run", {
        icon: "play",
        small: true,
        primary: live > 0,
        disabled: live === 0,
        title: live ? "Run in the active terminal" : "Open a terminal to run snippets",
        onClick: () => runActive(s),
      }),
      btn("Broadcast", {
        icon: "broadcast",
        small: true,
        disabled: live === 0,
        title: live ? "Send to several terminals" : "Open a terminal to broadcast",
        onClick: () => broadcastModal(s),
      }),
      h("span", { class: "spacer" }),
      btn("", { icon: "edit", ghost: true, small: true, title: "Edit snippet", onClick: () => editModal(s) }),
      btn("", {
        icon: "trash",
        ghost: true,
        small: true,
        title: "Delete snippet",
        onClick: async () => {
          if (!(await confirmBox("Delete snippet", `"${s.label}" will be removed from the vault.`, { danger: true, confirm: "Delete" }))) return;
          try {
            await api.snippetDelete(s.id);
            await refreshSnippets();
          } catch (e) {
            fail(e);
          }
        },
      }),
    ),
  );
}

export function snippetsPage(): Page {
  return {
    id: "snippets",
    title: "Snippets",
    icon: "snippet",
    render(root: HTMLElement) {
      const hint = h("div", { class: "snip-hint" });
      const grid = h("div");
      const draw = () => {
        const live = liveTerminals().length;
        hint.replaceChildren(
          icon("terminal", 14),
          live
            ? h("span", null, `${live} terminal${live === 1 ? "" : "s"} open. Run targets the most recently focused one.`)
            : h("span", null, "No terminals open. Connect to a host to run snippets."),
          h("span", { style: { flex: "1" } }),
          h("span", { class: "dim" }, "Snippets are also available from the command palette "),
          kbd(modKey, "K"),
        );
        clear(grid);
        const list = snippets.get();
        if (!list.length) {
          grid.append(
            empty(
              "No snippets yet",
              "Save commands you run often. Snippets are stored encrypted in the vault, sync with it, and can be broadcast to several sessions at once.",
              btn("New snippet", { icon: "plus", primary: true, onClick: () => editModal(null) }),
            ),
          );
          return;
        }
        grid.append(h("div", { class: "snip-grid" }, list.map((s) => card(s, live))));
      };
      root.append(
        h(
          "div",
          { class: "page-inner" },
          h(
            "div",
            { class: "page-head" },
            h("div", null, h("h1", null, "Snippets"), h("div", { class: "sub" }, "Reusable commands for one terminal or many.")),
            h("div", { class: "spacer" }),
            btn("New snippet", { icon: "plus", primary: true, onClick: () => editModal(null) }),
          ),
          hint,
          grid,
        ),
      );
      const unsubs = [snippets.subscribe(draw), onTabsChanged(draw)];
      void refreshSnippets().catch((e) => fail(e));
      return () => {
        for (const u of unsubs) u();
      };
    },
  };
}
