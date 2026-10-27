import SwiftUI

/// A `show_recipe` card: the photo, the times, the ingredients as a checklist, and the steps to
/// work through one after another.
struct RecipeCardView: View {
    let card: RecipeCard
    @State private var got: Set<Int> = []
    @State private var done: Set<Int> = []

    /// The first step not done yet, or `nil` when all are.
    private var current: Int? { card.steps.indices.first { !done.contains($0) } }

    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            header
            if !card.facts.isEmpty {
                ScrollView(.horizontal, showsIndicators: false) {
                    HStack(spacing: 6) {
                        ForEach(card.facts, id: \.self) { fact in
                            HStack(spacing: 4) {
                                Text(L10n.string(fact.label)).foregroundStyle(.secondary)
                                Text(fact.value).fontWeight(.medium)
                            }
                            .font(.caption)
                            .padding(.horizontal, 9)
                            .padding(.vertical, 5)
                            .background(Color(.tertiarySystemFill), in: Capsule())
                        }
                    }
                }
            }
            if let description = card.description {
                Text(description).font(.subheadline).foregroundStyle(.secondary)
            }
            ingredients
            steps
            if !card.tips.isEmpty {
                VStack(alignment: .leading, spacing: 6) {
                    CardLabel(card.tips.count == 1 ? "Tip" : "Tips")
                    ForEach(card.tips, id: \.self) { tip in
                        Label(tip, systemImage: "lightbulb")
                            .font(.subheadline)
                    }
                }
                .padding(12)
                .frame(maxWidth: .infinity, alignment: .leading)
                .background(Color.accentColor.opacity(0.08), in: RoundedRectangle(cornerRadius: 10))
            }
        }
        .cardStyle()
    }

    // MARK: Header

    @ViewBuilder private var header: some View {
        if let photo = card.photo {
            AsyncImage(url: photo.url) { phase in
                switch phase {
                case .success(let image):
                    VStack(alignment: .leading, spacing: 4) {
                        image
                            .resizable()
                            .scaledToFill()
                            .frame(height: 190)
                            .frame(maxWidth: .infinity)
                            .clipped()
                            .overlay(alignment: .bottomLeading) {
                                LinearGradient(colors: [.clear, .black.opacity(0.7)], startPoint: .center, endPoint: .bottom)
                                    .overlay(alignment: .bottomLeading) {
                                        title.foregroundStyle(.white).padding(12)
                                    }
                            }
                            .clipShape(RoundedRectangle(cornerRadius: 10))
                            .accessibilityElement(children: .combine)
                        credit(photo)
                    }
                case .failure:
                    title
                default:
                    RoundedRectangle(cornerRadius: 10)
                        .fill(Color(.tertiarySystemFill))
                        .frame(height: 190)
                        .overlay(alignment: .bottomLeading) { title.padding(12) }
                }
            }
        } else {
            title
        }
    }

    private var title: some View {
        Text(card.title)
            .font(.title3.weight(.semibold))
            .accessibilityAddTraits(.isHeader)
    }

    @ViewBuilder private func credit(_ photo: RecipeCard.Photo) -> some View {
        let text = Text("Photo: \(photo.source)").font(.caption2).foregroundStyle(.secondary)
        if let page = photo.pageURL {
            Link(destination: page) { text }
        } else {
            text
        }
    }

    // MARK: Ingredients

    private var ingredients: some View {
        VStack(alignment: .leading, spacing: 8) {
            CardLabel("Ingredients")
            ForEach(Array(card.ingredients.enumerated()), id: \.offset) { index, ingredient in
                let checked = got.contains(index)
                Button {
                    if checked { got.remove(index) } else { got.insert(index) }
                } label: {
                    HStack(alignment: .firstTextBaseline, spacing: 8) {
                        Image(systemName: checked ? "checkmark.circle.fill" : "circle")
                            .foregroundStyle(checked ? Color.accentColor : .secondary)
                        Group {
                            if let quantity = ingredient.quantity {
                                Text(quantity).fontWeight(.semibold) + Text(verbatim: " ") + Text(ingredient.item)
                            } else {
                                Text(ingredient.item)
                            }
                        }
                        .font(.subheadline)
                        .strikethrough(checked)
                        .foregroundStyle(checked ? .secondary : .primary)
                        .multilineTextAlignment(.leading)
                        Spacer(minLength: 0)
                    }
                    .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                .accessibilityAddTraits(checked ? .isSelected : [])
            }
        }
    }

    // MARK: Steps

    private var steps: some View {
        VStack(alignment: .leading, spacing: 10) {
            HStack {
                CardLabel("Steps")
                Spacer()
                Text(progress)
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }
            ForEach(Array(card.steps.enumerated()), id: \.offset) { index, step in
                stepRow(index: index, step: step)
            }
            Button {
                withAnimation(.snappy) {
                    if let current { done.insert(current) } else { done.removeAll() }
                }
            } label: {
                Text(nextLabel).frame(maxWidth: .infinity)
            }
            .buttonStyle(.borderedProminent)
            .tint(current == nil ? .secondary : .accentColor)
        }
    }

    /// How many steps there are, or how many are done.
    private var progress: LocalizedStringKey {
        let count = card.steps.count
        if !done.isEmpty { return "\(done.count) of \(count) done" }
        return count == 1 ? "\(count) step" : "\(count) steps"
    }

    private var nextLabel: String {
        guard let current else { return L10n.string("All done. Start over") }
        return current == card.steps.count - 1 ? L10n.string("Finish") : L10n.format("Done with step %lld", current + 1)
    }

    private func stepRow(index: Int, step: RecipeCard.Step) -> some View {
        let isDone = done.contains(index)
        let isCurrent = index == current
        return HStack(alignment: .top, spacing: 10) {
            Button {
                withAnimation(.snappy) {
                    if isDone { done.remove(index) } else { done.insert(index) }
                }
            } label: {
                ZStack {
                    Circle()
                        .fill(isDone ? Color.accentColor : isCurrent ? Color.accentColor.opacity(0.15) : Color(.tertiarySystemFill))
                    if isDone {
                        Image(systemName: "checkmark").font(.caption.weight(.bold)).foregroundStyle(.white)
                    } else {
                        Text(verbatim: "\(index + 1)").font(.caption.weight(.bold))
                            .foregroundStyle(isCurrent ? Color.accentColor : .secondary)
                    }
                }
                .frame(width: 26, height: 26)
            }
            .buttonStyle(.plain)
            .accessibilityLabel(isDone ? "Step \(index + 1): done, mark as not done" : "Step \(index + 1): mark as done")
            VStack(alignment: .leading, spacing: 3) {
                if step.title != nil || step.duration != nil {
                    HStack(spacing: 8) {
                        if let title = step.title {
                            Text(title).font(.subheadline.weight(.semibold))
                        }
                        if let duration = step.duration {
                            Label(duration, systemImage: "clock")
                                .font(.caption)
                                .foregroundStyle(.secondary)
                        }
                    }
                }
                Text(step.text)
                    .font(.subheadline)
                    .foregroundStyle(isDone ? .secondary : .primary)
                    .fixedSize(horizontal: false, vertical: true)
            }
            .opacity(isDone ? 0.7 : 1)
        }
    }
}
