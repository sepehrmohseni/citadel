use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::error::CmdResult;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Prefs {
    pub autolock_minutes: u32,
    pub agent: Option<String>,
    pub font_family: String,
    pub font_size: u16,
    pub line_height: f32,
    pub scrollback: u32,
    pub cursor_blink: bool,
    pub cursor_style: String,
    pub theme: String,
    pub telemetry_ms: u32,
    pub paste_confirm_lines: u32,
    pub copy_on_select: bool,
    pub renderer: String,
}

impl Default for Prefs {
    fn default() -> Self {
        Self {
            autolock_minutes: 10,
            agent: None,
            font_family:
                "\"JetBrains Mono Variable\", \"JetBrains Mono\", \"Cascadia Code\", \"SF Mono\", Menlo, Consolas, monospace"
                    .into(),
            font_size: 13,
            line_height: 1.2,
            scrollback: 10_000,
            cursor_blink: true,
            cursor_style: "bar".into(),
            theme: "citadel".into(),
            telemetry_ms: 2000,
            paste_confirm_lines: 5,
            copy_on_select: false,
            renderer: "auto".into(),
        }
    }
}

impl Prefs {
    pub fn autolock(&self) -> Option<Duration> {
        (self.autolock_minutes > 0)
            .then(|| Duration::from_secs(u64::from(self.autolock_minutes) * 60))
    }

    pub fn sanitized(mut self) -> Self {
        self.font_size = self.font_size.clamp(8, 32);
        self.line_height = self.line_height.clamp(1.0, 2.0);
        self.scrollback = self.scrollback.clamp(500, 200_000);
        self.telemetry_ms = self.telemetry_ms.clamp(500, 60_000);
        self.autolock_minutes = self.autolock_minutes.min(24 * 60);
        if !matches!(self.renderer.as_str(), "auto" | "webgl" | "dom") {
            self.renderer = "auto".into();
        }
        self
    }

    pub fn path(dir: &Path) -> PathBuf {
        dir.join("prefs.json")
    }

    pub fn load(dir: &Path) -> Self {
        std::fs::read(Self::path(dir))
            .ok()
            .and_then(|raw| serde_json::from_slice::<serde_json::Value>(&raw).ok())
            .map(Self::lenient)
            .unwrap_or_default()
            .sanitized()
    }

    fn lenient(stored: serde_json::Value) -> Self {
        let Ok(serde_json::Value::Object(mut merged)) = serde_json::to_value(Self::default())
        else {
            return Self::default();
        };
        let serde_json::Value::Object(stored) = stored else {
            return Self::default();
        };
        for (key, value) in stored {
            if !merged.contains_key(&key) {
                continue;
            }
            let previous = merged.insert(key.clone(), value);
            if serde_json::from_value::<Self>(serde_json::Value::Object(merged.clone())).is_err()
                && let Some(p) = previous
            {
                merged.insert(key, p);
            }
        }
        serde_json::from_value(serde_json::Value::Object(merged)).unwrap_or_default()
    }

    pub fn save(&self, dir: &Path) -> CmdResult<()> {
        std::fs::create_dir_all(dir)?;
        let body = serde_json::to_vec_pretty(self)
            .map_err(|e| crate::error::AppError::invalid(e.to_string()))?;
        let tmp = dir.join("prefs.json.tmp");
        std::fs::write(&tmp, body)?;
        std::fs::rename(tmp, Self::path(dir))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_bad_field_does_not_reset_the_rest() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            Prefs::path(dir.path()),
            r#"{"font_size": 17, "theme": "ember", "scrollback": "lots", "removed_in_future": 1, "renderer": "quantum"}"#,
        )
        .unwrap();
        let p = Prefs::load(dir.path());
        assert_eq!(p.font_size, 17);
        assert_eq!(p.theme, "ember");
        assert_eq!(p.scrollback, Prefs::default().scrollback);
        assert_eq!(p.renderer, "auto");
    }

    #[test]
    fn garbage_falls_back_to_defaults() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(Prefs::path(dir.path()), b"{not json").unwrap();
        assert_eq!(Prefs::load(dir.path()), Prefs::default());
    }
}
