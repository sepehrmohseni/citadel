import { api, events, type Answer, type PromptEvent } from "./api";
import { h, icon } from "./dom";
import { field, input, modal, toggle } from "./ui";

function respond(id: number, answer: Answer): void {
  void api.promptAnswer(id, answer).catch(() => undefined);
}

function hostKey(ev: Extract<PromptEvent, { kind: "host_key" }>): void {
  let answered = false;
  const reply = (accept: boolean) => {
    answered = true;
    respond(ev.id, { kind: "host_key", accept });
  };
  const target = `${ev.host}${ev.port !== 22 ? `:${ev.port}` : ""}`;
  if (ev.status === "changed") {
    let verified = false;
    const m = modal({
      title: "Host key changed",
      onClose: () => {
        if (!answered) reply(false);
      },
      body: h(
        "div",
        { class: "stack" },
        h(
          "div",
          { class: "alert danger" },
          icon("warn", 16),
          h("span", null, `The key presented by ${target} does not match the trusted one. The server may have been reinstalled, or the connection may be intercepted.`),
        ),
        field(`New ${ev.algorithm} fingerprint`, h("code", { class: "fingerprint" }, ev.fingerprint)),
        toggle(false, "I verified this fingerprint with the server owner out of band", (v) => {
          verified = v;
          const dangerBtn = m.el.querySelector<HTMLButtonElement>(".modal-actions .danger");
          if (dangerBtn) dangerBtn.disabled = !v;
        }),
      ),
      actions: [
        { label: "Replace key and connect", kind: "danger", run: () => (verified ? reply(true) : false) },
        { label: "Abort", kind: "primary", run: () => reply(false) },
      ],
    });
    const dangerBtn = m.el.querySelector<HTMLButtonElement>(".modal-actions .danger");
    if (dangerBtn) dangerBtn.disabled = true;
    return;
  }
  modal({
    title: "Verify host identity",
    onClose: () => {
      if (!answered) reply(false);
    },
    body: h(
      "div",
      { class: "stack" },
      h("p", { class: "muted" }, `First connection to ${target}. Compare this fingerprint with the server's before trusting it. Citadel stores the key encrypted in your vault.`),
      field(`${ev.algorithm} fingerprint`, h("code", { class: "fingerprint" }, ev.fingerprint)),
    ),
    actions: [
      { label: "Reject", kind: "ghost", run: () => reply(false) },
      { label: "Trust and connect", kind: "primary", run: () => reply(true) },
    ],
  });
}

function interactive(ev: Extract<PromptEvent, { kind: "interactive" }>): void {
  let answered = false;
  const fields = ev.prompts.map((p) => input({ type: p.echo ? "text" : "password", "aria-label": p.text }));
  const reply = (a: Answer) => {
    answered = true;
    respond(ev.id, a);
    for (const f of fields) f.value = "";
  };
  modal({
    title: `Authenticate ${ev.user}@${ev.host}`,
    onClose: () => {
      if (!answered) reply({ kind: "cancel" });
    },
    body: h(
      "form",
      {
        class: "stack",
        onSubmit: (e: Event) => {
          e.preventDefault();
          (document.querySelector(".modal-actions .primary") as HTMLButtonElement | null)?.click();
        },
      },
      ev.name ? h("strong", null, ev.name) : null,
      ev.instructions ? h("p", { class: "muted" }, ev.instructions) : null,
      ev.prompts.map((p, i) => field(p.text.trim() || `Response ${i + 1}`, fields[i] ?? input())),
      h("button", { type: "submit", hidden: true }),
    ),
    actions: [
      { label: "Cancel", kind: "ghost", run: () => reply({ kind: "cancel" }) },
      { label: "Continue", kind: "primary", run: () => reply({ kind: "interactive", answers: fields.map((f) => f.value) }) },
    ],
  });
}

export function initPrompts(): void {
  void events.prompt((ev) => {
    if (ev.kind === "host_key") hostKey(ev);
    else interactive(ev);
  });
}
