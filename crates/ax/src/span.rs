//! Source spans. Cheap to copy; all positions are byte offsets into interned source.

use std::fmt;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub struct FileId(pub u32);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub struct Span {
    pub file: FileId,
    pub start: u32,
    pub end: u32,
}

impl Span {
    pub const DUMMY: Span = Span {
        file: FileId(u32::MAX),
        start: 0,
        end: 0,
    };

    #[inline]
    pub fn new(file: FileId, start: u32, end: u32) -> Self {
        Self { file, start, end }
    }

    #[inline]
    pub fn is_dummy(self) -> bool {
        self.file.0 == u32::MAX
    }

    #[inline]
    pub fn merge(self, other: Span) -> Span {
        if self.is_dummy() {
            return other;
        }
        if other.is_dummy() {
            return self;
        }
        debug_assert_eq!(self.file, other.file);
        Span {
            file: self.file,
            start: self.start.min(other.start),
            end: self.end.max(other.end),
        }
    }

    #[inline]
    pub fn len(self) -> u32 {
        self.end.saturating_sub(self.start)
    }
}

impl fmt::Display for Span {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}-{}", self.file.0, self.start, self.end)
    }
}

#[derive(Clone, Debug)]
pub struct SourceFile {
    pub id: FileId,
    pub name: String,
    pub src: String,
    line_starts: Vec<u32>,
}

impl SourceFile {
    pub fn new(id: FileId, name: String, src: String) -> Self {
        let mut line_starts = vec![0u32];
        for (i, b) in src.bytes().enumerate() {
            if b == b'\n' {
                line_starts.push((i + 1) as u32);
            }
        }
        Self {
            id,
            name,
            src,
            line_starts,
        }
    }

    pub fn line_col(&self, offset: u32) -> (u32, u32) {
        let i = self
            .line_starts
            .partition_point(|&s| s <= offset)
            .saturating_sub(1);
        let start = self.line_starts[i];
        (i as u32 + 1, offset.saturating_sub(start) + 1)
    }

    pub fn snippet(&self, span: Span) -> &str {
        let s = span.start as usize;
        let e = (span.end as usize).min(self.src.len());
        if s >= self.src.len() {
            ""
        } else {
            &self.src[s..e]
        }
    }

    pub fn line_text(&self, line: u32) -> &str {
        let idx = line.saturating_sub(1) as usize;
        let start = *self.line_starts.get(idx).unwrap_or(&0) as usize;
        let end = self
            .line_starts
            .get(idx + 1)
            .map(|&x| x as usize)
            .unwrap_or(self.src.len());
        let text = &self.src[start..end];
        text.trim_end_matches(['\n', '\r'])
    }
}

#[derive(Clone, Debug, Default)]
pub struct SourceMap {
    files: Vec<SourceFile>,
}

impl SourceMap {
    pub fn new() -> Self {
        Self { files: Vec::new() }
    }

    pub fn add(&mut self, name: String, src: String) -> FileId {
        let id = FileId(self.files.len() as u32);
        self.files.push(SourceFile::new(id, name, src));
        id
    }

    pub fn get(&self, id: FileId) -> Option<&SourceFile> {
        self.files.get(id.0 as usize)
    }

    pub fn files(&self) -> &[SourceFile] {
        &self.files
    }
}
