use std::path::{Path, PathBuf};
use std::process::Command;

use gpui::Entity;

use crate::git::PrDiffPanel;
use crate::git::github::gh_bin;

/// Status of a PR review workspace's background setup.
#[derive(Clone, Debug)]
pub enum PrReviewStatus {
    Loading(String),
    Ready,
    Error(String),
}

/// Non-entity state attached to a PR review workspace. The entity is stored
/// separately in `PrReviewState::panel` so the workspace can swap it out when
/// the async setup finishes.
pub struct PrReviewState {
    pub url: String,
    pub panel: Entity<PrDiffPanel>,
    pub status: PrReviewStatus,
    pub work_dir: Option<PathBuf>,
    pub pr: Option<PrRef>,
    pub title: Option<String>,
}

#[derive(Clone, Debug)]
pub struct PrRef {
    pub owner: String,
    pub repo: String,
    pub number: u64,
}

impl PrRef {
    pub fn short(&self) -> String {
        format!("{}/{}#{}", self.owner, self.repo, self.number)
    }
}

/// Parse a GitHub PR URL into its components.
/// Accepts `https://github.com/<owner>/<repo>/pull/<n>` (with optional trailing
/// path segments like `/files`) and the shorthand `<owner>/<repo>#<n>`.
pub fn parse_pr_url(url: &str) -> Option<PrRef> {
    let url = url.trim();
    if url.is_empty() {
        return None;
    }

    // Shorthand: owner/repo#number
    if let Some((left, num)) = url.split_once('#') {
        let parts: Vec<&str> = left.split('/').collect();
        if parts.len() == 2 {
            let number = num.parse::<u64>().ok()?;
            return Some(PrRef {
                owner: parts[0].to_string(),
                repo: parts[1].to_string(),
                number,
            });
        }
    }

    let without_scheme = url
        .trim_start_matches("https://")
        .trim_start_matches("http://")
        .trim_start_matches("www.")
        .trim_start_matches("github.com/");
    let parts: Vec<&str> = without_scheme.split('/').filter(|p| !p.is_empty()).collect();
    if parts.len() >= 4 && parts[2] == "pull" {
        let number = parts[3].parse::<u64>().ok()?;
        return Some(PrRef {
            owner: parts[0].to_string(),
            repo: parts[1].to_string(),
            number,
        });
    }
    None
}

/// Cache directory for a given PR — a dedicated clone per PR keeps concurrent
/// PR workspaces from fighting over the same checkout.
pub fn cache_dir_for(pr: &PrRef) -> PathBuf {
    let base = dirs::cache_dir()
        .unwrap_or_else(|| {
            dirs::home_dir()
                .unwrap_or_else(|| PathBuf::from("."))
                .join(".cache")
        })
        .join("operator")
        .join("pr-reviews");
    base.join(format!("{}__{}__{}", pr.owner, pr.repo, pr.number))
}

/// Result of successfully preparing a PR checkout.
pub struct PrReviewReady {
    pub work_dir: PathBuf,
    pub pr: PrRef,
    pub title: String,
}

/// Clone (or reuse) a per-PR cache dir and check out the PR branch.
/// Blocking — call from a background executor.
pub fn setup_pr_blocking(url: &str) -> Result<PrReviewReady, String> {
    let pr = parse_pr_url(url)
        .ok_or_else(|| format!("Not a GitHub PR URL: {url}"))?;
    let work_dir = cache_dir_for(&pr);

    if let Some(parent) = work_dir.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("create cache dir: {e}"))?;
    }

    let gh = gh_bin();

    // Clone once if missing, otherwise refresh refs.
    if !work_dir.join(".git").exists() {
        let slug = format!("{}/{}", pr.owner, pr.repo);
        let out = Command::new(gh)
            .args([
                "repo",
                "clone",
                &slug,
                &work_dir.to_string_lossy(),
                "--",
                "--no-tags",
                "--filter=blob:none",
            ])
            .output()
            .map_err(|e| format!("gh repo clone: {e}"))?;
        if !out.status.success() {
            let _ = std::fs::remove_dir_all(&work_dir);
            return Err(format!(
                "Clone failed: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            ));
        }
    } else {
        let out = Command::new("git")
            .args(["fetch", "--all", "--prune"])
            .current_dir(&work_dir)
            .output()
            .map_err(|e| format!("git fetch: {e}"))?;
        if !out.status.success() {
            return Err(format!(
                "Fetch failed: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            ));
        }
    }

    let out = Command::new(gh)
        .args(["pr", "checkout", &pr.number.to_string(), "--force"])
        .current_dir(&work_dir)
        .output()
        .map_err(|e| format!("gh pr checkout: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "Checkout failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }

    let title = Command::new(gh)
        .args(["pr", "view", "--json", "title", "-q", ".title"])
        .current_dir(&work_dir)
        .output()
        .ok()
        .and_then(|o| {
            if o.status.success() {
                Some(String::from_utf8_lossy(&o.stdout).trim().to_string())
            } else {
                None
            }
        })
        .unwrap_or_default();

    Ok(PrReviewReady { work_dir, pr, title })
}

/// Check that a path is inside `dirs::cache_dir() / operator / pr-reviews`
/// before touching it, so we can't accidentally wipe unrelated files.
#[allow(dead_code)]
pub fn is_cache_dir(path: &Path) -> bool {
    let base = dirs::cache_dir()
        .map(|d| d.join("operator").join("pr-reviews"))
        .unwrap_or_else(|| PathBuf::from(""));
    path.starts_with(&base)
}
