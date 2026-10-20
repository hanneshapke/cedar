import XCTest
@testable import Cedar

final class CardsTests: XCTestCase {
    func testMapKeepsValidPinsAndDefaultsTheLabel() throws {
        let output = #"""
        {"title":"Coffee in Berlin","markers":[
          {"label":" The Barn ","latitude":52.5291,"longitude":13.4013},
          {"latitude":52.52,"longitude":13.405},
          {"label":"Nowhere","latitude":120,"longitude":0},
          {"label":"Text","latitude":"52","longitude":13}
        ],"not_found":["Bonanza",""," "]}
        """#
        guard case .map(let map)? = Card.parse(name: "show_map", output: output) else { return XCTFail("expected a map") }
        XCTAssertEqual(map.title, "Coffee in Berlin")
        XCTAssertEqual(map.markers.map(\.label), ["The Barn", "52.5200, 13.4050"])
        XCTAssertEqual(map.notFound, ["Bonanza"])
    }

    func testMapWithoutAValidPinIsNoCard() {
        XCTAssertNil(Card.parse(name: "show_map", output: #"{"markers":[{"latitude":100,"longitude":0}]}"#))
        XCTAssertNil(Card.parse(name: "show_map", output: "not json"))
        XCTAssertNil(Card.parse(name: "bash", output: #"{"markers":[{"latitude":1,"longitude":1}]}"#))
    }

    func testRecipe() throws {
        let output = #"""
        {"title":"Pancakes","servings":"4","prep_time":"10 min","cook_time":"","ingredients":[
          {"quantity":"200 g","item":"flour"},{"item":"2 eggs"},{"quantity":"1 cup"}
        ],"steps":[{"title":"Mix","text":"Whisk everything.","duration":"2 min"},{"text":"  "},{"text":"Fry."}],
        "tips":["Rest the batter."],
        "image":{"url":"http://example.com/p.jpg","page_url":"https://example.com","title":"P","source":"Wikimedia"}}
        """#
        guard case .recipe(let recipe)? = Card.parse(name: "show_recipe", output: output) else { return XCTFail("expected a recipe") }
        XCTAssertEqual(recipe.facts, [.init(label: "Prep", value: "10 min"), .init(label: "Serves", value: "4")])
        XCTAssertEqual(recipe.ingredients, [.init(quantity: "200 g", item: "flour"), .init(quantity: nil, item: "2 eggs")])
        XCTAssertEqual(recipe.steps.map(\.text), ["Whisk everything.", "Fry."])
        XCTAssertEqual(recipe.tips, ["Rest the batter."])
        XCTAssertNil(recipe.photo, "only https photos are used")
        XCTAssertNil(Card.parse(name: "show_recipe", output: #"{"title":"Toast","ingredients":[],"steps":[{"text":"Toast."}]}"#))
    }

    func testPolyline() {
        // Google's documented example.
        let points = Polyline.decode("_p~iF~ps|U_ulLnnqC_mqNvxq`@")
        XCTAssertEqual(points.count, 3)
        XCTAssertEqual(points[0].latitude, 38.5, accuracy: 1e-9)
        XCTAssertEqual(points[0].longitude, -120.2, accuracy: 1e-9)
        XCTAssertEqual(points[2].latitude, 43.252, accuracy: 1e-9)
        XCTAssertEqual(points[2].longitude, -126.453, accuracy: 1e-9)
        XCTAssertEqual(Polyline.decode("_p~iF~ps|U_ulLnnqC_mqNvxq"), [], "a truncated line is dropped")
        XCTAssertEqual(Polyline.decode("  "), [])
        XCTAssertEqual(Polyline.decode(nil), [])
    }

    func testTrip() throws {
        let output = #"""
        {"title":"To work","from":{"label":"Home","latitude":52.5,"longitude":13.4},
         "to":{"label":"Office","latitude":52.52,"longitude":13.41},
         "depart_at":"2026-10-19T08:30:00+02:00","time_zone":"Europe/Berlin",
         "recommendation":{"mode":"transit","option":1,"reason":"Fast and direct."},
         "options":[
           {"mode":"drive","duration_min":14,"distance_km":6.2,"path":"_p~iF~ps|U_ulLnnqC"},
           {"mode":"transit","duration_min":22,"transfers":0,"walk_min":6,
            "depart":"2026-10-19T08:31:00+02:00","arrive":"2026-10-19T08:53:00+02:00",
            "legs":[{"mode":"WALK","to":"Hbf","duration_min":3,"distance_m":240},
                    {"mode":"SUBURBAN","route":"S5","color":"#FF7700","headsign":"Spandau","from":"Hbf","to":"Zoo","stops":4,"duration_min":16},
                    {"mode":"WALK","to":"END","duration_min":3}]},
           {"mode":"teleport","duration_min":1},
           {"mode":"rideshare","duration_min":16,"cost":{"low":15,"high":22,"currency":"usd"}}
         ],
         "unavailable":[{"mode":"walk","reason":"Too far."},{"mode":"drive","reason":"dup"}],
         "attribution":"OSM"}
        """#
        guard case .trip(let trip)? = Card.parse(name: "plan_trip", output: output) else { return XCTFail("expected a trip") }
        XCTAssertEqual(trip.options.map(\.mode), [.drive, .transit, .rideshare])
        XCTAssertEqual(trip.options.map(\.id), [0, 1, 3], "ids are the result's indices")
        XCTAssertEqual(trip.recommendedOption?.mode, .transit)
        XCTAssertEqual(trip.reason, "Fast and direct.")
        XCTAssertEqual(trip.fastest?.mode, .drive)
        XCTAssertEqual(trip.timeZone?.identifier, "Europe/Berlin")
        XCTAssertEqual(trip.unavailable, [.init(mode: .walk, reason: "Too far.")], "modes with an option aren't unavailable")
        let transit = trip.options[1]
        XCTAssertEqual(transit.legs.map(\.kind), [.walk, .train, .walk])
        XCTAssertEqual(transit.legs[1].color, "#ff7700")
        XCTAssertEqual(transit.legs[1].route, "S5")
        XCTAssertNotNil(transit.depart)
        XCTAssertEqual(trip.options[0].path.count, 2)
        XCTAssertEqual(trip.options[2].cost, .init(low: 15, high: 22, currency: "USD"))
        XCTAssertEqual(TripStyle.duration(75), "1 h 15 min")
        XCTAssertEqual(TripStyle.money(trip.options[2].cost!), "~$15–22")
    }

    func testTripNeedsBothEndsAndAnOption() {
        XCTAssertNil(Card.parse(name: "plan_trip", output: #"{"from":{"latitude":1,"longitude":1},"to":{"latitude":2,"longitude":2},"options":[]}"#))
        XCTAssertNil(Card.parse(name: "plan_trip", output: #"{"from":{"latitude":1,"longitude":1},"options":[{"mode":"walk","duration_min":3}]}"#))
    }

    func testStocks() throws {
        let output = #"""
        {"range":"6mo","as_of":"2026-10-17T20:00:00Z",
         "benchmark":{"symbol":"SPY","name":"S&P 500","in_symbols":false},
         "benchmark_series":{"dates":["2026-10-15","2026-10-16","2026-10-17"],"close":[500,505,510]},
         "symbols":[
           {"symbol":"AAPL","name":"Apple","currency":"USD","price":230.5,"change":1.2,"change_pct":0.52,
            "returns":{"1w":1.5,"1y":20.1},"indicators":{"rsi14":55.2,"beta":1.1},
            "rating":{"call":"buy","score":4,"max_score":9,"confidence":"high","summary":"Strong.",
                      "factors":[{"name":"Trend","detail":"Above both averages.","score":3},{"name":"Empty"}],"watch":["Earnings"]},
            "chart":{"dates":["2026-10-15","2026-10-16","bad","2026-10-16","2026-10-17"],"close":[220,225,1,226,230.5]}},
           {"symbol":"NOPE","price":1,"chart":{"dates":["2026-10-17"],"close":[1]}}
         ],
         "news":[{"title":"Apple up","url":"https://news.example/a","publisher":"Wire","published":"2026-10-17T12:00:00Z","symbols":["AAPL","MSFT"]},
                 {"title":"Bad link","url":"javascript:alert(1)"}],
         "unavailable":[{"symbol":"MSFT","reason":"No data."}],
         "disclaimer":"Not advice."}
        """#
        guard case .stocks(let stocks)? = Card.parse(name: "compare_stocks", output: output) else { return XCTFail("expected stocks") }
        XCTAssertEqual(stocks.range, "6M")
        XCTAssertEqual(stocks.stocks.map(\.symbol), ["AAPL"], "a symbol with under two chart points is dropped")
        let apple = stocks.stocks[0]
        XCTAssertEqual(apple.series.closes, [220, 225, 230.5], "bad and repeated days are skipped")
        XCTAssertEqual(apple.returns.count, StocksCard.returnPeriods.count)
        XCTAssertEqual(apple.returns.first!, 1.5)
        XCTAssertNil(apple.returns[1])
        XCTAssertEqual(apple.rating?.call, .buy)
        XCTAssertEqual(apple.rating?.factors.map(\.score), [2], "scores are clamped and factors need a detail")
        XCTAssertEqual(stocks.benchmark?.series?.closes.count, 3)
        XCTAssertEqual(stocks.news.map(\.title), ["Apple up"])
        XCTAssertEqual(stocks.news[0].symbols, ["AAPL"])
        XCTAssertEqual(stocks.unavailable, [.init(symbol: "MSFT", reason: "No data.")])
        XCTAssertEqual(StockFormat.signed(-1.234), "−1.2%")
        XCTAssertEqual(StockFormat.money(230.5, "USD"), "$" + 230.5.formatted(.number.precision(.fractionLength(2))))
    }

    func testProducts() throws {
        let output = #"""
        {"query":"noise cancelling headphones","store":"Best Buy","currency":"usd","total":2,
         "as_of":"2026-10-20T09:12:00Z","sort":"price_low",
         "options":[
           {"sku":101,"name":" Sony WH-1000XM5 ","brand":"Sony","price":329.99,"regular_price":399.99,"savings_pct":17.5,
            "url":"https://www.bestbuy.com/site/101.p",
            "images":["https://img.example/101.jpg","http://img.example/plain.jpg","https://img.example/101.jpg","https://img.example/101-side.jpg"],
            "features":["30-hour battery",""],"specs":[{"name":"Weight","value":"250 g"},{"name":"Color"}],
            "category":"Over-Ear Headphones","availability":{"online":true,"in_store":false,"free_shipping":true},
            "reviews":{"average":4.7,"count":2345,"adjusted":4.7,"verdict":"excellent","confidence":"high","summary":"Well reviewed."},
            "comparable":[101,"202",999,303]},
           {"sku":"202","name":"Bose QC Ultra","price":299,"regular_price":299,"savings_pct":0,
            "reviews":{"average":null,"count":0,"verdict":"nonsense"},"comparable":[]},
           {"sku":303,"name":"No price"}
         ],
         "others":[{"sku":303,"name":"JBL Tune","price":69.99,"image":"https://img.example/303.jpg",
                    "reviews":{"average":5,"count":3,"adjusted":4.2,"verdict":"few","confidence":"low"}},
                   {"sku":101,"name":"Duplicate","price":1}],
         "picks":{"best_rated":101,"best_value":"101","most_popular":303},
         "disclaimer":"Prices can change."}
        """#
        guard case .products(let card)? = Card.parse(name: "search_products", output: output) else { return XCTFail("expected products") }
        XCTAssertEqual(card.currency, "USD")
        XCTAssertEqual(card.total, 3, "at least the products the card holds")
        XCTAssertEqual(card.sort, "cheapest first")
        XCTAssertEqual(card.options.map(\.id), ["101", "202"], "an option needs a price")
        XCTAssertEqual(card.others.map(\.id), ["303"], "an option already shown isn't repeated")
        let sony = card.options[0]
        XCTAssertEqual(sony.name, "Sony WH-1000XM5")
        XCTAssertEqual(sony.images.map(\.absoluteString), ["https://img.example/101.jpg", "https://img.example/101-side.jpg"],
                       "https only, without repeats")
        XCTAssertEqual(sony.regularPrice, 399.99)
        XCTAssertEqual(sony.savingsPct, 17.5)
        XCTAssertEqual(sony.features, ["30-hour battery"])
        XCTAssertEqual(sony.specs, [.init(name: "Weight", value: "250 g")])
        XCTAssertTrue(sony.online && sony.freeShipping && !sony.inStore)
        XCTAssertEqual(sony.reviews.verdict, .excellent)
        XCTAssertEqual(sony.reviews.average, 4.7)
        XCTAssertEqual(sony.comparable, ["202", "303"], "itself and unknown skus are dropped")
        let bose = card.options[1]
        XCTAssertNil(bose.regularPrice, "no sale, no regular price")
        XCTAssertNil(bose.savingsPct)
        XCTAssertNil(bose.reviews.average)
        XCTAssertEqual(bose.reviews.verdict, .unreviewed, "an unknown verdict falls back on the count")
        XCTAssertEqual(card.others[0].images.count, 1)
        XCTAssertEqual(card.others[0].reviews.verdict, .few)
        XCTAssertEqual(card.badges, ["101": ["Best rated", "Best value"]], "picks only mark shown options")
        XCTAssertEqual(card.product("303")?.name, "JBL Tune")
        XCTAssertNil(Card.parse(name: "search_products", output: #"{"options":[{"sku":1,"name":"x"}]}"#))
        XCTAssertNil(Card.parse(name: "search_products", output: #"{"options":[]}"#))
    }

    func testCardIsParsedWhenTheResultArrives() throws {
        var transcript = TurnTranscript(turn: Turn(id: "t1", conversationId: "c1", runId: "r1", prompt: "map", status: .running))
        transcript.apply(RunEvent(seq: 1, type: "tool.call", data: .object(["id": .string("m"), "name": .string("show_map"), "input": .object([:])])))
        let output = #"{"markers":[{"label":"A","latitude":1,"longitude":2}]}"#
        transcript.apply(RunEvent(seq: 2, type: "tool.result", data: .object(["id": .string("m"), "output": .string(output), "is_error": .bool(false)])))
        guard case .tool(let call)? = transcript.items.last else { return XCTFail("expected a tool call") }
        guard case .map(let map)? = call.card else { return XCTFail("expected a map card") }
        XCTAssertEqual(map.markers.first?.label, "A")
    }
}
