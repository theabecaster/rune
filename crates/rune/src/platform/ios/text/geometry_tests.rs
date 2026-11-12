use super::*;

/// A line of one-byte characters, each `width` wide, from `left`.
fn line(range: Range<usize>, left: f32, top: f32, width: f32, newline: bool) -> Line {
    let count = range.len();
    let drawn = if newline { count - 1 } else { count };
    let mut widths = vec![width; drawn];
    if newline { widths.push(0.0); }
    Line {
        range, rect: [left, top, drawn as f32 * width, 20.0], lengths: vec![1; count],
        positions: (0..drawn).map(|k| k as f32 * width).chain(newline.then_some(drawn as f32 * width)).collect(),
        widths, word_starts: Vec::new(),
    }
}

/// "abcd\n" over "efgh", ten points a character, from x 100, lines twenty tall from y 50.
fn two_lines() -> Geometry {
    Geometry::new(&[line(0..5, 100.0, 50.0, 10.0, true), line(5..9, 100.0, 70.0, 10.0, false)])
}

#[test]
fn a_caret_stands_at_its_characters_left_edge_on_its_line() {
    let g = two_lines();
    assert_eq!(g.caret(0), Some([100.0, 50.0, 0.0, 20.0]));
    assert_eq!(g.caret(2), Some([120.0, 50.0, 0.0, 20.0]));
    assert_eq!(g.caret(4), Some([140.0, 50.0, 0.0, 20.0]), "before the newline, at the first line's end");
    assert_eq!(g.caret(5), Some([100.0, 70.0, 0.0, 20.0]), "after it, at the second line's start");
    assert_eq!(g.caret(9), Some([140.0, 70.0, 0.0, 20.0]));
    assert_eq!(g.caret(99), Some([140.0, 70.0, 0.0, 20.0]), "past the end is the end");
}

#[test]
fn an_empty_field_has_a_caret_on_its_one_empty_line() {
    let empty = Line { range: 0..0, rect: [16.0, 8.0, 0.0, 20.0], lengths: Vec::new(), positions: Vec::new(), widths: Vec::new(), word_starts: Vec::new() };
    let g = Geometry::new(&[empty]);
    assert_eq!(g.caret(0), Some([16.0, 8.0, 0.0, 20.0]));
    assert_eq!(g.closest(300.0, 300.0), Some(0));
    assert_eq!(Geometry::default().caret(0), None);
}

#[test]
fn the_closest_position_is_on_the_line_under_the_point_at_the_nearest_edge() {
    let g = two_lines();
    assert_eq!(g.closest(118.0, 60.0), Some(2));
    assert_eq!(g.closest(113.0, 60.0), Some(1));
    assert_eq!(g.closest(500.0, 55.0), Some(4), "right of the first line is its end, before the newline");
    assert_eq!(g.closest(0.0, 80.0), Some(5));
    assert_eq!(g.closest(126.0, 500.0), Some(8), "below the text is the last line");
    assert_eq!(g.closest(126.0, -500.0), Some(3), "above it the first");
}

#[test]
fn a_line_up_or_down_keeps_the_x_and_stops_at_the_first_and_last_line() {
    let g = two_lines();
    assert_eq!(g.vertical(2, 1), Some(7));
    assert_eq!(g.vertical(7, -1), Some(2));
    assert_eq!(g.vertical(7, 1), Some(7));
    assert_eq!(g.vertical(2, -5), Some(2));
}

#[test]
fn a_ranges_first_rect_ends_with_the_range_or_its_first_line() {
    let g = two_lines();
    assert_eq!(g.first_rect(1..3), Some([110.0, 50.0, 20.0, 20.0]));
    assert_eq!(g.first_rect(2..8), Some([120.0, 50.0, 20.0, 20.0]), "to the first line's end");
    assert_eq!(g.first_rect(6..6), Some([110.0, 70.0, 0.0, 20.0]));
}
