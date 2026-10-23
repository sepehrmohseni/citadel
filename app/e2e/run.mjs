import { execFileSync, spawnSync } from "node:child_process";
import { cpSync, mkdirSync, readFileSync } from "node:fs";
import { connect } from "node:net";
import { join } from "node:path";
import { App } from "./driver.mjs";

const need = (name) => {
  const v = process.env[name];
  if (!v) throw new Error(`set ${name}`);
  return v;
};

const bin = need("CITADEL_BIN");
const out = process.env.CITADEL_E2E_OUT ?? join(process.cwd(), "e2e-artifacts");
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
const home = join(out, `home-${Date.now()}`);
const pause = (ms) => new Promise((r) => setTimeout(r, ms));
const results = [];

async function step(name, fn) {
  const t = Date.now();
  try {
    await fn();
    results.push({ name, ok: true, ms: Date.now() - t });
    console.log(`  ok   ${name} (${Date.now() - t}ms)`);
  } catch (e) {
    results.push({ name, ok: false, error: String(e) });
    console.log(`  FAIL ${name}: ${e}`);
    throw e;
  }
}

function banner(port) {
  return new Promise((resolve, reject) => {
    const s = connect(port, "127.0.0.1");
    let buf = "";
    s.setTimeout(8000, () => {
      s.destroy();
      reject(new Error("tunnel banner timeout"));
    });
    s.on("data", (d) => {
      buf += d.toString();
      if (buf.includes("\n")) {
        s.destroy();
        resolve(buf.trim());
      }
    });
    s.on("error", reject);
  });
}

const app = await App.launch({ bin, home, tools });
const shot = (name) => app.screenshot(join(out, `${name}.png`));
const invoke = (cmd, args) => app.eval(`window.__TAURI_INTERNALS__.invoke(${JSON.stringify(cmd)}, ${JSON.stringify(args)})`);

try {
  await step("create vault from the lock screen", async () => {
    await app.waitFor("!!document.querySelector('.lock form')");
    await shot("01-lock-create");
    await app.eval(`(() => { for (const i of document.querySelectorAll('.lock input')) { i.value = 'e2e-passphrase-123'; i.dispatchEvent(new Event('input', { bubbles: true })); } document.querySelector('.lock form').requestSubmit(); return true; })()`);
    await app.waitFor("!!document.querySelector('.shell') && !document.querySelector('.lock form')", 20000, "shell visible");
    await shot("02-hosts-empty");
  });

  await step("import an ssh key through the keys page", async () => {
    await app.click('.rail-btn[data-page="keys"]');
    await app.clickText("button", "Import key");
    await app.fill(".modal input", "e2e-key");
    await app.fill(".modal textarea", keyPem);
    await shot("03-key-import");
    await app.clickText(".modal-actions button", "Import");
    await app.waitFor("document.body.innerText.includes('SHA256:')", 10000, "fingerprint listed");
    await shot("04-keys");
  });

  await step("add a host with the editor and connect", async () => {
    await app.click('.rail-btn[data-page="hosts"]');
    await app.clickText("button", "new host");
    await app.fill('.modal input[placeholder="prod-db-1"]', "e2e-sshd");
    await app.fill('.modal input[placeholder="production"]', "lab");
    await app.fill('.modal input[placeholder="10.0.4.12 or db.internal"]', "127.0.0.1");
    await app.fill('.modal input[type="number"]', String(sshPort));
    await app.fill('.modal input[placeholder="root"]', user);
    await shot("05-host-form");
    await app.clickText(".modal-actions button", "Save and connect");
  });

  await step("verify the host key prompt and trust it", async () => {
    await app.waitFor("document.querySelector('.modal-head h2')?.textContent === 'Verify host identity'", 20000, "host key prompt");
    await shot("06-hostkey-prompt");
    await app.clickText(".modal-actions button", "Trust and connect");
  });

  await step("run a command in the terminal", async () => {
    await app.waitFor("window.__citadel && window.__citadel.terminals().length === 1", 20000, "terminal registered");
    const key = await app.eval("window.__citadel.terminals()[0]");
    await app.waitFor(`/[$#] ?$/m.test(window.__citadel.text(${JSON.stringify(key)}).trimEnd())`, 20000, "shell prompt");
    await app.eval(`window.__citadel.type(${JSON.stringify(key)}, "echo citadel-$((6*7)) && uname -s\\r")`);
    await app.waitFor(`window.__citadel.text(${JSON.stringify(key)}).includes("citadel-42")`, 10000, "command output");
    await pause(400);
    await shot("07-terminal");
  });

  await step("open live telemetry", async () => {
    await app.click('.term-bar button[title^="Host telemetry"]');
    await app.waitFor("!!document.querySelector('.telemetry .tm-head .host')", 20000, "telemetry sample");
    await pause(2600);
    await shot("08-telemetry");
    await app.click('.term-bar button[title^="Host telemetry"]');
  });

  await step("browse files over sftp", async () => {
    await app.click('.term-bar button[title^="Open files"]');
    await app.waitFor("document.querySelectorAll('.files-pane [role=row], .files-pane .fp-row').length > 1 || document.querySelector('.files-pane')?.innerText.includes('.ssh') === true", 20000, "directory listing");
    await pause(300);
    await shot("09-files");
  });

  await step("create and start a local tunnel, then use it", async () => {
    const hostId = await app.eval("window.__TAURI_INTERNALS__.invoke('hosts_list').then(h => h[0].id)");
    const port = 20000 + Math.floor(Math.random() * 20000);
    const t = await invoke("tunnel_save", {
      input: { id: null, label: "sshd-loop", host: hostId, autostart: false, forward: { kind: "local", bind: { host: "127.0.0.1", port }, dest: { host: "127.0.0.1", port: sshPort } } },
    });
    await invoke("tunnel_start", { id: t.id });
    const line = await banner(port);
    if (!line.startsWith("SSH-2.0-")) throw new Error(`unexpected banner ${line}`);
    await app.click('.rail-btn[data-page="tunnels"]');
    await pause(1600);
    await shot("10-tunnels-graph");
  });

  await step("fleet table shows the live link and latency", async () => {
    await app.click('.rail-btn[data-page="hosts"]');
    await app.waitFor("document.querySelector('.fleet-row .state-glyph.up') !== null", 10000, "link up glyph");
    await app.waitFor("/\\d+ms/.test(document.querySelector('.fleet-row .rtt')?.textContent ?? '')", 15000, "rtt measured");
    await shot("11-hosts-fleet");
  });

  await step("security audit grades the vault", async () => {
    await app.click('.rail-btn[data-page="audit"]');
    await app.waitFor("!!document.querySelector('.au-grade')", 10000, "grade rendered");
    await shot("12-audit");
  });

  await step("command palette and keyboard sheet", async () => {
    await app.key(null, "k", { ctrl: true });
    await app.waitFor("!!document.querySelector('.palette')", 5000, "palette open");
    await app.fill(".palette input", "e2e");
    await pause(200);
    await shot("13-palette");
    await app.key(".palette input", "Escape");
    await pause(300);
    await app.eval("(document.activeElement?.blur(), true)");
    await app.key(null, "?");
    await app.waitFor("document.querySelector('.modal-head h2')?.textContent === 'Keyboard'", 5000, "help sheet");
    await shot("14-help");
    await app.key(".modal", "Escape");
  });

  await step("switch the terminal theme", async () => {
    await app.click('.rail-btn[data-page="settings"]');
    await app.clickText(".set-nav button", "Terminal");
    await app.waitFor("document.querySelectorAll('.theme-tile').length === 6", 8000, "theme tiles");
    await shot("15-settings-terminal");
    await app.eval("([...document.querySelectorAll('.theme-tile')].find(t => t.textContent.includes('Phosphor')).click(), true)");
    await pause(1200);
    await app.eval("(document.querySelector('.tab').click(), true)");
    await pause(600);
    await shot("16-terminal-phosphor");
  });

  await step("lock and unlock keeps sessions alive", async () => {
    await invoke("vault_lock", {});
    await app.waitFor("!!document.querySelector('.lock form')", 8000, "lock screen");
    await shot("17-lock-unlock");
    await app.eval(`(() => { const i = document.querySelector('.lock input'); i.value = 'e2e-passphrase-123'; i.dispatchEvent(new Event('input', { bubbles: true })); document.querySelector('.lock form').requestSubmit(); return true; })()`);
    await app.waitFor("!document.querySelector('.lock form')", 20000, "unlocked");
    const key = await app.eval("window.__citadel.terminals()[0]");
    await app.eval(`window.__citadel.type(${JSON.stringify(key)}, "echo still-$((40+2))\\r")`);
    await app.waitFor(`window.__citadel.text(${JSON.stringify(key)}).includes("still-42")`, 10000, "session survived lock");
  });

  await step("an encrypted snapshot exists after unlock", async () => {
    await app.waitFor("window.__TAURI_INTERNALS__.invoke('backups_list').then((b) => b.length >= 1)", 15000, "snapshot written");
  });

  await step("no console errors", async () => {
    if (app.errors.length) throw new Error(app.errors.join("\n"));
  });

  const pidFile = process.env.CITADEL_E2E_SSHD_PIDFILE;
  const restart = process.env.CITADEL_E2E_SSHD_START;
  if (pidFile && restart) {
    await step("a dropped server is waited for and the shell comes back by itself", async () => {
      const key = JSON.stringify(await app.eval("window.__citadel.terminals()[0]"));
      const tree = [readFileSync(pidFile, "utf8").trim()];
      for (let i = 0; i < tree.length; i++) {
        const kids = spawnSync("pgrep", ["-P", tree[i]], { encoding: "utf8" }).stdout.split(/\s+/).filter(Boolean);
        tree.push(...kids);
      }
      spawnSync("kill", tree);
      await app.waitFor("[...document.querySelectorAll('.term-overlay')].some((o) => !o.hidden)", 30000, "session closed overlay");
      await app.eval("([...document.querySelectorAll('.term-overlay:not([hidden]) button')].find((b) => /reconnect/i.test(b.textContent))?.click(), true)");
      await app.waitFor("document.querySelector('.term-overlay:not([hidden])')?.innerText.includes('Waiting for') === true", 70000, "waiting overlay");
      const detail = await app.eval("document.querySelector('.term-overlay:not([hidden]) .err-detail')?.textContent ?? ''");
      if (!detail || /[{}]|attempt:/.test(detail)) throw new Error(`unexpected waiting detail: ${JSON.stringify(detail)}`);
      await shot("18-waiting-for-server");
      execFileSync("sh", ["-c", restart]);
      await app.waitFor("[...document.querySelectorAll('.term-overlay')].every((o) => o.hidden)", 150000, "shell reopened without a click");
      await app.eval(`window.__citadel.type(${key}, "echo back-$((40+2))\\r")`);
      await app.waitFor(`window.__citadel.text(${key}).includes("back-42")`, 15000, "shell works after the server returns");
    });
  }
} finally {
  await app.close();
}

const survivor = join(out, `home-restore-${Date.now()}`);
const restored = await (async () => {
  const dataDir = (h) => join(h, ".local/share/io.github.sepehrmohseni.citadel");
  mkdirSync(join(dataDir(survivor), "backups"), { recursive: true });
  cpSync(join(dataDir(home), "backups"), join(dataDir(survivor), "backups"), { recursive: true, filter: (src) => !src.endsWith(".redb") });
  return App.launch({ bin, home: survivor, tools, port: 9223 });
})();
try {
  await step("a lost vault is restored from its backups on the lock screen", async () => {
    await restored.waitFor("!!document.querySelector('.lock-restore:not([hidden]) button')", 15000, "restore offer");
    await restored.screenshot(join(out, "18-restore-offer.png"));
    await restored.clickText(".lock-restore button", "restore newest");
    await restored.eval(`(() => { const i = document.querySelector('.lock input'); i.value = 'e2e-passphrase-123'; i.dispatchEvent(new Event('input', { bubbles: true })); document.querySelector('.lock form').requestSubmit(); return true; })()`);
    await restored.waitFor("!!document.querySelector('.shell') && !document.querySelector('.lock form')", 20000, "restored vault open");
    await restored.waitFor("[...document.querySelectorAll('.fleet-row .name')].some((n) => n.textContent === 'e2e-sshd')", 15000, "host restored");
    await restored.screenshot(join(out, "19-restored-hosts.png"));
    if (restored.errors.length) throw new Error(restored.errors.join("\n"));
  });
} finally {
  await restored.close();
  const failed = results.filter((r) => !r.ok).length;
  console.log(`\n${results.length - failed}/${results.length} steps passed; screenshots in ${out}`);
  process.exitCode = failed ? 1 : 0;
}
