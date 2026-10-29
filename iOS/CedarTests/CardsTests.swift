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
        {"query":"trail running shoes","store":"the web","currency":"usd","total":2,
         "considered":{"pages":3,"products":5},"as_of":"2026-10-20T09:12:00Z","method":"model",
         "summary":" Get the Trail Runner 5. ","note":"",
         "options":[
           {"sku":"P1","name":" Trail Runner 5 ","brand":"Hoka","price":129.95,"currency":"USD","price_checked":true,
            "regular_price":159.95,"savings_pct":18.8,"url":"https://shop.example/tr5","store":"shop.example",
            "images":["https://img.example/tr5.jpg","http://img.example/plain.jpg","https://img.example/tr5.jpg"],
            "features":["Vibram sole",""],"specs":[{"name":"Drop","value":"5 mm"},{"name":"Weight"}],
            "availability":{"online":true},
            "reviews":{"average":4.6,"count":1204,"adjusted":4.6,"verdict":"excellent","confidence":"high","summary":"Well reviewed."},
            "badges":["Top pick","Best rated"],"why":"Grippy and roomy.","pros":["grip","",  "comfort"],"cons":["warm"],
            "best_for":"muddy trails","sources":[2,1,99],"comparable":["P2","P1","P9"]},
           {"sku":"P2","name":"Cloud 5","price":139.95,"currency":"eur","price_checked":false,"url":"https://reviews.example/a",
            "images":[],"reviews":{"average":null,"count":0,"verdict":"none"},"badges":[],"comparable":[]},
           {"sku":"P3","name":"Peregrine 14","price":null,"url":"https://brand.example/p",
            "reviews":{"verdict":"nonsense"},"comparable":[]},
           {"name":"No sku"}
         ],
         "others":[],
         "picks":{"top_pick":"P1","best_value":"P3"},
         "sources":[{"n":1,"title":"The best trail shoes","url":"https://reviews.example/a","host":"reviews.example"},
                    {"n":2,"title":"Trail Runner 5","url":"http://shop.example/tr5","host":"shop.example"},
                    {"n":3,"title":""}],
         "disclaimer":"Check the store."}
        """#
        guard case .products(let card)? = Card.parse(name: "search_products", output: output) else { return XCTFail("expected products") }
        XCTAssertEqual(card.currency, "USD")
        XCTAssertEqual(card.total, 3, "at least the products the card holds")
        XCTAssertEqual(card.pages, 3)
        XCTAssertEqual(card.productsFound, 5)
        XCTAssertEqual(card.summary, "Get the Trail Runner 5.")
        XCTAssertNil(card.note)
        XCTAssertEqual(card.options.map(\.id), ["P1", "P2", "P3"], "an option needs a sku and a name, not a price")

        let first = card.options[0]
        XCTAssertEqual(first.name, "Trail Runner 5")
        XCTAssertEqual(first.images.map(\.absoluteString), ["https://img.example/tr5.jpg"], "https only, without repeats")
        XCTAssertEqual(first.price, 129.95)
        XCTAssertTrue(first.priceChecked)
        XCTAssertEqual(first.regularPrice, 159.95)
        XCTAssertEqual(first.store, "shop.example")
        XCTAssertEqual(first.online, true)
        XCTAssertNil(first.inStore, "a store that doesn't say isn't taken to say no")
        XCTAssertEqual(first.features, ["Vibram sole"])
        XCTAssertEqual(first.specs, [.init(name: "Drop", value: "5 mm")])
        XCTAssertEqual(first.why, "Grippy and roomy.")
        XCTAssertEqual(first.pros, ["grip", "comfort"])
        XCTAssertEqual(first.bestFor, "muddy trails")
        XCTAssertEqual(first.sources, [2, 1, 99])
        XCTAssertEqual(first.comparable, ["P2"], "itself and unknown skus are dropped")
        XCTAssertEqual(card.badges["P1"], ["Top pick", "Best rated"], "an option's own badges win over the picks")

        let cloud = card.options[1]
        XCTAssertFalse(cloud.priceChecked)
        XCTAssertEqual(cloud.currency, "EUR")
        XCTAssertEqual(card.currency(of: cloud), "EUR")
        XCTAssertEqual(cloud.reviews.verdict, .unreviewed)
        XCTAssertEqual(card.badges["P2"], [], "an empty list of badges is kept")

        let peregrine = card.options[2]
        XCTAssertNil(peregrine.price)
        XCTAssertNil(peregrine.regularPrice)
        XCTAssertNil(peregrine.online)
        XCTAssertEqual(card.currency(of: peregrine), "USD")
        XCTAssertEqual(peregrine.reviews.verdict, .unreviewed, "an unknown verdict falls back on the count")
        XCTAssertEqual(card.badges["P3"], ["Best value"], "without badges of its own, the picks give them")

        XCTAssertEqual(card.sources[1]?.url?.absoluteString, "https://reviews.example/a")
        XCTAssertNil(card.sources[2]?.url, "only https links")
        XCTAssertEqual(card.sources[2]?.title, "Trail Runner 5")
        XCTAssertNil(card.sources[3], "a source needs a title")
        XCTAssertNil(Card.parse(name: "search_products", output: #"{"options":[{"name":"x"}]}"#))
        XCTAssertNil(Card.parse(name: "search_products", output: #"{"options":[]}"#))
    }

    func testFlights() throws {
        let output = #"""
        {"kind":"flights","leg":"outbound","from":["sfo"],"to":["MUC"],"date":"2026-12-01","return_date":"2026-12-10",
         "passengers":{"adults":2,"children":1,"infants_in_seat":0,"infants_on_lap":"1"},"cabin":"premium_economy",
         "currency":"chf","sort":"best",
         "options":[
           {"id":1,"top_pick":true,"price":1234.0,"airlines":["Lufthansa",""],
            "logo":"https://www.gstatic.com/flights/airline_logos/70px/LH.png",
            "depart":{"airport":"SFO","name":"San Francisco International Airport","time":"2026-12-01T15:40"},
            "arrive":{"airport":"MUC","name":"Munich International Airport","time":"2026-12-02T11:30"},
            "duration_min":650,"stops":0,
            "segments":[{"flight":"LH 455","airline":"Lufthansa","logo":"http://example.com/lh.png",
              "from":{"airport":"SFO","time":"2026-12-01T15:40"},"to":{"airport":"MUC","time":"2026-12-02T11:30"},
              "duration_min":650,"aircraft":"Airbus A350","cabin":"Economy","legroom":"31 in",
              "overnight":true,"often_delayed":true}],
            "layovers":[],"co2_kg":512,"co2_vs_typical_pct":-7,"departure_token":"DEP1"},
           {"id":2,"top_pick":false,"price":null,"airlines":["United"],
            "depart":{"airport":"SFO","time":"2026-12-01T07:00"},"arrive":{"airport":"MUC","time":"2026-12-02T25:99"},
            "stops":1,
            "segments":[{"flight":"UA 58","from":{"airport":"SFO"},"to":{"airport":"EWR"}},
                        {"flight":"UA 30","from":{"airport":"EWR"},"to":{"airport":"MUC"}}],
            "layovers":[{"airport":"EWR","name":"Newark","duration_min":120,"overnight":false}]},
           {"id":3,"price":-5,"segments":[]},
           {"id":4,"price":900}
         ],
         "more":12,
         "insights":{"lowest":999.0,"level":"Low","typical":[1600.0,1100.0],
                     "history":[{"date":"2026-10-01","price":1300},{"date":"2026-10-01","price":1200},
                                {"date":"bad","price":1},{"date":"2026-10-02","price":1250}]},
         "google_flights_url":"https://www.google.com/travel/flights?tfs=x",
         "as_of":"2026-10-29T02:43:00Z","note":"For the model."}
        """#
        guard case .flights(let card)? = Card.parse(name: "search_flights", output: output) else { return XCTFail("expected flights") }
        XCTAssertEqual(card.kind, .flights)
        XCTAssertEqual(card.leg, .outbound)
        XCTAssertEqual(card.route, "SFO → MUC")
        XCTAssertEqual(card.date, StocksCard.day("2026-12-01"))
        XCTAssertEqual(card.returnDate, StocksCard.day("2026-12-10"))
        XCTAssertEqual(card.passengers, .init(adults: 2, children: 1, infantsInSeat: 0, infantsOnLap: 0), "counts must be numbers")
        XCTAssertEqual(card.cabin, .premiumEconomy)
        XCTAssertEqual(card.currency, "CHF")
        XCTAssertEqual(card.more, 12)
        XCTAssertFalse(card.noResults)
        XCTAssertEqual(card.googleFlightsURL?.absoluteString, "https://www.google.com/travel/flights?tfs=x")
        XCTAssertNotNil(card.asOf)
        XCTAssertEqual(card.options.map(\.id), [0, 1], "an option needs its flights")

        let lh = card.options[0]
        XCTAssertTrue(lh.topPick)
        XCTAssertEqual(lh.price, 1234)
        XCTAssertEqual(lh.airlines, ["Lufthansa"])
        XCTAssertEqual(lh.logo?.absoluteString, "https://www.gstatic.com/flights/airline_logos/70px/LH.png")
        XCTAssertNil(lh.segments[0].logo, "only https logos")
        XCTAssertEqual(lh.depart.code, "SFO")
        XCTAssertEqual(lh.depart.name, "San Francisco International Airport")
        XCTAssertEqual(lh.depart.time?.clock, "15:40")
        XCTAssertEqual(lh.arrive.time?.clock, "11:30")
        XCTAssertEqual(lh.arrivalDayOffset, 1)
        XCTAssertEqual(lh.minutes, 650)
        XCTAssertEqual(lh.stops, 0)
        XCTAssertEqual(lh.segments[0].flight, "LH 455")
        XCTAssertEqual(lh.segments[0].legroom, "31 in")
        XCTAssertTrue(lh.oftenDelayed)
        XCTAssertTrue(lh.overnight)
        XCTAssertEqual(lh.co2Kg, 512)
        XCTAssertEqual(lh.co2VsTypicalPct, -7)

        let ua = card.options[1]
        XCTAssertNil(ua.price)
        XCTAssertFalse(ua.topPick)
        XCTAssertNil(ua.arrive.time, "an impossible time is dropped")
        XCTAssertEqual(ua.arrivalDayOffset, 0)
        XCTAssertEqual(ua.stops, 1)
        XCTAssertEqual(ua.stopAirports, ["EWR"])
        XCTAssertEqual(ua.layovers, [.init(airport: "EWR", name: "Newark", minutes: 120, overnight: false)])
        XCTAssertNil(ua.minutes)

        let insights = try XCTUnwrap(card.insights)
        XCTAssertEqual(insights.lowest, 999)
        XCTAssertEqual(insights.level, "low")
        XCTAssertEqual(insights.typicalLow, 1100)
        XCTAssertEqual(insights.typicalHigh, 1600)
        XCTAssertEqual(insights.history.map(\.price), [1300, 1250], "bad and repeated days are skipped")
    }

    func testFlightsBooking() throws {
        let output = #"""
        {"kind":"booking","leg":"one_way","from":["SFO"],"to":["MUC"],"date":"2026-12-01",
         "passengers":{"adults":1},"cabin":"business","currency":"USD",
         "flights":[{"id":1,"price":1234,"airlines":["Lufthansa"],
           "depart":{"airport":"SFO","time":"2026-12-01T15:40"},"arrive":{"airport":"MUC","time":"2026-12-02T11:30"},
           "stops":0,"segments":[{"flight":"LH 455","from":{"airport":"SFO"},"to":{"airport":"MUC"}}],"layovers":[]}],
         "options":[
           {"seller":"Lufthansa","part":"together","price":1234,"fare":"Economy Light",
            "bags":["1 free carry-on","","1st checked bag: 60 USD"],"marketed_as":["LH 455"]},
           {"seller":"Expedia","part":"departing","price":null,"bags":[],"marketed_as":[]},
           {"seller":"","part":"returning","price":10},
           {"part":"returning","price":10}
         ],
         "google_flights_url":"http://www.google.com/travel/flights","note":"..."}
        """#
        guard case .flights(let card)? = Card.parse(name: "search_flights", output: output) else { return XCTFail("expected a booking") }
        XCTAssertEqual(card.kind, .booking)
        XCTAssertEqual(card.leg, .oneWay)
        XCTAssertEqual(card.cabin, .business)
        XCTAssertEqual(card.passengers.adults, 1)
        XCTAssertTrue(card.options.isEmpty)
        XCTAssertNil(card.insights)
        XCTAssertEqual(card.chosen.count, 1)
        XCTAssertEqual(card.chosen[0].segments[0].flight, "LH 455")
        XCTAssertEqual(card.sellers.map(\.name), ["Lufthansa", "Expedia"], "a seller needs a name")
        XCTAssertEqual(card.sellers[0].part, .together)
        XCTAssertEqual(card.sellers[0].price, 1234)
        XCTAssertEqual(card.sellers[0].fare, "Economy Light")
        XCTAssertEqual(card.sellers[0].bags, ["1 free carry-on", "1st checked bag: 60 USD"])
        XCTAssertEqual(card.sellers[0].marketedAs, ["LH 455"])
        XCTAssertEqual(card.sellers[1].part, .departing)
        XCTAssertNil(card.sellers[1].price)
        XCTAssertFalse(card.noResults)
        XCTAssertNil(card.googleFlightsURL, "only https links")
    }

    func testFlightsWithoutResultsOrUsableData() throws {
        let none = #"{"kind":"flights","leg":"one_way","from":["SFO"],"to":["MUC"],"currency":"EUR","options":[],"more":0,"no_results":"Google Flights found no flights."}"#
        guard case .flights(let card)? = Card.parse(name: "search_flights", output: none) else { return XCTFail("expected flights") }
        XCTAssertTrue(card.noResults)
        XCTAssertTrue(card.options.isEmpty)

        let soldOut = #"{"kind":"booking","flights":[],"options":[],"no_results":"Gone."}"#
        guard case .flights(let booking)? = Card.parse(name: "search_flights", output: soldOut) else { return XCTFail("expected a booking") }
        XCTAssertTrue(booking.noResults)
        XCTAssertEqual(booking.currency, "USD", "a missing currency falls back")

        XCTAssertNil(Card.parse(name: "search_flights", output: #"{"kind":"flights","options":[]}"#), "nothing to show")
        XCTAssertNil(Card.parse(name: "search_flights", output: #"{"kind":"flights","options":[{"price":1,"segments":[]}]}"#))
        XCTAssertNil(Card.parse(name: "search_flights", output: #"{"kind":"hotels","options":[]}"#))
        XCTAssertNil(Card.parse(name: "search_flights", output: #"{"options":[{"segments":[{}]}]}"#), "a result needs its kind")
        XCTAssertNil(Card.parse(name: "search_flights", output: #"{"kind":"booking","flights":[],"options":[{"price":1}]}"#))
        XCTAssertNil(Card.parse(name: "search_flights", output: "Error: no SerpApi key"))

        let many = (0..<30).map { _ in #"{"segments":[{"from":{"airport":"SFO"},"to":{"airport":"MUC"}}]}"# }.joined(separator: ",")
        guard case .flights(let capped)? = Card.parse(name: "search_flights", output: #"{"kind":"flights","options":[\#(many)],"more":1e9}"#)
        else { return XCTFail("expected flights") }
        XCTAssertEqual(capped.options.count, FlightsCard.maxOptions)
        XCTAssertEqual(capped.more, 0, "an impossible count is dropped")
        XCTAssertEqual(capped.leg, .oneWay)
        XCTAssertNil(capped.cabin)
    }

    func testFlightsRoute() {
        XCTAssertEqual(FlightsCard.subject(.object(["from": .array([.string("SFO"), .string("oak")]), "to": .array([.string("MUC")])])),
                       "SFO/OAK → MUC")
        XCTAssertEqual(FlightsCard.subject(.object(["from": .string(" sfo, oak "), "to": .string("MUC,muc")])), "SFO/OAK → MUC")
        XCTAssertNil(FlightsCard.subject(.object(["from": .string("SFO")])))
        XCTAssertEqual(FlightsCard.airports(.array([.string("San Francisco"), .string("SFO"), .number(1)])), ["SFO"])
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

    func testChartFromItsInput() {
        let input: JSONValue = .object([
            "path": .string("charts/sales.png"), "kind": .string("bar"), "title": .string(" Sales "),
            "labels": .array([.string("Q1"), .string("Q2")]),
            "series": .array([
                .object(["name": .string("2025"), "values": .array([.number(1), .null])]),
                .object(["values": .array([.number(3), .number(4)])]),
            ]),
            "stacked": .bool(true), "log_scale": .bool(true), "bins": .number(1e20),
        ])
        guard case .chart(let chart)? = Card.parse(name: "generate_chart", output: "wrote charts/sales.png: 12 bytes", input: input)
        else { return XCTFail("expected a chart") }
        XCTAssertEqual(chart.kind, .bar)
        XCTAssertEqual(chart.title, "Sales")
        XCTAssertEqual(chart.labels, [.text("Q1"), .text("Q2")])
        XCTAssertEqual(chart.series.map(\.name), ["2025", "Series 2"])
        XCTAssertEqual(chart.series[0].values, [1, nil])
        XCTAssertTrue(chart.stacked)
        XCTAssertTrue(chart.log)
        XCTAssertNil(chart.bins, "an out-of-range bin count is ignored")

        func parse(_ fields: [String: JSONValue]) -> Card? {
            var base: [String: JSONValue] = ["kind": .string("line"), "labels": .array([.number(1), .number(2)]),
                                             "series": .array([.object(["values": .array([.number(1), .number(2)])])])]
            base.merge(fields) { $1 }
            return Card.parse(name: "generate_chart", output: "", input: .object(base))
        }
        XCTAssertNotNil(parse([:]))
        XCTAssertNil(parse(["kind": .string("donut")]))
        XCTAssertNil(parse(["labels": .array([.number(1)])]), "labels and values must match")
        XCTAssertNil(parse(["series": .array([.object(["values": .array([.string("1"), .number(2)])])])]))
        XCTAssertNil(parse(["kind": .string("scatter"), "labels": .array([.string("a"), .string("b")])]))
        XCTAssertNil(parse(["kind": .string("pie"), "series": .array([.object(["values": .array([.number(1), .number(-2)])])])]))
        guard case .chart(let hist)? = parse(["kind": .string("histogram"), "labels": .null, "bins": .number(2),
                                               "series": .array([.object(["values": .array([.number(0), .number(1), .number(1), .number(4)])])])])
        else { return XCTFail("expected a histogram") }
        XCTAssertEqual(hist.histogram?.edges, [0, 2, 4])
        XCTAssertEqual(hist.histogram?.counts, [[3, 1]])

        let long = (0..<5000).map { JSONValue.number(Double($0)) }
        guard case .chart(let thin)? = parse(["labels": .array(long), "series": .array([.object(["values": .array(long)])])])
        else { return XCTFail("expected a long line") }
        XCTAssertTrue(thin.thinned)
        XCTAssertLessThanOrEqual(thin.labels.count, 801)
        XCTAssertEqual(thin.labels.last, .number(4999), "the last point is kept")
        XCTAssertNil(Card.parse(name: "generate_chart", output: "", input: .null))
    }

    func testChartsAToolPrinted() {
        let output = """
        Report line
        [chart] {"kind":"line","title":"Glucose","labels":["10:00","10:15"],"series":[{"values":[120,null]}],"band":{"from":180,"to":70,"label":"Target"},"path":"x.png"}
        [chart] {"kind":"donut"}
        [chart] not json
        The end
        """
        let found = ChartCard.inOutput(output)
        XCTAssertEqual(found.charts.count, 1)
        XCTAssertEqual(found.charts[0].title, "Glucose")
        XCTAssertNil(found.charts[0].path, "a printed chart saves no file")
        XCTAssertEqual(found.charts[0].band, ChartCard.Band(from: 70, to: 180, label: "Target"))
        XCTAssertEqual(found.text, "Report line\n[chart] {\"kind\":\"donut\"}\n[chart] not json\nThe end",
                       "lines that aren't drawable charts stay in the text")
        XCTAssertTrue(ChartCard.inOutput("no charts here").charts.isEmpty)
    }
}

