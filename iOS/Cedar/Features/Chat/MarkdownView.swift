import SwiftUI

/// Renders the agent's Markdown: blocks from `MarkdownBlock`, inline styling from `Text`.
struct MarkdownView: View {
    let text: String

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            ForEach(Array(MarkdownBlock.parse(text).enumerated()), id: \.offset) { _, block in
                BlockView(block: block)
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .textSelection(.enabled)
    }
}

private struct BlockView: View {
    let block: MarkdownBlock

    var body: some View {
        switch block {
        case .heading(let level, let text):
            InlineText(text)
                .font(level == 1 ? .title2.bold() : level == 2 ? .title3.bold() : .headline)
                .padding(.top, 4)
        case .paragraph(let text):
            InlineText(text)
        case .code(let language, let code):
            CodeBox(label: language.isEmpty ? nil : language, text: code)
        case .list(let ordered, let start, let items):
            VStack(alignment: .leading, spacing: 4) {
                ForEach(Array(items.enumerated()), id: \.offset) { offset, item in
                    HStack(alignment: .firstTextBaseline, spacing: 6) {
                        Text(ordered ? "\(start + offset)." : "•")
                            .monospacedDigit()
                            .foregroundStyle(.secondary)
                        InlineText(item)
                    }
                }
            }
        case .quote(let text):
            HStack(spacing: 10) {
                RoundedRectangle(cornerRadius: 1.5)
                    .fill(.quaternary)
                    .frame(width: 3)
                InlineText(text)
                    .foregroundStyle(.secondary)
            }
            .fixedSize(horizontal: false, vertical: true)
        case .table(let header, let rows):
            ScrollView(.horizontal) {
                Grid(alignment: .leading, horizontalSpacing: 16, verticalSpacing: 6) {
                    GridRow {
                        ForEach(Array(header.enumerated()), id: \.offset) { _, cell in
                            InlineText(cell).fontWeight(.semibold)
                        }
                    }
                    Divider()
                    ForEach(Array(rows.enumerated()), id: \.offset) { _, row in
                        GridRow {
                            ForEach(Array(row.enumerated()), id: \.offset) { _, cell in
                                InlineText(cell)
                            }
                        }
                    }
                }
                .padding(.vertical, 4)
            }
        case .rule:
            Divider()
        }
    }
}

/// A run of inline Markdown (emphasis, code spans, links), keeping line breaks.
private struct InlineText: View {
    let source: String

    init(_ source: String) {
        self.source = source
    }

    var body: some View {
        Text(attributed)
    }

    private var attributed: AttributedString {
        let options = AttributedString.MarkdownParsingOptions(interpretedSyntax: .inlineOnlyPreservingWhitespace)
        return (try? AttributedString(markdown: source, options: options)) ?? AttributedString(source)
    }
}
