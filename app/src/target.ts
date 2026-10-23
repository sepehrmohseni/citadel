export interface Target {
  user: string | null;
  host: string;
  port: number;
}

const QUICK = /^(?:([A-Za-z0-9._-]+)@)?([A-Za-z0-9.-]+|\[[0-9a-fA-F:]+\])(?::(\d{1,5}))?$/;
const WITH_ARG = new Set("BbcDEeFIiJLlmOoPpQRSWw".split(""));

function port(raw: string | undefined, fallback = 22): number | null {
  const p = raw ? Number(raw) : fallback;
  return Number.isInteger(p) && p >= 1 && p <= 65535 ? p : null;
}

export function parseQuick(q: string): Target | null {
  const t = q.trim();
  const m = QUICK.exec(t);
  if (!m || (!t.includes(".") && !t.includes("@") && !t.includes(":"))) return null;
  const [, user, host, rawPort] = m;
  if (!host) return null;
  const p = port(rawPort);
  if (p === null) return null;
  return { user: user ?? null, host: host.replace(/^\[|\]$/g, ""), port: p };
}

export function parseSsh(cmd: string): Target | null {
  const words = cmd.trim().split(/\s+/).filter(Boolean);
  if (words[0] !== "ssh") return parseQuick(cmd);
  let user: string | null = null;
  let p: number | null = null;
  let dest: string | null = null;
  for (let i = 1; i < words.length && dest === null; i++) {
    const w = words[i] ?? "";
    if (w.startsWith("-") && w.length > 1) {
      const flag = w[1] ?? "";
      if (!WITH_ARG.has(flag)) continue;
      const value = w.length > 2 ? w.slice(2) : words[++i];
      if (flag === "p") p = port(value);
      if (flag === "l" && value) user = value;
      continue;
    }
    dest = w;
  }
  if (!dest) return null;
  const url = /^ssh:\/\/(?:([^@/]+)@)?([^/:]+|\[[^\]]+\])(?::(\d+))?\/?$/.exec(dest);
  const target = url ? { user: url[1] ?? null, host: (url[2] ?? "").replace(/^\[|\]$/g, ""), port: port(url[3]) ?? 22 } : parseQuick(dest.includes(".") || dest.includes("@") || dest.includes(":") ? dest : `${dest}:22`);
  if (!target || !target.host) return null;
  return { user: user ?? target.user, host: target.host, port: p ?? target.port };
}
