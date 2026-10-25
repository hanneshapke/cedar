package dev.cedar.core

/**
 * The block structure of a Markdown document, as iOS/Cedar/Models/MarkdownBlocks.swift splits it.
 * Inline Markdown (bold, code, links) is rendered per block by the app.
 */
sealed interface MarkdownBlock {
    data class Heading(val level: Int, val text: String) : MarkdownBlock
    data class Paragraph(val text: String) : MarkdownBlock
    data class Code(val language: String, val code: String) : MarkdownBlock
    data class ListBlock(val ordered: Boolean, val start: Int, val items: List<String>) : MarkdownBlock
    data class Quote(val text: String) : MarkdownBlock
    data class Table(val header: List<String>, val rows: List<List<String>>) : MarkdownBlock
    data object Rule : MarkdownBlock

    companion object {
        fun parse(source: String): List<MarkdownBlock> = Parser(source.replace("\r\n", "\n").split("\n")).run()
    }
}

private class Parser(val lines: List<String>) {
    var index = 0
    val blocks = mutableListOf<MarkdownBlock>()
    val paragraph = mutableListOf<String>()

    fun run(): List<MarkdownBlock> {
        while (index < lines.size) {
            val line = lines[index]
            val trimmed = line.trim(' ', '\t')
            val fence = fenceMarker(trimmed)
            val heading = heading(trimmed)
            when {
                trimmed.isEmpty() -> {
                    flushParagraph()
                    index += 1
                }
                fence != null -> {
                    flushParagraph()
                    parseFence(fence, trimmed.drop(fence.length))
                }
                heading != null -> {
                    flushParagraph()
                    blocks += heading
                    index += 1
                }
                isRule(trimmed) -> {
                    flushParagraph()
                    blocks += MarkdownBlock.Rule
                    index += 1
                }
                trimmed.startsWith(">") -> {
                    flushParagraph()
                    parseQuote()
                }
                listMarker(line) != null -> {
                    flushParagraph()
                    parseList()
                }
                trimmed.startsWith("|") && index + 1 < lines.size && isTableSeparator(lines[index + 1]) -> {
                    flushParagraph()
                    parseTable()
                }
                else -> {
                    paragraph += trimmed
                    index += 1
                }
            }
        }
        flushParagraph()
        return blocks
    }

    fun flushParagraph() {
        if (paragraph.isEmpty()) return
        blocks += MarkdownBlock.Paragraph(paragraph.joinToString("\n"))
        paragraph.clear()
    }

    fun fenceMarker(trimmed: String): String? {
        for (marker in listOf("```", "~~~")) {
            if (trimmed.startsWith(marker)) return trimmed.takeWhile { it == marker[0] }
        }
        return null
    }

    fun parseFence(fence: String, info: String) {
        index += 1
        val code = mutableListOf<String>()
        while (index < lines.size) {
            val trimmed = lines[index].trim(' ', '\t')
            if (trimmed.startsWith(fence) && trimmed.all { it == fence[0] }) {
                index += 1
                break
            }
            code += lines[index]
            index += 1
        }
        val language = info.trim(' ', '\t').split(" ").firstOrNull { it.isNotEmpty() } ?: ""
        blocks += MarkdownBlock.Code(language, code.joinToString("\n"))
    }

    fun heading(trimmed: String): MarkdownBlock? {
        val hashes = trimmed.takeWhile { it == '#' }.length
        if (hashes !in 1..6) return null
        val rest = trimmed.drop(hashes)
        if (rest.isNotEmpty() && rest[0] != ' ') return null
        var text = rest.trim(' ', '\t')
        while (text.endsWith("#")) text = text.dropLast(1)
        return MarkdownBlock.Heading(hashes, text.trim(' ', '\t'))
    }

    fun isRule(trimmed: String): Boolean {
        val compact = trimmed.replace(" ", "")
        if (compact.length < 3) return false
        val first = compact[0]
        if (first !in "-*_") return false
        return compact.all { it == first }
    }

    fun parseQuote() {
        val text = mutableListOf<String>()
        while (index < lines.size) {
            val trimmed = lines[index].trim(' ', '\t')
            if (!trimmed.startsWith(">")) break
            var rest = trimmed.drop(1)
            if (rest.startsWith(" ")) rest = rest.drop(1)
            text += rest
            index += 1
        }
        blocks += MarkdownBlock.Quote(text.joinToString("\n"))
    }

    data class Marker(val ordered: Boolean, val number: Int, val text: String)

    /** The list marker at the start of `line`: whether it is ordered, its number, and the text after it. */
    fun listMarker(line: String): Marker? {
        val trimmed = line.dropWhile { it == ' ' }
        if (trimmed.isNotEmpty() && trimmed[0] in "-*+" && trimmed.getOrNull(1) == ' ') {
            return Marker(false, 0, trimmed.drop(2))
        }
        val digits = trimmed.takeWhile { it in '0'..'9' }
        if (digits.isEmpty() || digits.length > 9) return null
        val rest = trimmed.drop(digits.length)
        val delimiter = rest.getOrNull(0) ?: return null
        if ((delimiter != '.' && delimiter != ')') || rest.getOrNull(1) != ' ') return null
        return Marker(true, digits.toIntOrNull() ?: 1, rest.drop(2))
    }

    fun parseList() {
        val first = listMarker(lines[index]) ?: return
        val items = mutableListOf(first.text)
        index += 1
        while (index < lines.size) {
            val line = lines[index]
            val marker = listMarker(line)
            if (marker != null && marker.ordered == first.ordered) {
                items += marker.text
                index += 1
            } else if (line.startsWith("  ") && line.trim(' ', '\t').isNotEmpty() && items.isNotEmpty()) {
                // An indented continuation (or nested item), kept with the item above.
                items[items.size - 1] = items.last() + "\n" + line.trim(' ', '\t')
                index += 1
            } else {
                break
            }
        }
        blocks += MarkdownBlock.ListBlock(first.ordered, first.number, items)
    }

    fun isTableSeparator(line: String): Boolean {
        val parts = cells(line)
        return parts.isNotEmpty() && parts.all { cell -> cell.isNotEmpty() && cell.all { it == '-' || it == ':' } && cell.contains('-') }
    }

    fun cells(line: String): List<String> {
        var trimmed = line.trim(' ', '\t')
        if (trimmed.startsWith("|")) trimmed = trimmed.drop(1)
        if (trimmed.endsWith("|")) trimmed = trimmed.dropLast(1)
        return trimmed.split("|").map { it.trim(' ', '\t') }
    }

    fun parseTable() {
        val header = cells(lines[index])
        index += 2
        val rows = mutableListOf<List<String>>()
        while (index < lines.size && lines[index].trim(' ', '\t').startsWith("|")) {
            var row = cells(lines[index])
            if (row.size < header.size) row = row + List(header.size - row.size) { "" }
            rows += row.take(header.size)
            index += 1
        }
        blocks += MarkdownBlock.Table(header, rows)
    }
}
