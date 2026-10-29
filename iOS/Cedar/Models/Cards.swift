import Foundation

/// A tool result the app draws as a card: `show_map`, `show_recipe`, `plan_trip`, `compare_stocks`,
/// `search_products`, `search_flights`, and the data of a `generate_chart` call. Parsed with the same rules as the web UI's
/// views (chat/static/map-view.js, recipe-view.js, transit-view.js, stock-view.js, product-view.js,
/// chart-view.js). Everything in a result came from the model or the network, so
/// text is clipped, numbers and coordinates are checked, and links are only kept if they are https.
enum Card: Hashable, Sendable {
    case map(MapCard)
    case recipe(RecipeCard)
    case trip(TripCard)
    case stocks(StocksCard)
    case products(ProductsCard)
    case flights(FlightsCard)
    case chart(ChartCard)

    /// The card for a finished call, or `nil` when it isn't a card tool or its result isn't usable.
    /// A chart is drawn from the call's `input`; its output only says where the picture was saved.
    static func parse(name: String, output: String, input: JSONValue = .null) -> Card? {
        guard ToolCall.cardTools.contains(name) else { return nil }
        if name == "generate_chart" { return ChartCard.parse(input).map(Card.chart) }
        guard let value = try? JSONDecoder().decode(JSONValue.self, from: Data(output.utf8)) else { return nil }
        switch name {
        case "show_map": return MapCard.parse(value).map(Card.map)
        case "show_recipe": return RecipeCard.parse(value).map(Card.recipe)
        case "plan_trip": return TripCard.parse(value).map(Card.trip)
        case "compare_stocks": return StocksCard.parse(value).map(Card.stocks)
        case "search_products": return ProductsCard.parse(value).map(Card.products)
        case "search_flights": return FlightsCard.parse(value).map(Card.flights)
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
            case .transit: return L10n.string("Transit")
            case .walk: return L10n.string("Walk")
            case .drive: return L10n.string("Drive")
            case .rideshare: return L10n.string("Rideshare")
            }
        }

        /// The recommendation's headline, "Take transit: 25 min", for a trip of `duration`.
        func take(_ duration: String) -> String {
            switch self {
            case .transit: return L10n.format("Take transit: %@", duration)
            case .walk: return L10n.format("Take walk: %@", duration)
            case .drive: return L10n.format("Take drive: %@", duration)
            case .rideshare: return L10n.format("Take rideshare: %@", duration)
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
            case .walk: return L10n.string("Walk")
            case .bike: return L10n.string("Bike")
            case .drive: return L10n.string("Drive")
            case .bus: return L10n.string("Bus")
            case .tram: return L10n.string("Tram")
            case .subway: return L10n.string("Subway")
            case .ferry: return L10n.string("Ferry")
            case .plane: return L10n.string("Flight")
            case .train: return L10n.string("Train")
            case .other: return L10n.string("Ride")
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
        var label: String { L10n.string(rawValue.capitalized) }
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
            case .excellent: return L10n.string("Excellent")
            case .good: return L10n.string("Good")
            case .mixed: return L10n.string("Mixed")
            case .poor: return L10n.string("Poor")
            case .few: return L10n.string("Few reviews")
            case .unreviewed: return L10n.string("No rating")
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
    /// How the options are ordered, in words (English, the key of its translation).
    let sort: String?
    /// The model's recommendation, when it weighed the options.
    let summary: String?
    let note: String?
    let options: [Product]
    /// The rest of the search, which comparable options may point at.
    let others: [Product]
    /// The badges each option has: the picks' in English, the keys of their translations, or the
    /// result's own.
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
            query: CardParse.text(value["query"], max: 160) ?? L10n.string("Products"),
            store: CardParse.text(value["store"], max: 60) ?? L10n.string("the store"),
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

// MARK: - search_flights

/// `{"kind": "flights", "leg", "from": [codes], "to": [codes], "date", "return_date"?, "passengers",
/// "cabin", "currency", "sort", "options": [{"id", "top_pick", "price", "airlines", "logo"?, "depart",
/// "arrive", "duration_min"?, "stops", "segments", "layovers", "co2_kg"?, "co2_vs_typical_pct"?}],
/// "more", "no_results"?, "insights"?, "google_flights_url"?, "as_of"}`, or for one chosen option
/// `{"kind": "booking", ..., "flights": [options], "options": [{"seller", "part", "price", "fare"?,
/// "bags", "marketed_as"}], "no_results"?, "google_flights_url"?}`. The data is Google Flights'
/// through SerpApi; the tokens and the note are for the model and aren't kept.
struct FlightsCard: Hashable, Sendable {
    enum Kind: String, Hashable, Sendable {
        case flights, booking
    }

    /// Which way the listed flights go: one way, a round trip's outbound flights (priced for the
    /// whole trip), or the return flights that go with a chosen outbound one.
    enum Leg: String, Hashable, Sendable {
        case oneWay = "one_way"
        case outbound
        case returning = "return"
    }

    enum Cabin: String, Hashable, Sendable {
        case economy
        case premiumEconomy = "premium_economy"
        case business, first

        var label: String {
            switch self {
            case .economy: return L10n.string("Economy")
            case .premiumEconomy: return L10n.string("Premium economy")
            case .business: return L10n.string("Business class")
            case .first: return L10n.string("First class")
            }
        }
    }

    struct Passengers: Hashable, Sendable {
        var adults = 1
        var children = 0
        var infantsInSeat = 0
        var infantsOnLap = 0

        static func parse(_ value: JSONValue?) -> Passengers {
            func count(_ key: String) -> Int { Int(CardParse.amount(value?[key], max: 9) ?? 0) }
            return Passengers(adults: count("adults"), children: count("children"),
                              infantsInSeat: count("infants_in_seat"), infantsOnLap: count("infants_on_lap"))
        }
    }

    /// A time as the airport's clock shows it, without a zone: never converted.
    struct LocalTime: Hashable, Sendable {
        /// `yyyy-MM-dd`, at midnight UTC.
        let day: Date
        /// `HH:mm`, as given.
        let clock: String

        /// `2026-12-01T15:40` (seconds, if any, are dropped).
        static func parse(_ value: JSONValue?) -> LocalTime? {
            guard let s = value?.stringValue, s.count >= 16, s.count <= 25 else { return nil }
            let chars = Array(s)
            guard chars[10] == "T" || chars[10] == " ", let day = StocksCard.day(String(chars[0..<10])),
                  chars[13] == ":", let h = Int(String(chars[11..<13])), let m = Int(String(chars[14..<16])),
                  (chars[11..<13] + chars[14..<16]).allSatisfy({ $0.isASCII && $0.isNumber }),
                  (0...23).contains(h), (0...59).contains(m) else { return nil }
            return LocalTime(day: day, clock: String(chars[11..<16]))
        }

        /// How many days after `other` this is (`+1` for an arrival the next morning).
        func days(after other: LocalTime) -> Int {
            Int((day.timeIntervalSince(other.day) / 86_400).rounded())
        }
    }

    /// Where a flight leaves from or lands.
    struct Airport: Hashable, Sendable {
        /// The IATA code; empty when Google gave none.
        let code: String
        let name: String?
        let time: LocalTime?

        /// The code, or the name when there is no code.
        var label: String { code.isEmpty ? (name ?? "?") : code }

        static func parse(_ value: JSONValue?) -> Airport {
            Airport(code: FlightsCard.code(value?["airport"]) ?? "", name: CardParse.text(value?["name"], max: 120),
                    time: LocalTime.parse(value?["time"]))
        }
    }

    /// One flight of an option.
    struct Segment: Hashable, Sendable {
        let flight: String?
        let airline: String?
        let logo: URL?
        let from: Airport
        let to: Airport
        let minutes: Int?
        let aircraft: String?
        let cabin: String?
        let legroom: String?
        let overnight: Bool
        let oftenDelayed: Bool
    }

    struct Layover: Hashable, Sendable {
        let airport: String
        let name: String?
        let minutes: Int?
        let overnight: Bool
    }

    /// A way to fly the leg: one or more flights, with the price for all passengers.
    struct Option: Hashable, Sendable, Identifiable {
        /// Its place in the card.
        let id: Int
        let topPick: Bool
        /// `nil` when Google gave no price.
        let price: Double?
        let airlines: [String]
        let logo: URL?
        let depart: Airport
        let arrive: Airport
        let minutes: Int?
        let stops: Int
        let segments: [Segment]
        let layovers: [Layover]
        /// Per passenger.
        let co2Kg: Int?
        /// Against a typical flight on the route; negative is less.
        let co2VsTypicalPct: Int?

        /// Days between departure and arrival, local dates (0 when unknown).
        var arrivalDayOffset: Int {
            guard let d = depart.time, let a = arrive.time else { return 0 }
            return a.days(after: d)
        }

        /// The airports it stops at: the layovers', else where its flights land on the way.
        var stopAirports: [String] {
            let codes = layovers.map(\.airport).filter { !$0.isEmpty }
            if !codes.isEmpty { return codes }
            return segments.dropLast().map(\.to.code).filter { !$0.isEmpty }
        }

        var oftenDelayed: Bool { segments.contains(where: \.oftenDelayed) }
        var overnight: Bool { segments.contains(where: \.overnight) || layovers.contains(where: \.overnight) }
    }

    struct PricePoint: Hashable, Sendable {
        let date: Date
        let price: Double
    }

    /// Google's verdict on the prices.
    struct Insights: Hashable, Sendable {
        let lowest: Double
        /// `low`, `typical`, `high`, or another word of Google's.
        let level: String?
        let typicalLow: Double?
        let typicalHigh: Double?
        /// Oldest first, at least two points.
        let history: [PricePoint]
    }

    /// Who sells a chosen flight, for how much.
    struct Seller: Hashable, Sendable, Identifiable {
        enum Part: String, Hashable, Sendable {
            /// The whole trip on one ticket.
            case together
            /// One way, on a ticket of its own.
            case departing, returning
        }

        let id: Int
        let name: String
        let part: Part
        let price: Double?
        let fare: String?
        let bags: [String]
        let marketedAs: [String]
    }

    static let maxOptions = 20
    static let maxSellers = 12
    static let maxAirports = 7

    let kind: Kind
    let leg: Leg
    let from: [String]
    let to: [String]
    /// The day of this leg, at midnight UTC.
    let date: Date?
    let returnDate: Date?
    let passengers: Passengers
    let cabin: Cabin?
    let currency: String
    /// A search's options; empty for a booking.
    let options: [Option]
    /// Options the search found that aren't listed.
    let more: Int
    let noResults: Bool
    let insights: Insights?
    let googleFlightsURL: URL?
    let asOf: Date?
    /// A booking's chosen flight(s).
    let chosen: [Option]
    let sellers: [Seller]

    /// "SFO/OAK → MUC"
    var route: String { Self.route(from: from, to: to) }

    static func route(from: [String], to: [String]) -> String {
        "\(from.joined(separator: "/")) → \(to.joined(separator: "/"))"
    }

    /// The route a `search_flights` call asks for, from its input, or `nil` without both ends.
    static func subject(_ input: JSONValue) -> String? {
        let from = airports(input["from"]), to = airports(input["to"])
        guard !from.isEmpty, !to.isEmpty else { return nil }
        return route(from: from, to: to)
    }

    /// Airport codes, as a list or a comma-separated string, upper-cased.
    static func airports(_ value: JSONValue?) -> [String] {
        let raw: [JSONValue]
        if let list = value?.arrayValue {
            raw = list
        } else if let s = value?.stringValue {
            raw = s.split(separator: ",").map { JSONValue.string(String($0)) }
        } else {
            raw = []
        }
        var out: [String] = []
        for code in raw.compactMap(code) where !out.contains(code) && out.count < maxAirports {
            out.append(code)
        }
        return out
    }

    /// An airport code: letters and digits, at most 4, upper-cased.
    static func code(_ value: JSONValue?) -> String? {
        guard let s = CardParse.text(value, max: 5), s.count <= 4,
              s.allSatisfy({ $0.isASCII && ($0.isLetter || $0.isNumber) }) else { return nil }
        return s.uppercased()
    }

    static func parse(_ value: JSONValue) -> FlightsCard? {
        guard let kind = value["kind"]?.stringValue.flatMap(Kind.init(rawValue:)),
              let rawOptions = value["options"]?.arrayValue else { return nil }
        let noResults = CardParse.text(value["no_results"]) != nil || value["no_results"]?.boolValue == true
        var options: [Option] = []
        var chosen: [Option] = []
        var sellers: [Seller] = []
        switch kind {
        case .flights:
            options = rawOptions.prefix(maxOptions).compactMap(option).enumerated().map { i, o in o.with(id: i) }
            guard !options.isEmpty || noResults else { return nil }
        case .booking:
            chosen = (value["flights"]?.arrayValue ?? []).prefix(4).compactMap(option).enumerated().map { i, o in o.with(id: i) }
            sellers = rawOptions.prefix(maxSellers).compactMap(seller).enumerated().map { i, s in
                Seller(id: i, name: s.name, part: s.part, price: s.price, fare: s.fare, bags: s.bags, marketedAs: s.marketedAs)
            }
            guard !chosen.isEmpty || !sellers.isEmpty || noResults else { return nil }
        }
        return FlightsCard(
            kind: kind,
            leg: value["leg"]?.stringValue.flatMap(Leg.init(rawValue:)) ?? .oneWay,
            from: airports(value["from"]), to: airports(value["to"]),
            date: value["date"]?.stringValue.flatMap(StocksCard.day),
            returnDate: value["return_date"]?.stringValue.flatMap(StocksCard.day),
            passengers: Passengers.parse(value["passengers"]),
            cabin: value["cabin"]?.stringValue.flatMap(Cabin.init(rawValue:)),
            currency: currencyCode(value["currency"]) ?? "USD",
            options: options,
            more: kind == .flights ? Int(CardParse.amount(value["more"], max: 1000) ?? 0) : 0,
            noResults: noResults && options.isEmpty && sellers.isEmpty,
            insights: kind == .flights ? insights(value["insights"]) : nil,
            googleFlightsURL: CardParse.https(value["google_flights_url"]),
            asOf: CardParse.time(value["as_of"]),
            chosen: chosen, sellers: sellers
        )
    }

    /// A three-letter currency code, upper-cased.
    private static func currencyCode(_ value: JSONValue?) -> String? {
        guard let c = CardParse.text(value, max: 4), c.count == 3, c.allSatisfy({ $0.isASCII && $0.isLetter }) else { return nil }
        return c.uppercased()
    }

    private static func minutes(_ value: JSONValue?) -> Int? {
        CardParse.amount(value, max: 10_000).map { Int($0.rounded()) }
    }

    private static func strings(_ value: JSONValue?, max: Int, count: Int) -> [String] {
        Array((value?.arrayValue ?? []).compactMap { CardParse.text($0, max: max) }.prefix(count))
    }

    private static func option(_ o: JSONValue) -> Option? {
        guard case .object = o, let rawSegments = o["segments"]?.arrayValue else { return nil }
        let segments = rawSegments.prefix(8).compactMap { s -> Segment? in
            guard case .object = s else { return nil }
            return Segment(
                flight: CardParse.text(s["flight"], max: 20), airline: CardParse.text(s["airline"], max: 80),
                logo: CardParse.https(s["logo"]),
                from: Airport.parse(s["from"]), to: Airport.parse(s["to"]),
                minutes: minutes(s["duration_min"]), aircraft: CardParse.text(s["aircraft"], max: 80),
                cabin: CardParse.text(s["cabin"], max: 40), legroom: CardParse.text(s["legroom"], max: 20),
                overnight: s["overnight"]?.boolValue == true, oftenDelayed: s["often_delayed"]?.boolValue == true
            )
        }
        guard let first = segments.first, let last = segments.last else { return nil }
        let layovers = (o["layovers"]?.arrayValue ?? []).prefix(8).compactMap { l -> Layover? in
            guard case .object = l else { return nil }
            return Layover(airport: code(l["airport"]) ?? "", name: CardParse.text(l["name"], max: 120),
                           minutes: minutes(l["duration_min"]), overnight: l["overnight"]?.boolValue == true)
        }
        var airlines = strings(o["airlines"], max: 80, count: 4)
        if airlines.isEmpty {
            for a in segments.compactMap(\.airline) where !airlines.contains(a) && airlines.count < 4 { airlines.append(a) }
        }
        let depart = o["depart"].map { Airport.parse($0) } ?? first.from
        let arrive = o["arrive"].map { Airport.parse($0) } ?? last.to
        return Option(
            id: 0, topPick: o["top_pick"]?.boolValue == true, price: CardParse.amount(o["price"]),
            airlines: airlines, logo: CardParse.https(o["logo"]) ?? first.logo,
            depart: depart, arrive: arrive, minutes: minutes(o["duration_min"]),
            stops: Int(CardParse.amount(o["stops"], max: 10) ?? Double(segments.count - 1)),
            segments: Array(segments), layovers: Array(layovers),
            co2Kg: CardParse.amount(o["co2_kg"], max: 1e6).map { Int($0.rounded()) },
            co2VsTypicalPct: o["co2_vs_typical_pct"]?.doubleValue.flatMap { abs($0) <= 1000 ? Int($0.rounded()) : nil }
        )
    }

    private static func seller(_ s: JSONValue) -> Seller? {
        guard let name = CardParse.text(s["seller"], max: 80) else { return nil }
        return Seller(
            id: 0, name: name,
            part: s["part"]?.stringValue.flatMap(Seller.Part.init(rawValue:)) ?? .together,
            price: CardParse.amount(s["price"]), fare: CardParse.text(s["fare"], max: 80),
            bags: strings(s["bags"], max: 120, count: 6), marketedAs: strings(s["marketed_as"], max: 20, count: 6)
        )
    }

    private static func insights(_ v: JSONValue?) -> Insights? {
        guard let v, let lowest = CardParse.amount(v["lowest"]) else { return nil }
        var low: Double?
        var high: Double?
        if let typical = v["typical"]?.arrayValue, typical.count == 2,
           let a = CardParse.amount(typical[0]), let b = CardParse.amount(typical[1]) {
            low = min(a, b)
            high = max(a, b)
        }
        var history: [PricePoint] = []
        for p in (v["history"]?.arrayValue ?? []).suffix(90) {
            guard let date = p["date"]?.stringValue.flatMap(StocksCard.day), let price = CardParse.amount(p["price"]) else { continue }
            if let last = history.last, date <= last.date { continue }
            history.append(PricePoint(date: date, price: price))
        }
        return Insights(lowest: lowest, level: CardParse.text(v["level"], max: 40)?.lowercased(),
                        typicalLow: low, typicalHigh: high, history: history.count >= 2 ? history : [])
    }
}

extension FlightsCard.Option {
    /// The same option at place `id` in the card.
    func with(id: Int) -> FlightsCard.Option {
        FlightsCard.Option(id: id, topPick: topPick, price: price, airlines: airlines, logo: logo, depart: depart,
                           arrive: arrive, minutes: minutes, stops: stops, segments: segments, layovers: layovers,
                           co2Kg: co2Kg, co2VsTypicalPct: co2VsTypicalPct)
    }
}

// MARK: - generate_chart

/// A `generate_chart` call's input: `{"path", "kind", "labels"?, "series": [{"name"?, "values"}],
/// "title"?, "x_label"?, "y_label"?, "stacked"?, "bins"?, "log_scale"?}`. Long series are thinned to
/// what a card can show; the saved picture has every point.
struct ChartCard: Hashable, Sendable {
    enum Kind: String, Sendable {
        case line, bar, barh, area, scatter, pie, histogram

        var name: String {
            switch self {
            case .line: return L10n.string("Line chart")
            case .bar, .barh: return L10n.string("Bar chart")
            case .area: return L10n.string("Area chart")
            case .scatter: return L10n.string("Scatter plot")
            case .pie: return L10n.string("Pie chart")
            case .histogram: return L10n.string("Histogram")
            }
        }

        /// The name of a stacked chart of this kind (only bars, areas and histograms stack).
        var stackedName: String {
            switch self {
            case .bar, .barh: return L10n.string("Stacked bar chart")
            case .area: return L10n.string("Stacked area chart")
            case .histogram: return L10n.string("Stacked histogram")
            case .line, .scatter, .pie: return name
            }
        }
    }

    /// An x-axis category or value.
    enum Label: Hashable, Sendable {
        case text(String)
        case number(Double)

        var text: String {
            switch self {
            case .text(let s): return s
            case .number(let n): return ChartCard.format(n)
            }
        }

        var number: Double? {
            if case .number(let n) = self { return n }
            return nil
        }
    }

    struct Series: Hashable, Sendable {
        let name: String
        /// Whether the model named it, rather than it being "Series 2".
        let named: Bool
        /// One per label; `nil` is a gap. For a histogram, the raw samples.
        let values: [Double?]
    }

    let kind: Kind
    let path: String?
    let title: String?
    let xLabel: String?
    let yLabel: String?
    let labels: [Label]
    let series: [Series]
    let stacked: Bool
    let bins: Int?
    let log: Bool
    /// A shaded range on the value axis, such as a target range.
    let band: Band?
    let thinned: Bool

    struct Band: Hashable, Sendable {
        let from: Double
        let to: Double
        let label: String?
    }

    static let maxSeries = 20
    static let maxValues = 100_000
    /// The categorical palette's size; later series are drawn in grey.
    static let slots = 8

    /// Whether the x values are numbers (a scatter, or a line or area over numbers).
    var numericX: Bool {
        kind == .scatter || ((kind == .line || kind == .area) && labels.allSatisfy { $0.number != nil })
    }

    static func parse(_ value: JSONValue) -> ChartCard? {
        guard let kind = value["kind"]?.stringValue.flatMap(Kind.init(rawValue:)),
              let rawSeries = value["series"]?.arrayValue, !rawSeries.isEmpty
        else { return nil }
        var series: [Series] = []
        for (i, s) in rawSeries.prefix(maxSeries).enumerated() {
            guard let raw = s["values"]?.arrayValue, !raw.isEmpty else { return nil }
            var values: [Double?] = []
            for v in raw.prefix(maxValues) {
                if case .null = v { values.append(nil); continue }
                guard let n = v.doubleValue else { return nil }
                values.append(n)
            }
            let name = CardParse.text(s["name"], max: 80)
            series.append(Series(name: name ?? L10n.format("Series %lld", i + 1), named: name != nil, values: values))
        }
        var labels: [Label] = []
        if kind != .histogram {
            guard let raw = value["labels"]?.arrayValue, !raw.isEmpty else { return nil }
            for l in raw.prefix(maxValues) {
                if let n = l.doubleValue { labels.append(.number(n)) } else if let s = l.stringValue { labels.append(.text(String(s.prefix(120)))) } else { return nil }
            }
            guard series.allSatisfy({ $0.values.count == labels.count }) else { return nil }
            if kind == .scatter, !labels.allSatisfy({ $0.number != nil }) { return nil }
        }
        if kind == .pie {
            guard series.count == 1, series[0].values.allSatisfy({ ($0 ?? -1) >= 0 }),
                  series[0].values.contains(where: { ($0 ?? 0) > 0 }) else { return nil }
        }

        var thinned = false
        if kind != .histogram, kind != .pie {
            let limit: Int
            switch kind {
            case .bar: limit = 300
            case .barh: limit = 60
            case .scatter: limit = 2000
            default: limit = 800
            }
            if labels.count > limit {
                thinned = true
                let step = Double(labels.count) / Double(limit)
                var keep = (0..<limit).map { Int(Double($0) * step) }
                if keep.last != labels.count - 1 { keep.append(labels.count - 1) }
                labels = keep.map { labels[$0] }
                series = series.map { s in Series(name: s.name, named: s.named, values: keep.map { s.values[$0] }) }
            }
        }
        let bins = value["bins"]?.doubleValue.flatMap { (1...500).contains($0) ? Int($0) : nil }
        var band: Band?
        if kind != .pie, let a = value["band"]?["from"]?.doubleValue, let b = value["band"]?["to"]?.doubleValue, a != b {
            band = Band(from: min(a, b), to: max(a, b), label: CardParse.text(value["band"]?["label"], max: 40))
        }
        return ChartCard(
            kind: kind,
            path: CardParse.text(value["path"], max: 300),
            title: CardParse.text(value["title"]),
            xLabel: CardParse.text(value["x_label"], max: 120),
            yLabel: CardParse.text(value["y_label"], max: 120),
            labels: labels,
            series: series,
            stacked: value["stacked"]?.boolValue == true && [.bar, .barh, .area, .histogram].contains(kind),
            bins: bins,
            log: value["log_scale"]?.boolValue == true && kind != .pie && kind != .histogram,
            band: band,
            thinned: thinned
        )
    }

    static let maxOutputCharts = 6

    /// The charts a tool printed as `[chart] {spec}` lines (generate_chart's input, without a path),
    /// and its output without them; the same rule as the web UI's `cedarChart.fromOutput`.
    static func inOutput(_ output: String) -> (charts: [ChartCard], text: String) {
        guard output.contains("[chart] ") else { return ([], output) }
        var charts: [ChartCard] = []
        var kept: [Substring] = []
        for line in output.split(separator: "\n", omittingEmptySubsequences: false) {
            if charts.count < maxOutputCharts, line.hasPrefix("[chart] {"),
               let value = try? JSONDecoder().decode(JSONValue.self, from: Data(line.dropFirst("[chart] ".count).utf8)),
               case .object(var fields) = value {
                fields["path"] = nil
                if let chart = ChartCard.parse(.object(fields)) {
                    charts.append(chart)
                    continue
                }
            }
            kept.append(line)
        }
        guard !charts.isEmpty else { return ([], output) }
        return (charts, kept.joined(separator: "\n").trimmingCharacters(in: .whitespacesAndNewlines))
    }

    /// Histogram bins over every series' samples: the edges, and the counts per series.
    var histogram: (edges: [Double], counts: [[Int]])? {
        let samples = series.map { $0.values.compactMap { $0 } }
        let all = samples.flatMap { $0 }
        guard var lo = all.min(), var hi = all.max() else { return nil }
        if lo == hi { lo -= 0.5; hi += 0.5 }
        let n = bins ?? max(5, min(40, Int((log2(Double(all.count)) + 1).rounded(.up))))
        let width = (hi - lo) / Double(n)
        let edges = (0...n).map { lo + Double($0) * width }
        let counts = samples.map { values in
            var c = [Int](repeating: 0, count: n)
            for v in values { c[min(n - 1, Int((v - lo) / width))] += 1 }
            return c
        }
        return (edges, counts)
    }

    /// A number as it reads in a tooltip or table.
    static func format(_ v: Double) -> String {
        let a = abs(v)
        let digits = a >= 1000 ? 0 : a >= 100 ? 1 : a >= 1 ? 2 : a == 0 ? 0 : 4
        return v.formatted(.number.precision(.fractionLength(0...digits)).locale(L10n.locale))
    }
}
