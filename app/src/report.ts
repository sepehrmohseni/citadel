import { api } from "./api";
import { h, icon } from "./dom";
import { fail, field, input, modal, textarea, toast, toggle } from "./ui";

/**
 * Bug reports go straight into a new GitHub issue: Citadel opens the issue form with the title,
 * text and (if wanted) version details filled in, and the user presses Submit on GitHub.
 */
export async function reportBug(): Promise<void> {
  const info = await api.supportInfo().catch(() => null);
  const title = input({ placeholder: "Ctrl+B types letters instead of reaching tmux", maxlength: 200 });
  const text = textarea({
    rows: 8,
    placeholder: "What happened, what you expected instead, and the steps that make it happen again.",
  });
  let details = info !== null;
  const detailLine = info ? `Citadel ${info.version} · ${info.os} ${info.arch} · ${info.package}` : "";
  modal({
    title: "Report a bug",
    wide: true,
    focus: "input",
    body: h(
      "div",
      { class: "stack" },
      h("p", { class: "muted" }, "Citadel opens a new issue on GitHub with your report filled in. Sign in to GitHub, look it over and press Submit."),
      field("Title", title),
      field("What happened", text),
      info
        ? toggle(true, `Include the version and system: ${detailLine}`, (v) => {
            details = v;
          })
        : null,
      h(
        "div",
        { class: "note" },
        icon("shield", 15),
        h("span", null, "Issues are public. Leave out passwords, keys, host names and anything else private. Report security problems privately through GitHub Security Advisories instead."),
      ),
    ),
    actions: [
      { label: "Cancel", kind: "ghost", run: () => undefined },
      {
        label: "Open on GitHub",
        kind: "primary",
        run: async () => {
          if (title.value.trim().length < 3) {
            toast("Give the report a short title", "warn");
            return false;
          }
          if (!text.value.trim()) {
            toast("Describe what happened", "warn");
            return false;
          }
          try {
            await api.issueOpen(title.value, text.value, details);
            toast("The issue is open on GitHub. Press Submit there to send it.", "ok", 7000);
            return true;
          } catch (e) {
            fail(e, "Could not open GitHub");
            return false;
          }
        },
      },
    ],
  });
}
