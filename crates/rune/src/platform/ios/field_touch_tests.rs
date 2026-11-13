use super::*;

fn run(state: &mut State, event: Event) -> Vec<Effect> {
    step(state, event).into_iter().collect()
}

/// A field holding "hello world" with this byte range selected.
fn field(selection: Range<usize>) -> ImeText {
    ImeText { text: "hello world".into(), start: 0, selection }
}

/// The field with its caret at this byte.
fn caret(at: usize) -> Option<ImeText> {
    Some(field(at..at))
}

const AT: (f32, f32) = (40.0, 20.0);

/// A finger down on the field, whose caret is at byte 5.
fn down() -> State {
    let mut state = State::default();
    assert_eq!(run(&mut state, Event::Landed { at: AT, text: caret(5), on_field: true }), []);
    assert_eq!(state.finger, Finger::Down { at: AT, before: field(5..5) });
    state
}

/// A finger that tapped the field, whose caret was at byte 5.
fn tapped() -> State {
    let mut state = down();
    assert_eq!(run(&mut state, Event::Touch { phase: TouchPhase::Ended, at: AT }), []);
    assert_eq!(state.finger, Finger::Lifted { at: AT, before: field(5..5) });
    state
}

/// A finger that landed on the field while bytes 0..5 were selected.
fn on_selection() -> State {
    let mut state = State::default();
    assert_eq!(run(&mut state, Event::Landed { at: AT, text: Some(field(0..5)), on_field: true }), []);
    assert_eq!(state.finger, Finger::OnSelection { before: field(0..5) });
    state
}

/// The field's menu showing, asked for at `AT`.
fn showing() -> State {
    let mut state = State::default();
    assert_eq!(run(&mut state, Event::Shown { at: AT, field: true }), []);
    assert_eq!(state.menu, Menu::Showing { at: AT });
    state
}

#[test]
fn a_tap_that_leaves_the_caret_where_it_was_asks_for_the_menu() {
    let mut state = tapped();
    assert!(state.awaits_frame());
    assert_eq!(run(&mut state, Event::Frame { text: caret(5), popup: false }), [Effect::Press(AT), Effect::Wake]);
    assert_eq!(state, State::default());
    assert!(!state.awaits_frame());
}

#[test]
fn a_tap_that_moves_the_caret_asks_for_nothing() {
    let mut state = tapped();
    assert_eq!(run(&mut state, Event::Frame { text: caret(8), popup: false }), []);
    assert_eq!(state, State::default());
}

#[test]
fn a_tap_that_takes_the_focus_away_asks_for_nothing() {
    let mut state = tapped();
    assert_eq!(run(&mut state, Event::Frame { text: None, popup: false }), []);
    assert_eq!(state, State::default());
}

#[test]
fn a_tap_asks_for_nothing_while_a_menu_shows() {
    let mut state = tapped();
    assert_eq!(run(&mut state, Event::Frame { text: caret(5), popup: true }), []);
    assert_eq!(state, State::default());
    // The tap is spent: the frame after the menu leaves asks for nothing either.
    assert_eq!(run(&mut state, Event::Frame { text: caret(5), popup: false }), []);
}

#[test]
fn a_tap_under_a_field_menu_leaves_the_menu_showing() {
    let mut state = showing();
    run(&mut state, Event::Landed { at: AT, text: caret(5), on_field: true });
    run(&mut state, Event::Touch { phase: TouchPhase::Ended, at: AT });
    assert_eq!(run(&mut state, Event::Frame { text: caret(5), popup: true }), []);
    assert_eq!(state, State { finger: Finger::Away, menu: Menu::Showing { at: AT } });
}

#[test]
fn a_finger_that_lands_off_the_field_or_with_no_field_is_not_watched() {
    let mut state = State::default();
    run(&mut state, Event::Landed { at: AT, text: caret(5), on_field: false });
    assert_eq!(state.finger, Finger::Away);
    run(&mut state, Event::Landed { at: AT, text: None, on_field: true });
    assert_eq!(state.finger, Finger::Away);
    run(&mut state, Event::Touch { phase: TouchPhase::Ended, at: AT });
    assert!(!state.awaits_frame());
    assert_eq!(run(&mut state, Event::Frame { text: caret(5), popup: false }), []);
}

#[test]
fn a_finger_that_moves_within_the_slop_still_taps() {
    let mut state = down();
    run(&mut state, Event::Touch { phase: TouchPhase::Moved, at: (AT.0 + 6.0, AT.1 + 8.0) });
    assert_eq!(state.finger, Finger::Down { at: AT, before: field(5..5) });
    run(&mut state, Event::Touch { phase: TouchPhase::Ended, at: (AT.0 + 6.0, AT.1 + 8.0) });
    // The menu is asked for where the finger landed.
    assert_eq!(run(&mut state, Event::Frame { text: caret(5), popup: false }), [Effect::Press(AT), Effect::Wake]);
}

#[test]
fn a_drag_beyond_the_slop_is_not_a_tap() {
    let mut state = down();
    run(&mut state, Event::Touch { phase: TouchPhase::Moved, at: (AT.0 + TAP_SLOP + 0.5, AT.1) });
    assert_eq!(state.finger, Finger::Away);
    // Coming back does not make it one again.
    run(&mut state, Event::Touch { phase: TouchPhase::Moved, at: AT });
    run(&mut state, Event::Touch { phase: TouchPhase::Ended, at: AT });
    assert_eq!(state.finger, Finger::Away);
    assert_eq!(run(&mut state, Event::Frame { text: caret(5), popup: false }), []);
}

#[test]
fn a_cancelled_touch_is_not_a_tap() {
    let mut state = down();
    run(&mut state, Event::Touch { phase: TouchPhase::Cancelled, at: AT });
    assert_eq!(state.finger, Finger::Away);
}

#[test]
fn a_second_finger_landing_leaves_the_tap_and_its_drag_ends_it() {
    let mut state = down();
    let other = (AT.0 + 200.0, AT.1);
    run(&mut state, Event::Touch { phase: TouchPhase::Started, at: other });
    assert_eq!(state.finger, Finger::Down { at: AT, before: field(5..5) });
    // Fingers are not told apart: one that moves far from where the first landed is a drag.
    run(&mut state, Event::Touch { phase: TouchPhase::Moved, at: other });
    assert_eq!(state.finger, Finger::Away);
}

#[test]
fn a_second_finger_lifting_beside_the_first_counts_as_the_lift() {
    let mut state = down();
    run(&mut state, Event::Touch { phase: TouchPhase::Started, at: (AT.0 + 4.0, AT.1) });
    run(&mut state, Event::Touch { phase: TouchPhase::Ended, at: (AT.0 + 4.0, AT.1) });
    assert_eq!(state.finger, Finger::Lifted { at: AT, before: field(5..5) });
}

#[test]
fn a_touch_after_the_lift_and_before_the_frame_spends_the_tap() {
    for phase in [TouchPhase::Started, TouchPhase::Moved, TouchPhase::Ended, TouchPhase::Cancelled] {
        let mut state = tapped();
        run(&mut state, Event::Touch { phase, at: AT });
        assert_eq!(state.finger, Finger::Away, "{phase:?}");
    }
}

#[test]
fn a_new_finger_starts_over() {
    let mut state = tapped();
    run(&mut state, Event::Landed { at: (5.0, 5.0), text: caret(2), on_field: true });
    assert_eq!(state.finger, Finger::Down { at: (5.0, 5.0), before: field(2..2) });
    run(&mut state, Event::Landed { at: AT, text: Some(field(0..5)), on_field: false });
    assert_eq!(state.finger, Finger::OnSelection { before: field(0..5) });
    run(&mut state, Event::Landed { at: AT, text: None, on_field: false });
    assert_eq!(state.finger, Finger::Away);
}

#[test]
fn a_long_press_on_a_selection_puts_the_selection_back() {
    let mut state = on_selection();
    // The landing placed the caret at byte 3.
    assert_eq!(run(&mut state, Event::Pressing { text: caret(3) }), [Effect::Select(0..5)]);
    assert_eq!(state.finger, Finger::Away);
    // Once: the next press finds nothing to put back.
    assert_eq!(run(&mut state, Event::Pressing { text: caret(3) }), []);
}

#[test]
fn the_selection_put_back_is_in_the_fields_bytes() {
    let mut state = State::default();
    let before = ImeText { text: "second line".into(), start: 100, selection: 7..11 };
    run(&mut state, Event::Landed { at: AT, text: Some(before.clone()), on_field: true });
    let now = ImeText { selection: 2..2, ..before };
    assert_eq!(run(&mut state, Event::Pressing { text: Some(now) }), [Effect::Select(107..111)]);
}

#[test]
fn a_selection_is_not_put_back_over_changed_text_or_a_new_selection() {
    let changed = ImeText { text: "hello".into(), start: 0, selection: 3..3 };
    let moved = ImeText { start: 4, ..field(3..3) };
    for now in [Some(changed), Some(moved), Some(field(2..4)), None] {
        let mut state = on_selection();
        assert_eq!(run(&mut state, Event::Pressing { text: now.clone() }), [], "{now:?}");
        // The press spends it all the same.
        assert_eq!(state.finger, Finger::Away);
    }
}

#[test]
fn the_selection_a_finger_landed_on_outlives_the_finger() {
    for phase in [TouchPhase::Started, TouchPhase::Moved, TouchPhase::Ended, TouchPhase::Cancelled] {
        let mut state = on_selection();
        run(&mut state, Event::Touch { phase, at: (AT.0 + 50.0, AT.1) });
        assert_eq!(state.finger, Finger::OnSelection { before: field(0..5) }, "{phase:?}");
    }
    let mut state = on_selection();
    assert!(!state.awaits_frame());
    assert_eq!(run(&mut state, Event::Frame { text: caret(3), popup: false }), []);
    assert_eq!(run(&mut state, Event::Pressing { text: caret(3) }), [Effect::Select(0..5)]);
}

#[test]
fn a_press_leaves_a_tap_as_it_was() {
    let mut state = down();
    assert_eq!(run(&mut state, Event::Pressing { text: caret(5) }), []);
    assert_eq!(state.finger, Finger::Down { at: AT, before: field(5..5) });
    let mut state = tapped();
    assert_eq!(run(&mut state, Event::Pressing { text: caret(5) }), []);
    assert_eq!(state.finger, Finger::Lifted { at: AT, before: field(5..5) });
}

#[test]
fn select_all_opens_the_field_menu_again_at_its_place() {
    let mut state = showing();
    assert_eq!(run(&mut state, Event::Answered { select_all: true }), []);
    assert_eq!(state.menu, Menu::Reopening { at: AT });
    assert!(state.awaits_frame());
    // The press comes once the frame that applies Select All is done.
    assert_eq!(run(&mut state, Event::Frame { text: Some(field(0..11)), popup: false }), [Effect::Press(AT), Effect::Wake]);
    assert_eq!(state.menu, Menu::Hidden);
    assert_eq!(run(&mut state, Event::Frame { text: Some(field(0..11)), popup: false }), []);
}

#[test]
fn another_item_or_a_dismissal_does_not_open_the_menu_again() {
    let mut state = showing();
    assert_eq!(run(&mut state, Event::Answered { select_all: false }), []);
    assert_eq!(state.menu, Menu::Hidden);
    assert!(!state.awaits_frame());
    assert_eq!(run(&mut state, Event::Frame { text: caret(5), popup: false }), []);
}

#[test]
fn an_apps_menu_records_no_place_to_open_again() {
    let mut state = State::default();
    assert_eq!(run(&mut state, Event::Shown { at: AT, field: false }), []);
    assert_eq!(state.menu, Menu::Hidden);
    // Its own item named Select All opens nothing.
    assert_eq!(run(&mut state, Event::Answered { select_all: true }), []);
    assert_eq!(state.menu, Menu::Hidden);
    assert!(!state.awaits_frame());
}

#[test]
fn an_apps_menu_takes_the_field_menus_place() {
    let mut state = showing();
    run(&mut state, Event::Shown { at: (1.0, 2.0), field: false });
    assert_eq!(state.menu, Menu::Hidden);
    run(&mut state, Event::Shown { at: (1.0, 2.0), field: true });
    assert_eq!(state.menu, Menu::Showing { at: (1.0, 2.0) });
}

#[test]
fn any_menu_shown_or_answered_ends_a_press_still_owed() {
    let owed = || {
        let mut state = showing();
        run(&mut state, Event::Answered { select_all: true });
        assert_eq!(state.menu, Menu::Reopening { at: AT });
        state
    };
    for select_all in [false, true] {
        let mut state = owed();
        assert_eq!(run(&mut state, Event::Answered { select_all }), []);
        assert_eq!(state.menu, Menu::Hidden);
    }
    let mut state = owed();
    run(&mut state, Event::Shown { at: (1.0, 2.0), field: false });
    assert_eq!(state.menu, Menu::Hidden);
    let mut state = owed();
    run(&mut state, Event::Shown { at: (1.0, 2.0), field: true });
    assert_eq!(state.menu, Menu::Showing { at: (1.0, 2.0) });
}

#[test]
fn a_press_still_owed_is_made_under_a_finger_that_then_taps() {
    let mut state = showing();
    run(&mut state, Event::Answered { select_all: true });
    let tap = (90.0, 20.0);
    run(&mut state, Event::Landed { at: tap, text: caret(5), on_field: true });
    // Landing leaves the menu's press owed.
    assert_eq!(state.menu, Menu::Reopening { at: AT });
    assert_eq!(run(&mut state, Event::Frame { text: caret(5), popup: false }), [Effect::Press(AT), Effect::Wake]);
    assert_eq!(state, State { finger: Finger::Down { at: tap, before: field(5..5) }, menu: Menu::Hidden });
    run(&mut state, Event::Touch { phase: TouchPhase::Ended, at: tap });
    assert_eq!(run(&mut state, Event::Frame { text: caret(5), popup: false }), [Effect::Press(tap), Effect::Wake]);
    assert_eq!(state, State::default());
}

#[test]
fn a_press_still_owed_is_made_when_a_menu_keeps_the_tap_from_asking() {
    let mut state = showing();
    run(&mut state, Event::Answered { select_all: true });
    run(&mut state, Event::Landed { at: (90.0, 20.0), text: caret(5), on_field: true });
    run(&mut state, Event::Touch { phase: TouchPhase::Ended, at: (90.0, 20.0) });
    assert_eq!(run(&mut state, Event::Frame { text: caret(5), popup: true }), [Effect::Press(AT), Effect::Wake]);
    assert_eq!(state, State::default());
}

#[test]
fn a_press_and_a_landing_leave_the_menu_as_it_was() {
    for menu in [Menu::Hidden, Menu::Showing { at: AT }, Menu::Reopening { at: AT }] {
        let mut state = State { finger: Finger::OnSelection { before: field(0..5) }, menu };
        run(&mut state, Event::Pressing { text: caret(3) });
        assert_eq!(state.menu, menu);
        run(&mut state, Event::Landed { at: AT, text: Some(field(0..5)), on_field: true });
        run(&mut state, Event::Landed { at: AT, text: caret(5), on_field: true });
        run(&mut state, Event::Landed { at: AT, text: None, on_field: false });
        assert_eq!(state.menu, menu);
    }
}

#[test]
fn a_tap_wins_over_a_press_still_owed() {
    let mut state = showing();
    run(&mut state, Event::Answered { select_all: true });
    let tap = (90.0, 20.0);
    run(&mut state, Event::Landed { at: tap, text: caret(5), on_field: true });
    run(&mut state, Event::Touch { phase: TouchPhase::Ended, at: tap });
    // One press, at the tap; the owed one is spent with it.
    assert_eq!(run(&mut state, Event::Frame { text: caret(5), popup: false }), [Effect::Press(tap), Effect::Wake]);
    assert_eq!(state, State::default());
}

#[test]
fn a_tap_that_moved_the_caret_leaves_a_press_still_owed_to_make() {
    let mut state = showing();
    run(&mut state, Event::Answered { select_all: true });
    run(&mut state, Event::Landed { at: (90.0, 20.0), text: caret(5), on_field: true });
    run(&mut state, Event::Touch { phase: TouchPhase::Ended, at: (90.0, 20.0) });
    assert_eq!(run(&mut state, Event::Frame { text: caret(9), popup: false }), [Effect::Press(AT), Effect::Wake]);
    assert_eq!(state, State::default());
}

#[test]
fn a_finger_lands_on_the_selection_while_the_menu_shows() {
    let mut state = showing();
    run(&mut state, Event::Landed { at: AT, text: Some(field(0..11)), on_field: true });
    // The landing dismisses the menu; the long press that follows keeps the selection.
    run(&mut state, Event::Answered { select_all: false });
    assert_eq!(state, State { finger: Finger::OnSelection { before: field(0..11) }, menu: Menu::Hidden });
    assert_eq!(run(&mut state, Event::Pressing { text: caret(4) }), [Effect::Select(0..11)]);
}
