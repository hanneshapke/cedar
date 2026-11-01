import XCTest
@testable import Cedar

final class WeatherCardTests: XCTestCase {
    /// As src/tools/weather.rs writes it (its `forecast_lists_place_current_conditions_and_each_day` test).
    static let report = """
    Berlin, State of Berlin, Germany (52.52, 13.41), timezone Europe/Berlin

    Now (2026-10-12 21:00): clear sky, 10.2°C (feels 8.3°C), humidity 70%, wind 4.6 km/h from NW, gusts 9.4 km/h, precipitation 0 mm

    Mon 2026-10-12: overcast, low 6.8°C, high 13.7°C, precipitation 0 mm (32% chance), wind up to 14.1 km/h (gusts 31 km/h), UV 2.9, sunrise 07:26, sunset 18:18
    Tue 2026-10-13: slight rain, low 5.1°C, high 15.1°C, precipitation 0.2 mm (19% chance), wind up to 13.4 km/h (gusts 29.5 km/h), UV 2.8, sunrise 07:28, sunset 18:16

    Weather data by Open-Meteo.com (CC BY 4.0)
    """

    func testReadsTheReportBack() throws {
        guard case .weather(let w)? = Card.parse(name: "weather_forecast", output: Self.report) else { return XCTFail("expected a forecast") }
        XCTAssertEqual(w.label, "Berlin, State of Berlin, Germany")
        XCTAssertEqual(w.placeName, "Berlin")
        XCTAssertEqual(w.region, "State of Berlin, Germany")
        XCTAssertEqual(w.latitude, 52.52)
        XCTAssertEqual(w.longitude, 13.41)
        XCTAssertEqual(w.timezone, "Europe/Berlin")

        let now = try XCTUnwrap(w.now)
        XCTAssertEqual(now.date, "2026-10-12")
        XCTAssertEqual(now.time, "21:00")
        XCTAssertEqual(now.code, 0)
        XCTAssertEqual(now.temp, .init(value: 10.2, unit: "°C"))
        XCTAssertEqual(now.feels, .init(value: 8.3, unit: "°C"))
        XCTAssertEqual(now.humidity, .init(value: 70, unit: "%"))
        XCTAssertEqual(now.wind, .init(value: 4.6, unit: "km/h"))
        XCTAssertEqual(now.from, "NW")
        XCTAssertEqual(now.gusts, .init(value: 9.4, unit: "km/h"))
        XCTAssertEqual(now.precip, .init(value: 0, unit: "mm"))

        XCTAssertEqual(w.days.map(\.date), ["2026-10-12", "2026-10-13"])
        let monday = w.days[0]
        XCTAssertEqual(monday.code, 3)
        XCTAssertEqual(monday.low, .init(value: 6.8, unit: "°C"))
        XCTAssertEqual(monday.high, .init(value: 13.7, unit: "°C"))
        XCTAssertEqual(monday.precip, .init(value: 0, unit: "mm"))
        XCTAssertEqual(monday.chance, .init(value: 32, unit: "%"))
        XCTAssertEqual(monday.wind, .init(value: 14.1, unit: "km/h"))
        XCTAssertEqual(monday.gusts, .init(value: 31, unit: "km/h"))
        XCTAssertEqual(monday.uv, .init(value: 2.9, unit: ""))
        XCTAssertEqual(monday.sunrise, "07:26")
        XCTAssertEqual(monday.sunset, "18:18")
        XCTAssertEqual(w.days[1].code, 61)

        // 21:00 is after sunset, so the card leads with a moon.
        XCTAssertTrue(w.isNight)
        XCTAssertEqual(w.today, "2026-10-12")
        XCTAssertEqual(w.todayDay?.date, "2026-10-12")
        XCTAssertEqual(w.leadCode, 0)
        XCTAssertEqual(w.forecastDays.count, 2)
    }

    func testCoordinatesOnlyAndMissingParts() throws {
        let output = """
        52.52, 13.41

        Now: weather code 4, 10°F, wind 3 mph

        Wed 2026-10-14: no data
        Thu 2026-10-15: fog, low 40°F, high 55°F

        Weather data by Open-Meteo.com (CC BY 4.0)
        """
        guard case .weather(let w)? = Card.parse(name: "weather_forecast", output: output) else { return XCTFail("expected a forecast") }
        XCTAssertEqual(w.label, "")
        XCTAssertEqual(w.placeName, "52.52, 13.41")
        XCTAssertNil(w.region)
        XCTAssertNil(w.timezone)
        XCTAssertEqual(w.now?.code, 4)
        XCTAssertEqual(w.now?.temp, .init(value: 10, unit: "°F"))
        XCTAssertNil(w.now?.feels)
        XCTAssertEqual(w.now?.wind, .init(value: 3, unit: "mph"))
        XCTAssertNil(w.now?.from)
        XCTAssertNil(w.now?.time)
        XCTAssertFalse(w.isNight)
        XCTAssertEqual(w.days.count, 2)
        XCTAssertNil(w.days[0].code)
        XCTAssertNil(w.days[0].low)
        XCTAssertEqual(w.days[1].code, 45)
        XCTAssertEqual(w.forecastDays.map(\.date), ["2026-10-15"])
        XCTAssertEqual(w.today, "2026-10-14", "without a local time, the first day is today")
        XCTAssertEqual(w.now?.temp?.celsius ?? 0, -12.2, accuracy: 0.05)
    }

    func testWhatIsNotAForecastIsNoCard() {
        XCTAssertNil(Card.parse(name: "weather_forecast", output: "Could not find a place named Atlantis."))
        XCTAssertNil(Card.parse(name: "weather_forecast", output: "Berlin (52.52, 13.41)\n\nWeather data by Open-Meteo.com (CC BY 4.0)"),
                     "no temperatures, nothing to draw")
        XCTAssertNil(Card.parse(name: "weather_forecast", output: "Nowhere (120, 13.41)\n\nNow: 10°C"))
        XCTAssertNil(Card.parse(name: "bash", output: Self.report))
    }

    func testTheToolCallIsTitledWithThePlace() {
        var call = ToolCall(id: "1", name: "weather_forecast", input: .object(["location": .string("Berlin")]))
        XCTAssertTrue(call.isCard)
        XCTAssertEqual(call.summary, "Berlin")
        call.output = Self.report
        XCTAssertEqual(call.summary, "Berlin, State of Berlin, Germany")
    }

    func testSkiesAndColours() {
        XCTAssertEqual(WeatherCard.Sky(code: 1), .clear)
        XCTAssertEqual(WeatherCard.Sky(code: 2), .partly)
        XCTAssertEqual(WeatherCard.Sky(code: 48), .fog)
        XCTAssertEqual(WeatherCard.Sky(code: 55), .drizzle)
        XCTAssertEqual(WeatherCard.Sky(code: 66), .sleet)
        XCTAssertEqual(WeatherCard.Sky(code: 81), .rain)
        XCTAssertEqual(WeatherCard.Sky(code: 86), .snow)
        XCTAssertEqual(WeatherCard.Sky(code: 99), .storm)
        XCTAssertEqual(WeatherCard.Sky(code: 4), .cloud)
        XCTAssertEqual(WeatherCard.Sky(code: nil), .cloud)
        XCTAssertEqual(WeatherCard.conditionKey(2), "Partly cloudy")
        XCTAssertEqual(WeatherCard.conditionKey(4), "")
        // Freezing is blue, mild is green, hot is red.
        XCTAssertEqual(WeatherCard.hue(celsius: -20), 230)
        XCTAssertEqual(WeatherCard.hue(celsius: 5), 180)
        XCTAssertEqual(WeatherCard.hue(celsius: 40), 5)
    }

    func testTemperatureScaleSpansTheForecastAndNow() {
        let days = [WeatherCard.Day(date: "2026-10-12", low: .init(value: 6, unit: "°C"), high: .init(value: 14, unit: "°C")),
                    WeatherCard.Day(date: "2026-10-13", low: .init(value: 4, unit: "°C"), high: .init(value: 12, unit: "°C"))]
        let scale = TemperatureScale(days: days, now: .init(value: 16, unit: "°C"))
        XCTAssertEqual(scale.min, 4)
        XCTAssertEqual(scale.span, 12)
        XCTAssertEqual(scale.fraction(4), 0)
        XCTAssertEqual(scale.fraction(16), 1)
        XCTAssertEqual(scale.fraction(10), 0.5)
        XCTAssertEqual(scale.fraction(100), 1, "clamped")
    }
}
