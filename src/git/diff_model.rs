pub enum DiffLineKind {
    Context,
    Added,
    Removed,
}

pub struct DiffLine {
    pub kind: DiffLineKind,
    pub content: String,
    pub old_lineno: Option<u32>,
    pub new_lineno: Option<u32>,
}

pub struct DiffHunk {
    pub header: String,
    pub lines: Vec<DiffLine>,
}

pub enum FileStatus {
    Added,
    Modified,
    Deleted,
    Renamed,
}

pub struct DiffFile {
    pub path: String,
    pub status: FileStatus,
    pub hunks: Vec<DiffHunk>,
}
