package dev.cedar.core

import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFalse
import kotlin.test.assertIs
import kotlin.test.assertNotNull
import kotlin.test.assertNull
import kotlin.test.assertTrue

// Ported from iOS/CedarTests/CardsTests.swift.
class CardsTest {
    @Test
    fun mapKeepsValidPinsAndDefaultsTheLabel() {
        val output = """
        {"title":"Coffee in Berlin","markers":[
          {"label":" The Barn ","latitude":52.5291,"longitude":13.4013},
          {"latitude":52.52,"longitude":13.405},
          {"label":"Nowhere","latitude":120,"longitude":0},
          {"label":"Text","latitude":"52","longitude":13}
        ],"not_found":["Bonanza",""," "]}
        """
        val map = assertIs<Card.Map>(Card.parse("show_map", output)).card
        assertEquals("Coffee in Berlin", map.title)
        assertEquals(listOf("The Barn", "52.5200, 13.4050"), map.markers.map { it.label })
        assertEquals(listOf("Bonanza"), map.notFound)
    }

    @Test
    fun mapWithoutAValidPinIsNoCard() {
        assertNull(Card.parse("show_map", """{"markers":[{"latitude":100,"longitude":0}]}"""))
        assertNull(Card.parse("show_map", "not json"))
        assertNull(Card.parse("bash", """{"markers":[{"latitude":1,"longitude":1}]}"""))
    }

    @Test
    fun recipe() {
        val output = """
        {"title":"Pancakes","servings":"4","prep_time":"10 min","cook_time":"","ingredients":[
          {"quantity":"200 g","item":"flour"},{"item":"2 eggs"},{"quantity":"1 cup"}
        ],"steps":[{"title":"Mix","text":"Whisk everything.","duration":"2 min"},{"text":"  "},{"text":"Fry."}],
        "tips":["Rest the batter."],
        "image":{"url":"http://example.com/p.jpg","page_url":"https://example.com","title":"P","source":"Wikimedia"}}
        """
        val recipe = assertIs<Card.Recipe>(Card.parse("show_recipe", output)).card
        assertEquals(listOf(RecipeCard.Fact("Prep", "10 min"), RecipeCard.Fact("Serves", "4")), recipe.facts)
        assertEquals(listOf(RecipeCard.Ingredient("200 g", "flour"), RecipeCard.Ingredient(null, "2 eggs")), recipe.ingredients)
        assertEquals(listOf("Whisk everything.", "Fry."), recipe.steps.map { it.text })
        assertEquals(listOf("Rest the batter."), recipe.tips)
        assertNull(recipe.photo, "only https photos are used")
        assertNull(Card.parse("show_recipe", """{"title":"Toast","ingredients":[],"steps":[{"text":"Toast."}]}"""))
    }

    @Test
    fun polyline() {
        // Google's documented example.
        val points = Polyline.decode("_p~iF~ps|U_ulLnnqC_mqNvxq`@")
        assertEquals(3, points.size)
        assertEquals(38.5, points[0].latitude, 1e-9)
        assertEquals(-120.2, points[0].longitude, 1e-9)
        assertEquals(43.252, points[2].latitude, 1e-9)
        assertEquals(-126.453, points[2].longitude, 1e-9)
        assertEquals(emptyList(), Polyline.decode("_p~iF~ps|U_ulLnnqC_mqNvxq"), "a truncated line is dropped")
        assertEquals(emptyList(), Polyline.decode("  "))
        assertEquals(emptyList(), Polyline.decode(null))
    }

    @Test
    fun trip() {
        val output = """
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
        """
        val trip = assertIs<Card.Trip>(Card.parse("plan_trip", output)).card
        assertEquals(listOf(TripCard.Mode.DRIVE, TripCard.Mode.TRANSIT, TripCard.Mode.RIDESHARE), trip.options.map { it.mode })
        assertEquals(listOf(0, 1, 3), trip.options.map { it.id }, "ids are the result's indices")
        assertEquals(TripCard.Mode.TRANSIT, trip.recommendedOption?.mode)
        assertEquals("Fast and direct.", trip.reason)
        assertEquals(TripCard.Mode.DRIVE, trip.fastest?.mode)
        assertEquals("Europe/Berlin", trip.timeZone?.id)
        assertEquals(listOf(TripCard.Unavailable(TripCard.Mode.WALK, "Too far.")), trip.unavailable, "modes with an option aren't unavailable")
        val transit = trip.options[1]
        assertEquals(listOf(TripCard.LegKind.WALK, TripCard.LegKind.TRAIN, TripCard.LegKind.WALK), transit.legs.map { it.kind })
        assertEquals("#ff7700", transit.legs[1].color)
        assertEquals("S5", transit.legs[1].route)
        assertNotNull(transit.depart)
        assertEquals(2, trip.options[0].path.size)
        assertEquals(TripCard.Cost(15.0, 22.0, "USD"), trip.options[2].cost)
        assertEquals("1 h 15 min", TripStyle.duration(75))
        assertEquals("~$15–22", TripStyle.money(trip.options[2].cost!!))
    }

    @Test
    fun tripNeedsBothEndsAndAnOption() {
        assertNull(Card.parse("plan_trip", """{"from":{"latitude":1,"longitude":1},"to":{"latitude":2,"longitude":2},"options":[]}"""))
        assertNull(Card.parse("plan_trip", """{"from":{"latitude":1,"longitude":1},"options":[{"mode":"walk","duration_min":3}]}"""))
    }

    @Test
    fun stocks() {
        val output = """
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
        """
        val stocks = assertIs<Card.Stocks>(Card.parse("compare_stocks", output)).card
        assertEquals("6M", stocks.range)
        assertEquals(listOf("AAPL"), stocks.stocks.map { it.symbol }, "a symbol with under two chart points is dropped")
        val apple = stocks.stocks[0]
        assertEquals(listOf(220.0, 225.0, 230.5), apple.series.closes, "bad and repeated days are skipped")
        assertEquals(StocksCard.RETURN_PERIODS.size, apple.returns.size)
        assertEquals(1.5, apple.returns.first())
        assertNull(apple.returns[1])
        assertEquals(StocksCard.Call.BUY, apple.rating?.call)
        assertEquals(listOf(2), apple.rating?.factors?.map { it.score }, "scores are clamped and factors need a detail")
        assertEquals(3, stocks.benchmark?.series?.closes?.size)
        assertEquals(listOf("Apple up"), stocks.news.map { it.title })
        assertEquals(listOf("AAPL"), stocks.news[0].symbols)
        assertEquals(listOf(StocksCard.Unavailable("MSFT", "No data.")), stocks.unavailable)
        assertEquals("−1.2%", StockFormat.signed(-1.234))
        assertEquals("$" + number(230.5, 2, 2), StockFormat.money(230.5, "USD"))
    }

    @Test
    fun products() {
        val output = """
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
        """
        val card = assertIs<Card.Products>(Card.parse("search_products", output)).card
        assertEquals("USD", card.currency)
        assertEquals(3, card.total, "at least the products the card holds")
        assertEquals(3, card.pages)
        assertEquals(5, card.productsFound)
        assertEquals("Get the Trail Runner 5.", card.summary)
        assertNull(card.note)
        assertEquals(listOf("P1", "P2", "P3"), card.options.map { it.id }, "an option needs a sku and a name, not a price")

        val first = card.options[0]
        assertEquals("Trail Runner 5", first.name)
        assertEquals(listOf("https://img.example/tr5.jpg"), first.images, "https only, without repeats")
        assertEquals(129.95, first.price)
        assertTrue(first.priceChecked)
        assertEquals(159.95, first.regularPrice)
        assertEquals("shop.example", first.store)
        assertEquals(true, first.online)
        assertNull(first.inStore, "a store that doesn't say isn't taken to say no")
        assertEquals(listOf("Vibram sole"), first.features)
        assertEquals(listOf(ProductsCard.Spec("Drop", "5 mm")), first.specs)
        assertEquals("Grippy and roomy.", first.why)
        assertEquals(listOf("grip", "comfort"), first.pros)
        assertEquals("muddy trails", first.bestFor)
        assertEquals(listOf(2, 1, 99), first.sources)
        assertEquals(listOf("P2"), first.comparable, "itself and unknown skus are dropped")
        assertEquals(listOf("Top pick", "Best rated"), card.badges["P1"], "an option's own badges win over the picks")

        val cloud = card.options[1]
        assertTrue(!cloud.priceChecked)
        assertEquals("EUR", cloud.currency)
        assertEquals("EUR", card.currencyOf(cloud))
        assertEquals(ProductsCard.Verdict.UNREVIEWED, cloud.reviews.verdict)
        assertEquals(emptyList(), card.badges["P2"], "an empty list of badges is kept")

        val peregrine = card.options[2]
        assertNull(peregrine.price)
        assertNull(peregrine.regularPrice)
        assertNull(peregrine.online)
        assertEquals("USD", card.currencyOf(peregrine))
        assertEquals(ProductsCard.Verdict.UNREVIEWED, peregrine.reviews.verdict, "an unknown verdict falls back on the count")
        assertEquals(listOf("Best value"), card.badges["P3"], "without badges of its own, the picks give them")

        assertEquals("https://reviews.example/a", card.sources[1]?.url)
        assertNull(card.sources[2]?.url, "only https links")
        assertEquals("Trail Runner 5", card.sources[2]?.title)
        assertNull(card.sources[3], "a source needs a title")
        assertNull(Card.parse("search_products", """{"options":[{"name":"x"}]}"""))
        assertNull(Card.parse("search_products", """{"options":[]}"""))
    }

    @Test
    fun cardIsParsedWhenTheResultArrives() {
        var t = TurnTranscript(turn(status = TurnStatus.RUNNING))
        t = t.apply(event(1, "tool.call", """{"id":"m","name":"show_map","input":{}}"""))
        t = t.apply(event(2, "tool.result", result("m", """{"markers":[{"label":"A","latitude":1,"longitude":2}]}""")))
        val call = assertIs<TranscriptItem.Tool>(t.items.last()).call
        assertEquals("A", assertIs<Card.Map>(call.card).card.markers.first().label)
    }

    @Test
    fun chartFromItsInput() {
        val input = j("""
        {"path":"charts/sales.png","kind":"bar","title":" Sales ","labels":["Q1","Q2"],
         "series":[{"name":"2025","values":[1,null]},{"values":[3,4]}],
         "stacked":true,"log_scale":true,"bins":1e20}
        """)
        val chart = assertIs<Card.Chart>(Card.parse("generate_chart", "wrote charts/sales.png: 12 bytes", input)).card
        assertEquals(ChartCard.Kind.BAR, chart.kind)
        assertEquals("Sales", chart.title)
        assertEquals(listOf<ChartCard.Label>(ChartCard.Label.Text("Q1"), ChartCard.Label.Text("Q2")), chart.labels)
        assertEquals(listOf("2025", "Series 2"), chart.series.map { it.name })
        assertEquals(listOf(1.0, null), chart.series[0].values)
        assertTrue(chart.stacked)
        assertTrue(chart.log)
        assertNull(chart.bins, "an out-of-range bin count is ignored")

        fun parse(fields: Map<String, kotlinx.serialization.json.JsonElement>): Card? {
            val base = j("""{"kind":"line","labels":[1,2],"series":[{"values":[1,2]}]}""") as JsonObject
            return Card.parse("generate_chart", "", JsonObject(base + fields))
        }
        assertNotNull(parse(emptyMap()))
        assertNull(parse(mapOf("kind" to JsonPrimitive("donut"))))
        assertNull(parse(mapOf("labels" to j("[1]"))), "labels and values must match")
        assertNull(parse(mapOf("series" to j("""[{"values":["1",2]}]"""))))
        assertNull(parse(mapOf("kind" to JsonPrimitive("scatter"), "labels" to j("""["a","b"]"""))))
        assertNull(parse(mapOf("kind" to JsonPrimitive("pie"), "series" to j("""[{"values":[1,-2]}]"""))))
        val hist = assertIs<Card.Chart>(parse(mapOf(
            "kind" to JsonPrimitive("histogram"), "labels" to j("null"), "bins" to JsonPrimitive(2),
            "series" to j("""[{"values":[0,1,1,4]}]"""),
        ))).card
        assertEquals(listOf(0.0, 2.0, 4.0), hist.histogram?.first)
        assertEquals(listOf(listOf(3, 1)), hist.histogram?.second)

        val long = JsonArray((0 until 5000).map { JsonPrimitive(it) })
        val thin = assertIs<Card.Chart>(parse(mapOf("labels" to long, "series" to JsonArray(listOf(JsonObject(mapOf("values" to long))))))).card
        assertTrue(thin.thinned)
        assertTrue(thin.labels.size <= 801)
        assertEquals(ChartCard.Label.Number(4999.0), thin.labels.last(), "the last point is kept")
        assertNull(Card.parse("generate_chart", ""))
    }

    @Test
    fun chartsAToolPrinted() {
        val output = """
            Report line
            [chart] {"kind":"line","title":"Glucose","labels":["10:00","10:15"],"series":[{"values":[120,null]}],"band":{"from":180,"to":70,"label":"Target"},"path":"x.png"}
            [chart] {"kind":"donut"}
            [chart] not json
            The end
        """.trimIndent()
        val found = ChartCard.inOutput(output)
        assertEquals(1, found.charts.size)
        assertEquals("Glucose", found.charts[0].title)
        assertNull(found.charts[0].path, "a printed chart saves no file")
        assertEquals(ChartCard.Band(70.0, 180.0, "Target"), found.charts[0].band)
        assertEquals("Report line\n[chart] {\"kind\":\"donut\"}\n[chart] not json\nThe end", found.text, "lines that aren't drawable charts stay in the text")
        assertTrue(ChartCard.inOutput("no charts here").charts.isEmpty())
    }

    @Test
    fun flightsSearch() {
        val output = """
        {"kind":"flights","leg":"outbound","from":["SFO","oak"],"to":["MUC"],"date":"2026-12-01","return_date":"2026-12-10",
         "passengers":{"adults":2,"children":1,"infants_in_seat":0,"infants_on_lap":1},"cabin":"business","currency":"chf","sort":"best",
         "options":[
           {"id":1,"top_pick":true,"price":1234.5,"airlines":["Lufthansa","Lufthansa",""],
            "logo":"https://www.gstatic.com/flights/airline_logos/70px/LH.png",
            "depart":{"airport":"SFO","name":"San Francisco International Airport","time":"2026-12-01T15:40"},
            "arrive":{"airport":"MUC","name":"Munich International Airport","time":"2026-12-02T11:30"},
            "duration_min":650,"stops":1,
            "segments":[
              {"flight":"LH 455","airline":"Lufthansa","logo":"http://insecure.example/LH.png",
               "from":{"airport":"SFO","time":"2026-12-01T15:40"},"to":{"airport":"EWR","time":"2026-12-01T23:59"},
               "duration_min":320,"aircraft":"Airbus A350","cabin":"Business","legroom":"31 in","overnight":false,"often_delayed":true},
              {"flight":"LH 401","from":{"airport":"EWR","time":"2026-12-02T02:00"},"to":{"airport":"MUC","time":"bad"},
               "duration_min":-5,"overnight":true}
            ],
            "layovers":[{"airport":"EWR","name":"Newark Liberty International Airport","duration_min":120,"overnight":false},{"name":"no code"}],
            "co2_kg":512,"co2_vs_typical_pct":-7,"departure_token":"secret","booking_token":"secret"},
           {"id":2,"price":null,"airlines":["United"],"logo":"javascript:alert(1)",
            "depart":{"airport":"OAK","time":"2026-12-01T06:00"},"arrive":{"airport":"MUC","time":"2026-12-01T22:00"},
            "stops":0,"segments":[],"layovers":[],"co2_vs_typical_pct":"12"},
           {"id":3,"price":900,"airlines":["Nobody"],"depart":{"name":"no code"},"arrive":{"airport":"MUC"}},
           "nonsense"
         ],
         "more":12,
         "insights":{"lowest":999.0,"level":"Low","typical":[1600.0,1100.0],
           "history":[{"date":"2026-10-01","price":1300},{"date":"2026-10-02","price":-1},{"date":"2026-09-30","price":1250},{"date":"2026-10-03","price":1200}]},
         "google_flights_url":"https://www.google.com/travel/flights?q=x",
         "as_of":"2026-10-29T02:43:00Z","note":"For the model."}
        """
        val card = assertIs<Card.Flights>(Card.parse("search_flights", output)).card
        assertEquals(FlightsCard.Kind.FLIGHTS, card.kind)
        assertEquals(FlightsCard.Leg.OUTBOUND, card.leg)
        assertEquals(listOf("SFO", "OAK"), card.from)
        assertEquals("SFO/OAK → MUC", card.route)
        assertEquals(java.time.LocalDate.of(2026, 12, 1), card.date)
        assertEquals(java.time.LocalDate.of(2026, 12, 10), card.returnDate)
        assertEquals(FlightsCard.Passengers(2, 1, 0, 1), card.passengers)
        assertEquals(4, card.passengers.total)
        assertEquals(FlightsCard.Cabin.BUSINESS, card.cabin)
        assertEquals("CHF", card.currency)
        assertEquals(12, card.more)
        assertEquals("https://www.google.com/travel/flights?q=x", card.googleFlightsUrl)
        assertEquals(java.time.Instant.parse("2026-10-29T02:43:00Z"), card.asOf)
        assertEquals(listOf(1, 2), card.options.map { it.id }, "an option needs where it leaves from and lands")

        val lh = card.options[0]
        assertTrue(lh.topPick)
        assertEquals(1234.5, lh.price)
        assertEquals(listOf("Lufthansa"), lh.airlines)
        assertEquals("https://www.gstatic.com/flights/airline_logos/70px/LH.png", lh.logo)
        assertEquals(java.time.LocalDateTime.of(2026, 12, 1, 15, 40), lh.depart.time)
        assertEquals(1, lh.daysLater, "it lands the next day, local time")
        assertEquals(650, lh.minutes)
        assertEquals(1, lh.stops)
        assertEquals(2, lh.segments.size)
        assertNull(lh.segments[0].logo, "https only")
        assertEquals("Airbus A350", lh.segments[0].aircraft)
        assertNull(lh.segments[1].minutes, "a negative duration is dropped")
        assertNull(lh.segments[1].to.time, "a time that doesn't parse is dropped")
        assertTrue(lh.oftenDelayed)
        assertTrue(lh.overnight)
        assertEquals(listOf("EWR"), lh.layovers.map { it.airport })
        assertEquals(120, lh.layovers[0].minutes)
        assertEquals(512, lh.co2Kg)
        assertEquals(-7, lh.co2VsTypicalPct)

        val ua = card.options[1]
        assertNull(ua.price)
        assertNull(ua.logo)
        assertFalse(ua.topPick)
        assertEquals(0, ua.daysLater)
        assertNull(ua.co2VsTypicalPct, "a number given as text isn't one")
        assertFalse(ua.oftenDelayed)

        val insights = assertNotNull(card.insights)
        assertEquals(999.0, insights.lowest)
        assertEquals("low", insights.level)
        assertEquals(1100.0, insights.typicalLow, "the range is put in order")
        assertEquals(1600.0, insights.typicalHigh)
        assertEquals(listOf(1300.0, 1200.0), insights.history.map { it.price }, "bad prices and dates out of order are dropped")
    }

    @Test
    fun flightsWithoutResultsAndRejects() {
        val empty = """{"kind":"flights","leg":"one_way","from":["SFO"],"to":["MUC"],"date":"2026-12-01",
            "passengers":{"adults":1},"cabin":"economy","currency":"EUR","options":[],"more":0,
            "no_results":"Google Flights found no flights for this search."}"""
        val card = assertIs<Card.Flights>(Card.parse("search_flights", empty)).card
        assertEquals(FlightsCard.Leg.ONE_WAY, card.leg)
        assertTrue(card.options.isEmpty())
        assertEquals("Google Flights found no flights for this search.", card.noResults)
        assertNull(card.insights)
        assertNull(card.googleFlightsUrl)

        val option = """{"depart":{"airport":"SFO"},"arrive":{"airport":"MUC"},"price":100}"""
        fun parse(body: String) = Card.parse("search_flights", body)
        assertNotNull(parse("""{"kind":"flights","from":["SFO"],"to":["MUC"],"options":[$option]}"""))
        assertNull(parse("""{"kind":"flights","from":["SFO"],"to":["MUC"],"options":[]}"""), "nothing to show")
        assertNull(parse("""{"kind":"hotels","from":["SFO"],"to":["MUC"],"options":[$option]}"""))
        assertNull(parse("""{"kind":"flights","from":["San Francisco"],"to":["MUC"],"options":[$option]}"""))
        assertNull(parse("""{"kind":"flights","from":["SFO"],"options":[$option]}"""))
        assertNull(parse("""{"kind":"flights","from":["SFO"],"to":["MUC"]}"""))
        assertNull(parse("not json"))
        val many = (1..30).joinToString(",") { option }
        assertEquals(FlightsCard.MAX_OPTIONS, assertIs<Card.Flights>(parse("""{"kind":"flights","from":"sfo, oak","to":"MUC","options":[$many]}""")).card.options.size)
        val odd = assertIs<Card.Flights>(parse(
            """{"kind":"flights","from":["SFO"],"to":["MUC"],"currency":"€€","passengers":{"adults":-3,"children":1e9},"cabin":"steerage",
               "insights":{"lowest":"cheap"},"google_flights_url":"http://www.google.com/travel/flights","options":[$option]}""",
        )).card
        assertEquals("USD", odd.currency)
        assertEquals(FlightsCard.Passengers(1, 0, 0, 0), odd.passengers)
        assertNull(odd.cabin)
        assertNull(odd.insights)
        assertNull(odd.googleFlightsUrl, "https only")
    }

    @Test
    fun flightsBooking() {
        val output = """
        {"kind":"booking","leg":"outbound","from":["SFO"],"to":["MUC"],"date":"2026-12-01","return_date":"2026-12-10",
         "passengers":{"adults":1},"cabin":"economy","currency":"USD",
         "flights":[{"id":1,"price":1234,"airlines":["Lufthansa"],"depart":{"airport":"SFO","time":"2026-12-01T15:40"},
                     "arrive":{"airport":"MUC","time":"2026-12-02T11:30"},"stops":0,"segments":[],"layovers":[]}],
         "options":[
           {"seller":"Lufthansa","part":"together","price":1234,"fare":"Economy Light","bags":["1 free carry-on","1st checked bag: 60 USD"],"marketed_as":["LH 455"]},
           {"seller":"Expedia","part":"departing","price":null,"bags":[],"marketed_as":[]},
           {"part":"returning","price":10},
           {"seller":"Odd","part":"sideways","price":5}
         ],
         "google_flights_url":"https://www.google.com/travel/flights/booking?x","note":"..."}
        """
        val card = assertIs<Card.Flights>(Card.parse("search_flights", output)).card
        assertEquals(FlightsCard.Kind.BOOKING, card.kind)
        assertTrue(card.options.isEmpty())
        assertEquals(1, card.flights.size)
        assertEquals(1, card.flights[0].daysLater)
        assertEquals(listOf("Lufthansa", "Expedia", "Odd"), card.sellers.map { it.seller }, "a seller needs a name")
        val lh = card.sellers[0]
        assertEquals(FlightsCard.Part.TOGETHER, lh.part)
        assertEquals(1234.0, lh.price)
        assertEquals("Economy Light", lh.fare)
        assertEquals(listOf("1 free carry-on", "1st checked bag: 60 USD"), lh.bags)
        assertEquals(listOf("LH 455"), lh.marketedAs)
        assertEquals(FlightsCard.Part.DEPARTING, card.sellers[1].part)
        assertNull(card.sellers[1].price)
        assertEquals(FlightsCard.Part.TOGETHER, card.sellers[2].part, "an unknown part is read as the whole trip")
        assertNull(card.asOf)
        assertEquals("https://www.google.com/travel/flights/booking?x", card.googleFlightsUrl)

        val gone = """{"kind":"booking","leg":"one_way","from":["SFO"],"to":["MUC"],"flights":[],"options":[],"no_results":"Gone."}"""
        assertEquals("Gone.", assertIs<Card.Flights>(Card.parse("search_flights", gone)).card.noResults)
        assertNull(Card.parse("search_flights", """{"kind":"booking","from":["SFO"],"to":["MUC"],"flights":[],"options":[]}"""))
    }

    @Test
    fun flightsRouteFromInput() {
        assertEquals(listOf("SFO", "OAK"), FlightsCard.airports(j("\" sfo, oak ,, SFO\"")))
        assertEquals(listOf("MUC"), FlightsCard.airports(j("""["muc", "Munich", 3]""")))
        assertEquals(emptyList(), FlightsCard.airports(j("{}")))
    }
}
