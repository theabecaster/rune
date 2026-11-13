use super::*;
use crate::native_menu::MenuItem;

fn d(item: MenuItem<()>) -> Description { item.description }

fn item(id: &str) -> MenuItem<()> { MenuItem::new(id, |_, _| {}) }

fn ids<'a>(group: &Group<'a>) -> Vec<Vec<&'a str>> {
    group.sections.iter().map(|s| s.iter().map(|d| d.id.as_str()).collect()).collect()
}

#[test]
fn items_go_to_the_app_menu_first_then_each_named_menu_in_the_order_it_first_appears() {
    let items = [d(item("open").menu("File")), d(item("prefs")), d(item("zoom").menu("View")), d(item("save").menu("File")), d(item("dock").dock())];
    let groups = groups(&items);
    let menus: Vec<_> = groups.iter().map(|g| g.menu).collect();
    assert_eq!(menus, [None, Some("File"), Some("View")]);
    assert_eq!(ids(&groups[0]), [["prefs"]]);
    assert_eq!(ids(&groups[1]), [["open", "save"]]);
    assert!(groups.iter().all(|g| g.sections.iter().flatten().all(|d| d.id != "dock")), "the Dock has no place");
}

#[test]
fn an_item_after_an_earlier_one_follows_it_and_several_after_one_keep_their_order() {
    let items = [d(item("a")), d(item("b")), d(item("x").after("a")), d(item("y").after("a")), d(item("z").after("b"))];
    assert_eq!(ids(&groups(&items)[0]), [["a", "x", "y", "b", "z"]]);
}

#[test]
fn an_anchor_that_names_no_earlier_item_of_the_menu_leaves_the_item_at_the_end() {
    let items = [d(item("early").after("late")), d(item("late")), d(item("paste-after").after("paste:")),
        d(item("other").menu("File")), d(item("elsewhere").after("other"))];
    assert_eq!(ids(&groups(&items)[0]), [["early", "late", "paste-after", "elsewhere"]]);
}

#[test]
fn separators_split_a_menu_into_sections_and_leave_none_empty() {
    let items = [d(MenuItem::separator()), d(item("a")), d(MenuItem::separator()), d(MenuItem::separator()), d(item("b")),
        d(item("c")), d(MenuItem::separator().after("a"))];
    assert_eq!(ids(&groups(&items)[0]), [vec!["a"], vec!["b", "c"]], "the anchored separator went after a, which a section break already followed");
}

#[test]
fn nothing_gives_no_menus() {
    assert!(groups(&[]).is_empty());
    assert!(groups(&[d(item("dock").dock())]).is_empty());
}
