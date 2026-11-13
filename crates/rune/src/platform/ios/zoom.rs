//! A zoomed view's two kinds of points. A view that zooms (`Cx::zoom`) lays out in points the
//! zoom scales, as a desktop window's does: at zoom 2 the app's point is two of the `UIView`'s.
//! UIKit speaks the `UIView`'s points, the window's, so what it reports is divided by the zoom
//! on its way in and what it is told is multiplied by it on its way out. Plain arithmetic,
//! testable on every platform.

/// A point the `UIView` reports, the window's points, in the app's own. A scroll's delta and
/// a fling's velocity convert the same way.
pub(crate) fn from_window(zoom: f32, x: f32, y: f32) -> (f32, f32) {
    (x / zoom, y / zoom)
}

/// A rect of the app's (`[x, y, width, height]`) in the `UIView`'s points.
pub(crate) fn to_window(zoom: f32, rect: [f32; 4]) -> [f32; 4] {
    rect.map(|v| v * zoom)
}

#[cfg(test)]
#[path = "zoom_tests.rs"]
mod tests;
