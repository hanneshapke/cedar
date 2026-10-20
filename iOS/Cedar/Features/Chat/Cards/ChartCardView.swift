import Charts
import SwiftUI

/// A `generate_chart` card: the call's data drawn with Swift Charts instead of the picture the
/// sandbox saved. A legend whose entries hide and show their series, the plot (touch it to read the
/// values), and the data as a table. The same eight hues as the web card, stepped for dark mode; a
/// ninth series and later, and a pie's "Other", are grey.
struct ChartCardView: View {
    let card: ChartCard
    /// Category names along the axis (bins for a histogram), unique so each is its own bar.
    private let categories: [String]
    /// Each series' values, one per category.
    private let columns: [[Double?]]
    private let points: [Point]
    private let slices: [Slice]
    @State private var hidden: Set<Int> = []
    @State private var selectedCategory: String?
    @State private var selectedNumber: Double?
    @State private var selectedAngle: Double?
    @State private var showData = false

    struct Point: Identifiable {
        let id: Int
        let series: Int
        /// Splits a line at its gaps.
        let segment: Int
        let index: Int
        let category: String
        let x: Double
        let y: Double
    }

    struct Slice: Identifiable {
        let id: Int
        let name: String
        let value: Double
    }

    static let maxSlices = 8
    static let maxTableRows = 200

    init(card: ChartCard) {
        self.card = card
        let categories: [String]
        let columns: [[Double?]]
        if card.kind == .histogram {
            let h = card.histogram
            categories = h.map { h in h.counts[0].indices.map { "\(ChartCard.format(h.edges[$0]))–\(ChartCard.format(h.edges[$0 + 1]))" } } ?? []
            columns = h?.counts.map { $0.map(Double.init) } ?? []
        } else {
            categories = Self.unique(card.labels.map(\.text))
            columns = card.series.map(\.values)
        }
        self.categories = categories
        self.columns = columns
        var points: [Point] = []
        for (s, values) in columns.enumerated() {
            var segment = 0
            for (i, value) in values.enumerated() {
                guard let y = value, !(card.log && y <= 0) else { segment += 1; continue }
                points.append(Point(id: points.count, series: s, segment: segment, index: i, category: categories[i],
                                    x: card.labels.indices.contains(i) ? card.labels[i].number ?? Double(i) : Double(i), y: y))
            }
        }
        self.points = points

        // A pie keeps its largest slices and folds the rest into "Other".
        var slices: [Slice] = []
        if card.kind == .pie {
            let all = zip(card.labels, card.series[0].values).compactMap { label, v -> (String, Double)? in
                guard let v, v > 0 else { return nil }
                return (label.text, v)
            }
            if all.count > Self.maxSlices {
                let kept = Set(all.enumerated().sorted { $0.element.1 > $1.element.1 }.prefix(Self.maxSlices - 1).map(\.offset))
                for (i, s) in all.enumerated() where kept.contains(i) { slices.append(Slice(id: slices.count, name: s.0, value: s.1)) }
                let rest = all.enumerated().filter { !kept.contains($0.offset) }
                slices.append(Slice(id: ChartCard.slots, name: "Other (\(rest.count))", value: rest.reduce(0) { $0 + $1.element.1 }))
            } else {
                slices = all.enumerated().map { Slice(id: $0.offset, name: $0.element.0, value: $0.element.1) }
            }
        }
        self.slices = slices
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            header
            if card.kind != .pie, card.series.count > 1 || card.series[0].named { legend }
            plot
            footer
        }
        .cardStyle()
    }

    // MARK: Header and legend

    private var header: some View {
        HStack(spacing: 10) {
            Image(systemName: card.kind == .pie ? "chart.pie" : card.kind == .line || card.kind == .area ? "chart.xyaxis.line" : "chart.bar")
                .font(.system(size: 15, weight: .semibold))
                .foregroundStyle(Color.accentColor)
                .frame(width: 30, height: 30)
                .background(Color.accentColor.opacity(0.14), in: RoundedRectangle(cornerRadius: 8))
            VStack(alignment: .leading, spacing: 1) {
                Text(card.title ?? card.kind.name).font(.headline)
                Text(subtitle).font(.caption).foregroundStyle(.secondary)
            }
        }
        .accessibilityElement(children: .combine)
    }

    private var subtitle: String {
        var parts: [String] = []
        if card.title != nil { parts.append(card.stacked ? "Stacked \(card.kind.name.lowercased())" : card.kind.name) }
        if card.log { parts.append("log scale") }
        switch card.kind {
        case .histogram: parts.append("\(card.series.reduce(0) { $0 + $1.values.compactMap { $0 }.count }) samples")
        case .pie: parts.append("\(card.labels.count) slices")
        default:
            let count = "\(card.labels.count) \(card.kind == .scatter ? "points" : "values")"
            parts.append(card.series.count > 1 ? "\(count) × \(card.series.count) series" : count)
        }
        return parts.joined(separator: " · ")
    }

    private var legend: some View {
        ScrollView(.horizontal, showsIndicators: false) {
            HStack(spacing: 4) {
                ForEach(Array(card.series.enumerated()), id: \.offset) { s, series in
                    let on = !hidden.contains(s)
                    Button {
                        withAnimation(.snappy(duration: 0.2)) {
                            if on, hidden.count < card.series.count - 1 { hidden.insert(s) } else { hidden.remove(s) }
                        }
                    } label: {
                        HStack(spacing: 6) {
                            RoundedRectangle(cornerRadius: 3)
                                .fill(on ? ChartPalette.color(s) : Color.clear)
                                .overlay(RoundedRectangle(cornerRadius: 3).strokeBorder(ChartPalette.color(s), lineWidth: on ? 0 : 1.5))
                                .frame(width: 10, height: 10)
                            Text(series.name)
                                .font(.caption)
                                .strikethrough(!on)
                                .foregroundStyle(on ? .primary : .secondary)
                        }
                        .padding(.horizontal, 9)
                        .padding(.vertical, 5)
                        .background(Color(.tertiarySystemFill).opacity(on ? 0.6 : 0.3), in: Capsule())
                    }
                    .buttonStyle(.plain)
                    .disabled(card.series.count == 1)
                    .accessibilityLabel(series.name)
                    .accessibilityValue(on ? "Shown" : "Hidden")
                }
            }
        }
    }

    // MARK: The plot

    private var visible: [Point] { points.filter { !hidden.contains($0.series) } }
    private var names: [String] { card.series.indices.map(seriesKey) }
    private var grouped: Bool { !card.stacked && card.series.count - hidden.count > 1 }

    /// The legend key of series `s`: its name, made unique so two series never share a color.
    private func seriesKey(_ s: Int) -> String {
        let name = card.series[s].name
        return card.series[..<s].contains { $0.name == name } ? "\(name) (\(s + 1))" : name
    }

    @ViewBuilder private var plot: some View {
        if card.kind == .pie {
            pie
        } else if card.kind == .barh {
            cartesian.chartYSelection(value: $selectedCategory)
        } else if card.kind == .scatter {
            cartesian
        } else if card.numericX {
            cartesian.chartXSelection(value: $selectedNumber)
        } else {
            cartesian.chartXSelection(value: $selectedCategory)
        }
    }

    private var cartesian: some View {
        Chart {
            ForEach(visible) { p in
                marks(p)
            }
            if let i = selectedIndex {
                if card.kind == .barh {
                    RuleMark(y: .value("Category", categories[i]))
                        .foregroundStyle(Color.secondary.opacity(0.35))
                        .annotation(position: .trailing, alignment: .center, spacing: 4, overflowResolution: .init(x: .fit(to: .chart), y: .fit(to: .chart))) {
                            readout(i)
                        }
                } else if card.numericX {
                    RuleMark(x: .value("X", xValue(i)))
                        .foregroundStyle(Color.secondary.opacity(0.5))
                        .lineStyle(StrokeStyle(lineWidth: 1, dash: [3, 3]))
                        .annotation(position: .top, spacing: 4, overflowResolution: .init(x: .fit(to: .chart), y: .disabled)) {
                            readout(i)
                        }
                } else {
                    RuleMark(x: .value("Category", categories[i]))
                        .foregroundStyle(Color.secondary.opacity(card.kind == .line || card.kind == .area ? 0.5 : 0.15))
                        .annotation(position: .top, spacing: 4, overflowResolution: .init(x: .fit(to: .chart), y: .disabled)) {
                            readout(i)
                        }
                }
            }
        }
        .chartForegroundStyleScale(domain: names, range: card.series.indices.map(ChartPalette.color))
        .chartLegend(.hidden)
        .modifier(ValueAxis(log: card.log, horizontal: card.kind == .barh))
        .modifier(AxisTitles(x: card.xLabel, y: card.yLabel))
        .frame(height: card.kind == .barh ? CGFloat(max(160, min(600, categories.count * (grouped ? 14 * (card.series.count - hidden.count) + 10 : 28) + 40))) : 220)
        .accessibilityLabel("\(card.title ?? card.kind.name): \(card.series.map(\.name).joined(separator: ", ")). The data is in the table below.")
    }

    @ChartContentBuilder private func marks(_ p: Point) -> some ChartContent {
        let key = seriesKey(p.series)
        switch card.kind {
        case .bar, .histogram:
            if grouped {
                BarMark(x: .value("Category", p.category), y: .value("Value", p.y))
                    .foregroundStyle(by: .value("Series", key))
                    .position(by: .value("Series", key))
                    .cornerRadius(4)
            } else {
                BarMark(x: .value("Category", p.category), y: .value("Value", p.y))
                    .foregroundStyle(by: .value("Series", key))
                    .cornerRadius(card.stacked ? 2 : 4)
            }
        case .barh:
            if grouped {
                BarMark(x: .value("Value", p.y), y: .value("Category", p.category))
                    .foregroundStyle(by: .value("Series", key))
                    .position(by: .value("Series", key))
                    .cornerRadius(4)
            } else {
                BarMark(x: .value("Value", p.y), y: .value("Category", p.category))
                    .foregroundStyle(by: .value("Series", key))
                    .cornerRadius(card.stacked ? 2 : 4)
            }
        case .scatter:
            PointMark(x: .value("X", p.x), y: .value("Y", p.y))
                .foregroundStyle(by: .value("Series", key))
                .symbolSize(card.labels.count > 400 ? 14 : 36)
                .opacity(0.85)
        case .area:
            if card.numericX {
                AreaMark(x: .value("X", p.x), y: .value("Value", p.y), stacking: card.stacked ? .standard : .unstacked)
                    .foregroundStyle(by: .value("Series", key))
                    .opacity(card.stacked ? 0.75 : 0.16)
                    .interpolationMethod(.monotone)
            } else {
                AreaMark(x: .value("Category", p.category), y: .value("Value", p.y), stacking: card.stacked ? .standard : .unstacked)
                    .foregroundStyle(by: .value("Series", key))
                    .opacity(card.stacked ? 0.75 : 0.16)
                    .interpolationMethod(.monotone)
            }
            if !card.stacked { line(p, key: key) }
        default:
            line(p, key: key)
            if card.labels.count <= 40 {
                if card.numericX {
                    PointMark(x: .value("X", p.x), y: .value("Value", p.y))
                        .foregroundStyle(by: .value("Series", key))
                        .symbolSize(18)
                } else {
                    PointMark(x: .value("Category", p.category), y: .value("Value", p.y))
                        .foregroundStyle(by: .value("Series", key))
                        .symbolSize(18)
                }
            }
        }
    }

    @ChartContentBuilder private func line(_ p: Point, key: String) -> some ChartContent {
        if card.numericX {
            LineMark(x: .value("X", p.x), y: .value("Value", p.y), series: .value("Line", "\(p.series)-\(p.segment)"))
                .foregroundStyle(by: .value("Series", key))
                .lineStyle(StrokeStyle(lineWidth: 2, lineCap: .round, lineJoin: .round))
                .interpolationMethod(.monotone)
        } else {
            LineMark(x: .value("Category", p.category), y: .value("Value", p.y), series: .value("Line", "\(p.series)-\(p.segment)"))
                .foregroundStyle(by: .value("Series", key))
                .lineStyle(StrokeStyle(lineWidth: 2, lineCap: .round, lineJoin: .round))
                .interpolationMethod(.monotone)
        }
    }

    /// The category or x value the touch is on.
    private var selectedIndex: Int? {
        if card.numericX, card.kind != .scatter, let x = selectedNumber {
            return card.labels.indices.min { abs(xValue($0) - x) < abs(xValue($1) - x) }
        }
        guard let c = selectedCategory else { return nil }
        return categories.firstIndex(of: c)
    }

    private func xValue(_ i: Int) -> Double { card.labels[i].number ?? Double(i) }

    /// Every shown series' value at category `i`, and their total when stacked.
    private func readout(_ i: Int) -> some View {
        let shown = card.series.indices.filter { !hidden.contains($0) }
        return VStack(alignment: .leading, spacing: 2) {
            Text(categories[i]).font(.caption2.weight(.semibold)).foregroundStyle(.secondary)
            ForEach(shown, id: \.self) { s in
                HStack(spacing: 5) {
                    RoundedRectangle(cornerRadius: 2).fill(ChartPalette.color(s)).frame(width: 8, height: 8)
                    Text(card.series[s].name).lineLimit(1)
                    Spacer(minLength: 6)
                    Text(columns[s][i].map(ChartCard.format) ?? "–").fontWeight(.semibold).monospacedDigit()
                }
            }
            if card.stacked, shown.count > 1 {
                Divider()
                HStack {
                    Text("Total")
                    Spacer(minLength: 6)
                    Text(ChartCard.format(shown.reduce(0) { $0 + (columns[$1][i] ?? 0) })).fontWeight(.semibold).monospacedDigit()
                }
            }
        }
        .font(.caption2)
        .padding(7)
        .frame(maxWidth: 200)
        .background(.regularMaterial, in: RoundedRectangle(cornerRadius: 8))
    }

    // MARK: The pie

    private var pieTotal: Double { slices.reduce(0) { $0 + $1.value } }

    /// The slice under the touch, from the angle Swift Charts reports in the values' units.
    private var selectedSlice: Slice? {
        guard let angle = selectedAngle else { return nil }
        var sum = 0.0
        for s in slices {
            sum += s.value
            if angle <= sum { return s }
        }
        return slices.last
    }

    private var pie: some View {
        let total = pieTotal
        let selected = selectedSlice
        return VStack(alignment: .leading, spacing: 10) {
            Chart(slices) { s in
                SectorMark(angle: .value("Value", s.value), innerRadius: .ratio(0.62), angularInset: 1.5)
                    .foregroundStyle(by: .value("Slice", s.name))
                    .cornerRadius(3)
                    .opacity(selected == nil || selected?.id == s.id ? 1 : 0.3)
            }
            .chartForegroundStyleScale(domain: slices.map(\.name), range: slices.map { ChartPalette.color($0.id) })
            .chartLegend(.hidden)
            .chartAngleSelection(value: $selectedAngle)
            .chartBackground { proxy in
                GeometryReader { geo in
                    if let anchor = proxy.plotFrame {
                        let frame = geo[anchor]
                        VStack(spacing: 0) {
                            Text(selected.map { Self.percent($0.value / total) } ?? ChartCard.format(total))
                                .font(.title3.weight(.bold).monospacedDigit())
                            Text(selected?.name ?? "Total")
                                .font(.caption2)
                                .foregroundStyle(.secondary)
                                .lineLimit(1)
                        }
                        .frame(maxWidth: frame.width * 0.5)
                        .position(x: frame.midX, y: frame.midY)
                    }
                }
            }
            .frame(height: 200)
            .accessibilityLabel("\(card.title ?? "Pie chart"): " + slices.map { "\($0.name) \(Self.percent($0.value / total))" }.joined(separator: ", "))

            VStack(spacing: 0) {
                ForEach(slices) { s in
                    HStack(spacing: 10) {
                        RoundedRectangle(cornerRadius: 3).fill(ChartPalette.color(s.id)).frame(width: 10, height: 10)
                        Text(s.name).lineLimit(1)
                        Spacer(minLength: 6)
                        Text(ChartCard.format(s.value)).foregroundStyle(.secondary).monospacedDigit()
                        Text(Self.percent(s.value / total))
                            .fontWeight(.semibold)
                            .monospacedDigit()
                            .frame(minWidth: 48, alignment: .trailing)
                    }
                    .font(.subheadline)
                    .padding(.vertical, 6)
                    .padding(.horizontal, 6)
                    .background(selected?.id == s.id ? Color(.tertiarySystemFill) : .clear, in: RoundedRectangle(cornerRadius: 6))
                    if s.id != slices.last?.id { Divider() }
                }
            }
        }
    }

    private static func percent(_ share: Double) -> String {
        share.formatted(.percent.precision(.fractionLength(1)))
    }

    // MARK: Footer: the data and where the picture was saved

    private var footer: some View {
        VStack(alignment: .leading, spacing: 6) {
            HStack(alignment: .firstTextBaseline) {
                Button {
                    withAnimation(.snappy(duration: 0.2)) { showData.toggle() }
                } label: {
                    Label("Data", systemImage: "chevron.right")
                        .labelStyle(DisclosureLabelStyle(open: showData))
                }
                .buttonStyle(.plain)
                .font(.caption.weight(.semibold))
                .foregroundStyle(.secondary)
                Spacer(minLength: 8)
                if let note {
                    Text(note)
                        .font(.caption)
                        .foregroundStyle(.secondary)
                        .multilineTextAlignment(.trailing)
                }
            }
            if showData { table }
        }
    }

    private var note: String? {
        var notes: [String] = []
        if card.thinned { notes.append("Long series are thinned here") }
        if let path = card.path { notes.append("Saved as \((path as NSString).lastPathComponent)") }
        return notes.isEmpty ? nil : notes.joined(separator: " · ")
    }

    private var table: some View {
        let header: [String]
        if card.kind == .pie {
            header = [card.xLabel ?? "Slice", card.yLabel ?? "Value", "Share"]
        } else {
            let first = card.kind == .histogram ? (card.xLabel ?? "Bin") : card.kind == .barh ? (card.yLabel ?? "") : (card.xLabel ?? "")
            let single = card.series.count == 1 && !card.series[0].named
            header = [first] + card.series.map { single ? ((card.kind == .barh ? card.xLabel : card.yLabel) ?? "Value") : $0.name }
        }
        let total = card.kind == .pie ? card.series[0].values.reduce(0) { $0 + ($1 ?? 0) } : 0
        let rows = categories.indices.prefix(Self.maxTableRows).map { i -> [String] in
            var row = [categories[i]] + columns.map { $0[i].map(ChartCard.format) ?? "–" }
            if card.kind == .pie, total > 0 { row.append(Self.percent((columns[0][i] ?? 0) / total)) }
            return row
        }
        return VStack(alignment: .leading, spacing: 4) {
            ScrollView(.horizontal, showsIndicators: false) {
                Grid(alignment: .trailing, horizontalSpacing: 16, verticalSpacing: 6) {
                    GridRow {
                        ForEach(Array(header.enumerated()), id: \.offset) { k, h in
                            Text(h).fontWeight(.semibold).foregroundStyle(.secondary)
                                .gridColumnAlignment(k == 0 ? .leading : .trailing)
                        }
                    }
                    Divider()
                    ForEach(Array(rows.enumerated()), id: \.offset) { _, row in
                        GridRow {
                            ForEach(Array(row.enumerated()), id: \.offset) { _, cell in
                                Text(cell).monospacedDigit().lineLimit(1)
                            }
                        }
                    }
                }
                .font(.caption)
                .padding(10)
            }
            .background(Color(.tertiarySystemFill).opacity(0.35), in: RoundedRectangle(cornerRadius: 8))
            if categories.count > Self.maxTableRows {
                Text("First \(Self.maxTableRows) of \(categories.count) rows; the saved file has them all.")
                    .font(.caption2)
                    .foregroundStyle(.secondary)
            }
        }
    }

    /// Labels made unique by numbering repeats, so two categories never share a bar.
    private static func unique(_ labels: [String]) -> [String] {
        var seen: [String: Int] = [:]
        return labels.map { label in
            let n = seen[label, default: 0] + 1
            seen[label] = n
            return n == 1 ? label : "\(label) (\(n))"
        }
    }
}

/// A disclosure chevron that turns when open, before the title.
private struct DisclosureLabelStyle: LabelStyle {
    let open: Bool

    func makeBody(configuration: Configuration) -> some View {
        HStack(spacing: 4) {
            configuration.icon
                .imageScale(.small)
                .rotationEffect(.degrees(open ? 90 : 0))
            configuration.title
        }
    }
}

/// The value axis: compact numbers, recessive grid lines, and a log scale when asked for.
private struct ValueAxis: ViewModifier {
    let log: Bool
    let horizontal: Bool

    @ViewBuilder func body(content: Content) -> some View {
        if horizontal {
            content
                .chartXScale(type: log ? .log : .linear)
                .chartXAxis { axisMarks }
        } else {
            content
                .chartYScale(type: log ? .log : .linear)
                .chartYAxis { axisMarks }
        }
    }

    private var axisMarks: some AxisContent {
        AxisMarks(values: .automatic(desiredCount: 5)) { value in
            AxisGridLine(stroke: StrokeStyle(lineWidth: 0.5))
            AxisValueLabel {
                if let v = value.as(Double.self) { Text(v.formatted(.number.notation(.compactName))) }
            }
        }
    }
}

/// Axis titles, where the model gave them: x under the plot, y up its side.
private struct AxisTitles: ViewModifier {
    let x: String?
    let y: String?

    @ViewBuilder func body(content: Content) -> some View {
        switch (x, y) {
        case let (x?, y?): content.chartXAxisLabel(x, alignment: .center).chartYAxisLabel(y)
        case let (x?, nil): content.chartXAxisLabel(x, alignment: .center)
        case let (nil, y?): content.chartYAxisLabel(y)
        case (nil, nil): content
        }
    }
}

/// The categorical palette, in a fixed order, with its own steps for dark mode.
enum ChartPalette {
    private static let light = ["#2a78d6", "#eb6834", "#1baf7a", "#eda100", "#e87ba4", "#008300", "#6250d6", "#e34948"]
    private static let dark = ["#3987e5", "#d95926", "#199e70", "#c98500", "#d55181", "#008300", "#9085e9", "#e66767"]
    private static let other = ("#a3a19a", "#6f6d66")

    static func color(_ slot: Int) -> Color {
        let (l, d) = slot < light.count ? (light[slot], dark[slot]) : other
        return Color(UIColor { $0.userInterfaceStyle == .dark ? uiColor(d) : uiColor(l) })
    }

    private static func uiColor(_ hex: String) -> UIColor {
        let value = UInt32(hex.dropFirst(), radix: 16) ?? 0
        return UIColor(red: CGFloat((value >> 16) & 0xff) / 255, green: CGFloat((value >> 8) & 0xff) / 255,
                       blue: CGFloat(value & 0xff) / 255, alpha: 1)
    }
}
