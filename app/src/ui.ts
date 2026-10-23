import { readText, writeText } from "@tauri-apps/plugin-clipboard-manager";

import { errText } from "./api";
import { h, icon, type Props } from "./dom";

type ToastKind = "info" | "ok" | "error" | "warn";

let toastHost: HTMLElement | null = null;

export const FORT = ["▄ ▄ ▄ ▄ ▄ ▄", "█▄█▄█▄█▄█▄█", "█  ▄▀▀▀▄  █", "█  █   █  █", "▀▀▀▀▀▀▀▀▀▀▀"].join("\n");

export function toast(message: string, kind: ToastKind = "info", ms = 4200): void {
  if (!toastHost) {
    toastHost = h("div", { class: "toasts", role: "status", "aria-live": "polite" });
    document.body.append(toastHost);
  }
  const lvl = kind === "ok" ? "OK" : kind === "error" ? "ERR" : kind === "warn" ? "WARN" : "INFO";
  const now = new Date();
  const ts = [now.getHours(), now.getMinutes(), now.getSeconds()].map((n) => String(n).padStart(2, "0")).join(":");
  const el = h("div", { class: `toast toast-${kind}` }, h("span", { class: "lvl" }, lvl), h("span", { class: "ts" }, ts), h("span", null, message));
  toastHost.append(el);
  requestAnimationFrame(() => el.classList.add("in"));
  const kill = () => {
    el.classList.remove("in");
    setTimeout(() => el.remove(), 220);
  };
  el.addEventListener("click", kill);
  setTimeout(kill, ms);
}

export function actionToast(message: string, actions: { label: string; run: () => void }[], ms = 12000): void {
  if (!toastHost) {
    toastHost = h("div", { class: "toasts", role: "status", "aria-live": "polite" });
    document.body.append(toastHost);
  }
  const now = new Date();
  const ts = [now.getHours(), now.getMinutes(), now.getSeconds()].map((n) => String(n).padStart(2, "0")).join(":");
  const el = h("div", { class: "toast toast-info toast-action" });
  const kill = () => {
    el.classList.remove("in");
    setTimeout(() => el.remove(), 220);
  };
  el.append(
    h("span", { class: "lvl" }, "NEW"),
    h("span", { class: "ts" }, ts),
    h("span", { class: "grow" }, message),
    ...actions.map((a) =>
      h(
        "button",
        {
          type: "button",
          class: "btn small",
          onClick: (e: MouseEvent) => {
            e.stopPropagation();
            kill();
            a.run();
          },
        },
        a.label,
      ),
    ),
    h("button", { type: "button", class: "btn small ghost", "aria-label": "Dismiss", onClick: kill }, "×"),
  );
  toastHost.append(el);
  requestAnimationFrame(() => el.classList.add("in"));
  setTimeout(kill, ms);
}

export function fail(e: unknown, prefix?: string): void {
  toast(prefix ? `${prefix}: ${errText(e)}` : errText(e), "error", 6500);
}

export interface Action {
  label: string;
  kind?: "primary" | "danger" | "ghost";
  run: () => unknown | Promise<unknown>;
  keep?: boolean;
}

export interface Modal {
  el: HTMLElement;
  close: () => void;
  busy: (on: boolean) => void;
}

const stack: Modal[] = [];

const FOCUSABLE = "input:not([type=hidden]):not([disabled]), textarea:not([disabled]), select:not([disabled]), button:not([disabled]), [href], [tabindex]:not([tabindex='-1'])";

export function modal(opts: { title: string; body: Node; actions?: Action[]; wide?: boolean; onClose?: () => void; focus?: string }): Modal {
  const actionsEl = h("div", { class: "modal-actions" });
  const opener = document.activeElement as HTMLElement | null;
  const card = h(
    "div",
    { class: `modal ticks${opts.wide ? " wide" : ""}`, role: "dialog", "aria-modal": "true", "aria-label": opts.title, tabindex: -1 },
    h("header", { class: "modal-head" }, h("h2", null, opts.title), btn("", { icon: "close", ghost: true, title: "Close", onClick: () => close() })),
    h("div", { class: "modal-body" }, opts.body),
    actionsEl,
  );
  const scrim = h("div", { class: "scrim" }, card);
  let closed = false;
  const close = () => {
    if (closed) return;
    closed = true;
    scrim.classList.remove("in");
    const i = stack.indexOf(handle);
    if (i >= 0) stack.splice(i, 1);
    setTimeout(() => scrim.remove(), 160);
    opts.onClose?.();
    if (opener?.isConnected && !stack.length) opener.focus();
  };
  const busy = (on: boolean) => {
    card.classList.toggle("busy", on);
    for (const b of actionsEl.querySelectorAll("button")) (b as HTMLButtonElement).disabled = on;
  };
  const handle: Modal = { el: card, close, busy };
  for (const a of opts.actions ?? []) {
    actionsEl.append(
      btn(a.label, {
        primary: a.kind === "primary",
        danger: a.kind === "danger",
        ghost: a.kind === "ghost",
        onClick: async () => {
          busy(true);
          try {
            const r = await a.run();
            if (!a.keep && r !== false) close();
          } catch (e) {
            fail(e);
          } finally {
            busy(false);
          }
        },
      }),
    );
  }
  scrim.addEventListener("mousedown", (e) => {
    if (e.target === scrim) close();
  });
  card.addEventListener("keydown", (e) => {
    if (e.key === "Escape") {
      e.stopPropagation();
      close();
    }
    if (e.key === "Tab") {
      const items = [...card.querySelectorAll<HTMLElement>(FOCUSABLE)].filter((x) => x.offsetParent !== null);
      const first = items[0];
      const last = items[items.length - 1];
      if (first && last) {
        if (e.shiftKey && (document.activeElement === first || document.activeElement === card)) {
          e.preventDefault();
          last.focus();
        } else if (!e.shiftKey && document.activeElement === last) {
          e.preventDefault();
          first.focus();
        }
      }
    }
    if (e.key === "Enter" && (e.metaKey || e.ctrlKey)) {
      const primary = actionsEl.querySelector<HTMLButtonElement>("button.primary");
      primary?.click();
    }
  });
  document.body.append(scrim);
  stack.push(handle);
  requestAnimationFrame(() => {
    scrim.classList.add("in");
    const risky = (opts.actions ?? []).some((a) => a.kind === "danger");
    const target =
      (opts.focus ? card.querySelector<HTMLElement>(opts.focus) : null) ??
      card.querySelector<HTMLElement>(".modal-body input:not([type=hidden]):not([disabled]), .modal-body textarea, .modal-body select") ??
      (risky ? actionsEl.querySelector<HTMLElement>("button.ghost") : actionsEl.querySelector<HTMLElement>("button.primary")) ??
      card;
    target.focus();
  });
  return handle;
}

export function topModal(): Modal | undefined {
  return stack[stack.length - 1];
}

export function confirmBox(title: string, message: string, opts: { danger?: boolean; confirm?: string } = {}): Promise<boolean> {
  return new Promise((resolve) => {
    let answered = false;
    modal({
      title,
      body: h("p", { class: "muted" }, message),
      onClose: () => {
        if (!answered) resolve(false);
      },
      actions: [
        { label: "Cancel", kind: "ghost", run: () => undefined },
        {
          label: opts.confirm ?? "Confirm",
          kind: opts.danger ? "danger" : "primary",
          run: () => {
            answered = true;
            resolve(true);
          },
        },
      ],
    });
  });
}

export function askText(
  title: string,
  label: string,
  opts: { secret?: boolean; value?: string; placeholder?: string; hint?: string; confirm?: string } = {},
): Promise<string | null> {
  return new Promise((resolve) => {
    const inp = input({ type: opts.secret ? "password" : "text", value: opts.value ?? "", placeholder: opts.placeholder ?? "" });
    let answered = false;
    const m = modal({
      title,
      body: field(label, inp, opts.hint),
      onClose: () => {
        if (!answered) resolve(null);
      },
      actions: [
        { label: "Cancel", kind: "ghost", run: () => undefined },
        {
          label: opts.confirm ?? "OK",
          kind: "primary",
          run: () => {
            answered = true;
            resolve(inp.value);
          },
        },
      ],
    });
    inp.addEventListener("keydown", (e) => {
      if (e.key === "Enter") {
        answered = true;
        resolve(inp.value);
        m.close();
      }
    });
  });
}

export function btn(
  label: string,
  o: { icon?: string; primary?: boolean; danger?: boolean; ghost?: boolean; small?: boolean; title?: string; onClick?: (e: MouseEvent) => void; disabled?: boolean } = {},
): HTMLButtonElement {
  const cls = ["btn"];
  if (o.primary) cls.push("primary");
  if (o.danger) cls.push("danger");
  if (o.ghost) cls.push("ghost");
  if (o.small) cls.push("small");
  if (!label) cls.push("icon-only");
  const b = h(
    "button",
    { type: "button", class: cls.join(" "), title: o.title ?? label, "aria-label": o.title ?? label, disabled: o.disabled },
    o.icon ? icon(o.icon, o.small ? 14 : 16) : null,
    label ? h("span", null, label) : null,
  );
  if (o.onClick) b.addEventListener("click", o.onClick);
  return b;
}

export function input(props: Props = {}): HTMLInputElement {
  return h("input", { class: "input", spellcheck: false, autocomplete: "off", dir: "auto", ...props }) as HTMLInputElement;
}

export function textarea(props: Props = {}): HTMLTextAreaElement {
  return h("textarea", { class: "input", spellcheck: false, dir: "auto", ...props }) as HTMLTextAreaElement;
}

export function select(options: { value: string; label: string }[], value: string, props: Props = {}): HTMLSelectElement {
  const s = h("select", { class: "input", ...props }, options.map((o) => h("option", { value: o.value }, o.label))) as HTMLSelectElement;
  s.value = value;
  return s;
}

export function toggle(checked: boolean, label: string, onChange?: (v: boolean) => void): HTMLLabelElement {
  const cb = h("input", { type: "checkbox", checked }) as HTMLInputElement;
  if (onChange) cb.addEventListener("change", () => onChange(cb.checked));
  return h("label", { class: "toggle" }, cb, h("span", { class: "track" }, h("span", { class: "thumb" })), h("span", null, label)) as HTMLLabelElement;
}

export function field(label: string, control: Node, hint?: string): HTMLElement {
  const labelable = control instanceof HTMLInputElement || control instanceof HTMLSelectElement || control instanceof HTMLTextAreaElement;
  return h(
    labelable ? "label" : "div",
    labelable ? { class: "field" } : { class: "field", role: "group", "aria-label": label },
    h("span", { class: "field-label" }, label),
    control,
    hint ? h("span", { class: "field-hint" }, hint) : null,
  );
}

export function segmented<T extends string>(options: { value: T; label: string }[], value: T, onChange: (v: T) => void): HTMLElement {
  const root = h("div", { class: "segmented", role: "tablist" });
  const render = (cur: T) => {
    root.replaceChildren(
      ...options.map((o) =>
        h(
          "button",
          {
            type: "button",
            role: "tab",
            class: o.value === cur ? "on" : "",
            "aria-selected": String(o.value === cur),
            onClick: () => {
              render(o.value);
              onChange(o.value);
            },
          },
          o.label,
        ),
      ),
    );
  };
  render(value);
  return root;
}

export function empty(title: string, text: string, action?: HTMLElement): HTMLElement {
  return h("div", { class: "empty" }, h("pre", { class: "empty-mark", "aria-hidden": "true" }, FORT), h("h3", null, title), h("p", null, text), action ?? null);
}

export async function writeClipboard(text: string): Promise<void> {
  try {
    await writeText(text);
  } catch {
    await navigator.clipboard.writeText(text);
  }
}

export async function readClipboard(): Promise<string> {
  try {
    return (await readText()) ?? "";
  } catch {
    return navigator.clipboard.readText();
  }
}

export async function copyText(text: string, what = "Copied"): Promise<void> {
  try {
    await writeClipboard(text);
    toast(what, "ok", 1800);
  } catch (e) {
    fail(e, "Clipboard unavailable");
  }
}

export interface MenuItem {
  label: string;
  icon: string;
  run: () => void;
  hint?: string;
  danger?: boolean;
  disabled?: boolean;
}

let openMenu: (() => void) | null = null;

export function contextMenu(x: number, y: number, items: (MenuItem | null)[]): void {
  openMenu?.();
  const menu = h("div", { class: "fp-menu", role: "menu" });
  const close = () => {
    menu.remove();
    document.removeEventListener("mousedown", onDown, true);
    document.removeEventListener("keydown", onKeyDown, true);
    window.removeEventListener("blur", close);
    if (openMenu === close) openMenu = null;
  };
  const onDown = (e: MouseEvent) => {
    if (!menu.contains(e.target as Node)) close();
  };
  const onKeyDown = (e: KeyboardEvent) => {
    if (e.key === "Escape") {
      e.preventDefault();
      close();
    }
  };
  for (const it of items) {
    if (!it) {
      menu.append(h("div", { class: "fp-menu-sep" }));
      continue;
    }
    menu.append(
      h(
        "button",
        {
          class: `fp-menu-item${it.danger ? " danger" : ""}`,
          type: "button",
          role: "menuitem",
          disabled: it.disabled,
          onMousedown: (e: MouseEvent) => e.preventDefault(),
          onClick: () => {
            close();
            it.run();
          },
        },
        icon(it.icon, 14),
        h("span", { class: "grow" }, it.label),
        it.hint ? h("span", { class: "fp-menu-hint" }, it.hint) : null,
      ),
    );
  }
  document.body.append(menu);
  const r = menu.getBoundingClientRect();
  menu.style.left = `${Math.max(4, Math.min(x, window.innerWidth - r.width - 8))}px`;
  menu.style.top = `${Math.max(4, Math.min(y, window.innerHeight - r.height - 8))}px`;
  document.addEventListener("mousedown", onDown, true);
  document.addEventListener("keydown", onKeyDown, true);
  window.addEventListener("blur", close);
  openMenu = close;
}
