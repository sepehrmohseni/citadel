import { describe, expect, it, vi } from "vitest";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn(), Channel: class {} }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn(async () => () => undefined) }));
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: vi.fn(), save: vi.fn() }));
vi.mock("@tauri-apps/plugin-clipboard-manager", () => ({ readText: vi.fn(), writeText: vi.fn() }));
vi.mock("@xterm/xterm", () => ({ Terminal: class {} }));
vi.mock("@xterm/xterm/css/xterm.css", () => ({}));
for (const m of ["@xterm/addon-fit", "@xterm/addon-search", "@xterm/addon-unicode11", "@xterm/addon-web-links", "@xterm/addon-webgl"]) {
  vi.doMock(m, () => ({ FitAddon: class {}, SearchAddon: class {}, Unicode11Addon: class {}, WebLinksAddon: class {}, WebglAddon: class {} }));
}
vi.mock("@tauri-apps/api/webview", () => ({ getCurrentWebview: () => ({ onDragDropEvent: async () => () => undefined }) }));

const { Signal } = await import("../store");
const { THEMES, themeFor } = await import("../themes");
const { parseQuick, parseSsh } = await import("../palette");
const { keyOf, terminalChord, foreignKey } = await import("../dom");
const { rtlRuns, withFallback } = await import("../terminal");

const key = (init: KeyboardEventInit) => new KeyboardEvent("keydown", init);

describe("Signal", () => {
  it("notifies subscribers and stops after unsubscribe", () => {
    const s = new Signal(1);
    const seen: number[] = [];
    const off = s.subscribe((v) => seen.push(v));
    s.set(2);
    s.update((v) => v * 10);
    off();
    s.set(99);
    expect(seen).toEqual([1, 2, 20]);
    expect(s.get()).toBe(99);
  });

  it("can skip the immediate call", () => {
    const s = new Signal("a");
    const fn = vi.fn();
    s.subscribe(fn, false);
    expect(fn).not.toHaveBeenCalled();
  });
});

describe("terminal themes", () => {
  const keys = ["black", "red", "green", "yellow", "blue", "magenta", "cyan", "white"];
  it("define the full 16-colour palette and unique ids", () => {
    expect(new Set(THEMES.map((t) => t.id)).size).toBe(THEMES.length);
    for (const t of THEMES) {
      for (const k of keys) {
        const cap = k[0]?.toUpperCase() + k.slice(1);
        expect(t.theme[k as keyof typeof t.theme], `${t.id}.${k}`).toMatch(/^#[0-9a-f]{6}$/i);
        expect(t.theme[`bright${cap}` as keyof typeof t.theme], `${t.id}.bright${cap}`).toMatch(/^#[0-9a-f]{6}$/i);
      }
      expect(t.theme.background).toMatch(/^#[0-9a-f]{6}$/i);
    }
  });

  it("falls back to the default theme", () => {
    expect(themeFor("nope").id).toBe("sodium");
    expect(themeFor(undefined).id).toBe("sodium");
    expect(themeFor("paper").id).toBe("paper");
  });
});

describe("quick connect parsing", () => {
  it("accepts user@host:port forms", () => {
    expect(parseQuick("ops@db.internal:2222")).toEqual({ user: "ops", host: "db.internal", port: 2222 });
    expect(parseQuick("10.0.0.5")).toEqual({ user: null, host: "10.0.0.5", port: 22 });
    expect(parseQuick("root@[fe80::1]:22")).toEqual({ user: "root", host: "fe80::1", port: 22 });
  });

  it("rejects ambiguous or invalid input", () => {
    expect(parseQuick("hosts")).toBeNull();
    expect(parseQuick("ops@db:99999")).toBeNull();
    expect(parseQuick("a b@c")).toBeNull();
    expect(parseQuick("")).toBeNull();
  });
});

describe("ssh command parsing", () => {
  it("understands pasted ssh invocations", () => {
    expect(parseSsh("ssh -p 2222 admin@10.0.0.5")).toEqual({ user: "admin", host: "10.0.0.5", port: 2222 });
    expect(parseSsh("ssh -i ~/.ssh/id_ed25519 -l deploy -p2200 build.internal uptime")).toEqual({ user: "deploy", host: "build.internal", port: 2200 });
    expect(parseSsh("ssh -A -J edge ops@db")).toEqual({ user: "ops", host: "db", port: 22 });
    expect(parseSsh("ssh ssh://root@[fe80::1]:2022")).toEqual({ user: "root", host: "fe80::1", port: 2022 });
    expect(parseSsh("ops@db.internal:2222")).toEqual({ user: "ops", host: "db.internal", port: 2222 });
  });

  it("rejects incomplete commands", () => {
    expect(parseSsh("ssh")).toBeNull();
    expect(parseSsh("ssh -p 70000 host.example")).toEqual({ user: null, host: "host.example", port: 22 });
    expect(parseSsh("ssh -p")).toBeNull();
  });
});

describe("keyboard layouts", () => {
  it("leaves Latin layouts alone", () => {
    expect(keyOf(key({ key: "k", code: "KeyK" }))).toBe("k");
    expect(keyOf(key({ key: "a", code: "KeyQ" }))).toBe("a");
    expect(foreignKey(key({ key: "Enter", code: "Enter" }))).toBe(false);
  });

  it("maps non-Latin characters back to the physical key", () => {
    expect(keyOf(key({ key: "ن", code: "KeyK" }))).toBe("k");
    expect(keyOf(key({ key: "ژ", code: "KeyC", shiftKey: true }))).toBe("C");
    expect(keyOf(key({ key: "؟", code: "Slash", shiftKey: true }))).toBe("?");
    expect(keyOf(key({ key: "۳", code: "Digit3" }))).toBe("3");
    expect(keyOf(key({ key: "п", code: "KeyG" }))).toBe("g");
  });

  it("produces terminal control sequences on non-Latin layouts", () => {
    expect(terminalChord(key({ key: "ز", code: "KeyC", ctrlKey: true }), true)).toBe("\x03");
    expect(terminalChord(key({ key: "ی", code: "KeyD", ctrlKey: true }), true)).toBe("\x04");
    expect(terminalChord(key({ key: "ج", code: "BracketLeft", ctrlKey: true }), true)).toBe("\x1b");
    expect(terminalChord(key({ key: "ذ", code: "KeyB", altKey: true }), true)).toBe("\x1bb");
    expect(terminalChord(key({ key: "c", code: "KeyC", ctrlKey: true }), true)).toBeNull();
    expect(terminalChord(key({ key: "ز", code: "KeyC", ctrlKey: true, altKey: true }), true)).toBeNull();
    expect(terminalChord(key({ key: "ز", code: "KeyC", ctrlKey: true, shiftKey: true }), true)).toBeNull();
  });
});

describe("right-to-left rendering", () => {
  it("joins runs of RTL script including inner spaces and punctuation", () => {
    const line = "echo سلام دنیا — hello";
    const runs = rtlRuns(line);
    expect(runs).toHaveLength(1);
    const [start, end] = runs[0] ?? [0, 0];
    expect(line.slice(start, end)).toBe("سلام دنیا");
    expect(rtlRuns("MARK-سلام-END").map(([a, b]) => "MARK-سلام-END".slice(a, b))).toEqual(["سلام"]);
    expect(rtlRuns("می‌خوام")).toEqual([[0, 7]]);
    expect(rtlRuns("plain ascii line")).toEqual([]);
    expect(rtlRuns("a ب c")).toEqual([]);
  });

  it("adds the bundled Persian fallback font once", () => {
    expect(withFallback('"JetBrains Mono", monospace')).toBe('"JetBrains Mono", "Vazirmatn Variable", monospace');
    expect(withFallback('"Fira Code"')).toBe('"Fira Code", "Vazirmatn Variable", monospace');
    const once = withFallback("Menlo");
    expect(withFallback(once)).toBe(once);
  });
});
