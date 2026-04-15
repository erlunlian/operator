use serde::Deserialize;
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

pub struct UpdateInfo {
    pub latest_version: String,
    pub download_url: String,
}

/// Check GitHub releases for a newer version. Runs synchronously (call from background thread).
/// Skips the network request if the last check was less than an hour ago.
pub fn check_for_update(current_version: &str) -> Option<UpdateInfo> {
    if !should_check() {
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

        Some(UpdateInfo {
            latest_version: latest.to_string(),
            download_url: dmg_url,
        })
    } else {
        None
    }
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
