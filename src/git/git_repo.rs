use git2::{Delta, DiffOptions, Repository};
use std::path::Path;

use super::diff_model::*;

pub struct GitRepo {
    repo: Repository,
}

impl GitRepo {
    pub fn open(path: &Path) -> Option<Self> {
        Repository::discover(path).ok().map(|repo| Self { repo })
    }

    pub fn current_branch(&self) -> String {
        self.repo
            .head()
            .ok()
            .and_then(|head| head.shorthand().map(|s| s.to_string()))
            .unwrap_or_else(|| "HEAD".to_string())
    }

    pub fn working_diff(&self) -> Vec<DiffFile> {
        let mut diff_files = Vec::new();

        let head_tree = self
            .repo
            .head()
            .ok()
            .and_then(|head| head.peel_to_tree().ok());

        let mut opts = DiffOptions::new();
        opts.include_untracked(true);

        let diff = match head_tree {
            Some(tree) => self
                .repo
                .diff_tree_to_workdir_with_index(Some(&tree), Some(&mut opts))
                .ok(),
            None => self
                .repo
                .diff_index_to_workdir(None, Some(&mut opts))
                .ok(),
        };

        let Some(diff) = diff else {
            return diff_files;
        };

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
            if let Ok(patch) = git2::Patch::from_diff(&diff, delta_idx) {
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
