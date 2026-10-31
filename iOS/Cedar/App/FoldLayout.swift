import SwiftUI

/// How the root split view takes the canvas it is given.
///
/// The same code serves an iPhone, an iPad, a window on either, and both displays of a book-style
/// foldable such as iPhone Duo: folded, its outer display is compact in width like any iPhone;
/// unfolded, its inner display is regular in both axes and nearly square, with the fold running
/// down the middle. An app can be moved between the two at any moment, so the layout reads size
/// classes and the container's size, never the device model, the screen or the orientation.
enum FoldLayout {
    /// A canvas at least this wide, in points, holds the conversation list and a conversation side by
    /// side. Below it the split view collapses to one column on its own.
    static let minimumSplitWidth: CGFloat = 600
    /// Width over height in this range is "nearly square": an unfolded book-style device, whose fold
    /// is the canvas's centre line. An iPad is 4:3 or longer in either orientation and stays out.
    static let squareAspect: ClosedRange<CGFloat> = 0.8...1.25

    /// Whether the split view should put its divider on the canvas's vertical centre line, where an
    /// unfolded device's fold is, so neither the list nor the conversation is cut by it.
    static func splitsAtCrease(_ size: CGSize) -> Bool {
        guard size.width >= minimumSplitWidth, size.height > 0 else { return false }
        return squareAspect.contains(size.width / size.height)
    }

    /// The sidebar's width for a canvas: half of it when splitting at the crease, otherwise `nil`
    /// for the system's own choice.
    static func sidebarWidth(for size: CGSize) -> CGFloat? {
        splitsAtCrease(size) ? (size.width / 2).rounded() : nil
    }

    /// How many lines the composer may grow to before it scrolls. A compact height (a phone or the
    /// folded outer display held sideways) keeps the conversation in view above it.
    static func composerLineLimit(verticalSizeClass: UserInterfaceSizeClass?) -> ClosedRange<Int> {
        verticalSizeClass == .compact ? 1...3 : 1...8
    }
}

extension View {
    /// Pins a split view column to `width` when one is given; leaves it to the system otherwise.
    func splitColumnWidth(_ width: CGFloat?) -> some View {
        modifier(SplitColumnWidth(width: width))
    }
}

private struct SplitColumnWidth: ViewModifier {
    let width: CGFloat?

    func body(content: Content) -> some View {
        Group {
            if let width {
                content.navigationSplitViewColumnWidth(min: width, ideal: width, max: width)
            } else {
                content
            }
        }
    }
}
