import SwiftUI
import UIKit

/// cedar's look: a neutral page with white cards (light) or a green-black one (dark), one green
/// family (dark green on the page, lime in the dark) and amber for what needs the user. The same
/// palette as the web's Today page. The mark keeps the brand's own green and clay.
enum Theme {
    /// The screen behind everything.
    static let canvas = dynamic(light: 0xF7F7F4, dark: 0x0F1411)
    /// Cards, bubbles and the composer's field.
    static let surface = dynamic(light: 0xFFFFFF, dark: 0x171E1A)
    /// A step up from `surface`: code, chips, the sidebar switch.
    static let raised = dynamic(light: 0xEEF0EB, dark: 0x222B25)
    /// Hairlines around cards and between rows.
    static let line = dynamic(light: 0xE2E4DE, dark: 0x26302A)
    /// The accent: dark green on the page, lime in the dark. The asset catalog's accent color,
    /// read directly so a `.tint` doesn't change it.
    static let green = Color("AccentColor")
    /// What needs the user (answers, approvals) and unread: amber, as text and tints. Never red,
    /// which is for what failed.
    static let attention = dynamic(light: 0x6B4A00, dark: 0xF2C14E)
    /// Amber as a fill, with `onAttention` on it.
    static let attentionFill = dynamic(light: 0xF2C14E, dark: 0xF2C14E)
    static let onAttention = dynamic(light: 0x1F1600, dark: 0x1F1600)
    /// Text on the accent: white on the dark green, dark ink on the lime.
    static let onGreen = dynamic(light: 0xFFFFFF, dark: 0x0F1411)
    /// A primary action's fill and the text on it: the dark green with lime, or lime with dark ink.
    static let primaryFill = dynamic(light: 0x0F1F17, dark: 0xC8F169)
    static let primaryInk = dynamic(light: 0xC8F169, dark: 0x0F1411)
    /// The mark's bars, and its dash: the brand's own colours, as in docs/assets.
    static let markInk = dynamic(light: 0x3A5F4B, dark: 0xEDEFE8)
    static let markDash = dynamic(light: 0xB5553A, dark: 0xE09A7A)

    static let cardRadius: CGFloat = 16

    private static func dynamic(light: UInt32, dark: UInt32) -> Color {
        Color(UIColor { traits in
            UIColor(rgb: traits.userInterfaceStyle == .dark ? dark : light)
        })
    }
}

private extension UIColor {
    convenience init(rgb: UInt32) {
        self.init(red: CGFloat((rgb >> 16) & 0xFF) / 255,
                  green: CGFloat((rgb >> 8) & 0xFF) / 255,
                  blue: CGFloat(rgb & 0xFF) / 255,
                  alpha: 1)
    }
}

extension View {
    /// A card: surface fill, a hairline and a soft shadow.
    func surfaceCard(radius: CGFloat = Theme.cardRadius, fill: Color = Theme.surface, stroke: Color = Theme.line) -> some View {
        background(fill, in: RoundedRectangle(cornerRadius: radius, style: .continuous))
            .overlay(RoundedRectangle(cornerRadius: radius, style: .continuous).strokeBorder(stroke, lineWidth: 1))
            .shadow(color: .black.opacity(0.04), radius: 8, y: 2)
    }
}

/// cedar's mark: a "c" of four bars with the clay dash, drawn from docs/assets/cedar-mark.svg.
struct CedarMark: View {
    var size: CGFloat = 28

    var body: some View {
        let s = size / 48
        ZStack(alignment: .topLeading) {
            bar(x: 6, y: 6, width: 36, Theme.markInk, s)
            bar(x: 6, y: 16, width: 12, Theme.markInk, s)
            bar(x: 6, y: 26, width: 12, Theme.markInk, s)
            bar(x: 6, y: 36, width: 36, Theme.markInk, s)
            bar(x: 33, y: 21, width: 9, Theme.markDash, s)
        }
        .frame(width: size, height: size, alignment: .topLeading)
        .accessibilityHidden(true)
    }

    private func bar(x: CGFloat, y: CGFloat, width: CGFloat, _ color: Color, _ s: CGFloat) -> some View {
        RoundedRectangle(cornerRadius: 2 * s, style: .continuous)
            .fill(color)
            .frame(width: width * s, height: 6 * s)
            .offset(x: x * s, y: y * s)
    }
}

/// The mark and the name, as the navigation bar shows them.
struct CedarWordmark: View {
    var size: CGFloat = 22

    var body: some View {
        HStack(spacing: size * 0.3) {
            CedarMark(size: size * 1.15)
            Text(verbatim: "cedar")
                .font(.system(size: size, weight: .semibold))
                .kerning(-0.4)
                .foregroundStyle(Theme.markInk)
        }
        .accessibilityElement(children: .ignore)
        .accessibilityLabel(Text(verbatim: "cedar"))
        .accessibilityAddTraits(.isHeader)
    }
}

/// The mark on a soft tile, for welcomes and empty screens.
struct CedarBadge: View {
    var size: CGFloat = 72

    var body: some View {
        CedarMark(size: size * 0.62)
            .frame(width: size, height: size)
            .background(
                RoundedRectangle(cornerRadius: size * 0.28, style: .continuous)
                    .fill(Theme.surface)
                    .shadow(color: .black.opacity(0.08), radius: 14, y: 6)
            )
            .overlay(RoundedRectangle(cornerRadius: size * 0.28, style: .continuous).strokeBorder(Theme.line))
    }
}

/// A primary action, full width: the dark green with lime on the page, lime in the dark.
struct ProminentButtonStyle: ButtonStyle {
    @Environment(\.isEnabled) private var isEnabled

    func makeBody(configuration: Configuration) -> some View {
        configuration.label
            .font(.headline)
            .foregroundStyle(Theme.primaryInk)
            .frame(maxWidth: .infinity)
            .padding(.vertical, 15)
            .background(Theme.primaryFill, in: RoundedRectangle(cornerRadius: 14, style: .continuous))
            .opacity(isEnabled ? (configuration.isPressed ? 0.85 : 1) : 0.4)
            .scaleEffect(configuration.isPressed ? 0.98 : 1)
            .animation(.snappy(duration: 0.15), value: configuration.isPressed)
    }
}
