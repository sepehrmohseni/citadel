import { api, events, type Host, type Id, type Identity, type LinkState, type Prefs, type Snippet, type Tunnel } from "./api";

export class Signal<T> {
  #value: T;
  #subs = new Set<(v: T) => void>();

  constructor(value: T) {
    this.#value = value;
  }

  get(): T {
    return this.#value;
  }

  set(value: T): void {
    this.#value = value;
    for (const fn of [...this.#subs]) fn(value);
  }

  update(fn: (v: T) => T): void {
    this.set(fn(this.#value));
  }

  subscribe(fn: (v: T) => void, immediate = true): () => void {
    this.#subs.add(fn);
    if (immediate) fn(this.#value);
    return () => this.#subs.delete(fn);
  }
}

export const hosts = new Signal<Host[]>([]);
export const tunnels = new Signal<Tunnel[]>([]);
export const identities = new Signal<Identity[]>([]);
export const snippets = new Signal<Snippet[]>([]);
export const links = new Signal<Map<Id, LinkState>>(new Map());
export const prefs = new Signal<Prefs | null>(null);

export function hostById(id: Id): Host | undefined {
  return hosts.get().find((h) => h.id === id);
}

export async function refreshHosts(): Promise<void> {
  hosts.set(await api.hosts());
}

export async function refreshTunnels(): Promise<void> {
  tunnels.set(await api.tunnels());
}

export async function refreshIdentities(): Promise<void> {
  identities.set(await api.identities());
}

export async function refreshSnippets(): Promise<void> {
  snippets.set(await api.snippets());
}

export async function refreshAll(): Promise<void> {
  const [h, t, i, s, p, l] = await Promise.all([
    api.hosts(),
    api.tunnels(),
    api.identities(),
    api.snippets(),
    api.prefsGet(),
    api.links(),
  ]);
  hosts.set(h);
  tunnels.set(t);
  identities.set(i);
  snippets.set(s);
  prefs.set(p);
  links.set(new Map(l.map(({ host, ...state }) => [host, state as LinkState])));
}

let wired = false;

export function wireEvents(): void {
  if (wired) return;
  wired = true;
  void events.link((e) => {
    const { host, ...state } = e;
    links.update((m) => {
      const next = new Map(m);
      if (state.state === "stopped") next.delete(host);
      else next.set(host, state as LinkState);
      return next;
    });
  });
  void events.tunnel((e) => {
    tunnels.update((list) =>
      list.map((t) => (t.id === e.id ? { ...t, running: e.running, state: e.state, stats: e.stats } : t)),
    );
  });
}
