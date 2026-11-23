//! Update management. Citadel looks for a newer release on GitHub shortly after it starts and then
//! every few hours. Desktop builds download a signed update in the background, verify it against
//! the public key built into the app, and install it when the user restarts or quits; nothing is
//! ever restarted without the user. Copies that cannot replace themselves (Android, development
//! builds, or a release published without update signatures) say that a new version is out and
//! link to it.

use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, State};
use tauri_plugin_opener::OpenerExt;

use crate::cmd::{AppState, blocking};
use crate::core::{Core, lock};
use crate::error::{AppError, CmdResult};

pub const EV_UPDATE: &str = "citadel://update";
const RELEASES: &str = "https://github.com/sepehrmohseni/citadel/releases";
const LATEST_API: &str = "https://api.github.com/repos/sepehrmohseni/citadel/releases/latest";
const FIRST_CHECK: Duration = Duration::from_secs(20);
const EVERY: Duration = Duration::from_secs(6 * 60 * 60);
const RETRY: Duration = Duration::from_secs(30 * 60);
const MAX_RELEASE_JSON: usize = 2 << 20;
const MAX_NOTES: usize = 4000;

/// How a found update gets onto this machine.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Install {
    /// Installs without asking (AppImage, Windows installers, macOS app) and restarts Citadel.
    Restart,
    /// A system package (.deb or .rpm): installing asks for the system password.
    Password,
    /// This copy cannot replace itself; the user downloads the new version.
    Manual,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum UpdateState {
    Idle,
    Checking,
    UpToDate {
        checked_ms: u64,
    },
    Available {
        version: String,
        notes: String,
        url: String,
        install: Install,
    },
    Downloading {
        version: String,
        done: u64,
        total: Option<u64>,
    },
    Ready {
        version: String,
        notes: String,
        url: String,
        install: Install,
    },
    Installing {
        version: String,
    },
    Failed {
        error: String,
        checked_ms: u64,
    },
}

struct Found {
    version: String,
    notes: String,
    url: String,
    install: Install,
}

#[cfg(desktop)]
struct Pending {
    update: tauri_plugin_updater::Update,
    bytes: Option<Vec<u8>>,
}

pub struct Updates {
    app: AppHandle,
    state: Mutex<UpdateState>,
    busy: AtomicBool,
    #[cfg(desktop)]
    pending: Mutex<Option<Pending>>,
}

/// Clears the busy flag when a check or download ends, however it ends.
struct Busy<'a>(&'a AtomicBool);

impl Drop for Busy<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
    }
}

impl Updates {
    pub fn new(app: AppHandle) -> Arc<Self> {
        Arc::new(Self {
            app,
            state: Mutex::new(UpdateState::Idle),
            busy: AtomicBool::new(false),
            #[cfg(desktop)]
            pending: Mutex::new(None),
        })
    }

    pub fn state(&self) -> UpdateState {
        lock(&self.state).clone()
    }

    fn set(&self, next: UpdateState) {
        *lock(&self.state) = next.clone();
        let _ = self.app.emit(EV_UPDATE, next);
    }

    fn claim(&self) -> Option<Busy<'_>> {
        (!self.busy.swap(true, Ordering::SeqCst)).then_some(Busy(&self.busy))
    }

    /// Looks for a newer release. A version the user skipped is ignored unless `manual`.
    pub async fn check(&self, manual: bool, skip: &str) -> UpdateState {
        let current = self.state();
        if matches!(
            current,
            UpdateState::Downloading { .. }
                | UpdateState::Ready { .. }
                | UpdateState::Installing { .. }
        ) {
            return current;
        }
        let Some(_busy) = self.claim() else {
            return current;
        };
        self.set(UpdateState::Checking);
        let next = match self.find().await {
            Ok(Some(found)) if manual || found.version != skip => UpdateState::Available {
                version: found.version,
                notes: found.notes,
                url: found.url,
                install: found.install,
            },
            Ok(_) => UpdateState::UpToDate {
                checked_ms: now_ms(),
            },
            Err(error) => UpdateState::Failed {
                error,
                checked_ms: now_ms(),
            },
        };
        self.set(next.clone());
        next
    }

    #[cfg(desktop)]
    async fn find(&self) -> Result<Option<Found>, String> {
        use tauri_plugin_updater::UpdaterExt;
        let signed = match self.app.updater() {
            Ok(updater) => updater.check().await.map_err(|e| e.to_string()),
            Err(e) => Err(e.to_string()),
        };
        match signed {
            Ok(Some(update)) => {
                let found = Found {
                    version: update.version.clone(),
                    notes: notes(update.body.as_deref().unwrap_or_default()),
                    url: format!("{RELEASES}/tag/v{}", update.version),
                    install: install_kind(),
                };
                *lock(&self.pending) = Some(Pending {
                    update,
                    bytes: None,
                });
                Ok(Some(found))
            }
            Ok(None) => Ok(None),
            Err(e) => {
                // No signed update information (for example a release published without update
                // signatures): fall back to telling the user about the release.
                tracing::info!("signed update check failed, asking GitHub instead: {e}");
                *lock(&self.pending) = None;
                let raw = citadel_sync::fetch(
                    LATEST_API,
                    "application/vnd.github+json",
                    MAX_RELEASE_JSON,
                )
                .await
                .map_err(|e| e.to_string())?;
                newer_release(&raw, env!("CARGO_PKG_VERSION"), false)
            }
        }
    }

    #[cfg(mobile)]
    async fn find(&self) -> Result<Option<Found>, String> {
        let app = self.app.clone();
        let raw =
            tauri::async_runtime::spawn_blocking(move || crate::cmd::device::latest_release(&app))
                .await
                .map_err(|e| e.to_string())?
                .map_err(|e| e.to_string())?;
        newer_release(raw.as_bytes(), env!("CARGO_PKG_VERSION"), true)
    }

    /// Downloads and verifies the update found by the last check.
    #[cfg(desktop)]
    pub async fn download(&self) -> CmdResult<()> {
        let UpdateState::Available {
            version,
            notes,
            url,
            install,
        } = self.state()
        else {
            return Ok(());
        };
        if install == Install::Manual {
            return Err(AppError::invalid(
                "this copy of Citadel cannot update itself; download the new version instead",
            ));
        }
        let Some(update) = lock(&self.pending).as_ref().map(|p| p.update.clone()) else {
            return Err(AppError::invalid("check for updates again first"));
        };
        let Some(_busy) = self.claim() else {
            return Ok(());
        };
        self.set(UpdateState::Downloading {
            version: version.clone(),
            done: 0,
            total: None,
        });
        let mut done = 0u64;
        let mut shown = 0u64;
        let result = update
            .download(
                |chunk, total| {
                    done += chunk as u64;
                    // Report progress about every 2% (or 1 MiB when the size is unknown).
                    let step = total.map_or(1 << 20, |t| (t / 50).max(1));
                    if done - shown >= step {
                        shown = done;
                        self.set(UpdateState::Downloading {
                            version: version.clone(),
                            done,
                            total,
                        });
                    }
                },
                || {},
            )
            .await;
        match result {
            Ok(bytes) => {
                if let Some(p) = lock(&self.pending).as_mut() {
                    p.bytes = Some(bytes);
                }
                self.set(UpdateState::Ready {
                    version,
                    notes,
                    url,
                    install,
                });
                Ok(())
            }
            Err(e) => {
                self.set(UpdateState::Failed {
                    error: format!("downloading Citadel {version} failed: {e}"),
                    checked_ms: now_ms(),
                });
                Err(AppError::invalid(e.to_string()))
            }
        }
    }

    #[cfg(mobile)]
    pub async fn download(&self) -> CmdResult<()> {
        Err(AppError::invalid(
            "updates are installed from the downloaded APK on Android",
        ))
    }

    /// Installs the downloaded update, then restarts Citadel. Blocking: on Linux packages this
    /// waits for the system password prompt.
    #[cfg(desktop)]
    pub fn install_now(&self) -> CmdResult<()> {
        let pending = lock(&self.pending).take();
        let Some(Pending {
            update,
            bytes: Some(bytes),
        }) = pending
        else {
            return Err(AppError::invalid("the update has not finished downloading"));
        };
        let previous = self.state();
        self.set(UpdateState::Installing {
            version: update.version.clone(),
        });
        if let Err(e) = update.install(&bytes) {
            // Keep the download so the user can try again.
            *lock(&self.pending) = Some(Pending {
                update,
                bytes: Some(bytes),
            });
            self.set(previous);
            return Err(AppError::invalid(format!(
                "installing the update failed: {e}"
            )));
        }
        if let Some(state) = self.app.try_state::<AppState>() {
            state.0.shutdown();
        }
        self.app.restart();
    }

    #[cfg(mobile)]
    pub fn install_now(&self) -> CmdResult<()> {
        Err(AppError::invalid(
            "updates are installed from the downloaded APK on Android",
        ))
    }

    /// Called when Citadel quits: a downloaded update that needs no password is installed now, so
    /// the next start runs the new version. Updates are only ever downloaded automatically (in
    /// automatic mode) or because the user asked, so either way they want it.
    pub fn install_on_exit(&self) {
        #[cfg(desktop)]
        {
            if install_kind() != Install::Restart {
                return;
            }
            let pending = lock(&self.pending).take();
            if let Some(Pending {
                update,
                bytes: Some(bytes),
            }) = pending
                && let Err(e) = update.install(&bytes)
            {
                tracing::warn!("could not install the downloaded update on exit: {e}");
            }
        }
    }

    /// The page to open for an update the user installs by hand.
    fn link(&self) -> String {
        match self.state() {
            UpdateState::Available { url, .. } | UpdateState::Ready { url, .. } => url,
            _ => RELEASES.to_owned(),
        }
    }
}

/// Starts the background schedule: a first look shortly after start, then every few hours.
/// Development builds (and `CITADEL_NO_UPDATE_CHECK`) never check on their own.
pub fn schedule(updates: Arc<Updates>, core: Arc<Core>) {
    if cfg!(debug_assertions) || std::env::var_os("CITADEL_NO_UPDATE_CHECK").is_some() {
        return;
    }
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(FIRST_CHECK).await;
        loop {
            let prefs = core.prefs();
            let wait = if prefs.updates == "off" {
                EVERY
            } else {
                match updates.check(false, &prefs.skip_version).await {
                    UpdateState::Available { install, .. }
                        if prefs.updates == "auto" && install != Install::Manual =>
                    {
                        if updates.download().await.is_ok() {
                            EVERY
                        } else {
                            RETRY
                        }
                    }
                    UpdateState::Failed { .. } => RETRY,
                    _ => EVERY,
                }
            };
            tokio::time::sleep(wait).await;
        }
    });
}

#[cfg(desktop)]
fn install_kind() -> Install {
    use tauri::utils::config::BundleType;
    if cfg!(debug_assertions) {
        return Install::Manual;
    }
    match tauri::utils::platform::bundle_type() {
        Some(BundleType::Deb | BundleType::Rpm) => Install::Password,
        Some(_) => Install::Restart,
        None => Install::Manual,
    }
}

/// How this copy was installed, for bug reports.
pub fn package() -> String {
    #[cfg(desktop)]
    {
        match tauri::utils::platform::bundle_type() {
            Some(kind) => format!("{kind:?}").to_lowercase(),
            None => "development build".into(),
        }
    }
    #[cfg(target_os = "android")]
    {
        "apk".into()
    }
    #[cfg(all(mobile, not(target_os = "android")))]
    {
        "mobile".into()
    }
}

#[derive(Deserialize)]
struct Release {
    tag_name: String,
    #[serde(default)]
    html_url: String,
    #[serde(default)]
    body: Option<String>,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    prerelease: bool,
    #[serde(default)]
    assets: Vec<Asset>,
}

#[derive(Deserialize)]
struct Asset {
    name: String,
    browser_download_url: String,
}

/// Reads GitHub's "latest release" answer and returns it when it is newer than `current`. On
/// Android the link points straight at the APK.
fn newer_release(raw: &[u8], current: &str, apk: bool) -> Result<Option<Found>, String> {
    let r: Release =
        serde_json::from_slice(raw).map_err(|e| format!("unexpected answer from GitHub: {e}"))?;
    let version = r.tag_name.trim_start_matches('v').to_owned();
    if r.draft || r.prerelease || !newer(&version, current) {
        return Ok(None);
    }
    let ours = |u: &str| u.starts_with("https://github.com/sepehrmohseni/citadel/");
    let page = if ours(&r.html_url) {
        r.html_url.clone()
    } else {
        format!("{RELEASES}/tag/{}", r.tag_name)
    };
    let url = if apk {
        r.assets
            .iter()
            .find(|a| a.name.ends_with("_android.apk") && ours(&a.browser_download_url))
            .map_or(page, |a| a.browser_download_url.clone())
    } else {
        page
    };
    Ok(Some(Found {
        version,
        notes: notes(r.body.as_deref().unwrap_or_default()),
        url,
        install: Install::Manual,
    }))
}

fn notes(text: &str) -> String {
    let text = text.trim();
    match text.char_indices().nth(MAX_NOTES) {
        Some((cut, _)) => format!("{}…", &text[..cut]),
        None => text.to_owned(),
    }
}

/// Whether `candidate` is a newer release than `current`. Pre-releases ("1.2.0-rc.1") never are.
fn newer(candidate: &str, current: &str) -> bool {
    fn parts(v: &str) -> Option<[u64; 3]> {
        if v.contains('-') {
            return None;
        }
        let mut out = [0u64; 3];
        let mut it = v.split('.');
        for slot in &mut out {
            *slot = it.next()?.parse().ok()?;
        }
        it.next().is_none().then_some(out)
    }
    // A pre-release of the running version ("0.1.8-dev") is older than that release.
    let (core, pre) = current
        .split_once('-')
        .map_or((current, false), |(c, _)| (c, true));
    match (parts(candidate), parts(core)) {
        (Some(a), Some(b)) => a > b || (pre && a == b),
        _ => false,
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}

#[tauri::command]
pub async fn update_status(updates: State<'_, Arc<Updates>>) -> CmdResult<UpdateState> {
    Ok(updates.state())
}

#[tauri::command]
pub async fn update_check(
    state: State<'_, AppState>,
    updates: State<'_, Arc<Updates>>,
) -> CmdResult<UpdateState> {
    let skip = state.0.prefs().skip_version;
    Ok(updates.check(true, &skip).await)
}

/// Starts downloading the found update; progress arrives as `citadel://update` events.
#[tauri::command]
pub async fn update_download(updates: State<'_, Arc<Updates>>) -> CmdResult<()> {
    let updates = updates.inner().clone();
    tauri::async_runtime::spawn(async move {
        let _ = updates.download().await;
    });
    Ok(())
}

#[tauri::command]
pub async fn update_install(updates: State<'_, Arc<Updates>>) -> CmdResult<()> {
    let updates = updates.inner().clone();
    blocking(move || updates.install_now()).await
}

/// Opens the release page (or, on Android, the APK) for an update installed by hand.
#[tauri::command]
pub async fn update_open(app: AppHandle, updates: State<'_, Arc<Updates>>) -> CmdResult<()> {
    app.opener()
        .open_url(updates.link(), None::<&str>)
        .map_err(|e| AppError::invalid(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_compare_numerically() {
        assert!(newer("0.1.10", "0.1.9"));
        assert!(newer("0.2.0", "0.1.99"));
        assert!(newer("1.0.0", "0.9.9"));
        assert!(!newer("0.1.7", "0.1.7"));
        assert!(!newer("0.1.6", "0.1.7"));
        assert!(!newer("0.2.0-rc.1", "0.1.7"));
        assert!(!newer("0.2", "0.1.7"));
        assert!(!newer("garbage", "0.1.7"));
        assert!(newer("0.1.8", "0.1.8-dev"));
    }

    #[test]
    fn github_answers_become_manual_updates() {
        let raw = br#"{
            "tag_name": "v0.2.0",
            "html_url": "https://github.com/sepehrmohseni/citadel/releases/tag/v0.2.0",
            "body": "Fixes",
            "draft": false,
            "prerelease": false,
            "assets": [
                {"name": "Citadel_0.2.0_android.apk", "browser_download_url": "https://github.com/sepehrmohseni/citadel/releases/download/v0.2.0/Citadel_0.2.0_android.apk"},
                {"name": "evil_android.apk", "browser_download_url": "https://evil.example/x.apk"}
            ]
        }"#;
        let desk = newer_release(raw, "0.1.7", false).unwrap().unwrap();
        assert_eq!(desk.version, "0.2.0");
        assert_eq!(desk.install, Install::Manual);
        assert!(desk.url.ends_with("/releases/tag/v0.2.0"));
        let phone = newer_release(raw, "0.1.7", true).unwrap().unwrap();
        assert!(phone.url.ends_with("/v0.2.0/Citadel_0.2.0_android.apk"));
        assert!(newer_release(raw, "0.2.0", false).unwrap().is_none());
        let foreign =
            br#"{"tag_name": "v9.0.0", "html_url": "https://evil.example/", "assets": []}"#;
        let found = newer_release(foreign, "0.1.7", true).unwrap().unwrap();
        assert_eq!(found.url, format!("{RELEASES}/tag/v9.0.0"));
        assert!(newer_release(b"not json", "0.1.7", false).is_err());
    }

    #[test]
    fn long_notes_are_cut_on_a_character_boundary() {
        let long = "ی".repeat(MAX_NOTES + 10);
        let cut = notes(&long);
        assert_eq!(cut.chars().count(), MAX_NOTES + 1);
        assert!(cut.ends_with('…'));
    }
}
