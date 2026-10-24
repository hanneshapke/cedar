package dev.cedar.cedar.ui

import androidx.compose.foundation.background
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.IntrinsicSize
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.text.selection.SelectionContainer
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.remember
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.AnnotatedString
import androidx.compose.ui.text.LinkAnnotation
import androidx.compose.ui.text.SpanStyle
import androidx.compose.ui.text.TextLinkStyles
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.buildAnnotatedString
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontStyle
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextDecoration
import androidx.compose.ui.text.withLink
import androidx.compose.ui.text.withStyle
import androidx.compose.ui.unit.dp
import dev.cedar.core.MarkdownBlock

/** Renders the agent's Markdown: blocks from `MarkdownBlock` (in :core), inline styling here. */
@Composable
fun MarkdownView(text: String) {
    val blocks = remember(text) { MarkdownBlock.parse(text) }
    SelectionContainer {
        Column(Modifier.fillMaxWidth(), verticalArrangement = Arrangement.spacedBy(10.dp)) {
            blocks.forEach { Block(it) }
        }
    }
}

@Composable
private fun Block(block: MarkdownBlock) {
    val type = MaterialTheme.typography
    when (block) {
        is MarkdownBlock.Heading -> InlineText(
            block.text,
            when (block.level) {
                1 -> type.titleLarge
                2 -> type.titleMedium
                else -> type.titleSmall
            }.copy(fontWeight = FontWeight.Bold),
            Modifier.padding(top = 4.dp),
        )
        is MarkdownBlock.Paragraph -> InlineText(block.text)
        is MarkdownBlock.Code -> CodeBox(block.language.ifEmpty { null }, block.code)
        is MarkdownBlock.ListBlock -> Column(verticalArrangement = Arrangement.spacedBy(4.dp)) {
            block.items.forEachIndexed { offset, item ->
                Row {
                    Text(if (block.ordered) "${block.start + offset}." else "•", color = CedarColors.secondaryText, style = type.bodyLarge)
                    Spacer(Modifier.width(6.dp))
                    InlineText(item)
                }
            }
        }
        is MarkdownBlock.Quote -> Row(Modifier.height(IntrinsicSize.Min)) {
            Spacer(Modifier.width(3.dp).fillMaxHeight().background(MaterialTheme.colorScheme.outlineVariant))
            Spacer(Modifier.width(10.dp))
            InlineText(block.text, type.bodyLarge.copy(color = CedarColors.secondaryText))
        }
        is MarkdownBlock.Table -> Row(Modifier.horizontalScroll(rememberScrollState())) {
            // Column by column, so each is as wide as its widest cell.
            block.header.indices.forEach { column ->
                Column(Modifier.padding(end = 16.dp).widthIn(max = 280.dp), verticalArrangement = Arrangement.spacedBy(6.dp)) {
                    InlineText(block.header[column], type.bodyMedium.copy(fontWeight = FontWeight.SemiBold))
                    block.rows.forEach { row -> InlineText(row.getOrElse(column) { "" }, type.bodyMedium) }
                }
            }
        }
        MarkdownBlock.Rule -> HorizontalDivider()
    }
}

/** A run of inline Markdown (emphasis, code spans, links), keeping line breaks. */
@Composable
private fun InlineText(source: String, style: TextStyle = MaterialTheme.typography.bodyLarge, modifier: Modifier = Modifier) {
    val link = MaterialTheme.colorScheme.primary
    val code = MaterialTheme.colorScheme.surfaceContainerHighest
    val text = remember(source, link, code) { inline(source, link, code) }
    Text(text, modifier, style = style)
}

/** Parses `**bold**`, `*italic*` / `_italic_`, `` `code` ``, `~~strike~~`, `[text](url)` and bare https links. */
fun inline(source: String, linkColor: Color, codeBackground: Color): AnnotatedString = buildAnnotatedString {
    val linkStyles = TextLinkStyles(SpanStyle(color = linkColor, textDecoration = TextDecoration.Underline))
    var i = 0
    val n = source.length
    fun emphasis(marker: String, style: SpanStyle): Boolean {
        if (!source.startsWith(marker, i)) return false
        val end = source.indexOf(marker, i + marker.length)
        if (end <= i + marker.length || source[i + marker.length] == ' ') return false
        withStyle(style) { append(inline(source.substring(i + marker.length, end), linkColor, codeBackground)) }
        i = end + marker.length
        return true
    }
    while (i < n) {
        val c = source[i]
        when {
            c == '\\' && i + 1 < n && source[i + 1] in "\\`*_[]()~#" -> {
                append(source[i + 1])
                i += 2
            }
            c == '`' -> {
                val end = source.indexOf('`', i + 1)
                if (end > i) {
                    withStyle(SpanStyle(fontFamily = FontFamily.Monospace, background = codeBackground)) { append(source.substring(i + 1, end)) }
                    i = end + 1
                } else {
                    append(c)
                    i += 1
                }
            }
            emphasis("**", SpanStyle(fontWeight = FontWeight.Bold)) -> {}
            emphasis("__", SpanStyle(fontWeight = FontWeight.Bold)) -> {}
            emphasis("~~", SpanStyle(textDecoration = TextDecoration.LineThrough)) -> {}
            c == '*' && emphasis("*", SpanStyle(fontStyle = FontStyle.Italic)) -> {}
            c == '_' && (i == 0 || !source[i - 1].isLetterOrDigit()) && emphasis("_", SpanStyle(fontStyle = FontStyle.Italic)) -> {}
            c == '[' -> {
                val close = source.indexOf("](", i + 1)
                val end = if (close > 0) source.indexOf(')', close + 2) else -1
                if (close > 0 && end > 0) {
                    val label = source.substring(i + 1, close)
                    val url = source.substring(close + 2, end).trim()
                    if (url.startsWith("http://") || url.startsWith("https://") || url.startsWith("mailto:")) {
                        withLink(LinkAnnotation.Url(url, linkStyles)) { append(inline(label, linkColor, codeBackground)) }
                    } else {
                        append(inline(label, linkColor, codeBackground))
                    }
                    i = end + 1
                } else {
                    append(c)
                    i += 1
                }
            }
            source.startsWith("https://", i) || source.startsWith("http://", i) -> {
                var end = i
                while (end < n && !source[end].isWhitespace() && source[end] !in "<>\"") end++
                while (end > i && source[end - 1] in ".,;:!?)") end--
                val url = source.substring(i, end)
                withLink(LinkAnnotation.Url(url, linkStyles)) { append(url) }
                i = end
            }
            else -> {
                append(c)
                i += 1
            }
        }
    }
}
