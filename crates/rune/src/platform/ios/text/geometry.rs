//! Where the focused field's characters are, for the keyboard. UIKit asks a text input for the
//! caret's rect at a position, the position nearest a point and the position a line up or
//! down: the space bar's trackpad, the dictation and candidate bubbles and a hardware
//! keyboard's selection all move by them. The answers come from the field's laid-out lines,
//! the same ones the accessibility tree gives a screen reader.

use std::ops::Range;

use crate::runtime::a11y_text::Line;

/// The focused field's lines, top to bottom, in the view's points. Offsets are bytes of the
/// field's text.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Geometry {
    rows: Vec<Row>,
}

#[derive(Clone, Debug, PartialEq)]
struct Row {
    bytes: Range<usize>,
    top: f32,
    height: f32,
    /// Each place a caret can stand on the line: its byte and its x, left to right in text
    /// order, the line's end last.
    stops: Vec<(usize, f32)>,
}

impl Geometry {
    /// `lines` as the runtime lays them out, already in the view's points.
    pub(crate) fn new(lines: &[Line]) -> Self {
        let rows = lines.iter().map(|line| {
            let mut at = line.range.start;
            let mut stops = Vec::with_capacity(line.lengths.len() + 1);
            for (length, x) in line.lengths.iter().zip(&line.positions) {
                stops.push((at, line.rect[0] + x));
                at += *length as usize;
            }
            // The end of the line: after its last character, or for a line that ends in a
            // newline, before it, where a caret at the line's end stands.
            let newline = line.lengths.last().zip(line.widths.last()).is_some_and(|(length, width)| *length == 1 && *width == 0.0 && stops.len() > 1);
            if !newline { stops.push((line.range.end, line.rect[0] + line.rect[2])); }
            Row { bytes: line.range.clone(), top: line.rect[1], height: line.rect[3], stops }
        }).collect();
        Geometry { rows }
    }

    /// The row a caret at `byte` stands on: the first whose stops reach it, else the last.
    fn row(&self, byte: usize) -> Option<&Row> {
        self.rows.iter().find(|row| row.stops.last().is_some_and(|(end, _)| byte <= *end)).or(self.rows.last())
    }

    fn x(row: &Row, byte: usize) -> f32 {
        row.stops.iter().rev().find(|(at, _)| *at <= byte).or(row.stops.first()).map_or(0.0, |(_, x)| *x)
    }

    /// The caret's line at `byte`: left, top, width and height. The width is nothing: the
    /// field draws its own caret.
    pub(crate) fn caret(&self, byte: usize) -> Option<[f32; 4]> {
        let row = self.row(byte)?;
        Some([Self::x(row, byte), row.top, 0.0, row.height])
    }

    /// What `range` covers on its first line.
    pub(crate) fn first_rect(&self, range: Range<usize>) -> Option<[f32; 4]> {
        let row = self.row(range.start)?;
        let (from, last) = (Self::x(row, range.start), row.stops.last()?);
        let to = if range.end <= last.0 { Self::x(row, range.end) } else { last.1 };
        Some([from.min(to), row.top, (to - from).abs(), row.height])
    }

    /// The byte whose caret is nearest (x, y): on the line y falls in, or the nearest one.
    pub(crate) fn closest(&self, x: f32, y: f32) -> Option<usize> {
        let distance = |row: &Row| if y < row.top { row.top - y } else { (y - row.top - row.height).max(0.0) };
        let row = self.rows.iter().min_by(|a, b| distance(a).total_cmp(&distance(b)))?;
        row.stops.iter().min_by(|a, b| (a.1 - x).abs().total_cmp(&(b.1 - x).abs())).map(|(byte, _)| *byte)
    }

    /// The byte `lines` lines below `byte` (above, when negative) at the same x, held to the
    /// first and last line.
    pub(crate) fn vertical(&self, byte: usize, lines: isize) -> Option<usize> {
        let from = self.row(byte)?;
        let index = self.rows.iter().position(|row| std::ptr::eq(row, from))?;
        let to = &self.rows[index.saturating_add_signed(lines).min(self.rows.len() - 1)];
        let x = Self::x(from, byte);
        to.stops.iter().min_by(|a, b| (a.1 - x).abs().total_cmp(&(b.1 - x).abs())).map(|(byte, _)| *byte)
    }
}

#[cfg(test)]
#[path = "geometry_tests.rs"]
mod tests;
