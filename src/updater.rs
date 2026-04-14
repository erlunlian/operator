use serde::Deserialize;
use std::process::Command;

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
pub fn check_for_update(current_version: &str) -> Option<UpdateInfo> {
    let output = Command::new("curl")
        .args([
            "-sL",
            "-H",
            "Accept: application/vnd.github+json",
            &format!(
                "https://api.github.com/repos/erlunlian/operator/releases/latest"
            ),
        ])
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let release: GitHubRelease = serde_json::from_slice(&output.stdout).ok()?;
    let latest = release.tag_name.trim_start_matches('v');
    let current = current_version.trim_start_matches('v');

    if version_newer(latest, current) {
        let dmg_url = release
            .assets
            .iter()
            .find(|a| a.name.ends_with(".dmg"))
            .map(|a| a.browser_download_url.clone())
            .unwrap_or_else(|| {
                format!(
                    "https://github.com/erlunlian/operator/releases/tag/{}",
                    release.tag_name
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
