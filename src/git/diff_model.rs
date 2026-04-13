use crate::editor::syntax::HighlightSpan;

#[derive(Clone)]
pub enum DiffLineKind {
    Context,
    Added,
    Removed,
}

#[derive(Clone)]
pub struct DiffLine {
    pub kind: DiffLineKind,
    pub content: String,
    pub old_lineno: Option<u32>,
    pub new_lineno: Option<u32>,
    /// Precomputed syntax highlight spans for this line's content.
    pub highlights: Option<Vec<HighlightSpan>>,
}

#[derive(Clone)]
pub struct DiffHunk {
    pub _header: String,
    pub lines: Vec<DiffLine>,
}

#[derive(Clone)]
pub enum FileStatus {
    Added,
    Modified,
    Deleted,
    Renamed,
}

#[derive(Clone)]
pub struct DiffFile {
    pub path: String,
    pub status: FileStatus,
    pub hunks: Vec<DiffHunk>,
}

impl DiffFile {
    /// Count added lines across all hunks.
    pub fn additions(&self) -> usize {
        self.hunks
            .iter()
            .flat_map(|h| &h.lines)
            .filter(|l| matches!(l.kind, DiffLineKind::Added))
            .count()
    }

    /// Count removed lines across all hunks.
    pub fn deletions(&self) -> usize {
        self.hunks
            .iter()
            .flat_map(|h| &h.lines)
            .filter(|l| matches!(l.kind, DiffLineKind::Removed))
            .count()
    }
}
