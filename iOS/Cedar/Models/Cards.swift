import Foundation

/// A tool result the app draws as a card: `show_map`, `show_recipe`, `plan_trip`, `compare_stocks`,
/// `search_products`. Parsed with the same rules as the web UI's views (chat/static/map-view.js,
/// recipe-view.js, transit-view.js, stock-view.js, product-view.js). Everything in a result came from the model or the network, so
/// text is clipped, numbers and coordinates are checked, and links are only kept if they are https.
enum Card: Hashable, Sendable {
    case map(MapCard)
    case recipe(RecipeCard)
    case trip(TripCard)
    case stocks(StocksCard)
    case products(ProductsCard)

    /// The card for a finished call, or `nil` when it isn't a card tool or its result isn't usable.
    static func parse(name: String, output: String) -> Card? {
        guard ToolCall.cardTools.contains(name),
              let value = try? JSONDecoder().decode(JSONValue.self, from: Data(output.utf8))
        else { return nil }
        switch name {
        case "show_map": return MapCard.parse(value).map(Card.map)
        case "show_recipe": return RecipeCard.parse(value).map(Card.recipe)
        case "plan_trip": return TripCard.parse(value).map(Card.trip)
        case "compare_stocks": return StocksCard.parse(value).map(Card.stocks)
        case "search_products": return ProductsCard.parse(value).map(Card.products)
        default: return nil
        }
    }
}

// MARK: - Shared rules

enum CardParse {
    /// Trimmed, non-empty text, at most `max` characters.
    static func text(_ value: JSONValue?, max: Int = 200) -> String? {
        guard let s = value?.stringValue?.trimmingCharacters(in: .whitespacesAndNewlines), !s.isEmpty else { return nil }
        return String(s.prefix(max))
    }

    /// A number in `0...max`.
    static func amount(_ value: JSONValue?, max: Double = 1e7) -> Double? {
        guard let n = value?.doubleValue, n >= 0, n <= max else { return nil }
        return n
    }

    static func https(_ value: JSONValue?) -> URL? {
        guard let s = value?.stringValue, s.hasPrefix("https://"), !s.contains(where: \.isWhitespace) else { return nil }
        return URL(string: s)
    }

    static func time(_ value: JSONValue?) -> Date? {
        value?.stringValue.flatMap(CedarDate.parse)
    }

    static func isCoordinate(_ v: Double, limit: Double) -> Bool { v.isFinite && abs(v) <= limit }

    /// `#rrggbb`, lowercased.
    static func hexColor(_ value: JSONValue?) -> String? {
        guard let s = value?.stringValue, s.count == 7, s.first == "#",
              s.dropFirst().allSatisfy(\.isHexDigit) else { return nil }
        return s.lowercased()
    }
}

/// A point on the map.
struct Coordinate: Hashable, Sendable {
    let latitude: Double
    let longitude: Double
}

/// A named point: a map pin, or where a trip starts or ends.
struct Place: Hashable, Sendable {
    let label: String
    let latitude: Double
    let longitude: Double

    var coordinate: Coordinate { Coordinate(latitude: latitude, longitude: longitude) }
    var coordinateText: String { String(format: "%.4f, %.4f", latitude, longitude) }

    static func parse(_ value: JSONValue?) -> Place? {
        guard let lat = value?["latitude"]?.doubleValue, CardParse.isCoordinate(lat, limit: 90),
              let lon = value?["longitude"]?.doubleValue, CardParse.isCoordinate(lon, limit: 180)
        else { return nil }
        let label = CardParse.text(value?["label"]) ?? String(format: "%.4f, %.4f", lat, lon)
        return Place(label: label, latitude: lat, longitude: lon)
    }
}

// MARK: - show_map

/// `{"title"?, "markers": [{"label", "latitude", "longitude"}], "not_found"?: [names]}`
struct MapCard: Hashable, Sendable {
    let title: String?
    let markers: [Place]
    let notFound: [String]

    static let maxMarkers = 25

    static func parse(_ value: JSONValue) -> MapCard? {
        guard let raw = value["markers"]?.arrayValue else { return nil }
        let markers = raw.prefix(maxMarkers).compactMap(Place.parse)
        guard !markers.isEmpty else { return nil }
        let notFound = (value["not_found"]?.arrayValue ?? []).compactMap { CardParse.text($0) }.prefix(maxMarkers)
        return MapCard(title: CardParse.text(value["title"]), markers: Array(markers), notFound: Array(notFound))
    }
}

// MARK: - show_recipe

/// `{"title", "description"?, "servings"?, "prep_time"?, "cook_time"?, "total_time"?,
/// "ingredients": [{"quantity"?, "item"}], "steps": [{"title"?, "text", "duration"?}], "tips"?,
/// "image"?: {"url", "page_url", "title", "source"}}`
struct RecipeCard: Hashable, Sendable {
    struct Fact: Hashable, Sendable {
        let label: String
        let value: String
    }

    struct Ingredient: Hashable, Sendable {
        let quantity: String?
        let item: String
    }

    struct Step: Hashable, Sendable {
        let title: String?
        let text: String
        let duration: String?
    }

    struct Photo: Hashable, Sendable {
        let url: URL
        let pageURL: URL?
        let title: String?
        let source: String
    }

    let title: String
    let description: String?
    let facts: [Fact]
    let ingredients: [Ingredient]
    let steps: [Step]
    let tips: [String]
    let photo: Photo?

    static func parse(_ value: JSONValue) -> RecipeCard? {
        guard let title = CardParse.text(value["title"]),
              let rawIngredients = value["ingredients"]?.arrayValue,
              let rawSteps = value["steps"]?.arrayValue
        else { return nil }
        let ingredients = rawIngredients.prefix(60).compactMap { i -> Ingredient? in
            guard let item = CardParse.text(i["item"]) else { return nil }
            return Ingredient(quantity: CardParse.text(i["quantity"]), item: item)
        }
        let steps = rawSteps.prefix(40).compactMap { s -> Step? in
            guard let text = CardParse.text(s["text"], max: 2000) else { return nil }
            return Step(title: CardParse.text(s["title"]), text: text, duration: CardParse.text(s["duration"]))
        }
        guard !ingredients.isEmpty, !steps.isEmpty else { return nil }
        let tips = (value["tips"]?.arrayValue ?? []).compactMap { CardParse.text($0, max: 2000) }.prefix(10)
        let facts = [("Prep", "prep_time"), ("Cook", "cook_time"), ("Total", "total_time"), ("Serves", "servings")]
            .compactMap { label, key in CardParse.text(value[key]).map { Fact(label: label, value: $0) } }
        var photo: Photo?
        if let image = value["image"], let url = CardParse.https(image["url"]) {
            photo = Photo(url: url, pageURL: CardParse.https(image["page_url"]),
                          title: CardParse.text(image["title"]), source: CardParse.text(image["source"]) ?? "Wikimedia")
        }
        return RecipeCard(title: title, description: CardParse.text(value["description"], max: 2000), facts: facts,
                          ingredients: Array(ingredients), steps: Array(steps), tips: Array(tips), photo: photo)
    }
}

// MARK: - plan_trip

/// `{"title"?, "from", "to", "depart_at"? | "arrive_by"?, "time_zone"?, "recommendation": {"mode",
/// "option", "reason"}, "options": [...], "unavailable"?: [{"mode", "reason"}], "attribution"}`
struct TripCard: Hashable, Sendable {
    enum Mode: String, Hashable, Sendable, CaseIterable {
        case transit, walk, drive, rideshare

        var label: String {
            switch self {
            case .transit: return "Transit"
            case .walk: return "Walk"
            case .drive: return "Drive"
            case .rideshare: return "Rideshare"
            }
        }
    }

    /// A leg's mode, grouped by the icon and word it gets (MOTIS names, lowercased).
    enum LegKind: String, Hashable, Sendable {
        case walk, bike, drive, bus, tram, subway, ferry, plane, train, other

        init(motis mode: String) {
            switch mode {
            case "walk": self = .walk
            case "bike", "rental": self = .bike
            case "car", "car_parking", "odm": self = .drive
            case "bus", "coach", "flex": self = .bus
            case "tram", "cable_car", "funicular", "areal_lift": self = .tram
            case "subway", "metro": self = .subway
            case "ferry": self = .ferry
            case "airplane": self = .plane
            case "rail", "suburban", "highspeed_rail", "long_distance", "night_rail", "regional_rail", "regional_fast_rail":
                self = .train
            default: self = .other
            }
        }

        var label: String {
            switch self {
            case .walk: return "Walk"
            case .bike: return "Bike"
            case .drive: return "Drive"
            case .bus: return "Bus"
            case .tram: return "Tram"
            case .subway: return "Subway"
            case .ferry: return "Ferry"
            case .plane: return "Flight"
            case .train: return "Train"
            case .other: return "Ride"
            }
        }
    }

    struct Leg: Hashable, Sendable {
        let kind: LegKind
        let from: String?
        let to: String?
        let depart: Date?
        let arrive: Date?
        let minutes: Int
        let route: String?
        let headsign: String?
        let agency: String?
        /// `#rrggbb`
        let color: String?
        let stops: Int?
        let meters: Double?
        let path: [Coordinate]
    }

    struct Cost: Hashable, Sendable {
        let low: Double
        let high: Double
        let currency: String
    }

    struct Option: Hashable, Sendable, Identifiable {
        /// The option's index in the result, which the recommendation refers to.
        let id: Int
        let mode: Mode
        let minutes: Int
        let legs: [Leg]
        let cost: Cost?
        let depart: Date?
        let arrive: Date?
        let transfers: Int?
        let walkMinutes: Int?
        let km: Double?
        let meters: Double?
        let note: String?
        let path: [Coordinate]
    }

    struct Unavailable: Hashable, Sendable {
        let mode: Mode
        let reason: String?
    }

    let title: String?
    let from: Place
    let to: Place
    let timeZone: TimeZone?
    let departAt: Date?
    let arriveBy: Date?
    let options: [Option]
    let recommended: Int?
    let reason: String?
    let unavailable: [Unavailable]
    let attribution: String?

    var recommendedOption: Option? { options.first { $0.id == recommended } }
    var fastest: Option? { options.min { $0.minutes < $1.minutes } }

    static func parse(_ value: JSONValue) -> TripCard? {
        guard let raw = value["options"]?.arrayValue,
              let from = Place.parse(value["from"]), let to = Place.parse(value["to"])
        else { return nil }
        let options = raw.prefix(8).enumerated().compactMap { index, o in option(o, index: index) }
        guard !options.isEmpty else { return nil }
        let rec = value["recommendation"]
        let recommended = rec?["option"]?.intValue.flatMap { i in options.contains { $0.id == i } ? i : nil }
        let unavailable = (value["unavailable"]?.arrayValue ?? []).compactMap { u -> Unavailable? in
            guard let mode = u["mode"]?.stringValue.flatMap(Mode.init(rawValue:)),
                  !options.contains(where: { $0.mode == mode }) else { return nil }
            return Unavailable(mode: mode, reason: CardParse.text(u["reason"]))
        }.prefix(4)
        return TripCard(
            title: CardParse.text(value["title"]), from: from, to: to,
            timeZone: CardParse.text(value["time_zone"]).flatMap(TimeZone.init(identifier:)),
            departAt: CardParse.time(value["depart_at"]), arriveBy: CardParse.time(value["arrive_by"]),
            options: Array(options), recommended: recommended,
            reason: recommended == nil ? nil : CardParse.text(rec?["reason"]),
            unavailable: Array(unavailable), attribution: CardParse.text(value["attribution"])
        )
    }

    private static func option(_ o: JSONValue, index: Int) -> Option? {
        guard let mode = o["mode"]?.stringValue.flatMap(Mode.init(rawValue:)),
              let minutes = CardParse.amount(o["duration_min"]) else { return nil }
        let legs = mode == .transit ? (o["legs"]?.arrayValue ?? []).prefix(30).compactMap(leg) : []
        var cost: Cost?
        if let c = o["cost"], let low = CardParse.amount(c["low"]), let high = CardParse.amount(c["high"]) {
            let currency = CardParse.text(c["currency"]).map { String($0.prefix(3)).uppercased() } ?? "USD"
            cost = Cost(low: low, high: high, currency: currency)
        }
        return Option(
            id: index, mode: mode, minutes: Int(minutes.rounded()), legs: Array(legs), cost: cost,
            depart: CardParse.time(o["depart"]), arrive: CardParse.time(o["arrive"]),
            transfers: CardParse.amount(o["transfers"], max: 50).map { Int($0) },
            walkMinutes: CardParse.amount(o["walk_min"]).map { Int($0.rounded()) },
            km: CardParse.amount(o["distance_km"], max: 1e5), meters: CardParse.amount(o["distance_m"]),
            note: CardParse.text(o["note"]), path: Polyline.decode(o["path"]?.stringValue)
        )
    }

    private static func leg(_ l: JSONValue) -> Leg? {
        guard case .object = l else { return nil }
        return Leg(
            kind: LegKind(motis: CardParse.text(l["mode"])?.lowercased() ?? ""),
            from: CardParse.text(l["from"]), to: CardParse.text(l["to"]),
            depart: CardParse.time(l["depart"]), arrive: CardParse.time(l["arrive"]),
            minutes: Int((CardParse.amount(l["duration_min"]) ?? 0).rounded()),
            route: CardParse.text(l["route"]).map { String($0.prefix(24)) },
            headsign: CardParse.text(l["headsign"]), agency: CardParse.text(l["agency"]),
            color: CardParse.hexColor(l["color"]),
            stops: CardParse.amount(l["stops"], max: 1000).map { Int($0) },
            meters: CardParse.amount(l["distance_m"]),
            path: Polyline.decode(l["path"]?.stringValue)
        )
    }
}

/// Google's encoded polyline format, precision 5.
enum Polyline {
    /// The points in `encoded`, or none if it doesn't decode cleanly.
    static func decode(_ encoded: String?) -> [Coordinate] {
        guard let encoded, encoded.utf8.count <= 20_000 else { return [] }
        let bytes = Array(encoded.utf8)
        var i = 0
        func next() -> Int? {
            var result = 0
            var shift = 0
            while true {
                guard i < bytes.count, shift <= 30 else { return nil }
                let b = Int(bytes[i]) - 63
                i += 1
                guard (0...63).contains(b) else { return nil }
                result |= (b & 0x1f) << shift
                shift += 5
                if b < 0x20 { break }
            }
            return result & 1 != 0 ? ~(result >> 1) : result >> 1
        }
        var points: [Coordinate] = []
        var lat = 0
        var lon = 0
        while i < bytes.count, points.count < 2000 {
            guard let dlat = next(), let dlon = next() else { return [] }
            lat += dlat
            lon += dlon
            let point = Coordinate(latitude: Double(lat) / 1e5, longitude: Double(lon) / 1e5)
            guard CardParse.isCoordinate(point.latitude, limit: 90), CardParse.isCoordinate(point.longitude, limit: 180) else { return [] }
            points.append(point)
        }
        return points
    }
}

// MARK: - compare_stocks

/// `{"range", "benchmark"?, "benchmark_series"?, "as_of", "symbols": [...], "news": [...],
/// "news_note"?, "unavailable"?, "disclaimer"}`
struct StocksCard: Hashable, Sendable {
    /// Daily closes, oldest first, at least two.
    struct Series: Hashable, Sendable {
        let dates: [Date]
        let closes: [Double]

        static func parse(_ chart: JSONValue?) -> Series? {
            guard let dates = chart?["dates"]?.arrayValue, let closes = chart?["close"]?.arrayValue else { return nil }
            var outDates: [Date] = []
            var outCloses: [Double] = []
            for (d, c) in zip(dates, closes).prefix(2000) {
                guard let date = d.stringValue.flatMap(StocksCard.day), let close = c.doubleValue, close > 0 else { continue }
                if let last = outDates.last, date <= last { continue }
                outDates.append(date)
                outCloses.append(close)
            }
            return outDates.count >= 2 ? Series(dates: outDates, closes: outCloses) : nil
        }
    }

    enum Call: String, Hashable, Sendable {
        case buy, hold, sell
        var label: String { rawValue.capitalized }
    }

    struct Factor: Hashable, Sendable {
        let name: String
        let headline: String?
        let detail: String
        /// -2...2
        let score: Int
    }

    struct Rating: Hashable, Sendable {
        let call: Call
        let lean: String?
        let score: Int
        let maxScore: Int
        /// low, medium, or high
        let confidence: String
        let summary: String?
        let factors: [Factor]
        let watch: [String]

        static func parse(_ r: JSONValue?) -> Rating? {
            guard let call = r?["call"]?.stringValue.flatMap(Call.init(rawValue:)) else { return nil }
            let factors = (r?["factors"]?.arrayValue ?? []).prefix(12).compactMap { f -> Factor? in
                guard let name = CardParse.text(f["name"], max: 80), let detail = CardParse.text(f["detail"], max: 1200) else { return nil }
                let score = Int((f["score"]?.doubleValue ?? 0).rounded())
                return Factor(name: name, headline: CardParse.text(f["headline"], max: 80), detail: detail, score: min(2, max(-2, score)))
            }
            let maxScore = max(1, Int((r?["max_score"]?.doubleValue ?? 9).rounded()))
            let score = Int((r?["score"]?.doubleValue ?? 0).rounded())
            let confidence = r?["confidence"]?.stringValue ?? ""
            return Rating(
                call: call, lean: CardParse.text(r?["lean"], max: 40),
                score: min(maxScore, max(-maxScore, score)), maxScore: maxScore,
                confidence: ["low", "medium", "high"].contains(confidence) ? confidence : "low",
                summary: CardParse.text(r?["summary"], max: 1200), factors: Array(factors),
                watch: Array((r?["watch"]?.arrayValue ?? []).compactMap { CardParse.text($0, max: 1200) }.prefix(6))
            )
        }
    }

    struct Indicators: Hashable, Sendable {
        let sma50: Double?
        let sma200: Double?
        let rsi: Double?
        let volatility: Double?
        let drawdown: Double?
        let beta: Double?
    }

    struct Stock: Hashable, Sendable, Identifiable {
        var id: String { symbol }
        let symbol: String
        let name: String
        let currency: String?
        let price: Double
        let change: Double?
        let changePct: Double?
        let dayLow: Double?
        let dayHigh: Double?
        let low52: Double?
        let high52: Double?
        /// One value per `StocksCard.returnPeriods` entry.
        let returns: [Double?]
        let indicators: Indicators
        let rating: Rating?
        let series: Series
    }

    struct Benchmark: Hashable, Sendable {
        let symbol: String
        let name: String
        /// `nil` when the benchmark is one of the symbols.
        let series: Series?
    }

    struct News: Hashable, Sendable, Identifiable {
        var id: URL { url }
        let title: String
        let url: URL
        let publisher: String?
        let published: Date?
        let symbols: [String]
    }

    struct Unavailable: Hashable, Sendable {
        let symbol: String
        let reason: String?
    }

    static let returnPeriods = [("1w", "1W"), ("1m", "1M"), ("3m", "3M"), ("ytd", "YTD"), ("6m", "6M"), ("1y", "1Y")]
    /// The chart periods, in days.
    static let ranges = [("1M", 30), ("3M", 91), ("6M", 182), ("1Y", 365), ("2Y", 730), ("5Y", 1826)]
    private static let rangeKeys = ["1mo": "1M", "3mo": "3M", "6mo": "6M", "1y": "1Y", "2y": "2Y", "5y": "5Y"]

    /// The period the chart starts on, a key of `ranges`.
    let range: String
    let asOf: Date?
    let stocks: [Stock]
    let benchmark: Benchmark?
    let news: [News]
    let newsNote: String?
    let unavailable: [Unavailable]
    let disclaimer: String?

    static func parse(_ value: JSONValue) -> StocksCard? {
        guard let raw = value["symbols"]?.arrayValue else { return nil }
        let stocks = raw.prefix(6).compactMap { s -> Stock? in
            guard let symbol = CardParse.text(s["symbol"], max: 20), let price = s["price"]?.doubleValue,
                  let series = Series.parse(s["chart"]) else { return nil }
            let ind = s["indicators"]
            return Stock(
                symbol: symbol, name: CardParse.text(s["name"], max: 120) ?? symbol,
                currency: CardParse.text(s["currency"], max: 8), price: price,
                change: s["change"]?.doubleValue, changePct: s["change_pct"]?.doubleValue,
                dayLow: s["day_low"]?.doubleValue, dayHigh: s["day_high"]?.doubleValue,
                low52: s["fifty_two_week_low"]?.doubleValue, high52: s["fifty_two_week_high"]?.doubleValue,
                returns: returnPeriods.map { key, _ in s["returns"]?[key]?.doubleValue },
                indicators: Indicators(
                    sma50: ind?["sma50"]?.doubleValue, sma200: ind?["sma200"]?.doubleValue, rsi: ind?["rsi14"]?.doubleValue,
                    volatility: ind?["volatility_pct"]?.doubleValue, drawdown: ind?["max_drawdown_pct"]?.doubleValue,
                    beta: ind?["beta"]?.doubleValue
                ),
                rating: Rating.parse(s["rating"]), series: series
            )
        }
        guard !stocks.isEmpty else { return nil }

        var benchmark: Benchmark?
        if let b = value["benchmark"], let symbol = CardParse.text(b["symbol"], max: 20) {
            let series = b["in_symbols"]?.boolValue == true ? nil : Series.parse(value["benchmark_series"])
            benchmark = Benchmark(symbol: symbol, name: CardParse.text(b["name"], max: 120) ?? symbol, series: series)
        }
        let symbols = Set(stocks.map(\.symbol))
        let news = (value["news"]?.arrayValue ?? []).prefix(24).compactMap { n -> News? in
            guard let url = CardParse.https(n["url"]), let title = CardParse.text(n["title"], max: 300) else { return nil }
            return News(title: title, url: url, publisher: CardParse.text(n["publisher"], max: 80),
                        published: CardParse.time(n["published"]),
                        symbols: (n["symbols"]?.arrayValue ?? []).compactMap(\.stringValue).filter(symbols.contains))
        }
        let unavailable = (value["unavailable"]?.arrayValue ?? []).compactMap { u -> Unavailable? in
            CardParse.text(u["symbol"], max: 20).map { Unavailable(symbol: $0, reason: CardParse.text(u["reason"])) }
        }
        return StocksCard(
            range: value["range"]?.stringValue.flatMap { rangeKeys[$0] } ?? "1Y",
            asOf: CardParse.time(value["as_of"]), stocks: Array(stocks), benchmark: benchmark, news: Array(news),
            newsNote: CardParse.text(value["news_note"], max: 400), unavailable: unavailable,
            disclaimer: CardParse.text(value["disclaimer"], max: 600)
        )
    }

    /// A `yyyy-MM-dd` day, at midnight UTC.
    static func day(_ s: String) -> Date? {
        guard s.count == 10 else { return nil }
        return dayFormatter.date(from: s)
    }

    private static let dayFormatter: DateFormatter = {
        let f = DateFormatter()
        f.locale = Locale(identifier: "en_US_POSIX")
        f.timeZone = TimeZone(identifier: "UTC")
        f.dateFormat = "yyyy-MM-dd"
        return f
    }()
}

// MARK: - search_products

/// `{"query", "needs"?, "store", "currency", "total", "considered"?: {"pages", "products"}, "as_of",
/// "method"?, "summary"?, "note"?, "options": [{"sku", "name", "brand"?, "model"?, "price"?,
/// "currency"?, "price_checked"?, "regular_price"?, "savings_pct"?, "url"?, "store"?, "images",
/// "description"?, "features", "specs": [{"name", "value"}], "category"?, "availability"?:
/// {"online"?, "in_store"?, "free_shipping"?}, "reviews": {"average", "count", "adjusted",
/// "verdict", "confidence", "summary"}, "badges"?, "why"?, "pros"?, "cons"?, "best_for"?,
/// "sources"?: [n], "comparable": [sku]}], "others": [...], "picks": {"top_pick"?, "best_value"?,
/// "best_rated"?, ...}, "sources"?: [{"n", "title", "url", "host"}], "disclaimer"}`
struct ProductsCard: Hashable, Sendable {
    /// What a product's reviews amount to, as cedar reads them.
    enum Verdict: String, Hashable, Sendable {
        case excellent, good, mixed, poor, few
        /// No rating found; named so it can't be mistaken for `Optional.none`.
        case unreviewed = "none"

        var label: String {
            switch self {
            case .excellent: return "Excellent"
            case .good: return "Good"
            case .mixed: return "Mixed"
            case .poor: return "Poor"
            case .few: return "Few reviews"
            case .unreviewed: return "No rating"
            }
        }
    }

    struct Reviews: Hashable, Sendable {
        /// 0...5
        let average: Double?
        let count: Int
        /// The average shrunk toward a typical rating by how few reviews there are, 0...5.
        let adjusted: Double?
        let verdict: Verdict
        /// low, medium, or high
        let confidence: String
        let summary: String?

        static func parse(_ r: JSONValue?) -> Reviews {
            let count = Int(CardParse.amount(r?["count"], max: 1e9) ?? 0)
            let fallback: Verdict = count > 0 ? .few : .unreviewed
            let verdict = r?["verdict"]?.stringValue.flatMap(Verdict.init(rawValue:)) ?? fallback
            let confidence = r?["confidence"]?.stringValue ?? ""
            return Reviews(
                average: count > 0 ? CardParse.amount(r?["average"], max: 5) : nil, count: count,
                adjusted: count > 0 ? CardParse.amount(r?["adjusted"], max: 5) : nil, verdict: verdict,
                confidence: ["low", "medium", "high"].contains(confidence) ? confidence : "low",
                summary: CardParse.text(r?["summary"], max: 600)
            )
        }
    }

    struct Spec: Hashable, Sendable {
        let name: String
        let value: String
    }

    /// A page the picks rest on, numbered as the model saw it.
    struct Source: Hashable, Sendable {
        let title: String
        let url: URL?
        let host: String?
    }

    struct Product: Hashable, Sendable, Identifiable {
        /// The store's sku, or the pick's id (`P1`).
        let id: String
        let name: String
        let brand: String?
        let model: String?
        let color: String?
        let condition: String?
        /// `nil` when no page stated one.
        let price: Double?
        /// A three-letter code; the card's when the product has none.
        let currency: String?
        /// The price comes from a store page or catalog, not from review or search text.
        let priceChecked: Bool
        /// Only when it is above `price`.
        let regularPrice: Double?
        let savingsPct: Double?
        let url: URL?
        /// Who sells it, e.g. "nike.com" or "Best Buy".
        let store: String?
        /// https only, the main photo first. Every `others` product has at most one.
        let images: [URL]
        let description: String?
        let features: [String]
        let specs: [Spec]
        let category: String?
        /// What the store said about stock; `nil` when it didn't.
        let online: Bool?
        let inStore: Bool?
        let freeShipping: Bool?
        let reviews: Reviews
        /// The badges the result gives it (`Top pick`, `Best value`, ...), if it names them.
        let badges: [String]?
        let why: String?
        let pros: [String]
        let cons: [String]
        let bestFor: String?
        /// Numbers of the card's sources behind it.
        let sources: [Int]
        /// Skus of other products in the card.
        var comparable: [String]
    }

    static let maxOptions = 8

    let query: String
    let store: String
    let currency: String
    let total: Int
    /// How many pages were read and products found, for a web search.
    let pages: Int?
    let productsFound: Int?
    let asOf: Date?
    /// How the options are ordered, in words.
    let sort: String?
    /// The model's recommendation, when it weighed the options.
    let summary: String?
    let note: String?
    let options: [Product]
    /// The rest of the search, which comparable options may point at.
    let others: [Product]
    /// The badges each option has.
    let badges: [String: [String]]
    let sources: [Int: Source]
    let disclaimer: String?

    func product(_ sku: String) -> Product? {
        options.first { $0.id == sku } ?? others.first { $0.id == sku }
    }

    /// The currency `product`'s price is in.
    func currency(of product: Product) -> String { product.currency ?? currency }

    private static let sorts = ["best_selling": "best selling first", "rating": "highest rated first",
                                "price_low": "cheapest first", "price_high": "priciest first"]
    private static let picks = [("top_pick", "Top pick"), ("best_value", "Best value"),
                                ("best_rated", "Best rated"), ("most_popular", "Most popular")]

    /// A sku given as a number or a string.
    static func sku(_ value: JSONValue?) -> String? {
        if let n = value?.doubleValue, n >= 0, n < 1e15 { return String(Int64(n.rounded())) }
        return CardParse.text(value, max: 40)
    }

    static func parse(_ value: JSONValue) -> ProductsCard? {
        guard let raw = value["options"]?.arrayValue else { return nil }
        var options = raw.prefix(maxOptions).compactMap(parseProduct)
        guard !options.isEmpty else { return nil }
        let shown = Set(options.map(\.id))
        var seen = shown
        let others = (value["others"]?.arrayValue ?? []).prefix(40).compactMap(parseProduct).filter { seen.insert($0.id).inserted }
        let known = seen
        for i in options.indices {
            let own = options[i].id
            options[i].comparable = Array(options[i].comparable.filter { $0 != own && known.contains($0) }.prefix(4))
        }
        var badges: [String: [String]] = [:]
        for (key, label) in picks {
            if let pick = sku(value["picks"]?[key]), shown.contains(pick) { badges[pick, default: []].append(label) }
        }
        // An option that names its own badges (the tool's web search does) keeps exactly those.
        for option in options {
            if let own = option.badges { badges[option.id] = own }
        }
        var sources: [Int: Source] = [:]
        for s in (value["sources"]?.arrayValue ?? []).prefix(40) {
            guard let n = CardParse.amount(s["n"], max: 1000).map({ Int($0) }),
                  let title = CardParse.text(s["title"]) ?? CardParse.text(s["host"], max: 80) else { continue }
            sources[n] = Source(title: title, url: CardParse.https(s["url"]), host: CardParse.text(s["host"], max: 80))
        }
        let considered = value["considered"]
        return ProductsCard(
            query: CardParse.text(value["query"], max: 160) ?? "Products",
            store: CardParse.text(value["store"], max: 60) ?? "the store",
            currency: currencyCode(value["currency"]) ?? "USD",
            total: max(options.count + others.count, Int(CardParse.amount(value["total"], max: 1e9) ?? 0)),
            pages: CardParse.amount(considered?["pages"], max: 1e6).map { Int($0) },
            productsFound: CardParse.amount(considered?["products"], max: 1e6).map { Int($0) },
            asOf: CardParse.time(value["as_of"]),
            sort: value["sort"]?.stringValue.flatMap { sorts[$0] },
            summary: CardParse.text(value["summary"], max: 800),
            note: CardParse.text(value["note"], max: 600),
            options: options, others: Array(others), badges: badges, sources: sources,
            disclaimer: CardParse.text(value["disclaimer"], max: 600)
        )
    }

    /// A three-letter currency code, upper-cased.
    private static func currencyCode(_ value: JSONValue?) -> String? {
        guard let c = CardParse.text(value, max: 4), c.count == 3, c.allSatisfy(\.isLetter) else { return nil }
        return c.uppercased()
    }

    private static func strings(_ value: JSONValue?, max: Int, count: Int) -> [String] {
        Array((value?.arrayValue ?? []).compactMap { CardParse.text($0, max: max) }.prefix(count))
    }

    private static func parseProduct(_ p: JSONValue) -> Product? {
        guard let id = sku(p["sku"]), let name = CardParse.text(p["name"]) else { return nil }
        let price = CardParse.amount(p["price"])
        let rawImages = p["images"]?.arrayValue ?? [p["image"] ?? .null]
        var images: [URL] = []
        for url in rawImages.compactMap(CardParse.https) where !images.contains(url) && images.count < 6 {
            images.append(url)
        }
        let regular = CardParse.amount(p["regular_price"]).flatMap { r -> Double? in
            guard let price, r > price else { return nil }
            return r
        }
        let availability = p["availability"]
        return Product(
            id: id, name: name,
            brand: CardParse.text(p["brand"], max: 80), model: CardParse.text(p["model"], max: 80),
            color: CardParse.text(p["color"], max: 60), condition: CardParse.text(p["condition"], max: 40),
            price: price, currency: currencyCode(p["currency"]),
            priceChecked: p["price_checked"]?.boolValue != false,
            regularPrice: regular,
            savingsPct: regular == nil ? nil : CardParse.amount(p["savings_pct"], max: 100),
            url: CardParse.https(p["url"]), store: CardParse.text(p["store"], max: 80), images: images,
            description: CardParse.text(p["description"], max: 600),
            features: Array((p["features"]?.arrayValue ?? []).compactMap { CardParse.text($0) }.prefix(6)),
            specs: Array((p["specs"]?.arrayValue ?? []).compactMap { s -> Spec? in
                guard let name = CardParse.text(s["name"], max: 80), let value = CardParse.text(s["value"]) else { return nil }
                return Spec(name: name, value: value)
            }.prefix(8)),
            category: CardParse.text(p["category"], max: 80),
            online: availability?["online"]?.boolValue,
            inStore: availability?["in_store"]?.boolValue,
            freeShipping: availability?["free_shipping"]?.boolValue,
            reviews: Reviews.parse(p["reviews"]),
            badges: p["badges"]?.arrayValue.map { items in items.compactMap { CardParse.text($0, max: 40) } },
            why: CardParse.text(p["why"], max: 600),
            pros: strings(p["pros"], max: 120, count: 4),
            cons: strings(p["cons"], max: 120, count: 4),
            bestFor: CardParse.text(p["best_for"], max: 160),
            sources: Array((p["sources"]?.arrayValue ?? []).compactMap { CardParse.amount($0, max: 1000).map { Int($0) } }.prefix(6)),
            comparable: (p["comparable"]?.arrayValue ?? []).compactMap { sku($0) }
        )
    }
}
