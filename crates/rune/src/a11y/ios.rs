//! The accessibility tree as UIKit wants it. VoiceOver, Switch Control and Voice Control read
//! a view as a flat, ordered list of elements, each with a label, a value, traits and a frame,
//! where AccessKit's tree nests groups, controls and the text inside them. The mirror keeps
//! the runtime's tree as its updates arrive and flattens it: a control, a piece of text or an
//! image is an element with everything inside it folded into its name, and a container is only
//! walked through. It also turns what a reader does to an element (a double tap, a swipe up on
//! a slider, a three-finger scroll) into the tree's action, and finds the words a live region
//! newly holds.

use accesskit::{Action, ActionRequest, Live, Node, NodeId, Role, Toggled, TreeId, TreeUpdate};

use crate::runtime::HashMap;

/// What kind of thing an element is and what state it is in, as UIKit's traits: the view maps
/// each bit to its `UIAccessibilityTraits` constant.
pub(crate) mod traits {
    pub(crate) const BUTTON: u16 = 1 << 0;
    pub(crate) const LINK: u16 = 1 << 1;
    pub(crate) const IMAGE: u16 = 1 << 2;
    pub(crate) const SELECTED: u16 = 1 << 3;
    pub(crate) const STATIC_TEXT: u16 = 1 << 4;
    pub(crate) const DISABLED: u16 = 1 << 5;
    pub(crate) const ADJUSTABLE: u16 = 1 << 6;
    pub(crate) const HEADER: u16 = 1 << 7;
    pub(crate) const TOGGLE: u16 = 1 << 8;
    pub(crate) const SEARCH: u16 = 1 << 9;
    pub(crate) const UPDATES: u16 = 1 << 10;
}

/// One thing a reader can land on.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Element {
    /// The tree node's id, which stays the same while the thing it stands for lives.
    pub(crate) id: u64,
    pub(crate) label: String,
    pub(crate) value: Option<String>,
    pub(crate) hint: Option<String>,
    pub(crate) traits: u16,
    /// Left, top, width and height in the app's points, which the view's zoom scales to its own.
    pub(crate) rect: [f32; 4],
    /// The nearest scroll view around it, for a reader's scroll gesture.
    scrolls: Option<u64>,
}

/// What a reader did to an element.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Gesture {
    /// A double tap.
    Activate,
    /// A swipe up or down on an adjustable element.
    Increment,
    Decrement,
    /// The reader's cursor landed on it.
    Focused,
    /// A scroll that shows the content further in this direction.
    Scroll(Toward),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Toward { Up, Down, Left, Right }

/// What an update changed for the reader.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Changes {
    /// Elements came, went or moved in the order: the reader should look again.
    pub(crate) layout: bool,
    /// What live regions newly say, in tree order.
    pub(crate) announce: Vec<String>,
}

#[derive(Default)]
pub(crate) struct Mirror {
    nodes: HashMap<u64, Node>,
    root: Option<NodeId>,
    elements: Vec<Element>,
    /// What each live region said last, by node.
    said: HashMap<u64, String>,
}

impl Mirror {
    pub(crate) fn elements(&self) -> &[Element] { &self.elements }

    /// Takes the tree's changes and flattens it again.
    pub(crate) fn apply(&mut self, update: TreeUpdate) -> Changes {
        // A whole tree after none (a reader just asked) says nothing: what a live region held
        // before anyone listened is old news.
        let first = self.root.is_none();
        if let Some(tree) = &update.tree { self.root = Some(tree.root); }
        for (id, node) in update.nodes { self.nodes.insert(id.0, node); }
        let mut walk = Walk { nodes: &self.nodes, elements: Vec::new(), live: Vec::new(), seen: Vec::new() };
        if let Some(root) = self.root { walk.node(root, [0.0, 0.0], true, None, None); }
        let Walk { elements, live, seen, .. } = walk;
        let seen: crate::runtime::HashSet<u64> = seen.into_iter().collect();
        self.nodes.retain(|id, _| seen.contains(id));
        let mut announce = Vec::new();
        let mut said = HashMap::default();
        for (id, words) in live {
            if !first && self.said.get(&id) != Some(&words) && !words.trim().is_empty() { announce.push(words.clone()); }
            said.insert(id, words);
        }
        self.said = said;
        let order = |list: &[Element]| list.iter().map(|e| e.id).collect::<Vec<_>>();
        let layout = order(&elements) != order(&self.elements);
        self.elements = elements;
        Changes { layout, announce }
    }

    /// The tree's action for what a reader did to element `id`, if the element takes it.
    pub(crate) fn request(&self, id: u64, gesture: Gesture) -> Option<ActionRequest> {
        let node = self.nodes.get(&id)?;
        let first = |actions: &[Action]| actions.iter().copied().find(|a| node.supports_action(*a));
        let (target, action) = match gesture {
            Gesture::Activate => (id, first(&[Action::Click, Action::Focus])?),
            Gesture::Increment => (id, first(&[Action::Increment])?),
            Gesture::Decrement => (id, first(&[Action::Decrement])?),
            Gesture::Focused => (id, first(&[Action::ScrollIntoView])?),
            Gesture::Scroll(toward) => {
                let scrolls = self.elements.iter().find(|e| e.id == id)?.scrolls?;
                let action = match toward {
                    Toward::Up => Action::ScrollUp,
                    Toward::Down => Action::ScrollDown,
                    Toward::Left => Action::ScrollLeft,
                    Toward::Right => Action::ScrollRight,
                };
                (scrolls, self.nodes.get(&scrolls).filter(|n| n.supports_action(action)).map(|_| action)?)
            }
        };
        Some(ActionRequest { action, target_tree: TreeId::ROOT, target_node: NodeId(target), data: None })
    }
}

struct Walk<'a> {
    nodes: &'a HashMap<u64, Node>,
    elements: Vec<Element>,
    live: Vec<(u64, String)>,
    seen: Vec<u64>,
}

impl Walk<'_> {
    /// `clip` is what the scroll views around the node show of it: left, top, right, bottom.
    fn node(&mut self, id: NodeId, origin: [f64; 2], root: bool, scrolls: Option<u64>, clip: Option<[f64; 4]>) {
        let Some(n) = self.nodes.get(&id.0) else { return };
        self.seen.push(id.0);
        let origin = super::read::origin(n, origin, root);
        let bounds = n.bounds().map(|b| [b.x0 + origin[0], b.y0 + origin[1], b.x1 + origin[0], b.y1 + origin[1]]);
        let hidden = n.is_hidden() || n.role() == Role::TextRun;
        let name = super::read::name(n, |id| self.nodes.get(&id.0));
        if matches!(n.live(), Some(Live::Polite | Live::Assertive)) {
            self.live.push((id.0, name.clone().unwrap_or_default()));
        }
        let scrolls = if SCROLLS.iter().any(|a| n.supports_action(*a)) { Some(id.0) } else { scrolls };
        let shown = bounds.map(|b| clip.map_or(b, |clip| within(b, clip)));
        let element = if hidden { None } else { shown.and_then(|shown| element(id.0, n, name, shown, scrolls)) };
        let clip = if n.clips_children() && !root { shown.or(clip) } else { clip };
        let leaf = element.is_some();
        self.elements.extend(element);
        // An element's name already holds the text inside it; its children are still walked so
        // they stay in the mirror, but add nothing a reader lands on.
        let mark = self.elements.len();
        for child in n.children() { self.node(*child, origin, false, scrolls, clip); }
        if leaf || hidden { self.elements.truncate(mark); }
    }
}

const SCROLLS: [Action; 4] = [Action::ScrollUp, Action::ScrollDown, Action::ScrollLeft, Action::ScrollRight];

/// The part of `b` inside `clip`. Something scrolled out of view keeps its place in the order
/// and collapses onto the edge it left by, where no finger finds it; a reader that steps onto
/// it scrolls it back.
fn within(b: [f64; 4], clip: [f64; 4]) -> [f64; 4] {
    let hold = |v: f64, low: f64, high: f64| v.max(low).min(high.max(low));
    [hold(b[0], clip[0], clip[2]), hold(b[1], clip[1], clip[3]), hold(b[2], clip[0], clip[2]), hold(b[3], clip[1], clip[3])]
}

/// The element `n` is, or none for a container and for text with nothing to say. `shown` is
/// the part of it in view: left, top, right, bottom in the app's points.
fn element(id: u64, n: &Node, name: Option<String>, shown: [f64; 4], scrolls: Option<u64>) -> Option<Element> {
    use traits::*;
    let role = n.role();
    let control = match role {
        Role::Button | Role::DefaultButton | Role::MenuItem | Role::Tab | Role::ListBoxOption | Role::TreeItem | Role::ComboBox
        | Role::RadioButton | Role::MenuItemRadio | Role::Splitter => BUTTON,
        Role::CheckBox | Role::Switch | Role::MenuItemCheckBox => BUTTON | TOGGLE,
        Role::Link => LINK,
        Role::Slider | Role::SpinButton => ADJUSTABLE,
        Role::SearchInput => SEARCH,
        Role::TextInput | Role::MultilineTextInput | Role::PasswordInput => 0,
        _ if n.supports_action(Action::Click) => BUTTON,
        _ => {
            let kind = match role {
                Role::Label | Role::Paragraph | Role::Status | Role::Alert | Role::Tooltip | Role::Note => STATIC_TEXT,
                Role::Heading => STATIC_TEXT | HEADER,
                Role::Image => IMAGE,
                Role::ProgressIndicator | Role::Meter | Role::Timer => UPDATES,
                _ => return None,
            };
            // Text, an image or a gauge with nothing to say is not worth a stop.
            if name.as_deref().is_none_or(|s| s.trim().is_empty()) && n.numeric_value().is_none() { return None; }
            kind
        }
    };
    let mut traits = control;
    if n.is_disabled() { traits |= DISABLED; }
    if n.is_selected() == Some(true) { traits |= SELECTED; }
    let text_field = matches!(role, Role::TextInput | Role::MultilineTextInput | Role::SearchInput);
    let value = match n.toggled() {
        // What `UISwitch` reports, which a reader speaks as on, off or mixed.
        Some(Toggled::True) => Some("1".to_owned()),
        Some(Toggled::False) => Some("0".to_owned()),
        Some(Toggled::Mixed) => Some("2".to_owned()),
        None if text_field || traits & ADJUSTABLE != 0 || traits & UPDATES != 0 => {
            n.value().map(str::to_owned).or_else(|| n.numeric_value().map(|x| format!("{x}")))
        }
        None => None,
    };
    // An empty field is read by its placeholder, as UIKit's own are.
    let (label, value) = match (name, n.placeholder()) {
        (None, Some(placeholder)) => (placeholder.to_owned(), value),
        (name, Some(placeholder)) if value.as_deref().is_none_or(str::is_empty) => (name.unwrap_or_default(), Some(placeholder.to_owned())),
        (name, _) => (name.unwrap_or_default(), value),
    };
    let rect = [shown[0] as f32, shown[1] as f32, (shown[2] - shown[0]) as f32, (shown[3] - shown[1]) as f32];
    let value = value.filter(|v| !v.is_empty());
    Some(Element { id, label, value, hint: n.description().map(str::to_owned), traits, rect, scrolls })
}

#[cfg(test)]
#[path = "ios_tests.rs"]
mod tests;
