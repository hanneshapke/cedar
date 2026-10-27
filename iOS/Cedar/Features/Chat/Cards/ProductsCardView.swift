import SwiftUI

/// A `search_products` card: the model's recommendation; a tile per option (its photo, its
/// badges, the price and savings, the stars and the verdict on the reviews), which also picks the
/// product shown below; the selected product with its photos, what is known of its stock, a link
/// to the store, why it fits and who it suits, its pros and cons, the reading of its reviews,
/// features, specs and sources; and the comparable options, which can be viewed in turn.
struct ProductsCardView: View {
    let card: ProductsCard
    @State private var selected: String
    /// The option a comparable product was opened from, for the way back.
    @State private var origin: String?
    /// The photo shown in the gallery.
    @State private var photo = 0

    init(card: ProductsCard) {
        self.card = card
        _selected = State(initialValue: card.options[0].id)
    }

    private var product: ProductsCard.Product { card.product(selected) ?? card.options[0] }

    private func isOption(_ sku: String) -> Bool { card.options.contains { $0.id == sku } }

    private func select(_ sku: String, from: String?) {
        withAnimation(.snappy(duration: 0.2)) {
            selected = sku
            origin = from
            photo = 0
        }
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            header
            if let summary = card.summary {
                VStack(alignment: .leading, spacing: 4) {
                    CardLabel("The recommendation")
                    Text(summary)
                        .font(.subheadline)
                        .fixedSize(horizontal: false, vertical: true)
                }
                .padding(10)
                .frame(maxWidth: .infinity, alignment: .leading)
                .background(Color.accentColor.opacity(0.08), in: RoundedRectangle(cornerRadius: 10))
            }
            if let note = card.note {
                Text(note)
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .fixedSize(horizontal: false, vertical: true)
            }
            tiles
            Divider()
            detail
            comparable
            footer
        }
        .cardStyle()
    }

    // MARK: Header

    private var header: some View {
        HStack(spacing: 10) {
            Image(systemName: "bag")
                .font(.subheadline.weight(.semibold))
                .foregroundStyle(Color.accentColor)
                .frame(width: 30, height: 30)
                .background(Color.accentColor.opacity(0.14), in: RoundedRectangle(cornerRadius: 8))
                .accessibilityHidden(true)
            VStack(alignment: .leading, spacing: 1) {
                Text(title)
                    .font(.headline)
                    .accessibilityAddTraits(.isHeader)
                Text(subtitle)
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }
        }
    }

    private var title: String {
        guard let first = card.query.first else { return card.query }
        return first.uppercased() + card.query.dropFirst()
    }

    private var subtitle: String {
        let count = { (n: Int) in n.formatted(.number.locale(L10n.locale)) }
        var parts = [card.store, L10n.format(card.total == 1 ? "%lld of %@ result" : "%lld of %@ results", card.options.count, count(card.total))]
        if let found = card.productsFound, let pages = card.pages {
            let key: String
            switch (found == 1, pages == 1) {
            case (true, true): key = "%lld picked from %@ product on %@ page"
            case (true, false): key = "%lld picked from %@ product on %@ pages"
            case (false, true): key = "%lld picked from %@ products on %@ page"
            case (false, false): key = "%lld picked from %@ products on %@ pages"
            }
            parts = [L10n.format(key, card.options.count, count(found), count(pages))]
        }
        if let sort = card.sort { parts.append(L10n.string(sort)) }
        return parts.joined(separator: " · ")
    }

    // MARK: Option tiles

    private var tiles: some View {
        ScrollView(.horizontal, showsIndicators: false) {
            HStack(alignment: .top, spacing: 8) {
                ForEach(card.options) { p in
                    let isSelected = p.id == selected
                    Button {
                        select(p.id, from: nil)
                    } label: {
                        VStack(alignment: .leading, spacing: 4) {
                            ProductPhoto(url: p.images.first, width: 126, height: 110)
                                .overlay(alignment: .topLeading) {
                                    VStack(alignment: .leading, spacing: 3) {
                                        ForEach(card.badges[p.id] ?? [], id: \.self) { PickBadge(text: $0) }
                                    }
                                    .padding(5)
                                }
                            Text(p.name)
                                .font(.caption.weight(.semibold))
                                .lineLimit(2, reservesSpace: true)
                                .multilineTextAlignment(.leading)
                            if let brand = p.brand {
                                Text(brand)
                                    .font(.caption2)
                                    .foregroundStyle(.secondary)
                                    .lineLimit(1)
                            }
                            PriceLine(product: p, currency: card.currency(of: p), large: false)
                            RatingLine(reviews: p.reviews)
                            VerdictPill(verdict: p.reviews.verdict, small: true)
                        }
                        .padding(8)
                        .frame(width: 142, alignment: .leading)
                        .background(isSelected ? Color.accentColor.opacity(0.1) : Color(.tertiarySystemFill).opacity(0.5),
                                    in: RoundedRectangle(cornerRadius: 10))
                        .overlay(RoundedRectangle(cornerRadius: 10)
                            .strokeBorder(isSelected ? Color.accentColor.opacity(0.6) : Color.clear))
                    }
                    .buttonStyle(.plain)
                    .accessibilityLabel(tileLabel(p))
                    .accessibilityAddTraits(isSelected ? .isSelected : [])
                }
            }
        }
    }

    private func tileLabel(_ p: ProductsCard.Product) -> String {
        var parts = [p.name, priceText(p)]
        if let average = p.reviews.average {
            parts.append(L10n.format("%@ stars from %@ reviews", ProductFormat.rating(average), p.reviews.count.formatted(.number.locale(L10n.locale))))
        } else {
            parts.append(L10n.string("no reviews"))
        }
        parts.append(p.reviews.verdict.label)
        parts += (card.badges[p.id] ?? []).map { L10n.string($0) }
        return parts.joined(separator: ", ")
    }

    // MARK: The selected product

    private var detail: some View {
        let p = product
        let badges = card.badges[p.id] ?? []
        let sub = [p.brand, p.model.map { L10n.format("Model %@", $0) }, p.color].compactMap { $0 }.joined(separator: " · ")
        return VStack(alignment: .leading, spacing: 12) {
            if !isOption(p.id), let origin = origin, let from = card.product(origin) {
                Button {
                    select(from.id, from: nil)
                } label: {
                    Label("Back to \(from.name)", systemImage: "chevron.left")
                        .font(.caption.weight(.medium))
                        .lineLimit(1)
                }
                .buttonStyle(.bordered)
                .controlSize(.small)
            }
            gallery(p)
            VStack(alignment: .leading, spacing: 4) {
                if !badges.isEmpty || p.category != nil {
                    HStack(spacing: 4) {
                        ForEach(badges, id: \.self) { PickBadge(text: $0) }
                        if let category = p.category {
                            Text(category)
                                .font(.caption2)
                                .foregroundStyle(.secondary)
                                .padding(.horizontal, 7)
                                .padding(.vertical, 2)
                                .overlay(Capsule().strokeBorder(Color(.separator)))
                                .lineLimit(1)
                        }
                    }
                }
                Text(p.name)
                    .font(.headline)
                    .fixedSize(horizontal: false, vertical: true)
                    .accessibilityAddTraits(.isHeader)
                if !sub.isEmpty {
                    Text(sub)
                        .font(.caption)
                        .foregroundStyle(.secondary)
                }
            }
            PriceLine(product: p, currency: card.currency(of: p), large: true)
            availability(p)
            if let url = p.url {
                Link(destination: url) {
                    Label("View at \(p.store ?? card.store)", systemImage: "arrow.up.right.square")
                        .font(.subheadline.weight(.semibold))
                        .frame(maxWidth: .infinity)
                }
                .buttonStyle(.borderedProminent)
            }
            if p.why != nil || p.bestFor != nil {
                VStack(alignment: .leading, spacing: 4) {
                    CardLabel("Why it fits")
                    if let why = p.why {
                        Text(why)
                            .font(.subheadline)
                            .fixedSize(horizontal: false, vertical: true)
                    }
                    if let bestFor = p.bestFor {
                        (Text("Best for: ").fontWeight(.semibold) + Text(bestFor))
                            .font(.subheadline)
                            .foregroundStyle(.secondary)
                            .fixedSize(horizontal: false, vertical: true)
                    }
                }
            }
            if !p.pros.isEmpty || !p.cons.isEmpty {
                HStack(alignment: .top, spacing: 16) {
                    if !p.pros.isEmpty { prosCons("Pros", p.pros, icon: "plus.circle.fill", tint: .green) }
                    if !p.cons.isEmpty { prosCons("Cons", p.cons, icon: "minus.circle.fill", tint: .red) }
                }
            }
            ReviewsPanel(reviews: p.reviews)
            if let description = p.description {
                Text(description)
                    .font(.subheadline)
                    .fixedSize(horizontal: false, vertical: true)
            }
            if !p.features.isEmpty {
                VStack(alignment: .leading, spacing: 5) {
                    CardLabel("Key features")
                    ForEach(p.features, id: \.self) { feature in
                        Label(feature, systemImage: "checkmark")
                            .font(.subheadline)
                            .fixedSize(horizontal: false, vertical: true)
                    }
                }
            }
            if !p.specs.isEmpty {
                VStack(alignment: .leading, spacing: 5) {
                    CardLabel("Specs")
                    Grid(alignment: .leading, horizontalSpacing: 12, verticalSpacing: 4) {
                        ForEach(p.specs, id: \.self) { spec in
                            GridRow {
                                Text(spec.name).foregroundStyle(.secondary)
                                Text(spec.value).fixedSize(horizontal: false, vertical: true)
                            }
                        }
                    }
                    .font(.subheadline)
                }
            }
            if p.sources.contains(where: { card.sources[$0] != nil }) {
                VStack(alignment: .leading, spacing: 4) {
                    CardLabel("Sources")
                    ForEach(p.sources, id: \.self) { n in
                        if let source = card.sources[n] {
                            HStack(alignment: .firstTextBaseline, spacing: 6) {
                                Text(verbatim: "\(n)")
                                    .font(.caption2.monospacedDigit())
                                    .foregroundStyle(.secondary)
                                if let url = source.url {
                                    Link(source.title, destination: url)
                                        .font(.caption)
                                        .lineLimit(1)
                                } else {
                                    Text(source.title)
                                        .font(.caption)
                                        .lineLimit(1)
                                }
                                if let host = source.host {
                                    Text(host)
                                        .font(.caption2)
                                        .foregroundStyle(.secondary)
                                        .lineLimit(1)
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    private func prosCons(_ title: LocalizedStringKey, _ items: [String], icon: String, tint: Color) -> some View {
        VStack(alignment: .leading, spacing: 4) {
            CardLabel(title)
            ForEach(items, id: \.self) { item in
                Label {
                    Text(item)
                        .font(.subheadline)
                        .fixedSize(horizontal: false, vertical: true)
                } icon: {
                    Image(systemName: icon).foregroundStyle(tint)
                }
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
    }

    /// A product's price as text: about, when it comes from text rather than a store page.
    private func priceText(_ p: ProductsCard.Product) -> String {
        guard let price = p.price else { return L10n.string("price at the store") }
        let amount = ProductFormat.money(price, card.currency(of: p))
        return p.priceChecked ? amount : L10n.format("about %@", amount)
    }

    /// How much more `c` costs than `p`, when both prices are known and in one currency.
    private func gap(_ c: ProductsCard.Product, _ p: ProductsCard.Product) -> Double? {
        guard let a = c.price, let b = p.price, card.currency(of: c) == card.currency(of: p) else { return nil }
        return a - b
    }

    private func gallery(_ p: ProductsCard.Product) -> some View {
        let index = min(photo, max(0, p.images.count - 1))
        return VStack(alignment: .leading, spacing: 8) {
            ProductPhoto(url: p.images.isEmpty ? nil : p.images[index], width: nil, height: 220)
                .accessibilityElement()
                .accessibilityLabel(p.images.isEmpty ? "No photo" : "Photo \(index + 1) of \(p.images.count) of \(p.name)")
                .accessibilityAddTraits(.isImage)
            if p.images.count > 1 {
                ScrollView(.horizontal, showsIndicators: false) {
                    HStack(spacing: 6) {
                        ForEach(Array(p.images.enumerated()), id: \.offset) { i, url in
                            Button {
                                photo = i
                            } label: {
                                ProductPhoto(url: url, width: 48, height: 48)
                                    .overlay(RoundedRectangle(cornerRadius: 8)
                                        .strokeBorder(i == index ? Color.accentColor : Color(.separator), lineWidth: i == index ? 2 : 1))
                            }
                            .buttonStyle(.plain)
                            .accessibilityLabel("Photo \(i + 1) of \(p.images.count)")
                            .accessibilityAddTraits(i == index ? .isSelected : [])
                        }
                    }
                    .padding(1)
                }
            }
        }
    }

    private struct Chip: Hashable {
        let text: String
        let ok: Bool
    }

    /// Only what the store said: a page that doesn't mention stock gets no chip for it.
    private func chipList(_ p: ProductsCard.Product) -> [Chip] {
        var chips: [Chip] = []
        if let online = p.online { chips.append(Chip(text: L10n.string(online ? "In stock" : "Out of stock"), ok: online)) }
        if p.inStore == true { chips.append(Chip(text: L10n.string("In store"), ok: true)) }
        if p.freeShipping == true { chips.append(Chip(text: L10n.string("Free shipping"), ok: true)) }
        if let condition = p.condition, condition.lowercased() != "new" { chips.append(Chip(text: condition, ok: false)) }
        return chips
    }

    @ViewBuilder private func availability(_ p: ProductsCard.Product) -> some View {
        let chips = chipList(p)
        if !chips.isEmpty {
            ViewThatFits(in: .horizontal) {
                HStack(spacing: 6) { chipRow(chips) }
                VStack(alignment: .leading, spacing: 6) { chipRow(chips) }
            }
        }
    }

    private func chipRow(_ chips: [Chip]) -> some View {
        ForEach(chips, id: \.self) { chip in
            Label(chip.text, systemImage: chip.ok ? "checkmark" : "info.circle")
                .font(.caption)
                .foregroundStyle(chip.ok ? Color.green : Color.secondary)
                .padding(.horizontal, 8)
                .padding(.vertical, 4)
                .background(Color(.tertiarySystemFill).opacity(0.6), in: Capsule())
        }
    }

    // MARK: Comparable options

    @ViewBuilder private var comparable: some View {
        let p = product
        let items = p.comparable.compactMap { card.product($0) }
        if !items.isEmpty {
            VStack(alignment: .leading, spacing: 8) {
                CardLabel("Comparable options")
                ScrollView(.horizontal, showsIndicators: false) {
                    HStack(alignment: .top, spacing: 8) {
                        ForEach(items) { c in
                            Button {
                                select(c.id, from: isOption(p.id) ? p.id : origin)
                            } label: {
                                mini(c, against: p)
                            }
                            .buttonStyle(.plain)
                            .accessibilityLabel([c.name, priceText(c), gap(c, p).map { difference($0, card.currency(of: c)) }]
                                .compactMap { $0 }.joined(separator: ", "))
                            .accessibilityHint("Shows this product")
                        }
                    }
                }
            }
        }
    }

    private func mini(_ c: ProductsCard.Product, against p: ProductsCard.Product) -> some View {
        let diff = gap(c, p)
        return HStack(alignment: .top, spacing: 8) {
            ProductPhoto(url: c.images.first, width: 56, height: 56)
            VStack(alignment: .leading, spacing: 3) {
                Text(c.name)
                    .font(.caption.weight(.semibold))
                    .lineLimit(2, reservesSpace: true)
                    .multilineTextAlignment(.leading)
                HStack(alignment: .firstTextBaseline, spacing: 5) {
                    Text(c.price.map { (c.priceChecked ? "" : "≈ ") + ProductFormat.money($0, card.currency(of: c)) } ?? L10n.string("Price at the store"))
                        .font(.caption.weight(.bold))
                        .monospacedDigit()
                    if let diff {
                        Text(difference(diff, card.currency(of: c)))
                            .font(.caption2.weight(.semibold))
                            .foregroundStyle(abs(diff) < 0.005 ? Color.secondary : diff < 0 ? Color.green : Color.red)
                    }
                }
                RatingLine(reviews: c.reviews)
                VerdictPill(verdict: c.reviews.verdict, small: true)
            }
        }
        .padding(8)
        .frame(width: 236, alignment: .leading)
        .background(Color(.tertiarySystemFill).opacity(0.5), in: RoundedRectangle(cornerRadius: 10))
    }

    private func difference(_ diff: Double, _ currency: String) -> String {
        if abs(diff) < 0.005 { return L10n.string("same price") }
        return L10n.format(diff < 0 ? "%@ less" : "%@ more", ProductFormat.money(abs(diff), currency))
    }

    // MARK: Footer

    private var footer: some View {
        VStack(alignment: .leading, spacing: 3) {
            if let asOf = card.asOf {
                Text("As of \(asOf.formatted(Date.FormatStyle(date: .abbreviated, time: .shortened, locale: L10n.locale)))")
            }
            if let disclaimer = card.disclaimer {
                Text(disclaimer).foregroundStyle(.tertiary)
            }
        }
        .font(.caption2)
        .foregroundStyle(.secondary)
    }
}

/// The reading of a product's reviews: the stars, the rating adjusted for how many reviews there
/// are on a bar, the verdict and how much to trust it, and cedar's sentence.
private struct ReviewsPanel: View {
    let reviews: ProductsCard.Reviews

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            CardLabel("Reviews")
            HStack(alignment: .center, spacing: 8) {
                Stars(value: reviews.average, size: 15)
                if let average = reviews.average {
                    Text(ProductFormat.rating(average))
                        .font(.title3.weight(.bold))
                        .monospacedDigit()
                    Text(reviews.count == 1 ? "from \(count) review" : "from \(count) reviews")
                        .font(.subheadline)
                        .foregroundStyle(.secondary)
                } else {
                    Text("No customer rating found")
                        .font(.subheadline)
                        .foregroundStyle(.secondary)
                }
            }
            if let adjusted = reviews.adjusted {
                VStack(alignment: .leading, spacing: 4) {
                    HStack {
                        Text("Adjusted for the number of reviews").foregroundStyle(.secondary)
                        Spacer(minLength: 6)
                        Text(verbatim: "\(ProductFormat.rating(adjusted)) / 5")
                            .fontWeight(.semibold)
                            .monospacedDigit()
                    }
                    .font(.caption)
                    ProgressView(value: adjusted, total: 5)
                        .tint(ProductFormat.color(reviews.verdict))
                }
                .accessibilityElement(children: .ignore)
                .accessibilityLabel("Adjusted for the number of reviews: \(ProductFormat.rating(adjusted)) out of 5")
            }
            HStack(spacing: 8) {
                VerdictPill(verdict: reviews.verdict, small: false)
                if reviews.count > 0 {
                    Text(confidence)
                        .font(.caption)
                        .foregroundStyle(.secondary)
                }
            }
            if let summary = reviews.summary, reviews.average != nil {
                Text(summary)
                    .font(.subheadline)
                    .fixedSize(horizontal: false, vertical: true)
            }
        }
        .padding(12)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(Color(.tertiarySystemFill).opacity(0.5), in: RoundedRectangle(cornerRadius: 10))
    }

    private var count: String { reviews.count.formatted(.number.locale(L10n.locale)) }

    /// "high confidence", for the reading's `low`, `medium` or `high`.
    private var confidence: LocalizedStringKey {
        switch reviews.confidence {
        case "high": return "high confidence"
        case "medium": return "medium confidence"
        default: return "low confidence"
        }
    }
}

/// A product photo in a box that keeps its size while it loads, on white (store photos are shot on
/// white), with a placeholder when there is none or it fails to load.
private struct ProductPhoto: View {
    let url: URL?
    let width: CGFloat?
    let height: CGFloat

    var body: some View {
        Group {
            if let url {
                AsyncImage(url: url) { phase in
                    switch phase {
                    case .success(let image):
                        image
                            .resizable()
                            .scaledToFit()
                            .padding(height > 60 ? 8 : 3)
                            .frame(maxWidth: .infinity, maxHeight: .infinity)
                            .background(Color.white)
                    case .failure:
                        placeholder
                    default:
                        ProgressView()
                            .frame(maxWidth: .infinity, maxHeight: .infinity)
                            .background(Color(.tertiarySystemFill))
                    }
                }
                .id(url)
            } else {
                placeholder
            }
        }
        .frame(width: width, height: height)
        .clipShape(RoundedRectangle(cornerRadius: 8))
        .accessibilityHidden(true)
    }

    private var placeholder: some View {
        Image(systemName: "photo")
            .font(height > 60 ? .title2 : .caption)
            .foregroundStyle(.secondary)
            .frame(maxWidth: .infinity, maxHeight: .infinity)
            .background(Color(.tertiarySystemFill))
    }
}

/// The price, the regular price struck through, and the savings.
private struct PriceLine: View {
    let product: ProductsCard.Product
    let currency: String
    let large: Bool

    private var savings: Int? {
        guard let price = product.price, let regular = product.regularPrice, regular > 0 else { return nil }
        let percent = Int((product.savingsPct ?? (1 - price / regular) * 100).rounded())
        return percent >= 1 ? percent : nil
    }

    var body: some View {
        if let price = product.price {
            priced(price)
        } else {
            Text("Price at the store")
                .font(large ? .headline : .caption.weight(.semibold))
                .foregroundStyle(.secondary)
        }
    }

    /// A price from review or search text is marked "≈", with a word on where it comes from.
    private func priced(_ price: Double) -> some View {
        let amount = ProductFormat.money(price, currency)
        return VStack(alignment: .leading, spacing: 3) {
            HStack(alignment: .firstTextBaseline, spacing: 5) {
                Text((product.priceChecked ? "" : "≈ ") + amount)
                    .font(large ? .title2.weight(.bold) : .subheadline.weight(.bold))
                    .accessibilityLabel(product.priceChecked ? amount : L10n.format("about %@", amount))
                if let regular = product.regularPrice {
                    Text(ProductFormat.money(regular, currency))
                        .font(large ? .subheadline : .caption2)
                        .strikethrough()
                        .foregroundStyle(.secondary)
                        .accessibilityLabel("was \(ProductFormat.money(regular, currency))")
                }
                if large, let savings { SaveTag(percent: savings) }
            }
            .monospacedDigit()
            .lineLimit(1)
            if !large, let savings { SaveTag(percent: savings) }
            if large, !product.priceChecked {
                Text("From review or search text, not checked on a store page")
                    .font(.caption2)
                    .foregroundStyle(.secondary)
            }
        }
    }
}

private struct SaveTag: View {
    let percent: Int

    var body: some View {
        Text("Save \(percent)%")
            .font(.caption2.weight(.bold))
            .foregroundStyle(.red)
            .padding(.horizontal, 6)
            .padding(.vertical, 2)
            .background(Color.red.opacity(0.13), in: Capsule())
    }
}

/// Small stars, the average, and the number of reviews.
private struct RatingLine: View {
    let reviews: ProductsCard.Reviews

    var body: some View {
        HStack(spacing: 4) {
            Stars(value: reviews.average, size: 10)
            if let average = reviews.average {
                Text(ProductFormat.rating(average)).fontWeight(.semibold)
                Text(verbatim: "(\(reviews.count.formatted(.number.locale(L10n.locale))))").foregroundStyle(.secondary)
            } else {
                Text("No reviews").foregroundStyle(.secondary)
            }
        }
        .font(.caption2)
        .monospacedDigit()
        .lineLimit(1)
    }
}

/// Five stars, filled to `value` out of 5 in half stars.
private struct Stars: View {
    let value: Double?
    var size: CGFloat = 12

    var body: some View {
        HStack(spacing: 1) {
            ForEach(0..<5, id: \.self) { i in
                Image(systemName: symbol(i)).font(.system(size: size))
            }
        }
        .foregroundStyle(value == nil ? Color.secondary : Color.orange)
        .accessibilityElement(children: .ignore)
        .accessibilityLabel(value.map { L10n.format("%@ out of 5 stars", ProductFormat.rating($0)) } ?? L10n.string("Not rated"))
    }

    private func symbol(_ i: Int) -> String {
        let halves = ((value ?? 0) * 2).rounded() / 2
        if halves >= Double(i) + 1 { return "star.fill" }
        if halves >= Double(i) + 0.5 { return "star.leadinghalf.filled" }
        return "star"
    }
}

/// The verdict on a product's reviews, in its status color.
private struct VerdictPill: View {
    let verdict: ProductsCard.Verdict
    let small: Bool

    var body: some View {
        let color = ProductFormat.color(verdict)
        Text(verdict.label)
            .font(small ? .caption2.weight(.bold) : .caption.weight(.bold))
            .padding(.horizontal, small ? 6 : 9)
            .padding(.vertical, small ? 2 : 4)
            .foregroundStyle(color)
            .background(color.opacity(0.15), in: Capsule())
            .lineLimit(1)
    }
}

/// One of cedar's picks: best rated, best value, most popular.
private struct PickBadge: View {
    let text: String

    var body: some View {
        // The picks' names are English keys; a result's own badges are shown as they are.
        Text(L10n.string(text).uppercased(with: L10n.locale))
            .font(.system(size: 9, weight: .bold))
            .tracking(0.3)
            .foregroundStyle(.white)
            .padding(.horizontal, 6)
            .padding(.vertical, 2)
            .background(Color.accentColor, in: Capsule())
            .lineLimit(1)
    }
}

enum ProductFormat {
    /// `x` as a price in `currency`, an ISO code, with cents.
    static func money(_ x: Double, _ currency: String) -> String {
        x.formatted(.currency(code: currency).locale(L10n.locale))
    }

    /// A rating out of 5 with one decimal.
    static func rating(_ x: Double) -> String {
        x.formatted(.number.precision(.fractionLength(1)).locale(L10n.locale))
    }

    static func color(_ verdict: ProductsCard.Verdict) -> Color {
        switch verdict {
        case .excellent: return .green
        case .good: return .teal
        case .mixed: return .orange
        case .poor: return .red
        case .few, .unreviewed: return .secondary
        }
    }
}
