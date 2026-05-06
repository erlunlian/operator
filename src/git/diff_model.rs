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

/// A line from the full source file with precomputed syntax highlights.
#[derive(Clone)]
pub struct SourceLine {
    pub content: String,
    pub highlights: Option<Vec<HighlightSpan>>,
}

#[derive(Clone)]
pub struct DiffFile {
    pub path: String,
    pub status: FileStatus,
    /// For renames: the file's prior path (where it moved from). `None` for
    /// non-rename statuses or when the previous path can't be determined.
    pub old_path: Option<String>,
    pub hunks: Vec<DiffHunk>,
    /// Full new-side file content split into lines, with precomputed highlights.
    /// Used for correct multiline syntax highlighting and expanding context
    /// beyond hunk boundaries.
    pub source_lines: Option<Vec<SourceLine>>,
}

impl SourceLine {
    /// Estimate heap bytes used by this source line.
    pub fn estimated_bytes(&self) -> usize {
        self.content.capacity()
            + self
                .highlights
                .as_ref()
                .map(|v| v.capacity() * std::mem::size_of::<HighlightSpan>())
                .unwrap_or(0)
    }
}

impl DiffLine {
    /// Estimate heap bytes used by this diff line.
    pub fn estimated_bytes(&self) -> usize {
        self.content.capacity()
            + self
                .highlights
                .as_ref()
                .map(|v| v.capacity() * std::mem::size_of::<HighlightSpan>())
                .unwrap_or(0)
    }
}

impl DiffFile {
    /// Estimate total heap bytes used by this file's diff data.
    pub fn estimated_bytes(&self) -> usize {
        let hunks: usize = self.hunks.iter().map(|h| {
            h._header.capacity()
                + h.lines.iter().map(|l| l.estimated_bytes()).sum::<usize>()
        }).sum();
        let source_lines: usize = self
            .source_lines
            .as_ref()
            .map(|sl| sl.iter().map(|l| l.estimated_bytes()).sum())
            .unwrap_or(0);
        let old_path = self.old_path.as_ref().map(|s| s.capacity()).unwrap_or(0);
        self.path.capacity() + old_path + hunks + source_lines
    }

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
