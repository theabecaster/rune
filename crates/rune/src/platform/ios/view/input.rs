//! What reaches a `RuneView` from fingers and keys: touches, the pinch and rotation
//! recognizers, a hardware keyboard's presses and their repeat, and the edits the keyboard's
//! text input makes. Each goes to the runtime through the view's surface and wakes its pacing.

use objc2_core_foundation::CGPoint;
use objc2_foundation_video::NSString;
use objc2_video::rc::{Allocated, Retained, Weak};
use objc2_video::runtime::{AnyClass, AnyObject, Sel};
use objc2_video::{DefinedClass, class, msg_send, sel};

use super::super::keys;
use super::super::pacing::Event;
use super::super::text::doc::Effect as Edit;
use super::super::text::geometry::Geometry;
use super::{MainOnly, RuneView, Stage};
use crate::machine::Effects;
use crate::platform::main_queue::after_main;
use crate::runtime::TouchPhase;
use crate::{GesturePhase, Ime, Key, KeyEvent, Mods};

/// `UIGestureRecognizerState`'s `Began`, `Changed`, `Ended` and `Cancelled`.
const GESTURE_BEGAN: isize = 1;
const GESTURE_CHANGED: isize = 2;
const GESTURE_ENDED: isize = 3;
const GESTURE_CANCELLED: isize = 4;
/// `UIEventButtonMaskSecondary`: a mouse's right button, a trackpad's two-finger click.
const BUTTON_SECONDARY: isize = 1 << 1;
/// `UIScrollTypeMaskAll`: a mouse's wheel and a trackpad's two fingers.
const SCROLL_ALL: isize = 3;

impl RuneView {
    /// Adds a recognizer of `class` that calls `action` on the view. When it begins, UIKit
    /// cancels the touches the view holds, so a pinch never also pans or presses.
    pub(super) fn recognize(&self, class: &AnyClass, action: Sel) -> Retained<AnyObject> {
        // SAFETY: `initWithTarget:action:` is a gesture recognizer's designated initializer,
        // `action` is a method of this class that takes the recognizer, and the view conforms
        // to `UIGestureRecognizerDelegate`. A recognizer keeps neither its target nor its
        // delegate, and the view keeps the recognizer.
        unsafe {
            let recognizer: Allocated<AnyObject> = msg_send![class, alloc];
            let recognizer: Retained<AnyObject> = msg_send![recognizer, initWithTarget: self, action: action];
            let _: () = msg_send![&*recognizer, setDelegate: self];
            let _: () = msg_send![self, addGestureRecognizer: &*recognizer];
            recognizer
        }
    }

    /// Adds what hears a mouse or a trackpad on an iPad: a hover recognizer, and a pan that
    /// takes scrolls and no touches, so a finger's pan stays the view's own.
    pub(super) fn recognize_pointer(&self) {
        self.recognize(class!(UIHoverGestureRecognizer), sel!(runeHover:));
        let scroll = self.recognize(class!(UIPanGestureRecognizer), sel!(runeScroll:));
        // SAFETY: a pan recognizer's scroll types are an option set and its touch types an
        // array of numbers, here none.
        unsafe {
            let _: () = msg_send![&*scroll, setAllowedScrollTypesMask: SCROLL_ALL];
            let none: Retained<AnyObject> = msg_send![class!(NSArray), array];
            let _: () = msg_send![&*scroll, setAllowedTouchTypes: &*none];
        }
    }

    /// The pointer moved over the view, or left it.
    pub(super) fn hovered(&self, recognizer: &AnyObject) {
        let Some((phase, x, y)) = self.gesture(recognizer) else { return };
        self.with_surface(|surface| surface.hover((phase != GesturePhase::Ended).then_some((x, y))));
        self.pace(Event::Wake);
    }

    /// A wheel turned or two fingers moved on a trackpad: the content under the pointer moves
    /// by as much, and two fingers that lift while moving leave it gliding.
    pub(super) fn scrolled(&self, recognizer: &AnyObject) {
        let Some((phase, x, y)) = self.gesture(recognizer) else { return };
        // SAFETY: a pan recognizer's translation and velocity in a view are `CGPoint`s.
        // Setting the translation back to zero makes the next one the move since this one.
        let (moved, velocity): (CGPoint, CGPoint) = unsafe {
            let moved = msg_send![recognizer, translationInView: self];
            let _: () = msg_send![recognizer, setTranslation: CGPoint::new(0.0, 0.0), inView: self];
            (moved, msg_send![recognizer, velocityInView: self])
        };
        self.with_surface(|surface| surface.scroll(phase, (x, y), (moved.x as f32, moved.y as f32), (velocity.x as f32, velocity.y as f32)));
        self.track(phase);
    }

    /// Whether these touches are a secondary click, which opens what a long press opens. The
    /// runtime hears nothing else of them.
    pub(super) fn secondary_click(&self, touches: &AnyObject, event: Option<&AnyObject>) -> bool {
        // SAFETY: an event's `buttonMask` is an option set; a set of touches has any one of
        // them, whose location in a view is a `CGPoint`.
        let at: Option<CGPoint> = unsafe {
            let mask: isize = match event { Some(event) => msg_send![event, buttonMask], None => 0 };
            let touch: Option<Retained<AnyObject>> = if mask & BUTTON_SECONDARY != 0 { msg_send![touches, anyObject] } else { None };
            touch.map(|touch| msg_send![&*touch, locationInView: self])
        };
        let Some(at) = at else { return false };
        self.with_surface(|surface| surface.secondary(at.x as f32, at.y as f32));
        self.pace(Event::Wake);
        true
    }

    /// A recognizer's phase and where its fingers' centre is in the view, or `None` in a state
    /// that carries no gesture (possible, failed).
    pub(super) fn gesture(&self, recognizer: &AnyObject) -> Option<(GesturePhase, f32, f32)> {
        // SAFETY: a recognizer's `state` is an `NSInteger` enum and its location a `CGPoint`.
        let (state, at): (isize, CGPoint) = unsafe { (msg_send![recognizer, state], msg_send![recognizer, locationInView: self]) };
        let phase = match state {
            GESTURE_BEGAN => GesturePhase::Began,
            GESTURE_CHANGED => GesturePhase::Moved,
            GESTURE_ENDED | GESTURE_CANCELLED => GesturePhase::Ended,
            _ => return None,
        };
        Some((phase, at.x as f32, at.y as f32))
    }

    /// Tells the pacing a two-finger gesture began, moved or ended.
    pub(super) fn track(&self, phase: GesturePhase) {
        match phase {
            GesturePhase::Began => self.pace(Event::Hold),
            GesturePhase::Ended => self.pace(Event::Release),
            GesturePhase::Moved => {}
        }
        self.pace(Event::Wake);
    }

    /// Where the focused field's characters are, for the keyboard's text input. None while the
    /// view's surface is busy with a frame, which asks nothing of the keyboard.
    pub(crate) fn text_geometry(&self) -> Option<Geometry> {
        match &*self.ivars().stage.try_borrow().ok()? {
            Stage::Live(surface) => surface.text_geometry(),
            Stage::Waiting(_) | Stage::Failed => None,
        }
    }

    /// Carries the keyboard's edits to the runtime, which applies them on its next frame.
    pub(crate) fn text_edits(&self, edits: Effects<Edit>) {
        self.with_surface(|surface| {
            for edit in edits {
                match edit {
                    Edit::Key(key) => surface.key(KeyEvent { key, mods: Mods::default() }),
                    Edit::Commit(text) => surface.ime(Ime::Commit(text)),
                    Edit::Preedit(text, cursor) => surface.ime(Ime::Preedit(text, cursor, Vec::new())),
                    Edit::Select(range) => surface.ime(Ime::Select(range)),
                    // For the keyboard, not the runtime.
                    Edit::TextChanged | Edit::SelectionChanged => {}
                }
            }
        });
        self.pace(Event::Wake);
    }

    /// Takes the hardware keyboard, unless the keyboard's text input has it for a field.
    pub(crate) fn take_keys(&self) {
        // SAFETY: `UIResponder` methods of these signatures.
        unsafe {
            let first: bool = msg_send![self, isFirstResponder];
            if !self.typing() && !first { let _: bool = msg_send![self, becomeFirstResponder]; }
        }
    }

    /// Whether the keyboard's text input is first responder, for a focused field.
    pub(super) fn typing(&self) -> bool {
        // SAFETY: a `UIResponder` method of this signature.
        self.ivars().text.borrow().as_ref().is_some_and(|text| unsafe { msg_send![&**text, isFirstResponder] })
    }

    /// Hardware keys going down or up, for this view or (`typing`) for its text input. True
    /// when the runtime took every one of them; otherwise they are UIKit's.
    pub(crate) fn presses(&self, presses: &AnyObject, down: bool, typing: bool) -> bool {
        // Presses the text input left to UIKit come up the responder chain to this view, its
        // superview: they are still UIKit's.
        if !typing && self.typing() { return false; }
        let mut read = Vec::new();
        // SAFETY: `presses` is the `NSSet<UIPress *>` UIKit passes; a press's `key` is a
        // `UIKey` or nil, whose properties have these types.
        unsafe {
            let all: Retained<AnyObject> = msg_send![presses, allObjects];
            let count: usize = msg_send![&*all, count];
            for index in 0..count {
                let press: Retained<AnyObject> = msg_send![&*all, objectAtIndex: index];
                let key: Option<Retained<AnyObject>> = msg_send![&*press, key];
                let Some(key) = key else { return false };
                let base: Retained<NSString> = msg_send![&*key, charactersIgnoringModifiers];
                let typed: Retained<NSString> = msg_send![&*key, characters];
                read.push(keys::Press {
                    usage: msg_send![&*key, keyCode],
                    flags: msg_send![&*key, modifierFlags],
                    base: base.to_string(),
                    typed: typed.to_string(),
                });
            }
        }
        if !down {
            for press in &read { self.keys(keys::Event::Up { usage: press.usage }); }
            return true;
        }
        let events: Option<Vec<_>> = read.iter().map(|press| keys::event(press, typing).map(|event| (press.usage, event))).collect();
        let Some(events) = events else { return false };
        for (usage, event) in events { self.keys(keys::Event::Down { usage, event }); }
        true
    }

    /// The shortcut Command plus `c`, for an edit action UIKit sent (copy, cut, paste, select all).
    pub(crate) fn command(&self, c: char) {
        let mods = Mods { cmd: true, logo: true, ..Mods::default() };
        self.with_surface(|surface| surface.key(KeyEvent { key: Key::Command(c), mods }));
        self.pace(Event::Wake);
    }

    /// Steps the key repeat and does what it asks.
    pub(super) fn keys(&self, event: keys::Event) {
        let ivars = self.ivars();
        let mut seq = ivars.key_seq.get();
        let effects = keys::step(&mut seq, &mut ivars.keys.borrow_mut(), event);
        ivars.key_seq.set(seq);
        let mut sent = false;
        for effect in effects {
            match effect {
                keys::Effect::Send(key) => sent |= self.with_surface(|surface| surface.key(key)).is_some(),
                keys::Effect::TimerAfter(attempt, seconds) => {
                    let view = MainOnly(Weak::new(self));
                    after_main(seconds, move || view.keys(keys::Event::Timer(attempt)));
                }
            }
        }
        if sent { self.pace(Event::Wake); }
    }

    pub(super) fn touches(&self, touches: &AnyObject, phase: TouchPhase) {
        if phase == TouchPhase::Started { self.take_keys(); }
        let mut count = 0;
        self.with_surface(|surface| {
            // SAFETY: `touches` is the `NSSet<UITouch *>` UIKit passes; a touch's location and
            // timestamp are a `CGPoint` and an `NSTimeInterval`.
            unsafe {
                let all: Retained<AnyObject> = msg_send![touches, allObjects];
                count = msg_send![&*all, count];
                for index in 0..count {
                    let touch: Retained<AnyObject> = msg_send![&*all, objectAtIndex: index];
                    let at: CGPoint = msg_send![&*touch, locationInView: self];
                    let time: f64 = msg_send![&*touch, timestamp];
                    // UIKit keeps one `UITouch` for a finger from down to up, so its address
                    // tells fingers apart.
                    surface.touch(Retained::as_ptr(&touch) as usize as u64, phase, at.x as f32, at.y as f32, time);
                }
            }
        });
        // Each finger holds the display link from when it lands until it lifts.
        for _ in 0..count {
            match phase {
                TouchPhase::Started => self.pace(Event::Hold),
                TouchPhase::Ended | TouchPhase::Cancelled => self.pace(Event::Release),
                TouchPhase::Moved => {}
            }
        }
        self.pace(Event::Wake);
    }}
