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
