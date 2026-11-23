//! Bug reports. The text the user writes in Citadel is placed straight into a new GitHub issue,
//! opened in the browser, where they review it and press Submit. Filing it with no browser step
//! would need a GitHub token inside the app, and anyone could pull that token out and abuse it.

use serde::Serialize;
use tauri::{AppHandle, Url};
use tauri_plugin_opener::OpenerExt;

use crate::error::{AppError, CmdResult};

const NEW_ISSUE: &str = "https://github.com/sepehrmohseni/citadel/issues/new";
/// GitHub refuses longer links; stay well under its limit.
const MAX_URL: usize = 7000;
const CUT_NOTE: &str =
    "\n\n(The report was shortened to fit in a link. Paste the rest here before submitting.)";

#[derive(Serialize)]
pub struct SupportInfo {
    pub version: String,
    pub os: String,
    pub arch: String,
    pub package: String,
}

fn info() -> SupportInfo {
    SupportInfo {
        version: env!("CARGO_PKG_VERSION").to_owned(),
        os: std::env::consts::OS.to_owned(),
        arch: std::env::consts::ARCH.to_owned(),
        package: crate::updates::package(),
    }
}

/// The version, system and package type a bug report includes, so the form can show exactly what
/// will be sent.
#[tauri::command]
pub async fn support_info() -> CmdResult<SupportInfo> {
    Ok(info())
}

/// Opens a new GitHub issue with the user's title and text already filled in.
#[tauri::command]
pub async fn issue_open(
    app: AppHandle,
    title: String,
    description: String,
    details: bool,
) -> CmdResult<()> {
    let url = issue_url(&title, &description, details.then(info).as_ref())?;
    app.opener()
        .open_url(url.as_str(), None::<&str>)
        .map_err(|e| AppError::invalid(e.to_string()))
}

fn issue_url(title: &str, description: &str, info: Option<&SupportInfo>) -> CmdResult<Url> {
    let title: String = title
        .chars()
        .filter(|c| !c.is_control())
        .collect::<String>()
        .trim()
        .chars()
        .take(200)
        .collect();
    if title.chars().count() < 3 {
        return Err(AppError::invalid("give the report a short title"));
    }
    let mut text = description.trim().replace("\r\n", "\n");
    if text.is_empty() {
        return Err(AppError::invalid("describe what happened"));
    }
    let footer = info.map_or_else(String::new, |i| {
        format!(
            "\n\n---\nCitadel {} · {} {} · {}",
            i.version, i.os, i.arch, i.package
        )
    });
    let mut cut = false;
    loop {
        let body = if cut {
            format!("{text}{CUT_NOTE}{footer}")
        } else {
            format!("{text}{footer}")
        };
        let mut url = Url::parse(NEW_ISSUE).map_err(|e| AppError::invalid(e.to_string()))?;
        url.query_pairs_mut()
            .append_pair("title", &title)
            .append_pair("body", &body)
            .append_pair("labels", "bug");
        if url.as_str().len() <= MAX_URL {
            return Ok(url);
        }
        // Drop the last tenth of the text and try again, on a character boundary.
        let keep = text.chars().count() * 9 / 10;
        if keep == 0 {
            return Err(AppError::invalid("the title is too long"));
        }
        text = text.chars().take(keep).collect();
        cut = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> SupportInfo {
        SupportInfo {
            version: "0.1.7".into(),
            os: "linux".into(),
            arch: "x86_64".into(),
            package: "deb".into(),
        }
    }

    fn pairs(url: &Url) -> Vec<(String, String)> {
        url.query_pairs()
            .map(|(k, v)| (k.into_owned(), v.into_owned()))
            .collect()
    }

    #[test]
    fn reports_open_a_prefilled_issue_on_the_repository() {
        let url = issue_url(
            "Ctrl+B types bs",
            "In tmux, & and = and # stay text.\r\nSecond line",
            Some(&sample()),
        )
        .unwrap();
        assert!(url.as_str().starts_with(NEW_ISSUE));
        let q = pairs(&url);
        assert_eq!(q[0], ("title".into(), "Ctrl+B types bs".into()));
        assert_eq!(q[2], ("labels".into(), "bug".into()));
        assert!(
            q[1].1
                .starts_with("In tmux, & and = and # stay text.\nSecond line")
        );
        assert!(q[1].1.ends_with("Citadel 0.1.7 · linux x86_64 · deb"));
        let bare = issue_url("Title", "Text", None).unwrap();
        assert_eq!(pairs(&bare)[1].1, "Text");
    }

    #[test]
    fn long_reports_are_shortened_to_fit_a_link() {
        let long = "سلام دنیا ".repeat(2000);
        let url = issue_url("Long", &long, Some(&sample())).unwrap();
        assert!(url.as_str().len() <= MAX_URL);
        let body = &pairs(&url)[1].1;
        assert!(body.contains("The report was shortened"));
        assert!(body.ends_with("deb"));
    }

    #[test]
    fn empty_reports_are_refused() {
        assert!(issue_url("ab", "text", None).is_err());
        assert!(issue_url("Title", "   ", None).is_err());
        assert!(issue_url("\u{7}\u{7}\u{7}", "text", None).is_err());
    }
}
