import { getCurrentWebview } from "@tauri-apps/api/webview";
import { api, Channel, errText, type DirEntry, type Id, type Progress, type Picked } from "../api";
import { bytes, clear, h, icon, isMac, keyOf, rate as fmtRate } from "../dom";
import { retitleTab, type Tab } from "../shell";
import { hostById } from "../store";
import { askText, btn, confirmBox, copyText, fail, input, toast } from "../ui";
import "./files.css";

type SortKey = "name" | "size" | "mtime" | "mode";
type Dir = "up" | "down";

interface Transfer {
  id: number;
  name: string;
  dir: Dir;
  done: number;
  total: number;
  started: number;
  ended: number;
  state: "queued" | "active" | "done" | "failed";
  error: string;
}

const ROW_H = 30;
const OVERSCAN = 8;
const EDIT_CAP = 8 * 1024 * 1024;
const MAX_PARALLEL = 3;
const STALE_MS = 15_000;

const stamp = new Intl.DateTimeFormat(undefined, {
  year: "numeric",
  month: "short",
  day: "2-digit",
  hour: "2-digit",
  minute: "2-digit",
});

function join(dir: string, name: string): string {
  if (!dir || dir === "/") return `/${name}`;
  return `${dir.replace(/\/+$/, "")}/${name}`;
}

function parent(p: string): string {
  const trimmed = p.replace(/\/+$/, "");
  if (!trimmed) return "/";
  const i = trimmed.lastIndexOf("/");
  return i <= 0 ? "/" : trimmed.slice(0, i);
}

function basename(p: string): string {
  const parts = p.split(/[\\/]/).filter(Boolean);
  return parts[parts.length - 1] ?? p;
}

function modeStr(m: number): string {
  if (!m) return "";
  const kind = m & 0o170000;
  let s = kind === 0o040000 ? "d" : kind === 0o120000 ? "l" : "-";
  const bits = "rwxrwxrwx";
  for (let i = 0; i < 9; i++) s += m & (1 << (8 - i)) ? (bits[i] ?? "-") : "-";
  return s;
}

function when(mtime: number): string {
  return mtime ? stamp.format(new Date(mtime * 1000)) : "";
}

function secs(s: number): string {
  if (!Number.isFinite(s) || s < 0) return "–";
  if (s < 60) return `${Math.max(1, Math.round(s))}s`;
  const m = Math.floor(s / 60);
  return m < 60 ? `${m}m ${Math.round(s % 60)}s` : `${Math.floor(m / 60)}h ${m % 60}m`;
}

function validName(name: string): string | null {
  const v = name.trim();
  if (!v || v === "." || v === "..") return null;
  if (v.includes("/") || v.includes("\0")) return null;
  return v;
}

let transferSeq = 0;

export function filesTab(host: Id): Tab {
  const label = hostById(host)?.label ?? "host";
  const key = `files:${host}`;
  const title = `${label} · files`;
  const el = h("div", { class: "files-pane" });

  let home = "";
  let cwd = "";
  const back: string[] = [];
  const fwd: string[] = [];
  let entries: DirEntry[] = [];
  let view: DirEntry[] = [];
  let sel = -1;
  let sortKey: SortKey = "name";
  let sortDir = 1;
  let filter = "";
  let showHidden = true;
  let loading = false;
  let loadedAt = 0;
  let token = 0;
  let disposed = false;
  let failure: string | null = null;
  let menu: HTMLElement | null = null;
  let typeBuf = "";
  let typeAt = 0;
  const transfers: Transfer[] = [];
  const queue: (() => Promise<void>)[] = [];
  let running = 0;
  let trayOpen = true;

  const backBtn = btn("", { icon: "chevron", ghost: true, small: true, title: "Back", onClick: () => void goBack() });
  backBtn.classList.add("fp-flip");
  const fwdBtn = btn("", { icon: "chevron", ghost: true, small: true, title: "Forward", onClick: () => void goForward() });
  const upBtn = btn("", { icon: "up", ghost: true, small: true, title: "Parent folder (Backspace)", onClick: () => void goUp() });
  const crumbs = h("div", { class: "fp-crumbs", title: "Click empty space to type a path" });
  const filterInput = input({ class: "input fp-filter", placeholder: "Filter", type: "search" });
  const hiddenBtn = h(
    "button",
    { class: "btn small ghost fp-hidden on", type: "button", title: "Show hidden files", "aria-pressed": "true" },
    h("span", { class: "mono" }, ".*"),
  );
  const toolbar = h(
    "div",
    { class: "fp-toolbar" },
    h("div", { class: "fp-nav" }, backBtn, fwdBtn, upBtn),
    crumbs,
    h(
      "div",
      { class: "fp-tools" },
      h("div", { class: "fp-filter-wrap" }, icon("search", 13), filterInput),
      hiddenBtn,
      btn("", { icon: "refresh", ghost: true, small: true, title: "Refresh", onClick: () => void reload() }),
      btn("", { icon: "folder", ghost: true, small: true, title: "New folder", onClick: () => void mkdir() }),
      btn("Upload", { icon: "upload", small: true, onClick: () => void pickUpload() }),
    ),
  );

  const headCells: Record<SortKey, HTMLElement> = {
    name: h("button", { class: "fp-th fp-c-name", type: "button" }, "Name"),
    size: h("button", { class: "fp-th fp-c-size", type: "button" }, "Size"),
    mtime: h("button", { class: "fp-th fp-c-mtime", type: "button" }, "Modified"),
    mode: h("button", { class: "fp-th fp-c-mode", type: "button" }, "Mode"),
  };
  for (const [k, cell] of Object.entries(headCells) as [SortKey, HTMLElement][]) {
    cell.addEventListener("click", () => {
      if (sortKey === k) sortDir = -sortDir;
      else {
        sortKey = k;
        sortDir = k === "name" ? 1 : -1;
      }
      const current = view[sel]?.name;
      recompute();
      if (current !== undefined) sel = Math.max(0, view.findIndex((e) => e.name === current));
      paint();
    });
  }
  const header = h("div", { class: "fp-head" }, headCells.name, headCells.size, headCells.mtime, headCells.mode);
  const spacer = h("div", { class: "fp-spacer" });
  const scroller = h("div", { class: "fp-scroll", tabindex: 0, role: "grid", "aria-label": "Remote files", "data-typeahead": "" }, spacer);
  const notice = h("div", { class: "fp-notice", hidden: true });
  const bar = h("div", { class: "fp-loadbar", hidden: true });
  const status = h("div", { class: "fp-status" });
  const trayList = h("div", { class: "fp-tray-list" });
  const trayTitle = h("span", { class: "fp-tray-title" });
  const trayToggle = btn("", {
    icon: "chevron",
    ghost: true,
    small: true,
    title: "Collapse",
    onClick: () => {
      trayOpen = !trayOpen;
      paintTray();
    },
  });
  const tray = h(
    "div",
    { class: "fp-tray", hidden: true },
    h(
      "div",
      { class: "fp-tray-head" },
      trayTitle,
      h("span", { class: "spacer" }),
      btn("Clear finished", {
        small: true,
        ghost: true,
        onClick: () => {
          for (let i = transfers.length - 1; i >= 0; i--) {
            const t = transfers[i];
            if (t && (t.state === "done" || t.state === "failed")) transfers.splice(i, 1);
          }
          paintTray();
        },
      }),
      trayToggle,
    ),
    trayList,
  );
  const drop = h(
    "div",
    { class: "fp-drop", hidden: true },
    h("div", { class: "fp-drop-card" }, icon("upload", 26), h("b", null, "Drop to upload"), h("span", { class: "fp-drop-path mono" })),
  );
  const body = h("div", { class: "fp-body" }, header, bar, scroller, notice);
  el.append(toolbar, body, status, tray, drop);

  const recompute = () => {
    const q = filter.trim().toLowerCase();
    const list = entries.filter((e) => (showHidden || !e.name.startsWith(".")) && (!q || e.name.toLowerCase().includes(q)));
    const byName = (a: DirEntry, b: DirEntry) => a.name.localeCompare(b.name, undefined, { numeric: true, sensitivity: "base" });
    const cmp = (a: DirEntry, b: DirEntry): number => {
      if (a.is_dir !== b.is_dir) return a.is_dir ? -1 : 1;
      if (sortKey === "name") return byName(a, b) * sortDir;
      const r = sortKey === "size" ? a.size - b.size : sortKey === "mtime" ? a.mtime - b.mtime : (a.mode & 0o7777) - (b.mode & 0o7777);
      return r !== 0 ? r * sortDir : byName(a, b);
    };
    view = list.sort(cmp);
    if (sel >= view.length) sel = view.length - 1;
  };

  const paintCrumbs = () => {
    clear(crumbs);
    const parts = cwd.split("/").filter(Boolean);
    const rootBtn = h("button", { class: "fp-crumb", type: "button", onClick: () => void navigate("/") }, "/");
    crumbs.append(rootBtn);
    let acc = "";
    parts.forEach((p, i) => {
      acc += `/${p}`;
      const target = acc;
      if (i > 0) crumbs.append(h("span", { class: "fp-sep" }, icon("chevron", 11)));
      crumbs.append(
        h(
          "button",
          { class: `fp-crumb${i === parts.length - 1 ? " last" : ""}`, type: "button", title: target, onClick: () => void navigate(target) },
          p,
        ),
      );
    });
    backBtn.disabled = back.length === 0;
    fwdBtn.disabled = fwd.length === 0;
    upBtn.disabled = cwd === "/" || !cwd;
  };

  const editPath = () => {
    const field = input({ class: "input mono fp-path-input", value: cwd });
    let done = false;
    const finish = (go: boolean) => {
      if (done) return;
      done = true;
      const raw = field.value.trim();
      paintCrumbs();
      if (!go || !raw) return;
      let target = raw;
      if (target === "~" || target.startsWith("~/")) target = home + target.slice(1);
      else if (!target.startsWith("/")) target = join(cwd, target);
      void navigate(target.replace(/\/{2,}/g, "/"));
    };
    field.addEventListener("keydown", (e) => {
      if (e.key === "Enter") {
        e.preventDefault();
        finish(true);
      } else if (e.key === "Escape") {
        e.preventDefault();
        finish(false);
        scroller.focus();
      }
    });
    field.addEventListener("blur", () => finish(false));
    crumbs.replaceChildren(field);
    field.focus();
    field.select();
  };
  crumbs.addEventListener("click", (e) => {
    if (e.target === crumbs) editPath();
  });
  crumbs.addEventListener("dblclick", (e) => {
    e.preventDefault();
    editPath();
  });

  const paintHead = () => {
    for (const [k, cell] of Object.entries(headCells) as [SortKey, HTMLElement][]) {
      const on = k === sortKey;
      cell.classList.toggle("on", on);
      cell.setAttribute("aria-sort", on ? (sortDir > 0 ? "ascending" : "descending") : "none");
      cell.dataset.dir = on ? (sortDir > 0 ? "asc" : "desc") : "";
    }
  };

  const paintStatus = () => {
    const dirs = view.filter((e) => e.is_dir).length;
    const files = view.length - dirs;
    const size = view.reduce((n, e) => n + (e.is_dir ? 0 : e.size), 0);
    const picked = view[sel];
    const parts: Node[] = [h("span", null, `${dirs} folder${dirs === 1 ? "" : "s"} · ${files} file${files === 1 ? "" : "s"} · ${bytes(size)}`)];
    if (filter) parts.push(h("span", { class: "muted" }, `filtered from ${entries.length}`));
    parts.push(h("span", { class: "spacer" }));
    if (picked) parts.push(h("span", { class: "mono muted" }, picked.name));
    status.replaceChildren(...parts);
  };

  const rowFor = (e: DirEntry, i: number): HTMLElement => {
    const glyph = e.is_dir ? "folder" : e.is_link ? "link" : "file";
    const row = h(
      "div",
      {
        class: `fp-row${i === sel ? " sel" : ""}${e.is_dir ? " dir" : ""}${e.name.startsWith(".") ? " hidden-file" : ""}`,
        role: "row",
        "aria-selected": String(i === sel),
        style: { transform: `translateY(${i * ROW_H}px)` },
      },
      h("span", { class: "fp-c-name" }, icon(glyph, 15), h("span", { class: "fp-name" }, e.name)),
      h("span", { class: "fp-c-size mono" }, e.is_dir ? "—" : bytes(e.size)),
      h("span", { class: "fp-c-mtime" }, when(e.mtime)),
      h("span", { class: "fp-c-mode mono" }, modeStr(e.mode)),
    );
    row.addEventListener("mousedown", (ev) => {
      if (ev.button === 0 || ev.button === 2) select(i, false);
    });
    row.addEventListener("dblclick", () => void activate(e));
    row.addEventListener("contextmenu", (ev) => {
      ev.preventDefault();
      select(i, false);
      showMenu(e, ev.clientX, ev.clientY);
    });
    return row;
  };

  const paintRows = () => {
    spacer.style.height = `${view.length * ROW_H}px`;
    const top = scroller.scrollTop;
    const height = scroller.clientHeight || 600;
    const first = Math.max(0, Math.floor(top / ROW_H) - OVERSCAN);
    const last = Math.min(view.length, Math.ceil((top + height) / ROW_H) + OVERSCAN);
    const rows: HTMLElement[] = [];
    for (let i = first; i < last; i++) {
      const e = view[i];
      if (e) rows.push(rowFor(e, i));
    }
    spacer.replaceChildren(...rows);
  };

  const paintNotice = () => {
    if (failure) {
      notice.hidden = false;
      notice.replaceChildren(
        icon("warn", 22),
        h("b", null, "Could not read this folder"),
        h("span", { class: "muted" }, failure),
        btn("Retry", { icon: "refresh", small: true, onClick: () => void navigate(cwd || home || "/", false) }),
      );
    } else if (!loading && cwd && view.length === 0) {
      notice.hidden = false;
      notice.replaceChildren(
        icon(filter ? "search" : "folder", 22),
        h("b", null, filter ? "No matches" : "This folder is empty"),
        h("span", { class: "muted" }, filter ? `Nothing here matches "${filter}".` : "Drop files here or use Upload."),
      );
    } else {
      notice.hidden = true;
    }
  };

  const paint = () => {
    paintHead();
    paintRows();
    paintNotice();
    paintStatus();
  };

  const ensureVisible = () => {
    if (sel < 0) return;
    const y = sel * ROW_H;
    if (y < scroller.scrollTop) scroller.scrollTop = y;
    else if (y + ROW_H > scroller.scrollTop + scroller.clientHeight) scroller.scrollTop = y + ROW_H - scroller.clientHeight;
  };

  const select = (i: number, scroll = true) => {
    if (!view.length) {
      sel = -1;
      return;
    }
    sel = Math.max(0, Math.min(view.length - 1, i));
    if (scroll) ensureVisible();
    paintRows();
    paintStatus();
  };

  const navigate = async (path: string, push = true, focusName?: string) => {
    const ticket = ++token;
    loading = true;
    bar.hidden = false;
    try {
      const list = await api.sftpList(host, path);
      if (ticket !== token || disposed) return;
      if (push && cwd && cwd !== path) {
        back.push(cwd);
        fwd.length = 0;
      }
      const leaving = cwd;
      cwd = path;
      entries = list;
      failure = null;
      loadedAt = Date.now();
      recompute();
      const want = focusName ?? (parent(leaving) === path ? basename(leaving) : undefined);
      const idx = want !== undefined ? view.findIndex((e) => e.name === want) : -1;
      sel = idx >= 0 ? idx : view.length ? 0 : -1;
      scroller.scrollTop = 0;
      paintCrumbs();
      ensureVisible();
      retitleTab(key, title, "ok");
    } catch (e) {
      if (ticket !== token || disposed) return;
      if (!cwd) {
        failure = errText(e);
        retitleTab(key, title, "err");
      } else {
        fail(e, `Cannot open ${path}`);
      }
    } finally {
      if (ticket === token) {
        loading = false;
        bar.hidden = true;
        paint();
      }
    }
  };

  const reload = async (focusName?: string) => {
    if (!cwd) return init();
    await navigate(cwd, false, focusName ?? view[sel]?.name);
  };

  const init = async () => {
    loading = true;
    bar.hidden = false;
    try {
      home = await api.sftpHome(host);
      if (disposed) return;
      await navigate(home || "/", false);
    } catch (e) {
      if (disposed) return;
      failure = errText(e);
      loading = false;
      bar.hidden = true;
      retitleTab(key, title, "err");
      paint();
    }
  };

  const goBack = async () => {
    const prev = back.pop();
    if (prev === undefined) return;
    fwd.push(cwd);
    await navigate(prev, false);
  };

  const goForward = async () => {
    const next = fwd.pop();
    if (next === undefined) return;
    back.push(cwd);
    await navigate(next, false);
  };

  const goUp = async () => {
    if (!cwd || cwd === "/") return;
    await navigate(parent(cwd));
  };

  const pathOf = (e: DirEntry) => join(cwd, e.name);

  const activate = async (e: DirEntry) => {
    if (e.is_dir) {
      await navigate(pathOf(e));
      return;
    }
    if (e.is_link) {
      try {
        const list = await api.sftpList(host, pathOf(e));
        back.push(cwd);
        fwd.length = 0;
        cwd = pathOf(e);
        entries = list;
        failure = null;
        recompute();
        sel = view.length ? 0 : -1;
        paintCrumbs();
        paint();
        return;
      } catch {
        try {
          const real = await api.sftpRealpath(host, pathOf(e));
          void edit(e, real);
        } catch (err) {
          fail(err, `Cannot resolve ${e.name}`);
        }
        return;
      }
    }
    if (e.size > EDIT_CAP) {
      toast(`${e.name} is larger than 8 MiB; downloading instead`, "info");
      await download(e);
      return;
    }
    await edit(e);
  };

  const pump = () => {
    while (running < MAX_PARALLEL) {
      const job = queue.shift();
      if (!job) break;
      running++;
      void job().finally(() => {
        running--;
        pump();
      });
    }
  };

  let trayQueued = false;
  const scheduleTray = () => {
    if (trayQueued) return;
    trayQueued = true;
    requestAnimationFrame(() => {
      trayQueued = false;
      paintTray();
    });
  };

  const paintTray = () => {
    tray.hidden = transfers.length === 0;
    tray.classList.toggle("collapsed", !trayOpen);
    trayToggle.classList.toggle("fp-down", trayOpen);
    const active = transfers.filter((t) => t.state === "active" || t.state === "queued").length;
    trayTitle.textContent = active ? `Transfers · ${active} in progress` : `Transfers · ${transfers.length} finished`;
    if (!trayOpen) return;
    trayList.replaceChildren(
      ...transfers
        .slice()
        .reverse()
        .map((t) => {
          const pct = t.total > 0 ? Math.min(100, (t.done / t.total) * 100) : t.state === "done" ? 100 : 0;
          const elapsed = ((t.ended || Date.now()) - t.started) / 1000;
          const speed = elapsed > 0 ? t.done / elapsed : 0;
          let meta: string;
          if (t.state === "queued") meta = "Queued";
          else if (t.state === "active") meta = `${bytes(t.done)} of ${bytes(t.total)} · ${fmtRate(speed)} · ${speed > 0 ? secs((t.total - t.done) / speed) : "–"} left`;
          else if (t.state === "done") meta = `${bytes(t.done)} in ${secs(elapsed)} · ${fmtRate(speed)}`;
          else meta = t.error;
          return h(
            "div",
            { class: `fp-xfer st-${t.state}` },
            h("span", { class: "fp-xfer-icon" }, icon(t.state === "failed" ? "warn" : t.state === "done" ? "check" : t.dir === "up" ? "upload" : "download", 14)),
            h(
              "div",
              { class: "fp-xfer-main" },
              h("div", { class: "fp-xfer-top" }, h("span", { class: "fp-xfer-name" }, t.name), h("span", { class: "fp-xfer-pct mono" }, t.state === "failed" ? "failed" : `${Math.round(pct)}%`)),
              h("div", { class: "bar" }, h("span", { style: { width: `${pct}%` } })),
              h("div", { class: "fp-xfer-meta", title: meta }, meta),
            ),
          );
        }),
    );
  };

  const enqueue = (dir: Dir, name: string, run: (ch: Channel<Progress>) => Promise<number>) => {
    const t: Transfer = { id: ++transferSeq, name, dir, done: 0, total: 0, started: Date.now(), ended: 0, state: "queued", error: "" };
    transfers.push(t);
    trayOpen = true;
    paintTray();
    queue.push(async () => {
      if (disposed) return;
      t.state = "active";
      t.started = Date.now();
      scheduleTray();
      const ch = new Channel<Progress>();
      ch.onmessage = (p) => {
        t.done = p.done;
        t.total = p.total;
        scheduleTray();
      };
      try {
        const n = await run(ch);
        t.done = n;
        t.total = Math.max(t.total, n);
        t.state = "done";
      } catch (e) {
        t.state = "failed";
        t.error = errText(e);
        if (!disposed) fail(e, `${dir === "up" ? "Upload" : "Download"} of ${name} failed`);
      } finally {
        t.ended = Date.now();
        scheduleTray();
        if (dir === "up" && !disposed && !queue.length && running <= 1) void reload(name);
      }
    });
    pump();
  };

  const uploadPaths = async (paths: Picked[]) => {
    if (!paths.length || !cwd) return;
    const target = cwd;
    const names = paths.map((p) => p.name);
    const clashes = names.filter((n) => entries.some((e) => e.name === n));
    if (clashes.length) {
      const list = clashes.length > 3 ? `${clashes.slice(0, 3).join(", ")} and ${clashes.length - 3} more` : clashes.join(", ");
      const ok = await confirmBox("Replace files", `${list} already exist in ${target}. Uploads replace them atomically.`, { confirm: "Replace" });
      if (!ok) return;
    }
    paths.forEach((local, i) => {
      const name = names[i] ?? local.name;
      enqueue("up", name, (ch) => api.sftpUpload(host, local.token, join(target, name), ch));
    });
  };

  const pickUpload = async () => {
    if (!cwd) return;
    try {
      const picked = await api.pickOpen(`Upload to ${cwd}`, [], true);
      if (!picked.length) return;
      await uploadPaths(picked);
    } catch (e) {
      fail(e);
    }
  };

  const download = async (e: DirEntry) => {
    if (e.is_dir) {
      toast("Folders cannot be downloaded yet; open it and download files individually", "warn");
      return;
    }
    try {
      const target = await api.pickSave(`Download ${e.name}`, e.name, []);
      if (!target) return;
      const remote = pathOf(e);
      enqueue("down", e.name, (ch) => api.sftpDownload(host, remote, target.token, ch));
    } catch (err) {
      fail(err);
    }
  };

  const mkdir = async () => {
    if (!cwd) return;
    const raw = await askText("New folder", "Name", { placeholder: "releases", confirm: "Create" });
    if (raw === null) return;
    const name = validName(raw);
    if (!name) {
      toast("Folder names cannot be empty or contain '/'", "warn");
      return;
    }
    try {
      await api.sftpMkdir(host, join(cwd, name));
      await reload(name);
    } catch (e) {
      fail(e, "Could not create folder");
    }
  };

  const rename = async (e: DirEntry) => {
    const raw = await askText("Rename", "New name", { value: e.name, confirm: "Rename" });
    if (raw === null || raw === e.name) return;
    const name = validName(raw);
    if (!name) {
      toast("Names cannot be empty or contain '/'", "warn");
      return;
    }
    try {
      await api.sftpRename(host, pathOf(e), join(cwd, name));
      await reload(name);
    } catch (err) {
      fail(err, "Rename failed");
    }
  };

  const remove = async (e: DirEntry) => {
    const ok = await confirmBox(
      e.is_dir ? "Delete folder" : "Delete file",
      e.is_dir ? `Delete the folder "${e.name}"? It must be empty.` : `Delete "${e.name}" from ${label}? This cannot be undone.`,
      { danger: true, confirm: "Delete" },
    );
    if (!ok) return;
    try {
      await api.sftpRemove(host, pathOf(e), e.is_dir);
      const next = view[sel + 1]?.name ?? view[sel - 1]?.name;
      await reload(next);
      toast(`Deleted ${e.name}`, "ok", 1800);
    } catch (err) {
      fail(err, "Delete failed");
    }
  };

  const closeMenu = () => {
    menu?.remove();
    menu = null;
  };

  const showMenu = (e: DirEntry, x: number, y: number) => {
    closeMenu();
    const item = (label_: string, glyph: string, run: () => void, danger = false, hint = "") =>
      h(
        "button",
        {
          class: `fp-menu-item${danger ? " danger" : ""}`,
          type: "button",
          onClick: () => {
            closeMenu();
            run();
          },
        },
        icon(glyph, 14),
        h("span", { class: "grow" }, label_),
        hint ? h("span", { class: "fp-menu-hint" }, hint) : null,
      );
    menu = h(
      "div",
      { class: "fp-menu", role: "menu" },
      item(e.is_dir ? "Open" : "Edit", e.is_dir ? "folder" : "edit", () => void activate(e), false, "Enter"),
      e.is_dir ? null : item("Download", "download", () => void download(e)),
      item("Rename", "edit", () => void rename(e), false, "F2"),
      item("Copy path", "copy", () => void copyText(pathOf(e), "Path copied")),
      h("div", { class: "fp-menu-sep" }),
      item("Delete", "trash", () => void remove(e), true, isMac ? "⌫" : "Del"),
    );
    document.body.append(menu);
    const r = menu.getBoundingClientRect();
    menu.style.left = `${Math.min(x, window.innerWidth - r.width - 8)}px`;
    menu.style.top = `${Math.min(y, window.innerHeight - r.height - 8)}px`;
  };

  const onDocDown = (ev: MouseEvent) => {
    if (menu && !menu.contains(ev.target as Node)) closeMenu();
  };
  const onDocKey = (ev: KeyboardEvent) => {
    if (menu && ev.key === "Escape") closeMenu();
  };
  document.addEventListener("mousedown", onDocDown, true);
  document.addEventListener("keydown", onDocKey, true);

  scroller.addEventListener("scroll", () => {
    closeMenu();
    paintRows();
  });
  scroller.addEventListener("keydown", (e) => {
    const page = Math.max(1, Math.floor(scroller.clientHeight / ROW_H) - 1);
    const cur = view[sel];
    switch (e.key) {
      case "ArrowDown":
        e.preventDefault();
        select(sel + 1);
        return;
      case "ArrowUp":
        e.preventDefault();
        select(sel - 1);
        return;
      case "PageDown":
        e.preventDefault();
        select(sel + page);
        return;
      case "PageUp":
        e.preventDefault();
        select(sel - page);
        return;
      case "Home":
        e.preventDefault();
        select(0);
        return;
      case "End":
        e.preventDefault();
        select(view.length - 1);
        return;
      case "Enter":
        if (cur) {
          e.preventDefault();
          void activate(cur);
        }
        return;
      case "Backspace":
        e.preventDefault();
        if (e.metaKey && cur) void remove(cur);
        else void goUp();
        return;
      case "Delete":
        if (cur) {
          e.preventDefault();
          void remove(cur);
        }
        return;
      case "F2":
        if (cur) {
          e.preventDefault();
          void rename(cur);
        }
        return;
      case "ContextMenu":
        if (cur) {
          e.preventDefault();
          const r = scroller.getBoundingClientRect();
          showMenu(cur, r.left + 40, r.top + (sel * ROW_H - scroller.scrollTop) + ROW_H);
        }
        return;
    }
    if (e.key.length === 1 && !e.ctrlKey && !e.metaKey && !e.altKey) {
      const now = Date.now();
      typeBuf = now - typeAt > 700 ? e.key.toLowerCase() : typeBuf + e.key.toLowerCase();
      typeAt = now;
      const idx = view.findIndex((x) => x.name.toLowerCase().startsWith(typeBuf));
      if (idx >= 0) select(idx);
    }
  });

  filterInput.addEventListener("input", () => {
    filter = filterInput.value;
    recompute();
    sel = view.length ? 0 : -1;
    scroller.scrollTop = 0;
    paint();
  });
  filterInput.addEventListener("keydown", (e) => {
    if (e.key === "ArrowDown" || e.key === "Enter") {
      e.preventDefault();
      scroller.focus();
      if (e.key === "Enter" && view[sel]) void activate(view[sel] as DirEntry);
    } else if (e.key === "Escape") {
      filterInput.value = "";
      filter = "";
      recompute();
      paint();
      scroller.focus();
    }
  });
  hiddenBtn.addEventListener("click", () => {
    showHidden = !showHidden;
    hiddenBtn.classList.toggle("on", showHidden);
    hiddenBtn.setAttribute("aria-pressed", String(showHidden));
    const current = view[sel]?.name;
    recompute();
    sel = current !== undefined ? Math.max(0, view.findIndex((x) => x.name === current)) : 0;
    paint();
  });

  const resize = new ResizeObserver(() => paintRows());
  resize.observe(scroller);

  const dropPath = drop.querySelector(".fp-drop-path");
  const showDrop = () => {
    if (dropPath) dropPath.textContent = cwd;
    drop.hidden = false;
  };
  const hideDrop = () => {
    drop.hidden = true;
  };
  const unlistenDrop = getCurrentWebview()
    .onDragDropEvent((ev) => {
      if (disposed || el.hidden || !cwd) {
        hideDrop();
        return;
      }
      const p = ev.payload;
      if (p.type === "enter" || p.type === "over") showDrop();
      else if (p.type === "leave") hideDrop();
      else {
        hideDrop();
        void api
          .grantDropped(p.paths)
          .then(uploadPaths)
          .catch((e) => fail(e));
      }
    })
    .catch(() => () => undefined);

  const edit = async (e: DirEntry, resolved?: string) => {
    const path = resolved ?? pathOf(e);
    let text: string;
    try {
      text = await api.sftpRead(host, path);
    } catch (err) {
      fail(err, `Cannot open ${e.name}`);
      return;
    }
    if (disposed) return;
    let saved = text;
    const area = h("textarea", { class: "input fp-editor", spellcheck: false, wrap: "off", "aria-label": `Editing ${path}` }) as HTMLTextAreaElement;
    area.value = text;
    const pos = h("span", { class: "fp-pos mono" });
    const dirty = h("span", { class: "fp-dirty", title: "Unsaved changes", hidden: true });
    const saveBtn = btn("Save", { icon: "check", primary: true, disabled: true });
    const closeBtn = btn("Close", { ghost: true });
    const card = h(
      "div",
      { class: "modal wide fp-edit", role: "dialog", "aria-modal": "true", "aria-label": `Edit ${e.name}` },
      h(
        "header",
        { class: "modal-head" },
        h("div", { class: "fp-edit-title" }, icon("edit", 15), h("h2", null, e.name), dirty, h("span", { class: "mono muted fp-edit-path" }, path)),
        btn("", { icon: "close", ghost: true, title: "Close", onClick: () => void close() }),
      ),
      h("div", { class: "fp-edit-body" }, area),
      h("div", { class: "modal-actions fp-edit-foot" }, pos, h("span", { class: "spacer" }), h("span", { class: "muted fp-edit-hint" }, `${isMac ? "⌘" : "Ctrl"}+S saves atomically`), closeBtn, saveBtn),
    );
    const scrim = h("div", { class: "scrim" }, card);
    const isDirty = () => area.value !== saved;
    const meta = () => {
      const upto = area.value.slice(0, area.selectionStart);
      const line = upto.split("\n").length;
      const col = upto.length - upto.lastIndexOf("\n");
      const lines = area.value.split("\n").length;
      pos.textContent = `Ln ${line}, Col ${col} · ${lines.toLocaleString()} lines · ${bytes(new TextEncoder().encode(area.value).length)}`;
      dirty.hidden = !isDirty();
      saveBtn.disabled = !isDirty();
    };
    const doSave = async () => {
      if (!isDirty()) return;
      saveBtn.disabled = true;
      card.classList.add("busy");
      const content = area.value;
      try {
        await api.sftpWrite(host, path, content);
        saved = content;
        toast("Saved atomically", "ok", 1800);
        void reload(e.name);
      } catch (err) {
        fail(err, "Save failed");
      } finally {
        card.classList.remove("busy");
        meta();
      }
    };
    let closing = false;
    const close = async () => {
      if (closing) return;
      if (isDirty()) {
        const discard = await confirmBox("Discard changes", `${e.name} has unsaved changes.`, { danger: true, confirm: "Discard" });
        if (!discard) {
          area.focus();
          return;
        }
      }
      closing = true;
      scrim.classList.remove("in");
      setTimeout(() => scrim.remove(), 160);
      scroller.focus();
    };
    saveBtn.addEventListener("click", () => void doSave());
    closeBtn.addEventListener("click", () => void close());
    scrim.addEventListener("mousedown", (ev) => {
      if (ev.target === scrim) void close();
    });
    area.addEventListener("keydown", (ev) => {
      const mod = isMac ? ev.metaKey : ev.ctrlKey;
      if (ev.key === "Tab" && !ev.shiftKey && !mod) {
        ev.preventDefault();
        area.setRangeText("\t", area.selectionStart, area.selectionEnd, "end");
        meta();
      } else if (mod && keyOf(ev).toLowerCase() === "s") {
        ev.preventDefault();
        void doSave();
      } else if (ev.key === "Escape") {
        ev.preventDefault();
        ev.stopPropagation();
        void close();
      }
    });
    for (const evName of ["input", "click", "keyup", "select"]) area.addEventListener(evName, meta);
    document.body.append(scrim);
    meta();
    requestAnimationFrame(() => {
      scrim.classList.add("in");
      area.focus();
      area.setSelectionRange(0, 0);
      area.scrollTop = 0;
    });
  };

  paintCrumbs();
  paint();
  let started = false;

  return {
    key,
    title,
    icon: "folder",
    host,
    el,
    tone: "idle",
    onShow() {
      if (!started) {
        started = true;
        void init();
      } else if (!loading && Date.now() - loadedAt > STALE_MS) {
        void reload();
      }
      requestAnimationFrame(() => {
        paintRows();
        if (!el.contains(document.activeElement)) scroller.focus();
      });
    },
    onHide() {
      closeMenu();
      hideDrop();
    },
    dispose() {
      disposed = true;
      token++;
      queue.length = 0;
      closeMenu();
      resize.disconnect();
      document.removeEventListener("mousedown", onDocDown, true);
      document.removeEventListener("keydown", onDocKey, true);
      void unlistenDrop.then((off) => off());
    },
  };
}
