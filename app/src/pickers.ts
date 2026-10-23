import { api, type FontFamily } from "./api";
import { h } from "./dom";

const BUNDLED = "JetBrains Mono Variable";
const SAMPLE = "Ag 0O 1lI {} => ~/";

export function primaryFamily(stack: string): string {
  const first = stack.split(",")[0] ?? stack;
  return first.trim().replace(/^["']|["']$/g, "") || BUNDLED;
}

export function stackFor(family: string): string {
  return family === BUNDLED ? `"${BUNDLED}", monospace` : `"${family}", "${BUNDLED}", monospace`;
}

let cache: Promise<FontFamily[]> | null = null;

function families(): Promise<FontFamily[]> {
  cache ??= api.fonts().catch(() => []);
  return cache;
}

export function fontPicker(current: string, onChange: (stack: string) => void): HTMLElement {
  let chosen = primaryFamily(current);
  const face = h("span", { class: "fpk-face" });
  const name = h("span", { class: "fpk-name" });
  const paint = () => {
    face.textContent = SAMPLE;
    face.style.fontFamily = `"${chosen}", monospace`;
    name.textContent = chosen;
  };
  paint();
  const button = h(
    "button",
    { type: "button", class: "fpk-button", "aria-haspopup": "listbox", onClick: () => openList() },
    h("span", { class: "fpk-stack" }, name, face),
    h("span", { class: "fpk-caret" }, "▾"),
  );
  const root = h("div", { class: "fpk" }, button);

  const openList = () => {
    if (root.querySelector(".fpk-pop")) return;
    const query = h("input", { class: "input", placeholder: "filter fonts", spellcheck: false, autocomplete: "off", "aria-label": "Filter fonts" }) as HTMLInputElement;
    const list = h("div", { class: "fpk-list", role: "listbox" });
    let showAll = false;
    const allToggle = h("button", { type: "button", class: "fpk-more" }, "show proportional fonts");
    const pop = h("div", { class: "fpk-pop" }, query, list, allToggle);
    root.append(pop);
    let items: HTMLButtonElement[] = [];
    let sel = 0;
    let fonts: FontFamily[] = [];
    const close = () => {
      pop.remove();
      document.removeEventListener("mousedown", outside, true);
      button.focus();
    };
    const outside = (e: MouseEvent) => {
      if (!root.contains(e.target as Node)) close();
    };
    document.addEventListener("mousedown", outside, true);
    const pick = (fam: string) => {
      chosen = fam;
      paint();
      onChange(stackFor(fam));
      close();
    };
    const highlight = () => {
      items.forEach((b, i) => b.classList.toggle("on", i === sel));
      items[sel]?.scrollIntoView({ block: "nearest" });
    };
    const render = () => {
      const q = query.value.trim().toLowerCase();
      const mono = fonts.filter((f) => f.mono && f.name !== BUNDLED);
      const prop = showAll ? fonts.filter((f) => !f.mono) : [];
      const match = (f: string) => !q || f.toLowerCase().includes(q);
      const groups: [string, string[]][] = [
        ["bundled", [BUNDLED].filter(match)],
        ["monospace on this system", mono.map((f) => f.name).filter(match)],
        ["proportional", prop.map((f) => f.name).filter(match)],
      ];
      items = [];
      list.replaceChildren(
        ...groups.flatMap(([title, names]) =>
          names.length
            ? [
                h("div", { class: "fpk-group" }, title),
                ...names.slice(0, 400).map((fam) => {
                  const b = h(
                    "button",
                    {
                      type: "button",
                      class: `fpk-item${fam === chosen ? " current" : ""}`,
                      role: "option",
                      "aria-selected": String(fam === chosen),
                      onClick: () => pick(fam),
                    },
                    h("span", { class: "fpk-label" }, fam),
                    h("span", { class: "fpk-sample", style: { fontFamily: `"${fam}", monospace` } }, SAMPLE),
                  ) as HTMLButtonElement;
                  items.push(b);
                  return b;
                }),
              ]
            : [],
        ),
      );
      if (!items.length) list.append(h("div", { class: "fpk-group" }, fonts.length ? "no font matches" : "reading installed fonts…"));
      sel = Math.max(0, items.findIndex((b) => b.classList.contains("current")));
      highlight();
      allToggle.textContent = showAll ? "monospace only" : "show proportional fonts";
    };
    allToggle.addEventListener("click", () => {
      showAll = !showAll;
      render();
      query.focus();
    });
    query.addEventListener("input", () => {
      render();
      sel = 0;
      highlight();
    });
    query.addEventListener("keydown", (e) => {
      if (e.key === "ArrowDown") {
        e.preventDefault();
        sel = Math.min(items.length - 1, sel + 1);
        highlight();
      } else if (e.key === "ArrowUp") {
        e.preventDefault();
        sel = Math.max(0, sel - 1);
        highlight();
      } else if (e.key === "Enter") {
        e.preventDefault();
        const b = items[sel];
        if (b) b.click();
      } else if (e.key === "Escape") {
        e.preventDefault();
        e.stopPropagation();
        close();
      }
    });
    render();
    requestAnimationFrame(() => query.focus());
    void families().then((f) => {
      fonts = f;
      if (pop.isConnected) render();
    });
  };
  return root;
}

export function stepper(
  value: number,
  o: { min: number; max: number; step: number; format?: (v: number) => string; label: string },
  onChange: (v: number) => void,
): HTMLElement {
  let v = value;
  const out = h("output", { class: "stp-value", "aria-live": "polite" });
  const fmt = o.format ?? ((x: number) => String(x));
  const set = (next: number) => {
    const clamped = Math.min(o.max, Math.max(o.min, Math.round(next / o.step) * o.step));
    const fixed = Number(clamped.toFixed(4));
    if (fixed === v) return;
    v = fixed;
    out.textContent = fmt(v);
    dec.disabled = v <= o.min;
    inc.disabled = v >= o.max;
    onChange(v);
  };
  const dec = h("button", { type: "button", class: "stp-btn", "aria-label": `Decrease ${o.label}`, onClick: () => set(v - o.step) }, "−") as HTMLButtonElement;
  const inc = h("button", { type: "button", class: "stp-btn", "aria-label": `Increase ${o.label}`, onClick: () => set(v + o.step) }, "+") as HTMLButtonElement;
  out.textContent = fmt(v);
  dec.disabled = v <= o.min;
  inc.disabled = v >= o.max;
  const root = h("div", { class: "stp", role: "group", "aria-label": o.label, tabindex: 0 }, dec, out, inc);
  root.addEventListener("keydown", (e) => {
    if (e.key === "ArrowUp" || e.key === "ArrowRight") {
      e.preventDefault();
      set(v + o.step);
    } else if (e.key === "ArrowDown" || e.key === "ArrowLeft") {
      e.preventDefault();
      set(v - o.step);
    }
  });
  return root;
}
