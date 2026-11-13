use super::*;
use crate::popup_menu::{PopupAnchor, PopupItem, PopupMenu};

fn run(seq: &mut Seq, state: &mut State, event: Event) -> Vec<Effect> {
    step(seq, state, event).into_iter().collect()
}

fn menu(id: &str) -> PopupMenu {
    PopupMenu::new().item(PopupItem::new(id, id))
}

/// A menu asked for at a point.
fn at_point(id: &str, x: f32, y: f32) -> Show {
    Show::new(menu(id), &PopupAnchor::Point(x, y), [x, y, 0.0, 0.0])
}

/// A menu asked for over an element.
fn over_element(id: &str, rect: [f32; 4]) -> Show {
    Show::new(menu(id), &PopupAnchor::Key("row".into()), rect)
}

const CARET: Rect = [100.0, 40.0, 2.0, 20.0];

/// A menu that is up, with no caret.
fn open() -> (Seq, State) {
    let (mut seq, mut state) = (Seq::default(), State::Closed);
    run(&mut seq, &mut state, Event::Present { show: at_point("copy", 30.0, 60.0), caret: None });
    (seq, state)
}

/// A field's menu that is up, pointing at `CARET`.
fn open_on_caret() -> (Seq, State) {
    let (mut seq, mut state) = (Seq::default(), State::Closed);
    run(&mut seq, &mut state, Event::Present { show: at_point("paste", 30.0, 60.0), caret: Some(CARET) });
    (seq, state)
}

/// A menu UIKit is taking down, and its grace timer's attempt.
fn leaving() -> (Seq, State, Attempt) {
    let (mut seq, mut state) = open();
    let effects = run(&mut seq, &mut state, Event::Dismissed);
    let [Effect::TimerAfter(attempt, _)] = effects[..] else { panic!("a dismissal starts the grace: {effects:?}") };
    (seq, state, attempt)
}

#[test]
fn a_menu_is_presented_from_where_it_was_asked_for() {
    let (mut seq, mut state) = (Seq::default(), State::Closed);
    let show = at_point("copy", 30.0, 60.0);
    let effects = run(&mut seq, &mut state, Event::Present { show: show.clone(), caret: None });
    assert_eq!(effects, [Effect::Present { at: (30.0, 60.0) }]);
    assert_eq!(state, State::Open { show: show.clone(), caret: None });
    assert_eq!(state.show(), Some(&show));
    assert_eq!(state.target(), [30.0, 60.0, 0.0, 0.0]);
}

#[test]
fn a_menu_over_an_element_points_at_the_element() {
    let (mut seq, mut state) = (Seq::default(), State::Closed);
    let show = over_element("copy", [10.0, 20.0, 80.0, 30.0]);
    let effects = run(&mut seq, &mut state, Event::Present { show: show.clone(), caret: None });
    assert_eq!(effects, [Effect::Present { at: show.at }]);
    assert_eq!(state.target(), [10.0, 20.0, 80.0, 30.0]);
}

#[test]
fn a_fields_menu_is_presented_from_the_middle_of_its_caret() {
    let (mut seq, mut state) = (Seq::default(), State::Closed);
    let show = over_element("paste", [10.0, 20.0, 80.0, 30.0]);
    let effects = run(&mut seq, &mut state, Event::Present { show, caret: Some(CARET) });
    assert_eq!(effects, [Effect::Present { at: (101.0, 50.0) }]);
    assert!(state.follows_caret());
    assert_eq!(state.target(), CARET);
}

#[test]
fn a_menu_presented_over_an_open_one_replaces_it_without_an_answer() {
    let (mut seq, mut state) = open_on_caret();
    let show = at_point("delete", 5.0, 6.0);
    let effects = run(&mut seq, &mut state, Event::Present { show: show.clone(), caret: None });
    assert_eq!(effects, [Effect::Present { at: (5.0, 6.0) }]);
    assert_eq!(state, State::Open { show, caret: None });
}

#[test]
fn a_menu_presented_while_one_leaves_opens_and_the_old_grace_is_stale() {
    let (mut seq, mut state, attempt) = leaving();
    let show = at_point("delete", 5.0, 6.0);
    let effects = run(&mut seq, &mut state, Event::Present { show: show.clone(), caret: None });
    assert_eq!(effects, [Effect::Present { at: (5.0, 6.0) }]);
    assert_eq!(run(&mut seq, &mut state, Event::Timer(attempt)), []);
    assert_eq!(state, State::Open { show, caret: None });
}

#[test]
fn the_second_menus_grace_is_a_new_attempt() {
    let (mut seq, mut state, first) = leaving();
    run(&mut seq, &mut state, Event::Present { show: at_point("delete", 5.0, 6.0), caret: None });
    let effects = run(&mut seq, &mut state, Event::Dismissed);
    let [Effect::TimerAfter(second, _)] = effects[..] else { panic!("a dismissal starts the grace: {effects:?}") };
    assert_ne!(first, second);
    assert_eq!(run(&mut seq, &mut state, Event::Timer(first)), []);
    assert_eq!(state, State::Leaving(second));
    assert_eq!(run(&mut seq, &mut state, Event::Timer(second)), [Effect::Answer(None)]);
    assert_eq!(state, State::Closed);
}

#[test]
fn a_dismissed_menu_waits_a_grace_for_its_pick() {
    let (mut seq, mut state) = open();
    let effects = run(&mut seq, &mut state, Event::Dismissed);
    assert_eq!(effects, [Effect::TimerAfter(seq.current(), PICK_GRACE)]);
    assert_eq!(state, State::Leaving(seq.current()));
    assert_eq!(state.show(), None);
    assert_eq!(state.target(), [0.0; 4]);
}

#[test]
fn a_dismissed_fields_menu_leaves_its_caret_behind() {
    let (mut seq, mut state) = open_on_caret();
    let effects = run(&mut seq, &mut state, Event::Dismissed);
    assert_eq!(effects, [Effect::TimerAfter(seq.current(), PICK_GRACE)]);
    assert_eq!(state, State::Leaving(seq.current()));
    assert!(!state.follows_caret());
    assert_eq!(run(&mut seq, &mut state, Event::Caret([100.0, 12.0, 2.0, 20.0])), []);
    assert_eq!(state.target(), [0.0; 4]);
}

#[test]
fn a_dismissal_with_no_menu_up_changes_nothing() {
    let (mut seq, mut state) = (Seq::default(), State::Closed);
    assert_eq!(run(&mut seq, &mut state, Event::Dismissed), []);
    assert_eq!(state, State::Closed);

    let (mut seq, mut state, attempt) = leaving();
    assert_eq!(run(&mut seq, &mut state, Event::Dismissed), []);
    assert_eq!(state, State::Leaving(attempt));
}

#[test]
fn a_pick_from_an_open_menu_answers_with_its_id() {
    let (mut seq, mut state) = open();
    assert_eq!(run(&mut seq, &mut state, Event::Picked(Some("copy".into()))), [Effect::Answer(Some("copy".into()))]);
    assert_eq!(state, State::Closed);
}

#[test]
fn a_pick_during_the_grace_answers_and_the_timer_then_changes_nothing() {
    let (mut seq, mut state, attempt) = leaving();
    assert_eq!(run(&mut seq, &mut state, Event::Picked(Some("copy".into()))), [Effect::Answer(Some("copy".into()))]);
    assert_eq!(state, State::Closed);
    assert_eq!(run(&mut seq, &mut state, Event::Timer(attempt)), []);
    assert_eq!(state, State::Closed);
}

#[test]
fn a_pick_with_no_id_answers_with_nothing() {
    let (mut seq, mut state) = open();
    assert_eq!(run(&mut seq, &mut state, Event::Picked(None)), [Effect::Answer(None)]);
    assert_eq!(state, State::Closed);
}

#[test]
fn a_pick_with_no_menu_up_changes_nothing() {
    let (mut seq, mut state) = (Seq::default(), State::Closed);
    assert_eq!(run(&mut seq, &mut state, Event::Picked(Some("copy".into()))), []);
    assert_eq!(state, State::Closed);
}

#[test]
fn a_grace_that_runs_out_answers_that_nothing_was_picked() {
    let (mut seq, mut state, attempt) = leaving();
    assert_eq!(run(&mut seq, &mut state, Event::Timer(attempt)), [Effect::Answer(None)]);
    assert_eq!(state, State::Closed);
}

#[test]
fn a_stale_timer_changes_nothing() {
    let (mut seq, mut state, attempt) = leaving();
    assert_eq!(run(&mut seq, &mut state, Event::Timer(Attempt(attempt.0 + 1))), []);
    assert_eq!(state, State::Leaving(attempt));

    let (mut seq, mut state) = open();
    let before = state.clone();
    let current = seq.current();
    assert_eq!(run(&mut seq, &mut state, Event::Timer(current)), []);
    assert_eq!(state, before);

    let (mut seq, mut state) = (Seq::default(), State::Closed);
    assert_eq!(run(&mut seq, &mut state, Event::Timer(Attempt(1))), []);
    assert_eq!(state, State::Closed);
}

#[test]
fn a_fields_menu_moves_with_its_caret() {
    let (mut seq, mut state) = open_on_caret();
    let moved = [100.0, 12.0, 2.0, 20.0];
    assert_eq!(run(&mut seq, &mut state, Event::Caret(moved)), [Effect::Move]);
    assert_eq!(state.target(), moved);
    assert_eq!(run(&mut seq, &mut state, Event::Caret(moved)), []);
}

#[test]
fn a_menu_with_no_caret_ignores_the_caret() {
    let (mut seq, mut state) = open();
    let before = state.clone();
    assert!(!state.follows_caret());
    assert_eq!(run(&mut seq, &mut state, Event::Caret(CARET)), []);
    assert_eq!(state, before);

    let (mut seq, mut state, attempt) = leaving();
    assert!(!state.follows_caret());
    assert_eq!(run(&mut seq, &mut state, Event::Caret(CARET)), []);
    assert_eq!(state, State::Leaving(attempt));
}

#[test]
fn a_caret_with_no_menu_up_changes_nothing() {
    let (mut seq, mut state) = (Seq::default(), State::Closed);
    assert!(!state.follows_caret());
    assert_eq!(run(&mut seq, &mut state, Event::Caret(CARET)), []);
    assert_eq!(state, State::Closed);
}
