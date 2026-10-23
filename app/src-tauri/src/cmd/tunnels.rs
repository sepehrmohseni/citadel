use std::sync::Arc;
use std::time::Duration;

use citadel_ssh::{ForwardSpec, Tunnel};
use citadel_vault::{Endpoint, Forward, HostProfile, Id, TunnelSpec};
use serde::Serialize;
use tauri::{Emitter, State};

use super::{AppState, blocking};
use crate::core::{Core, EV_TUNNEL, TunnelEntry, lock, parse_id};
use crate::dto::{TunnelInput, TunnelView};
use crate::error::{AppError, CmdResult};

const STATS_EVERY: Duration = Duration::from_secs(1);

fn spec_of(f: &Forward) -> ForwardSpec {
    match f {
        Forward::Local { bind, dest } => ForwardSpec::Local {
            bind: bind.host.clone(),
            bind_port: bind.port,
            dest: dest.host.clone(),
            dest_port: dest.port,
        },
        Forward::Remote { bind, dest } => ForwardSpec::Remote {
            bind: bind.host.clone(),
            bind_port: bind.port,
            dest: dest.host.clone(),
            dest_port: dest.port,
        },
        Forward::Dynamic { bind } => ForwardSpec::Dynamic {
            bind: bind.host.clone(),
            bind_port: bind.port,
        },
    }
}

fn check_endpoint(what: &str, e: &Endpoint, zero_ok: bool) -> CmdResult<()> {
    let h = e.host.trim();
    if h.is_empty() || h.len() > 253 || h.contains(char::is_whitespace) {
        return Err(AppError::invalid(format!("{what} host is malformed")));
    }
    if e.port == 0 && !zero_ok {
        return Err(AppError::invalid(format!("{what} port is required")));
    }
    Ok(())
}

fn validate(f: &Forward) -> CmdResult<()> {
    match f {
        Forward::Local { bind, dest } => {
            check_endpoint("bind", bind, true)?;
            check_endpoint("destination", dest, false)
        }
        Forward::Remote { bind, dest } => {
            check_endpoint("remote bind", bind, true)?;
            check_endpoint("local destination", dest, false)
        }
        Forward::Dynamic { bind } => check_endpoint("bind", bind, true),
    }
}

fn live(core: &Core, id: Id) -> Option<(citadel_ssh::TunnelState, citadel_ssh::MeterSnapshot)> {
    lock(&core.tunnels)
        .get(&id)
        .map(|e| (e.tunnel.state().borrow().clone(), e.tunnel.stats()))
}

#[tauri::command]
pub async fn tunnels_list(state: State<'_, AppState>) -> CmdResult<Vec<TunnelView>> {
    let core = state.0.clone();
    let vault = core.vault.clone();
    let mut all = blocking(move || Ok(vault.list::<TunnelSpec>()?)).await?;
    all.sort_by_cached_key(|t| t.label.to_lowercase());
    Ok(all
        .iter()
        .map(|t| TunnelView::new(t, live(&core, t.id)))
        .collect())
}

#[tauri::command]
pub async fn tunnel_save(state: State<'_, AppState>, input: TunnelInput) -> CmdResult<TunnelView> {
    let core = state.0.clone();
    validate(&input.forward)?;
    let label = input.label.trim().to_owned();
    if label.is_empty() || label.len() > 128 {
        return Err(AppError::invalid("label is required"));
    }
    let host = parse_id(&input.host)?;
    let vault = core.vault.clone();
    let (saved, rewired) = blocking(move || {
        vault
            .get::<HostProfile>(host)?
            .ok_or(AppError::NotFound("host"))?;
        let (mut t, rewired) = match &input.id {
            Some(raw) => {
                let t = vault
                    .get::<TunnelSpec>(parse_id(raw)?)?
                    .ok_or(AppError::NotFound("tunnel"))?;
                let rewired = t.host != host || t.forward != input.forward;
                (t, rewired)
            }
            None => (TunnelSpec::new(&label, host, input.forward.clone())?, false),
        };
        t.label = label;
        t.host = host;
        t.forward = input.forward;
        t.autostart = input.autostart;
        vault.put(&mut t)?;
        Ok((t, rewired))
    })
    .await?;
    let running = lock(&core.tunnels).contains_key(&saved.id);
    if running && rewired {
        halt(&core, saved.id);
        start(&core, &saved).await?;
    }
    Ok(TunnelView::new(&saved, live(&core, saved.id)))
}

#[tauri::command]
pub async fn tunnel_delete(state: State<'_, AppState>, id: String) -> CmdResult<()> {
    let id = parse_id(&id)?;
    halt(&state.0, id);
    let vault = state.0.vault.clone();
    blocking(move || {
        vault.remove(id)?;
        Ok(())
    })
    .await
}

#[tauri::command]
pub async fn tunnel_start(state: State<'_, AppState>, id: String) -> CmdResult<TunnelView> {
    let core = state.0.clone();
    let id = parse_id(&id)?;
    let vault = core.vault.clone();
    let spec = blocking(move || {
        vault
            .get::<TunnelSpec>(id)?
            .ok_or(AppError::NotFound("tunnel"))
    })
    .await?;
    if !lock(&core.tunnels).contains_key(&id) {
        start(&core, &spec).await?;
    }
    Ok(TunnelView::new(&spec, live(&core, id)))
}

#[tauri::command]
pub async fn tunnel_stop(state: State<'_, AppState>, id: String) -> CmdResult<()> {
    halt(&state.0, parse_id(&id)?);
    Ok(())
}

#[derive(Clone, Serialize)]
struct TunnelEvent {
    id: String,
    running: bool,
    state: Option<citadel_ssh::TunnelState>,
    stats: Option<citadel_ssh::MeterSnapshot>,
}

struct Starting<'a> {
    core: &'a Core,
    id: Id,
}

impl Drop for Starting<'_> {
    fn drop(&mut self) {
        lock(&self.core.starting).remove(&self.id);
    }
}

async fn start(core: &Arc<Core>, spec: &TunnelSpec) -> CmdResult<()> {
    if lock(&core.tunnels).contains_key(&spec.id) || !lock(&core.starting).insert(spec.id) {
        return Ok(());
    }
    let _starting = Starting { core, id: spec.id };
    let handle = core.acquire(spec.host);
    let tunnel = match Tunnel::open(spec_of(&spec.forward), handle).await {
        Ok(t) => t,
        Err(e) => {
            core.release(spec.host);
            return Err(e.into());
        }
    };
    let id = spec.id;
    let app = core.app.clone();
    let mut state = tunnel.state();
    let weak = Arc::downgrade(core);
    let watch = tauri::async_runtime::spawn(async move {
        let mut tick = tokio::time::interval(STATS_EVERY);
        loop {
            tokio::select! {
                changed = state.changed() => if changed.is_err() { return },
                _ = tick.tick() => {}
            }
            let Some(core) = weak.upgrade() else { return };
            let Some((st, stats)) = live(&core, id) else {
                return;
            };
            let _ = app.emit(
                EV_TUNNEL,
                TunnelEvent {
                    id: id.to_string(),
                    running: true,
                    state: Some(st),
                    stats: Some(stats),
                },
            );
        }
    });
    lock(&core.tunnels).insert(
        id,
        TunnelEntry {
            tunnel,
            host: spec.host,
            watch: watch.inner().abort_handle(),
        },
    );
    Ok(())
}

pub fn halt(core: &Arc<Core>, id: Id) {
    let entry = lock(&core.tunnels).remove(&id);
    if let Some(e) = entry {
        e.watch.abort();
        e.tunnel.stop();
        core.release(e.host);
        let _ = core.app.emit(
            EV_TUNNEL,
            TunnelEvent {
                id: id.to_string(),
                running: false,
                state: None,
                stats: None,
            },
        );
    }
}

pub async fn autostart(core: &Arc<Core>) {
    let vault = core.vault.clone();
    let Ok(Ok(specs)) = tokio::task::spawn_blocking(move || vault.list::<TunnelSpec>()).await
    else {
        return;
    };
    for spec in specs.iter().filter(|t| t.autostart) {
        if lock(&core.tunnels).contains_key(&spec.id) {
            continue;
        }
        if let Err(e) = start(core, spec).await {
            tracing::warn!(tunnel = %spec.label, "autostart failed: {e}");
        }
    }
}
