//! `RuneTextInput`: what the iOS keyboard talks to. A hidden view inside a `RuneView` that
//! becomes first responder while a text field has the focus, and answers UIKit's text input
//! protocol from the keyboard's copy of the field (`doc`): the soft keyboard, a hardware
//! one, autocorrection, dictation and marked text all edit through it.
//!
//! Positions and ranges are UTF-16 offsets into that copy. Rune draws its own caret and
//! selection, so the geometry UIKit asks for is the caret's rect for every position: enough to
//! place the keyboard's own bubbles, and no system selection handles or loupe.

use std::cell::{Cell, RefCell};

use objc2_core_foundation::{CGFloat, CGPoint, CGRect, CGSize};
use objc2_foundation_video::{NSArray, NSDictionary, NSRange, NSString};
use objc2_video::rc::{Allocated, Retained, Weak};
use objc2_video::runtime::{AnyObject, NSObject, NSObjectProtocol};
use objc2_video::{AnyThread, DefinedClass, MainThreadMarker, MainThreadOnly, class, define_class, extern_class, extern_protocol, msg_send};

use super::doc::{Document, Effect, Event, Snapshot};
use super::geometry::Geometry;
use super::super::view::{RuneView, UIResponder, UIView};

extern_class!(
    #[unsafe(super(NSObject))]
    #[name = "UITextPosition"]
    struct UITextPosition;
);

extern_class!(
    #[unsafe(super(NSObject))]
    #[name = "UITextRange"]
    struct UITextRange;
);

extern_protocol!(
    /// The methods are declared where `RuneTextInput` implements them.
    #[name = "UITextInput"]
    unsafe trait TextInput: NSObjectProtocol {}
);

/// `NSOrderedAscending`, `NSOrderedSame` and `NSOrderedDescending`.
const ORDER: [isize; 3] = [-1, 0, 1];
/// `UITextLayoutDirectionLeft` and `UITextLayoutDirectionUp`: the two that move backward.
const DIRECTION_LEFT: isize = 3;
const DIRECTION_UP: isize = 4;
const DIRECTION_DOWN: isize = 5;
/// `NSWritingDirectionNatural`.
const WRITING_NATURAL: isize = -1;

define_class!(
    // SAFETY: `UITextPosition` is made to be subclassed, and `Position` has no `Drop`.
    #[unsafe(super(UITextPosition, NSObject))]
    #[name = "RuneTextPosition"]
    #[ivars = usize]
    struct Position;
);

impl Position {
    /// UTF-16 offset `at` of the document.
    fn new(at: usize) -> Retained<Self> {
        let this = Self::alloc().set_ivars(at);
        // SAFETY: `init` is `UITextPosition`'s initializer.
        unsafe { msg_send![super(this), init] }
    }

    fn at(&self) -> usize { *self.ivars() }
}

define_class!(
    // SAFETY: `UITextRange` is made to be subclassed with these three overridden, and `Span`
    // has no `Drop`.
    #[unsafe(super(UITextRange, NSObject))]
    #[name = "RuneTextRange"]
    #[ivars = (usize, usize)]
    struct Span;

    impl Span {
        #[unsafe(method_id(start))]
        fn start(&self) -> Retained<Position> {
            Position::new(self.ivars().0)
        }

        #[unsafe(method_id(end))]
        fn end(&self) -> Retained<Position> {
            Position::new(self.ivars().1)
        }

        #[unsafe(method(isEmpty))]
        fn is_empty(&self) -> bool {
            self.ivars().0 == self.ivars().1
        }
    }
);

impl Span {
    /// UTF-16 range `range` of the document.
    fn new(range: std::ops::Range<usize>) -> Retained<Self> {
        let this = Self::alloc().set_ivars((range.start, range.end.max(range.start)));
        // SAFETY: `init` is `UITextRange`'s initializer.
        unsafe { msg_send![super(this), init] }
    }

    fn range(&self) -> std::ops::Range<usize> { self.ivars().0..self.ivars().1 }
}

pub(crate) struct Ivars {
    /// The `RuneView` this sits in, which carries the keyboard's edits to the runtime.
    host: Weak<RuneView>,
    doc: RefCell<Document>,
    /// The caret's rect in the host view's points.
    caret: Cell<CGRect>,
    /// The focused field is a password.
    secure: Cell<bool>,
    /// UIKit's `inputDelegate`, which hears that the text or selection changed under it.
    delegate: RefCell<Option<Weak<AnyObject>>>,
    tokenizer: RefCell<Option<Retained<AnyObject>>>,
}

define_class!(
    // SAFETY: `UIView` is made to be subclassed, and `RuneTextInput` has no `Drop`.
    #[unsafe(super(UIView, UIResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "RuneTextInput"]
    #[ivars = Ivars]
    pub(crate) struct RuneTextInput;

    unsafe impl NSObjectProtocol for RuneTextInput {}

    // SAFETY: the protocol's required methods are the ones below.
    unsafe impl TextInput for RuneTextInput {}

    impl RuneTextInput {
        #[unsafe(method(canBecomeFirstResponder))]
        fn can_become_first_responder(&self) -> bool {
            true
        }

        // The standard edit actions, which UIKit sends a first responder for the three-finger
        // copy, cut and paste gestures and from any system menu. The field does each itself,
        // as for the hardware keyboard's shortcut, with Rune's clipboard.
        #[unsafe(method(selectAll:))]
        fn select_all(&self, _sender: Option<&AnyObject>) { self.command('a'); }

        #[unsafe(method(copy:))]
        fn copy(&self, _sender: Option<&AnyObject>) { self.command('c'); }

        #[unsafe(method(cut:))]
        fn cut(&self, _sender: Option<&AnyObject>) { self.command('x'); }

        #[unsafe(method(paste:))]
        fn paste(&self, _sender: Option<&AnyObject>) { self.command('v'); }

        /// A hardware key: the host's when it names an edit, a move or a shortcut, and UIKit's
        /// when it types a character or the keyboard is composing.
        #[unsafe(method(pressesBegan:withEvent:))]
        fn presses_began(&self, presses: &AnyObject, event: Option<&AnyObject>) {
            let composing = self.ivars().doc.borrow().marked().is_some();
            let taken = !composing && self.ivars().host.load().is_some_and(|host| host.presses(presses, true, true));
            if !taken {
                // SAFETY: UIKit's own handling, which types the key into this text input.
                let _: () = unsafe { msg_send![super(self), pressesBegan: presses, withEvent: event] };
            }
        }

        #[unsafe(method(pressesEnded:withEvent:))]
        fn presses_ended(&self, presses: &AnyObject, event: Option<&AnyObject>) {
            if let Some(host) = self.ivars().host.load() { host.presses(presses, false, true); }
            // SAFETY: as in `pressesBegan:withEvent:`.
            let _: () = unsafe { msg_send![super(self), pressesEnded: presses, withEvent: event] };
        }

        #[unsafe(method(pressesCancelled:withEvent:))]
        fn presses_cancelled(&self, presses: &AnyObject, event: Option<&AnyObject>) {
            if let Some(host) = self.ivars().host.load() { host.presses(presses, false, true); }
            // SAFETY: as in `pressesBegan:withEvent:`.
            let _: () = unsafe { msg_send![super(self), pressesCancelled: presses, withEvent: event] };
        }

        #[unsafe(method(isSecureTextEntry))]
        fn is_secure(&self) -> bool {
            self.ivars().secure.get()
        }

        #[unsafe(method(hasText))]
        fn has_text(&self) -> bool {
            self.ivars().doc.borrow().len() > 0
        }

        #[unsafe(method(insertText:))]
        fn insert_text(&self, text: &NSString) {
            self.edit(Event::Insert(text.to_string()));
        }

        #[unsafe(method(deleteBackward))]
        fn delete_backward(&self) {
            self.edit(Event::DeleteBackward);
        }

        #[unsafe(method_id(textInRange:))]
        fn text_in_range(&self, range: &Span) -> Option<Retained<NSString>> {
            Some(NSString::from_str(&self.ivars().doc.borrow().slice(range.range())))
        }

        #[unsafe(method(replaceRange:withText:))]
        fn replace_range(&self, range: &Span, text: &NSString) {
            self.edit(Event::Replace { range: range.range(), text: text.to_string() });
        }

        #[unsafe(method_id(selectedTextRange))]
        fn selected_text_range(&self) -> Option<Retained<Span>> {
            Some(Span::new(self.ivars().doc.borrow().selection()))
        }

        #[unsafe(method(setSelectedTextRange:))]
        fn set_selected_text_range(&self, range: Option<&Span>) {
            if let Some(range) = range { self.edit(Event::Select(range.range())); }
        }

        #[unsafe(method_id(markedTextRange))]
        fn marked_text_range(&self) -> Option<Retained<Span>> {
            self.ivars().doc.borrow().marked().map(Span::new)
        }

        #[unsafe(method_id(markedTextStyle))]
        fn marked_text_style(&self) -> Option<Retained<NSDictionary>> {
            None
        }

        /// Rune draws the composition its own way.
        #[unsafe(method(setMarkedTextStyle:))]
        fn set_marked_text_style(&self, _style: Option<&NSDictionary>) {}

        #[unsafe(method(setMarkedText:selectedRange:))]
        fn set_marked_text(&self, text: Option<&NSString>, selected: NSRange) {
            let text = text.map(NSString::to_string).unwrap_or_default();
            self.edit(Event::SetMarked { text, selection: selected.location..selected.location + selected.length });
        }

        #[unsafe(method(unmarkText))]
        fn unmark_text(&self) {
            self.edit(Event::Unmark);
        }

        #[unsafe(method_id(beginningOfDocument))]
        fn beginning_of_document(&self) -> Retained<Position> {
            Position::new(0)
        }

        #[unsafe(method_id(endOfDocument))]
        fn end_of_document(&self) -> Retained<Position> {
            Position::new(self.ivars().doc.borrow().len())
        }

        #[unsafe(method_id(textRangeFromPosition:toPosition:))]
        fn text_range(&self, from: &Position, to: &Position) -> Option<Retained<Span>> {
            Some(Span::new(from.at().min(to.at())..from.at().max(to.at())))
        }

        #[unsafe(method_id(positionFromPosition:offset:))]
        fn position_offset(&self, position: &Position, offset: isize) -> Option<Retained<Position>> {
            self.moved(position.at(), offset)
        }

        #[unsafe(method_id(positionFromPosition:inDirection:offset:))]
        fn position_in_direction(&self, position: &Position, direction: isize, offset: isize) -> Option<Retained<Position>> {
            let signed = if backward(direction) { -offset } else { offset };
            // A line up or down where the field's lines are known, else by characters.
            let line = (direction == DIRECTION_UP || direction == DIRECTION_DOWN)
                .then(|| self.place(position.at())).flatten()
                .and_then(|(geometry, byte)| geometry.vertical(byte, signed));
            match line {
                Some(byte) => Some(Position::new(self.ivars().doc.borrow().offset_of(byte))),
                None => self.moved(position.at(), signed),
            }
        }

        #[unsafe(method(comparePosition:toPosition:))]
        fn compare(&self, a: &Position, b: &Position) -> isize {
            ORDER[a.at().cmp(&b.at()) as usize + 1]
        }

        #[unsafe(method(offsetFromPosition:toPosition:))]
        fn offset(&self, from: &Position, to: &Position) -> isize {
            to.at() as isize - from.at() as isize
        }

        #[unsafe(method_id(inputDelegate))]
        fn input_delegate(&self) -> Option<Retained<AnyObject>> {
            self.delegate()
        }

        #[unsafe(method(setInputDelegate:))]
        fn set_input_delegate(&self, delegate: Option<&AnyObject>) {
            *self.ivars().delegate.borrow_mut() = delegate.map(Weak::new);
        }

        #[unsafe(method_id(tokenizer))]
        fn tokenizer(&self) -> Retained<AnyObject> {
            self.ivars().tokenizer.borrow_mut().get_or_insert_with(|| {
                // SAFETY: UIKit's own tokenizer, made for a view that conforms to `UITextInput`.
                unsafe {
                    let tokenizer: Allocated<AnyObject> = msg_send![class!(UITextInputStringTokenizer), alloc];
                    msg_send![tokenizer, initWithTextInput: self]
                }
            }).clone()
        }

        #[unsafe(method_id(positionWithinRange:farthestInDirection:))]
        fn farthest(&self, range: &Span, direction: isize) -> Option<Retained<Position>> {
            Some(Position::new(if backward(direction) { range.range().start } else { range.range().end }))
        }

        #[unsafe(method_id(characterRangeByExtendingPosition:inDirection:))]
        fn extending(&self, position: &Position, direction: isize) -> Option<Retained<Span>> {
            let end = if backward(direction) { 0 } else { self.ivars().doc.borrow().len() };
            Some(Span::new(position.at().min(end)..position.at().max(end)))
        }

        #[unsafe(method(baseWritingDirectionForPosition:inDirection:))]
        fn base_writing_direction(&self, _position: &Position, _direction: isize) -> isize {
            WRITING_NATURAL
        }

        /// The field takes its direction from its text.
        #[unsafe(method(setBaseWritingDirection:forRange:))]
        fn set_base_writing_direction(&self, _direction: isize, _range: &Span) {}

        #[unsafe(method(firstRectForRange:))]
        fn first_rect(&self, range: &Span) -> CGRect {
            let range = range.range();
            let rect = self.place(range.start).zip(self.ivars().doc.borrow().field_byte(range.end))
                .and_then(|((geometry, from), to)| geometry.first_rect(from..to));
            rect.map_or(self.ivars().caret.get(), cg_rect)
        }

        #[unsafe(method(caretRectForPosition:))]
        fn caret_rect(&self, position: &Position) -> CGRect {
            let rect = self.place(position.at()).and_then(|(geometry, byte)| geometry.caret(byte));
            rect.map_or(self.ivars().caret.get(), cg_rect)
        }

        #[unsafe(method_id(selectionRectsForRange:))]
        fn selection_rects(&self, _range: &Span) -> Retained<NSArray> {
            NSArray::new()
        }

        #[unsafe(method_id(closestPositionToPoint:))]
        fn closest_position(&self, point: CGPoint) -> Option<Retained<Position>> {
            Some(Position::new(self.closest(point)))
        }

        #[unsafe(method_id(closestPositionToPoint:withinRange:))]
        fn closest_position_within(&self, point: CGPoint, range: &Span) -> Option<Retained<Position>> {
            Some(Position::new(self.closest(point).clamp(range.range().start, range.range().end)))
        }

        #[unsafe(method_id(characterRangeAtPoint:))]
        fn character_range_at(&self, _point: CGPoint) -> Option<Retained<Span>> {
            None
        }
    }
);

/// A rect of the view's points as UIKit's.
fn cg_rect([x, y, w, h]: [f32; 4]) -> CGRect {
    CGRect::new(CGPoint::new(x as CGFloat, y as CGFloat), CGSize::new(w as CGFloat, h as CGFloat))
}

/// Whether a `UITextLayoutDirection` moves toward the start of the text.
fn backward(direction: isize) -> bool {
    direction == DIRECTION_LEFT || direction == DIRECTION_UP
}

impl RuneTextInput {
    /// A hidden, empty view for `host`, which adds it to itself.
    pub(crate) fn new(mtm: MainThreadMarker, host: &RuneView) -> Retained<Self> {
        let zero = CGRect::new(CGPoint::new(0.0, 0.0), CGSize::new(0.0, 0.0));
        let this = Self::alloc(mtm).set_ivars(Ivars {
            host: Weak::new(host),
            doc: RefCell::new(Document::default()),
            caret: Cell::new(zero),
            secure: Cell::new(false),
            delegate: RefCell::new(None),
            tokenizer: RefCell::new(None),
        });
        // SAFETY: `initWithFrame:` is `UIView`'s designated initializer.
        unsafe { msg_send![super(this), initWithFrame: zero] }
    }

    /// The field's geometry and the byte of its text at UTF-16 offset `at` of the document,
    /// where the document is the field's text and the field has drawn.
    fn place(&self, at: usize) -> Option<(Geometry, usize)> {
        let byte = self.ivars().doc.borrow().field_byte(at)?;
        Some((self.ivars().host.load()?.text_geometry()?, byte))
    }

    /// The document offset nearest `point`: by the field's lines, or the caret's where they
    /// are not known.
    fn closest(&self, point: CGPoint) -> usize {
        let doc = self.ivars().doc.borrow();
        let caret = doc.selection().end;
        let found = self.place(caret).and_then(|(geometry, _)| geometry.closest(point.x as f32, point.y as f32));
        found.map_or(caret, |byte| doc.offset_of(byte))
    }

    /// The keyboard's side of the conversation, while it lives.
    fn delegate(&self) -> Option<Retained<AnyObject>> {
        self.ivars().delegate.borrow().as_ref().and_then(Weak::load)
    }

    /// The position `offset` UTF-16 units from `at`, or none past either end of the document.
    fn moved(&self, at: usize, offset: isize) -> Option<Retained<Position>> {
        let to = at as isize + offset;
        (0..=self.ivars().doc.borrow().len() as isize).contains(&to).then(|| Position::new(to as usize))
    }

    /// Sends the field the shortcut Command plus `c`.
    fn command(&self, c: char) {
        if let Some(host) = self.ivars().host.load() { host.command(c); }
    }

    /// An edit the keyboard made: the copy takes it now, and the runtime hears of it.
    fn edit(&self, event: Event) {
        let effects = self.ivars().doc.borrow_mut().step(event);
        if let Some(host) = self.ivars().host.load() { host.text_edits(effects); }
    }

    /// After a frame: whether a field wants the keyboard, what it holds, where its caret is in
    /// the host's points and whether it is a password. Takes or gives up the keyboard, and
    /// tells it when the runtime's text is not what it believed.
    pub(crate) fn sync(&self, wanted: bool, snapshot: Snapshot, caret: Option<[f32; 4]>, secure: bool) {
        let ivars = self.ivars();
        if let Some([x, y, w, h]) = caret {
            ivars.caret.set(CGRect::new(CGPoint::new(x as CGFloat, y as CGFloat), CGSize::new(w as CGFloat, h as CGFloat)));
        }
        let was_secure = ivars.secure.replace(secure);
        let effects = if wanted { ivars.doc.borrow_mut().step(Event::Runtime(snapshot)) } else { Default::default() };
        let delegate = self.delegate();
        for effect in effects {
            let Some(delegate) = &delegate else { break };
            // SAFETY: `UITextInputDelegate`'s methods, each taking the text input.
            unsafe {
                match effect {
                    Effect::TextChanged => {
                        let _: () = msg_send![&**delegate, textWillChange: self];
                        let _: () = msg_send![&**delegate, textDidChange: self];
                    }
                    // UIKit answers news of a selected range by collapsing it to its end (it
                    // draws no selection here to keep), so it hears only of a caret that moved
                    // and reads a range when it next needs one.
                    Effect::SelectionChanged if ivars.doc.borrow().selection().is_empty() => {
                        let _: () = msg_send![&**delegate, selectionWillChange: self];
                        let _: () = msg_send![&**delegate, selectionDidChange: self];
                    }
                    Effect::SelectionChanged => {}
                    Effect::Key(_) | Effect::Commit(_) | Effect::Preedit(..) | Effect::Select(_) => {}
                }
            }
        }
        // SAFETY: `UIResponder` methods of these signatures.
        unsafe {
            let first: bool = msg_send![self, isFirstResponder];
            if wanted && !first {
                let _: bool = msg_send![self, becomeFirstResponder];
            } else if !wanted && first {
                // The host takes the hardware keyboard back, which also puts the soft one away.
                match ivars.host.load() {
                    Some(host) => { let _: bool = msg_send![&*host, becomeFirstResponder]; }
                    None => { let _: bool = msg_send![self, resignFirstResponder]; }
                }
            } else if first && was_secure != secure {
                // The keyboard reads the field's traits when it attaches.
                let _: () = msg_send![self, reloadInputViews];
            }
        }
    }
}
