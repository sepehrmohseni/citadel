use tauri::AppHandle;
use tauri::ipc::Channel;

use crate::error::{AppError, CmdResult};

const MAX_DOWNLOAD: usize = 64 << 20;
const MAX_TEXT: usize = 200;

fn file_name(name: &str) -> CmdResult<String> {
    let clean: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || "._-".contains(c) {
                c
            } else {
                '_'
            }
        })
        .take(120)
        .collect();
    let clean = clean.trim_start_matches('.');
    let (stem, ext) = clean.rsplit_once('.').unwrap_or((clean, ""));
    if stem.is_empty() || !matches!(ext, "cast" | "txt") {
        return Err(AppError::invalid("only .cast and .txt files can be saved"));
    }
    Ok(clean.to_owned())
}

#[cfg(target_os = "android")]
mod native {
    use serde::{Deserialize, Serialize};
    use tauri::ipc::Channel;
    use tauri::{AppHandle, Manager};

    use crate::error::{AppError, CmdResult};
    use crate::mobile::Native;

    #[derive(Serialize)]
    struct KeepAlive<'a> {
        sessions: u32,
        text: &'a str,
    }

    #[derive(Serialize)]
    struct Watch {
        channel: Channel<serde_json::Value>,
    }

    #[derive(Serialize)]
    struct Download<'a> {
        name: &'a str,
        content: &'a str,
        mime: &'a str,
    }

    #[derive(Deserialize)]
    struct Saved {
        location: String,
    }

    fn plugin(app: &AppHandle) -> tauri::State<'_, Native<tauri::Wry>> {
        app.state::<Native<tauri::Wry>>()
    }

    pub fn keep_alive(app: &AppHandle, sessions: u32, text: &str) -> CmdResult<()> {
        plugin(app)
            .0
            .run_mobile_plugin::<serde_json::Value>("keepAlive", KeepAlive { sessions, text })
            .map_err(|e| AppError::invalid(e.to_string()))?;
        Ok(())
    }

    pub fn watch_disconnect(app: &AppHandle, feed: Channel<serde_json::Value>) -> CmdResult<()> {
        plugin(app)
            .0
            .run_mobile_plugin::<serde_json::Value>("watchDisconnect", Watch { channel: feed })
            .map_err(|e| AppError::invalid(e.to_string()))?;
        Ok(())
    }

    pub fn save(app: &AppHandle, name: &str, content: &str) -> CmdResult<String> {
        let mime = if name.ends_with(".cast") {
            "application/x-asciicast"
        } else {
            "text/plain"
        };
        let saved: Saved = plugin(app)
            .0
            .run_mobile_plugin(
                "saveDownload",
                Download {
                    name,
                    content,
                    mime,
                },
            )
            .map_err(|e| AppError::invalid(e.to_string()))?;
        Ok(saved.location)
    }
}

#[cfg(not(target_os = "android"))]
mod native {
    use tauri::AppHandle;
    use tauri::ipc::Channel;

    use crate::error::{AppError, CmdResult};

    pub fn keep_alive(_app: &AppHandle, _sessions: u32, _text: &str) -> CmdResult<()> {
        Ok(())
    }

    pub fn watch_disconnect(_app: &AppHandle, _feed: Channel<serde_json::Value>) -> CmdResult<()> {
        Ok(())
    }

    pub fn save(_app: &AppHandle, _name: &str, _content: &str) -> CmdResult<String> {
        Err(AppError::invalid(
            "saving to Downloads is only available on Android",
        ))
    }
}

#[tauri::command]
pub async fn keep_alive(app: AppHandle, sessions: u32, text: String) -> CmdResult<()> {
    let text: String = text.chars().take(MAX_TEXT).collect();
    native::keep_alive(&app, sessions, &text)
}

#[tauri::command]
pub async fn watch_disconnect(app: AppHandle, feed: Channel<serde_json::Value>) -> CmdResult<()> {
    native::watch_disconnect(&app, feed)
}

#[tauri::command]
pub async fn save_download(app: AppHandle, name: String, content: String) -> CmdResult<String> {
    let name = file_name(&name)?;
    if content.len() > MAX_DOWNLOAD {
        return Err(AppError::invalid("file exceeds 64 MiB"));
    }
    native::save(&app, &name, &content)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn download_names_are_flat_and_typed() {
        assert_eq!(
            file_name("crm-2026-10-30.cast").unwrap(),
            "crm-2026-10-30.cast"
        );
        assert_eq!(
            file_name("../../etc/passwd.txt").unwrap(),
            "_.._etc_passwd.txt"
        );
        assert_eq!(file_name("db 1/log.txt").unwrap(), "db_1_log.txt");
        assert!(file_name(".cast").is_err());
        assert!(file_name("evil.sh").is_err());
        assert!(file_name("noext").is_err());
    }
}
