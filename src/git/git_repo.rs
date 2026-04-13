use git2::{Delta, Diff, DiffOptions, Repository};
use std::path::{Path, PathBuf};

use super::diff_model::*;

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
            Some(d) => Self::extract_files(&d),
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
            Some(d) => Self::extract_files(&d),
            None => Vec::new(),
        }
    }

    /// Combined working diff (kept for backward compat / session capture).
    pub fn working_diff(&self) -> Vec<DiffFile> {
        let mut staged = self.staged_diff();
        let unstaged = self.unstaged_diff();
        staged.extend(unstaged);
        staged
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

    /// Revert a file to index state (git checkout -- file).
    pub fn revert_file(&self, path: &str) -> Result<(), git2::Error> {
        self.repo.checkout_head(Some(
            git2::build::CheckoutBuilder::new()
                .force()
                .path(path),
        ))?;
        Ok(())
    }

    fn extract_files(diff: &Diff) -> Vec<DiffFile> {
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
                                    });
                                }
                            }

                            hunks.push(DiffHunk { header, lines });
                        }
                    }
                }
            }

            diff_files.push(DiffFile {
                path,
                status,
                hunks,
            });
        }

        diff_files
    }
}
