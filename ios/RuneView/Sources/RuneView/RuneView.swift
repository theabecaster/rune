import SwiftUI
import UIKit

/// A Rune app drawn inside a SwiftUI hierarchy.
///
/// `make` is a function the app's Rust library exports that returns `rune::ios::view(app)`: a
/// retained `UIView *`, or null off the main thread. The view fills the space SwiftUI gives it,
/// draws only while something moves and stops in the background.
///
/// ```swift
/// RuneView(my_screen)
/// ```
public struct RuneView: UIViewRepresentable {
    private let make: () -> UnsafeMutableRawPointer?

    public init(_ make: @escaping () -> UnsafeMutableRawPointer?) {
        self.make = make
    }

    public func makeUIView(context: Context) -> UIView {
        guard let view = make() else { return UIView() }
        return Unmanaged<UIView>.fromOpaque(view).takeRetainedValue()
    }

    public func updateUIView(_ view: UIView, context: Context) {}
}
