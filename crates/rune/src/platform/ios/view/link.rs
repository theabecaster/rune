//! A `RuneView`'s display link, and the wakes that reach the view from timers and other
//! threads. The link is made paused on the main run loop, runs while the pacing has frames to
//! draw, and names the time each frame will be on screen. A wake from another thread, and a
//! timer's answer, come back to the view on the main queue through a weak reference.

use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use objc2_video::rc::{Retained, Weak};
use objc2_video::runtime::AnyObject;
use objc2_video::{DefinedClass, Encode, Encoding, class, msg_send, sel};

use super::super::keys;
use super::super::pacing::Event;
use super::RuneView;
use crate::machine::Attempt;
use crate::platform::main_queue::on_main;
use crate::runtime::Waker;
use crate::time::Instant;

#[link(name = "QuartzCore", kind = "framework")]
unsafe extern "C" {
    /// Seconds on the clock a display link's timestamps are on.
    fn CACurrentMediaTime() -> f64;
}

#[link(name = "Foundation", kind = "framework")]
unsafe extern "C" {
    static NSRunLoopCommonModes: &'static AnyObject;
}

/// The frame rates a running link asks for. A ProMotion display gives 120; a 60 Hz display
/// gives 60, the most it has. An iPhone gives more than 60 only to an app whose `Info.plist`
/// sets `CADisableMinimumFrameDurationOnPhone`.
#[repr(C)]
#[derive(Clone, Copy)]
struct FrameRateRange {
    minimum: f32,
    maximum: f32,
    preferred: f32,
}

// SAFETY: `CAFrameRateRange` is three floats in this order.
unsafe impl Encode for FrameRateRange {
    const ENCODING: Encoding = Encoding::Struct("CAFrameRateRange", &[f32::ENCODING, f32::ENCODING, f32::ENCODING]);
}

const FRAME_RATES: FrameRateRange = FrameRateRange { minimum: 80.0, maximum: 120.0, preferred: 120.0 };

/// A weak view that crosses threads but is only read on the main one.
pub(super) struct MainOnly(pub(super) Weak<RuneView>);

// SAFETY: the weak reference is copied and dropped from any thread, which the Objective-C
// runtime allows, and loaded only inside `on_main` and `after_main` jobs, on the main thread.
unsafe impl Send for MainOnly {}
// SAFETY: as above.
unsafe impl Sync for MainOnly {}

impl MainOnly {
    /// Feeds `event` to the view's pacing, if the view is still alive. Main thread only.
    pub(super) fn pace(&self, event: Event) {
        if let Some(view) = self.0.load() { view.pace(event); }
    }

    /// Tells the view a dismissed menu's grace ran out.
    pub(super) fn menu_left(&self, attempt: Attempt) {
        if let Some(view) = self.0.load() { view.menu_left(attempt); }
    }

    /// Feeds `event` to the view's key repeat, as `pace` does its pacing.
    pub(super) fn keys(&self, event: keys::Event) {
        if let Some(view) = self.0.load() { view.keys(event); }
    }
}

impl RuneView {
    /// What other threads call when the app has something new to draw.
    pub(super) fn waker(&self) -> Waker {
        let view = Arc::new(MainOnly(Weak::new(self)));
        let queued = self.ivars().wake_queued.clone();
        Arc::new(move || {
            if queued.swap(true, Ordering::AcqRel) { return; }
            let (view, queued) = (view.clone(), queued.clone());
            on_main(move || {
                queued.store(false, Ordering::Release);
                view.pace(Event::Wake);
            });
        })
    }

    /// A paused display link on the main run loop, in the modes that also run while a finger
    /// tracks. The link retains the view until `destroy_link` invalidates it.
    pub(super) fn create_link(&self) {
        // SAFETY: `runeTick:` is a method of this class that takes the link; a display link's
        // `paused` is a `BOOL`, its frame rate range a `CAFrameRateRange`.
        let link = unsafe {
            let link: Retained<AnyObject> = msg_send![class!(CADisplayLink), displayLinkWithTarget: self, selector: sel!(runeTick:)];
            let _: () = msg_send![&*link, setPaused: true];
            let _: () = msg_send![&*link, setPreferredFrameRateRange: FRAME_RATES];
            let run_loop: Retained<AnyObject> = msg_send![class!(NSRunLoop), mainRunLoop];
            let _: () = msg_send![&*link, addToRunLoop: &*run_loop, forMode: NSRunLoopCommonModes];
            link
        };
        *self.ivars().link.borrow_mut() = Some(link);
    }

    /// Ends the display link, if there is one.
    pub(super) fn destroy_link(&self) {
        if let Some(link) = self.ivars().link.borrow_mut().take() {
            // SAFETY: `invalidate` takes the link off its run loop and releases the view.
            let _: () = unsafe { msg_send![&*link, invalidate] };
        }
    }

    pub(super) fn set_paused(&self, paused: bool) {
        if let Some(link) = &*self.ivars().link.borrow() {
            // SAFETY: a display link's `paused` is a `BOOL`.
            let _: () = unsafe { msg_send![&**link, setPaused: paused] };
        }
    }

    /// When the frame the display link's next refresh shows will be on screen, or none without
    /// a link.
    pub(super) fn frame_target(&self) -> Option<Instant> {
        let shown = self.ivars().link.borrow().as_ref().map(|link| {
            // SAFETY: a display link's `targetTimestamp` is a `CFTimeInterval`.
            let shown: f64 = unsafe { msg_send![&**link, targetTimestamp] };
            shown
        })?;
        // SAFETY: a C function that reads a clock.
        let ahead = (shown - unsafe { CACurrentMediaTime() }).max(0.0);
        Some(Instant::now() + Duration::from_secs_f64(ahead))
    }
}
