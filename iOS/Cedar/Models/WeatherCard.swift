import Foundation

/// A `weather_forecast` result, read back from the plain-text report the model gets (see
/// src/tools/weather.rs, `format_forecast`), with the same rules as the web UI's
/// chat/static/weather-view.js, so the tool and what the model sees are unchanged and older
/// conversations get the card too:
///
///     Berlin, State of Berlin, Germany (52.52, 13.41), timezone Europe/Berlin
///
///     Now (2026-10-12 21:00): clear sky, 10.2°C (feels 8.3°C), humidity 70%, wind 4.6 km/h from NW, ...
///
///     Mon 2026-10-12: overcast, low 6.8°C, high 13.7°C, precipitation 0 mm (32% chance), ...
///
///     Weather data by Open-Meteo.com (CC BY 4.0)
///
/// Any part the report leaves out is `nil`. The text came from the network, so it is clipped and
/// only shown as text.
struct WeatherCard: Hashable, Sendable {
    /// A number with its unit: `10.2 °C`, `4.6 km/h`, `70 %`, or unitless for the UV index.
    struct Quantity: Hashable, Sendable {
        let value: Double
        let unit: String

        /// "10.2°C", "4.6 km/h", "70%", "2.9"; `nil` if `text` doesn't start with a number.
        init?(_ text: Substring) {
            let s = text.trimmingCharacters(in: .whitespaces)
            var end = s.startIndex
            while end < s.endIndex, "-0123456789.e".contains(s[end]) { end = s.index(after: end) }
            guard end > s.startIndex, let value = Double(s[s.startIndex..<end]), value.isFinite else { return nil }
            self.value = value
            self.unit = WeatherCard.clip(s[end...])
        }

        init(value: Double, unit: String) {
            self.value = value
            self.unit = unit
        }

        /// The temperature in °C whichever unit it came in, for picking a colour.
        var celsius: Double { unit == "°F" ? (value - 32) * 5 / 9 : value }
    }

    struct Now: Hashable, Sendable {
        var date: String?
        /// "21:00", the report's local time.
        var time: String?
        var code: Int?
        var temp: Quantity?
        var feels: Quantity?
        var humidity: Quantity?
        var wind: Quantity?
        /// A 16-point compass name the wind comes from.
        var from: String?
        var gusts: Quantity?
        var precip: Quantity?
    }

    struct Day: Hashable, Sendable, Identifiable {
        /// "2026-10-12"
        let date: String
        var code: Int?
        var low: Quantity?
        var high: Quantity?
        var precip: Quantity?
        var chance: Quantity?
        var wind: Quantity?
        var gusts: Quantity?
        var uv: Quantity?
        var sunrise: String?
        var sunset: String?

        var id: String { date }
    }

    /// "Berlin, State of Berlin, Germany", or empty when the report only has coordinates.
    let label: String
    let latitude: Double
    let longitude: Double
    let timezone: String?
    let now: Now?
    let days: [Day]

    static let maxDays = 16
    static let maxText = 200
    static let compass = ["N", "NNE", "NE", "ENE", "E", "ESE", "SE", "SSE", "S", "SSW", "SW", "WSW", "W", "WNW", "NW", "NNW"]

    /// The words weather.rs writes for each WMO weather code, read back to the code.
    static let codes: [String: Int] = [
        "clear sky": 0, "mainly clear": 1, "partly cloudy": 2, "overcast": 3, "fog": 45, "freezing fog": 48,
        "light drizzle": 51, "moderate drizzle": 53, "dense drizzle": 55, "light freezing drizzle": 56,
        "dense freezing drizzle": 57, "slight rain": 61, "moderate rain": 63, "heavy rain": 65,
        "light freezing rain": 66, "heavy freezing rain": 67, "slight snow": 71, "moderate snow": 73,
        "heavy snow": 75, "snow grains": 77, "slight rain showers": 80, "moderate rain showers": 81,
        "violent rain showers": 82, "slight snow showers": 85, "heavy snow showers": 86, "thunderstorm": 95,
        "thunderstorm with slight hail": 96, "thunderstorm with heavy hail": 99,
    ]

    // MARK: Parsing

    /// The forecast in a `weather_forecast` result, or `nil` when `output` isn't one.
    static func parse(_ output: String) -> WeatherCard? {
        let lines = output.split(whereSeparator: \.isNewline).map { $0.trimmingCharacters(in: .whitespaces) }.filter { !$0.isEmpty }
        guard let first = lines.first, let head = parseHeader(first) else { return nil }
        var now: Now?
        var days: [Day] = []
        for line in lines.dropFirst() {
            if now == nil, line.hasPrefix("Now") {
                now = parseNow(line)
            } else if days.count < maxDays, let day = parseDay(line) {
                days.append(day)
            }
        }
        guard now?.temp != nil || days.contains(where: { $0.low != nil || $0.high != nil }) else { return nil }
        return WeatherCard(label: head.label, latitude: head.latitude, longitude: head.longitude,
                           timezone: head.timezone, now: now, days: days)
    }

    static func clip<S: StringProtocol>(_ s: S) -> String {
        String(s.trimmingCharacters(in: .whitespacesAndNewlines).prefix(maxText))
    }

    /// "Berlin, Germany (52.52, 13.41), timezone Europe/Berlin", or just "52.52, 13.41".
    private static func parseHeader(_ line: String) -> (label: String, latitude: Double, longitude: Double, timezone: String?)? {
        var rest = Substring(line)
        var timezone: String?
        if let range = rest.range(of: ", timezone ") {
            let zone = rest[range.upperBound...]
            guard !zone.isEmpty, !zone.contains(where: \.isWhitespace) else { return nil }
            timezone = clip(zone)
            rest = rest[..<range.lowerBound]
        }
        var label = ""
        var coordinates = rest
        if rest.hasSuffix(")"), let open = rest.lastIndex(of: "(") {
            label = clip(rest[..<open])
            coordinates = rest[rest.index(after: open)..<rest.index(before: rest.endIndex)]
        }
        let parts = coordinates.split(separator: ",", maxSplits: 1).map { $0.trimmingCharacters(in: .whitespaces) }
        guard parts.count == 2, let lat = Double(parts[0]), let lon = Double(parts[1]),
              CardParse.isCoordinate(lat, limit: 90), CardParse.isCoordinate(lon, limit: 180) else { return nil }
        return (label, lat, lon, timezone)
    }

    /// The weather code for a condition in words ("partly cloudy", "weather code 4"), or `nil`.
    private static func code(of words: Substring) -> Int? {
        if let code = codes[String(words)] { return code }
        if words.hasPrefix("weather code "), let code = Int(words.dropFirst("weather code ".count)) { return code }
        return nil
    }

    /// "Now (2026-10-12 21:00): clear sky, 10.2°C (feels 8.3°C), humidity 70%, wind 4.6 km/h from NW, ..."
    private static func parseNow(_ line: String) -> Now? {
        guard let colon = line.range(of: ": ") else { return nil }
        var now = Now()
        let when = line[line.index(line.startIndex, offsetBy: 3)..<colon.lowerBound]
        if when.hasPrefix(" ("), when.hasSuffix(")") {
            let stamp = when.dropFirst(2).dropLast().split(separator: " ")
            if stamp.count == 2, isDate(stamp[0]), isClock(stamp[1]) {
                now.date = String(stamp[0])
                now.time = String(stamp[1])
            }
        }
        for part in line[colon.upperBound...].components(separatedBy: ", ").map({ Substring($0) }) {
            if let code = code(of: part) {
                if now.code == nil { now.code = code }
            } else if let open = part.range(of: " (feels "), part.hasSuffix(")") {
                now.temp = Quantity(part[..<open.lowerBound])
                now.feels = Quantity(part[open.upperBound..<part.index(before: part.endIndex)])
            } else if let rest = after("humidity ", in: part) {
                now.humidity = Quantity(rest)
            } else if let rest = after("wind ", in: part) {
                if let from = rest.range(of: " from "), compass.contains(String(rest[from.upperBound...])) {
                    now.wind = Quantity(rest[..<from.lowerBound])
                    now.from = String(rest[from.upperBound...])
                } else {
                    now.wind = Quantity(rest)
                }
            } else if let rest = after("gusts ", in: part) {
                now.gusts = Quantity(rest)
            } else if let rest = after("precipitation ", in: part) {
                now.precip = Quantity(rest)
            } else if part.contains("°"), now.temp == nil {
                now.temp = Quantity(part)
            }
        }
        return now
    }

    /// "Mon 2026-10-12: overcast, low 6.8°C, high 13.7°C, precipitation 0 mm (32% chance), wind up to
    /// 14.1 km/h (gusts 31 km/h), UV 2.9, sunrise 07:26, sunset 18:18"
    private static func parseDay(_ line: String) -> Day? {
        guard let colon = line.range(of: ": ") else { return nil }
        let head = line[..<colon.lowerBound].split(separator: " ")
        guard head.count == 2, head[0].count == 3, head[0].first?.isUppercase == true, isDate(head[1]) else { return nil }
        var day = Day(date: String(head[1]))
        for part in line[colon.upperBound...].components(separatedBy: ", ").map({ Substring($0) }) {
            if let code = code(of: part) {
                if day.code == nil { day.code = code }
            } else if let rest = after("low ", in: part) {
                day.low = Quantity(rest)
            } else if let rest = after("high ", in: part) {
                day.high = Quantity(rest)
            } else if let rest = after("precipitation ", in: part) {
                if let open = rest.range(of: " ("), rest.hasSuffix(" chance)") {
                    day.precip = Quantity(rest[..<open.lowerBound])
                    day.chance = Quantity(rest[open.upperBound..<rest.index(rest.endIndex, offsetBy: -" chance)".count)])
                } else {
                    day.precip = Quantity(rest)
                }
            } else if let rest = after("wind up to ", in: part) {
                if let open = rest.range(of: " (gusts "), rest.hasSuffix(")") {
                    day.wind = Quantity(rest[..<open.lowerBound])
                    day.gusts = Quantity(rest[open.upperBound..<rest.index(before: rest.endIndex)])
                } else {
                    day.wind = Quantity(rest)
                }
            } else if let rest = after("UV ", in: part) {
                day.uv = Quantity(rest)
            } else if let rest = after("sunrise ", in: part), isClock(rest) {
                day.sunrise = String(rest)
            } else if let rest = after("sunset ", in: part), isClock(rest) {
                day.sunset = String(rest)
            }
        }
        return day
    }

    private static func after(_ prefix: String, in part: Substring) -> Substring? {
        part.hasPrefix(prefix) ? part.dropFirst(prefix.count) : nil
    }

    /// "2026-10-12"
    static func isDate(_ s: Substring) -> Bool {
        s.count == 10 && s[s.index(s.startIndex, offsetBy: 4)] == "-" && s[s.index(s.startIndex, offsetBy: 7)] == "-"
            && s.enumerated().allSatisfy { i, c in (i == 4 || i == 7) || c.isNumber }
    }

    /// "07:26"
    static func isClock(_ s: Substring) -> Bool {
        s.count == 5 && s[s.index(s.startIndex, offsetBy: 2)] == ":" && s.enumerated().allSatisfy { i, c in i == 2 || c.isNumber }
    }

    // MARK: What the card shows

    /// The report's local date: the current conditions', else the first day's.
    var today: String? { now?.date ?? days.first?.date }

    var todayDay: Day? { days.first { $0.date == today } }

    /// Night if the report's local time is outside today's sunrise to sunset.
    var isNight: Bool {
        guard let time = now?.time, let rise = todayDay?.sunrise, let set = todayDay?.sunset else { return false }
        return time < rise || time >= set
    }

    /// The condition the card leads with: now, else the first day.
    var leadCode: Int? { now?.code ?? days.first?.code }

    /// The place's name, and the rest of its label ("State of Berlin, Germany").
    var placeName: String {
        let name = label.split(separator: ", ", maxSplits: 1).first.map(String.init) ?? ""
        return name.isEmpty ? String(format: "%.2f, %.2f", latitude, longitude) : name
    }

    var region: String? {
        let parts = label.components(separatedBy: ", ")
        return parts.count > 1 ? parts.dropFirst().joined(separator: ", ") : nil
    }

    /// The days with both a low and a high, which the forecast rows need.
    var forecastDays: [Day] { days.filter { $0.low != nil && $0.high != nil } }

    /// What each WMO code looks like; clear and partly cloudy have a night version.
    enum Sky: Hashable, Sendable {
        case clear, partly, cloud, fog, drizzle, rain, sleet, snow, storm

        init(code: Int?) {
            switch code ?? -1 {
            case 0, 1: self = .clear
            case 2: self = .partly
            case 3: self = .cloud
            case 45, 48: self = .fog
            case 51...57: self = .drizzle
            case 66, 67: self = .sleet
            case 61...65, 80...82: self = .rain
            case 71...77, 85, 86: self = .snow
            case 95...99: self = .storm
            default: self = .cloud
            }
        }
    }

    /// The English words for a WMO code, as the web UI's translations key them; unknown codes say so.
    static func conditionKey(_ code: Int) -> String {
        switch code {
        case 0: return "Clear sky"
        case 1: return "Mainly clear"
        case 2: return "Partly cloudy"
        case 3: return "Overcast"
        case 45: return "Fog"
        case 48: return "Freezing fog"
        case 51: return "Light drizzle"
        case 53: return "Moderate drizzle"
        case 55: return "Dense drizzle"
        case 56: return "Light freezing drizzle"
        case 57: return "Dense freezing drizzle"
        case 61: return "Light rain"
        case 63: return "Moderate rain"
        case 65: return "Heavy rain"
        case 66: return "Light freezing rain"
        case 67: return "Heavy freezing rain"
        case 71: return "Light snow"
        case 73: return "Moderate snow"
        case 75: return "Heavy snow"
        case 77: return "Snow grains"
        case 80: return "Light rain showers"
        case 81: return "Moderate rain showers"
        case 82: return "Violent rain showers"
        case 85: return "Light snow showers"
        case 86: return "Heavy snow showers"
        case 95: return "Thunderstorm"
        case 96: return "Thunderstorm with light hail"
        case 99: return "Thunderstorm with heavy hail"
        default: return ""
        }
    }

    /// The hue (0–360) for a temperature in °C: cold blue through mild green and yellow to hot red.
    static func hue(celsius c: Double) -> Double {
        let stops: [(Double, Double)] = [(-15, 230), (0, 200), (10, 160), (18, 95), (25, 40), (35, 5)]
        guard c > stops[0].0 else { return stops[0].1 }
        for i in 1..<stops.count {
            let (t1, h1) = stops[i], (t0, h0) = stops[i - 1]
            if c <= t1 { return (h0 + (h1 - h0) * ((c - t0) / (t1 - t0))).rounded() }
        }
        return stops[stops.count - 1].1
    }
}
