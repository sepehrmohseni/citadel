use std::collections::BTreeMap;
use std::sync::OnceLock;

use serde::Serialize;

use super::blocking;
use crate::error::CmdResult;

#[derive(Clone, Debug, Serialize)]
pub struct FontFamily {
    pub name: String,
    pub mono: bool,
}

static FAMILIES: OnceLock<Vec<FontFamily>> = OnceLock::new();

fn scan() -> Vec<FontFamily> {
    let mut db = fontdb::Database::new();
    db.load_system_fonts();
    let mut seen: BTreeMap<String, bool> = BTreeMap::new();
    for face in db.faces() {
        let Some((name, _)) = face.families.first() else {
            continue;
        };
        let name = name.trim();
        if name.is_empty() || name.starts_with('.') || name.len() > 96 {
            continue;
        }
        let mono = seen.entry(name.to_owned()).or_insert(false);
        *mono |= face.monospaced;
    }
    let mut out: Vec<FontFamily> = seen
        .into_iter()
        .map(|(name, mono)| FontFamily { name, mono })
        .collect();
    out.sort_by(|a, b| {
        b.mono
            .cmp(&a.mono)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    out
}

#[tauri::command]
pub async fn fonts_list() -> CmdResult<Vec<FontFamily>> {
    if let Some(cached) = FAMILIES.get() {
        return Ok(cached.clone());
    }
    let found = blocking(|| Ok(scan())).await?;
    Ok(FAMILIES.get_or_init(|| found).clone())
}
