//! Editable text model for the code editor panel.
//!
//! Text lives in a rope so edits and line lookups stay cheap in large files.
//! Every offset in this API is a `char` index into the document. Line breaks
//! are always `\n` in memory; a file loaded with CRLF endings is written back
//! with CRLF.

use ropey::Rope;
use std::{
    io::Write,
    ops::Range,
    path::Path,
    time::{Duration, Instant},
};

/// Larger files open read-only elsewhere rather than as an editable buffer.
pub const MAX_EDITABLE_BYTES: u64 = 32 * 1024 * 1024;
/// Consecutive typing within this window undoes as one step.
const GROUP_WINDOW: Duration = Duration::from_millis(1000);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Selection {
    /// Where the selection started; stays put while extending.
    pub anchor: usize,
    /// The caret.
    pub head: usize,
}

impl Selection {
    pub fn caret(offset: usize) -> Self {
        Self {
            anchor: offset,
            head: offset,
        }
    }

    pub fn range(self) -> Range<usize> {
        self.anchor.min(self.head)..self.anchor.max(self.head)
    }

    pub fn is_empty(self) -> bool {
        self.anchor == self.head
    }

    pub fn reversed(self) -> bool {
        self.head < self.anchor
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LineEnding {
    Lf,
    CrLf,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum EditKind {
    Typing,
    Deleting,
    Other,
}

#[derive(Clone, Debug)]
struct Edit {
    start: usize,
    removed: String,
    inserted: String,
}

#[derive(Clone, Debug)]
struct Transaction {
    id: u64,
    edits: Vec<Edit>,
    before: Selection,
    after: Selection,
    kind: EditKind,
    at: Instant,
}

/// Why a file cannot be edited as text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LoadError {
    TooLarge(u64),
    Binary,
    Io(String),
}

impl std::fmt::Display for LoadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TooLarge(bytes) => write!(
                f,
                "file is too large to edit ({} MB)",
                bytes / (1024 * 1024)
            ),
            Self::Binary => f.write_str("binary files cannot be edited"),
            Self::Io(error) => f.write_str(error),
        }
    }
}

/// Read a file for editing. Blocking: call off the UI thread.
pub fn load(path: &Path) -> Result<TextBuffer, LoadError> {
    let metadata = std::fs::metadata(path).map_err(|error| LoadError::Io(error.to_string()))?;
    if metadata.len() > MAX_EDITABLE_BYTES {
        return Err(LoadError::TooLarge(metadata.len()));
    }
    let bytes = std::fs::read(path).map_err(|error| LoadError::Io(error.to_string()))?;
    if bytes[..bytes.len().min(8192)].contains(&0) {
        return Err(LoadError::Binary);
    }
    let text = String::from_utf8(bytes).map_err(|_| LoadError::Binary)?;
    Ok(TextBuffer::new(&text))
}

/// Replace `path` with `contents` without ever leaving a truncated file:
/// write a sibling temporary file, then rename it over the original.
/// Keeps the original permissions. Blocking: call off the UI thread.
pub fn save_atomic(path: &Path, contents: &str) -> std::io::Result<()> {
    let dir = path
        .parent()
        .filter(|dir| !dir.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let permissions = std::fs::metadata(path).ok().map(|meta| meta.permissions());
    let mut file = tempfile::NamedTempFile::new_in(dir)?;
    file.write_all(contents.as_bytes())?;
    file.as_file().sync_all()?;
    if let Some(permissions) = permissions {
        file.as_file().set_permissions(permissions)?;
    }
    file.persist(path).map_err(|error| error.error)?;
    Ok(())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CharClass {
    Space,
    Word,
    Punct,
    Newline,
}

fn class(ch: char) -> CharClass {
    if ch == '\n' {
        CharClass::Newline
    } else if ch.is_whitespace() {
        CharClass::Space
    } else if ch.is_alphanumeric() || ch == '_' {
        CharClass::Word
    } else {
        CharClass::Punct
    }
}

pub struct TextBuffer {
    rope: Rope,
    selection: Selection,
    /// Column kept while moving vertically through shorter lines.
    goal_column: Option<usize>,
    undo: Vec<Transaction>,
    redo: Vec<Transaction>,
    next_id: u64,
    saved_state: u64,
    /// Bumped on every text change, for caches keyed on content.
    revision: u64,
    line_ending: LineEnding,
    indent_unit: String,
    /// Lowest offset touched since [`TextBuffer::take_changed_from`].
    changed_from: Option<usize>,
}

impl TextBuffer {
    pub fn new(text: &str) -> Self {
        let line_ending = if text.contains("\r\n") {
            LineEnding::CrLf
        } else {
            LineEnding::Lf
        };
        let normalized;
        let text = if line_ending == LineEnding::CrLf {
            normalized = text.replace("\r\n", "\n");
            normalized.as_str()
        } else {
            text
        };
        Self {
            indent_unit: detect_indent(text),
            rope: Rope::from_str(text),
            selection: Selection::caret(0),
            goal_column: None,
            undo: Vec::new(),
            redo: Vec::new(),
            next_id: 1,
            saved_state: 0,
            revision: 0,
            line_ending,
            changed_from: None,
        }
    }

    // ---- Reading -------------------------------------------------------

    pub fn len_chars(&self) -> usize {
        self.rope.len_chars()
    }

    /// Lines including the empty line after a trailing newline.
    pub fn line_count(&self) -> usize {
        self.rope.len_lines()
    }

    /// A line's text without its newline.
    pub fn line(&self, line: usize) -> String {
        if line >= self.line_count() {
            return String::new();
        }
        let mut text = self.rope.line(line).to_string();
        if text.ends_with('\n') {
            text.pop();
        }
        text
    }

    pub fn text(&self) -> String {
        self.rope.to_string()
    }

    /// Text with the file's original line endings, for writing to disk.
    pub fn text_for_save(&self) -> String {
        match self.line_ending {
            LineEnding::Lf => self.text(),
            LineEnding::CrLf => self.text().replace('\n', "\r\n"),
        }
    }

    pub fn slice(&self, range: Range<usize>) -> String {
        let range = self.clamp(range.start)..self.clamp(range.end);
        self.rope
            .slice(range.start..range.end.max(range.start))
            .to_string()
    }

    pub fn selection(&self) -> Selection {
        self.selection
    }

    pub fn selected_text(&self) -> String {
        self.slice(self.selection.range())
    }

    pub fn revision(&self) -> u64 {
        self.revision
    }

    pub fn line_ending(&self) -> LineEnding {
        self.line_ending
    }

    pub fn indent_unit(&self) -> &str {
        &self.indent_unit
    }

    /// Identifies the current undo state; pass it to
    /// [`TextBuffer::mark_saved_at`] once that text is on disk.
    pub fn state_token(&self) -> u64 {
        self.state_id()
    }

    /// Record that the text at `token` reached disk. Edits made while the save
    /// was running keep the buffer dirty.
    pub fn mark_saved_at(&mut self, token: u64) {
        self.saved_state = token;
    }

    /// The lowest char offset changed since the last call, for invalidating
    /// per-line caches. Offsets before it are unaffected.
    pub fn take_changed_from(&mut self) -> Option<usize> {
        self.changed_from.take()
    }

    fn note_change(&mut self, offset: usize) {
        self.changed_from = Some(self.changed_from.map_or(offset, |from| from.min(offset)));
    }

    pub fn is_dirty(&self) -> bool {
        self.state_id() != self.saved_state
    }

    /// Record that the current text is what is on disk.
    pub fn mark_saved(&mut self) {
        self.saved_state = self.state_id();
    }

    fn state_id(&self) -> u64 {
        self.undo.last().map_or(0, |transaction| transaction.id)
    }

    /// `(line, column)` of a char offset; column counts chars.
    pub fn position(&self, offset: usize) -> (usize, usize) {
        let offset = self.clamp(offset);
        let line = self.rope.char_to_line(offset);
        (line, offset - self.rope.line_to_char(line))
    }

    /// Char offset of a `(line, column)`, clamped to the document and line.
    pub fn offset(&self, line: usize, column: usize) -> usize {
        let line = line.min(self.line_count().saturating_sub(1));
        self.rope.line_to_char(line) + column.min(self.line_len(line))
    }

    pub fn line_len(&self, line: usize) -> usize {
        if line >= self.line_count() {
            return 0;
        }
        let slice = self.rope.line(line);
        let len = slice.len_chars();
        if len > 0 && slice.char(len - 1) == '\n' {
            len - 1
        } else {
            len
        }
    }

    pub fn line_start(&self, line: usize) -> usize {
        self.rope
            .line_to_char(line.min(self.line_count().saturating_sub(1)))
    }

    fn clamp(&self, offset: usize) -> usize {
        offset.min(self.len_chars())
    }

    fn char_at(&self, offset: usize) -> Option<char> {
        (offset < self.len_chars()).then(|| self.rope.char(offset))
    }

    // ---- UTF-16 bridging for platform text input -------------------------

    pub fn offset_to_utf16(&self, offset: usize) -> usize {
        self.rope.char_to_utf16_cu(self.clamp(offset))
    }

    pub fn offset_from_utf16(&self, utf16: usize) -> usize {
        self.rope
            .utf16_cu_to_char(utf16.min(self.rope.len_utf16_cu()))
    }

    // ---- Selection -----------------------------------------------------

    pub fn set_selection(&mut self, selection: Selection) {
        self.selection = Selection {
            anchor: self.clamp(selection.anchor),
            head: self.clamp(selection.head),
        };
        self.goal_column = None;
    }

    fn move_to(&mut self, offset: usize, extend: bool) {
        let head = self.clamp(offset);
        self.selection = if extend {
            Selection {
                anchor: self.selection.anchor,
                head,
            }
        } else {
            Selection::caret(head)
        };
    }

    pub fn select_all(&mut self) {
        self.selection = Selection {
            anchor: 0,
            head: self.len_chars(),
        };
        self.goal_column = None;
    }

    /// Select the word (or run of punctuation/space) around `offset`.
    pub fn select_word_at(&mut self, offset: usize) {
        let offset = self.clamp(offset);
        let (line, _) = self.position(offset);
        let start_limit = self.line_start(line);
        let end_limit = start_limit + self.line_len(line);
        let probe = if offset == end_limit && offset > start_limit {
            offset - 1
        } else {
            offset
        };
        let Some(target) = self.char_at(probe).map(class) else {
            return;
        };
        let mut start = probe;
        while start > start_limit && self.char_at(start - 1).map(class) == Some(target) {
            start -= 1;
        }
        let mut end = probe;
        while end < end_limit && self.char_at(end).map(class) == Some(target) {
            end += 1;
        }
        self.selection = Selection {
            anchor: start,
            head: end,
        };
        self.goal_column = None;
    }

    pub fn select_line_at(&mut self, offset: usize) {
        let (line, _) = self.position(offset);
        let start = self.line_start(line);
        let end = if line + 1 < self.line_count() {
            self.line_start(line + 1)
        } else {
            self.len_chars()
        };
        self.selection = Selection {
            anchor: start,
            head: end,
        };
        self.goal_column = None;
    }

    // ---- Movement --------------------------------------------------------

    pub fn move_left(&mut self, extend: bool) {
        self.goal_column = None;
        let selection = self.selection;
        if !extend && !selection.is_empty() {
            return self.move_to(selection.range().start, false);
        }
        self.move_to(selection.head.saturating_sub(1), extend);
    }

    pub fn move_right(&mut self, extend: bool) {
        self.goal_column = None;
        let selection = self.selection;
        if !extend && !selection.is_empty() {
            return self.move_to(selection.range().end, false);
        }
        self.move_to(selection.head + 1, extend);
    }

    pub fn move_vertically(&mut self, lines: isize, extend: bool) {
        let (line, column) = self.position(self.selection.head);
        let goal = *self.goal_column.get_or_insert(column);
        let target = line as isize + lines;
        let offset = if target < 0 {
            0
        } else if target as usize >= self.line_count() {
            self.len_chars()
        } else {
            self.offset(target as usize, goal)
        };
        self.move_to(offset, extend);
    }

    pub fn move_up(&mut self, extend: bool) {
        self.move_vertically(-1, extend);
    }

    pub fn move_down(&mut self, extend: bool) {
        self.move_vertically(1, extend);
    }

    /// Home: first non-blank column, or column 0 when already there.
    pub fn move_line_start(&mut self, extend: bool) {
        self.goal_column = None;
        let (line, column) = self.position(self.selection.head);
        let start = self.line_start(line);
        let text = self.line(line);
        let first = text
            .chars()
            .position(|ch| !ch.is_whitespace())
            .unwrap_or(text.chars().count());
        let target = if column == first { 0 } else { first };
        self.move_to(start + target, extend);
    }

    pub fn move_line_end(&mut self, extend: bool) {
        self.goal_column = None;
        let (line, _) = self.position(self.selection.head);
        self.move_to(self.line_start(line) + self.line_len(line), extend);
    }

    pub fn move_doc_start(&mut self, extend: bool) {
        self.goal_column = None;
        self.move_to(0, extend);
    }

    pub fn move_doc_end(&mut self, extend: bool) {
        self.goal_column = None;
        self.move_to(self.len_chars(), extend);
    }

    fn word_left_of(&self, offset: usize) -> usize {
        let mut offset = offset;
        // A line break is its own stop.
        if offset > 0 && self.char_at(offset - 1) == Some('\n') {
            return offset - 1;
        }
        while offset > 0 && self.char_at(offset - 1).map(class) == Some(CharClass::Space) {
            offset -= 1;
        }
        if let Some(target) = offset
            .checked_sub(1)
            .and_then(|o| self.char_at(o))
            .map(class)
            && target != CharClass::Newline
        {
            while offset > 0 && self.char_at(offset - 1).map(class) == Some(target) {
                offset -= 1;
            }
        }
        offset
    }

    fn word_right_of(&self, offset: usize) -> usize {
        let len = self.len_chars();
        let mut offset = offset;
        if self.char_at(offset) == Some('\n') {
            return offset + 1;
        }
        while offset < len && self.char_at(offset).map(class) == Some(CharClass::Space) {
            offset += 1;
        }
        if let Some(target) = self.char_at(offset).map(class)
            && target != CharClass::Newline
        {
            while offset < len && self.char_at(offset).map(class) == Some(target) {
                offset += 1;
            }
        }
        offset
    }

    pub fn move_word_left(&mut self, extend: bool) {
        self.goal_column = None;
        self.move_to(self.word_left_of(self.selection.head), extend);
    }

    pub fn move_word_right(&mut self, extend: bool) {
        self.goal_column = None;
        self.move_to(self.word_right_of(self.selection.head), extend);
    }

    // ---- Editing ---------------------------------------------------------

    /// Apply edits as one undo step. Ranges refer to the text before any of
    /// them is applied and must not overlap.
    fn transact(
        &mut self,
        mut edits: Vec<(Range<usize>, String)>,
        after: Selection,
        kind: EditKind,
    ) {
        let before = self.selection;
        edits.sort_by_key(|edit| std::cmp::Reverse(edit.0.start));
        let mut applied = Vec::with_capacity(edits.len());
        for (range, text) in edits {
            let range = self.clamp(range.start)..self.clamp(range.end);
            if range.is_empty() && text.is_empty() {
                continue;
            }
            let removed = self.rope.slice(range.clone()).to_string();
            self.rope.remove(range.clone());
            self.rope.insert(range.start, &text);
            self.note_change(range.start);
            applied.push(Edit {
                start: range.start,
                removed,
                inserted: text,
            });
        }
        if applied.is_empty() {
            self.set_selection(after);
            return;
        }
        self.revision += 1;
        self.redo.clear();
        self.selection = Selection {
            anchor: self.clamp(after.anchor),
            head: self.clamp(after.head),
        };
        self.goal_column = None;
        let now = Instant::now();
        if let Some(last) = self.undo.last_mut()
            && kind != EditKind::Other
            && last.kind == kind
            && last.after == before
            && last.id != self.saved_state
            && now.duration_since(last.at) < GROUP_WINDOW
        {
            last.edits.extend(applied);
            last.after = self.selection;
            last.at = now;
            return;
        }
        let id = self.next_id;
        self.next_id += 1;
        self.undo.push(Transaction {
            id,
            edits: applied,
            before,
            after: self.selection,
            kind,
            at: now,
        });
    }

    /// Replace a range and leave the caret after the new text.
    pub fn replace(&mut self, range: Range<usize>, text: &str) {
        let end = range.start.min(self.len_chars()) + text.chars().count();
        let kind = if text.chars().count() == 1 && !text.contains('\n') && range.is_empty() {
            EditKind::Typing
        } else {
            EditKind::Other
        };
        self.transact(vec![(range, text.into())], Selection::caret(end), kind);
    }

    /// Replace the selection with typed or pasted text.
    pub fn insert(&mut self, text: &str) {
        let range = self.selection.range();
        let text = text.replace("\r\n", "\n").replace('\r', "\n");
        let kind = if text.chars().count() == 1 && text != "\n" && !text.contains(' ') {
            EditKind::Typing
        } else {
            EditKind::Other
        };
        let end = range.start + text.chars().count();
        self.transact(vec![(range, text)], Selection::caret(end), kind);
    }

    /// Enter: keep the current line's indentation, one level deeper after an
    /// opening bracket.
    pub fn newline(&mut self) {
        let range = self.selection.range();
        let (line, column) = self.position(range.start);
        let text = self.line(line);
        let indent: String = text
            .chars()
            .take(column)
            .take_while(|ch| *ch == ' ' || *ch == '\t')
            .collect();
        let before_cursor: String = text.chars().take(column).collect();
        let opens = matches!(
            before_cursor.trim_end().chars().last(),
            Some('{' | '(' | '[')
        );
        let mut inserted = format!("\n{indent}");
        if opens {
            inserted.push_str(&self.indent_unit.clone());
        }
        let end = range.start + inserted.chars().count();
        self.transact(
            vec![(range, inserted)],
            Selection::caret(end),
            EditKind::Other,
        );
    }

    pub fn backspace(&mut self) {
        let selection = self.selection;
        if !selection.is_empty() {
            return self.delete_selection();
        }
        let head = selection.head;
        if head == 0 {
            return;
        }
        // Inside leading spaces, delete back to the previous indent stop.
        let (line, column) = self.position(head);
        let unit = self.indent_unit.chars().count();
        let leading: String = self.line(line).chars().take(column).collect();
        let start = if self.indent_unit != "\t"
            && unit > 1
            && column > 0
            && leading.chars().all(|ch| ch == ' ')
        {
            let remove = match column % unit {
                0 => unit,
                rest => rest,
            };
            head - remove
        } else {
            head - 1
        };
        self.transact(
            vec![(start..head, String::new())],
            Selection::caret(start),
            EditKind::Deleting,
        );
    }

    pub fn delete_forward(&mut self) {
        let selection = self.selection;
        if !selection.is_empty() {
            return self.delete_selection();
        }
        let head = selection.head;
        if head >= self.len_chars() {
            return;
        }
        self.transact(
            vec![(head..head + 1, String::new())],
            Selection::caret(head),
            EditKind::Deleting,
        );
    }

    pub fn delete_word_left(&mut self) {
        if !self.selection.is_empty() {
            return self.delete_selection();
        }
        let head = self.selection.head;
        let start = self.word_left_of(head);
        self.transact(
            vec![(start..head, String::new())],
            Selection::caret(start),
            EditKind::Other,
        );
    }

    pub fn delete_word_right(&mut self) {
        if !self.selection.is_empty() {
            return self.delete_selection();
        }
        let head = self.selection.head;
        let end = self.word_right_of(head);
        self.transact(
            vec![(head..end, String::new())],
            Selection::caret(head),
            EditKind::Other,
        );
    }

    pub fn delete_to_line_start(&mut self) {
        if !self.selection.is_empty() {
            return self.delete_selection();
        }
        let head = self.selection.head;
        let (line, column) = self.position(head);
        // At column 0 join with the previous line, like other editors.
        let start = if column == 0 {
            head.saturating_sub(1)
        } else {
            self.line_start(line)
        };
        self.transact(
            vec![(start..head, String::new())],
            Selection::caret(start),
            EditKind::Other,
        );
    }

    fn delete_selection(&mut self) {
        let range = self.selection.range();
        self.transact(
            vec![(range.clone(), String::new())],
            Selection::caret(range.start),
            EditKind::Other,
        );
    }

    fn selected_lines(&self) -> Range<usize> {
        let range = self.selection.range();
        let (first, _) = self.position(range.start);
        let (mut last, last_column) = self.position(range.end);
        // A selection ending at column 0 does not include that line.
        if last > first && last_column == 0 {
            last -= 1;
        }
        first..last + 1
    }

    /// Tab: indent the selected lines, or insert one indent unit.
    pub fn indent(&mut self) {
        let selection = self.selection;
        let lines = self.selected_lines();
        if selection.is_empty() || lines.len() == 1 && !self.spans_whole_line(selection) {
            let unit = self.indent_unit.clone();
            let text = if unit == "\t" {
                unit
            } else {
                // Pad to the next indent stop.
                let width = unit.chars().count();
                let (_, column) = self.position(selection.range().start);
                " ".repeat(width - column % width)
            };
            return self.insert(&text);
        }
        let unit = self.indent_unit.clone();
        let edits = lines
            .clone()
            .filter(|line| self.line_len(*line) > 0)
            .map(|line| {
                let start = self.line_start(line);
                (start..start, unit.clone())
            })
            .collect::<Vec<_>>();
        // Text inserted before an offset pushes it right; an offset at a
        // line start stays put so the selection grows to cover the indent.
        let shift = |offset: usize| {
            offset
                + unit.chars().count()
                    * edits
                        .iter()
                        .filter(|(range, _)| range.start < offset)
                        .count()
        };
        let after = Selection {
            anchor: shift(selection.anchor),
            head: shift(selection.head),
        };
        self.transact(edits, after, EditKind::Other);
    }

    fn spans_whole_line(&self, selection: Selection) -> bool {
        let range = selection.range();
        let (line, column) = self.position(range.start);
        column == 0 && range.end >= self.line_start(line) + self.line_len(line)
    }

    /// Shift+Tab: remove one indent level from the selected lines.
    pub fn outdent(&mut self) {
        let selection = self.selection;
        let width = if self.indent_unit == "\t" {
            1
        } else {
            self.indent_unit.chars().count()
        };
        let mut edits = Vec::new();
        for line in self.selected_lines() {
            let text = self.line(line);
            let remove = if text.starts_with('\t') {
                1
            } else {
                text.chars().take(width).take_while(|ch| *ch == ' ').count()
            };
            if remove > 0 {
                let start = self.line_start(line);
                edits.push((start..start + remove, String::new()));
            }
        }
        let shift = |offset: usize| {
            offset
                - edits
                    .iter()
                    .map(|(range, _): &(Range<usize>, String)| {
                        if range.end <= offset {
                            range.len()
                        } else {
                            offset.saturating_sub(range.start)
                        }
                    })
                    .sum::<usize>()
        };
        let after = Selection {
            anchor: shift(selection.anchor),
            head: shift(selection.head),
        };
        self.transact(edits, after, EditKind::Other);
    }

    /// Text to copy: the selection, or the whole current line when empty.
    pub fn copy_text(&self) -> String {
        if self.selection.is_empty() {
            let (line, _) = self.position(self.selection.head);
            format!("{}\n", self.line(line))
        } else {
            self.selected_text()
        }
    }

    /// Cut the selection, or the whole current line when empty.
    pub fn cut(&mut self) -> String {
        let text = self.copy_text();
        if self.selection.is_empty() {
            let (line, _) = self.position(self.selection.head);
            let start = self.line_start(line);
            let end = if line + 1 < self.line_count() {
                self.line_start(line + 1)
            } else {
                self.len_chars()
            };
            // On the last line, also take the newline before it.
            let start = if end == self.len_chars() && line > 0 {
                start - 1
            } else {
                start
            };
            self.transact(
                vec![(start..end, String::new())],
                Selection::caret(start.min(self.len_chars())),
                EditKind::Other,
            );
        } else {
            self.delete_selection();
        }
        text
    }

    pub fn undo(&mut self) -> bool {
        let Some(transaction) = self.undo.pop() else {
            return false;
        };
        for edit in transaction.edits.iter().rev() {
            let end = edit.start + edit.inserted.chars().count();
            self.rope.remove(edit.start..end);
            self.rope.insert(edit.start, &edit.removed);
            self.note_change(edit.start);
        }
        self.selection = transaction.before;
        self.goal_column = None;
        self.revision += 1;
        self.redo.push(transaction);
        true
    }

    pub fn redo(&mut self) -> bool {
        let Some(mut transaction) = self.redo.pop() else {
            return false;
        };
        for edit in &transaction.edits {
            let end = edit.start + edit.removed.chars().count();
            self.rope.remove(edit.start..end);
            self.rope.insert(edit.start, &edit.inserted);
            self.note_change(edit.start);
        }
        self.selection = transaction.after;
        self.goal_column = None;
        self.revision += 1;
        // A redone step never merges with later typing.
        transaction.at = Instant::now() - GROUP_WINDOW;
        self.undo.push(transaction);
        true
    }
}

/// Tabs when most indented lines use them, otherwise the smallest common
/// space step (2 or 4).
fn detect_indent(text: &str) -> String {
    let mut tabs = 0usize;
    let mut spaces = 0usize;
    let mut two = 0usize;
    for line in text.lines().take(2000) {
        if line.starts_with('\t') {
            tabs += 1;
        } else {
            let count = line.chars().take_while(|ch| *ch == ' ').count();
            if count > 0 && line.len() > count {
                spaces += 1;
                if count % 4 != 0 && count % 2 == 0 {
                    two += 1;
                }
            }
        }
    }
    if tabs > spaces {
        "\t".into()
    } else if spaces > 0 && two * 3 >= spaces {
        "  ".into()
    } else {
        "    ".into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn buffer(text: &str, cursor: usize) -> TextBuffer {
        let mut buffer = TextBuffer::new(text);
        buffer.set_selection(Selection::caret(cursor));
        buffer
    }

    #[test]
    fn typing_groups_into_one_undo_step_and_tracks_dirty_state() {
        let mut b = buffer("", 0);
        assert!(!b.is_dirty());
        for ch in "abc".chars() {
            b.insert(&ch.to_string());
        }
        assert_eq!(b.text(), "abc");
        assert!(b.is_dirty());
        b.mark_saved();
        assert!(!b.is_dirty());
        b.insert("d");
        assert!(b.is_dirty(), "typing after a save starts a new step");
        assert!(b.undo());
        assert_eq!(b.text(), "abc");
        assert!(!b.is_dirty(), "undoing back to the saved text is clean");
        assert!(b.undo());
        assert_eq!(b.text(), "");
        assert!(b.is_dirty());
        assert!(b.redo());
        assert_eq!(b.text(), "abc");
        assert_eq!(b.selection(), Selection::caret(3));
        assert!(!b.is_dirty());
    }

    #[test]
    fn saving_marks_the_state_at_save_start_and_reports_changed_offsets() {
        let mut b = buffer("abc\ndef", 5);
        assert_eq!(b.take_changed_from(), None);
        b.insert("X");
        let token = b.state_token();
        b.set_selection(Selection::caret(1));
        b.backspace();
        assert_eq!(b.take_changed_from(), Some(0));
        assert_eq!(b.take_changed_from(), None);
        // The save finished after more typing: still dirty.
        b.mark_saved_at(token);
        assert!(b.is_dirty());
        b.undo();
        assert!(!b.is_dirty());
        assert_eq!(b.take_changed_from(), Some(0));
    }

    #[test]
    fn edits_replace_the_selection_and_clear_redo() {
        let mut b = buffer("hello world", 0);
        b.set_selection(Selection {
            anchor: 6,
            head: 11,
        });
        b.insert("there");
        assert_eq!(b.text(), "hello there");
        b.undo();
        assert_eq!(b.text(), "hello world");
        assert_eq!(
            b.selection(),
            Selection {
                anchor: 6,
                head: 11
            }
        );
        b.insert("x");
        assert!(!b.redo(), "a new edit discards the redo stack");
    }

    #[test]
    fn newline_keeps_indentation_and_indents_after_openers() {
        let mut b = buffer("fn main() {\n    let x = 1;\n}", 11);
        b.newline();
        assert_eq!(b.text(), "fn main() {\n    \n    let x = 1;\n}");
        let mut b = buffer("    let x = 1;", 14);
        b.newline();
        assert_eq!(b.text(), "    let x = 1;\n    ");
        assert_eq!(b.selection(), Selection::caret(19));
    }

    #[test]
    fn backspace_removes_whole_indent_steps_in_leading_spaces() {
        let mut b = buffer("fn a() {\n        x\n}", 17);
        assert_eq!(b.indent_unit(), "    ");
        b.backspace();
        assert_eq!(b.text(), "fn a() {\n    x\n}");
        b.backspace();
        assert_eq!(b.text(), "fn a() {\nx\n}");
        b.backspace();
        assert_eq!(b.text(), "fn a() {x\n}");
    }

    #[test]
    fn vertical_movement_remembers_the_goal_column() {
        let mut b = buffer("long line here\nab\nanother long", 10);
        b.move_down(false);
        assert_eq!(b.position(b.selection().head), (1, 2));
        b.move_down(false);
        assert_eq!(b.position(b.selection().head), (2, 10));
        b.move_up(true);
        assert_eq!(b.selection().anchor, b.offset(2, 10));
        b.move_vertically(-10, false);
        assert_eq!(b.selection(), Selection::caret(0));
        b.move_vertically(10, false);
        assert_eq!(b.selection(), Selection::caret(b.len_chars()));
    }

    #[test]
    fn word_and_line_movement() {
        let mut b = buffer("    let foo_bar = baz(1);", 4);
        b.move_word_right(false);
        assert_eq!(b.selection().head, 7);
        b.move_word_right(false);
        assert_eq!(b.selection().head, 15);
        b.move_word_left(false);
        assert_eq!(b.selection().head, 8);
        b.move_line_end(false);
        assert_eq!(b.selection().head, 25);
        b.move_line_start(false);
        assert_eq!(b.selection().head, 4, "Home goes to the first non-blank");
        b.move_line_start(false);
        assert_eq!(b.selection().head, 0, "then to column 0");
        b.move_line_start(true);
        assert_eq!(b.selection(), Selection { anchor: 0, head: 4 });
        b.move_left(false);
        assert_eq!(
            b.selection(),
            Selection::caret(0),
            "left collapses to the start"
        );

        let mut b = buffer("ab\ncd", 3);
        b.move_word_left(false);
        assert_eq!(b.selection().head, 2, "a line break is its own stop");
        b.delete_word_right();
        assert_eq!(b.text(), "abcd");
    }

    #[test]
    fn word_and_line_selection() {
        let mut b = buffer("let foo_bar = 1;\nnext", 0);
        b.select_word_at(6);
        assert_eq!(b.selected_text(), "foo_bar");
        b.select_word_at(16);
        assert_eq!(b.selected_text(), ";");
        b.select_line_at(2);
        assert_eq!(b.selected_text(), "let foo_bar = 1;\n");
        b.select_all();
        assert_eq!(b.selected_text(), b.text());
    }

    #[test]
    fn tab_indents_lines_and_shift_tab_outdents_them() {
        let mut b = buffer("a\nb\n\nc", 0);
        b.set_selection(Selection { anchor: 0, head: 6 });
        b.indent();
        assert_eq!(b.text(), "    a\n    b\n\n    c", "blank lines stay blank");
        assert_eq!(b.selected_text(), "    a\n    b\n\n    c");
        b.outdent();
        assert_eq!(b.text(), "a\nb\n\nc");
        assert_eq!(b.selection(), Selection { anchor: 0, head: 6 });
        b.undo();
        b.undo();
        assert_eq!(b.text(), "a\nb\n\nc");

        let mut b = buffer("ab", 1);
        b.indent();
        assert_eq!(b.text(), "a   b", "pads to the next indent stop");
    }

    #[test]
    fn detects_tab_and_two_space_indentation() {
        assert_eq!(TextBuffer::new("fn a() {\n\tx\n\ty\n}").indent_unit(), "\t");
        assert_eq!(
            TextBuffer::new("{\n  \"a\": {\n    \"b\": 1\n  }\n}").indent_unit(),
            "  "
        );
        assert_eq!(TextBuffer::new("plain").indent_unit(), "    ");
    }

    #[test]
    fn crlf_files_edit_with_lf_and_save_with_crlf() {
        let mut b = TextBuffer::new("a\r\nb\r\n");
        assert_eq!(b.line_ending(), LineEnding::CrLf);
        assert_eq!(b.line_count(), 3);
        assert_eq!(b.line(1), "b");
        b.set_selection(Selection::caret(1));
        b.newline();
        assert_eq!(b.text_for_save(), "a\r\n\r\nb\r\n");
        b.insert("x\r\ny");
        assert_eq!(b.text(), "a\nx\ny\nb\n", "pasted CRLF is normalized");
    }

    #[test]
    fn copy_and_cut_take_the_whole_line_without_a_selection() {
        let mut b = buffer("one\ntwo\nthree", 5);
        assert_eq!(b.copy_text(), "two\n");
        assert_eq!(b.cut(), "two\n");
        assert_eq!(b.text(), "one\nthree");
        b.set_selection(Selection::caret(b.len_chars()));
        assert_eq!(b.cut(), "three\n");
        assert_eq!(b.text(), "one");
    }

    #[test]
    fn utf16_offsets_round_trip_through_multibyte_text() {
        let b = TextBuffer::new("ğü😀x");
        assert_eq!(b.offset_to_utf16(3), 4, "the emoji is two UTF-16 units");
        assert_eq!(b.offset_from_utf16(4), 3);
        assert_eq!(b.offset_from_utf16(999), 4);
        assert_eq!(b.line(0), "ğü😀x");
    }

    #[test]
    fn delete_to_line_start_joins_lines_at_column_zero() {
        let mut b = buffer("ab\ncd", 5);
        b.delete_to_line_start();
        assert_eq!(b.text(), "ab\n");
        b.delete_to_line_start();
        assert_eq!(b.text(), "ab");
    }

    #[test]
    fn load_and_atomic_save_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.txt");
        std::fs::write(&path, "x\r\ny").unwrap();
        let mut b = load(&path).unwrap();
        b.set_selection(Selection::caret(b.len_chars()));
        b.insert("z");
        save_atomic(&path, &b.text_for_save()).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "x\r\nyz");

        std::fs::write(dir.path().join("bin"), [0u8, 1, 2]).unwrap();
        assert_eq!(load(&dir.path().join("bin")).err(), Some(LoadError::Binary));
        assert!(matches!(
            load(&dir.path().join("missing")).err(),
            Some(LoadError::Io(_))
        ));
    }
}
