//! `Cx::system_text_scale`: the system's text size setting reaches the view, and a change
//! re-runs the view and the memos that read it.
use super::*;
use crate::{Cx, El, Env, col, el, memo};

fn view(_: &(), cx: &mut Cx<()>) -> El {
    let scale = cx.system_text_scale();
    let row = memo("row", (), move || el().key("row").w_full().h(20.0 * scale));
    col().w_full().h_full().child(row)
}

fn runtime() -> Runtime<()> {
    let mut rt = Runtime::new((), view, (400.0, 800.0), Env::new(true, 3.0));
    rt.frame(0.0);
    rt
}

/// The row's height once its layout has come to rest.
fn row(rt: &mut Runtime<()>) -> f32 {
    for _ in 0..600 { rt.frame(1.0 / 120.0); }
    rt.presented("row").unwrap().0[3]
}

#[test]
fn a_headless_view_reads_the_default_size() {
    assert_eq!(row(&mut runtime()), 20.0);
}

#[test]
fn the_hosts_size_reaches_the_view_and_a_change_re_runs_it_and_its_memos() {
    let mut rt = runtime();
    assert!(rt.set_system_text_scale(1.5));
    assert_eq!(row(&mut rt), 30.0);
    assert!(!rt.set_system_text_scale(1.5), "the same size changes nothing");
    assert!(rt.set_system_text_scale(1.0));
    assert_eq!(row(&mut rt), 20.0);
}

#[test]
fn a_size_that_is_no_size_is_the_default_and_an_extreme_one_is_held() {
    let mut rt = runtime();
    assert!(!rt.set_system_text_scale(f32::NAN));
    assert!(!rt.set_system_text_scale(0.0));
    assert!(rt.set_system_text_scale(100.0));
    assert_eq!(row(&mut rt), 160.0);
}
