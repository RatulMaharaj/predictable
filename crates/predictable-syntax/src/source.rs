//! Source files, byte spans and line/column resolution.
//!
//! Every diagnostic the parser emits points at a [`Span`], and a `Span` is only
//! meaningful relative to the [`SourceMap`] that produced it. Spans are byte
//! offsets, not char offsets: they are what a mechanical suggested-edit applier
//! needs (`01-ir.md` §7) and what `annotate-snippets` consumes downstream.

/// Identifier of a file registered in a [`SourceMap`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct FileId(pub u32);

/// A half-open byte range `[start, end)` within a single file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Span {
    pub file: FileId,
    pub start: u32,
    pub end: u32,
}

impl Span {
    pub fn new(file: FileId, start: usize, end: usize) -> Span {
        Span {
            file,
            start: start as u32,
            end: end as u32,
        }
    }

    /// The smallest span covering both operands. Both must be in one file;
    /// if they are not, `self`'s file wins (the parser never mixes files).
    pub fn to(self, other: Span) -> Span {
        Span {
            file: self.file,
            start: self.start.min(other.start),
            end: self.end.max(other.end),
        }
    }

    /// A zero-width span at this span's start, used to anchor insertions.
    pub fn at_start(self) -> Span {
        Span {
            file: self.file,
            start: self.start,
            end: self.start,
        }
    }

    pub fn len(self) -> usize {
        (self.end - self.start) as usize
    }

    pub fn is_empty(self) -> bool {
        self.end == self.start
    }
}

/// A value paired with the span of the syntax that produced it.
#[derive(Debug, Clone, PartialEq)]
pub struct Spanned<T> {
    pub value: T,
    pub span: Span,
}

impl<T> Spanned<T> {
    pub fn new(value: T, span: Span) -> Spanned<T> {
        Spanned { value, span }
    }

    pub fn map<U>(self, f: impl FnOnce(T) -> U) -> Spanned<U> {
        Spanned {
            value: f(self.value),
            span: self.span,
        }
    }

    pub fn as_ref(&self) -> Spanned<&T> {
        Spanned {
            value: &self.value,
            span: self.span,
        }
    }
}

/// A 1-based, human-facing position, as it appears in a rendered diagnostic.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Location {
    pub file: String,
    pub line: u32,
    pub col: u32,
}

struct SourceFile {
    name: String,
    text: String,
    /// Byte offset of the start of each line, `line_starts[0] == 0`.
    line_starts: Vec<u32>,
}

/// The set of files a parse ran over, and the only thing that can turn a
/// [`Span`] back into a file name, a line and a column.
#[derive(Default)]
pub struct SourceMap {
    files: Vec<SourceFile>,
}

impl SourceMap {
    pub fn new() -> SourceMap {
        SourceMap { files: Vec::new() }
    }

    /// Register a file and return its id. `name` is presentational (it is what
    /// appears in a diagnostic header); it need not exist on disk.
    pub fn add(&mut self, name: impl Into<String>, text: impl Into<String>) -> FileId {
        let text = text.into();
        let mut line_starts = vec![0u32];
        for (i, b) in text.bytes().enumerate() {
            if b == b'\n' {
                line_starts.push(i as u32 + 1);
            }
        }
        self.files.push(SourceFile {
            name: name.into(),
            text,
            line_starts,
        });
        FileId(self.files.len() as u32 - 1)
    }

    pub fn file_count(&self) -> usize {
        self.files.len()
    }

    pub fn name(&self, file: FileId) -> &str {
        &self.files[file.0 as usize].name
    }

    pub fn text(&self, file: FileId) -> &str {
        &self.files[file.0 as usize].text
    }

    /// The source text a span covers.
    pub fn snippet(&self, span: Span) -> &str {
        let f = &self.files[span.file.0 as usize];
        &f.text[span.start as usize..span.end as usize]
    }

    /// 1-based line/column of a span's start.
    pub fn location(&self, span: Span) -> Location {
        let f = &self.files[span.file.0 as usize];
        let line_idx = match f.line_starts.binary_search(&span.start) {
            Ok(i) => i,
            Err(i) => i - 1,
        };
        let line_start = f.line_starts[line_idx] as usize;
        // Column counts characters, not bytes: that is what a human counts.
        let col = f.text[line_start..span.start as usize].chars().count() + 1;
        Location {
            file: f.name.clone(),
            line: line_idx as u32 + 1,
            col: col as u32,
        }
    }

    /// The full text of the line a span starts on, without its newline.
    pub fn line_text(&self, span: Span) -> &str {
        let f = &self.files[span.file.0 as usize];
        let line_idx = match f.line_starts.binary_search(&span.start) {
            Ok(i) => i,
            Err(i) => i - 1,
        };
        let start = f.line_starts[line_idx] as usize;
        let end = f
            .line_starts
            .get(line_idx + 1)
            .map(|e| *e as usize - 1)
            .unwrap_or(f.text.len());
        f.text[start..end].trim_end_matches('\r')
    }
}
