use super::*;

fn run(seq: &mut Seq, state: &mut State, event: Event) -> Vec<Effect> {
    step(seq, state, event).into_iter().collect()
}

fn running() -> (Seq, State) {
    let (mut seq, mut state) = (Seq::default(), State::Detached);
    run(&mut seq, &mut state, Event::Attached { foreground: true });
    (seq, state)
}

/// A tick that draws a frame with nothing moving, and the frame's effects.
fn still_frame(seq: &mut Seq, state: &mut State, wake_after: Option<f64>) -> Vec<Effect> {
    assert_eq!(run(seq, state, Event::Tick), [Effect::Draw]);
    run(seq, state, Event::Frame { animating: false, wake_after })
}

#[test]
fn a_view_that_joins_a_window_in_front_starts_ticking() {
    let (mut seq, mut state) = (Seq::default(), State::Detached);
    let effects = run(&mut seq, &mut state, Event::Attached { foreground: true });
    assert_eq!(effects, [Effect::Create, Effect::Resume]);
    assert_eq!(state, State::STARTED);
}

#[test]
fn a_view_that_joins_a_window_in_the_background_waits_for_the_app_to_come_forward() {
    let (mut seq, mut state) = (Seq::default(), State::Detached);
    assert_eq!(run(&mut seq, &mut state, Event::Attached { foreground: false }), [Effect::Create]);
    assert_eq!(state, State::Background);
    assert_eq!(run(&mut seq, &mut state, Event::Wake), []);
    assert_eq!(run(&mut seq, &mut state, Event::Foreground), [Effect::Resume]);
    assert_eq!(state, State::STARTED);
}

#[test]
fn a_frame_with_nothing_moving_rests_the_link() {
    let (mut seq, mut state) = running();
    assert_eq!(run(&mut seq, &mut state, Event::Frame { animating: true, wake_after: None }), []);
    assert_eq!(state, State::STARTED);
    assert_eq!(run(&mut seq, &mut state, Event::Frame { animating: false, wake_after: None }), [Effect::Pause]);
    assert_eq!(state, State::Resting { timer: None });
}

#[test]
fn a_wake_starts_a_resting_link_and_changes_nothing_while_it_runs() {
    let (mut seq, mut state) = running();
    assert_eq!(run(&mut seq, &mut state, Event::Wake), []);
    run(&mut seq, &mut state, Event::Frame { animating: false, wake_after: None });
    assert_eq!(run(&mut seq, &mut state, Event::Wake), [Effect::Resume]);
    assert_eq!(state, State::STARTED);
}

#[test]
fn a_resting_link_starts_again_when_the_runtimes_timer_comes_due() {
    let (mut seq, mut state) = running();
    let effects = run(&mut seq, &mut state, Event::Frame { animating: false, wake_after: Some(0.5) });
    let attempt = seq.current();
    assert_eq!(effects, [Effect::Pause, Effect::TimerAfter(attempt, 0.5)]);
    assert_eq!(run(&mut seq, &mut state, Event::Timer(attempt)), [Effect::Resume]);
    assert_eq!(state, State::STARTED);
}

#[test]
fn a_timer_from_an_earlier_rest_changes_nothing() {
    let (mut seq, mut state) = running();
    run(&mut seq, &mut state, Event::Frame { animating: false, wake_after: Some(0.5) });
    let stale = seq.current();
    run(&mut seq, &mut state, Event::Wake);
    run(&mut seq, &mut state, Event::Frame { animating: false, wake_after: Some(2.0) });
    assert_eq!(run(&mut seq, &mut state, Event::Timer(stale)), []);
    assert_eq!(state, State::Resting { timer: Some(seq.current()) });
    run(&mut seq, &mut state, Event::Wake);
    run(&mut seq, &mut state, Event::Frame { animating: false, wake_after: None });
    assert_eq!(run(&mut seq, &mut state, Event::Timer(stale)), []);
    assert_eq!(state, State::Resting { timer: None });
}

#[test]
fn a_timer_already_due_is_asked_for_at_zero_not_in_the_past() {
    let (mut seq, mut state) = running();
    let effects = run(&mut seq, &mut state, Event::Frame { animating: false, wake_after: Some(-0.25) });
    assert_eq!(effects, [Effect::Pause, Effect::TimerAfter(seq.current(), 0.0)]);
}

#[test]
fn the_background_pauses_the_link_and_holds_it_until_the_app_comes_forward() {
    for rest in [false, true] {
        let (mut seq, mut state) = running();
        if rest { run(&mut seq, &mut state, Event::Frame { animating: false, wake_after: Some(1.0) }); }
        let timer = seq.current();
        assert_eq!(run(&mut seq, &mut state, Event::Background), [Effect::Pause]);
        assert_eq!(state, State::Background);
        for event in [Event::Wake, Event::Timer(timer), Event::Frame { animating: true, wake_after: None }, Event::Background] {
            assert_eq!(run(&mut seq, &mut state, event), []);
            assert_eq!(state, State::Background);
        }
        assert_eq!(run(&mut seq, &mut state, Event::Foreground), [Effect::Resume]);
        assert_eq!(state, State::STARTED);
    }
}

#[test]
fn leaving_the_window_destroys_the_link_from_any_state() {
    for prepare in [None, Some(Event::Background), Some(Event::Frame { animating: false, wake_after: Some(1.0) })] {
        let (mut seq, mut state) = running();
        if let Some(event) = prepare { run(&mut seq, &mut state, event); }
        assert_eq!(run(&mut seq, &mut state, Event::Detached), [Effect::Destroy]);
        assert_eq!(state, State::Detached);
        for event in [Event::Wake, Event::Foreground, Event::Background, Event::Timer(seq.current()), Event::Detached] {
            assert_eq!(run(&mut seq, &mut state, event), []);
            assert_eq!(state, State::Detached);
        }
    }
}

#[test]
fn joining_a_window_twice_makes_one_link() {
    let (mut seq, mut state) = running();
    assert_eq!(run(&mut seq, &mut state, Event::Attached { foreground: true }), []);
    assert_eq!(state, State::STARTED);
}

#[test]
fn coming_forward_while_in_front_changes_nothing() {
    let (mut seq, mut state) = running();
    assert_eq!(run(&mut seq, &mut state, Event::Foreground), []);
    run(&mut seq, &mut state, Event::Frame { animating: false, wake_after: None });
    assert_eq!(run(&mut seq, &mut state, Event::Foreground), []);
    assert_eq!(state, State::Resting { timer: None });
}

#[test]
fn a_tick_draws_only_when_something_may_have_changed() {
    let (mut seq, mut state) = running();
    assert_eq!(run(&mut seq, &mut state, Event::Tick), [Effect::Draw]);
    run(&mut seq, &mut state, Event::Frame { animating: true, wake_after: None });
    assert_eq!(run(&mut seq, &mut state, Event::Tick), [Effect::Draw]);
    run(&mut seq, &mut state, Event::Hold);
    still_frame(&mut seq, &mut state, None);
    assert_eq!(run(&mut seq, &mut state, Event::Tick), []);
    run(&mut seq, &mut state, Event::Wake);
    assert_eq!(run(&mut seq, &mut state, Event::Tick), [Effect::Draw]);
}

#[test]
fn a_held_finger_keeps_the_link_ticking_through_frames_with_nothing_moving() {
    let (mut seq, mut state) = running();
    run(&mut seq, &mut state, Event::Hold);
    for _ in 0..3 {
        run(&mut seq, &mut state, Event::Wake);
        assert_eq!(still_frame(&mut seq, &mut state, None), []);
        assert_eq!(state, State::Running { held: 1, stale: false, timer: None });
    }
}

#[test]
fn a_finger_that_lands_on_a_resting_link_starts_it() {
    let (mut seq, mut state) = running();
    still_frame(&mut seq, &mut state, None);
    assert_eq!(run(&mut seq, &mut state, Event::Hold), [Effect::Resume]);
    assert_eq!(state, State::Running { held: 1, stale: true, timer: None });
}

#[test]
fn the_link_rests_on_the_first_still_frame_after_the_last_finger_lifts() {
    let (mut seq, mut state) = running();
    run(&mut seq, &mut state, Event::Hold);
    run(&mut seq, &mut state, Event::Hold);
    still_frame(&mut seq, &mut state, None);
    run(&mut seq, &mut state, Event::Release);
    assert_eq!(run(&mut seq, &mut state, Event::Tick), [], "one finger is still down, and nothing changed");
    run(&mut seq, &mut state, Event::Release);
    run(&mut seq, &mut state, Event::Wake);
    assert_eq!(still_frame(&mut seq, &mut state, None), [Effect::Pause]);
    assert_eq!(state, State::Resting { timer: None });
}

#[test]
fn a_release_with_nothing_held_holds_nothing() {
    let (mut seq, mut state) = running();
    run(&mut seq, &mut state, Event::Release);
    assert_eq!(state, State::STARTED);
    still_frame(&mut seq, &mut state, None);
    assert_eq!(run(&mut seq, &mut state, Event::Release), []);
    assert_eq!(state, State::Resting { timer: None });
}

#[test]
fn the_runtimes_timer_draws_a_frame_while_a_finger_is_held_still() {
    let (mut seq, mut state) = running();
    run(&mut seq, &mut state, Event::Hold);
    let effects = still_frame(&mut seq, &mut state, Some(0.5));
    let attempt = seq.current();
    assert_eq!(effects, [Effect::TimerAfter(attempt, 0.5)]);
    assert_eq!(run(&mut seq, &mut state, Event::Tick), []);
    assert_eq!(run(&mut seq, &mut state, Event::Timer(attempt)), []);
    assert_eq!(run(&mut seq, &mut state, Event::Tick), [Effect::Draw]);
}

#[test]
fn a_held_fingers_stale_timer_draws_nothing() {
    let (mut seq, mut state) = running();
    run(&mut seq, &mut state, Event::Hold);
    still_frame(&mut seq, &mut state, Some(0.5));
    let stale = seq.current();
    run(&mut seq, &mut state, Event::Wake);
    still_frame(&mut seq, &mut state, Some(2.0));
    assert_eq!(run(&mut seq, &mut state, Event::Timer(stale)), []);
    assert_eq!(run(&mut seq, &mut state, Event::Tick), []);
}

#[test]
fn the_background_forgets_held_fingers() {
    let (mut seq, mut state) = running();
    run(&mut seq, &mut state, Event::Hold);
    run(&mut seq, &mut state, Event::Background);
    assert_eq!(run(&mut seq, &mut state, Event::Release), []);
    run(&mut seq, &mut state, Event::Foreground);
    assert_eq!(state, State::STARTED);
}
