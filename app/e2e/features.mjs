import { execFileSync } from "node:child_process";
import { chmodSync, existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { connect } from "node:net";
import { join } from "node:path";
import { App } from "./driver.mjs";

const need = (name) => {
  const v = process.env[name];
  if (!v) throw new Error(`set ${name}`);
  return v;
};

const bin = need("CITADEL_BIN");
const out = join(process.env.CITADEL_E2E_OUT ?? join(process.cwd(), "e2e-artifacts"), "features");
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
const fakebin = join(out, "fakebin");
const opened = join(out, "opened.log");
mkdirSync(fakebin, { recursive: true });
for (const name of ["xdg-open", "gio", "wslview", "gnome-open", "kde-open", "sensible-browser", "powershell.exe", "cmd.exe"]) {
  const p = join(fakebin, name);
  writeFileSync(p, `#!/bin/sh\nprintf '%s %s\\n' "$*" "$OPEN_RS_TARGET" >> '${opened}'\n`);
  chmodSync(p, 0o755);
}
const appEnv = { PATH: `${fakebin}:${process.env.PATH}`, BROWSER: join(fakebin, "xdg-open") };
const pause = (ms) => new Promise((r) => setTimeout(r, ms));
const x = (cmd, args) => execFileSync(`${xbin}/${cmd}`, args, { env: tools.env }).toString();
const results = [];
const PASS = "e2e-passphrase-123";

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

function httpGet(port) {
  return new Promise((resolve, reject) => {
    const s = connect(port, "127.0.0.1");
    let buf = "";
    s.setTimeout(8000, () => {
      s.destroy();
      reject(new Error("http timeout"));
    });
    s.on("connect", () => s.write("GET / HTTP/1.0\r\nHost: localhost\r\n\r\n"));
    s.on("data", (d) => (buf += d.toString()));
    s.on("end", () => resolve(buf));
    s.on("error", reject);
  });
}

async function launch() {
  const app = await App.launch({ bin, home, tools, port: 9255, env: appEnv });
  const invoke = (cmd, args) => app.eval(`window.__TAURI_INTERNALS__.invoke(${JSON.stringify(cmd)}, ${JSON.stringify(args ?? {})})`);
  const focus = () => x("xdotool", ["windowfocus", "--sync", String(Number(app.windowId()))]);
  const keys = (...k) => x("xdotool", ["key", "--delay", "40", ...k]);
  const unlock = async () => {
    await app.waitFor("!!document.querySelector('.lock form')", 20000, "lock screen");
    await app.eval(`(() => { for (const i of document.querySelectorAll('.lock input')) { i.value = ${JSON.stringify(PASS)}; i.dispatchEvent(new Event('input', { bubbles: true })); } document.querySelector('.lock form').requestSubmit(); return true; })()`);
    await app.waitFor("!!document.querySelector('.shell') && !document.querySelector('.lock form')", 20000, "shell visible");
  };
  return { app, invoke, focus, keys, unlock, shot: (n) => app.screenshot(join(out, `${n}.png`)) };
}

const terms = (app) => app.eval("window.__citadel.terminals()");
const text = (app, key) => app.eval(`window.__citadel.text(${JSON.stringify(key)})`);
const type = (app, key, s) => app.eval(`window.__citadel.type(${JSON.stringify(key)}, ${JSON.stringify(s)})`);
const promptIn = (app, key) => app.waitFor(`/[$#] ?$/.test(window.__citadel.text(${JSON.stringify(key)}).trimEnd())`, 15000, "shell prompt");
const focusPane = (app, key) => app.eval(`(() => { const ta = [...document.querySelectorAll('.term-pane')].map((p) => p.querySelector('.xterm-helper-textarea')); const i = window.__citadel.terminals().indexOf(${JSON.stringify(key)}); ta[i]?.focus(); return true; })()`);

let s = await launch();
let hostIds = [];
try {
  await check("setup: vault, key and three hosts", async () => {
    await s.unlock();
    const id = await s.invoke("identity_import", { label: "e2e", pem: keyPem, passphrase: null });
    for (const [label, tags] of [
      ["web-1", ["web", "prod"]],
      ["web-2", ["web", "prod"]],
      ["db-1", ["db", "prod"]],
    ]) {
      const h = await s.invoke("host_save", {
        input: { id: null, label, hostname: "127.0.0.1", port: sshPort, user, auth: { kind: "identity", identity: id.id }, jump: [], workspace: "lab", tags, keepalive_secs: 15, agent_forward: false, legacy_crypto: false, ssh_alias: null },
      });
      hostIds.push(h.id);
    }
    await s.app.click('.rail-btn[data-page="keys"]');
    await s.app.click('.rail-btn[data-page="hosts"]');
    await s.app.waitFor("document.querySelectorAll('.fleet-row').length === 3", 10000, "three hosts");
  });

  await check("the global hotkey hides and shows the window", async () => {
    const st = await s.invoke("hotkey_status");
    if (!st.active) throw new Error(`hotkey inactive: ${JSON.stringify(st)}`);
    const wid = String(Number(s.app.windowId()));
    const mapped = () => /Map State: IsViewable/.test(x("xwininfo", ["-id", wid]));
    s.focus();
    await pause(200);
    x("xdotool", ["keydown", "Control_L", "sleep", "0.06", "keydown", "Alt_L", "sleep", "0.06", "key", "space", "sleep", "0.04", "keyup", "Alt_L", "keyup", "Control_L"]);
    for (let i = 0; i < 20 && mapped(); i++) await pause(150);
    if (mapped()) throw new Error("window still visible after the hotkey");
    s.keys("ctrl+alt+space");
    for (let i = 0; i < 20 && !mapped(); i++) await pause(150);
    if (!mapped()) throw new Error("window did not come back");
  });

  await check("fleet run fans out, groups identical output and flags failures", async () => {
    await s.app.click('.rail-btn[data-page="fleet"]');
    await s.app.waitFor("!!document.querySelector('.fleet-cmd')", 5000, "fleet page");
    await s.app.clickText(".fleet-chips .chip-btn", "#prod3");
    await s.app.waitFor("document.querySelector('.fleet-count')?.textContent.includes('3 selected')", 5000, "three selected");
    await s.app.fill(".fleet-cmd", "printf 'fleet-%s\\n' $((6*7))");
    await s.app.eval("(() => { [...document.querySelectorAll('.fleet-compose button')].find((b) => b.textContent.startsWith('run'))?.click(); return true; })()");
    await s.app.waitFor("document.body.innerText.includes('Host key') || document.querySelector('.modal-head h2')?.textContent === 'Verify host identity' || document.querySelector('.fleet-summary')?.textContent.includes('3 ok')", 30000, "fleet progress");
    for (let i = 0; i < 3; i++) {
      const prompt = await s.app.eval("document.querySelector('.modal-head h2')?.textContent ?? ''");
      if (prompt !== "Verify host identity") break;
      await s.app.clickText(".modal-actions button", "Trust and connect");
      await pause(400);
    }
    await s.app.waitFor("document.querySelector('.fleet-summary')?.textContent.includes('3 ok')", 40000, "three ok");
    const grouped = await s.app.eval("[...document.querySelectorAll('.fleet-card')].map((c) => c.innerText).join('|')");
    if (!/3 hosts/.test(grouped) || !grouped.includes("fleet-42")) throw new Error(`cards: ${grouped}`);
    await s.shot("01-fleet-ok");
    await s.app.fill(".fleet-cmd", "test \"$(hostname)\" = nowhere || exit 3");
    await s.app.eval("(() => { [...document.querySelectorAll('.fleet-compose button')].find((b) => b.textContent.startsWith('run'))?.click(); return true; })()");
    await s.app.waitFor("document.querySelector('.fleet-summary')?.textContent.includes('3 failed')", 30000, "three failed");
    const failed = await s.app.eval("document.querySelector('.fleet-card.st-fail')?.innerText ?? ''");
    if (!failed.includes("exit 3")) throw new Error(`failure card: ${failed}`);
    await s.shot("02-fleet-fail");
  });

  let first = "";
  await check("split panes with real keys, Ctrl+W stays in the shell", async () => {
    await s.app.click('.rail-btn[data-page="hosts"]');
    await s.app.clickText(".fleet-row button", "connect");
    await s.app.waitFor("window.__citadel.terminals().length === 1", 20000, "terminal");
    [first] = await terms(s.app);
    await promptIn(s.app, first);
    s.focus();
    await focusPane(s.app, first);
    await pause(150);
    await type(s.app, first, "echo keep these words");
    s.keys("ctrl+w");
    await pause(300);
    s.keys("Return");
    await s.app.waitFor(`window.__citadel.text(${JSON.stringify(first)}).split("\\n").some((l) => l.trimEnd() === "keep these")`, 5000, "word deleted by readline");
    if ((await s.app.eval("document.querySelectorAll('.tab').length")) !== 1) throw new Error("Ctrl+W closed the tab");
    s.keys("ctrl+shift+d");
    await s.app.waitFor("window.__citadel.terminals().length === 2 && document.querySelectorAll('.term-pane').length === 2", 5000, "second pane");
    const [, second] = await terms(s.app);
    await promptIn(s.app, second);
    await type(s.app, second, "echo pane-$((2+2))\r");
    await s.app.waitFor(`window.__citadel.text(${JSON.stringify(second)}).includes("pane-4")`, 5000, "second pane output");
    await pause(300);
    await s.shot("03-split");
    s.keys("ctrl+shift+w");
    await s.app.waitFor("document.querySelectorAll('.term-pane').length === 1", 5000, "pane closed");
    s.keys("ctrl+shift+e");
    await s.app.waitFor("document.querySelectorAll('.term-pane').length === 2 && !!document.querySelector('.split-col')", 5000, "split down");
  });

  await check("a new server port is detected and forwarded end to end", async () => {
    const port = 18000 + Math.floor(Math.random() * 1500);
    await focusPane(s.app, first);
    await type(s.app, first, `mkdir -p /tmp/citadel-e2e-www && echo hello-from-${port} > /tmp/citadel-e2e-www/index.html && (cd /tmp/citadel-e2e-www && python3 -m http.server ${port} --bind 127.0.0.1 >/dev/null 2>&1 &)\r`);
    await s.app.waitFor(`[...document.querySelectorAll('.toast-action')].some((t) => t.textContent.includes('port ${port}'))`, 40000, "new port toast");
    await s.shot("04-port-toast");
    await s.app.eval(`(() => { const t = [...document.querySelectorAll('.toast-action')].find((x) => x.textContent.includes('port ${port}')); [...t.querySelectorAll('button')].find((b) => b.textContent.startsWith('forward'))?.click(); return true; })()`);
    const local = await s.app.waitFor(
      `window.__TAURI_INTERNALS__.invoke('tunnels_list').then((l) => { const t = l.find((t) => t.forward.kind === 'local' && t.forward.dest.port === ${port} && t.running); return t ? t.forward.bind.port : 0; })`,
      15000,
      "forward running",
    );
    const body = await httpGet(local);
    if (!body.includes(`hello-from-${port}`)) throw new Error(`unexpected body ${body.slice(0, 120)}`);
    await s.app.waitFor(`true`, 1000);
    const log = existsSync(opened) ? readFileSync(opened, "utf8") : "";
    if (!log.includes(`http://127.0.0.1:${local}/`)) throw new Error(`browser not asked to open, log=${JSON.stringify(log)}`);
    await type(s.app, first, `pkill -f "http.server ${port}"\r`);
  });

  await check("a shortcut is rebound with the recorder and works", async () => {
    await s.app.click('.rail-btn[data-page="settings"]');
    await s.app.clickText(".set-nav button", "Shortcuts");
    await s.app.waitFor("!!document.querySelector('.sc-row')", 5000, "shortcut list");
    await s.app.eval("(() => { const row = [...document.querySelectorAll('.sc-row')].find((r) => r.querySelector('.sc-label')?.textContent === 'Go to audit'); row.querySelector('.sc-acts button').click(); return true; })()");
    await s.app.waitFor("!!document.querySelector('.rec-keys')", 5000, "recorder");
    s.focus();
    s.keys("ctrl+alt+9");
    await s.app.waitFor("document.querySelector('.rec-keys')?.textContent.includes('9')", 3000, "chord captured");
    await s.shot("05-recorder");
    await s.app.clickText(".modal-actions button", "Save");
    await s.app.waitFor("window.__TAURI_INTERNALS__.invoke('prefs_get').then((p) => (p.keybindings['go.audit'] ?? []).includes('Ctrl+Alt+9'))", 5000, "binding saved");
    await s.app.click('.rail-btn[data-page="hosts"]');
    await s.app.eval("(document.activeElement?.blur(), true)");
    s.keys("ctrl+alt+9");
    await s.app.waitFor("document.querySelector('.rail-btn.on')?.dataset.page === 'audit'", 3000, "audit opened by new chord");
  });

  await check("the font picker lists installed monospace fonts", async () => {
    await s.app.click('.rail-btn[data-page="settings"]');
    await s.app.clickText(".set-nav button", "Terminal");
    await s.app.click(".fpk-button");
    await s.app.waitFor("[...document.querySelectorAll('.fpk-item .fpk-label')].some((l) => l.textContent === 'DejaVu Sans Mono')", 10000, "system fonts listed");
    await s.shot("06-font-picker");
    await s.app.eval("(() => { [...document.querySelectorAll('.fpk-item')].find((b) => b.querySelector('.fpk-label').textContent === 'DejaVu Sans Mono').click(); return true; })()");
    await s.app.waitFor("window.__TAURI_INTERNALS__.invoke('prefs_get').then((p) => p.font_family.startsWith('\"DejaVu Sans Mono\"'))", 5000, "font saved");
  });

  await check("no console errors", async () => {
    if (s.app.errors.length) throw new Error(s.app.errors.join("\n"));
  });
} finally {
  await s.app.close();
}

s = await launch();
try {
  await check("tabs and split layout come back after a restart", async () => {
    await s.unlock();
    await s.app.waitFor("document.querySelectorAll('.term-pane').length === 2 && !!document.querySelector('.split-col')", 15000, "restored split");
    await s.app.eval("(document.querySelector('.tab')?.click(), true)");
    const [a] = await terms(s.app);
    await promptIn(s.app, a);
    await pause(400);
    await s.shot("07-restored");
  });
} finally {
  await s.app.close();
}

const failed = results.filter((r) => !r.ok);
writeFileSync(join(out, "results.json"), JSON.stringify(results, null, 2));
console.log(`\n${results.length - failed.length}/${results.length} passed`);
process.exit(failed.length ? 1 : 0);
