use serde::Deserialize;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

const GITHUB_REPO: &str = "erlunlian/operator";
const CHECK_INTERVAL_SECS: u64 = 3600; // 1 hour

#[derive(Deserialize)]
struct GitHubRelease {
    tag_name: String,
    assets: Vec<GitHubAsset>,
}

#[derive(Deserialize)]
struct GitHubAsset {
    name: String,
    browser_download_url: String,
}

#[derive(Clone)]
pub struct UpdateInfo {
    pub latest_version: String,
    /// URL to the release page or DMG — used as a user-facing fallback link.
    pub download_url: String,
    /// URL to a `.zip` of `Operator.app` for in-app auto-update.
    /// `None` for older releases that only ship a DMG.
    pub zip_url: Option<String>,
}

/// Which stage of the apply-update pipeline is currently running.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum InstallPhase {
    #[default]
    Starting,
    Downloading,
    Extracting,
    Finalizing,
}

/// Live progress shared between the background updater thread and the UI.
#[derive(Clone, Copy, Debug, Default)]
pub struct ProgressState {
    pub phase: InstallPhase,
    pub bytes_downloaded: u64,
    /// `None` until the download response provides Content-Length.
    pub total_bytes: Option<u64>,
}

/// Check GitHub releases for a newer version. Runs synchronously (call from background thread).
/// Skips the network request if the last check was less than an hour ago, unless `force` is true.
pub fn check_for_update(current_version: &str, force: bool) -> Option<UpdateInfo> {
    if !force && !should_check() {
        return None;
    }

    let url = format!(
        "https://api.github.com/repos/{GITHUB_REPO}/releases/latest"
    );

    let agent = ureq::Agent::config_builder()
        .timeout_global(Some(std::time::Duration::from_secs(10)))
        .user_agent("operator-updater")
        .build()
        .new_agent();

    let response: GitHubRelease = agent
        .get(&url)
        .header("Accept", "application/vnd.github+json")
        .call()
        .ok()?
        .body_mut()
        .read_json()
        .ok()?;

    record_check();

    let latest = response.tag_name.trim_start_matches('v');
    let current = current_version.trim_start_matches('v');

    if version_newer(latest, current) {
        let dmg_url = response
            .assets
            .iter()
            .find(|a| a.name.ends_with(".dmg"))
            .map(|a| a.browser_download_url.clone())
            .unwrap_or_else(|| {
                format!(
                    "https://github.com/{GITHUB_REPO}/releases/tag/{}",
                    response.tag_name
                )
            });
        let zip_url = response
            .assets
            .iter()
            .find(|a| a.name.ends_with(".zip"))
            .map(|a| a.browser_download_url.clone());

        Some(UpdateInfo {
            latest_version: latest.to_string(),
            download_url: dmg_url,
            zip_url,
        })
    } else {
        None
    }
}

/// Download the update zip, extract it, and spawn a detached helper
/// that swaps the `.app` bundle in place and relaunches once this
/// process exits. The caller should quit the app immediately after
/// this returns `Ok`.
///
/// `progress` is updated in place throughout the pipeline; the caller
/// polls it from the UI thread to drive a progress bar.
///
/// Runs synchronously (shells out to `ditto`) — call from a
/// background thread.
pub fn apply_update(info: &UpdateInfo, progress: &Mutex<ProgressState>) -> Result<(), String> {
    let zip_url = info
        .zip_url
        .as_deref()
        .ok_or_else(|| "no zip asset for this release".to_string())?;

    let target_app = current_app_bundle()
        .ok_or_else(|| "not running from a .app bundle".to_string())?;

    let temp_dir = make_temp_dir()?;
    let zip_path = temp_dir.join("update.zip");
    let extract_dir = temp_dir.join("extracted");
    std::fs::create_dir(&extract_dir).map_err(|e| format!("mkdir extract: {e}"))?;

    set_phase(progress, InstallPhase::Downloading);
    download_to_file(zip_url, &zip_path, progress)?;

    set_phase(progress, InstallPhase::Extracting);
    let status = std::process::Command::new("ditto")
        .arg("-x")
        .arg("-k")
        .arg(&zip_path)
        .arg(&extract_dir)
        .status()
        .map_err(|e| format!("spawn ditto: {e}"))?;
    if !status.success() {
        return Err(format!("ditto exited with {status}"));
    }

    set_phase(progress, InstallPhase::Finalizing);
    let staged_app = find_app_in(&extract_dir)?;
    let script_path = temp_dir.join("apply.sh");
    std::fs::write(&script_path, APPLY_SH).map_err(|e| format!("write apply.sh: {e}"))?;
    use std::os::unix::fs::PermissionsExt;
    let mut perms = std::fs::metadata(&script_path)
        .map_err(|e| format!("stat apply.sh: {e}"))?
        .permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&script_path, perms)
        .map_err(|e| format!("chmod apply.sh: {e}"))?;

    let pid = std::process::id().to_string();
    std::process::Command::new("/bin/bash")
        .arg(&script_path)
        .arg(&pid)
        .arg(&staged_app)
        .arg(&target_app)
        .arg(&temp_dir)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|e| format!("spawn apply.sh: {e}"))?;

    Ok(())
}

/// Helper that waits for the parent PID to exit, then atomically
/// swaps the app bundle and relaunches. Kept out of band so it
/// survives the parent's termination.
const APPLY_SH: &str = r#"#!/bin/bash
set +e
PARENT_PID="$1"
STAGED_APP="$2"
TARGET_APP="$3"
TEMP_DIR="$4"

LOG="$TEMP_DIR/apply.log"
exec > "$LOG" 2>&1
set -x

# Wait up to 30s for the old app to quit.
for i in $(seq 1 60); do
  if ! kill -0 "$PARENT_PID" 2>/dev/null; then
    break
  fi
  sleep 0.5
done

BAK="${TARGET_APP}.old"
if [ -d "$TARGET_APP" ]; then
  rm -rf "$BAK"
  if ! mv "$TARGET_APP" "$BAK"; then
    echo "failed to move old bundle aside"
    open -n "$TARGET_APP"
    exit 1
  fi
fi

if ! ditto "$STAGED_APP" "$TARGET_APP"; then
  echo "ditto failed; restoring backup"
  rm -rf "$TARGET_APP"
  mv "$BAK" "$TARGET_APP"
  open -n "$TARGET_APP"
  exit 1
fi

xattr -cr "$TARGET_APP" 2>/dev/null || true
rm -rf "$BAK"
open -n "$TARGET_APP"

sleep 2
rm -rf "$TEMP_DIR"
"#;

/// Walk up from the running executable to find the enclosing
/// `.app` bundle (e.g. `.../Operator.app/Contents/MacOS/operator`
/// → `.../Operator.app`). Returns `None` when running a raw
/// `cargo run` binary.
fn current_app_bundle() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?.canonicalize().ok()?;
    let mut cur = exe.as_path();
    while let Some(parent) = cur.parent() {
        if parent.extension().and_then(|e| e.to_str()) == Some("app") {
            return Some(parent.to_path_buf());
        }
        cur = parent;
    }
    None
}

fn make_temp_dir() -> Result<PathBuf, String> {
    let base = std::env::temp_dir().join(format!(
        "operator-update-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0)
    ));
    std::fs::create_dir_all(&base).map_err(|e| format!("mkdir temp: {e}"))?;
    Ok(base)
}

fn download_to_file(
    url: &str,
    dest: &Path,
    progress: &Mutex<ProgressState>,
) -> Result<(), String> {
    let agent = ureq::Agent::config_builder()
        .timeout_global(Some(std::time::Duration::from_secs(120)))
        .user_agent("operator-updater")
        .build()
        .new_agent();
    let mut resp = agent
        .get(url)
        .call()
        .map_err(|e| format!("download: {e}"))?;

    let total_bytes = resp
        .headers()
        .get("content-length")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse::<u64>().ok());
    {
        let mut p = progress.lock().unwrap();
        p.total_bytes = total_bytes;
        p.bytes_downloaded = 0;
    }

    let mut reader = resp.body_mut().as_reader();
    let mut file = std::fs::File::create(dest).map_err(|e| format!("create file: {e}"))?;
    // 64 KiB buffer: large enough to keep syscall overhead low, small enough to
    // update the progress bar smoothly on fast connections.
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let n = reader.read(&mut buf).map_err(|e| format!("read: {e}"))?;
        if n == 0 {
            break;
        }
        file.write_all(&buf[..n]).map_err(|e| format!("write file: {e}"))?;
        progress.lock().unwrap().bytes_downloaded += n as u64;
    }
    file.flush().map_err(|e| format!("flush: {e}"))?;
    Ok(())
}

fn set_phase(progress: &Mutex<ProgressState>, phase: InstallPhase) {
    progress.lock().unwrap().phase = phase;
}

fn find_app_in(dir: &Path) -> Result<PathBuf, String> {
    let entries = std::fs::read_dir(dir).map_err(|e| format!("read_dir: {e}"))?;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) == Some("app") {
            return Ok(path);
        }
    }
    Err(format!("no .app found in {}", dir.display()))
}

/// Returns true if enough time has passed since the last check.
fn should_check() -> bool {
    let path = cache_path();
    let contents = std::fs::read_to_string(&path).unwrap_or_default();
    let last: u64 = contents.trim().parse().unwrap_or(0);
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    now.saturating_sub(last) >= CHECK_INTERVAL_SECS
}

/// Record the current time as the last check timestamp.
fn record_check() {
    let path = cache_path();
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let _ = std::fs::write(&path, now.to_string());
}

fn cache_path() -> std::path::PathBuf {
    dirs::cache_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("/tmp"))
        .join("operator-last-update-check")
}

/// Simple semver comparison: returns true if `a` is newer than `b`.
fn version_newer(a: &str, b: &str) -> bool {
    let parse = |s: &str| -> Vec<u32> {
        s.split('.')
            .filter_map(|p| p.parse().ok())
            .collect()
    };
    let va = parse(a);
    let vb = parse(b);
    for i in 0..va.len().max(vb.len()) {
        let pa = va.get(i).copied().unwrap_or(0);
        let pb = vb.get(i).copied().unwrap_or(0);
        if pa > pb {
            return true;
        }
        if pa < pb {
            return false;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_version_newer() {
        assert!(version_newer("0.2.0", "0.1.0"));
        assert!(version_newer("1.0.0", "0.9.9"));
        assert!(version_newer("0.1.1", "0.1.0"));
        assert!(!version_newer("0.1.0", "0.1.0"));
        assert!(!version_newer("0.1.0", "0.2.0"));
    }
}
