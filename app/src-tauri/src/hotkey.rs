#[cfg(desktop)]
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
#[cfg(desktop)]
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;
use tauri::AppHandle;
#[cfg(desktop)]
use tauri::Manager;
#[cfg(desktop)]
use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut, ShortcutEvent, ShortcutState};

use crate::core::lock;
use crate::error::{AppError, CmdResult};

#[derive(Clone, Debug, Default, Serialize)]
pub struct HotkeyStatus {
    pub chord: String,
    pub active: bool,
    pub error: Option<String>,
}

pub fn accelerator(chord: &str) -> CmdResult<String> {
    let parts: Vec<&str> = chord.split('+').map(str::trim).collect();
    let Some((key, mods)) = parts.split_last() else {
        return Err(AppError::invalid("empty shortcut"));
    };
    if key.is_empty() || !mods.iter().any(|m| matches!(*m, "Ctrl" | "Alt" | "Meta")) {
        return Err(AppError::invalid(
            "a global shortcut needs Ctrl, Alt or the system key",
        ));
    }
    let mut out: Vec<&str> = mods
        .iter()
        .map(|m| match *m {
            "Meta" => Ok("Super"),
            "Ctrl" | "Alt" | "Shift" => Ok(*m),
            other => Err(AppError::invalid(format!("unknown modifier {other}"))),
        })
        .collect::<CmdResult<_>>()?;
    out.push(key);
    Ok(out.join("+"))
}

#[cfg(desktop)]
static FOCUSED: AtomicBool = AtomicBool::new(false);
#[cfg(desktop)]
static LOST_AT: AtomicU64 = AtomicU64::new(0);
#[cfg(desktop)]
const GRAB_GRACE_MS: u64 = 400;

#[cfg(desktop)]
fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}

#[cfg(desktop)]
pub fn focus_changed(on: bool) {
    if !on && FOCUSED.load(Ordering::SeqCst) {
        LOST_AT.store(now_ms(), Ordering::SeqCst);
    }
    FOCUSED.store(on, Ordering::SeqCst);
}

#[cfg(desktop)]
pub fn toggle(app: &AppHandle) {
    let Some(win) = app.get_webview_window("main") else {
        return;
    };
    let shown = win.is_visible().unwrap_or(false) && !win.is_minimized().unwrap_or(false);
    let recently = now_ms().saturating_sub(LOST_AT.load(Ordering::SeqCst)) < GRAB_GRACE_MS;
    let focused = FOCUSED.load(Ordering::SeqCst) || recently || win.is_focused().unwrap_or(false);
    if shown && focused {
        FOCUSED.store(false, Ordering::SeqCst);
        LOST_AT.store(0, Ordering::SeqCst);
        let _ = win.hide();
    } else {
        let _ = win.unminimize();
        let _ = win.show();
        let _ = win.set_focus();
    }
}

#[cfg(desktop)]
pub fn handler(app: &AppHandle, _shortcut: &Shortcut, event: ShortcutEvent) {
    if event.state() == ShortcutState::Pressed {
        toggle(app);
    }
}

#[cfg(desktop)]
pub fn apply(app: &AppHandle, slot: &std::sync::Mutex<HotkeyStatus>, chord: &str) -> HotkeyStatus {
    let gs = app.global_shortcut();
    let previous = lock(slot).clone();
    if previous.active
        && let Ok(old) = accelerator(&previous.chord)
    {
        let _ = gs.unregister(old.as_str());
    }
    let status = if chord.is_empty() {
        HotkeyStatus::default()
    } else {
        match accelerator(chord).and_then(|acc| {
            gs.register(acc.as_str())
                .map_err(|e| AppError::invalid(e.to_string()))
        }) {
            Ok(()) => HotkeyStatus {
                chord: chord.to_owned(),
                active: true,
                error: None,
            },
            Err(e) => HotkeyStatus {
                chord: chord.to_owned(),
                active: false,
                error: Some(e.to_string()),
            },
        }
    };
    *lock(slot) = status.clone();
    status
}

#[cfg(mobile)]
pub fn apply(_app: &AppHandle, slot: &std::sync::Mutex<HotkeyStatus>, chord: &str) -> HotkeyStatus {
    let status = HotkeyStatus {
        chord: chord.to_owned(),
        active: false,
        error: (!chord.is_empty())
            .then(|| "global shortcuts are not available on this device".to_owned()),
    };
    *lock(slot) = status.clone();
    status
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chords_become_accelerators() {
        assert_eq!(accelerator("Ctrl+Alt+Space").unwrap(), "Ctrl+Alt+Space");
        assert_eq!(accelerator("Meta+Shift+K").unwrap(), "Super+Shift+K");
        assert!(accelerator("Shift+K").is_err());
        assert!(accelerator("K").is_err());
        assert!(accelerator("Hyper+K").is_err());
    }
}
