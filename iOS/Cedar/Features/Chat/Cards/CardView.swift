import MapKit
import SwiftUI

/// The card a tool's result draws: a map, a recipe, a trip, a stock comparison, or products.
struct CardView: View {
    let card: Card

    var body: some View {
        switch card {
        case .map(let map): MapCardView(card: map)
        case .recipe(let recipe): RecipeCardView(card: recipe)
        case .trip(let trip): TripCardView(card: trip)
        case .stocks(let stocks): StocksCardView(card: stocks)
        case .products(let products): ProductsCardView(card: products)
        }
    }
}

extension View {
    /// The frame every card sits in, the same as a tool call's.
    func cardStyle() -> some View {
        padding(14)
            .frame(maxWidth: .infinity, alignment: .leading)
            .background(Color(.secondarySystemBackground).opacity(0.6), in: RoundedRectangle(cornerRadius: 14))
            .overlay(RoundedRectangle(cornerRadius: 14).strokeBorder(Color(.separator).opacity(0.5)))
    }
}

/// A small heading over a part of a card.
struct CardLabel: View {
    let text: String

    init(_ text: String) { self.text = text }

    var body: some View {
        Text(text.uppercased())
            .font(.caption2.weight(.semibold))
            .tracking(0.6)
            .foregroundStyle(.secondary)
    }
}

extension Coordinate {
    var location: CLLocationCoordinate2D { CLLocationCoordinate2D(latitude: latitude, longitude: longitude) }
}

extension Place {
    var location: CLLocationCoordinate2D { coordinate.location }

    /// The place in Apple Maps.
    var mapsURL: URL? {
        var components = URLComponents(string: "https://maps.apple.com/")
        components?.queryItems = [
            URLQueryItem(name: "ll", value: "\(latitude),\(longitude)"),
            URLQueryItem(name: "q", value: label),
        ]
        return components?.url
    }
}

extension Color {
    /// A `#rrggbb` color, as the parsers check it.
    init?(hex: String) {
        guard hex.count == 7, let value = UInt32(hex.dropFirst(), radix: 16) else { return nil }
        self.init(red: Double((value >> 16) & 0xff) / 255, green: Double((value >> 8) & 0xff) / 255,
                  blue: Double(value & 0xff) / 255)
    }

    /// Black or white, whichever reads better on the `#rrggbb` color `hex`.
    static func ink(on hex: String) -> Color {
        guard let value = UInt32(hex.dropFirst(), radix: 16) else { return .primary }
        let r = Double((value >> 16) & 0xff) / 255, g = Double((value >> 8) & 0xff) / 255, b = Double(value & 0xff) / 255
        return 0.2126 * r + 0.7152 * g + 0.0722 * b > 0.55 ? Color(white: 0.07) : .white
    }
}

/// A map in a card: still in the conversation, so it doesn't catch the scrolling, and full screen
/// and movable when tapped.
struct CardMap<Content: MapContent>: View {
    let title: String
    var position: MapCameraPosition = .automatic
    @MapContentBuilder let content: () -> Content
    @State private var expanded = false

    var body: some View {
        Map(initialPosition: position, interactionModes: []) { content() }
            .frame(height: 220)
            .clipShape(RoundedRectangle(cornerRadius: 10))
            .overlay {
                Color.clear
                    .contentShape(Rectangle())
                    .onTapGesture { expanded = true }
            }
            .overlay(alignment: .topTrailing) {
                Image(systemName: "arrow.up.left.and.arrow.down.right")
                    .font(.caption.weight(.semibold))
                    .padding(7)
                    .background(.regularMaterial, in: Circle())
                    .padding(8)
                    .allowsHitTesting(false)
            }
            .accessibilityElement()
            .accessibilityLabel(title)
            .accessibilityHint("Opens the map full screen")
            .accessibilityAddTraits(.isButton)
            .accessibilityAction { expanded = true }
            .fullScreenCover(isPresented: $expanded) {
                NavigationStack {
                    Map(initialPosition: position) { content() }
                        .mapControls {
                            MapCompass()
                            MapScaleView()
                        }
                        .ignoresSafeArea(edges: .bottom)
                        .navigationTitle(title)
                        .navigationBarTitleDisplayMode(.inline)
                        .toolbar {
                            ToolbarItem(placement: .confirmationAction) {
                                Button("Done") { expanded = false }
                            }
                        }
                }
            }
    }
}

// MARK: - show_map

struct MapCardView: View {
    let card: MapCard

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            if let title = card.title {
                Text(title).font(.headline)
            }
            CardMap(title: card.title ?? "Map", position: position) {
                ForEach(Array(card.markers.enumerated()), id: \.offset) { index, place in
                    Marker(place.label, monogram: Text("\(index + 1)"), coordinate: place.location)
                        .tint(Color.accentColor)
                }
            }
            VStack(alignment: .leading, spacing: 8) {
                ForEach(Array(card.markers.enumerated()), id: \.offset) { index, place in
                    row(index: index, place: place)
                }
            }
            if !card.notFound.isEmpty {
                Text("Not found: \(card.notFound.joined(separator: ", "))")
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }
        }
        .cardStyle()
    }

    /// A single pin is shown at city scale; several are fitted to the map.
    private var position: MapCameraPosition {
        guard card.markers.count == 1, let only = card.markers.first else { return .automatic }
        return .region(MKCoordinateRegion(center: only.location, latitudinalMeters: 8000, longitudinalMeters: 8000))
    }

    @ViewBuilder private func row(index: Int, place: Place) -> some View {
        let label = HStack(spacing: 10) {
            Text("\(index + 1)")
                .font(.caption.weight(.bold))
                .foregroundStyle(.white)
                .frame(width: 22, height: 22)
                .background(Color.accentColor, in: Circle())
            Text(place.label)
                .font(.subheadline)
                .foregroundStyle(.primary)
                .multilineTextAlignment(.leading)
            Spacer(minLength: 4)
            Text(place.coordinateText)
                .font(.caption2.monospacedDigit())
                .foregroundStyle(.secondary)
        }
        if let url = place.mapsURL {
            Link(destination: url) { label }
                .accessibilityHint("Opens in Maps")
        } else {
            label
        }
    }
}
