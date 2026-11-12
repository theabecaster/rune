use accesskit::{Affine, Rect, TreeInfo};

use super::*;

fn node(role: Role, bounds: [f64; 4], children: &[u64]) -> Node {
    let mut n = Node::new(role);
    n.set_bounds(Rect { x0: bounds[0], y0: bounds[1], x1: bounds[0] + bounds[2], y1: bounds[1] + bounds[3] });
    n.set_children(children.iter().map(|id| NodeId(*id)).collect::<Vec<_>>());
    n
}

fn label(text: &str, bounds: [f64; 4]) -> Node {
    let mut n = node(Role::Label, bounds, &[]);
    n.set_value(text);
    n
}

fn whole(nodes: Vec<(u64, Node)>) -> TreeUpdate {
    TreeUpdate { nodes: nodes.into_iter().map(|(id, n)| (NodeId(id), n)).collect(), tree: Some(TreeInfo::new(NodeId(1))), tree_id: TreeId::ROOT, focus: NodeId(1) }
}

fn part(nodes: Vec<(u64, Node)>) -> TreeUpdate {
    TreeUpdate { tree: None, ..whole(nodes) }
}

fn pressable(mut n: Node) -> Node { n.add_action(Action::Click); n }

fn labels(mirror: &Mirror) -> Vec<&str> { mirror.elements().iter().map(|e| e.label.as_str()).collect() }

/// A window with a title, a button holding its own text, and a switch.
fn screen() -> Vec<(u64, Node)> {
    let mut heading = node(Role::Heading, [16.0, 60.0, 200.0, 30.0], &[]);
    heading.set_label("Messages");
    let mut switch = pressable(node(Role::Switch, [16.0, 200.0, 60.0, 30.0], &[]));
    switch.set_label("Mute");
    switch.set_toggled(Toggled::True);
    vec![
        (1, node(Role::Window, [0.0, 0.0, 400.0, 800.0], &[2, 3, 5])),
        (2, heading),
        (3, pressable(node(Role::Button, [300.0, 60.0, 80.0, 44.0], &[4]))),
        (4, label("Send", [10.0, 10.0, 60.0, 24.0])),
        (5, switch),
    ]
}

#[test]
fn a_tree_flattens_to_its_controls_and_text_in_order_with_containers_walked_through() {
    let mut mirror = Mirror::default();
    let changes = mirror.apply(whole(screen()));
    assert!(changes.layout);
    assert_eq!(labels(&mirror), ["Messages", "Send", "Mute"]);
    let [heading, button, switch] = mirror.elements() else { panic!() };
    assert_eq!(heading.traits, traits::STATIC_TEXT | traits::HEADER);
    assert_eq!((button.traits, button.rect), (traits::BUTTON, [300.0, 60.0, 80.0, 44.0]));
    assert_eq!((switch.traits, switch.value.as_deref()), (traits::BUTTON | traits::TOGGLE, Some("1")));
}

#[test]
fn text_inside_a_control_is_its_name_and_not_a_stop_of_its_own() {
    let mut mirror = Mirror::default();
    mirror.apply(whole(screen()));
    assert_eq!(mirror.elements().iter().filter(|e| e.label == "Send").count(), 1);
    assert_eq!(mirror.elements()[1].id, 3);
}

#[test]
fn an_offset_moves_everything_below_it_and_the_roots_scale_moves_nothing() {
    let mut root = node(Role::Window, [0.0, 0.0, 400.0, 800.0], &[2]);
    root.set_transform(Affine::scale(3.0));
    let mut list = node(Role::ScrollView, [0.0, 100.0, 400.0, 600.0], &[3]);
    list.set_transform(Affine::translate((0.0, -250.0)));
    let mut mirror = Mirror::default();
    mirror.apply(whole(vec![(1, root), (2, list), (3, label("Row", [16.0, 400.0, 200.0, 40.0]))]));
    assert_eq!(mirror.elements()[0].rect, [16.0, 150.0, 200.0, 40.0]);
}

#[test]
fn what_a_scroll_view_cuts_off_is_cut_from_the_frame_and_what_it_hides_collapses_onto_its_edge() {
    let mut list = node(Role::ScrollView, [0.0, 100.0, 400.0, 600.0], &[3, 4, 5]);
    list.set_clips_children();
    let mut mirror = Mirror::default();
    mirror.apply(whole(vec![
        (1, node(Role::Window, [0.0, 0.0, 400.0, 800.0], &[2])),
        (2, list),
        (3, label("Above", [16.0, 20.0, 200.0, 40.0])),
        (4, label("Half", [16.0, 80.0, 200.0, 40.0])),
        (5, label("Inside", [16.0, 300.0, 200.0, 40.0])),
    ]));
    assert_eq!(labels(&mirror), ["Above", "Half", "Inside"], "all three stay in the order");
    let rects: Vec<_> = mirror.elements().iter().map(|e| e.rect).collect();
    assert_eq!(rects, [[16.0, 100.0, 200.0, 0.0], [16.0, 100.0, 200.0, 20.0], [16.0, 300.0, 200.0, 40.0]]);
}

#[test]
fn hidden_nodes_and_everything_in_them_are_skipped_and_so_is_text_with_nothing_to_say() {
    let mut group = node(Role::Group, [0.0, 0.0, 100.0, 100.0], &[3]);
    group.set_hidden();
    let mut mirror = Mirror::default();
    mirror.apply(whole(vec![
        (1, node(Role::Window, [0.0, 0.0, 400.0, 800.0], &[2, 4, 5])),
        (2, group),
        (3, label("Unseen", [0.0, 0.0, 10.0, 10.0])),
        (4, label("  ", [0.0, 0.0, 10.0, 10.0])),
        (5, label("Seen", [0.0, 0.0, 10.0, 10.0])),
    ]));
    assert_eq!(labels(&mirror), ["Seen"]);
}

#[test]
fn an_empty_field_is_read_by_its_placeholder_and_a_filled_one_by_its_text() {
    let mut field = pressable(node(Role::TextInput, [0.0, 0.0, 200.0, 40.0], &[]));
    field.set_placeholder("Message");
    let mut mirror = Mirror::default();
    mirror.apply(whole(vec![(1, node(Role::Window, [0.0, 0.0, 400.0, 800.0], &[2])), (2, field.clone())]));
    assert_eq!((mirror.elements()[0].label.as_str(), mirror.elements()[0].value.as_deref()), ("Message", None));
    field.set_label("Draft");
    field.set_value("hello");
    let changes = mirror.apply(part(vec![(2, field)]));
    assert!(!changes.layout, "the same elements in the same order");
    assert_eq!((mirror.elements()[0].label.as_str(), mirror.elements()[0].value.as_deref()), ("Draft", Some("hello")));
}

#[test]
fn a_disabled_or_selected_control_says_so_and_a_slider_is_adjustable_with_its_value() {
    let mut tab = pressable(node(Role::Tab, [0.0, 0.0, 80.0, 40.0], &[]));
    tab.set_label("Inbox");
    tab.set_selected(true);
    tab.set_disabled();
    let mut slider = node(Role::Slider, [0.0, 50.0, 200.0, 40.0], &[]);
    slider.set_label("Volume");
    slider.set_numeric_value(40.0);
    slider.add_action(Action::Increment);
    let mut mirror = Mirror::default();
    mirror.apply(whole(vec![(1, node(Role::Window, [0.0, 0.0, 400.0, 800.0], &[2, 3])), (2, tab), (3, slider)]));
    assert_eq!(mirror.elements()[0].traits, traits::BUTTON | traits::DISABLED | traits::SELECTED);
    assert_eq!((mirror.elements()[1].traits, mirror.elements()[1].value.as_deref()), (traits::ADJUSTABLE, Some("40")));
    assert_eq!(mirror.request(3, Gesture::Increment).map(|r| r.action), Some(Action::Increment));
    assert_eq!(mirror.request(3, Gesture::Decrement), None, "the slider takes no decrement");
}

#[test]
fn a_double_tap_clicks_or_else_focuses_and_asks_nothing_of_what_takes_neither() {
    let mut field = node(Role::TextInput, [0.0, 0.0, 200.0, 40.0], &[]);
    field.add_action(Action::Focus);
    let mut mirror = Mirror::default();
    mirror.apply(whole(vec![
        (1, node(Role::Window, [0.0, 0.0, 400.0, 800.0], &[2, 3, 4])),
        (2, pressable(node(Role::Button, [0.0, 0.0, 10.0, 10.0], &[]))),
        (3, field),
        (4, label("Text", [0.0, 0.0, 10.0, 10.0])),
    ]));
    let action = |id| mirror.request(id, Gesture::Activate).map(|r| (r.action, r.target_node));
    assert_eq!(action(2), Some((Action::Click, NodeId(2))));
    assert_eq!(action(3), Some((Action::Focus, NodeId(3))));
    assert_eq!(action(4), None);
    assert_eq!(action(99), None);
}

#[test]
fn a_scroll_goes_to_the_nearest_scroll_view_around_the_element() {
    let mut list = node(Role::ScrollView, [0.0, 0.0, 400.0, 600.0], &[3]);
    list.add_action(Action::ScrollDown);
    let mut mirror = Mirror::default();
    mirror.apply(whole(vec![
        (1, node(Role::Window, [0.0, 0.0, 400.0, 800.0], &[2, 4])),
        (2, list),
        (3, label("Row", [0.0, 0.0, 10.0, 10.0])),
        (4, label("Outside", [0.0, 0.0, 10.0, 10.0])),
    ]));
    let scroll = |id, toward| mirror.request(id, Gesture::Scroll(toward)).map(|r| (r.action, r.target_node));
    assert_eq!(scroll(3, Toward::Down), Some((Action::ScrollDown, NodeId(2))));
    assert_eq!(scroll(3, Toward::Up), None, "the list is at its top");
    assert_eq!(scroll(4, Toward::Down), None);
}

#[test]
fn a_live_region_announces_new_words_once_and_nothing_it_held_before_a_reader_asked() {
    let live = |words: &str| {
        let mut n = node(Role::Status, [0.0, 0.0, 0.0, 0.0], &[]);
        n.set_label(words);
        n.set_live(Live::Polite);
        n
    };
    let mut mirror = Mirror::default();
    let first = mirror.apply(whole(vec![(1, node(Role::Window, [0.0, 0.0, 400.0, 800.0], &[2])), (2, live("Old news"))]));
    assert_eq!(first.announce, Vec::<String>::new());
    assert_eq!(mirror.apply(part(vec![(2, live("Sent"))])).announce, ["Sent"]);
    assert_eq!(mirror.apply(part(vec![(2, live("Sent"))])).announce, Vec::<String>::new());
    assert_eq!(mirror.apply(part(vec![(2, live("Sent\u{200B}"))])).announce, ["Sent\u{200B}"], "said again");
}

#[test]
fn nodes_the_tree_no_longer_reaches_leave_the_mirror() {
    let mut mirror = Mirror::default();
    mirror.apply(whole(screen()));
    let changes = mirror.apply(part(vec![(1, node(Role::Window, [0.0, 0.0, 400.0, 800.0], &[2]))]));
    assert!(changes.layout);
    assert_eq!(labels(&mirror), ["Messages"]);
    assert_eq!(mirror.request(3, Gesture::Activate), None);
}
