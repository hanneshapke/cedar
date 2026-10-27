import MapKit
import SwiftUI

/// A `plan_trip` card: the recommendation, a map of the chosen option's route, the options to
/// choose from, and the chosen one's steps (for transit, each walk and ride with its line).
struct TripCardView: View {
    let card: TripCard
    @State private var selected: Int

    init(card: TripCard) {
        self.card = card
        _selected = State(initialValue: (card.recommendedOption ?? card.options[0]).id)
    }

    private var option: TripCard.Option { card.options.first { $0.id == selected } ?? card.options[0] }
    private var route: String { "\(card.from.label) → \(card.to.label)" }

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            VStack(alignment: .leading, spacing: 2) {
                Text(card.title ?? route).font(.headline)
                Text(card.title == nil ? when : "\(route) · \(when)")
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }
            if let rec = card.recommendedOption {
                HStack(alignment: .top, spacing: 10) {
                    Image(systemName: TripStyle.icon(rec.mode))
                        .foregroundStyle(Color.accentColor)
                        .frame(width: 22)
                    VStack(alignment: .leading, spacing: 2) {
                        Text(rec.mode.take(TripStyle.duration(rec.minutes)))
                            .font(.subheadline.weight(.semibold))
                        if let reason = card.reason {
                            Text(reason).font(.caption).foregroundStyle(.secondary)
                        }
                    }
                }
                .padding(10)
                .frame(maxWidth: .infinity, alignment: .leading)
                .background(Color.accentColor.opacity(0.1), in: RoundedRectangle(cornerRadius: 10))
            }

            CardMap(title: card.title ?? route) {
                Marker(card.from.label, monogram: Text(verbatim: "A"), coordinate: card.from.location).tint(.green)
                Marker(card.to.label, monogram: Text(verbatim: "B"), coordinate: card.to.location).tint(.red)
                ForEach(Array(lines.enumerated()), id: \.offset) { _, line in
                    MapPolyline(coordinates: line.points.map(\.location))
                        .stroke(line.color, style: StrokeStyle(lineWidth: line.walk ? 3 : 5, lineCap: .round,
                                                               lineJoin: .round, dash: line.walk ? [1, 7] : []))
                }
            }
            .id(selected)

            VStack(spacing: 6) {
                ForEach(card.options) { o in
                    optionRow(o)
                }
            }

            TripSteps(card: card, option: option)

            if !card.unavailable.isEmpty || card.attribution != nil {
                VStack(alignment: .leading, spacing: 3) {
                    ForEach(card.unavailable, id: \.self) { u in
                        Text(L10n.labeled(u.mode.label, u.reason ?? L10n.string("not available")))
                    }
                    if let attribution = card.attribution {
                        Text(attribution).foregroundStyle(.tertiary)
                    }
                }
                .font(.caption2)
                .foregroundStyle(.secondary)
            }
        }
        .cardStyle()
    }

    private var when: String {
        if let arriveBy = card.arriveBy { return L10n.format("Arrive by %@", TripStyle.clock(arriveBy, in: card, weekday: true)) }
        if let departAt = card.departAt { return L10n.format("Leave %@", TripStyle.clock(departAt, in: card, weekday: true)) }
        return L10n.string("Leaving now")
    }

    // MARK: The map

    private struct Line {
        let points: [Coordinate]
        let walk: Bool
        let color: Color
    }

    /// The lines to draw for the option: each transit leg in its color, walks dashed, a car route
    /// solid, or a dashed straight line when there is no geometry.
    private var lines: [Line] {
        let o = option
        var drawn: [Line] = []
        if o.mode == .transit {
            drawn = o.legs.filter { $0.path.count > 1 }.map { leg in
                let walk = leg.kind == .walk
                return Line(points: leg.path, walk: walk,
                            color: walk ? .gray : leg.color.flatMap(Color.init(hex:)) ?? .accentColor)
            }
        } else {
            // A rideshare drives the car's route.
            let path = o.path.count > 1 ? o.path : (card.options.first { $0.mode == .drive }?.path ?? [])
            if path.count > 1 {
                drawn = [Line(points: path, walk: o.mode == .walk, color: o.mode == .walk ? .gray : .accentColor)]
            }
        }
        if drawn.isEmpty {
            drawn = [Line(points: [card.from.coordinate, card.to.coordinate], walk: true, color: .accentColor)]
        }
        return drawn
    }

    // MARK: Options

    private func optionRow(_ o: TripCard.Option) -> some View {
        let isSelected = o.id == selected
        return Button {
            withAnimation(.snappy(duration: 0.2)) { selected = o.id }
        } label: {
            HStack(spacing: 10) {
                Image(systemName: TripStyle.icon(o.mode))
                    .frame(width: 22)
                    .foregroundStyle(isSelected ? Color.accentColor : .secondary)
                VStack(alignment: .leading, spacing: 4) {
                    HStack(spacing: 6) {
                        Text(o.mode.label).font(.subheadline.weight(.medium))
                        if o.id == card.recommended {
                            tag("Recommended", prominent: true)
                        } else if o.id == card.fastest?.id {
                            tag("Fastest", prominent: false)
                        }
                    }
                    let rides = o.legs.filter { $0.kind != .walk }.prefix(5)
                    if !rides.isEmpty {
                        HStack(spacing: 3) {
                            ForEach(Array(rides.enumerated()), id: \.offset) { index, leg in
                                if index > 0 {
                                    Image(systemName: "chevron.right").font(.system(size: 8, weight: .bold)).foregroundStyle(.tertiary)
                                }
                                LineBadge(leg: leg)
                            }
                        }
                    }
                    let facts = TripStyle.facts(o, in: card)
                    if !facts.isEmpty {
                        Text(facts).font(.caption).foregroundStyle(.secondary)
                    }
                }
                Spacer(minLength: 4)
                Text(TripStyle.duration(o.minutes))
                    .font(.subheadline.weight(.semibold).monospacedDigit())
            }
            .padding(10)
            .contentShape(Rectangle())
            .background(isSelected ? Color.accentColor.opacity(0.08) : Color.clear, in: RoundedRectangle(cornerRadius: 10))
            .overlay(RoundedRectangle(cornerRadius: 10)
                .strokeBorder(isSelected ? Color.accentColor.opacity(0.6) : Color(.separator).opacity(0.5)))
        }
        .buttonStyle(.plain)
        .accessibilityAddTraits(isSelected ? .isSelected : [])
    }

    private func tag(_ text: LocalizedStringKey, prominent: Bool) -> some View {
        Text(text)
            .font(.caption2.weight(.semibold))
            .padding(.horizontal, 6)
            .padding(.vertical, 2)
            .foregroundStyle(prominent ? Color.white : Color.accentColor)
            .background(prominent ? Color.accentColor : Color.accentColor.opacity(0.12), in: Capsule())
    }
}

/// A transit line: its icon and name, on the line's color when it has one.
private struct LineBadge: View {
    let leg: TripCard.Leg

    var body: some View {
        HStack(spacing: 3) {
            Image(systemName: TripStyle.icon(leg.kind))
            if let route = leg.route {
                Text(route)
            }
        }
        .font(.caption2.weight(.bold))
        .padding(.horizontal, 5)
        .padding(.vertical, 2)
        .foregroundStyle(leg.color.map(Color.ink(on:)) ?? .primary)
        .background(leg.color.flatMap(Color.init(hex:)) ?? Color(.tertiarySystemFill), in: RoundedRectangle(cornerRadius: 4))
    }
}

/// The chosen option's steps as a timeline: for transit each leg, otherwise one line with the totals.
private struct TripSteps: View {
    let card: TripCard
    let option: TripCard.Option

    private struct Step {
        enum Kind { case walk, ride, end, other }
        let kind: Kind
        var leg: TripCard.Leg?
        let main: String
        var sub: String?
        let at: Date?
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            ForEach(Array(steps.enumerated()), id: \.offset) { index, step in
                HStack(alignment: .top, spacing: 8) {
                    Text(step.at.map { TripStyle.clock($0, in: card) } ?? "")
                        .font(.caption.monospacedDigit())
                        .foregroundStyle(.secondary)
                        .frame(width: 62, alignment: .trailing)
                    VStack(spacing: 0) {
                        Circle()
                            .fill(dotColor(step))
                            .frame(width: 10, height: 10)
                            .padding(.top, 3)
                        if index < steps.count - 1 {
                            Rectangle()
                                .fill(lineColor(step))
                                .frame(width: step.kind == .ride ? 4 : 2)
                                .frame(maxHeight: .infinity)
                        }
                    }
                    .frame(width: 12)
                    VStack(alignment: .leading, spacing: 3) {
                        HStack(spacing: 6) {
                            if let leg = step.leg { LineBadge(leg: leg) }
                            Text(step.main).font(.subheadline.weight(step.kind == .end ? .semibold : .regular))
                        }
                        if let sub = step.sub, !sub.isEmpty {
                            Text(sub).font(.caption).foregroundStyle(.secondary)
                        }
                    }
                    .padding(.bottom, 12)
                }
            }
        }
    }

    private func dotColor(_ step: Step) -> Color {
        if step.kind == .end { return .red }
        return step.leg?.color.flatMap(Color.init(hex:)) ?? (step.kind == .walk ? .gray : .accentColor)
    }

    private func lineColor(_ step: Step) -> Color {
        if step.kind == .walk { return Color.gray.opacity(0.4) }
        return step.leg?.color.flatMap(Color.init(hex:)) ?? Color.accentColor.opacity(0.6)
    }

    private var steps: [Step] {
        let o = option
        var out: [Step] = []
        if o.mode != .transit || o.legs.isEmpty {
            let goTo = [TripCard.Mode.walk: "Walk to %@", .drive: "Drive to %@", .rideshare: "Ride to %@"][o.mode] ?? "Go to %@"
            let sub = [TripStyle.duration(o.minutes), TripStyle.distance(km: o.km, meters: o.meters), o.note]
                .compactMap { $0 }.filter { !$0.isEmpty }.joined(separator: " · ")
            out.append(Step(kind: o.mode == .walk ? .walk : .other, main: L10n.format(goTo, card.to.label), sub: sub, at: o.depart))
        } else {
            for (index, leg) in o.legs.enumerated() {
                let to = leg.to.flatMap { $0 == "END" ? nil : $0 } ?? card.to.label
                if leg.kind == .walk {
                    let far = TripStyle.distance(km: nil, meters: leg.meters).map { " · \($0)" } ?? ""
                    out.append(Step(kind: .walk, main: L10n.format("Walk to %@", to), sub: "\(TripStyle.duration(leg.minutes))\(far)", at: leg.depart))
                    continue
                }
                let from = leg.from.flatMap { $0 == "START" ? nil : $0 } ?? card.from.label
                let stops = leg.stops.flatMap { $0 > 0 ? L10n.format($0 == 1 ? "%lld stop" : "%lld stops", $0) : nil }
                let sub = ["\(from) → \(to)", stops, TripStyle.duration(leg.minutes), leg.agency]
                    .compactMap { $0 }.joined(separator: " · ")
                let main = leg.headsign.map { L10n.format("%@ towards %@", leg.kind.label, $0) } ?? leg.kind.label
                out.append(Step(kind: .ride, leg: leg, main: main, sub: sub, at: leg.depart))
                if index == o.legs.count - 1, let arrive = leg.arrive {
                    out.append(Step(kind: .end, main: L10n.format("Arrive at %@", to), at: arrive))
                }
            }
        }
        if o.mode != .transit || o.legs.last?.kind == .walk, let arrive = o.arrive {
            out.append(Step(kind: .end, main: L10n.format("Arrive at %@", card.to.label), at: arrive))
        }
        return out
    }
}

/// Words, icons and formats the trip card shares.
enum TripStyle {
    static func icon(_ mode: TripCard.Mode) -> String {
        switch mode {
        case .transit: return "tram.fill"
        case .walk: return "figure.walk"
        case .drive: return "car.fill"
        case .rideshare: return "car.side.fill"
        }
    }

    static func icon(_ kind: TripCard.LegKind) -> String {
        switch kind {
        case .walk: return "figure.walk"
        case .bike: return "bicycle"
        case .drive: return "car.fill"
        case .bus: return "bus.fill"
        case .tram: return "tram.fill"
        case .subway: return "tram.fill.tunnel"
        case .ferry: return "ferry.fill"
        case .plane: return "airplane"
        case .train: return "train.side.front.car"
        case .other: return "circle.fill"
        }
    }

    static func duration(_ minutes: Int) -> String {
        if minutes < 60 { return "\(minutes) min" }
        let h = minutes / 60, m = minutes % 60
        return m == 0 ? "\(h) h" : "\(h) h \(m) min"
    }

    static func distance(km: Double?, meters: Double?) -> String? {
        if let km { return "\(km.formatted(.number.precision(.fractionLength(0...1)).locale(L10n.locale))) km" }
        guard let meters else { return nil }
        return meters >= 1000
            ? "\((meters / 1000).formatted(.number.precision(.fractionLength(1)).grouping(.never).locale(L10n.locale))) km"
            : "\(Int(meters.rounded())) m"
    }

    /// An estimate, "~$15–22": the symbol before the numbers in English, as the locale has it otherwise.
    static func money(_ cost: TripCard.Cost) -> String {
        let locale = L10n.locale
        let number = FloatingPointFormatStyle<Double>(locale: locale).precision(.fractionLength(0...2))
        if locale.language.languageCode?.identifier != "en" {
            let currency = FloatingPointFormatStyle<Double>.Currency(code: cost.currency, locale: locale).precision(.fractionLength(0...2))
            return cost.low == cost.high
                ? "~\(cost.low.formatted(currency))"
                : "~\(cost.low.formatted(number))–\(cost.high.formatted(currency))"
        }
        let symbol = ["USD": "$", "EUR": "€", "GBP": "£"][cost.currency] ?? "\(cost.currency) "
        let low = cost.low.formatted(number)
        let high = cost.high.formatted(number)
        return cost.low == cost.high ? "~\(symbol)\(low)" : "~\(symbol)\(low)–\(high)"
    }

    /// A time of day in the trip's time zone when the planner gave one, else the phone's.
    static func clock(_ date: Date, in card: TripCard, weekday: Bool = false) -> String {
        var style = Date.FormatStyle(date: .omitted, time: .shortened)
        style.locale = L10n.locale
        style.timeZone = card.timeZone ?? .current
        return date.formatted(weekday ? style.weekday(.abbreviated) : style)
    }

    /// The facts under an option's name: its times, transfers, distance or price.
    static func facts(_ o: TripCard.Option, in card: TripCard) -> String {
        var out: [String] = []
        if let depart = o.depart, let arrive = o.arrive {
            out.append("\(clock(depart, in: card)) – \(clock(arrive, in: card))")
        }
        if o.mode == .transit {
            if let transfers = o.transfers {
                out.append(transfers == 0 ? L10n.string("direct") : L10n.format(transfers == 1 ? "%lld transfer" : "%lld transfers", transfers))
            }
            if let walk = o.walkMinutes, walk > 0 { out.append(L10n.format("%lld min walk", walk)) }
        } else if let distance = distance(km: o.km, meters: o.meters) {
            out.append(distance)
        }
        if let cost = o.cost { out.append(money(cost)) }
        return out.joined(separator: " · ")
    }
}
