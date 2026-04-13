use serde::Deserialize;
use std::path::Path;
use std::process::Command;

/// Resolve the `gh` binary path, checking PATH first, then common install locations.
fn gh_bin() -> &'static str {
    use std::sync::OnceLock;
    static GH: OnceLock<String> = OnceLock::new();
    GH.get_or_init(|| {
        // Try PATH first
        if Command::new("gh").arg("--version").output().map(|o| o.status.success()).unwrap_or(false) {
            return "gh".to_string();
        }
        // Common install locations
        for path in &[
            "/opt/homebrew/bin/gh",
            "/usr/local/bin/gh",
            "/usr/bin/gh",
        ] {
            if std::path::Path::new(path).exists() {
                return path.to_string();
            }
        }
        "gh".to_string()
    })
}

// ── Status ──

#[derive(Clone, Debug, PartialEq)]
pub enum GhStatus {
    /// Haven't checked yet.
    Unknown,
    NotInstalled,
    NotAuthenticated,
    Available,
}

/// Check whether `gh` CLI is installed and authenticated.
pub fn check_gh() -> GhStatus {
    let version = Command::new(gh_bin()).arg("--version").output();
    if version.is_err() || !version.unwrap().status.success() {
        return GhStatus::NotInstalled;
    }
    let auth = Command::new(gh_bin())
        .args(["auth", "status"])
        .output();
    if auth.is_err() || !auth.unwrap().status.success() {
        return GhStatus::NotAuthenticated;
    }
    GhStatus::Available
}

// ── PR info ──

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
#[allow(dead_code)]
pub struct PrInfo {
    pub number: u64,
    pub title: String,
    pub state: String,
    pub base_ref_name: String,
    pub head_ref_name: String,
    pub url: String,
}

/// Detect whether a PR is open for the current branch.
pub fn detect_pr(repo_dir: &Path) -> Option<PrInfo> {
    let output = Command::new(gh_bin())
        .args([
            "pr",
            "view",
            "--json",
            "number,title,state,baseRefName,headRefName,url",
        ])
        .current_dir(repo_dir)
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    serde_json::from_str(&stdout).ok()
}

// ── PR review comments ──

#[derive(Clone, Debug, Deserialize)]
#[allow(dead_code)]
pub struct PrReviewComment {
    pub id: u64,
    pub body: String,
    pub path: String,
    pub line: Option<u32>,
    pub side: Option<String>,
    pub user: GhUser,
    pub created_at: String,
    pub in_reply_to_id: Option<u64>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct GhUser {
    pub login: String,
}

/// Get the owner/repo for the current repository.
pub fn repo_owner_name(repo_dir: &Path) -> Option<(String, String)> {
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct RepoView {
        name_with_owner: String,
    }

    let output = Command::new(gh_bin())
        .args(["repo", "view", "--json", "nameWithOwner"])
        .current_dir(repo_dir)
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let view: RepoView = serde_json::from_str(&stdout).ok()?;
    let parts: Vec<&str> = view.name_with_owner.splitn(2, '/').collect();
    if parts.len() == 2 {
        Some((parts[0].to_string(), parts[1].to_string()))
    } else {
        None
    }
}

/// Fetch review comments for a PR.
pub fn fetch_pr_comments(repo_dir: &Path, pr_number: u64) -> Vec<PrReviewComment> {
    let (owner, repo) = match repo_owner_name(repo_dir) {
        Some(pair) => pair,
        None => return Vec::new(),
    };

    let output = Command::new(gh_bin())
        .args([
            "api",
            &format!("repos/{owner}/{repo}/pulls/{pr_number}/comments"),
            "--paginate",
        ])
        .current_dir(repo_dir)
        .output();

    let output = match output {
        Ok(o) if o.status.success() => o,
        _ => return Vec::new(),
    };

    let stdout = String::from_utf8_lossy(&output.stdout);
    serde_json::from_str(&stdout).unwrap_or_default()
}

/// Post an inline review comment on a PR.
/// If `start_line` is provided, creates a multi-line comment spanning start_line..=line.
pub fn post_pr_comment(
    repo_dir: &Path,
    pr_number: u64,
    body: &str,
    commit_sha: &str,
    path: &str,
    line: u32,
    side: &str,
    start_line: Option<u32>,
    start_side: Option<&str>,
) -> Result<PrReviewComment, String> {
    let (owner, repo) = repo_owner_name(repo_dir)
        .ok_or_else(|| "Could not determine repository owner/name".to_string())?;

    let mut args = vec![
        "api".to_string(),
        format!("repos/{owner}/{repo}/pulls/{pr_number}/comments"),
        "-X".to_string(),
        "POST".to_string(),
        "-f".to_string(),
        format!("body={body}"),
        "-f".to_string(),
        format!("commit_id={commit_sha}"),
        "-f".to_string(),
        format!("path={path}"),
        "-F".to_string(),
        format!("line={line}"),
        "-f".to_string(),
        format!("side={side}"),
    ];

    if let Some(sl) = start_line {
        args.push("-F".to_string());
        args.push(format!("start_line={sl}"));
        if let Some(ss) = start_side {
            args.push("-f".to_string());
            args.push(format!("start_side={ss}"));
        }
    }

    let output = Command::new(gh_bin())
        .args(&args)
        .current_dir(repo_dir)
        .output()
        .map_err(|e| e.to_string())?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("gh api failed: {stderr}"));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    serde_json::from_str(&stdout).map_err(|e| e.to_string())
}

/// Reply to an existing review comment.
#[allow(dead_code)]
pub fn reply_to_comment(
    repo_dir: &Path,
    pr_number: u64,
    comment_id: u64,
    body: &str,
) -> Result<PrReviewComment, String> {
    let (owner, repo) = repo_owner_name(repo_dir)
        .ok_or_else(|| "Could not determine repository owner/name".to_string())?;

    let output = Command::new(gh_bin())
        .args([
            "api",
            &format!(
                "repos/{owner}/{repo}/pulls/{pr_number}/comments/{comment_id}/replies"
            ),
            "-X",
            "POST",
            "-f",
            &format!("body={body}"),
        ])
        .current_dir(repo_dir)
        .output()
        .map_err(|e| e.to_string())?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("gh api failed: {stderr}"));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    serde_json::from_str(&stdout).map_err(|e| e.to_string())
}
