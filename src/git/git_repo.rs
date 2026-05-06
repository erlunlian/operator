use git2::{BranchType, Delta, Diff, DiffFile as GitDiffFile, DiffOptions, Repository};
use std::path::{Path, PathBuf};

use super::diff_model::*;
use crate::editor::syntax;

pub struct GitRepo {
    repo: Repository,
}

impl GitRepo {
    pub fn open(path: &Path) -> Option<Self> {
        Repository::discover(path).ok().map(|repo| Self { repo })
    }

    /// Returns the path to the `.git` directory.
    pub fn git_dir(&self) -> PathBuf {
        self.repo.path().to_path_buf()
    }

    /// Returns the path to the working tree root, if any (None for bare repos).
    pub fn workdir(&self) -> Option<PathBuf> {
        self.repo.workdir().map(|p| p.to_path_buf())
    }

    pub fn current_branch(&self) -> String {
        self.repo
            .head()
            .ok()
            .and_then(|head| head.shorthand().map(|s| s.to_string()))
            .unwrap_or_else(|| "HEAD".to_string())
    }

    /// Get diff of staged changes (index vs HEAD).
    pub fn staged_diff(&self) -> Vec<DiffFile> {
        let head_tree = self
            .repo
            .head()
            .ok()
            .and_then(|head| head.peel_to_tree().ok());

        let diff = match head_tree {
            Some(tree) => self.repo.diff_tree_to_index(Some(&tree), None, None).ok(),
            None => {
                // No HEAD yet — everything in the index is "staged"
                let mut opts = DiffOptions::new();
                self.repo
                    .diff_tree_to_index(None, None, Some(&mut opts))
                    .ok()
            }
        };

        match diff {
            Some(d) => self.extract_files(&d),
            None => Vec::new(),
        }
    }

    /// Get diff of unstaged changes (workdir vs index), including untracked files.
    pub fn unstaged_diff(&self) -> Vec<DiffFile> {
        let mut opts = DiffOptions::new();
        opts.include_untracked(true);
        opts.recurse_untracked_dirs(true);

        let diff = self.repo.diff_index_to_workdir(None, Some(&mut opts)).ok();

        match diff {
            Some(d) => self.extract_files(&d),
            None => Vec::new(),
        }
    }

    /// Stage a file (git add).
    pub fn stage_file(&self, path: &str) -> Result<(), git2::Error> {
        let mut index = self.repo.index()?;
        index.add_path(std::path::Path::new(path))?;
        index.write()?;
        Ok(())
    }

    /// Unstage a file (git reset HEAD -- file).
    pub fn unstage_file(&self, path: &str) -> Result<(), git2::Error> {
        let head = self.repo.head()?.peel_to_commit()?;
        self.repo.reset_default(Some(head.as_object()), [path])?;
        Ok(())
    }

    /// Revert a file to HEAD state (git checkout -- file).
    /// For untracked files, removes them from disk instead.
    pub fn revert_file(&self, path: &str, status: &super::diff_model::FileStatus) -> Result<(), Box<dyn std::error::Error>> {
        match status {
            super::diff_model::FileStatus::Added => {
                // Untracked / newly added file — delete from disk
                if let Some(workdir) = self.repo.workdir() {
                    let full_path = workdir.join(path);
                    if full_path.is_file() {
                        std::fs::remove_file(&full_path)?;
                    } else if full_path.is_dir() {
                        std::fs::remove_dir_all(&full_path)?;
                    }
                    // Clean up empty parent directories
                    let mut parent = full_path.parent();
                    while let Some(dir) = parent {
                        if let Some(workdir) = self.repo.workdir() {
                            if dir == workdir {
                                break;
                            }
                        }
                        if dir.read_dir().map(|mut d| d.next().is_none()).unwrap_or(false) {
                            let _ = std::fs::remove_dir(dir);
                            parent = dir.parent();
                        } else {
                            break;
                        }
                    }
                }
                Ok(())
            }
            _ => {
                // Modified / deleted / renamed — checkout from HEAD
                self.repo.checkout_head(Some(
                    git2::build::CheckoutBuilder::new()
                        .force()
                        .path(path),
                ))?;
                Ok(())
            }
        }
    }

    /// Diff current HEAD against merge-base with the given base ref.
    /// This produces the same diff as a PR would show.
    pub fn branch_diff(&self, base_ref: &str) -> Vec<DiffFile> {
        let head_commit = match self.repo.head().ok().and_then(|h| h.peel_to_commit().ok()) {
            Some(c) => c,
            None => return Vec::new(),
        };

        // Resolve base ref: prefer remote tracking branch (origin/<base>) over local,
        // since the local branch may be stale while origin reflects the actual GitHub state.
        let base_oid = {
            let remote_ref = format!("origin/{base_ref}");
            self.repo
                .find_branch(&remote_ref, BranchType::Remote)
                .ok()
                .and_then(|b| b.get().peel_to_commit().ok())
                .map(|c| c.id())
        }
        .or_else(|| {
            self.repo
                .find_branch(base_ref, BranchType::Local)
                .ok()
                .and_then(|b| b.get().peel_to_commit().ok())
                .map(|c| c.id())
        })
        .or_else(|| {
            self.repo
                .revparse_single(base_ref)
                .ok()
                .and_then(|obj| obj.peel_to_commit().ok())
                .map(|c| c.id())
        });

        let base_oid = match base_oid {
            Some(oid) => oid,
            None => return Vec::new(),
        };

        let merge_base = match self.repo.merge_base(base_oid, head_commit.id()) {
            Ok(oid) => oid,
            Err(_) => return Vec::new(),
        };

        let base_tree = self
            .repo
            .find_commit(merge_base)
            .ok()
            .and_then(|c| c.tree().ok());
        let head_tree = head_commit.tree().ok();

        let diff = match (base_tree.as_ref(), head_tree.as_ref()) {
            (Some(bt), Some(ht)) => self.repo.diff_tree_to_tree(Some(bt), Some(ht), None).ok(),
            _ => return Vec::new(),
        };

        match diff {
            Some(d) => self.extract_files(&d),
            None => Vec::new(),
        }
    }

    /// Detect the default branch (main/master).
    pub fn default_branch(&self) -> String {
        // Try refs/remotes/origin/HEAD (set by `git clone`)
        if let Ok(reference) = self.repo.find_reference("refs/remotes/origin/HEAD") {
            if let Some(target) = reference.symbolic_target() {
                // target is like "refs/remotes/origin/main"
                if let Some(branch) = target.strip_prefix("refs/remotes/origin/") {
                    return branch.to_string();
                }
            }
        }

        // Fall back to checking if main or master exist
        for name in &["main", "master"] {
            if self
                .repo
                .find_branch(name, BranchType::Local)
                .is_ok()
                || self
                    .repo
                    .find_branch(&format!("origin/{name}"), BranchType::Remote)
                    .is_ok()
            {
                return name.to_string();
            }
        }

        "main".to_string()
    }

    /// Get HEAD commit SHA.
    pub fn head_sha(&self) -> Option<String> {
        self.repo
            .head()
            .ok()
            .and_then(|h| h.peel_to_commit().ok())
            .map(|c| c.id().to_string())
    }

    fn extract_files(&self, diff: &Diff) -> Vec<DiffFile> {
        let mut diff_files = Vec::new();

        for delta_idx in 0..diff.deltas().len() {
            let delta = diff.deltas().nth(delta_idx).unwrap();
            let status = match delta.status() {
                Delta::Added | Delta::Untracked => FileStatus::Added,
                Delta::Deleted => FileStatus::Deleted,
                Delta::Modified => FileStatus::Modified,
                Delta::Renamed => FileStatus::Renamed,
                _ => FileStatus::Modified,
            };

            let path = delta
                .new_file()
                .path()
                .or_else(|| delta.old_file().path())
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_else(|| "unknown".to_string());

            // For renames, capture the prior path so the UI can show
            // "renamed from old → new" instead of a full content diff.
            let old_path = if matches!(status, FileStatus::Renamed) {
                delta
                    .old_file()
                    .path()
                    .map(|p| p.to_string_lossy().to_string())
                    .filter(|p| p != &path)
            } else {
                None
            };

            let mut hunks = Vec::new();
            if let Ok(patch) = git2::Patch::from_diff(diff, delta_idx) {
                if let Some(patch) = patch {
                    for hunk_idx in 0..patch.num_hunks() {
                        if let Ok((hunk, _)) = patch.hunk(hunk_idx) {
                            let header = String::from_utf8_lossy(hunk.header()).to_string();
                            let mut lines = Vec::new();

                            let num_lines = patch.num_lines_in_hunk(hunk_idx).unwrap_or(0);
                            for line_idx in 0..num_lines {
                                if let Ok(line) = patch.line_in_hunk(hunk_idx, line_idx) {
                                    let kind = match line.origin() {
                                        '+' => DiffLineKind::Added,
                                        '-' => DiffLineKind::Removed,
                                        _ => DiffLineKind::Context,
                                    };
                                    let content =
                                        String::from_utf8_lossy(line.content()).to_string();
                                    lines.push(DiffLine {
                                        kind,
                                        content,
                                        old_lineno: line.old_lineno(),
                                        new_lineno: line.new_lineno(),
                                        highlights: None,
                                    });
                                }
                            }

                            hunks.push(DiffHunk { _header: header, lines });
                        }
                    }
                }
            }

            // Read full file content for proper syntax highlighting context.
            let new_content = self.read_diff_file_content(&delta.new_file(), &path);
            let old_content = self.read_diff_file_content(&delta.old_file(), &path);

            let mut file = DiffFile {
                path,
                status,
                old_path,
                hunks,
                source_lines: None,
            };

            // Precompute syntax highlights using full file context.
            if let Some(lang) = syntax::detect_language(&file.path) {
                let new_source_lines = new_content
                    .as_deref()
                    .map(|c| Self::highlight_full_file(c, lang));
                let old_source_lines = old_content
                    .as_deref()
                    .map(|c| Self::highlight_full_file(c, lang));

                // Map highlights to diff lines by their actual file line numbers.
                for hunk in &mut file.hunks {
                    for diff_line in &mut hunk.lines {
                        let (source, lineno) = match diff_line.kind {
                            DiffLineKind::Removed => (&old_source_lines, diff_line.old_lineno),
                            _ => (&new_source_lines, diff_line.new_lineno),
                        };
                        if let (Some(lines), Some(n)) = (source, lineno) {
                            let idx = (n as usize).saturating_sub(1);
                            if let Some(sl) = lines.get(idx) {
                                diff_line.highlights = sl.highlights.clone();
                            }
                        }
                    }
                }

                // Store new-side source lines for context expansion.
                file.source_lines = new_source_lines;
            }

            diff_files.push(file);
        }

        diff_files
    }

    /// Read the content of a file referenced by a diff entry.
    /// Tries the git blob first, falls back to reading from the working directory.
    fn read_diff_file_content(&self, diff_file: &GitDiffFile<'_>, path: &str) -> Option<String> {
        let oid = diff_file.id();
        if !oid.is_zero() {
            if let Ok(blob) = self.repo.find_blob(oid) {
                return std::str::from_utf8(blob.content())
                    .ok()
                    .map(|s| s.to_string());
            }
        }
        // Fall back to the working directory (untracked/new files have no
        // blob in the object database). Retry briefly on empty/missing reads
        // because the FS watcher can fire while a writer is mid-write — we
        // don't want to cache a zero-byte snapshot of a file that's about
        // to have content.
        let workdir = self.repo.workdir()?;
        let full = workdir.join(path);
        // Total budget: ~50ms (2 retries × 25ms). This runs on the main
        // thread via panel.refresh(), so we cap aggressively. If a file
        // is genuinely empty we'll fall through and report empty content.
        for attempt in 0..3 {
            match std::fs::read_to_string(&full) {
                Ok(s) if !s.is_empty() => return Some(s),
                Ok(_) | Err(_) => {
                    if attempt < 2 {
                        std::thread::sleep(std::time::Duration::from_millis(25));
                    }
                }
            }
        }
        // Final read: prefer Some("") over None so the caller still renders
        // a header for a known-empty file rather than dropping it entirely.
        std::fs::read_to_string(&full).ok().or(Some(String::new()))
    }

    /// Highlight a full file and return per-line SourceLine entries.
    fn highlight_full_file(source: &str, lang: &str) -> Vec<SourceLine> {
        let spans = syntax::highlight_source(source, lang);
        let mut result = Vec::new();
        let mut offset = 0usize;

        for line_text in source.split('\n') {
            let line_start = offset;
            let line_end = offset + line_text.len();
            let trimmed = line_text.trim_end();

            let line_spans: Vec<_> = spans
                .iter()
                .filter(|s| s.byte_range.start < line_end && s.byte_range.end > line_start)
                .filter_map(|s| {
                    let start = s.byte_range.start.max(line_start) - line_start;
                    let end = s.byte_range.end.min(line_end) - line_start;
                    // Clamp to trimmed length so spans don't extend past visible content.
                    let end = end.min(trimmed.len());
                    if start < end
                        && end <= trimmed.len()
                        && trimmed.is_char_boundary(start)
                        && trimmed.is_char_boundary(end)
                    {
                        Some(syntax::HighlightSpan {
                            byte_range: start..end,
                            color: s.color,
                        })
                    } else {
                        None
                    }
                })
                .collect();

            result.push(SourceLine {
                content: trimmed.to_string(),
                highlights: if line_spans.is_empty() {
                    None
                } else {
                    Some(line_spans)
                },
            });

            offset = line_end + 1; // +1 for the '\n'
        }

        result
    }
}
