import { execFileSync } from "node:child_process";
import { mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { App } from "./driver.mjs";

const need = (name) => {
  const v = process.env[name];
  if (!v) throw new Error(`set ${name}`);
  return v;
};

const bin = need("CITADEL_BIN");
const display = need("DISPLAY");
const out = join(process.env.CITADEL_E2E_OUT ?? join(process.cwd(), "e2e-artifacts"), "intl");
const sshPort = Number(need("CITADEL_E2E_SSH_PORT"));
const keyPem = readFileSync(need("CITADEL_E2E_KEY"), "utf8");
const user = process.env.CITADEL_E2E_USER ?? process.env.USER;
const libs = process.env.CITADEL_E2E_X11_LIBS;
const xbin = process.env.CITADEL_E2E_X11_BIN ?? "/usr/bin";
const tools = {
  xwd: `${xbin}/xwd`,
  xwininfo: `${xbin}/xwininfo`,
  xprop: `${xbin}/xprop`,
  env: { ...process.env, ...(libs ? { LD_LIBRARY_PATH: libs } : {}) },
};
mkdirSync(out, { recursive: true });
const home = join(out, `home-${Date.now()}`);
const pause = (ms) => new Promise((r) => setTimeout(r, ms));
const x = (cmd, args) => execFileSync(`${xbin}/${cmd}`, args, { env: tools.env }).toString();

const KC = { c: 54, v: 55, k: 45, j: 44, g: 42, h: 43, slash: 61, insert: 118 };
const WORD = ["Arabic_seen", "Arabic_lam", "Arabic_alef", "Arabic_meem"];
const SALAM = "سلام";
const MIXED = "می‌خوام";

function layout(name) {
  const file = join(out, `${name}.xkb`);
  writeFileSync(file, x("setxkbmap", ["-print", "-layout", name]));
  x("xkbcomp", ["-w0", file, display]);
}

const keys = (...names) => x("xdotool", ["key", "--delay", "35", ...names]);

function chord(mods, code) {
  const args = [];
  for (const m of mods) args.push("keydown", m);
  args.push("key", String(code));
  for (const m of [...mods].reverse()) args.push("keyup", m);
  x("xdotool", args);
}

const results = [];
async function check(name, fn) {
  const t = Date.now();
  try {
    await fn();
    results.push({ name, ok: true });
    console.log(`  ok   ${name} (${Date.now() - t}ms)`);
  } catch (e) {
    results.push({ name, ok: false, error: String(e) });
    console.log(`  FAIL ${name}: ${e}`);
  }
}

const app = await App.launch({ bin, home, tools, port: 9233 });
const shot = (name) => app.screenshot(join(out, `${name}.png`));
const invoke = (cmd, args) => app.eval(`window.__TAURI_INTERNALS__.invoke(${JSON.stringify(cmd)}, ${JSON.stringify(args)})`);
let term = "";
const text = () => app.eval(`window.__citadel.text(${JSON.stringify(term)})`);
const type = (s) => app.eval(`window.__citadel.type(${JSON.stringify(term)}, ${JSON.stringify(s)})`);
const lines = async () => (await text()).split("\n").map((l) => l.trimEnd());
const prompt = () => app.waitFor(`/[$#] ?$/.test(window.__citadel.text(${JSON.stringify(term)}).trimEnd())`, 10000, "shell prompt");

async function focusTerminal() {
  const wid = app.windowId();
  x("xdotool", ["windowfocus", "--sync", String(Number(wid))]);
  await app.eval("(() => { document.querySelector('.term-pane:not([hidden]) .xterm-helper-textarea, .xterm-helper-textarea')?.focus(); return true; })()");
  await pause(150);
}

async function pointAt(row, col) {
  const wid = String(Number(app.windowId()));
  const geo = x("xwininfo", ["-id", wid]);
  const ox = Number(/Absolute upper-left X:\s+(-?\d+)/.exec(geo)?.[1] ?? 0);
  const oy = Number(/Absolute upper-left Y:\s+(-?\d+)/.exec(geo)?.[1] ?? 0);
  await app.eval("(() => { window.__probe = null; addEventListener('mousemove', (e) => { window.__probe = [e.clientX, e.clientY]; }, { once: true }); return true; })()");
  x("xdotool", ["mousemove", String(ox + 200), String(oy + 200)]);
  const probe = await app.waitFor("window.__probe", 3000, "pointer calibration");
  const [dx, dy] = [200 - probe[0], 200 - probe[1]];
  const size = (await lines()).find((l) => /^\d+ \d+$/.test(l));
  const [rows, cols] = (size ?? "24 80").split(" ").map(Number);
  const r = await app.eval("(() => { const b = document.querySelector('.term-pane:not([hidden]) .xterm-screen').getBoundingClientRect(); return [b.left, b.top, b.width, b.height]; })()");
  const cw = r[2] / cols;
  const ch = r[3] / rows;
  return [Math.round(ox + dx + r[0] + (col + 0.5) * cw), Math.round(oy + dy + r[1] + (row + 0.5) * ch)];
}

try {
  await check("setup: vault, key, host, terminal", async () => {
    await app.waitFor("!!document.querySelector('.lock form')");
    await app.eval(`(() => { for (const i of document.querySelectorAll('.lock input')) { i.value = 'e2e-passphrase-123'; i.dispatchEvent(new Event('input', { bubbles: true })); } document.querySelector('.lock form').requestSubmit(); return true; })()`);
    await app.waitFor("!!document.querySelector('.shell') && !document.querySelector('.lock form')", 20000, "shell visible");
    const id = await invoke("identity_import", { label: "e2e", pem: keyPem, passphrase: null });
    await invoke("host_save", {
      input: { id: null, label: "intl", hostname: "127.0.0.1", port: sshPort, user, auth: { kind: "identity", identity: id.id }, jump: [], workspace: null, tags: [], keepalive_secs: 15, agent_forward: false, ssh_alias: null },
    });
    await app.click('.rail-btn[data-page="keys"]');
    await app.click('.rail-btn[data-page="hosts"]');
    await app.waitFor("!!document.querySelector('.fleet-row')", 20000, "host listed");
    await app.clickText(".fleet-row button", "connect");
    await app.waitFor("document.querySelector('.modal-head h2')?.textContent === 'Verify host identity'", 20000, "host key prompt");
    await app.clickText(".modal-actions button", "Trust and connect");
    await app.waitFor("window.__citadel && window.__citadel.terminals().length === 1", 20000, "terminal registered");
    term = await app.eval("window.__citadel.terminals()[0]");
    await prompt();
    await type("stty size; locale | grep -m1 CTYPE\r");
    await app.waitFor(`/^\\d+ \\d+$/m.test(window.__citadel.text(${JSON.stringify(term)}))`, 8000, "stty size");
  });

  await check("remote shell receives a UTF-8 locale", async () => {
    const l = await lines();
    const ctype = l.find((s) => s.startsWith("LC_CTYPE="));
    if (!ctype || !/utf-?8/i.test(ctype)) throw new Error(`remote ${ctype ?? "LC_CTYPE unset"}`);
  });

  layout("ir");

  await check("type Persian on a Persian layout", async () => {
    await type("clear; echo '");
    await focusTerminal();
    keys(...WORD);
    await type("'\r");
    await app.waitFor(`window.__citadel.text(${JSON.stringify(term)}).split("\\n").some((l) => l.trimEnd() === ${JSON.stringify(SALAM)})`, 5000, "echoed Persian word");
  });

  await check("backspace removes one whole Persian letter", async () => {
    await type("echo '");
    await focusTerminal();
    keys(...WORD, "Arabic_beh", "BackSpace");
    await type("'\r");
    await app.waitFor(`window.__citadel.text(${JSON.stringify(term)}).split("\\n").filter((l) => l.trimEnd() === ${JSON.stringify(SALAM)}).length >= 2`, 5000, "second clean echo");
  });

  await check("ZWNJ and Farsi yeh survive the round trip", async () => {
    await type("echo '");
    await focusTerminal();
    keys("Arabic_meem", "Farsi_yeh", "shift+space", "Arabic_khah", "Arabic_waw", "Arabic_alef", "Arabic_meem");
    await type("'\r");
    await app.waitFor(`window.__citadel.text(${JSON.stringify(term)}).split("\\n").some((l) => l.trimEnd() === ${JSON.stringify(MIXED)})`, 5000, "echoed word with ZWNJ");
  });

  await check("Ctrl+C interrupts on a Persian layout", async () => {
    await type("sleep 30; echo slept-through\r");
    await pause(600);
    await focusTerminal();
    chord(["Control_L"], KC.c);
    await app.waitFor(`window.__citadel.text(${JSON.stringify(term)}).includes("^C")`, 4000, "^C echoed");
    await prompt();
  });

  await check("Ctrl+Shift+C copies the selection on a Persian layout", async () => {
    await type(`clear; stty size; echo MARK-${SALAM}-END\r`);
    await app.waitFor(`window.__citadel.text(${JSON.stringify(term)}).split("\\n").some((l) => l.trimEnd() === "MARK-${SALAM}-END")`, 5000, "marker printed");
    const l = await lines();
    const row = l.findIndex((s) => s === `MARK-${SALAM}-END`);
    const [px, py] = await pointAt(row, 1);
    x("xdotool", ["mousemove", String(px), String(py), "click", "--repeat", "2", "--delay", "60", "1"]);
    await pause(250);
    await focusTerminal();
    chord(["Control_L", "Shift_L"], KC.c);
    await app.waitFor("document.body.innerText.includes('Copied')", 3000, "copy confirmation");
  });

  await check("Ctrl+Shift+V pastes Persian text on a Persian layout", async () => {
    await type("echo pasted:");
    await focusTerminal();
    chord(["Control_L", "Shift_L"], KC.v);
    await pause(400);
    await type("\r");
    await app.waitFor(`window.__citadel.text(${JSON.stringify(term)}).split("\\n").some((l) => l.trimEnd() === "pasted:MARK-${SALAM}-END")`, 5000, "pasted line echoed");
  });

  await check("Shift+Insert pastes too", async () => {
    await type("echo ins:");
    await focusTerminal();
    chord(["Shift_L"], KC.insert);
    await pause(400);
    await type("\r");
    await app.waitFor(`window.__citadel.text(${JSON.stringify(term)}).split("\\n").some((l) => l.trimEnd() === "ins:MARK-${SALAM}-END")`, 5000, "shift+insert paste echoed");
  });

  await check("terminal renders Persian runs", async () => {
    await type(`clear; printf '%s\\n' '${SALAM} دنیا — hello ۱۲۳' 'ls -la /srv/پروژه' '${MIXED}'\r`);
    await pause(700);
    await shot("01-terminal-persian");
  });

  await check("Ctrl+K opens the palette on a Persian layout", async () => {
    await app.eval("(() => { document.activeElement?.blur(); document.body.focus(); return true; })()");
    x("xdotool", ["windowfocus", "--sync", String(Number(app.windowId()))]);
    chord(["Control_L"], KC.k);
    await app.waitFor("!!document.querySelector('.palette')", 3000, "palette open");
    keys("Escape");
    await app.waitFor("!document.querySelector('.palette')", 3000, "palette closed");
  });

  await check("g h navigation works on a Persian layout", async () => {
    await app.click('.rail-btn[data-page="keys"]');
    await app.eval("(() => { document.activeElement?.blur(); return true; })()");
    keys(String(KC.g), String(KC.h));
    await app.waitFor("document.querySelector('.rail-btn[data-page=\"hosts\"]')?.classList.contains('on') || document.querySelector('.rail-btn.on')?.dataset.page === 'hosts'", 3000, "hosts page active");
  });

  await check("form inputs accept Persian and lay it out right-to-left", async () => {
    await app.clickText("button", "new host");
    await app.waitFor("!!document.querySelector('.modal input')", 5000, "host form");
    await app.eval("(() => { const i = document.querySelector('.modal input'); i.focus(); i.select(); return true; })()");
    keys(...WORD);
    const v = await app.eval("document.querySelector('.modal input').value");
    if (v !== SALAM) throw new Error(`input value ${JSON.stringify(v)}`);
    const dir = await app.eval("getComputedStyle(document.querySelector('.modal input')).direction");
    await shot("02-form-persian");
    keys("Escape");
    if (dir !== "rtl") throw new Error(`direction ${dir}`);
  });

  await check("GPU renderer joins Persian runs", async () => {
    const gl = await app.eval("(() => { const g = document.createElement('canvas').getContext('webgl2'); if (!g) return null; const i = g.getExtension('WEBGL_debug_renderer_info'); return String(i ? g.getParameter(i.UNMASKED_RENDERER_WEBGL) : g.getParameter(g.RENDERER)); })()");
    console.log(`       webgl: ${gl ?? "unavailable"}, first terminal used ${await app.eval(`window.__citadel.renderer(${JSON.stringify(term)})`)}`);
    if (!gl) return;
    const p = await invoke("prefs_get", {});
    await invoke("prefs_set", { prefs: { ...p, renderer: "webgl" } });
    await app.clickText(".fleet-row button", "connect");
    await app.waitFor("window.__citadel.terminals().length === 2", 20000, "second terminal");
    term = await app.eval("window.__citadel.terminals()[1]");
    await prompt();
    const r = await app.eval(`window.__citadel.renderer(${JSON.stringify(term)})`);
    if (r !== "webgl") throw new Error(`renderer ${r}`);
    await type(`clear; printf '%s\\n' '${SALAM} دنیا — hello ۱۲۳' 'ls -la /srv/پروژه' '${MIXED}'\r`);
    await pause(700);
    await shot("03-terminal-persian-webgl");
    await invoke("prefs_set", { prefs: p });
  });

  await check("no console errors", async () => {
    if (app.errors.length) throw new Error(app.errors.join("\n"));
  });
} finally {
  layout("us");
  await app.close();
}

const failed = results.filter((r) => !r.ok);
writeFileSync(join(out, "results.json"), JSON.stringify(results, null, 2));
console.log(`\n${results.length - failed.length}/${results.length} passed`);
process.exit(failed.length ? 1 : 0);
