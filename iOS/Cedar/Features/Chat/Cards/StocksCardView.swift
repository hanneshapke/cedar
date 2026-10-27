import Charts
import SwiftUI

/// A `compare_stocks` card: a quote tile per symbol (which also picks the symbol shown below), a
/// chart of % change against the benchmark or one symbol's price, the selected symbol's rating,
/// the returns side by side, and the news.
struct StocksCardView: View {
    let card: StocksCard
    @State private var selected: String
    @State private var range: String
    @State private var percent: Bool
    @State private var probe: Date?
    @State private var allNews = false

    init(card: StocksCard) {
        self.card = card
        let lines = Self.lines(card)
        let offered = Self.offeredRanges(card, lines: lines)
        _selected = State(initialValue: card.stocks[0].symbol)
        _range = State(initialValue: offered.contains(card.range) ? card.range : offered.last ?? "1Y")
        _percent = State(initialValue: lines.count > 1)
    }

    private var stock: StocksCard.Stock { card.stocks.first { $0.symbol == selected } ?? card.stocks[0] }

    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            tiles
            chart
            RatingPanel(stock: stock, benchmark: card.benchmark?.name)
            returns
            news
            footer
        }
        .cardStyle()
    }

    // MARK: Quote tiles

    private var tiles: some View {
        ScrollView(.horizontal, showsIndicators: false) {
            HStack(spacing: 8) {
                ForEach(card.stocks) { s in
                    let isSelected = s.symbol == selected
                    Button {
                        withAnimation(.snappy(duration: 0.2)) { selected = s.symbol }
                    } label: {
                        VStack(alignment: .leading, spacing: 4) {
                            HStack(spacing: 6) {
                                Text(s.symbol).font(.subheadline.weight(.bold))
                                if let rating = s.rating { CallPill(call: rating.call, small: true) }
                            }
                            .fixedSize()
                            Text(StockFormat.money(s.price, s.currency))
                                .font(.subheadline.monospacedDigit())
                            Text(StockFormat.signed(s.changePct, digits: 2))
                                .font(.caption.monospacedDigit().weight(.medium))
                                .foregroundStyle(StockFormat.color(s.changePct))
                        }
                        .padding(10)
                        .frame(minWidth: 108, alignment: .leading)
                        .background(isSelected ? Color.accentColor.opacity(0.1) : Color(.tertiarySystemFill).opacity(0.5),
                                    in: RoundedRectangle(cornerRadius: 10))
                        .overlay(RoundedRectangle(cornerRadius: 10)
                            .strokeBorder(isSelected ? Color.accentColor.opacity(0.6) : Color.clear))
                    }
                    .buttonStyle(.plain)
                    .accessibilityLabel("\(s.symbol), \(s.name), \(StockFormat.money(s.price, s.currency)), \(StockFormat.signed(s.changePct, digits: 2)) today")
                    .accessibilityAddTraits(isSelected ? .isSelected : [])
                }
            }
        }
    }

    // MARK: The chart

    /// A line on the chart: a symbol, or the benchmark when it isn't one of them.
    private struct ChartLine {
        let key: String
        let label: String
        let series: StocksCard.Series
        let isBenchmark: Bool
    }

    private struct Point: Identifiable {
        let id: Int
        let key: String
        let date: Date
        let value: Double
    }

    private static func lines(_ card: StocksCard) -> [ChartLine] {
        var lines = card.stocks.map { ChartLine(key: $0.symbol, label: $0.symbol, series: $0.series, isBenchmark: false) }
        if let b = card.benchmark, let series = b.series {
            lines.append(ChartLine(key: b.symbol, label: b.name, series: series, isBenchmark: true))
        }
        return lines
    }

    /// The periods the data covers, with some slack for weekends and holidays.
    private static func offeredRanges(_ card: StocksCard, lines: [ChartLine]) -> [String] {
        guard let first = lines.compactMap(\.series.dates.first).min(),
              let last = lines.compactMap(\.series.dates.last).max() else { return [card.range] }
        return StocksCard.ranges.filter { key, days in
            key == card.range || last.addingTimeInterval(-Double(days) * 86400) >= first.addingTimeInterval(-7 * 86400)
        }.map(\.0)
    }

    /// Each shown series from its first point inside the period; % change counts from that point.
    /// At most about 260 points per line, which is plenty for a phone's width.
    private var plotted: [(line: ChartLine, points: [Point])] {
        let all = Self.lines(card)
        let shown = percent ? all : all.filter { $0.key == selected }
        guard let last = all.compactMap(\.series.dates.last).max() else { return [] }
        let days = StocksCard.ranges.first { $0.0 == range }?.1 ?? 365
        let from = last.addingTimeInterval(-Double(days) * 86400)
        return shown.map { line in
            let s = line.series
            let start = min(s.dates.firstIndex { $0 >= from } ?? s.dates.count - 2, s.dates.count - 2)
            let base = s.closes[start]
            let step = max(1, (s.dates.count - start) / 260)
            var indices = Array(stride(from: start, to: s.dates.count, by: step))
            if indices.last != s.dates.count - 1 { indices.append(s.dates.count - 1) }
            let points = indices.map { i in
                Point(id: i, key: line.label, date: s.dates[i], value: percent ? (s.closes[i] / base - 1) * 100 : s.closes[i])
            }
            return (line, points)
        }
    }

    private var chart: some View {
        let data = plotted
        let lines = Self.lines(card)
        let offered = Self.offeredRanges(card, lines: lines)
        return VStack(alignment: .leading, spacing: 8) {
            HStack {
                Picker("Period", selection: $range) {
                    ForEach(offered, id: \.self) { Text($0).tag($0) }
                }
                .pickerStyle(.segmented)
                if lines.count > 1 {
                    Picker("Values", selection: $percent) {
                        Text(verbatim: "%").tag(true)
                        Text(StockFormat.currencySymbol(stock.currency)).tag(false)
                    }
                    .pickerStyle(.segmented)
                    .frame(width: 84)
                }
            }
            Chart {
                ForEach(data, id: \.line.key) { item in
                    ForEach(item.points) { p in
                        if !percent {
                            AreaMark(x: .value("Date", p.date), yStart: .value("Low", lowerBound(data)), yEnd: .value(stock.symbol, p.value))
                                .foregroundStyle(LinearGradient(colors: [Color.accentColor.opacity(0.25), .clear], startPoint: .top, endPoint: .bottom))
                        }
                        LineMark(x: .value("Date", p.date), y: .value(percent ? "Change" : "Price", p.value))
                            .foregroundStyle(by: .value("Symbol", item.line.label))
                            .lineStyle(StrokeStyle(lineWidth: item.line.isBenchmark ? 1.5 : 2, dash: item.line.isBenchmark ? [4, 3] : []))
                            .interpolationMethod(.monotone)
                    }
                }
                if percent {
                    RuleMark(y: .value("Zero", 0))
                        .foregroundStyle(Color(.separator))
                        .lineStyle(StrokeStyle(lineWidth: 1))
                }
                if let probe, let readings = readings(data, at: probe) {
                    RuleMark(x: .value("Date", readings.date))
                        .foregroundStyle(Color.secondary.opacity(0.5))
                        .annotation(position: .top, spacing: 4, overflowResolution: .init(x: .fit(to: .chart), y: .disabled)) {
                            VStack(alignment: .leading, spacing: 2) {
                                Text(readings.date.formatted(Date.FormatStyle(date: .abbreviated, time: .omitted, locale: L10n.locale, timeZone: .gmt)))
                                    .font(.caption2.weight(.semibold))
                                ForEach(readings.values, id: \.0) { label, value in
                                    Text(verbatim: "\(label) \(format(value))").font(.caption2.monospacedDigit())
                                }
                            }
                            .padding(6)
                            .background(.regularMaterial, in: RoundedRectangle(cornerRadius: 6))
                        }
                }
            }
            .chartYScale(domain: .automatic(includesZero: percent))
            .chartYAxis {
                AxisMarks(position: .leading) { value in
                    AxisGridLine()
                    AxisValueLabel {
                        if let v = value.as(Double.self) { Text(format(v, compact: true)) }
                    }
                }
            }
            .chartLegend(data.count > 1 ? .visible : .hidden)
            .chartXSelection(value: $probe)
            .frame(height: 220)
            .accessibilityLabel(percent
                ? L10n.format("Percent change over %@: %@", range,
                              data.map { "\($0.line.label) \(StockFormat.signed($0.points.last?.value))" }.joined(separator: ", "))
                : L10n.format("%@ price over %@", selected, range))
        }
    }

    private func lowerBound(_ data: [(line: ChartLine, points: [Point])]) -> Double {
        let values = data.flatMap { $0.points.map(\.value) }
        guard let lo = values.min(), let hi = values.max() else { return 0 }
        return lo - (hi - lo) * 0.05
    }

    private func format(_ v: Double, compact: Bool = false) -> String {
        if percent {
            return compact ? "\(v > 0 ? "+" : "")\(v.formatted(.number.precision(.fractionLength(0...1)).locale(L10n.locale)))%" : StockFormat.signed(v)
        }
        return StockFormat.money(v, stock.currency)
    }

    /// The day nearest `date` on the first line, and every line's value on that day.
    private func readings(_ data: [(line: ChartLine, points: [Point])], at date: Date) -> (date: Date, values: [(String, Double)])? {
        guard let first = data.first,
              let nearest = first.points.min(by: { abs($0.date.timeIntervalSince(date)) < abs($1.date.timeIntervalSince(date)) })
        else { return nil }
        let values = data.compactMap { item -> (String, Double)? in
            guard let p = item.points.last(where: { $0.date <= nearest.date }) else { return nil }
            return (item.line.label, p.value)
        }
        return (nearest.date, values)
    }

    // MARK: Returns

    private var returns: some View {
        VStack(alignment: .leading, spacing: 6) {
            CardLabel("Returns")
            ScrollView(.horizontal, showsIndicators: false) {
                Grid(alignment: .trailing, horizontalSpacing: 6, verticalSpacing: 4) {
                    GridRow {
                        Text(verbatim: "")
                        ForEach(StocksCard.returnPeriods, id: \.0) { _, label in
                            Text(label).font(.caption2.weight(.semibold)).foregroundStyle(.secondary)
                        }
                    }
                    ForEach(card.stocks) { s in
                        GridRow {
                            Text(s.symbol).font(.caption.weight(.semibold)).gridColumnAlignment(.leading)
                            ForEach(Array(s.returns.enumerated()), id: \.offset) { _, value in
                                Text(StockFormat.signed(value))
                                    .font(.caption.monospacedDigit())
                                    .foregroundStyle(StockFormat.color(value))
                                    .padding(.horizontal, 5)
                                    .padding(.vertical, 2)
                                    .background(StockFormat.color(value).opacity(value.map { min(1, abs($0) / 40) * 0.25 } ?? 0),
                                                in: RoundedRectangle(cornerRadius: 4))
                            }
                        }
                    }
                }
            }
        }
    }

    // MARK: News

    @ViewBuilder private var news: some View {
        if !card.news.isEmpty || card.newsNote != nil {
            VStack(alignment: .leading, spacing: 8) {
                CardLabel("News")
                let shown = allNews ? card.news : Array(card.news.prefix(4))
                ForEach(shown) { item in
                    Link(destination: item.url) {
                        VStack(alignment: .leading, spacing: 2) {
                            Text(item.title)
                                .font(.subheadline)
                                .foregroundStyle(.primary)
                                .multilineTextAlignment(.leading)
                            Text(newsMeta(item))
                                .font(.caption)
                                .foregroundStyle(.secondary)
                        }
                        .frame(maxWidth: .infinity, alignment: .leading)
                    }
                }
                if card.news.count > shown.count {
                    Button("Show all \(card.news.count) headlines") { withAnimation { allNews = true } }
                        .font(.caption)
                }
                if let note = card.newsNote {
                    Text(note).font(.caption).foregroundStyle(.secondary)
                }
            }
        }
    }

    private func newsMeta(_ item: StocksCard.News) -> String {
        var parts = item.symbols
        if let publisher = item.publisher { parts.append(publisher) }
        if let published = item.published { parts.append(published.formatted(.relative(presentation: .named).locale(L10n.locale))) }
        return parts.joined(separator: " · ")
    }

    // MARK: Footer

    private var footer: some View {
        VStack(alignment: .leading, spacing: 3) {
            ForEach(card.unavailable, id: \.self) { u in
                Text(L10n.labeled(u.symbol, u.reason ?? L10n.string("not available")))
            }
            if let asOf = card.asOf {
                Text("As of \(asOf.formatted(Date.FormatStyle(date: .abbreviated, time: .shortened, locale: L10n.locale)))")
            }
            if let disclaimer = card.disclaimer {
                Text(disclaimer).foregroundStyle(.tertiary)
            }
        }
        .font(.caption2)
        .foregroundStyle(.secondary)
    }
}

/// The selected symbol's rating: the call on a sell–hold–buy gauge, the summary, every factor
/// with its score and reasoning, what would change the view, and the key numbers.
private struct RatingPanel: View {
    let stock: StocksCard.Stock
    let benchmark: String?

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            if let r = stock.rating {
                HStack(alignment: .center, spacing: 10) {
                    CallPill(call: r.call, small: false)
                    VStack(alignment: .leading, spacing: 2) {
                        (Text(stock.symbol).bold() + Text(verbatim: " \(stock.name)"))
                            .font(.subheadline)
                            .lineLimit(1)
                        HStack(spacing: 6) {
                            Text(Self.confidence(r.confidence))
                            if let lean = r.lean { Text(verbatim: "· \(lean)") }
                        }
                        .font(.caption)
                        .foregroundStyle(.secondary)
                    }
                }
                ScoreGauge(rating: r)
                if let summary = r.summary {
                    Text(summary).font(.subheadline)
                }
                if !r.factors.isEmpty {
                    VStack(alignment: .leading, spacing: 10) {
                        CardLabel("Why: the factors behind the call")
                        ForEach(r.factors, id: \.self) { f in
                            VStack(alignment: .leading, spacing: 3) {
                                HStack(alignment: .firstTextBaseline, spacing: 6) {
                                    Text(f.name).font(.subheadline.weight(.semibold))
                                    if let headline = f.headline {
                                        Text(headline).font(.caption).foregroundStyle(.secondary).lineLimit(1)
                                    }
                                    Spacer(minLength: 4)
                                    ScoreBar(score: f.score)
                                }
                                Text(f.detail).font(.caption).foregroundStyle(.secondary)
                                    .fixedSize(horizontal: false, vertical: true)
                            }
                        }
                    }
                }
                if !r.watch.isEmpty {
                    VStack(alignment: .leading, spacing: 4) {
                        CardLabel("What would change the view")
                        ForEach(r.watch, id: \.self) { w in
                            Label(w, systemImage: "eye").font(.caption)
                        }
                    }
                }
            } else {
                (Text(stock.symbol).bold() + Text(verbatim: " ") + Text("\(stock.name): no rating")).font(.subheadline)
            }
            stats
        }
        .padding(12)
        .background(Color(.systemBackground).opacity(0.6), in: RoundedRectangle(cornerRadius: 10))
    }

    /// "High confidence", for the rating's `low`, `medium` or `high`.
    private static func confidence(_ level: String) -> LocalizedStringKey {
        switch level {
        case "high": return "High confidence"
        case "medium": return "Medium confidence"
        default: return "Low confidence"
        }
    }

    private var stats: some View {
        let m = { (v: Double) in StockFormat.money(v, stock.currency) }
        let n = { (v: Double, digits: Int) in v.formatted(.number.precision(.fractionLength(digits)).grouping(.never).locale(L10n.locale)) }
        let i = stock.indicators
        var items: [(String, String)] = []
        if let low = stock.low52, let high = stock.high52 { items.append((L10n.string("52-week range"), "\(m(low)) – \(m(high))")) }
        if let low = stock.dayLow, let high = stock.dayHigh { items.append((L10n.string("Day range"), "\(m(low)) – \(m(high))")) }
        if let v = i.sma50 { items.append((L10n.string("50-day avg"), m(v))) }
        if let v = i.sma200 { items.append((L10n.string("200-day avg"), m(v))) }
        if let v = i.rsi { items.append((L10n.string("RSI (14)"), n(v, 0))) }
        if let v = i.volatility { items.append((L10n.string("Volatility"), L10n.format("%@%% / yr", n(v, 0)))) }
        if let v = i.drawdown { items.append((L10n.string("Max drawdown (1Y)"), StockFormat.signed(v))) }
        if let v = i.beta { items.append((L10n.format("Beta vs %@", benchmark ?? L10n.string("index")), n(v, 2))) }
        return LazyVGrid(columns: [GridItem(.flexible(), alignment: .leading), GridItem(.flexible(), alignment: .leading)],
                         alignment: .leading, spacing: 8) {
            ForEach(items, id: \.0) { label, value in
                VStack(alignment: .leading, spacing: 1) {
                    Text(label).font(.caption2).foregroundStyle(.secondary)
                    Text(value).font(.caption.monospacedDigit().weight(.medium))
                }
            }
        }
    }
}

/// Buy, hold, or sell, in its color.
private struct CallPill: View {
    let call: StocksCard.Call
    let small: Bool

    var body: some View {
        Label(call.label, systemImage: icon)
            .font(small ? .caption2.weight(.bold) : .subheadline.weight(.bold))
            .padding(.horizontal, small ? 5 : 10)
            .padding(.vertical, small ? 2 : 5)
            .foregroundStyle(StockFormat.color(call))
            .background(StockFormat.color(call).opacity(0.15), in: Capsule())
            .labelStyle(TightLabel(small: small))
    }

    private var icon: String {
        switch call {
        case .buy: return "arrow.up.right"
        case .hold: return "pause"
        case .sell: return "arrow.down.right"
        }
    }

    private struct TightLabel: LabelStyle {
        let small: Bool
        func makeBody(configuration: Configuration) -> some View {
            HStack(spacing: small ? 2 : 4) {
                configuration.icon
                configuration.title
            }
        }
    }
}

/// The score on a sell–hold–buy track from -max to +max.
private struct ScoreGauge: View {
    let rating: StocksCard.Rating

    var body: some View {
        VStack(spacing: 3) {
            GeometryReader { geo in
                let span = Double(rating.maxScore * 2)
                let x = { (v: Double) in geo.size.width * (v + Double(rating.maxScore)) / span }
                // A score of 3 or more is a buy and -3 or less a sell, so the zones meet halfway between.
                let edge = min(2.5, Double(rating.maxScore))
                HStack(spacing: 0) {
                    Rectangle().fill(Color.red.opacity(0.3)).frame(width: x(-edge))
                    Rectangle().fill(Color.gray.opacity(0.3)).frame(width: x(edge) - x(-edge))
                    Rectangle().fill(Color.green.opacity(0.3))
                }
                .frame(height: 8)
                .clipShape(Capsule())
                .frame(maxHeight: .infinity)
                .overlay(alignment: .leading) {
                    Circle()
                        .fill(StockFormat.color(rating.call))
                        .overlay(Circle().strokeBorder(.white, lineWidth: 2))
                        .frame(width: 14, height: 14)
                        .offset(x: min(max(x(Double(rating.score)) - 7, 0), geo.size.width - 14))
                }
            }
            .frame(height: 14)
            HStack {
                Text("Sell")
                Spacer()
                Text("Hold")
                Spacer()
                Text("Buy")
            }
            .font(.caption2)
            .foregroundStyle(.secondary)
        }
        .accessibilityElement()
        .accessibilityLabel("Score \(rating.score) on a scale from −\(rating.maxScore), sell, to +\(rating.maxScore), buy")
    }
}

/// A factor's score as a bar growing left (against) or right (for) from the middle.
private struct ScoreBar: View {
    let score: Int

    var body: some View {
        HStack(spacing: 4) {
            ZStack {
                Capsule().fill(Color(.tertiarySystemFill))
                HStack(spacing: 0) {
                    Spacer(minLength: 0)
                    Capsule().fill(score < 0 ? Color.red : .clear).frame(width: score < 0 ? CGFloat(-score) * 12 : 0)
                    Rectangle().fill(Color(.separator)).frame(width: 1)
                    Capsule().fill(score > 0 ? Color.green : .clear).frame(width: score > 0 ? CGFloat(score) * 12 : 0)
                    Spacer(minLength: 0)
                }
            }
            .frame(width: 50, height: 6)
            Text(score > 0 ? "+\(score)" : score < 0 ? "−\(-score)" : "0")
                .font(.caption2.monospacedDigit().weight(.semibold))
                .foregroundStyle(score > 0 ? .green : score < 0 ? .red : .secondary)
                .frame(width: 18, alignment: .trailing)
        }
        .accessibilityElement()
        .accessibilityLabel("Score \(score)")
    }
}

/// Number formats the stock card shares.
enum StockFormat {
    private static let symbols = ["USD": "$", "EUR": "€", "GBP": "£", "JPY": "¥", "INR": "₹", "CNY": "¥", "KRW": "₩"]

    static func currencySymbol(_ currency: String?) -> String {
        currency.flatMap { symbols[$0] } ?? currency ?? L10n.string("Price")
    }

    /// `x` in `currency`, with as many decimals as a price its size needs: the symbol before the
    /// number in English, where the locale puts it otherwise.
    static func money(_ x: Double, _ currency: String?) -> String {
        let digits = abs(x) >= 1000 ? 0 : abs(x) >= 1 ? 2 : 4
        let locale = L10n.locale
        if let currency, currency.count == 3, locale.language.languageCode?.identifier != "en" {
            return x.formatted(.currency(code: currency).precision(.fractionLength(digits)).locale(locale))
        }
        let n = x.formatted(.number.precision(.fractionLength(digits)).locale(locale))
        if let currency, let symbol = symbols[currency] { return "\(symbol)\(n)" }
        return currency.map { "\(n) \($0)" } ?? n
    }

    static func signed(_ x: Double?, digits: Int = 1) -> String {
        guard let x else { return "–" }
        let sign = x > 0 ? "+" : x < 0 ? "−" : ""
        return "\(sign)\(abs(x).formatted(.number.precision(.fractionLength(digits)).grouping(.never).locale(L10n.locale)))%"
    }

    static func color(_ x: Double?) -> Color {
        guard let x, abs(x) >= 0.005 else { return .secondary }
        return x > 0 ? .green : .red
    }

    static func color(_ call: StocksCard.Call) -> Color {
        switch call {
        case .buy: return .green
        case .hold: return .orange
        case .sell: return .red
        }
    }
}
