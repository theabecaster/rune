use super::*;

fn field(text: &str, selection: Range<usize>) -> Document {
    let mut doc = Document::default();
    doc.step(Event::Runtime(Snapshot::Field { text: text.into(), start: 0, selection }));
    doc
}

fn run(doc: &mut Document, event: Event) -> Vec<Effect> {
    doc.step(event).into_iter().collect()
}

fn text(s: &str) -> Effect { Effect::Key(Key::Text(s.into())) }

#[test]
fn typing_goes_to_the_runtime_as_keys_and_shows_in_the_document_at_once() {
    let mut doc = field("helo", 3..3);
    assert_eq!(run(&mut doc, Event::Insert("l".into())), [text("l")]);
    assert_eq!((doc.string().as_str(), doc.selection()), ("hello", 4..4));
    assert_eq!(run(&mut doc, Event::Insert("\n".into())), [Effect::Key(Key::Enter)]);
    assert_eq!(run(&mut doc, Event::Insert("\t".into())), [Effect::Key(Key::Tab)]);
}

#[test]
fn typing_over_a_selection_replaces_it() {
    let mut doc = field("hello world", 6..11);
    run(&mut doc, Event::Insert("you".into()));
    assert_eq!((doc.string().as_str(), doc.selection()), ("hello you", 9..9));
}

#[test]
fn delete_takes_the_selection_or_the_whole_grapheme_before_the_caret() {
    let mut doc = field("a👨‍👩‍👧b", "a👨‍👩‍👧".len().."a👨‍👩‍👧".len());
    assert_eq!(run(&mut doc, Event::DeleteBackward), [Effect::Key(Key::Backspace)]);
    assert_eq!((doc.string().as_str(), doc.selection()), ("ab", 1..1));
    let mut doc = field("hello", 1..4);
    run(&mut doc, Event::DeleteBackward);
    assert_eq!((doc.string().as_str(), doc.selection()), ("ho", 1..1));
    let mut doc = field("hello", 0..0);
    assert_eq!(run(&mut doc, Event::DeleteBackward), [Effect::Key(Key::Backspace)], "the runtime decides what a delete at the start does");
    assert_eq!(doc.string(), "hello");
}

#[test]
fn offsets_are_utf16_code_units() {
    let doc = field("a😀b", "a😀".len().."a😀".len());
    assert_eq!((doc.len(), doc.selection()), (4, 3..3));
    assert_eq!(doc.slice(1..3), "😀");
    assert_eq!(doc.slice(2..4), "😀b", "an offset inside a surrogate pair is the character's start");
    assert_eq!(doc.slice(3..99), "b");
}

#[test]
fn offsets_map_to_the_fields_bytes_and_back_except_while_text_is_marked_or_hidden() {
    let mut doc = Document::default();
    doc.step(Event::Runtime(Snapshot::Field { text: "é😀x".into(), start: 100, selection: 0..0 }));
    assert_eq!((doc.field_byte(1), doc.field_byte(3), doc.field_byte(99)), (Some(102), Some(106), Some(107)));
    assert_eq!((doc.offset_of(102), doc.offset_of(106), doc.offset_of(104), doc.offset_of(5), doc.offset_of(999)), (1, 3, 1, 0, 4));
    run(&mut doc, Event::SetMarked { text: "か".into(), selection: 1..1 });
    assert_eq!(doc.field_byte(1), None);
    doc.step(Event::Runtime(Snapshot::Hidden));
    assert_eq!(doc.field_byte(0), None);
}

#[test]
fn an_autocorrection_selects_the_word_in_the_field_then_commits_over_it() {
    let mut doc = Document::default();
    doc.step(Event::Runtime(Snapshot::Field { text: "teh cat".into(), start: 40, selection: 7..7 }));
    let effects = run(&mut doc, Event::Replace { range: 0..3, text: "the".into() });
    assert_eq!(effects, [Effect::Select(40..43), Effect::Commit("the".into())]);
    assert_eq!((doc.string().as_str(), doc.selection()), ("the cat", 3..3));
}

#[test]
fn a_replacement_with_nothing_deletes_the_range() {
    let mut doc = field("hello world", 11..11);
    let effects = run(&mut doc, Event::Replace { range: 5..11, text: String::new() });
    assert_eq!(effects, [Effect::Select(5..11), Effect::Key(Key::Backspace)]);
    assert_eq!(doc.string(), "hello");
}

#[test]
fn marked_text_stands_in_the_document_over_the_selection_and_reaches_the_runtime_as_a_composition() {
    let mut doc = field("ab", 1..1);
    let effects = run(&mut doc, Event::SetMarked { text: "にほ".into(), selection: 2..2 });
    assert_eq!(effects, [Effect::Preedit("にほ".into(), Some((6, 6)))]);
    assert_eq!((doc.string().as_str(), doc.marked(), doc.selection()), ("aにほb", Some(1..3), 3..3));
    run(&mut doc, Event::SetMarked { text: "にほん".into(), selection: 3..3 });
    assert_eq!((doc.string().as_str(), doc.marked()), ("aにほんb", Some(1..4)));
    // The runtime composes: its text is as the keyboard left it.
    assert_eq!(run(&mut doc, Event::Runtime(Snapshot::Composing)), []);
    assert_eq!(run(&mut doc, Event::Insert("日本".into())), [Effect::Commit("日本".into())]);
    assert_eq!((doc.string().as_str(), doc.marked(), doc.selection()), ("a日本b", None, 3..3));
}

#[test]
fn unmarking_commits_the_marked_text_where_it_stands() {
    let mut doc = field("ab", 1..1);
    run(&mut doc, Event::SetMarked { text: "xy".into(), selection: 2..2 });
    assert_eq!(run(&mut doc, Event::Unmark), [Effect::Commit("xy".into())]);
    assert_eq!((doc.string().as_str(), doc.marked(), doc.selection()), ("axyb", None, 3..3));
    assert_eq!(run(&mut doc, Event::Unmark), [], "with nothing marked there is nothing to commit");
}

#[test]
fn marking_nothing_ends_the_marking_and_leaves_nothing() {
    let mut doc = field("ab", 1..1);
    run(&mut doc, Event::SetMarked { text: "xy".into(), selection: 2..2 });
    assert_eq!(run(&mut doc, Event::SetMarked { text: String::new(), selection: 0..0 }), [Effect::Preedit(String::new(), None)]);
    assert_eq!((doc.string().as_str(), doc.marked(), doc.selection()), ("ab", None, 1..1));
}

#[test]
fn delete_inside_marked_text_shortens_the_composition() {
    let mut doc = field("ab", 1..1);
    run(&mut doc, Event::SetMarked { text: "xyz".into(), selection: 3..3 });
    assert_eq!(run(&mut doc, Event::DeleteBackward), [Effect::Preedit("xy".into(), Some((2, 2)))]);
    assert_eq!(doc.string(), "axyb");
}

#[test]
fn a_selection_inside_marked_text_moves_the_compositions_cursor_and_one_outside_ends_it() {
    let mut doc = field("ab", 1..1);
    run(&mut doc, Event::SetMarked { text: "xyz".into(), selection: 3..3 });
    assert_eq!(run(&mut doc, Event::Select(2..2)), [Effect::Preedit("xyz".into(), Some((1, 1)))]);
    assert_eq!(doc.selection(), 2..2);
    assert_eq!(run(&mut doc, Event::Select(5..5)), [Effect::Commit("xyz".into()), Effect::Select(5..5)]);
    assert_eq!((doc.marked(), doc.selection()), (None, 5..5));
}

#[test]
fn a_selection_is_sent_as_the_fields_own_byte_range() {
    let mut doc = Document::default();
    doc.step(Event::Runtime(Snapshot::Field { text: "é😀x".into(), start: 100, selection: 0..0 }));
    assert_eq!(run(&mut doc, Event::Select(1..3)), [Effect::Select(102..106)]);
    assert_eq!(doc.selection(), 1..3);
}

#[test]
fn a_frame_that_agrees_with_the_keyboard_tells_it_nothing() {
    let mut doc = field("helo", 3..3);
    run(&mut doc, Event::Insert("l".into()));
    assert_eq!(run(&mut doc, Event::Runtime(Snapshot::Field { text: "hello".into(), start: 0, selection: 4..4 })), []);
}

#[test]
fn a_frame_that_changed_the_text_under_the_keyboard_tells_it_so() {
    let mut doc = field("hello", 5..5);
    run(&mut doc, Event::Insert("\n".into()));
    // The app sent the message and cleared the field.
    assert_eq!(run(&mut doc, Event::Runtime(Snapshot::Field { text: String::new(), start: 0, selection: 0..0 })), [Effect::TextChanged]);
    assert_eq!((doc.string().as_str(), doc.selection()), ("", 0..0));
}

#[test]
fn a_frame_that_only_moved_the_caret_tells_the_keyboard_the_selection_changed() {
    let mut doc = field("hello", 5..5);
    assert_eq!(run(&mut doc, Event::Runtime(Snapshot::Field { text: "hello".into(), start: 0, selection: 2..2 })), [Effect::SelectionChanged]);
    assert_eq!(doc.selection(), 2..2);
}

#[test]
fn a_field_that_ended_its_composition_itself_takes_the_keyboards_marked_text_away() {
    let mut doc = field("ab", 1..1);
    run(&mut doc, Event::SetMarked { text: "xy".into(), selection: 2..2 });
    // It lost focus: what the composition showed became text.
    assert_eq!(run(&mut doc, Event::Runtime(Snapshot::Field { text: "axyb".into(), start: 0, selection: 3..3 })), [Effect::TextChanged]);
    assert_eq!(doc.marked(), None);
}

#[test]
fn a_hidden_field_takes_keys_and_keeps_nothing() {
    let mut doc = field("hello", 5..5);
    assert_eq!(run(&mut doc, Event::Runtime(Snapshot::Hidden)), [Effect::TextChanged]);
    assert_eq!(run(&mut doc, Event::Insert("s3cret".into())), [text("s3cret")]);
    assert_eq!(run(&mut doc, Event::DeleteBackward), [Effect::Key(Key::Backspace)]);
    assert_eq!((doc.string().as_str(), doc.len()), ("", 0));
    assert_eq!(run(&mut doc, Event::Runtime(Snapshot::Hidden)), []);
}
