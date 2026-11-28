import SwiftUI
import UIKit

/// Today's own colours ("Now in the dark"): a neutral page where Earlier and Ahead sit quietly,
/// and the Now section as one dark-green block with lime, the way the dark theme looks. What
/// needs the person is amber, never the mark's clay, which reads as a warning next to green.
/// In dark mode the app's dark surfaces stay, with the same lime and amber.
enum TodayColors {
    // MARK: On the page (light), or the app's dark surfaces

    /// The ground behind an outcome.
    static let page = dynamic(light: 0xF7F7F4, dark: 0x111311)
    /// What needs the person, as text on `needSoft`.
    static let needText = dynamic(light: 0x6B4A00, dark: 0xF2C14E)
    static let needSoft = dynamic(light: 0xFFF1CC, dark: 0x3A2F10)
    /// A primary action on the page: the Now block's green with lime on it; lime itself in the dark.
    static let primaryFill = dynamic(light: 0x0F1F17, dark: 0xC8F169)
    static let primaryInk = dynamic(light: 0xC8F169, dark: 0x0F1F17)

    // MARK: In the Now block, which is dark in both modes

    /// The Now label, its line and the primary action.
    static let lime = fixed(0xC8F169)
    static let limeInk = fixed(0x0F1F17)
    /// What needs the person: a card's border, a button's fill, a chip's text.
    static let amber = fixed(0xF2C14E)
    static let amberInk = fixed(0x1F1600)
    static let amberSoft = fixed(0x3A2F10)
    /// The "Working" chip's ground.
    static let workingSoft = fixed(0x22361A)

    /// The block's own surfaces. In light mode it is the dark-green block; in dark mode the Now
    /// rows sit on the app's dark surfaces, with no block of their own.
    struct Now {
        let block: Color
        let card: Color
        let line: Color
    }

    static func now(_ scheme: ColorScheme) -> Now {
        scheme == .dark
            ? Now(block: fixed(0x111311), card: fixed(0x1B1E1B), line: fixed(0x2E332E))
            : Now(block: fixed(0x0F1F17), card: fixed(0x17291F), line: fixed(0x2E4638))
    }

    /// How round the block's outer corners are.
    static let blockRadius: CGFloat = 24

    private static func fixed(_ rgb: UInt32) -> Color {
        Color(uiColor(rgb))
    }

    private static func dynamic(light: UInt32, dark: UInt32) -> Color {
        Color(UIColor { traits in
            uiColor(traits.userInterfaceStyle == .dark ? dark : light)
        })
    }

    private static func uiColor(_ rgb: UInt32) -> UIColor {
        UIColor(red: CGFloat((rgb >> 16) & 0xFF) / 255,
                green: CGFloat((rgb >> 8) & 0xFF) / 255,
                blue: CGFloat(rgb & 0xFF) / 255,
                alpha: 1)
    }
}

/// Where a row sits in the Now block: its first row has the block's top corners, its last the
/// bottom ones.
enum TodayNowEdge {
    case top, middle, bottom
}

extension View {
    /// A row of the Now block: the block's fill behind it (rounded at the block's ends), no
    /// separators, and the dark scheme for what it holds, so text and the shared cards (a held
    /// call's) read on it. The fill is given outside that scheme: it is the palette's own.
    func todayNowRow(_ edge: TodayNowEdge, palette: TodayColors.Now) -> some View {
        let top: CGFloat = edge == .top ? TodayColors.blockRadius : 0
        let bottom: CGFloat = edge == .bottom ? TodayColors.blockRadius : 0
        return tint(TodayColors.lime)
            .environment(\.colorScheme, .dark)
            .listRowSeparator(.hidden)
            .listRowBackground(
                UnevenRoundedRectangle(topLeadingRadius: top, bottomLeadingRadius: bottom,
                                       bottomTrailingRadius: bottom, topTrailingRadius: top, style: .continuous)
                    .fill(palette.block)
                    .padding(.horizontal, 8)
                    .background(Theme.canvas)
            )
    }
}

/// Today's primary action, full width: lime with dark ink in the Now block (`onDark`), and the
/// block's green with lime on it on the page.
struct TodayPrimaryButtonStyle: ButtonStyle {
    var onDark = false
    @Environment(\.isEnabled) private var isEnabled

    func makeBody(configuration: Configuration) -> some View {
        configuration.label
            .font(.headline)
            .foregroundStyle(onDark ? TodayColors.limeInk : TodayColors.primaryInk)
            .frame(maxWidth: .infinity)
            .padding(.vertical, 15)
            .background(onDark ? TodayColors.lime : TodayColors.primaryFill,
                        in: RoundedRectangle(cornerRadius: 14, style: .continuous))
            .opacity(isEnabled ? (configuration.isPressed ? 0.85 : 1) : 0.4)
            .scaleEffect(configuration.isPressed ? 0.98 : 1)
            .animation(.snappy(duration: 0.15), value: configuration.isPressed)
    }
}
