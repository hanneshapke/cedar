import SwiftUI

/// A `weather_forecast` card: the place and its local time, the current conditions with a large
/// icon (a moon at night), the temperature with feels like and the day's high and low, tiles for
/// humidity, wind with its direction, gusts, precipitation, UV and daylight, and a row per day with
/// its icon, chance of rain and a low-to-high temperature bar on a shared scale. A day row opens to
/// show its details. The Open-Meteo credit its CC BY 4.0 licence asks for closes the card.
struct WeatherCardView: View {
    let card: WeatherCard
    /// The day rows opened to their details.
    @State private var expanded: Set<String> = []

    private var night: Bool { card.isNight }
    private var today: WeatherCard.Day? { card.todayDay }

    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            header
            hero
            stats
            forecast
            footer
        }
        .cardStyle()
        .accessibilityElement(children: .contain)
        .accessibilityLabel(L10n.format("Weather for %@", card.placeName))
    }

    // MARK: Now

    private var header: some View {
        VStack(alignment: .leading, spacing: 2) {
            Text(card.placeName)
                .font(.headline)
                .accessibilityAddTraits(.isHeader)
            let sub = [card.region, card.now?.time.map { L10n.format("%@ local time", $0) }].compactMap { $0 }
            if !sub.isEmpty {
                Text(sub.joined(separator: " · "))
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }
        }
    }

    private var hero: some View {
        HStack(alignment: .center, spacing: 14) {
            WeatherIcon(code: card.leadCode, night: night, size: 54)
            VStack(alignment: .leading, spacing: 2) {
                if let temp = card.now?.temp {
                    HStack(alignment: .top, spacing: 1) {
                        Text(WeatherFormat.whole(temp.value))
                            .font(.system(size: 46, weight: .light, design: .rounded))
                        Text(temp.unit.isEmpty ? "°" : temp.unit)
                            .font(.title3.weight(.medium))
                            .foregroundStyle(.secondary)
                            .padding(.top, 6)
                    }
                    .accessibilityElement(children: .combine)
                }
                if let code = card.leadCode {
                    Text(WeatherFormat.condition(code))
                        .font(.subheadline.weight(.medium))
                }
                let range = [
                    card.now?.feels.map { L10n.format("Feels like %@", WeatherFormat.degrees($0)) },
                    (today?.high).flatMap { high in today?.low.map { low in L10n.format("H %@ · L %@", WeatherFormat.degrees(high), WeatherFormat.degrees(low)) } },
                ].compactMap { $0 }
                if !range.isEmpty {
                    Text(range.joined(separator: " · "))
                        .font(.caption)
                        .foregroundStyle(.secondary)
                }
            }
        }
    }

    /// Humidity, wind, gusts, precipitation, UV and daylight: whichever the report has.
    @ViewBuilder private var stats: some View {
        let tiles = statTiles
        if !tiles.isEmpty {
            LazyVGrid(columns: [GridItem(.adaptive(minimum: 96), spacing: 8)], alignment: .leading, spacing: 8) {
                ForEach(tiles) { tile in
                    StatTile(tile: tile)
                }
            }
        }
    }

    private var statTiles: [StatTile.Item] {
        var tiles: [StatTile.Item] = []
        let now = card.now
        if let humidity = now?.humidity { tiles.append(.init(label: L10n.string("Humidity"), value: WeatherFormat.amount(humidity, digits: 0))) }
        if let wind = now?.wind {
            let label = now?.from.map { L10n.format("Wind from %@", $0) } ?? L10n.string("Wind")
            tiles.append(.init(label: label, value: WeatherFormat.amount(wind), windFrom: now?.from))
        }
        if let gusts = now?.gusts { tiles.append(.init(label: L10n.string("Gusts"), value: WeatherFormat.amount(gusts))) }
        if let precip = now?.precip { tiles.append(.init(label: L10n.string("Precipitation"), value: WeatherFormat.amount(precip, digits: 2))) }
        if let uv = today?.uv { tiles.append(.init(label: L10n.string("UV index"), value: WeatherFormat.number(uv.value, digits: 1))) }
        tiles.append(contentsOf: WeatherFormat.daylight(today))
        return tiles
    }

    // MARK: Days

    @ViewBuilder private var forecast: some View {
        let days = card.forecastDays
        if !days.isEmpty {
            let scale = TemperatureScale(days: days, now: card.now?.temp)
            let anyChance = days.contains { $0.chance != nil }
            let title: LocalizedStringKey = days.count == 1 ? "1-day forecast" : "\(days.count)-day forecast"
            VStack(alignment: .leading, spacing: 4) {
                CardLabel(title)
                    .padding(.bottom, 4)
                ForEach(Array(days.enumerated()), id: \.element.id) { index, day in
                    dayRow(day, index: index, scale: scale, anyChance: anyChance)
                    if index < days.count - 1 { Divider() }
                }
            }
        }
    }

    private func dayRow(_ day: WeatherCard.Day, index: Int, scale: TemperatureScale, anyChance: Bool) -> some View {
        let open = expanded.contains(day.id)
        let names = WeatherFormat.dayNames(day.date, index: index, today: card.today)
        return VStack(alignment: .leading, spacing: 8) {
            Button {
                withAnimation(.snappy(duration: 0.2)) {
                    if open { expanded.remove(day.id) } else { expanded.insert(day.id) }
                }
            } label: {
                HStack(spacing: 8) {
                    VStack(alignment: .leading, spacing: 0) {
                        Text(names.name).font(.subheadline.weight(.medium))
                        Text(names.short).font(.caption2).foregroundStyle(.secondary)
                    }
                    .frame(width: 64, alignment: .leading)
                    WeatherIcon(code: day.code, night: false, size: 22)
                        .frame(width: 26)
                    if anyChance {
                        Text(day.chance.map { $0.value >= 10 ? WeatherFormat.amount($0, digits: 0) : "" } ?? "")
                            .font(.caption)
                            .foregroundStyle(Color(hue: 210 / 360, saturation: 0.7, brightness: 0.75))
                            .frame(width: 38, alignment: .trailing)
                    }
                    Text(WeatherFormat.degrees(day.low))
                        .font(.subheadline)
                        .foregroundStyle(.secondary)
                        .frame(width: 36, alignment: .trailing)
                    TemperatureBar(day: day, scale: scale, now: day.date == card.today ? card.now?.temp : nil)
                    Text(WeatherFormat.degrees(day.high))
                        .font(.subheadline.weight(.medium))
                        .frame(width: 36, alignment: .leading)
                    Image(systemName: "chevron.right")
                        .font(.caption2.weight(.semibold))
                        .foregroundStyle(.tertiary)
                        .rotationEffect(.degrees(open ? 90 : 0))
                }
                .contentShape(Rectangle())
                .padding(.vertical, 6)
            }
            .buttonStyle(.plain)
            .accessibilityLabel(dayLabel(day, names: names))
            .accessibilityAddTraits(open ? [.isSelected] : [])
            if open {
                LazyVGrid(columns: [GridItem(.adaptive(minimum: 96), spacing: 8)], alignment: .leading, spacing: 8) {
                    ForEach(dayTiles(day)) { tile in
                        StatTile(tile: tile)
                    }
                }
                .padding(.bottom, 8)
            }
        }
    }

    private func dayLabel(_ day: WeatherCard.Day, names: WeatherFormat.DayNames) -> String {
        var parts = [names.long]
        if let code = day.code { parts.append(WeatherFormat.condition(code)) }
        parts.append("\(WeatherFormat.degrees(day.low)) – \(WeatherFormat.degrees(day.high))")
        return parts.joined(separator: ", ")
    }

    private func dayTiles(_ day: WeatherCard.Day) -> [StatTile.Item] {
        var tiles: [StatTile.Item] = []
        tiles.append(.init(label: L10n.string("Conditions"), value: day.code.map(WeatherFormat.condition) ?? "–"))
        if let precip = day.precip {
            let amount = WeatherFormat.amount(precip, digits: 2)
            let value = day.chance.map { L10n.format("%@ (%@ chance)", amount, WeatherFormat.amount($0, digits: 0)) } ?? amount
            tiles.append(.init(label: L10n.string("Precipitation"), value: value))
        }
        if let wind = day.wind {
            let value = day.gusts.map { L10n.format("up to %@, gusts %@", WeatherFormat.amount(wind), WeatherFormat.amount($0)) } ?? WeatherFormat.amount(wind)
            tiles.append(.init(label: L10n.string("Wind"), value: value))
        }
        if let uv = day.uv { tiles.append(.init(label: L10n.string("UV index"), value: WeatherFormat.number(uv.value, digits: 1))) }
        tiles.append(contentsOf: WeatherFormat.daylight(day))
        return tiles
    }

    // MARK: Credit

    private var footer: some View {
        HStack(alignment: .firstTextBaseline) {
            let credit = L10n.format("Weather data by %@ (CC BY 4.0)", "Open-Meteo.com")
            if let url = URL(string: "https://open-meteo.com/") {
                Link(destination: url) {
                    Text(credit).underline()
                }
            } else {
                Text(credit)
            }
            Spacer(minLength: 8)
            if let zone = card.timezone {
                Text(zone).lineLimit(1).truncationMode(.middle)
            }
        }
        .font(.caption2)
        .foregroundStyle(.secondary)
    }
}

// MARK: - Pieces

/// A label over a value, for the current conditions and a day's details.
struct StatTile: View {
    struct Item: Identifiable {
        let label: String
        let value: String
        var windFrom: String?
        var id: String { label }
    }

    let tile: Item

    var body: some View {
        VStack(alignment: .leading, spacing: 2) {
            Text(tile.label)
                .font(.caption2)
                .foregroundStyle(.secondary)
                .lineLimit(1)
            HStack(spacing: 4) {
                if let from = tile.windFrom, let index = WeatherCard.compass.firstIndex(of: from) {
                    // Points where the wind blows to: it comes from `from`.
                    Image(systemName: "arrow.up")
                        .font(.caption.weight(.semibold))
                        .rotationEffect(.degrees(Double(index) * 22.5 + 180))
                        .accessibilityHidden(true)
                }
                Text(tile.value)
                    .font(.subheadline.weight(.medium))
                    .lineLimit(2)
                    .minimumScaleFactor(0.8)
            }
        }
        .padding(.horizontal, 10)
        .padding(.vertical, 8)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(Theme.raised, in: RoundedRectangle(cornerRadius: 10, style: .continuous))
        .accessibilityElement(children: .combine)
    }
}

/// The SF Symbol for a WMO code, in the symbol's own colours; clear and partly cloudy have a moon at night.
struct WeatherIcon: View {
    let code: Int?
    let night: Bool
    var size: CGFloat = 24

    var body: some View {
        Image(systemName: symbol)
            .symbolRenderingMode(.multicolor)
            .font(.system(size: size))
            .accessibilityLabel(code.map(WeatherFormat.condition) ?? "")
    }

    private var symbol: String {
        switch WeatherCard.Sky(code: code) {
        case .clear: return night ? "moon.stars.fill" : "sun.max.fill"
        case .partly: return night ? "cloud.moon.fill" : "cloud.sun.fill"
        case .cloud: return "cloud.fill"
        case .fog: return "cloud.fog.fill"
        case .drizzle: return "cloud.drizzle.fill"
        case .rain: return "cloud.rain.fill"
        case .sleet: return "cloud.sleet.fill"
        case .snow: return "cloud.snow.fill"
        case .storm: return "cloud.bolt.rain.fill"
        }
    }
}

/// Where the forecast's temperatures fall between the coldest low and the warmest high (the current
/// temperature included), as a fraction of the bar.
struct TemperatureScale {
    let min: Double
    let span: Double

    init(days: [WeatherCard.Day], now: WeatherCard.Quantity?) {
        var values = days.flatMap { [$0.low?.value, $0.high?.value].compactMap { $0 } }
        if let now { values.append(now.value) }
        let low = values.min() ?? 0
        let high = values.max() ?? 1
        min = low
        span = Swift.max(high - low, 1)
    }

    func fraction(_ value: Double) -> Double {
        Swift.min(Swift.max((value - min) / span, 0), 1)
    }
}

/// A day's low-to-high bar, coloured by temperature, with a dot for the current temperature today.
private struct TemperatureBar: View {
    let day: WeatherCard.Day
    let scale: TemperatureScale
    let now: WeatherCard.Quantity?

    var body: some View {
        GeometryReader { geo in
            let width = geo.size.width
            if let low = day.low, let high = day.high {
                let start = scale.fraction(low.value) * width
                let end = scale.fraction(high.value) * width
                ZStack(alignment: .leading) {
                    Capsule().fill(Theme.raised)
                    Capsule()
                        .fill(LinearGradient(colors: [WeatherFormat.color(celsius: low.celsius), WeatherFormat.color(celsius: high.celsius)],
                                             startPoint: .leading, endPoint: .trailing))
                        .frame(width: Swift.max(end - start, 6))
                        .offset(x: start)
                    if let now {
                        Circle()
                            .fill(Theme.surface)
                            .overlay(Circle().strokeBorder(Color.primary, lineWidth: 1.5))
                            .frame(width: 10, height: 10)
                            .offset(x: scale.fraction(now.value) * width - 5)
                            .accessibilityLabel(L10n.format("Now %@", WeatherFormat.degrees(now)))
                    }
                }
            }
        }
        .frame(height: 10)
        .frame(maxWidth: .infinity)
    }
}

/// How the weather card writes its numbers and names, in the interface language.
enum WeatherFormat {
    struct DayNames {
        /// "Today", "Tomorrow", else the weekday.
        let name: String
        /// "12 Oct"
        let short: String
        /// "Monday, 12 October"
        let long: String
    }

    static func number(_ value: Double, digits: Int) -> String {
        let formatter = NumberFormatter()
        formatter.locale = L10n.locale
        formatter.numberStyle = .decimal
        formatter.maximumFractionDigits = digits
        return formatter.string(from: value as NSNumber) ?? "\(value)"
    }

    static func whole(_ value: Double) -> String { number(value.rounded(), digits: 0) }

    /// "10.2°C" → "10°"; the unit is shown once, by the current temperature.
    static func degrees(_ q: WeatherCard.Quantity?) -> String {
        q.map { "\(whole($0.value))°" } ?? "–"
    }

    /// "4.6 km/h", "0.2 mm", "70%": the number in the reader's format with its unit.
    static func amount(_ q: WeatherCard.Quantity, digits: Int = 1) -> String {
        let unit = q.unit == "inch" ? "in" : q.unit
        let n = number(q.value, digits: digits)
        if unit.isEmpty { return n }
        return unit == "%" || unit.hasPrefix("°") ? "\(n)\(unit)" : "\(n) \(unit)"
    }

    /// The condition in the reader's language ("Partly cloudy"); unknown codes say so.
    static func condition(_ code: Int) -> String {
        let key = WeatherCard.conditionKey(code)
        return key.isEmpty ? L10n.format("Weather code %lld", code) : L10n.string(key)
    }

    /// Cold blue through mild green and yellow to hot red, for a temperature in °C.
    static func color(celsius: Double) -> Color {
        // HSL (hue, 75%, 55%) as the web draws it, in HSB.
        Color(hue: WeatherCard.hue(celsius: celsius) / 360, saturation: 0.76, brightness: 0.89)
    }

    /// Sunrise and sunset as one "Daylight" tile, or whichever one the day has.
    static func daylight(_ day: WeatherCard.Day?) -> [StatTile.Item] {
        guard let day else { return [] }
        if let rise = day.sunrise, let set = day.sunset { return [.init(label: L10n.string("Daylight"), value: "\(rise) – \(set)")] }
        if let rise = day.sunrise { return [.init(label: L10n.string("Sunrise"), value: rise)] }
        if let set = day.sunset { return [.init(label: L10n.string("Sunset"), value: set)] }
        return []
    }

    private static let dayParser: DateFormatter = {
        let f = DateFormatter()
        f.locale = Locale(identifier: "en_US_POSIX")
        f.timeZone = TimeZone(identifier: "UTC")
        f.dateFormat = "yyyy-MM-dd"
        return f
    }()

    /// "Today", "Tomorrow", else the weekday; and the date, short and long, in the reader's format.
    static func dayNames(_ date: String, index: Int, today: String?) -> DayNames {
        guard let day = dayParser.date(from: date) else { return DayNames(name: date, short: "", long: date) }
        var calendar = Calendar(identifier: .gregorian)
        calendar.timeZone = TimeZone(identifier: "UTC")!
        func format(_ template: String) -> String {
            let f = DateFormatter()
            f.locale = L10n.locale
            f.timeZone = calendar.timeZone
            f.setLocalizedDateFormatFromTemplate(template)
            return f.string(from: day)
        }
        var name = format("EEE")
        if let today, let todayDate = dayParser.date(from: today) {
            let days = calendar.dateComponents([.day], from: todayDate, to: day).day ?? -1
            if days == 0 { name = L10n.string("Today") } else if days == 1 { name = L10n.string("Tomorrow") }
        } else if index == 0 {
            name = L10n.string("Today")
        }
        return DayNames(name: name, short: format("d MMM"), long: format("EEEE d MMMM"))
    }
}
