import SwiftUI

/// A `search_flights` card. For a search: the route, the day and who flies in which cabin; Google's
/// verdict on the prices with their recent history; a row per option (its price, a top pick badge,
/// the airline, the times with the days they move on, the duration, the stops and what to know),
/// which opens to its flights and layovers; and a link to the search on Google Flights. For a
/// booking: the chosen flight(s) as rows, then who sells them for how much, with the fare and bags.
struct FlightsCardView: View {
    let card: FlightsCard
    /// The rows opened to their flights.
    @State private var expanded: Set<Int> = []

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            header
            switch card.kind {
            case .flights:
                if let insights = card.insights {
                    InsightsPanel(insights: insights, currency: card.currency)
                }
                if !card.options.isEmpty {
                    VStack(spacing: 6) {
                        ForEach(card.options) { o in
                            row(o, priceHint: card.leg == .outbound)
                        }
                    }
                }
                if card.more > 0 {
                    Text(L10n.format(card.more == 1 ? "%lld more option" : "%lld more options", card.more))
                        .font(.caption)
                        .foregroundStyle(.secondary)
                }
                if card.noResults {
                    noResults(L10n.string("Google Flights found no flights for this search. Try nearby dates, more airports or more stops."))
                }
            case .booking:
                booking
            }
            footer
        }
        .cardStyle()
    }

    // MARK: Header

    private var header: some View {
        HStack(alignment: .top, spacing: 10) {
            Image(systemName: card.leg == .returning ? "airplane.arrival" : "airplane.departure")
                .foregroundStyle(Color.accentColor)
                .frame(width: 22)
                .padding(.top, 2)
                .accessibilityHidden(true)
            VStack(alignment: .leading, spacing: 2) {
                Text(card.from.isEmpty || card.to.isEmpty ? L10n.string("Flights") : card.route)
                    .font(.headline)
                if let names {
                    Text(names)
                        .font(.caption)
                        .foregroundStyle(.secondary)
                        .lineLimit(2)
                }
                Text(facts)
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }
        }
    }

    /// The airports' names, when the search is between two airports and the result names them.
    private var names: String? {
        guard card.from.count == 1, card.to.count == 1,
              let first = (card.kind == .flights ? card.options : card.chosen).first,
              let from = first.depart.name, let to = first.arrive.name else { return nil }
        return "\(from) → \(to)"
    }

    /// The day, the kind of trip, the passengers and the cabin.
    private var facts: String {
        var parts: [String] = []
        if let date = card.date { parts.append(FlightFormat.day(date)) }
        switch card.leg {
        case .oneWay: parts.append(L10n.string("One way"))
        case .outbound:
            parts.append(card.returnDate.map { L10n.format("Round trip, back %@", FlightFormat.day($0)) } ?? L10n.string("Round trip"))
        case .returning: parts.append(L10n.string("Return flight"))
        }
        parts.append(FlightFormat.passengers(card.passengers))
        if let cabin = card.cabin { parts.append(cabin.label) }
        return parts.joined(separator: " · ")
    }

    // MARK: Options

    private func row(_ o: FlightsCard.Option, priceHint: Bool) -> some View {
        FlightRow(option: o, currency: card.currency, priceHint: priceHint, expanded: expanded.contains(o.id)) {
            withAnimation(.snappy(duration: 0.2)) {
                if expanded.contains(o.id) { expanded.remove(o.id) } else { expanded.insert(o.id) }
            }
        }
    }

    private func noResults(_ text: String) -> some View {
        Label(text, systemImage: "airplane.circle")
            .font(.subheadline)
            .foregroundStyle(.secondary)
            .fixedSize(horizontal: false, vertical: true)
    }

    // MARK: Booking

    @ViewBuilder private var booking: some View {
        if !card.chosen.isEmpty {
            VStack(alignment: .leading, spacing: 6) {
                SectionLabel(text: L10n.string(card.chosen.count == 1 ? "Your flight" : "Your flights"))
                ForEach(card.chosen) { o in
                    row(o, priceHint: false)
                }
            }
        }
        if !card.sellers.isEmpty {
            VStack(alignment: .leading, spacing: 6) {
                SectionLabel(text: L10n.string("Where to book"))
                ForEach(card.sellers) { s in
                    SellerRow(seller: s, currency: card.currency)
                }
            }
        }
        if card.noResults {
            noResults(L10n.string("Google Flights lists no booking options for this choice any more. Search again for fresh prices."))
        }
    }

    // MARK: Footer

    private var footer: some View {
        VStack(alignment: .leading, spacing: 6) {
            if let url = card.googleFlightsURL {
                Link(destination: url) {
                    Label(L10n.string(card.kind == .booking ? "Book on Google Flights" : "Open in Google Flights"),
                          systemImage: "arrow.up.right.square")
                        .font(.subheadline.weight(.semibold))
                }
            }
            VStack(alignment: .leading, spacing: 3) {
                if let asOf = card.asOf {
                    Text(L10n.format("Prices as of %@; they change quickly",
                                     asOf.formatted(Date.FormatStyle(date: .abbreviated, time: .shortened, locale: L10n.locale))))
                } else {
                    Text(L10n.string("Prices change quickly and no seat is held."))
                }
            }
            .font(.caption2)
            .foregroundStyle(.secondary)
        }
    }
}

/// A small heading over a part of the card, in capitals, like `CardLabel` but for text in a variable.
private struct SectionLabel: View {
    let text: String

    var body: some View {
        Text(text)
            .font(.caption2.weight(.semibold))
            .tracking(0.6)
            .textCase(.uppercase)
            .foregroundStyle(.secondary)
    }
}

/// One option: its summary, and when opened its flights and layovers.
private struct FlightRow: View {
    let option: FlightsCard.Option
    let currency: String
    /// A round trip's outbound price covers both ways.
    let priceHint: Bool
    let expanded: Bool
    let toggle: () -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            Button(action: toggle) {
                summary
                    .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .accessibilityHint(L10n.string(expanded ? "Hides the flights" : "Shows the flights and layovers"))
            if expanded {
                Divider()
                details
            }
        }
        .padding(10)
        .background(option.topPick ? Color.accentColor.opacity(0.06) : Color.clear, in: RoundedRectangle(cornerRadius: 10))
        .overlay(RoundedRectangle(cornerRadius: 10)
            .strokeBorder(expanded ? Color.accentColor.opacity(0.6) : Color(.separator).opacity(0.5)))
    }

    private var summary: some View {
        HStack(alignment: .top, spacing: 10) {
            AirlineLogo(url: option.logo, size: 28)
            VStack(alignment: .leading, spacing: 3) {
                FlightTimes(depart: option.depart.time, arrive: option.arrive.time)
                    .font(.subheadline.weight(.semibold).monospacedDigit())
                Text(route)
                    .font(.caption)
                    .foregroundStyle(.secondary)
                if !option.airlines.isEmpty {
                    Text(option.airlines.joined(separator: ", "))
                        .font(.caption)
                        .foregroundStyle(.secondary)
                        .lineLimit(1)
                }
                if let flags {
                    flags.font(.caption2)
                }
            }
            Spacer(minLength: 4)
            VStack(alignment: .trailing, spacing: 3) {
                if option.topPick {
                    Text(L10n.string("Top pick").uppercased(with: L10n.locale))
                        .font(.system(size: 9, weight: .bold))
                        .tracking(0.3)
                        .foregroundStyle(.white)
                        .padding(.horizontal, 6)
                        .padding(.vertical, 2)
                        .background(Color.accentColor, in: Capsule())
                        .lineLimit(1)
                }
                if let price = option.price {
                    Text(FlightFormat.money(price, currency))
                        .font(.subheadline.weight(.semibold).monospacedDigit())
                } else {
                    Text(verbatim: "—")
                        .font(.subheadline.weight(.semibold))
                        .foregroundStyle(.secondary)
                        .accessibilityLabel(L10n.string("Price unavailable"))
                }
                if priceHint, option.price != nil {
                    Text(L10n.string("Price for the round trip"))
                        .font(.caption2)
                        .foregroundStyle(.secondary)
                        .multilineTextAlignment(.trailing)
                }
                Image(systemName: "chevron.down")
                    .font(.caption2.weight(.semibold))
                    .foregroundStyle(.tertiary)
                    .rotationEffect(.degrees(expanded ? 180 : 0))
                    .accessibilityHidden(true)
            }
        }
    }

    /// "SFO – MUC · 10 h 50 min · Nonstop"
    private var route: String {
        var parts = ["\(option.depart.label) – \(option.arrive.label)"]
        if let minutes = option.minutes { parts.append(TripStyle.duration(minutes)) }
        parts.append(FlightFormat.stops(option))
        return parts.joined(separator: " · ")
    }

    /// What to know before picking it: delays, a night flight or stop, the emissions.
    private var flags: Text? {
        var parts: [Text] = []
        if option.oftenDelayed {
            parts.append((Text(Image(systemName: "exclamationmark.triangle")) + Text(verbatim: " " + L10n.string("Often delayed")))
                .foregroundStyle(.orange))
        }
        if option.overnight {
            parts.append((Text(Image(systemName: "moon")) + Text(verbatim: " " + L10n.string("Overnight")))
                .foregroundStyle(.secondary))
        }
        if let pct = option.co2VsTypicalPct {
            let text: String
            if pct < 0 {
                text = L10n.format("%lld%% less CO₂", -pct)
            } else if pct > 0 {
                text = L10n.format("%lld%% more CO₂", pct)
            } else {
                text = L10n.string("Typical CO₂")
            }
            parts.append((Text(Image(systemName: "leaf")) + Text(verbatim: " " + text))
                .foregroundStyle(pct < 0 ? Color.green : Color.secondary))
        }
        guard var out = parts.first else { return nil }
        for part in parts.dropFirst() {
            out = out + Text(verbatim: "  ") + part
        }
        return out
    }

    private var details: some View {
        VStack(alignment: .leading, spacing: 10) {
            ForEach(Array(option.segments.enumerated()), id: \.offset) { index, segment in
                SegmentView(segment: segment)
                if index < option.segments.count - 1, index < option.layovers.count {
                    LayoverView(layover: option.layovers[index])
                }
            }
            if let kg = option.co2Kg {
                Label(L10n.format("%lld kg CO₂ per passenger", kg), systemImage: "leaf")
                    .font(.caption2)
                    .foregroundStyle(.secondary)
            }
        }
    }
}

/// "15:40 → 11:30 +1", the clocks as the airports show them.
private struct FlightTimes: View {
    let depart: FlightsCard.LocalTime?
    let arrive: FlightsCard.LocalTime?

    private var offset: Int {
        guard let depart, let arrive else { return 0 }
        return arrive.days(after: depart)
    }

    var body: some View {
        HStack(alignment: .firstTextBaseline, spacing: 4) {
            Text(verbatim: depart?.clock ?? "–")
            Text(verbatim: "→").foregroundStyle(.secondary)
            Text(verbatim: arrive?.clock ?? "–")
            if offset != 0 {
                Text(verbatim: offset > 0 ? "+\(offset)" : "−\(-offset)")
                    .font(.caption2.weight(.bold))
                    .foregroundStyle(.orange)
                    .baselineOffset(5)
                    .accessibilityLabel(L10n.format(abs(offset) == 1 ? "%lld day later" : "%lld days later", offset))
            }
        }
    }
}

/// One flight of an option: its number and airline, times and airports, and the plane.
private struct SegmentView: View {
    let segment: FlightsCard.Segment

    var body: some View {
        HStack(alignment: .top, spacing: 8) {
            AirlineLogo(url: segment.logo, size: 18)
            VStack(alignment: .leading, spacing: 2) {
                let title = [segment.flight, segment.airline].compactMap { $0 }.joined(separator: " · ")
                if !title.isEmpty {
                    Text(title).font(.subheadline.weight(.medium))
                }
                HStack(alignment: .firstTextBaseline, spacing: 4) {
                    Text(verbatim: endpoint(segment.from))
                    Text(verbatim: "→").foregroundStyle(.secondary)
                    Text(verbatim: endpoint(segment.to))
                    if let offset = dayOffset {
                        Text(verbatim: offset)
                            .font(.caption2.weight(.bold))
                            .foregroundStyle(.orange)
                            .baselineOffset(5)
                    }
                }
                .font(.caption.monospacedDigit())
                if let from = segment.from.name, let to = segment.to.name {
                    Text(verbatim: "\(from) → \(to)")
                        .font(.caption2)
                        .foregroundStyle(.secondary)
                        .lineLimit(2)
                }
                let facts = [
                    segment.minutes.map(TripStyle.duration), segment.aircraft, segment.cabin,
                    segment.legroom.map { L10n.labeled(L10n.string("Legroom"), $0) },
                ].compactMap { $0 }
                if !facts.isEmpty {
                    Text(facts.joined(separator: " · "))
                        .font(.caption2)
                        .foregroundStyle(.secondary)
                }
                if segment.oftenDelayed || segment.overnight {
                    HStack(spacing: 8) {
                        if segment.oftenDelayed {
                            Label(L10n.string("Often delayed"), systemImage: "exclamationmark.triangle")
                                .foregroundStyle(.orange)
                        }
                        if segment.overnight {
                            Label(L10n.string("Overnight"), systemImage: "moon")
                                .foregroundStyle(.secondary)
                        }
                    }
                    .font(.caption2)
                }
            }
        }
    }

    /// "15:40 SFO"
    private func endpoint(_ airport: FlightsCard.Airport) -> String {
        airport.time.map { "\($0.clock) \(airport.label)" } ?? airport.label
    }

    /// "+1" when the flight lands on a later local day.
    private var dayOffset: String? {
        guard let from = segment.from.time, let to = segment.to.time else { return nil }
        let days = to.days(after: from)
        if days == 0 { return nil }
        return days > 0 ? "+\(days)" : "−\(-days)"
    }
}

/// The wait between two flights: how long, and where.
private struct LayoverView: View {
    let layover: FlightsCard.Layover

    private var place: String {
        if layover.airport.isEmpty { return layover.name ?? "?" }
        return layover.name.map { "\(layover.airport) (\($0))" } ?? layover.airport
    }

    var body: some View {
        HStack(alignment: .top, spacing: 8) {
            Image(systemName: "clock")
                .frame(width: 18)
                .foregroundStyle(.secondary)
            VStack(alignment: .leading, spacing: 2) {
                Text(layover.minutes.map { L10n.format("%@ layover in %@", TripStyle.duration($0), place) }
                     ?? L10n.format("Layover in %@", place))
                if layover.overnight {
                    Label(L10n.string("Overnight"), systemImage: "moon")
                        .foregroundStyle(.secondary)
                }
            }
        }
        .font(.caption)
        .padding(.vertical, 6)
        .padding(.horizontal, 8)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(Color(.tertiarySystemFill), in: RoundedRectangle(cornerRadius: 8))
    }
}

/// Who sells the chosen flight(s), for how much, with which fare and bags.
private struct SellerRow: View {
    let seller: FlightsCard.Seller
    let currency: String

    var body: some View {
        VStack(alignment: .leading, spacing: 3) {
            HStack(alignment: .firstTextBaseline) {
                Text(seller.name).font(.subheadline.weight(.medium))
                Spacer(minLength: 4)
                if let price = seller.price {
                    Text(FlightFormat.money(price, currency))
                        .font(.subheadline.weight(.semibold).monospacedDigit())
                } else {
                    Text(verbatim: "—")
                        .font(.subheadline.weight(.semibold))
                        .foregroundStyle(.secondary)
                        .accessibilityLabel(L10n.string("Price unavailable"))
                }
            }
            switch seller.part {
            case .together:
                EmptyView()
            case .departing:
                Text(L10n.string("Separate ticket: departing")).font(.caption).foregroundStyle(.orange)
            case .returning:
                Text(L10n.string("Separate ticket: returning")).font(.caption).foregroundStyle(.orange)
            }
            if let fare = seller.fare {
                Text(fare).font(.caption).foregroundStyle(.secondary)
            }
            ForEach(Array(seller.bags.enumerated()), id: \.offset) { _, bag in
                Label(bag, systemImage: "suitcase")
                    .font(.caption2)
                    .foregroundStyle(.secondary)
            }
            if !seller.marketedAs.isEmpty {
                Text(L10n.format("Sold as %@", seller.marketedAs.joined(separator: ", ")))
                    .font(.caption2)
                    .foregroundStyle(.tertiary)
            }
        }
        .padding(10)
        .frame(maxWidth: .infinity, alignment: .leading)
        .overlay(RoundedRectangle(cornerRadius: 10).strokeBorder(Color(.separator).opacity(0.5)))
    }
}

/// Google's verdict on the prices, the typical range, and a line of how they moved.
private struct InsightsPanel: View {
    let insights: FlightsCard.Insights
    let currency: String

    private var color: Color {
        switch insights.level {
        case "low"?: return .green
        case "high"?: return .orange
        default: return .accentColor
        }
    }

    private var text: String {
        let verdict: String
        switch insights.level {
        case "low"?: verdict = L10n.string("Prices are low for this route")
        case "typical"?: verdict = L10n.string("Prices are typical for this route")
        case "high"?: verdict = L10n.string("Prices are high for this route")
        case let level?: verdict = L10n.format("Prices are %@ for this route", level)
        case nil: verdict = L10n.labeled(L10n.string("Lowest price"), FlightFormat.money(insights.lowest, currency))
        }
        guard let low = insights.typicalLow, let high = insights.typicalHigh else { return verdict }
        let range = L10n.format("typical %@–%@", FlightFormat.money(low, currency), FlightFormat.money(high, currency))
        return "\(verdict) — \(range)"
    }

    var body: some View {
        HStack(alignment: .center, spacing: 10) {
            Image(systemName: "chart.line.uptrend.xyaxis")
                .foregroundStyle(color)
                .accessibilityHidden(true)
            Text(text)
                .font(.subheadline)
                .fixedSize(horizontal: false, vertical: true)
            Spacer(minLength: 4)
            if insights.history.count >= 2 {
                Sparkline(values: insights.history.map(\.price), color: color)
                    .frame(width: 72, height: 26)
            }
        }
        .padding(10)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(color.opacity(0.1), in: RoundedRectangle(cornerRadius: 10))
    }
}

/// A small line of values, oldest first.
private struct Sparkline: View {
    let values: [Double]
    let color: Color

    var body: some View {
        GeometryReader { geo in
            Path { path in
                guard values.count > 1, let lo = values.min(), let hi = values.max() else { return }
                let span = hi > lo ? hi - lo : 1
                for (i, v) in values.enumerated() {
                    let x = geo.size.width * CGFloat(i) / CGFloat(values.count - 1)
                    let y = geo.size.height * (1 - CGFloat((v - lo) / span))
                    if i == 0 {
                        path.move(to: CGPoint(x: x, y: y))
                    } else {
                        path.addLine(to: CGPoint(x: x, y: y))
                    }
                }
            }
            .stroke(color, style: StrokeStyle(lineWidth: 1.5, lineCap: .round, lineJoin: .round))
        }
        .accessibilityHidden(true)
    }
}

/// An airline's logo (https only, as the parser keeps it), or a plane while there is none.
private struct AirlineLogo: View {
    let url: URL?
    let size: CGFloat

    var body: some View {
        Group {
            if let url {
                AsyncImage(url: url) { phase in
                    switch phase {
                    case .success(let image):
                        image.resizable().scaledToFit()
                    default:
                        placeholder
                    }
                }
                .id(url)
            } else {
                placeholder
            }
        }
        .frame(width: size, height: size)
        .accessibilityHidden(true)
    }

    private var placeholder: some View {
        Image(systemName: "airplane")
            .font(.system(size: size * 0.5))
            .foregroundStyle(.secondary)
            .frame(width: size, height: size)
            .background(Color(.tertiarySystemFill), in: RoundedRectangle(cornerRadius: 6))
    }
}

/// Words and formats the flights card uses.
enum FlightFormat {
    /// `x` in `currency`, an ISO code, without cents when it is whole.
    static func money(_ x: Double, _ currency: String) -> String {
        x.formatted(.currency(code: currency).precision(.fractionLength(0...2)).locale(L10n.locale))
    }

    /// A `yyyy-MM-dd` day as parsed (midnight UTC), "Tue, Dec 1, 2026".
    static func day(_ date: Date) -> String {
        var style = Date.FormatStyle(date: .abbreviated, time: .omitted)
        style.locale = L10n.locale
        style.timeZone = TimeZone(secondsFromGMT: 0) ?? .current
        return date.formatted(style.weekday(.abbreviated))
    }

    /// "2 adults, 1 child"
    static func passengers(_ p: FlightsCard.Passengers) -> String {
        var parts: [String] = []
        func add(_ n: Int, _ one: String, _ many: String) {
            if n > 0 { parts.append(L10n.format(n == 1 ? one : many, n)) }
        }
        add(p.adults, "%lld adult", "%lld adults")
        add(p.children, "%lld child", "%lld children")
        add(p.infantsInSeat, "%lld infant with a seat", "%lld infants with a seat")
        add(p.infantsOnLap, "%lld lap infant", "%lld lap infants")
        return parts.joined(separator: ", ")
    }

    /// "Nonstop", "1 stop (EWR)", "2 stops (EWR, FRA)"
    static func stops(_ o: FlightsCard.Option) -> String {
        if o.stops == 0 { return L10n.string("Nonstop") }
        let airports = o.stopAirports
        if airports.isEmpty {
            return L10n.format(o.stops == 1 ? "%lld stop on the way" : "%lld stops on the way", o.stops)
        }
        return L10n.format(o.stops == 1 ? "%lld stop (%@)" : "%lld stops (%@)", o.stops, airports.joined(separator: ", "))
    }
}
