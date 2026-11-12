//! Reading an AccessKit tree as a screen reader does: where a node is, and the name it is heard
//! by. Shared by everything that walks the tree Rune builds.

use accesskit::{Node, NodeId, Role};

/// Where `n`'s own bounds start, given its parent's `origin`. The root's transform scales
/// points to pixels and is left out; below it every transform is an offset in points.
pub(crate) fn origin(n: &Node, origin: [f64; 2], root: bool) -> [f64; 2] {
    match n.transform().filter(|_| !root) {
        Some(t) => { let c = t.as_coeffs(); [origin[0] + c[4], origin[1] + c[5]] }
        None => origin,
    }
}

/// The name a reader hears for `n`, by AccessKit's rules: its label, a label's own text, or
/// for a button, a link and the like the text and image labels inside it.
pub(crate) fn name<'a>(n: &Node, get: impl Fn(NodeId) -> Option<&'a Node> + Copy) -> Option<String> {
    if let Some(l) = n.label() { return Some(l.to_owned()); }
    if n.role() == Role::Label { return n.value().map(str::to_owned); }
    if !super::roles::named_by_accesskit(n.role()) { return None; }
    let mut parts = Vec::new();
    descendant_labels(n, get, &mut parts);
    (!parts.is_empty()).then(|| parts.join(" "))
}

fn descendant_labels<'a>(n: &Node, get: impl Fn(NodeId) -> Option<&'a Node> + Copy, out: &mut Vec<String>) {
    for c in n.children() {
        let Some(c) = get(*c) else { continue };
        match c.role() {
            Role::Label => if let Some(v) = c.value() { out.push(v.to_owned()) },
            Role::Image => if let Some(l) = c.label() { out.push(l.to_owned()) },
            Role::GenericContainer => descendant_labels(c, get, out),
            _ => {}
        }
    }
}
