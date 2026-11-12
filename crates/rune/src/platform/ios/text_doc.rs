//! The document the iOS keyboard edits. UIKit's text input asks a first responder for its text,
//! selection and marked text at any moment and expects each edit it makes to show in the next
//! answer, while the runtime applies input on its next frame. So the view keeps the keyboard's
//! copy here: an edit changes it at once and returns what to tell the runtime, and after each
//! frame the runtime's own text is compared with it. When they differ (the app cleared the
//! field, a handler changed what was typed, the field lost focus), the copy takes the
//! runtime's and the keyboard is told its text changed.
//!
//! The copy is the field's text around its selection, as the runtime publishes it for any
//! input method (a paragraph, at most a KiB either side), with the marked text, which the
//! runtime draws at the caret and keeps out of its text, spliced in over the selection.
//! UIKit counts in UTF-16 code units; everything here is in bytes except at the edges.

use std::ops::Range;

use unicode_segmentation::UnicodeSegmentation;

use crate::Key;
use crate::machine::Effects;

/// What the keyboard believes the field holds.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Document {
    /// The field's text around the selection, without any marked text.
    text: String,
    /// Where `text` starts in the field's text, in bytes.
    start: usize,
    phase: Phase,
    /// The field shows no text to the keyboard (a password): edits go through, the copy
    /// stays empty.
    hidden: bool,
}

#[derive(Clone, Debug, PartialEq)]
enum Phase {
    /// No marked text. `selection` is a byte range of the text.
    Plain { selection: Range<usize> },
    /// Marked text stands over byte range `over` of the text, with `selection` a byte range of
    /// the marked text.
    Marked { over: Range<usize>, marked: String, selection: Range<usize> },
}

impl Default for Phase {
    fn default() -> Self { Phase::Plain { selection: 0..0 } }
}

/// What the runtime says the focused field holds, after a frame.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Snapshot {
    /// Its text around the selection, where that starts in the field, and the selection in it.
    Field { text: String, start: usize, selection: Range<usize> },
    /// It is composing: its text is as the keyboard left it.
    Composing,
    /// It shows the keyboard nothing: a password, or a caret an app draws itself.
    Hidden,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Event {
    /// The keyboard typed or committed `text` over the marked text or the selection.
    Insert(String),
    /// The keyboard's delete key.
    DeleteBackward,
    /// The keyboard replaced UTF-16 range `range` of the document: an autocorrection, a
    /// suggestion, a dictation's rewrite.
    Replace { range: Range<usize>, text: String },
    /// The keyboard marked `text` over the marked text or the selection, with UTF-16 range
    /// `selection` of it selected. Empty text ends the marking and leaves nothing.
    SetMarked { text: String, selection: Range<usize> },
    /// The keyboard committed the marked text as it stands.
    Unmark,
    /// The keyboard moved the selection to UTF-16 range `range` of the document.
    Select(Range<usize>),
    /// A frame ended with the field holding this.
    Runtime(Snapshot),
}

/// What an event asks the view to do, in order.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Effect {
    /// Send the runtime this key.
    Key(Key),
    /// Send the runtime this committed text, which replaces its composition or selection.
    Commit(String),
    /// Send the runtime this composition, with the byte range of its cursor or selection.
    Preedit(String, Option<(usize, usize)>),
    /// Have the field select this byte range of its text.
    Select(Range<usize>),
    /// Tell the keyboard the document's text changed under it.
    TextChanged,
    /// Tell the keyboard only the selection moved.
    SelectionChanged,
}

impl Document {
    /// The document as the keyboard sees it: the text with any marked text spliced in.
    pub(crate) fn string(&self) -> String {
        match &self.phase {
            Phase::Plain { .. } => self.text.clone(),
            Phase::Marked { over, marked, .. } => [&self.text[..over.start], marked, &self.text[over.end..]].concat(),
        }
    }

    /// The selection, as a UTF-16 range of the document.
    pub(crate) fn selection(&self) -> Range<usize> {
        let string = self.string();
        let bytes = match &self.phase {
            Phase::Plain { selection } => selection.clone(),
            Phase::Marked { over, selection, .. } => over.start + selection.start..over.start + selection.end,
        };
        utf16(&string, bytes.start)..utf16(&string, bytes.end)
    }

    /// The marked text, as a UTF-16 range of the document.
    pub(crate) fn marked(&self) -> Option<Range<usize>> {
        let Phase::Marked { over, marked, .. } = &self.phase else { return None };
        let string = self.string();
        Some(utf16(&string, over.start)..utf16(&string, over.start + marked.len()))
    }

    /// The document's length in UTF-16 code units.
    pub(crate) fn len(&self) -> usize {
        self.string().encode_utf16().count()
    }

    /// UTF-16 range `range` of the document, held to its ends and to whole characters.
    pub(crate) fn slice(&self, range: Range<usize>) -> String {
        let string = self.string();
        let (a, b) = (byte(&string, range.start), byte(&string, range.end));
        string[a..b.max(a)].to_owned()
    }

    /// UTF-16 offset `at` of the document as a byte of the field's text, for asking the field
    /// where it is. None while text is marked or hidden: the field's text is not the
    /// document's then.
    pub(crate) fn field_byte(&self, at: usize) -> Option<usize> {
        let Phase::Plain { .. } = self.phase else { return None };
        (!self.hidden).then(|| self.start + byte(&self.text, at))
    }

    /// Byte `at` of the field's text as a UTF-16 offset of the document, held to its ends.
    pub(crate) fn offset_of(&self, at: usize) -> usize {
        let within = at.saturating_sub(self.start).min(self.text.len());
        // A byte inside a character is that character's start.
        let within = (0..=within).rev().find(|&i| self.text.is_char_boundary(i)).unwrap_or(0);
        utf16(&self.text, within)
    }

    pub(crate) fn step(&mut self, event: Event) -> Effects<Effect> {
        match event {
            Event::Insert(text) => {
                let composing = matches!(self.phase, Phase::Marked { .. });
                self.write(&text);
                Effects::one(match text.as_str() {
                    _ if composing => Effect::Commit(text),
                    "\n" => Effect::Key(Key::Enter),
                    "\t" => Effect::Key(Key::Tab),
                    _ => Effect::Key(Key::Text(text)),
                })
            }
            Event::DeleteBackward => match self.phase.clone() {
                Phase::Marked { over, mut marked, selection } => {
                    let end = selection.end.min(marked.len());
                    let from = if selection.is_empty() { previous_grapheme(&marked, end) } else { selection.start };
                    marked.replace_range(from..end, "");
                    self.mark(over, marked, from..from)
                }
                Phase::Plain { selection } => {
                    let from = if selection.is_empty() { previous_grapheme(&self.text, selection.start) } else { selection.start };
                    if !self.hidden {
                        self.text.replace_range(from..selection.end, "");
                        self.phase = Phase::Plain { selection: from..from };
                    }
                    Effects::one(Effect::Key(Key::Backspace))
                }
            },
            Event::Replace { range, text } => {
                let mut effects = self.unmark();
                let string = self.string();
                let bytes = byte(&string, range.start)..byte(&string, range.end).max(byte(&string, range.start));
                effects.push(Effect::Select(self.start + bytes.start..self.start + bytes.end));
                self.phase = Phase::Plain { selection: bytes };
                self.write(&text);
                // The runtime writes nothing for an empty commit, so a replacement with
                // nothing is a delete of what was selected.
                effects.push(if text.is_empty() { Effect::Key(Key::Backspace) } else { Effect::Commit(text) });
                effects
            }
            Event::SetMarked { text, selection } => {
                let over = match &self.phase {
                    Phase::Plain { selection } => selection.clone(),
                    Phase::Marked { over, .. } => over.clone(),
                };
                let selection = byte(&text, selection.start)..byte(&text, selection.end).max(byte(&text, selection.start));
                self.mark(over, text, selection)
            }
            Event::Unmark => self.unmark(),
            Event::Select(range) => {
                let string = self.string();
                let bytes = byte(&string, range.start)..byte(&string, range.end).max(byte(&string, range.start));
                if let Phase::Marked { over, marked, .. } = self.phase.clone() {
                    let inside = over.start..over.start + marked.len();
                    if bytes.start >= inside.start && bytes.end <= inside.end {
                        return self.mark(over.clone(), marked, bytes.start - over.start..bytes.end - over.start);
                    }
                }
                // A selection outside the marked text ends the marking first, which shifts
                // nothing: the marked text becomes text where it stood.
                let mut effects = self.unmark();
                self.phase = Phase::Plain { selection: bytes.clone() };
                effects.push(Effect::Select(self.start + bytes.start..self.start + bytes.end));
                effects
            }
            Event::Runtime(snapshot) => self.reconcile(snapshot),
        }
    }

    /// Writes `text` over the marked text or the selection and leaves the caret after it.
    fn write(&mut self, text: &str) {
        let over = match &self.phase {
            Phase::Plain { selection } => selection.clone(),
            Phase::Marked { over, .. } => over.clone(),
        };
        if self.hidden { return; }
        self.text.replace_range(over.clone(), text);
        let caret = over.start + text.len();
        self.phase = Phase::Plain { selection: caret..caret };
    }

    /// Marks `marked` over `over`, or ends the marking when it is empty.
    fn mark(&mut self, over: Range<usize>, marked: String, selection: Range<usize>) -> Effects<Effect> {
        let cursor = (!marked.is_empty()).then_some((selection.start, selection.end));
        let effect = Effect::Preedit(marked.clone(), cursor);
        if marked.is_empty() {
            // The marked text stood over the selection, which is gone with it.
            if !self.hidden { self.text.replace_range(over.clone(), ""); }
            self.phase = Phase::Plain { selection: over.start..over.start };
        } else {
            self.phase = Phase::Marked { over, marked, selection };
        }
        Effects::one(effect)
    }

    /// Makes the marked text part of the text, where it stands.
    fn unmark(&mut self) -> Effects<Effect> {
        let Phase::Marked { marked, .. } = self.phase.clone() else { return Effects::none() };
        self.write(&marked);
        Effects::one(Effect::Commit(marked))
    }

    fn reconcile(&mut self, snapshot: Snapshot) -> Effects<Effect> {
        let before = self.clone();
        match snapshot {
            Snapshot::Composing => return Effects::none(),
            Snapshot::Hidden => *self = Document { hidden: true, ..Document::default() },
            Snapshot::Field { text, start, selection } => {
                *self = Document { text, start, phase: Phase::Plain { selection }, hidden: false };
            }
        }
        if *self == before {
            Effects::none()
        } else if self.string() == before.string() && self.start == before.start && self.marked() == before.marked() {
            Effects::one(Effect::SelectionChanged)
        } else {
            Effects::one(Effect::TextChanged)
        }
    }
}

/// Byte offset `at` of `text` as a count of UTF-16 code units.
fn utf16(text: &str, at: usize) -> usize {
    text[..at].encode_utf16().count()
}

/// UTF-16 offset `at` of `text` as a byte offset, held to the text's end; one inside a
/// character's surrogate pair is that character's start.
fn byte(text: &str, at: usize) -> usize {
    let mut units = 0;
    for (index, c) in text.char_indices() {
        if units + c.len_utf16() > at { return index; }
        units += c.len_utf16();
    }
    text.len()
}

/// Where the grapheme that ends at byte `at` of `text` starts.
fn previous_grapheme(text: &str, at: usize) -> usize {
    text[..at].grapheme_indices(true).next_back().map_or(0, |(index, _)| index)
}

#[cfg(test)]
#[path = "text_doc_tests.rs"]
mod tests;
