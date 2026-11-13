//! What a `RuneView` draws: one app's runtime and renderer on the view's `CAMetalLayer`, with the
//! app's state type erased so one Objective-C class serves every app.

use std::ffi::c_void;
use std::sync::Arc;

use crate::app::{App, Shared};
use crate::embed::{EmbedFrame, LayerHost};
use crate::runtime::{TouchPhase, Waker};
use crate::{GesturePhase, Ime, KeyEvent, MagnifyEvent, RotateEvent};
use super::text::doc::Snapshot;
use super::text::geometry::Geometry;
use crate::a11y::AccessibilityLink;
use crate::app::OnWindow;
use crate::file_dialog::{FileDialogError, FileDialogKind, FileDialogOptions};
use crate::popup_menu::Show;
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use crate::edit::ImeText;
use crate::PhysicalInput;
use super::keys::Stroke;
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
    /// The pointer of a mouse or trackpad is at this point of the view, or left it.
    fn hover(&mut self, at: Option<(f32, f32)>);
    /// A wheel or a trackpad scrolled the content under `at` by `moved` points; `velocity` is
    /// its speed in points a second when the scroll ends.
    fn scroll(&mut self, phase: GesturePhase, at: (f32, f32), moved: (f32, f32), velocity: (f32, f32));
    /// Two fingers pinched: scale by `1.0 + delta` about (x, y) in the view's points.
    fn magnify(&mut self, delta: f32, x: f32, y: f32, phase: GesturePhase);
    /// Two fingers turned `degrees`, positive counterclockwise, about (x, y).
    fn rotate(&mut self, degrees: f32, x: f32, y: f32, phase: GesturePhase);
    /// Whether a secondary press at (x, y) would open a menu or reach a handler.
    fn takes_secondary(&self, x: f32, y: f32) -> bool;
    /// A secondary press at (x, y): a long press.
    fn secondary(&mut self, x: f32, y: f32);
    /// The next pop-up menu to show, once the frame that asked for it is done.
    fn popup(&mut self) -> Option<(Show, bool)>;
    /// The menu showing was answered with an item's id, or dismissed.
    fn popup_pick(&mut self, picked: Option<String>);
    /// The file dialog to show, once the frame that asked for it is done.
    fn file_dialog(&mut self) -> Option<(u64, FileDialogKind, FileDialogOptions)>;
    /// The dialog's answer: the chosen file, none when cancelled, or why it could not show.
    fn file_dialog_answer(&mut self, id: u64, answer: Result<Option<PathBuf>, String>);
    /// A key from the soft keyboard, or an edit action UIKit sent.
    fn key(&mut self, key: KeyEvent);
    /// A hardware key going down (`pressed`, held when `repeat`) or up.
    fn stroke(&mut self, stroke: Stroke, pressed: bool, repeat: bool);
    /// A composition, a commit or a selection from the keyboard.
    fn ime(&mut self, ime: Ime);
    /// What the focused field tells the keyboard, as of the last frame.
    fn text(&self) -> TextState;
    /// Where the focused field's characters are, as last drawn.
    fn text_geometry(&self) -> Option<Geometry>;
    /// An assistive client asked for the view's elements: the runtime builds its tree.
    fn accessibility_wanted(&mut self);
    /// What a client did to an element, for the next frame.
    fn accessibility_act(&mut self, request: ActionRequest);
    /// The accessibility tree's changes since the last frame, while a client listens, and the
    /// view's zoom: the tree is in the app's points, which the zoom scales to the view's.
    fn accessibility(&mut self) -> Option<(TreeUpdate, f32)>;
    /// Whether the app is in front, for `AppHandle::is_visible`.
    fn set_visible(&mut self, visible: bool);
    /// The system's appearance.
    fn set_dark(&mut self, dark: bool);
    /// The Reduce Motion setting.
    fn set_reduced_motion(&mut self, reduced: bool);
    /// The text size setting, as a scale of its default.
    fn set_text_scale(&mut self, scale: f32);
    /// What the screen keeps at the view's top, right, bottom and left edges, in points.
    fn set_safe_area(&mut self, insets: [f32; 4]);
}

struct Hosted<S: 'static> {
    host: LayerHost<S>,
    /// What the app's handles reach it through: `AppHandle::update` and `wake` from any thread.
    shared: Arc<Shared<S>>,
    accessibility: AccessibilityLink,
    /// The app's name, which names the accessibility tree's root.
    title: &'static str,
    /// `App::on_ready`, which runs once the first frame is on screen, as a window's does.
    on_ready: Option<OnWindow>,
    /// The focused field's text and selection when the finger that is down landed, if it had
    /// one selected.
    selected: Option<ImeText>,
    tap: Tap,
    /// Where the field menu showing was asked for.
    field_menu: Option<(f32, f32)>,
    /// A secondary press to make once the next frame is done: it opens the field's menu again.
    reopen: Option<(f32, f32)>,
    wake: Waker,
}

/// How far a finger may move, in points, and still be a tap.
const TAP_SLOP: f32 = 10.0;

/// A finger on the focused text field, watched for a tap that leaves the caret where it was:
/// the tap that asks for the field's menu, as in any iOS text field.
#[derive(Default)]
enum Tap {
    #[default]
    None,
    /// Down at this point of the field, which held `before`.
    Down { at: (f32, f32), before: ImeText },
    /// Lifted without moving: the frame that applies the tap tells whether the caret moved.
    Lifted { at: (f32, f32), before: ImeText },
}

impl<S: 'static> Surface for Hosted<S> {
    fn resize(&mut self, size: (f32, f32), scale: f32) { self.host.resize(size, scale); }

    fn frame(&mut self, target: Instant) -> EmbedFrame {
        self.accessibility.receive(self.host.runtime_mut(), self.title);
        // What the app's handles asked since the last frame. A view is not a window: requests
        // to show, move or quit one have nothing to act on, and only the updates run.
        for update in self.shared.take().updates { self.host.runtime_mut().update_if(update); }
        let frame = self.host.frame_at(target);
        // There is no window to hand over: the handle is one no window answers, as on the web.
        if frame.drew && let Some(mut ready) = self.on_ready.take() { ready(&crate::WindowHandle::new("main")); }
        // A tap on the focused field that left its caret where it was asks for the menu.
        if matches!(self.tap, Tap::Lifted { .. })
            && let Tap::Lifted { at, before } = std::mem::take(&mut self.tap)
            && self.host.runtime().ime_text().as_ref() == Some(&before)
            && self.host.runtime().popup_menu().is_none()
        {
            self.reopen = Some(at);
        }
        if let Some((x, y)) = self.reopen.take() {
            self.secondary(x, y);
            // The press lands in the next frame, which nothing else may ask for.
            (self.wake)();
        }
        frame
    }

    fn accessibility_wanted(&mut self) { self.accessibility.activate(); }

    fn accessibility_act(&mut self, request: ActionRequest) { self.accessibility.act(request); }

    fn accessibility(&mut self) -> Option<(TreeUpdate, f32)> {
        let zoom = self.host.runtime().zoom();
        self.accessibility.update(self.host.runtime_mut()).map(|update| (update, zoom))
    }

    fn touch(&mut self, id: u64, phase: TouchPhase, x: f32, y: f32, time: f64) {
        let (vx, vy) = self.in_view(x, y);
        let rt = self.host.runtime_mut();
        if phase == TouchPhase::Started && !rt.touching() {
            self.selected = rt.ime_text().filter(|text| !text.selection.is_empty());
            self.tap = match rt.ime_text().filter(|text| text.selection.is_empty() && rt.on_focused_field(vx, vy)) {
                Some(before) => Tap::Down { at: (x, y), before },
                None => Tap::None,
            };
        }
        self.tap = match (std::mem::take(&mut self.tap), phase) {
            (Tap::Down { at, before }, TouchPhase::Moved) if (x - at.0).hypot(y - at.1) <= TAP_SLOP => Tap::Down { at, before },
            (Tap::Down { at, before }, TouchPhase::Ended) => Tap::Lifted { at, before },
            (Tap::Down { at, before }, TouchPhase::Started) => Tap::Down { at, before },
            _ => Tap::None,
        };
        rt.touch(id, phase, vx, vy, time);
    }

    fn hover(&mut self, at: Option<(f32, f32)>) {
        let at = at.map(|(x, y)| self.in_view(x, y));
        let rt = self.host.runtime_mut();
        // A finger's touch is the pointer while it is down.
        if rt.touching() { return; }
        match at {
            Some((x, y)) => rt.pointer_move(x, y),
            None => rt.pointer_leave(),
        }
    }

    fn scroll(&mut self, phase: GesturePhase, (x, y): (f32, f32), (dx, dy): (f32, f32), (vx, vy): (f32, f32)) {
        // A scroll moves the content as far on screen at any zoom, as a desktop wheel does.
        let (x, y) = self.in_view(x, y);
        let ((dx, dy), (vx, vy)) = (self.in_view(dx, dy), self.in_view(vx, vy));
        let rt = self.host.runtime_mut();
        match phase {
            GesturePhase::Began => { rt.wheel_began(); rt.wheel_xy(x, y, dx, dy); }
            GesturePhase::Moved => rt.wheel_xy(x, y, dx, dy),
            GesturePhase::Ended => {
                rt.wheel_xy(x, y, dx, dy);
                rt.wheel_ended();
                // UIKit sends no momentum after the fingers lift: the runtime glides it.
                if vx != 0.0 || vy != 0.0 { rt.fling_xy(x, y, vx, vy); }
            }
        }
    }

    fn magnify(&mut self, delta: f32, x: f32, y: f32, phase: GesturePhase) {
        let (x, y) = self.in_view(x, y);
        self.host.runtime_mut().magnify_event(MagnifyEvent::new(delta).at(x, y).phase(phase));
    }

    fn rotate(&mut self, degrees: f32, x: f32, y: f32, phase: GesturePhase) {
        let (x, y) = self.in_view(x, y);
        self.host.runtime_mut().rotate_event(RotateEvent::new(degrees).at(x, y).phase(phase));
    }

    fn takes_secondary(&self, x: f32, y: f32) -> bool {
        let (x, y) = self.in_view(x, y);
        self.host.runtime().takes_secondary(x, y)
    }

    fn secondary(&mut self, x: f32, y: f32) {
        let (x, y) = self.in_view(x, y);
        let rt = self.host.runtime_mut();
        // A finger that lands in a field places the caret at once, as a mouse press does, so a
        // long press on selected text would find nothing selected. The selection the finger
        // landed on is put back first; the field's menu then keeps it for a press inside it
        // and moves the caret for one outside, as for a right-click.
        if let Some(before) = self.selected.take()
            && rt.ime_text().is_some_and(|now| now.text == before.text && now.start == before.start && now.selection.is_empty())
        {
            rt.ime(Ime::Select(before.start + before.selection.start..before.start + before.selection.end));
        }
        rt.secondary_down(x, y);
        rt.secondary_up(x, y);
    }

    fn popup(&mut self) -> Option<(Show, bool)> {
        let rt = self.host.runtime_mut();
        if rt.popup_waits_for_layout() { return None; }
        let field = rt.popup_is_field_menu();
        match rt.take_popup()? {
            Some(show) => {
                self.field_menu = field.then_some(show.at);
                Some((show, field))
            }
            // It can't open: its anchor is gone.
            None => { let _ = rt.resolve_popup(None); None }
        }
    }

    fn popup_pick(&mut self, picked: Option<String>) {
        let _ = self.host.runtime_mut().resolve_popup(picked.as_deref());
        // After Select All a text field shows its menu again, for what is now selected.
        let at = self.field_menu.take();
        if picked.as_deref() == Some("select-all") { self.reopen = at; }
    }

    fn file_dialog(&mut self) -> Option<(u64, FileDialogKind, FileDialogOptions)> {
        self.host.runtime_mut().take_file_dialog()
    }

    fn file_dialog_answer(&mut self, id: u64, answer: Result<Option<PathBuf>, String>) {
        self.host.runtime_mut().answer_file_dialog(id, answer.map_err(FileDialogError));
    }

    fn key(&mut self, key: KeyEvent) { self.host.key(key); }

    fn stroke(&mut self, stroke: Stroke, pressed: bool, repeat: bool) {
        let rt = self.host.runtime_mut();
        let Stroke { code, mods, key, base } = stroke;
        match code {
            // The physical key first, with the logical one for when no handler takes it. A
            // release carries its logical key too, for `on_key_up`.
            Some(code) => { rt.physical_then(PhysicalInput::Key { key: code, mods, pressed, repeat }, key, base, None); }
            // A key with no physical code still edits and moves.
            None => if let Some(key) = key.filter(|_| pressed) { self.host.key(key) },
        }
    }

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

    fn text_geometry(&self) -> Option<Geometry> {
        self.host.runtime().focused_field_lines().map(|lines| Geometry::new(&lines))
    }

    fn set_visible(&mut self, visible: bool) { self.shared.visible.store(visible, Ordering::Release); }

    fn set_dark(&mut self, dark: bool) { self.host.runtime_mut().set_system_dark(dark); }

    fn set_reduced_motion(&mut self, reduced: bool) { self.host.runtime_mut().set_system_reduced_motion(reduced); }

    fn set_text_scale(&mut self, scale: f32) { self.host.runtime_mut().set_system_text_scale(scale); }

    fn set_safe_area(&mut self, insets: [f32; 4]) { self.host.runtime_mut().set_safe_area(insets); }
}

/// Starts an app on a `CAMetalLayer`, once its view has a size: the layer, the view's size in
/// points, its scale, whether the system is dark, and the waker other threads call.
pub(crate) type Mount = Box<dyn FnOnce(*mut c_void, (f32, f32), f32, bool, Waker) -> Result<Box<dyn Surface>, String>>;

impl<S: 'static> Hosted<S> {
    /// A point of the `UIView`, the window's points, in the app's own: a view that zooms
    /// (`Cx::zoom`) lays out in points the zoom scales, as a desktop window's does.
    fn in_view(&self, x: f32, y: f32) -> (f32, f32) {
        let zoom = self.host.runtime().zoom();
        (x / zoom, y / zoom)
    }
}

pub(crate) fn mount<S: 'static>(mut app: App<S>) -> Mount {
    let on_ready = app.on_ready.take();
    let (spec, state, shared) = app.into_parts(None);
    Box::new(move |layer, size, scale, dark, wake| {
        // SAFETY: `layer` is the `CAMetalLayer` of the view that owns this surface, and the
        // view calls this on the main thread.
        let host = unsafe { LayerHost::from_core_animation_layer(layer, &spec, state, size, scale, dark, wake.clone())? };
        let mut host = host.with_system_clipboard(wake.clone()).with_popup_menus().with_file_dialogs();
        // The view is the app's whole host here: links open in the system, as from a window.
        host.runtime_mut().open_urls_natively = true;
        // The handles' requests wake the view, as they wake a window's event loop.
        shared.connect(wake.clone());
        shared.visible.store(true, Ordering::Release);
        Ok(Box::new(Hosted { host, shared, accessibility: AccessibilityLink::with_wake(wake.clone()), wake, title: spec.title, on_ready, selected: None, tap: Tap::None, field_menu: None, reopen: None }))
    })
}
