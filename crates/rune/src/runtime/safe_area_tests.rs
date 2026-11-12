//! `Cx::safe_area`: what the host's screen keeps at each edge reaches the view in its own
//! points, and a change re-runs the view and the memos that read it.
use super::*;
use crate::{Cx, El, Env, col, el, memo};

/// Whether the view zooms to 2.
type S = bool;

fn view(zoomed: &S, cx: &mut Cx<S>) -> El {
    if *zoomed { cx.zoom(2.0); }
    let [top, right, bottom, left] = cx.safe_area();
    let inside = memo("inside", (), || el().key("inside").w_full().h_full());
    col().w_full().h_full().pt(top).pr(right).pb(bottom).pl(left).child(inside)
}

fn runtime(zoomed: bool) -> Runtime<S> {
    let mut rt = Runtime::new(zoomed, view, (400.0, 800.0), Env::new(true, 3.0));
    rt.frame(0.0);
    rt
}

/// Where "inside" comes to rest: a moved layout springs to its new place.
fn inside(rt: &mut Runtime<S>) -> [f32; 4] {
    for _ in 0..600 { rt.frame(1.0 / 120.0); }
    rt.presented("inside").unwrap().0
}

#[test]
fn a_headless_view_reads_no_safe_area() {
    assert_eq!(inside(&mut runtime(false)), [0.0, 0.0, 400.0, 800.0]);
}

#[test]
fn the_hosts_insets_reach_the_view_and_a_change_re_runs_it_and_its_memos() {
    let mut rt = runtime(false);
    assert!(rt.set_safe_area([59.0, 0.0, 34.0, 0.0]));
    assert_eq!(inside(&mut rt), [0.0, 59.0, 400.0, 800.0 - 59.0 - 34.0]);
    // Landscape: the notch moves to the leading edge.
    assert!(rt.set_safe_area([0.0, 0.0, 21.0, 59.0]));
    assert_eq!(inside(&mut rt), [59.0, 0.0, 400.0 - 59.0, 800.0 - 21.0]);
}

#[test]
fn the_same_insets_again_change_nothing() {
    let mut rt = runtime(false);
    assert!(rt.set_safe_area([59.0, 0.0, 34.0, 0.0]));
    rt.frame(1.0 / 120.0);
    assert!(!rt.set_safe_area([59.0, 0.0, 34.0, 0.0]));
    assert!(!rt.dirty);
}

#[test]
fn a_negative_inset_is_none() {
    let mut rt = runtime(false);
    assert!(!rt.set_safe_area([-4.0, 0.0, 0.0, 0.0]));
}

#[test]
fn a_zoomed_view_reads_the_insets_in_its_own_points() {
    let mut rt = runtime(true);
    rt.set_safe_area([60.0, 0.0, 30.0, 0.0]);
    // The view's points are the window's over the zoom: 30 and 15 of a 200 by 400 view.
    assert_eq!(inside(&mut rt), [0.0, 30.0, 200.0, 400.0 - 30.0 - 15.0]);
}
