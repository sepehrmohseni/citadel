import { readFileSync, readdirSync, writeFileSync, mkdirSync } from "node:fs";
import { join } from "node:path";
import { App } from "./driver.mjs";

const need = (name) => {
  const v = process.env[name];
  if (!v) throw new Error(`set ${name}`);
  return v;
};

const bin = need("CITADEL_BIN");
const out = join(process.env.CITADEL_E2E_OUT ?? join(process.cwd(), "e2e-artifacts"), "perf");
const sshPort = Number(need("CITADEL_E2E_SSH_PORT"));
const keyPem = readFileSync(need("CITADEL_E2E_KEY"), "utf8");
const user = process.env.CITADEL_E2E_USER ?? process.env.USER;
const xbin = process.env.CITADEL_E2E_X11_BIN ?? "/usr/bin";
const libs = process.env.CITADEL_E2E_X11_LIBS;
const tools = {
  xwd: `${xbin}/xwd`,
  xwininfo: `${xbin}/xwininfo`,
  xprop: `${xbin}/xprop`,
  env: { ...process.env, ...(libs ? { LD_LIBRARY_PATH: libs } : {}) },
};
mkdirSync(out, { recursive: true });
const pause = (ms) => new Promise((r) => setTimeout(r, ms));
const HZ = 100;

function tree(root) {
  const kids = new Map();
  for (const d of readdirSync("/proc")) {
    if (!/^\d+$/.test(d)) continue;
    try {
      const stat = readFileSync(`/proc/${d}/stat`, "utf8");
      const ppid = Number(stat.slice(stat.lastIndexOf(")") + 2).split(" ")[1]);
      if (!kids.has(ppid)) kids.set(ppid, []);
      kids.get(ppid).push(Number(d));
    } catch {}
  }
  const all = [root];
  for (let i = 0; i < all.length; i++) all.push(...(kids.get(all[i]) ?? []));
  return all;
}

function ticks(pids) {
  let t = 0;
  for (const p of pids) {
    try {
      const f = readFileSync(`/proc/${p}/stat`, "utf8").slice(readFileSync(`/proc/${p}/stat`, "utf8").lastIndexOf(")") + 2).split(" ");
      t += Number(f[11]) + Number(f[12]);
    } catch {}
  }
  return t;
}

function pssMiB(pids) {
  let kb = 0;
  for (const p of pids) {
    try {
      const m = /^Pss:\s+(\d+)/m.exec(readFileSync(`/proc/${p}/smaps_rollup`, "utf8"));
      if (m) {
        kb += Number(m[1]);
        continue;
      }
    } catch {}
    try {
      const m = /^VmRSS:\s+(\d+)/m.exec(readFileSync(`/proc/${p}/status`, "utf8"));
      if (m) kb += Number(m[1]);
    } catch {}
  }
  return kb / 1024;
}

async function cpuOver(pid, ms) {
  const pids = tree(pid);
  const a = ticks(pids);
  const t0 = Date.now();
  await pause(ms);
  return ((ticks(tree(pid)) - a) / HZ / ((Date.now() - t0) / 1000)) * 100;
}

const app = await App.launch({ bin, home: join(out, `home-${Date.now()}`), tools, port: 9244 });
const invoke = (cmd, args) => app.eval(`window.__TAURI_INTERNALS__.invoke(${JSON.stringify(cmd)}, ${JSON.stringify(args)})`);
const report = {};
try {
  await app.waitFor("!!document.querySelector('.lock form')");
  if (process.env.CITADEL_PERF_PREFS) {
    const p = await invoke("prefs_get", {});
    await invoke("prefs_set", { prefs: { ...p, ...JSON.parse(process.env.CITADEL_PERF_PREFS) } });
  }
  await app.eval(`(() => { for (const i of document.querySelectorAll('.lock input')) { i.value = 'e2e-passphrase-123'; i.dispatchEvent(new Event('input', { bubbles: true })); } document.querySelector('.lock form').requestSubmit(); return true; })()`);
  await app.waitFor("!!document.querySelector('.shell') && !document.querySelector('.lock form')", 20000);
  const id = await invoke("identity_import", { label: "perf", pem: keyPem, passphrase: null });
  const base = { id: null, port: sshPort, user, auth: { kind: "identity", identity: id.id }, jump: [], workspace: null, tags: [], keepalive_secs: 15, agent_forward: false, legacy_crypto: false, ssh_alias: null };
  for (let i = 0; i < 40; i++) await invoke("host_save", { input: { ...base, label: `fleet-${i}`, hostname: "127.0.0.1" } });
  await app.click('.rail-btn[data-page="keys"]');
  await app.click('.rail-btn[data-page="hosts"]');
  await app.waitFor("document.querySelectorAll('.fleet-row').length >= 40", 20000);
  await pause(1500);
  report.idleNoTerminalCpu = await cpuOver(app.child.pid, 8000);
  report.idleNoTerminalPss = pssMiB(tree(app.child.pid));
  await app.clickText(".fleet-row button", "connect");
  await app.waitFor("document.querySelector('.modal-head h2')?.textContent === 'Verify host identity'", 20000);
  await app.clickText(".modal-actions button", "Trust and connect");
  await app.waitFor("window.__citadel && window.__citadel.terminals().length === 1", 20000);
  const term = await app.eval("window.__citadel.terminals()[0]");
  const text = () => app.eval(`window.__citadel.text(${JSON.stringify(term)})`);
  const type = (s) => app.eval(`window.__citadel.type(${JSON.stringify(term)}, ${JSON.stringify(s)})`);
  await app.waitFor(`/[$#] ?$/.test(window.__citadel.text(${JSON.stringify(term)}).trimEnd())`, 20000);
  await pause(12000);
  if (process.env.CITADEL_PERF_BREAKDOWN) {
    console.log("    animations:", JSON.stringify(await app.eval("document.getAnimations().map((a) => (a.effect?.target?.className?.baseVal ?? a.effect?.target?.className ?? '?') + ':' + (a.animationName ?? a.constructor.name))")));
    console.log("    frames/2s:", await app.eval("new Promise((r) => { let n = 0; const t0 = performance.now(); const f = () => { n++; if (performance.now() - t0 < 2000) requestAnimationFrame(f); else r(n); }; requestAnimationFrame(f); })"));
    const before = new Map(tree(app.child.pid).map((p) => [p, ticks([p])]));
    await pause(8000);
    for (const [p, t] of before) {
      let comm = "?";
      try {
        comm = readFileSync(`/proc/${p}/comm`, "utf8").trim();
      } catch {}
      const d = ticks([p]) - t;
      if (d > 0) console.log(`    ${comm.padEnd(20)} ${((d / HZ / 8) * 100).toFixed(1)}%  pss ${pssMiB([p]).toFixed(1)} MiB`);
    }
    for (const p of tree(app.child.pid)) {
      let comm = "?";
      try {
        comm = readFileSync(`/proc/${p}/comm`, "utf8").trim();
      } catch {}
      console.log(`    mem ${comm.padEnd(20)} pss ${pssMiB([p]).toFixed(1)} MiB`);
    }
  }
  report.idleTerminalCpu = await cpuOver(app.child.pid, 8000);
  report.idleTerminalPss = pssMiB(tree(app.child.pid));
  if (process.env.CITADEL_PERF_IDLE_ONLY) throw new Error("idle-only");

  await app.eval("(() => { window.__lag = { max: 0 }; let last = performance.now(); setInterval(() => { const n = performance.now(); window.__lag.max = Math.max(window.__lag.max, n - last - 50); last = n; }, 50); return true; })()");
  const pids = tree(app.child.pid);
  const t0 = Date.now();
  const c0 = ticks(pids);
  let peak = 0;
  await type("seq 1 1500000; echo flood-done-$((40+2))\r");
  for (;;) {
    peak = Math.max(peak, pssMiB(tree(app.child.pid)));
    if ((await text()).includes("flood-done-42")) break;
    if (Date.now() - t0 > 180000) throw new Error("flood did not finish");
    await pause(400);
  }
  const secs = (Date.now() - t0) / 1000;
  report.floodSeconds = secs;
  report.floodCpu = ((ticks(tree(app.child.pid)) - c0) / HZ / secs) * 100;
  report.floodPeakPss = peak;
  report.floodMaxUiStallMs = await app.eval("Math.round(window.__lag.max)");

  await app.eval("(window.__lag.max = 0, true)");
  await type("clear; yes citadel-flood\r");
  await pause(1500);
  const s0 = Date.now();
  await type("\x03");
  try {
    await app.waitFor(`/[$#] ?$/.test(window.__citadel.text(${JSON.stringify(term)}).trimEnd())`, 60000, "prompt after ^C");
    report.ctrlCToPromptMs = Date.now() - s0;
  } catch {
    report.ctrlCToPromptMs = "over 60000";
  }
  report.yesMaxUiStallMs = await app.eval("Math.round(window.__lag.max)").catch(() => "unresponsive");
  await pause(2000);
  report.afterFloodPss = pssMiB(tree(app.child.pid));
  report.renderer = await app.eval(`window.__citadel.renderer ? window.__citadel.renderer(${JSON.stringify(term)}) : "unknown"`).catch(() => "unknown");
} catch (e) {
  if (String(e?.message) !== "idle-only") throw e;
} finally {
  await app.close();
}
for (const [k, v] of Object.entries(report)) console.log(`  ${k.padEnd(22)} ${typeof v === "number" ? v.toFixed(1) : v}`);
writeFileSync(join(out, `perf-${Date.now()}.json`), JSON.stringify(report, null, 2));
