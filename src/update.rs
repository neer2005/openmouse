//! Self-update via GitHub Releases, driven from the tray menu.
//!
//! Only stable releases are ever offered: the check hits GitHub's
//! `/releases/latest` endpoint, which excludes drafts and prereleases, so
//! the rolling `latest` prerelease never triggers an update.
//!
//! The install replaces the running binary in place (Windows allows renaming
//! a running exe, which is what `self_update` relies on). There is no
//! console to prompt on, so the updater runs fully unattended.

use self_update::backends::github::Update;
use self_update::cargo_crate_version;

const REPO_OWNER: &str = "neer2005";
const REPO_NAME: &str = "openmouse";

fn updater(pinned_tag: Option<&str>) -> Result<Update, self_update::Error> {
    let mut b = Update::configure();
    b.repo_owner(REPO_OWNER)
        .repo_name(REPO_NAME)
        .bin_name("openmouse")
        // The release carries a single `openmouse.exe` asset, which matches
        // neither the target triple nor arch/os tokens, so select it by name.
        .asset_identifier("openmouse")
        .current_version(cargo_crate_version!())
        // No console: never block on a stdin prompt, never print a status block.
        .unattended()
        .show_download_progress(false);
    if let Some(tag) = pinned_tag {
        b.release_tag(tag);
    }
    b.build()
}

/// Check for a newer stable release. Returns the new version, or `None` when
/// up to date. Blocking: call off any async runtime / UI thread.
pub fn check() -> Result<Option<String>, String> {
    let releases = updater(None)
        .and_then(|u| u.get_latest_release())
        .map_err(|e| e.to_string())?;
    if releases.is_update_available().map_err(|e| e.to_string())? {
        Ok(releases.latest().map(|r| r.version().to_owned()))
    } else {
        Ok(None)
    }
}

/// Download and install `version`, replacing this binary on disk. The tags in
/// this repo are `v`-prefixed (`v0.2.0`), so pin the exact tag that was
/// offered — the unpinned path could otherwise pick a different release.
/// Blocking: call off any async runtime / UI thread.
///
/// On success the caller should relaunch the new binary and exit; the
/// pairing token is fresh every run, so the phone re-pairs via the QR popup.
pub fn install(version: &str) -> Result<(), String> {
    let status = updater(Some(&format!("v{version}")))
        .and_then(|u| u.update())
        .map_err(|e| e.to_string())?;
    if status.is_updated() {
        Ok(())
    } else {
        Err("update finished without replacing the binary".to_string())
    }
}
