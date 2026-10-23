mod cmd;
mod core;
mod dto;
mod error;
mod plan;
mod prefs;
mod safety;

use std::sync::Arc;

use citadel_vault::{AutoLock, Vault};
use tauri::{Manager, RunEvent};

use crate::cmd::AppState;
use crate::core::{Core, Dirs};
use crate::prefs::Prefs;

#[cfg(unix)]
#[allow(unsafe_code)]
fn harden() {
    let zero = libc::rlimit {
        rlim_cur: 0,
        rlim_max: 0,
    };
    unsafe {
        libc::setrlimit(libc::RLIMIT_CORE, &raw const zero);
    }
    #[cfg(target_os = "linux")]
    unsafe {
        libc::prctl(libc::PR_SET_DUMPABLE, 0, 0, 0, 0);
    }
}

#[cfg(not(unix))]
fn harden() {}

#[cfg(target_os = "linux")]
fn on_wsl() -> bool {
    std::env::var_os("WSL_DISTRO_NAME").is_some()
        || std::fs::read_to_string("/proc/sys/kernel/osrelease")
            .is_ok_and(|r| r.to_ascii_lowercase().contains("microsoft"))
}

#[cfg(target_os = "linux")]
#[allow(unsafe_code)]
fn webview_env() {
    let wsl = on_wsl();
    let nvidia = std::path::Path::new("/proc/driver/nvidia").exists();
    let mut vars = Vec::new();
    if wsl || nvidia {
        vars.push(("WEBKIT_DISABLE_DMABUF_RENDERER", "1"));
    }
    if wsl {
        vars.push(("WEBKIT_DISABLE_COMPOSITING_MODE", "1"));
    }
    for (key, value) in vars {
        if std::env::var_os(key).is_none() {
            unsafe { std::env::set_var(key, value) };
        }
    }
}

#[cfg(not(target_os = "linux"))]
fn webview_env() {}

fn setup(app: &mut tauri::App) -> Result<(), Box<dyn std::error::Error>> {
    let paths = app.path();
    let dirs = Dirs {
        data: paths.app_data_dir()?,
        config: paths.app_config_dir()?,
        cache: paths.app_cache_dir()?,
    };
    std::fs::create_dir_all(&dirs.data)?;
    match safety::adopt_orphaned_vault(&dirs.data) {
        Ok(Some(from)) => {
            tracing::warn!(from = %from.display(), "recovered vault from a previous app directory")
        }
        Ok(None) => {}
        Err(e) => tracing::warn!("could not look for a previous vault: {e}"),
    }
    match safety::preserve_before_upgrade(&dirs.data, env!("CARGO_PKG_VERSION")) {
        Ok(Some(copy)) => {
            tracing::info!(copy = %copy.display(), "saved a pre-upgrade copy of the vault")
        }
        Ok(None) => {}
        Err(e) => tracing::warn!("could not save a pre-upgrade copy: {e}"),
    }
    let prefs = Prefs::load(&dirs.config);
    let vault = Arc::new(Vault::open(dirs.data.join("vault.redb"))?);
    let idle = prefs.autolock();
    let autolock = tauri::async_runtime::block_on({
        let vault = vault.clone();
        async move { AutoLock::spawn(vault, idle) }
    });
    let core = Arc::new(Core::new(
        app.handle().clone(),
        vault,
        autolock,
        prefs,
        dirs,
    ));
    core.watch_vault();
    app.manage(AppState(core));
    Ok(())
}

pub fn run() {
    webview_env();
    harden();
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_env("CITADEL_LOG")
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn")),
        )
        .with_writer(std::io::stderr)
        .try_init();
    let built = tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_clipboard_manager::init())
        .setup(setup)
        .invoke_handler(tauri::generate_handler![
            cmd::vault::vault_status,
            cmd::vault::vault_create,
            cmd::vault::vault_unlock,
            cmd::vault::vault_lock,
            cmd::vault::vault_change_password,
            cmd::vault::vault_export,
            cmd::vault::vault_import,
            cmd::vault::backups_list,
            cmd::vault::backup_now,
            cmd::vault::backup_restore,
            cmd::hosts::hosts_list,
            cmd::hosts::host_save,
            cmd::hosts::host_delete,
            cmd::hosts::canvas_move,
            cmd::hosts::identities_list,
            cmd::hosts::identity_generate,
            cmd::hosts::identity_import,
            cmd::hosts::identity_import_file,
            cmd::hosts::identity_agent,
            cmd::hosts::identity_delete,
            cmd::hosts::agent_keys,
            cmd::hosts::snippets_list,
            cmd::hosts::snippet_save,
            cmd::hosts::snippet_delete,
            cmd::hosts::known_hosts_list,
            cmd::hosts::known_host_delete,
            cmd::term::pty_open,
            cmd::term::pty_write,
            cmd::term::pty_ack,
            cmd::term::pty_resize,
            cmd::term::pty_close,
            cmd::tunnels::tunnels_list,
            cmd::tunnels::tunnel_save,
            cmd::tunnels::tunnel_delete,
            cmd::tunnels::tunnel_start,
            cmd::tunnels::tunnel_stop,
            cmd::files::sftp_home,
            cmd::files::sftp_list,
            cmd::files::sftp_mkdir,
            cmd::files::sftp_remove,
            cmd::files::sftp_rename,
            cmd::files::sftp_download,
            cmd::files::sftp_upload,
            cmd::files::sftp_read,
            cmd::files::sftp_write,
            cmd::telemetry::telemetry_start,
            cmd::telemetry::telemetry_stop,
            cmd::config::sshconfig_hosts,
            cmd::config::sshconfig_import,
            cmd::config::sshconfig_export,
            cmd::config::agents_list,
            cmd::sync::sync_targets_list,
            cmd::sync::sync_target_save,
            cmd::sync::sync_target_delete,
            cmd::sync::sync_run,
            cmd::misc::prefs_get,
            cmd::misc::prefs_set,
            cmd::misc::prompt_answer,
            cmd::misc::links_list,
            cmd::misc::link_reconnect,
            cmd::audit::vault_audit,
            cmd::audit::recording_save
        ])
        .build(tauri::generate_context!());
    let app = match built {
        Ok(app) => app,
        Err(e) => {
            tracing::error!("failed to start: {e}");
            std::process::exit(1);
        }
    };
    app.run(|handle, event| {
        if let RunEvent::Exit = event
            && let Some(state) = handle.try_state::<AppState>()
        {
            state.0.shutdown();
        }
    });
}
