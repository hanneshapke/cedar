import SwiftUI

/// The pictures the feature tour shows: small mock-ups of the app's own screens, drawn from the
/// theme's shapes and SF Symbols so they match the real thing in both appearances and need no
/// image assets. Each is laid out on a 320 × 250 stage and scaled to the room it gets.
enum TourIllustration: CaseIterable {
    case server, work, cards, plans, agents, memory, connect

    /// Whether the elements are shown (they fade and rise in, one after another, when a page
    /// comes up).
    @ViewBuilder
    func view(shown: Bool) -> some View {
        switch self {
        case .server: ServerPicture(shown: shown)
        case .work: WorkPicture(shown: shown)
        case .cards: CardsPicture(shown: shown)
        case .plans: PlansPicture(shown: shown)
        case .agents: AgentsPicture(shown: shown)
        case .memory: MemoryPicture(shown: shown)
        case .connect: ConnectPicture(shown: shown)
        }
    }
}

/// A fixed 320 × 250 drawing, scaled to fit the space it is given.
struct Stage<Content: View>: View {
    @ViewBuilder let content: () -> Content
    private let width: CGFloat = 320
    private let height: CGFloat = 250

    var body: some View {
        GeometryReader { geo in
            let scale = min(geo.size.width / width, geo.size.height / height)
            content()
                .frame(width: width, height: height)
                .scaleEffect(scale)
                .frame(width: geo.size.width, height: geo.size.height)
        }
        .accessibilityHidden(true)
    }
}

private struct Reveal: ViewModifier {
    let shown: Bool
    let index: Int
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    func body(content: Content) -> some View {
        content
            .opacity(shown ? 1 : 0)
            .offset(y: shown || reduceMotion ? 0 : 14)
            .animation(.spring(duration: 0.55, bounce: 0.2).delay(Double(index) * 0.1), value: shown)
    }
}

private extension View {
    /// Fades and rises in as the `index`th element of its picture.
    func reveal(_ shown: Bool, _ index: Int) -> some View {
        modifier(Reveal(shown: shown, index: index))
    }
}

// MARK: Parts

/// A small card on the stage: the surface, a hairline and a soft shadow, like the app's cards.
private struct MiniCard<Content: View>: View {
    var width: CGFloat? = nil
    var padding: CGFloat = 12
    @ViewBuilder let content: () -> Content

    var body: some View {
        content()
            .padding(padding)
            .frame(width: width, alignment: .leading)
            .surfaceCard(radius: 14)
    }
}

/// A line of text stood in for by a soft bar.
private struct TextLine: View {
    var width: CGFloat
    var tint: Color = Theme.raised

    var body: some View {
        Capsule()
            .fill(tint)
            .frame(width: width, height: 8)
    }
}

/// A chat bubble with real words.
private struct Bubble: View {
    let text: LocalizedStringKey
    var mine = false

    var body: some View {
        Text(text)
            .font(.system(size: 12, weight: .medium))
            .foregroundStyle(mine ? Theme.onGreen : Color.primary)
            .padding(.horizontal, 11)
            .padding(.vertical, 8)
            .background(mine ? Theme.green : Theme.surface, in: RoundedRectangle(cornerRadius: 14, style: .continuous))
            .overlay(RoundedRectangle(cornerRadius: 14, style: .continuous).strokeBorder(mine ? Color.clear : Theme.line))
    }
}

/// A tool call's row, as the chat shows it: the symbol, the name, the time it took and a check.
private struct ToolRow: View {
    let symbol: String
    let name: String
    let duration: String

    var body: some View {
        HStack(spacing: 7) {
            Image(systemName: symbol)
                .font(.system(size: 11, weight: .semibold))
                .foregroundStyle(Theme.green)
                .frame(width: 18, height: 18)
                .background(Theme.green.opacity(0.13), in: RoundedRectangle(cornerRadius: 5, style: .continuous))
            Text(verbatim: name)
                .font(.system(size: 11, weight: .medium, design: .monospaced))
            Spacer(minLength: 4)
            Text(verbatim: duration)
                .font(.system(size: 10))
                .foregroundStyle(.secondary)
            Image(systemName: "checkmark.circle.fill")
                .font(.system(size: 11))
                .foregroundStyle(Theme.green)
        }
    }
}

/// A rounded tile with a symbol on it, for the server page.
private struct Tile: View {
    let symbol: String
    var tint: Color = Theme.green
    var size: CGFloat = 56

    var body: some View {
        RoundedRectangle(cornerRadius: size * 0.28, style: .continuous)
            .fill(Theme.surface)
            .overlay(RoundedRectangle(cornerRadius: size * 0.28, style: .continuous).strokeBorder(Theme.line))
            .shadow(color: .black.opacity(0.06), radius: 10, y: 4)
            .frame(width: size, height: size)
            .overlay {
                Image(systemName: symbol)
                    .font(.system(size: size * 0.42, weight: .medium))
                    .foregroundStyle(tint)
            }
    }
}

/// A dashed link between two tiles.
private struct Wire: View {
    var body: some View {
        Line()
            .stroke(Theme.green.opacity(0.6), style: StrokeStyle(lineWidth: 2, lineCap: .round, dash: [1, 6]))
            .frame(height: 2)
    }

    private struct Line: Shape {
        func path(in rect: CGRect) -> Path {
            var path = Path()
            path.move(to: CGPoint(x: rect.minX, y: rect.midY))
            path.addLine(to: CGPoint(x: rect.maxX, y: rect.midY))
            return path
        }
    }
}

/// A caption under a tile.
private struct Caption: View {
    let text: LocalizedStringKey

    var body: some View {
        Text(text)
            .font(.system(size: 11, weight: .medium))
            .foregroundStyle(.secondary)
            .lineLimit(1)
    }
}

// MARK: Pictures

/// The phone, the server with cedar on it, and the sandbox, wired together; a padlock on the server.
private struct ServerPicture: View {
    let shown: Bool

    var body: some View {
        Stage {
            VStack(spacing: 26) {
                HStack(spacing: 0) {
                    VStack(spacing: 8) {
                        Tile(symbol: "iphone", tint: Theme.markInk)
                        Caption(text: "This app")
                    }
                    .reveal(shown, 0)
                    Wire().padding(.horizontal, 6).padding(.bottom, 22).reveal(shown, 1)
                    VStack(spacing: 8) {
                        ZStack(alignment: .topTrailing) {
                            RoundedRectangle(cornerRadius: 22, style: .continuous)
                                .fill(Theme.surface)
                                .overlay(RoundedRectangle(cornerRadius: 22, style: .continuous).strokeBorder(Theme.line))
                                .shadow(color: .black.opacity(0.08), radius: 14, y: 6)
                                .frame(width: 78, height: 78)
                                .overlay { CedarMark(size: 44) }
                            Image(systemName: "lock.fill")
                                .font(.system(size: 11, weight: .bold))
                                .foregroundStyle(Theme.onGreen)
                                .frame(width: 24, height: 24)
                                .background(Theme.green, in: Circle())
                                .offset(x: 6, y: -6)
                        }
                        Caption(text: "Your server")
                    }
                    .reveal(shown, 2)
                    Wire().padding(.horizontal, 6).padding(.bottom, 22).reveal(shown, 3)
                    VStack(spacing: 8) {
                        Tile(symbol: "terminal", tint: Theme.clay)
                        Caption(text: "Sandbox")
                    }
                    .reveal(shown, 4)
                }
                HStack(spacing: 10) {
                    ForEach(Array(["globe", "doc.text.magnifyingglass", "envelope", "calendar", "chart.bar"].enumerated()), id: \.offset) { i, symbol in
                        Image(systemName: symbol)
                            .font(.system(size: 14, weight: .medium))
                            .foregroundStyle(Theme.green)
                            .frame(width: 34, height: 34)
                            .background(Theme.green.opacity(0.12), in: Circle())
                            .reveal(shown, 5 + i)
                    }
                }
            }
        }
    }
}

/// A question, and the reply taking shape: the folded reasoning, two tool calls, and text.
private struct WorkPicture: View {
    let shown: Bool

    var body: some View {
        Stage {
            VStack(alignment: .leading, spacing: 10) {
                HStack {
                    Spacer()
                    Bubble(text: "Compare our last three quarters", mine: true)
                }
                .reveal(shown, 0)
                MiniCard(width: 260, padding: 10) {
                    VStack(alignment: .leading, spacing: 8) {
                        HStack(spacing: 6) {
                            Image(systemName: "chevron.right")
                                .font(.system(size: 9, weight: .bold))
                            Text("Reasoning")
                                .font(.system(size: 11, weight: .semibold))
                            Text(verbatim: "· 4 s")
                                .font(.system(size: 11))
                        }
                        .foregroundStyle(.secondary)
                        .reveal(shown, 1)
                        ToolRow(symbol: "magnifyingglass", name: "web_search", duration: "1.8 s").reveal(shown, 2)
                        ToolRow(symbol: "chevron.left.forwardslash.chevron.right", name: "run_python", duration: "0.6 s").reveal(shown, 3)
                        VStack(alignment: .leading, spacing: 6) {
                            TextLine(width: 220)
                            TextLine(width: 190)
                            TextLine(width: 120)
                        }
                        .padding(.top, 2)
                        .reveal(shown, 4)
                        HStack(spacing: 5) {
                            ProgressView().controlSize(.mini)
                            Text("Working…")
                                .font(.system(size: 10))
                                .foregroundStyle(.secondary)
                        }
                        .reveal(shown, 5)
                    }
                }
            }
            .padding(.horizontal, 10)
        }
    }
}

/// Four of the native cards: the weather, a chart, a recipe and a route.
private struct CardsPicture: View {
    let shown: Bool

    var body: some View {
        Stage {
            VStack(spacing: 10) {
                HStack(spacing: 10) {
                    MiniCard(width: 150) {
                        HStack(spacing: 10) {
                            Image(systemName: "sun.max.fill")
                                .font(.system(size: 26))
                                .foregroundStyle(.orange)
                            VStack(alignment: .leading, spacing: 2) {
                                Text(verbatim: "21°")
                                    .font(.system(size: 22, weight: .bold))
                                Text("Sunny")
                                    .font(.system(size: 11))
                                    .foregroundStyle(.secondary)
                            }
                        }
                    }
                    .reveal(shown, 0)
                    MiniCard(width: 150) {
                        HStack(alignment: .bottom, spacing: 7) {
                            ForEach(Array([22, 34, 28, 44, 52].enumerated()), id: \.offset) { i, h in
                                RoundedRectangle(cornerRadius: 3, style: .continuous)
                                    .fill(i == 4 ? Theme.clay : Theme.green)
                                    .frame(width: 14, height: CGFloat(h))
                            }
                        }
                        .frame(height: 52, alignment: .bottom)
                    }
                    .reveal(shown, 1)
                }
                HStack(spacing: 10) {
                    MiniCard(width: 150) {
                        VStack(alignment: .leading, spacing: 6) {
                            HStack(spacing: 6) {
                                Image(systemName: "fork.knife")
                                    .foregroundStyle(Theme.clay)
                                Text("Recipe")
                                    .font(.system(size: 12, weight: .semibold))
                            }
                            HStack(spacing: 5) {
                                ForEach(0..<3, id: \.self) { i in
                                    Circle().fill(Theme.green).frame(width: 6, height: 6)
                                    TextLine(width: 24)
                                    if i < 2 { Spacer(minLength: 0) }
                                }
                            }
                            TextLine(width: 100)
                        }
                    }
                    .reveal(shown, 2)
                    MiniCard(width: 150, padding: 0) {
                        ZStack {
                            RoundedRectangle(cornerRadius: 14, style: .continuous)
                                .fill(Theme.green.opacity(0.1))
                            RoutePath()
                                .stroke(Theme.green, style: StrokeStyle(lineWidth: 3, lineCap: .round, lineJoin: .round))
                                .padding(16)
                            Image(systemName: "mappin.circle.fill")
                                .font(.system(size: 18))
                                .foregroundStyle(Theme.clay)
                                .offset(x: 48, y: -16)
                        }
                        .frame(height: 70)
                    }
                    .reveal(shown, 3)
                }
            }
        }
    }

    private struct RoutePath: Shape {
        func path(in rect: CGRect) -> Path {
            var path = Path()
            path.move(to: CGPoint(x: rect.minX, y: rect.maxY))
            path.addCurve(to: CGPoint(x: rect.maxX, y: rect.minY),
                          control1: CGPoint(x: rect.minX + rect.width * 0.2, y: rect.minY),
                          control2: CGPoint(x: rect.maxX - rect.width * 0.2, y: rect.maxY))
            return path
        }
    }
}

/// A plan with its steps and the buttons that agree to it, and a question with its choices.
private struct PlansPicture: View {
    let shown: Bool

    var body: some View {
        Stage {
            VStack(spacing: 12) {
                MiniCard(width: 270) {
                    VStack(alignment: .leading, spacing: 8) {
                        HStack(spacing: 6) {
                            Image(systemName: "list.bullet.clipboard")
                                .foregroundStyle(Theme.green)
                            Text("Plan")
                                .font(.system(size: 12, weight: .semibold))
                            Spacer()
                            Text("Waiting for you")
                                .font(.system(size: 10, weight: .semibold))
                                .padding(.horizontal, 7)
                                .padding(.vertical, 3)
                                .background(Theme.clay.opacity(0.15), in: Capsule())
                                .foregroundStyle(Theme.clay)
                        }
                        .reveal(shown, 0)
                        step("Read the three reports", 1)
                        step("Compute the change by quarter", 2)
                        step("Draw the chart and write it up", 3)
                        HStack(spacing: 8) {
                            Text("Go ahead")
                                .font(.system(size: 11, weight: .semibold))
                                .foregroundStyle(Theme.onGreen)
                                .padding(.horizontal, 12)
                                .padding(.vertical, 6)
                                .background(Theme.green, in: Capsule())
                            Text("Stop")
                                .font(.system(size: 11, weight: .semibold))
                                .foregroundStyle(Theme.clay)
                                .padding(.horizontal, 12)
                                .padding(.vertical, 6)
                                .background(Theme.clay.opacity(0.12), in: Capsule())
                        }
                        .padding(.top, 2)
                        .reveal(shown, 4)
                    }
                }
                MiniCard(width: 270) {
                    VStack(alignment: .leading, spacing: 7) {
                        Text("Which quarters count as the last three?")
                            .font(.system(size: 11, weight: .semibold))
                        HStack(spacing: 8) {
                            choice("Calendar year", on: true)
                            choice("Fiscal year", on: false)
                        }
                    }
                }
                .reveal(shown, 5)
            }
        }
    }

    private func step(_ text: LocalizedStringKey, _ index: Int) -> some View {
        HStack(spacing: 7) {
            Text(verbatim: "\(index)")
                .font(.system(size: 9, weight: .bold))
                .foregroundStyle(Theme.green)
                .frame(width: 16, height: 16)
                .background(Theme.green.opacity(0.13), in: Circle())
            Text(text)
                .font(.system(size: 11))
        }
        .reveal(shown, index)
    }

    private func choice(_ text: LocalizedStringKey, on: Bool) -> some View {
        HStack(spacing: 5) {
            Image(systemName: on ? "largecircle.fill.circle" : "circle")
                .font(.system(size: 12))
                .foregroundStyle(on ? Theme.green : Color.secondary)
            Text(text)
                .font(.system(size: 11, weight: on ? .semibold : .regular))
        }
        .padding(.horizontal, 10)
        .padding(.vertical, 6)
        .background(Theme.raised, in: Capsule())
    }
}

/// Three agents in the sidebar, each started by something else (the clock, a value, new mail),
/// and the notification a finished run sends.
private struct AgentsPicture: View {
    let shown: Bool

    var body: some View {
        Stage {
            VStack(spacing: 12) {
                HStack(spacing: 10) {
                    Image(systemName: "bell.badge.fill")
                        .font(.system(size: 16))
                        .foregroundStyle(Theme.clay)
                        .frame(width: 32, height: 32)
                        .background(Theme.clay.opacity(0.13), in: RoundedRectangle(cornerRadius: 9, style: .continuous))
                    VStack(alignment: .leading, spacing: 2) {
                        Text("Morning briefing is ready")
                            .font(.system(size: 12, weight: .semibold))
                        Text("Three things changed overnight.")
                            .font(.system(size: 11))
                            .foregroundStyle(.secondary)
                    }
                    Spacer(minLength: 0)
                    Text("Now")
                        .font(.system(size: 10))
                        .foregroundStyle(.secondary)
                }
                .padding(10)
                .frame(width: 280)
                .background(.regularMaterial, in: RoundedRectangle(cornerRadius: 16, style: .continuous))
                .shadow(color: .black.opacity(0.1), radius: 12, y: 6)
                .reveal(shown, 0)
                MiniCard(width: 280, padding: 6) {
                    VStack(spacing: 0) {
                        row("Morning briefing", when: "Daily at 7:00", systemImage: "clock", unread: true).reveal(shown, 1)
                        Divider().padding(.horizontal, 8)
                        row("Price watch", when: "When the price drops", systemImage: "eye", unread: false).reveal(shown, 2)
                        Divider().padding(.horizontal, 8)
                        row("New invoices", when: "When mail arrives", systemImage: "envelope", unread: false).reveal(shown, 3)
                    }
                }
            }
        }
    }

    private func row(_ title: LocalizedStringKey, when: LocalizedStringKey, systemImage: String, unread: Bool) -> some View {
        HStack(spacing: 10) {
            Image(systemName: systemImage)
                .font(.system(size: 13, weight: .medium))
                .foregroundStyle(Theme.green)
                .frame(width: 30, height: 30)
                .background(Theme.green.opacity(0.13), in: RoundedRectangle(cornerRadius: 9, style: .continuous))
            VStack(alignment: .leading, spacing: 2) {
                Text(title)
                    .font(.system(size: 12, weight: unread ? .semibold : .regular))
                Text(when)
                    .font(.system(size: 10))
                    .foregroundStyle(.secondary)
            }
            Spacer(minLength: 0)
            if unread { UnreadDot() }
        }
        .padding(8)
    }
}

/// What Cedar remembers, as chips, with a lock on the server they stay on.
private struct MemoryPicture: View {
    let shown: Bool

    var body: some View {
        Stage {
            VStack(spacing: 16) {
                ZStack {
                    Circle()
                        .fill(Theme.green.opacity(0.12))
                        .frame(width: 84, height: 84)
                    Image(systemName: "bookmark.fill")
                        .font(.system(size: 34, weight: .medium))
                        .foregroundStyle(Theme.green)
                    Image(systemName: "lock.fill")
                        .font(.system(size: 11, weight: .bold))
                        .foregroundStyle(Theme.onGreen)
                        .frame(width: 26, height: 26)
                        .background(Theme.clay, in: Circle())
                        .offset(x: 32, y: 30)
                }
                .reveal(shown, 0)
                VStack(spacing: 8) {
                    chip("Prefers metric units", 1)
                    chip("Works on the cedar app", 2)
                    chip("Plans a trip to Lisbon in May", 3)
                }
            }
        }
    }

    private func chip(_ text: LocalizedStringKey, _ index: Int) -> some View {
        HStack(spacing: 8) {
            Image(systemName: "bookmark")
                .font(.system(size: 11))
                .foregroundStyle(Theme.green)
            Text(text)
                .font(.system(size: 12, weight: .medium))
            Spacer(minLength: 0)
            Image(systemName: "xmark")
                .font(.system(size: 9, weight: .bold))
                .foregroundStyle(.secondary)
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 9)
        .frame(width: 250)
        .surfaceCard(radius: 12)
        .reveal(shown, index)
    }
}

/// A QR code in the viewfinder's corners, and the fields it fills in.
private struct ConnectPicture: View {
    let shown: Bool

    var body: some View {
        Stage {
            HStack(spacing: 22) {
                ZStack {
                    QRCodeMock()
                        .frame(width: 96, height: 96)
                    Corners()
                        .stroke(Theme.green, style: StrokeStyle(lineWidth: 4, lineCap: .round))
                        .frame(width: 136, height: 136)
                }
                .reveal(shown, 0)
                VStack(alignment: .leading, spacing: 10) {
                    field("Server URL", value: "https://home.ts.net", 1)
                    field("Access Token", value: "••••••••••", 2)
                    HStack(spacing: 5) {
                        Image(systemName: "checkmark.circle.fill")
                        Text("Connected.")
                    }
                    .font(.system(size: 11, weight: .semibold))
                    .foregroundStyle(Theme.green)
                    .reveal(shown, 3)
                }
            }
        }
    }

    private func field(_ label: LocalizedStringKey, value: String, _ index: Int) -> some View {
        VStack(alignment: .leading, spacing: 3) {
            Text(label)
                .font(.system(size: 9))
                .foregroundStyle(.secondary)
            Text(verbatim: value)
                .font(.system(size: 12, design: .monospaced))
                .lineLimit(1)
        }
        .padding(.horizontal, 10)
        .padding(.vertical, 7)
        .frame(width: 150, alignment: .leading)
        .surfaceCard(radius: 10)
        .reveal(shown, index)
    }

    /// A fixed pattern that looks like a QR code (it encodes nothing).
    private struct QRCodeMock: View {
        private var cells: [Bool] {
            (0..<(13 * 13)).map { i in
                let x = i % 13, y = i / 13
                let finder = (x < 5 && y < 5) || (x > 7 && y < 5) || (x < 5 && y > 7)
                if finder {
                    let fx = x > 7 ? x - 8 : x, fy = y > 7 ? y - 8 : y
                    return fx == 0 || fx == 4 || fy == 0 || fy == 4 || (fx > 0 && fx < 4 && fy > 0 && fy < 4)
                }
                return (x * 7 + y * 13 + x * y) % 3 == 0
            }
        }

        var body: some View {
            let cells = cells
            VStack(spacing: 0) {
                ForEach(0..<13, id: \.self) { y in
                    HStack(spacing: 0) {
                        ForEach(0..<13, id: \.self) { x in
                            Rectangle()
                                .fill(cells[y * 13 + x] ? Theme.markInk : Color.clear)
                        }
                    }
                }
            }
            .padding(6)
            .background(Theme.surface, in: RoundedRectangle(cornerRadius: 8, style: .continuous))
        }
    }

    private struct Corners: Shape {
        func path(in rect: CGRect) -> Path {
            var path = Path()
            let l: CGFloat = 22
            let r: CGFloat = 10
            // Top left
            path.move(to: CGPoint(x: rect.minX, y: rect.minY + l))
            path.addLine(to: CGPoint(x: rect.minX, y: rect.minY + r))
            path.addQuadCurve(to: CGPoint(x: rect.minX + r, y: rect.minY), control: CGPoint(x: rect.minX, y: rect.minY))
            path.addLine(to: CGPoint(x: rect.minX + l, y: rect.minY))
            // Top right
            path.move(to: CGPoint(x: rect.maxX - l, y: rect.minY))
            path.addLine(to: CGPoint(x: rect.maxX - r, y: rect.minY))
            path.addQuadCurve(to: CGPoint(x: rect.maxX, y: rect.minY + r), control: CGPoint(x: rect.maxX, y: rect.minY))
            path.addLine(to: CGPoint(x: rect.maxX, y: rect.minY + l))
            // Bottom right
            path.move(to: CGPoint(x: rect.maxX, y: rect.maxY - l))
            path.addLine(to: CGPoint(x: rect.maxX, y: rect.maxY - r))
            path.addQuadCurve(to: CGPoint(x: rect.maxX - r, y: rect.maxY), control: CGPoint(x: rect.maxX, y: rect.maxY))
            path.addLine(to: CGPoint(x: rect.maxX - l, y: rect.maxY))
            // Bottom left
            path.move(to: CGPoint(x: rect.minX + l, y: rect.maxY))
            path.addLine(to: CGPoint(x: rect.minX + r, y: rect.maxY))
            path.addQuadCurve(to: CGPoint(x: rect.minX, y: rect.maxY - r), control: CGPoint(x: rect.minX, y: rect.maxY))
            path.addLine(to: CGPoint(x: rect.minX, y: rect.maxY - l))
            return path
        }
    }
}
