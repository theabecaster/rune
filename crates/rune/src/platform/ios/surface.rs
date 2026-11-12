//! What a `RuneView` draws: one app's runtime and renderer on the view's `CAMetalLayer`, with the
//! app's state type erased so one Objective-C class serves every app.

use std::ffi::c_void;
use std::sync::Arc;

use crate::app::{App, Shared};
use crate::embed::{EmbedFrame, LayerHost};
use crate::runtime::{TouchPhase, Waker};
use crate::{GesturePhase, Ime, KeyEvent, MagnifyEvent, RotateEvent};
use super::text_doc::Snapshot;
use crate::a11y::AccessibilityLink;
use accesskit::{ActionRequest, TreeUpdate};
use crate::time::Instant;

/// What the focused field tells the keyboard after a frame.
pub(crate) struct TextState {
    /// A text field, or a caret an app draws itself, has the focus.
    pub(crate) wanted: bool,
    pub(crate) snapshot: Snapshot,
    /// The caret's rect in the view's points.
    pub(crate) caret: Option<[f32; 4]>,
    /// The field is a password.
    pub(crate) secure: bool,
}

pub(crate) trait Surface {
    /// The view's size in points and its display scale.
    fn resize(&mut self, size: (f32, f32), scale: f32);
    /// Steps the app to `target`, when the frame will be on screen, and presents it.
    fn frame(&mut self, target: Instant) -> EmbedFrame;
    /// One finger's event at (x, y) in the view's points, `time` in seconds since boot.
    fn touch(&mut self, id: u64, phase: TouchPhase, x: f32, y: f32, time: f64);
    /// Two fingers pinched: scale by `1.0 + delta` about (x, y) in the view's points.
    fn magnify(&mut self, delta: f32, x: f32, y: f32, phase: GesturePhase);
    /// Two fingers turned `degrees`, positive counterclockwise, about (x, y).
    fn rotate(&mut self, degrees: f32, x: f32, y: f32, phase: GesturePhase);
    /// A key from the soft keyboard or a hardware one.
    fn key(&mut self, key: KeyEvent);
    /// A composition, a commit or a selection from the keyboard.
    fn ime(&mut self, ime: Ime);
    /// What the focused field tells the keyboard, as of the last frame.
    fn text(&self) -> TextState;
    /// An assistive client asked for the view's elements: the runtime builds its tree.
    fn accessibility_wanted(&mut self);
    /// What a client did to an element, for the next frame.
    fn accessibility_act(&mut self, request: ActionRequest);
    /// The accessibility tree's changes since the last frame, while a client listens.
    fn accessibility(&mut self) -> Option<TreeUpdate>;
    /// The system's appearance.
    fn set_dark(&mut self, dark: bool);
    /// The Reduce Motion setting.
    fn set_reduced_motion(&mut self, reduced: bool);
    /// What the screen keeps at the view's top, right, bottom and left edges, in points.
    fn set_safe_area(&mut self, insets: [f32; 4]);
}

struct Hosted<S: 'static> {
    host: LayerHost<S>,
    /// What the app's handles reach it through, kept for as long as the app draws.
    _shared: Arc<Shared<S>>,
    accessibility: AccessibilityLink,
    /// The app's name, which names the accessibility tree's root.
    title: &'static str,
}

impl<S: 'static> Surface for Hosted<S> {
    fn resize(&mut self, size: (f32, f32), scale: f32) { self.host.resize(size, scale); }

    fn frame(&mut self, target: Instant) -> EmbedFrame {
        self.accessibility.receive(self.host.runtime_mut(), self.title);
        self.host.frame_at(target)
    }

    fn accessibility_wanted(&mut self) { self.accessibility.activate(); }

    fn accessibility_act(&mut self, request: ActionRequest) { self.accessibility.act(request); }

    fn accessibility(&mut self) -> Option<TreeUpdate> { self.accessibility.update(self.host.runtime_mut()) }

    fn touch(&mut self, id: u64, phase: TouchPhase, x: f32, y: f32, time: f64) {
        self.host.runtime_mut().touch(id, phase, x, y, time);
    }

    fn magnify(&mut self, delta: f32, x: f32, y: f32, phase: GesturePhase) {
        self.host.runtime_mut().magnify_event(MagnifyEvent::new(delta).at(x, y).phase(phase));
    }

    fn rotate(&mut self, degrees: f32, x: f32, y: f32, phase: GesturePhase) {
        self.host.runtime_mut().rotate_event(RotateEvent::new(degrees).at(x, y).phase(phase));
    }

    fn key(&mut self, key: KeyEvent) { self.host.key(key); }

    fn ime(&mut self, ime: Ime) { self.host.runtime_mut().ime(ime); }

    fn text(&self) -> TextState {
        let rt = self.host.runtime();
        let secure = rt.has_secure_focus();
        let snapshot = match rt.ime_text() {
            _ if secure => Snapshot::Hidden,
            _ if rt.focused_preedit().is_some() => Snapshot::Composing,
            Some(text) => Snapshot::Field { text: text.text, start: text.start, selection: text.selection },
            None => Snapshot::Hidden,
        };
        TextState { wanted: rt.wants_text(), snapshot, caret: rt.ime_area.map(|area| rt.to_window(area)), secure }
    }

    fn set_dark(&mut self, dark: bool) { self.host.runtime_mut().set_system_dark(dark); }

    fn set_reduced_motion(&mut self, reduced: bool) { self.host.runtime_mut().set_system_reduced_motion(reduced); }

    fn set_safe_area(&mut self, insets: [f32; 4]) { self.host.runtime_mut().set_safe_area(insets); }
}

/// Starts an app on a `CAMetalLayer`, once its view has a size: the layer, the view's size in
/// points, its scale, whether the system is dark, and the waker other threads call.
pub(crate) type Mount = Box<dyn FnOnce(*mut c_void, (f32, f32), f32, bool, Waker) -> Result<Box<dyn Surface>, String>>;

pub(crate) fn mount<S: 'static>(app: App<S>) -> Mount {
    let (spec, state, shared) = app.into_parts(None);
    Box::new(move |layer, size, scale, dark, wake| {
        // SAFETY: `layer` is the `CAMetalLayer` of the view that owns this surface, and the
        // view calls this on the main thread.
        let host = unsafe { LayerHost::from_core_animation_layer(layer, &spec, state, size, scale, dark, wake.clone())? };
        let mut host = host.with_system_clipboard(wake.clone());
        // The view is the app's whole host here: links open in the system, as from a window.
        host.runtime_mut().open_urls_natively = true;
        Ok(Box::new(Hosted { host, _shared: shared, accessibility: AccessibilityLink::with_wake(wake), title: spec.title }))
    })
}
