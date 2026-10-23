import { Channel, invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

export type Id = string;
export type LockState = "uninitialized" | "locked" | "unlocked";

export interface AppError {
  code: string;
  message: string;
}

export function errText(e: unknown): string {
  if (e && typeof e === "object" && "message" in e) return String((e as AppError).message);
  return String(e);
}

export function errCode(e: unknown): string {
  if (e && typeof e === "object" && "code" in e) return String((e as AppError).code);
  return "unknown";
}

export interface VaultStatus {
  state: LockState;
  path: string;
  autolock_minutes: number;
  backups: number;
  latest_backup_ms: number | null;
  unreadable: number;
}

export interface BackupInfo {
  name: string;
  created_ms: number;
  bytes: number;
}

export interface Point {
  x: number;
  y: number;
}

export type AuthView =
  | { kind: "identity"; identity: Id }
  | { kind: "agent"; fingerprint: string | null }
  | { kind: "password"; stored: boolean }
  | { kind: "interactive" };

export type AuthInput =
  | { kind: "identity"; identity: Id }
  | { kind: "agent"; fingerprint: string | null }
  | { kind: "password"; password: string | null }
  | { kind: "interactive" };

export interface Host {
  id: Id;
  label: string;
  hostname: string;
  port: number;
  user: string;
  auth: AuthView;
  jump: Id[];
  workspace: string | null;
  tags: string[];
  keepalive_secs: number;
  agent_forward: boolean;
  legacy_crypto: boolean;
  ssh_alias: string | null;
  canvas: Point | null;
  updated_ms: number;
}

export interface HostInput {
  id: Id | null;
  label: string;
  hostname: string;
  port: number;
  user: string;
  auth: AuthInput;
  jump: Id[];
  workspace: string | null;
  tags: string[];
  keepalive_secs: number;
  agent_forward: boolean;
  legacy_crypto: boolean;
  ssh_alias: string | null;
}

export interface Identity {
  id: Id;
  label: string;
  algorithm: string;
  public_key: string;
  fingerprint: string;
  source: "vault" | "agent";
  encrypted: boolean;
}

export interface AgentKey {
  algorithm: string;
  public_key: string;
  fingerprint: string;
  comment: string;
}

export type KeyKind = "ed25519" | "ecdsa_p256";

export interface Endpoint {
  host: string;
  port: number;
}

export type Forward =
  | { kind: "local"; bind: Endpoint; dest: Endpoint }
  | { kind: "remote"; bind: Endpoint; dest: Endpoint }
  | { kind: "dynamic"; bind: Endpoint };

export type TunnelState =
  | { state: "listening"; addr: string }
  | { state: "waiting"; reason: string }
  | { state: "forwarding"; remote_port: number }
  | { state: "failed"; error: string }
  | { state: "stopped" };

export interface Meter {
  active: number;
  accepted: number;
  failed: number;
  tx: number;
  rx: number;
}

export interface Tunnel {
  id: Id;
  label: string;
  host: Id;
  forward: Forward;
  autostart: boolean;
  canvas: Point | null;
  running: boolean;
  state: TunnelState | null;
  stats: Meter | null;
}

export interface TunnelInput {
  id: Id | null;
  label: string;
  host: Id;
  forward: Forward;
  autostart: boolean;
}

export interface TunnelEvent {
  id: Id;
  running: boolean;
  state: TunnelState | null;
  stats: Meter | null;
}

export type LinkState =
  | { state: "idle" }
  | { state: "connecting"; attempt: number }
  | { state: "up"; attempt: number; rtt_ms: number | null }
  | { state: "backoff"; attempt: number; retry_ms: number; error: string }
  | { state: "stopped" };

export type LinkEvent = { host: Id } & LinkState;

export interface Snippet {
  id: Id;
  label: string;
  body: string;
}

export interface KnownHost {
  id: Id;
  host: string;
  port: number;
  algorithm: string;
  fingerprint: string;
}

export interface DirEntry {
  name: string;
  size: number;
  mode: number;
  mtime: number;
  is_dir: boolean;
  is_link: boolean;
}

export interface Progress {
  done: number;
  total: number;
}

export interface HostInfo {
  hostname: string;
  kernel: string;
  uptime_secs: number;
  cores: number;
}

export interface CpuStat {
  busy_pct: number;
  per_core: number[];
  load: [number, number, number];
}

export interface MemStat {
  total_kib: number;
  available_kib: number;
  used_pct: number;
  swap_total_kib: number;
  swap_used_kib: number;
}

export interface NetIf {
  name: string;
  rx_bytes: number;
  tx_bytes: number;
  rx_bps: number;
  tx_bps: number;
}

export interface Disk {
  mount: string;
  fs: string;
  total_kib: number;
  used_kib: number;
  used_pct: number;
}

export interface Container {
  id: string;
  name: string;
  image: string;
  state: string;
  status: string;
}

export type Docker =
  | { state: "unknown" }
  | { state: "available"; containers: Container[] }
  | { state: "unavailable"; reason: string };

export interface Snapshot {
  at_ms: number;
  host: HostInfo;
  cpu: CpuStat;
  mem: MemStat;
  net: NetIf[];
  disks: Disk[];
  docker: Docker;
}

export type TelemetryMsg =
  | ({ kind: "sample" } & Snapshot)
  | { kind: "error"; message: string }
  | { kind: "ended" };

export interface Prefs {
  autolock_minutes: number;
  agent: string | null;
  font_family: string;
  font_size: number;
  line_height: number;
  scrollback: number;
  cursor_blink: boolean;
  cursor_style: string;
  theme: string;
  telemetry_ms: number;
  paste_confirm_lines: number;
  copy_on_select: boolean;
  renderer: "auto" | "webgl" | "dom";
  keybindings: Record<string, string[]>;
  global_hotkey: string;
  restore_tabs: boolean;
  detect_ports: boolean;
}

export interface FileFilter {
  name: string;
  extensions: string[];
}

export interface Picked {
  token: string;
  name: string;
  path: string;
}

export type RestoreSource = { kind: "snapshot"; name: string } | { kind: "file"; token: string };

export interface FontFamily {
  name: string;
  mono: boolean;
}

export type FleetEvent =
  | { kind: "started"; host: Id }
  | { kind: "output"; host: Id; stderr: boolean; text: string }
  | { kind: "truncated"; host: Id }
  | { kind: "done"; host: Id; status: number | null; ms: number }
  | { kind: "failed"; host: Id; error: string; ms: number }
  | { kind: "finished" };

export interface Listener {
  port: number;
  scope: "address" | "loopback" | "any";
  address: string;
  process: string | null;
  pid: number | null;
}

export interface HotkeyStatus {
  chord: string;
  active: boolean;
  error: string | null;
}

export interface MergeReport {
  added: number;
  updated: number;
  deleted: number;
  unchanged: number;
}

export interface ImportOutcome {
  report: MergeReport;
  neutralized: { autostart: number; agent_forward: number; host_keys: number; rejected: number };
}

export type Severity = "info" | "low" | "medium" | "high" | "critical";

export interface Finding {
  severity: Severity;
  area: string;
  title: string;
  detail: string;
  subject: Id | null;
}

export interface AuditReport {
  score: number;
  grade: string;
  checked: number;
  findings: Finding[];
}

export interface Absorbed {
  report: MergeReport;
  rekeyed: boolean;
  header_adopted: boolean;
}

export interface SyncReport {
  pulled: Absorbed | null;
  pushed: boolean;
  rounds: number;
  version: string | null;
}

export type SyncBackendView =
  | { kind: "s3"; endpoint: string; region: string; bucket: string; object: string; access_key: string; path_style: boolean }
  | { kind: "web_dav"; url: string; user: string }
  | { kind: "git"; remote: string; branch: string; path: string };

export type SyncBackendInput =
  | {
      kind: "s3";
      endpoint: string;
      region: string;
      bucket: string;
      object: string;
      access_key: string;
      secret_key: string | null;
      path_style: boolean;
    }
  | { kind: "web_dav"; url: string; user: string; password: string | null }
  | { kind: "git"; remote: string; branch: string; path: string };

export interface SyncTarget {
  id: Id;
  label: string;
  backend: SyncBackendView;
}

export interface SshHost {
  alias: string;
  hostname: string | null;
  user: string | null;
  port: number | null;
  identity_files: string[];
  proxy_jump: string[];
  identity_agent: string | null;
  forward_agent: boolean | null;
  identities_only: boolean | null;
  local_forwards: string[];
  remote_forwards: string[];
  dynamic_forwards: string[];
}

export interface ConfigEntry {
  host: SshHost;
  imported: Id | null;
}

export interface ImportSummary {
  hosts: number;
  keys: number;
  skipped: string[];
}

export interface AgentSocket {
  kind: "native" | "one_password" | "bitwarden" | "custom";
  path: string;
  available: boolean;
}

export type PromptEvent =
  | { id: number; kind: "host_key"; host: string; port: number; algorithm: string; fingerprint: string; status: "unknown" | "changed"; previous: string | null }
  | {
      id: number;
      kind: "interactive";
      host: string;
      user: string;
      name: string;
      instructions: string;
      prompts: { text: string; echo: boolean }[];
    };

export type Answer = { kind: "host_key"; accept: boolean } | { kind: "interactive"; answers: string[] } | { kind: "cancel" };

export type PtyStatus = { kind: "exit"; code: number } | { kind: "signal"; name: string } | { kind: "closed" };

export const api = {
  vaultStatus: () => invoke<VaultStatus>("vault_status"),
  vaultCreate: (password: string) => invoke<void>("vault_create", { password }),
  vaultUnlock: (password: string) => invoke<void>("vault_unlock", { password }),
  vaultLock: () => invoke<void>("vault_lock"),
  vaultChangePassword: (current: string, next: string) => invoke<void>("vault_change_password", { current, next }),
  vaultExport: (target: string, password: string) => invoke<number>("vault_export", { target, password }),
  vaultImport: (source: string, password: string, replace: boolean) =>
    invoke<ImportOutcome>("vault_import", { source, password, replace }),
  vaultAudit: () => invoke<AuditReport>("vault_audit"),
  backups: () => invoke<BackupInfo[]>("backups_list"),
  backupNow: () => invoke<BackupInfo>("backup_now"),
  backupRestore: (source: RestoreSource, password: string, replace: boolean) => invoke<ImportOutcome>("backup_restore", { source, password, replace }),
  recordingSave: (target: string, content: string) => invoke<number>("recording_save", { target, content }),
  pickOpen: (title: string, filters: FileFilter[], multiple = false) => invoke<Picked[]>("pick_open", { title, filters, multiple }),
  pickSave: (title: string, defaultName: string, filters: FileFilter[]) => invoke<Picked | null>("pick_save", { title, defaultName, filters }),
  grantDropped: (paths: string[]) => invoke<Picked[]>("grant_dropped", { paths }),
  fonts: () => invoke<FontFamily[]>("fonts_list"),
  sessionLoad: () => invoke<string | null>("session_load"),
  sessionSave: (body: string) => invoke<void>("session_save", { body }),
  fleetRun: (hosts: Id[], command: string, timeoutSecs: number, feed: Channel<FleetEvent>) => invoke<number>("fleet_run", { hosts, command, timeoutSecs, feed }),
  fleetCancel: (id: number) => invoke<void>("fleet_cancel", { id }),
  ports: (host: Id) => invoke<Listener[]>("ports_list", { host }),
  portFree: (preferred: number) => invoke<number>("port_free", { preferred }),
  openLocal: (port: number) => invoke<void>("open_local", { port }),
  openLink: (which: "repo" | "issues" | "releases" | "site" | "author") => invoke<void>("open_link", { which }),
  hotkeyStatus: () => invoke<HotkeyStatus>("hotkey_status"),
  hotkeySet: (chord: string) => invoke<HotkeyStatus>("hotkey_set", { chord }),

  hosts: () => invoke<Host[]>("hosts_list"),
  hostSave: (input: HostInput) => invoke<Host>("host_save", { input }),
  hostDelete: (id: Id) => invoke<void>("host_delete", { id }),
  canvasMove: (id: Id, x: number, y: number) => invoke<void>("canvas_move", { id, x, y }),

  identities: () => invoke<Identity[]>("identities_list"),
  identityGenerate: (label: string, kind: KeyKind) => invoke<Identity>("identity_generate", { label, kind }),
  identityImport: (label: string, pem: string, passphrase: string | null) =>
    invoke<Identity>("identity_import", { label, pem, passphrase }),
  identityImportFile: (label: string, source: string, passphrase: string | null) =>
    invoke<Identity>("identity_import_file", { label, source, passphrase }),
  identityAgent: (label: string, publicKey: string) => invoke<Identity>("identity_agent", { label, publicKey }),
  identityDelete: (id: Id) => invoke<void>("identity_delete", { id }),
  agentKeys: () => invoke<AgentKey[]>("agent_keys"),

  snippets: () => invoke<Snippet[]>("snippets_list"),
  snippetSave: (id: Id | null, label: string, body: string) => invoke<Snippet>("snippet_save", { id, label, body }),
  snippetDelete: (id: Id) => invoke<void>("snippet_delete", { id }),

  knownHosts: () => invoke<KnownHost[]>("known_hosts_list"),
  knownHostDelete: (id: Id) => invoke<void>("known_host_delete", { id }),

  ptyOpen: (host: Id, cols: number, rows: number, data: Channel<ArrayBuffer>, status: Channel<PtyStatus>) =>
    invoke<number>("pty_open", { host, cols, rows, data, status }),
  ptyWrite: (pid: number, bytes: Uint8Array) => invoke<void>("pty_write", bytes, { headers: { "x-pty": String(pid) } }),
  ptyAck: (pid: number, bytes: number) => invoke<void>("pty_ack", { pid, bytes }),
  ptyResize: (pid: number, cols: number, rows: number) => invoke<void>("pty_resize", { pid, cols, rows }),
  ptyClose: (pid: number) => invoke<void>("pty_close", { pid }),

  tunnels: () => invoke<Tunnel[]>("tunnels_list"),
  tunnelSave: (input: TunnelInput) => invoke<Tunnel>("tunnel_save", { input }),
  tunnelDelete: (id: Id) => invoke<void>("tunnel_delete", { id }),
  tunnelStart: (id: Id) => invoke<Tunnel>("tunnel_start", { id }),
  tunnelStop: (id: Id) => invoke<void>("tunnel_stop", { id }),

  sftpHome: (host: Id) => invoke<string>("sftp_home", { host }),
  sftpList: (host: Id, path: string) => invoke<DirEntry[]>("sftp_list", { host, path }),
  sftpMkdir: (host: Id, path: string) => invoke<void>("sftp_mkdir", { host, path }),
  sftpRemove: (host: Id, path: string, dir: boolean) => invoke<void>("sftp_remove", { host, path, dir }),
  sftpRename: (host: Id, from: string, to: string) => invoke<void>("sftp_rename", { host, from, to }),
  sftpDownload: (host: Id, remote: string, local: string, progress: Channel<Progress>) =>
    invoke<number>("sftp_download", { host, remote, local, progress }),
  sftpUpload: (host: Id, local: string, remote: string, progress: Channel<Progress>) =>
    invoke<number>("sftp_upload", { host, local, remote, progress }),
  sftpRealpath: (host: Id, path: string) => invoke<string>("sftp_realpath", { host, path }),
  sftpRead: (host: Id, path: string) => invoke<string>("sftp_read", { host, path }),
  sftpWrite: (host: Id, path: string, content: string) => invoke<void>("sftp_write", { host, path, content }),

  telemetryStart: (host: Id, feed: Channel<TelemetryMsg>) => invoke<number>("telemetry_start", { host, feed }),
  telemetryStop: (id: number) => invoke<void>("telemetry_stop", { id }),

  sshConfigHosts: () => invoke<ConfigEntry[]>("sshconfig_hosts"),
  sshConfigImport: (aliases: string[]) => invoke<ImportSummary>("sshconfig_import", { aliases }),
  sshConfigExport: (ids: Id[]) => invoke<string>("sshconfig_export", { ids }),
  agents: () => invoke<AgentSocket[]>("agents_list"),

  syncTargets: () => invoke<SyncTarget[]>("sync_targets_list"),
  syncTargetSave: (input: { id: Id | null; label: string; backend: SyncBackendInput }) =>
    invoke<SyncTarget>("sync_target_save", { input }),
  syncTargetDelete: (id: Id) => invoke<void>("sync_target_delete", { id }),
  syncRun: (id: Id, password: string | null) => invoke<SyncReport>("sync_run", { id, password }),

  prefsGet: () => invoke<Prefs>("prefs_get"),
  prefsSet: (prefs: Prefs) => invoke<Prefs>("prefs_set", { prefs }),
  promptAnswer: (id: number, answer: Answer) => invoke<boolean>("prompt_answer", { id, answer }),
  links: () => invoke<LinkEvent[]>("links_list"),
  linkReconnect: (host: Id) => invoke<void>("link_reconnect", { host }),
};

export const events = {
  vault: (fn: (s: LockState) => void): Promise<UnlistenFn> => listen<LockState>("citadel://vault", (e) => fn(e.payload)),
  link: (fn: (e: LinkEvent) => void): Promise<UnlistenFn> => listen<LinkEvent>("citadel://link", (e) => fn(e.payload)),
  tunnel: (fn: (e: TunnelEvent) => void): Promise<UnlistenFn> =>
    listen<TunnelEvent>("citadel://tunnel", (e) => fn(e.payload)),
  prompt: (fn: (e: PromptEvent) => void): Promise<UnlistenFn> =>
    listen<PromptEvent>("citadel://prompt", (e) => fn(e.payload)),
  banner: (fn: (label: string, text: string) => void): Promise<UnlistenFn> =>
    listen<[string, string]>("citadel://banner", (e) => fn(e.payload[0], e.payload[1])),
};

export { Channel };
