import SwiftUI
import UIKit

/// cedar's look: the brand's forest green and clay on warm paper (light) or deep charcoal (dark),
/// the same palette as the mark in docs/assets.
enum Theme {
    /// The screen behind everything.
    static let canvas = dynamic(light: 0xF7F5F0, dark: 0x111311)
    /// Cards, bubbles and the composer's field.
    static let surface = dynamic(light: 0xFFFFFF, dark: 0x1B1E1B)
    /// A step up from `surface`: code, chips, the sidebar switch.
    static let raised = dynamic(light: 0xEFECE5, dark: 0x252925)
    /// Hairlines around cards and between rows.
    static let line = dynamic(light: 0xE3DFD6, dark: 0x2E332E)
    /// The brand green: the asset catalog's accent color, read directly so a `.tint` doesn't change it.
    static let green = Color("AccentColor")
    /// The mark's clay: what needs the user (answers, approvals) and unread.
    static let clay = dynamic(light: 0xB5553A, dark: 0xE09A7A)
    /// Text on the brand green.
    static let onGreen = Color.white
    /// The mark's bars.
    static let markInk = dynamic(light: 0x3A5F4B, dark: 0xF3EDE3)

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
            bar(x: 33, y: 21, width: 9, Theme.clay, s)
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

/// A primary action: the brand green, full width.
struct ProminentButtonStyle: ButtonStyle {
    @Environment(\.isEnabled) private var isEnabled

    func makeBody(configuration: Configuration) -> some View {
        configuration.label
            .font(.headline)
            .foregroundStyle(Theme.onGreen)
            .frame(maxWidth: .infinity)
            .padding(.vertical, 15)
            .background(Theme.green, in: RoundedRectangle(cornerRadius: 14, style: .continuous))
            .opacity(isEnabled ? (configuration.isPressed ? 0.85 : 1) : 0.4)
            .scaleEffect(configuration.isPressed ? 0.98 : 1)
            .animation(.snappy(duration: 0.15), value: configuration.isPressed)
    }
}
