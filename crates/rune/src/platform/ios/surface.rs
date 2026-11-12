//! What a `RuneView` draws: one app's runtime and renderer on the view's `CAMetalLayer`, with the
//! app's state type erased so one Objective-C class serves every app.

use std::ffi::c_void;
use std::sync::Arc;

use crate::app::{App, Shared};
use crate::embed::{EmbedFrame, LayerHost};
use crate::runtime::{TouchPhase, Waker};

pub(crate) trait Surface {
    /// The view's size in points and its display scale.
    fn resize(&mut self, size: (f32, f32), scale: f32);
    /// Steps the app to now and presents a frame.
    fn frame(&mut self) -> EmbedFrame;
    /// One finger's event at (x, y) in the view's points, `time` in seconds since boot.
    fn touch(&mut self, id: u64, phase: TouchPhase, x: f32, y: f32, time: f64);
    /// The system's appearance.
    fn set_dark(&mut self, dark: bool);
    /// What the screen keeps at the view's top, right, bottom and left edges, in points.
    fn set_safe_area(&mut self, insets: [f32; 4]);
}

struct Hosted<S: 'static> {
    host: LayerHost<S>,
    /// What the app's handles reach it through, kept for as long as the app draws.
    _shared: Arc<Shared<S>>,
}

impl<S: 'static> Surface for Hosted<S> {
    fn resize(&mut self, size: (f32, f32), scale: f32) { self.host.resize(size, scale); }

    fn frame(&mut self) -> EmbedFrame { self.host.frame() }

    fn touch(&mut self, id: u64, phase: TouchPhase, x: f32, y: f32, time: f64) {
        self.host.runtime_mut().touch(id, phase, x, y, time);
    }

    fn set_dark(&mut self, dark: bool) { self.host.runtime_mut().set_system_dark(dark); }

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
        let host = unsafe { LayerHost::from_core_animation_layer(layer, &spec, state, size, scale, dark, wake)? };
        Ok(Box::new(Hosted { host, _shared: shared }))
    })
}
