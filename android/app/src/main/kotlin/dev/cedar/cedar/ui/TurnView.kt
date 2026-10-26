package dev.cedar.cedar.ui

import androidx.compose.animation.animateContentSize
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.layout.ExperimentalLayoutApi
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.selection.SelectionContainer
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.outlined.HelpOutline
import androidx.compose.material.icons.automirrored.outlined.ListAlt
import androidx.compose.material.icons.outlined.BackHand
import androidx.compose.material.icons.outlined.BarChart
import androidx.compose.material.icons.outlined.Build
import androidx.compose.material.icons.outlined.Check
import androidx.compose.material.icons.outlined.CheckBox
import androidx.compose.material.icons.outlined.CheckBoxOutlineBlank
import androidx.compose.material.icons.outlined.CheckCircle
import androidx.compose.material.icons.outlined.ChevronRight
import androidx.compose.material.icons.outlined.Cloud
import androidx.compose.material.icons.outlined.Code
import androidx.compose.material.icons.outlined.ContentCopy
import androidx.compose.material.icons.outlined.Description
import androidx.compose.material.icons.outlined.EditNote
import androidx.compose.material.icons.outlined.Error
import androidx.compose.material.icons.outlined.ExpandMore
import androidx.compose.material.icons.outlined.Language
import androidx.compose.material.icons.outlined.Map
import androidx.compose.material.icons.outlined.OpenInBrowser
import androidx.compose.material.icons.outlined.Psychology
import androidx.compose.material.icons.outlined.RadioButtonChecked
import androidx.compose.material.icons.outlined.RadioButtonUnchecked
import androidx.compose.material.icons.outlined.Restaurant
import androidx.compose.material.icons.outlined.Search
import androidx.compose.material.icons.outlined.ShoppingBag
import androidx.compose.material.icons.outlined.ShowChart
import androidx.compose.material.icons.outlined.StopCircle
import androidx.compose.material.icons.outlined.Terminal
import androidx.compose.material.icons.outlined.Train
import androidx.compose.material.icons.outlined.WarningAmber
import androidx.compose.material3.Button
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateListOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.draw.rotate
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import dev.cedar.cedar.ChatModel
import dev.cedar.cedar.ui.cards.CardView
import dev.cedar.cedar.ui.cards.ChartCardView
import dev.cedar.core.Approval
import dev.cedar.core.ChartCard
import dev.cedar.core.ClarifyAnswer
import dev.cedar.core.Plan
import dev.cedar.core.Questions
import dev.cedar.core.ToolCall
import dev.cedar.core.TranscriptItem
import dev.cedar.core.TurnStatus
import dev.cedar.core.TurnTranscript
import dev.cedar.core.get
import dev.cedar.core.pretty
import dev.cedar.core.str
import kotlinx.coroutines.delay
import java.text.NumberFormat
import java.util.Currency
import java.util.Locale

/** A message and everything the agent did to answer it (iOS/Cedar/Features/Chat/TurnView.swift). */
@Composable
fun TurnView(turn: TurnTranscript, chat: ChatModel, webURL: String?, isLast: Boolean) {
    Column(verticalArrangement = Arrangement.spacedBy(12.dp)) {
        UserBubble(turn.prompt, turn.turn.attachments)
        for (item in turn.items) {
            when (item) {
                is TranscriptItem.Text -> MarkdownView(item.text)
                is TranscriptItem.Reasoning -> ReasoningView(item.text, item.durationMs)
                is TranscriptItem.PlanItem -> PlanView(item.plan, canAgree = isLast && turn.isActive) { chat.agree(item.plan, turn) }
                is TranscriptItem.ApprovalItem -> ApprovalView(item.approval, canDecide = isLast && turn.isActive) { decision ->
                    chat.decide(item.approval, turn, decision)
                }
                is TranscriptItem.Tool -> {
                    ToolCallView(item.call, webURL)
                    item.call.card?.let { CardView(it) }
                    item.call.charts.forEach { ChartCardView(it) }
                }
            }
        }
        val final = turn.turn.finalText
        if (turn.items.isEmpty() && !final.isNullOrEmpty()) MarkdownView(final)
        val conversationId = chat.conversationId
        val client = chat.client
        if (!turn.isActive && conversationId != null && client != null) {
            val images = turn.images
            if (images.isNotEmpty()) ImagesView(images, conversationId, client)
        }
        StatusFooter(turn)
    }
}

@Composable
private fun StatusFooter(turn: TurnTranscript) {
    val context = LocalContext.current
    val secondary = CedarColors.secondaryText
    val small = MaterialTheme.typography.bodySmall
    when (turn.turn.status) {
        TurnStatus.QUEUED -> Working("Queued")
        TurnStatus.RUNNING -> Working(
            when {
                turn.pendingApproval != null -> "Waiting for your say-so"
                turn.awaitingPlan != null -> "Waiting for the plan"
                else -> "Working…"
            },
        )
        TurnStatus.FAILED -> Row(verticalAlignment = Alignment.Top) {
            Icon(Icons.Outlined.WarningAmber, null, Modifier.size(16.dp), tint = CedarColors.red)
            Spacer(Modifier.width(6.dp))
            SelectionContainer { Text(turn.turn.error ?: "The run failed.", style = small, color = CedarColors.red) }
        }
        TurnStatus.CANCELLED -> Row(verticalAlignment = Alignment.CenterVertically) {
            Icon(Icons.Outlined.StopCircle, null, Modifier.size(16.dp), tint = secondary)
            Spacer(Modifier.width(6.dp))
            Text(turn.cancelReason?.let { "Stopped: $it" } ?: "Stopped", style = small, color = secondary)
        }
        TurnStatus.COMPLETED -> {
            var copied by remember { mutableStateOf(false) }
            LaunchedEffect(copied) {
                if (copied) {
                    delay(1500)
                    copied = false
                }
            }
            Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(12.dp)) {
                Icon(
                    if (copied) Icons.Outlined.Check else Icons.Outlined.ContentCopy,
                    "Copy answer",
                    Modifier.size(16.dp).clickable {
                        copyText(context, turn.answer)
                        copied = true
                    },
                    tint = secondary,
                )
                val usage = turn.usage
                if (usage.inputTokens != 0 || usage.outputTokens != 0 || usage.costUSD != null) {
                    val n = NumberFormat.getIntegerInstance()
                    Text("${n.format(usage.inputTokens)} in · ${n.format(usage.outputTokens)} out", style = MaterialTheme.typography.labelSmall, color = secondary)
                }
                usage.costUSD?.let { cost ->
                    val f = NumberFormat.getCurrencyInstance(Locale.US).apply {
                        currency = Currency.getInstance("USD")
                        maximumFractionDigits = if (cost < 0.01) 4 else 2
                        minimumFractionDigits = if (cost < 0.01) 4 else 2
                    }
                    Text(f.format(cost), style = MaterialTheme.typography.labelSmall, color = secondary)
                }
                if (turn.turn.needsAnswer) {
                    Row(verticalAlignment = Alignment.CenterVertically) {
                        Icon(Icons.AutoMirrored.Outlined.HelpOutline, null, Modifier.size(14.dp), tint = CedarColors.orange)
                        Spacer(Modifier.width(3.dp))
                        Text("Needs your answer", style = MaterialTheme.typography.labelSmall, color = CedarColors.orange)
                    }
                } else if (turn.turn.stopReason == "max_tokens") {
                    Text("Cut short", style = MaterialTheme.typography.labelSmall, color = secondary)
                }
            }
        }
    }
}

@Composable
private fun Working(text: String) {
    Row(verticalAlignment = Alignment.CenterVertically) {
        CircularProgressIndicator(Modifier.size(14.dp), strokeWidth = 2.dp)
        Spacer(Modifier.width(8.dp))
        Text(text, style = MaterialTheme.typography.bodySmall, color = CedarColors.secondaryText)
    }
}

/** The model's reasoning, folded to one line. */
@Composable
private fun ReasoningView(text: String, durationMs: Int?) {
    var expanded by rememberSaveable { mutableStateOf(false) }
    Column(Modifier.animateContentSize()) {
        Row(Modifier.clickable { expanded = !expanded }.padding(vertical = 2.dp), verticalAlignment = Alignment.CenterVertically) {
            Icon(Icons.Outlined.Psychology, null, Modifier.size(16.dp), tint = CedarColors.secondaryText)
            Spacer(Modifier.width(6.dp))
            val title = if (durationMs != null && durationMs > 0) "Thought for ${Math.round(durationMs / 1000.0)} s" else "Thought it through"
            Text(title, style = MaterialTheme.typography.bodySmall, color = CedarColors.secondaryText)
            Icon(Icons.Outlined.ExpandMore, null, Modifier.size(16.dp).rotate(if (expanded) 180f else 0f), tint = CedarColors.secondaryText)
        }
        if (expanded) {
            SelectionContainer {
                Text(text, style = MaterialTheme.typography.bodySmall, color = CedarColors.secondaryText, modifier = Modifier.padding(top = 4.dp))
            }
        }
    }
}

/** A bordered box for plans, approvals and questions. */
@Composable
private fun Panel(tint: androidx.compose.ui.graphics.Color? = null, content: @Composable () -> Unit) {
    val shape = RoundedCornerShape(12.dp)
    Column(
        Modifier
            .fillMaxWidth()
            .clip(shape)
            .background(tint?.copy(alpha = 0.12f) ?: MaterialTheme.colorScheme.surfaceContainer)
            .border(1.dp, tint?.copy(alpha = 0.5f) ?: MaterialTheme.colorScheme.outlineVariant.copy(alpha = 0.6f), shape)
            .padding(12.dp),
        verticalArrangement = Arrangement.spacedBy(8.dp),
    ) { content() }
}

@Composable
private fun PanelTitle(icon: ImageVector, text: String) {
    Row(verticalAlignment = Alignment.CenterVertically) {
        Icon(icon, null, Modifier.size(18.dp))
        Spacer(Modifier.width(6.dp))
        Text(text, style = MaterialTheme.typography.titleSmall)
    }
}

/** The steps the agent proposed; "Go ahead" when it waits for them. */
@Composable
private fun PlanView(plan: Plan, canAgree: Boolean, agree: () -> Unit) {
    var sent by remember { mutableStateOf(false) }
    Panel {
        PanelTitle(
            Icons.AutoMirrored.Outlined.ListAlt,
            when {
                plan.awaiting && plan.isProposed -> "Cedar planned this, waiting for your go-ahead"
                plan.awaiting -> "Plan, waiting for your go-ahead"
                plan.by == "timeout" -> "Plan (followed as proposed)"
                plan.isProposed -> "Cedar's plan"
                else -> "Plan"
            },
        )
        plan.steps.forEachIndexed { index, step ->
            Row {
                Text("${index + 1}.", style = MaterialTheme.typography.bodyMedium, color = CedarColors.secondaryText)
                Spacer(Modifier.width(8.dp))
                Text(step.title, style = MaterialTheme.typography.bodyMedium)
            }
        }
        if (plan.awaiting && canAgree) {
            Button(onClick = { sent = true; agree() }, enabled = !sent) {
                Text(if (sent) "Sent" else if (plan.isProposed) "Go on" else "Go ahead")
            }
        }
    }
}

/** A tool call waiting for the user's say-so, with what it would do. */
@OptIn(ExperimentalLayoutApi::class)
@Composable
private fun ApprovalView(approval: Approval, canDecide: Boolean, decide: (String) -> Unit) {
    Panel(tint = if (approval.isPending) CedarColors.orange else CedarColors.orange.copy(alpha = 0.4f)) {
        PanelTitle(Icons.Outlined.BackHand, if (approval.isPending) "May the agent do this?" else "Asked: ${approval.reason}")
        if (approval.isPending) Text(approval.reason, style = MaterialTheme.typography.bodyMedium)
        val summary = when (approval.tool) {
            "bash" -> approval.input["command"].str
            "run_python" -> approval.input["code"].str
            "download_file" -> approval.input["url"].str
            else -> null
        } ?: approval.input.pretty()
        CodeBox(approval.tool, summary)
        if (approval.isPending) {
            if (canDecide) {
                FlowRow(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    Button(onClick = { decide("allow") }) { Text("Allow") }
                    OutlinedButton(onClick = { decide("allow_always") }) { Text(approval.alwaysLabel) }
                    OutlinedButton(onClick = { decide("deny") }) { Text("Deny", color = CedarColors.red) }
                }
            }
        } else {
            val text = when (approval.decision) {
                "allow" -> "Allowed"
                "allow_always" -> "Allowed, and remembered"
                "deny" -> "Denied"
                "timeout" -> "No answer in time; not done"
                else -> approval.decision.orEmpty()
            }
            Text(text, style = MaterialTheme.typography.bodySmall, color = CedarColors.secondaryText)
        }
    }
}

/** The clarifying questions the agent asked, as a form whose answer is the next message. */
@Composable
fun QuestionsView(questions: Questions, submit: (List<ClarifyAnswer.Answer>) -> Unit) {
    val answers = remember(questions) { mutableStateListOf(*Array(questions.questions.size) { ClarifyAnswer.Answer() }) }
    var sent by remember(questions) { mutableStateOf(false) }
    Panel {
        PanelTitle(Icons.AutoMirrored.Outlined.HelpOutline, "Cedar asks")
        questions.questions.forEachIndexed { index, question ->
            Column(verticalArrangement = Arrangement.spacedBy(6.dp)) {
                question.header?.takeIf { it.isNotEmpty() }?.let {
                    Text(it.uppercase(), style = MaterialTheme.typography.labelSmall, fontWeight = FontWeight.SemiBold, color = CedarColors.secondaryText)
                }
                Text(question.question, style = MaterialTheme.typography.bodyMedium)
                question.options.forEach { option ->
                    val chosen = option.label in answers[index].chosen
                    Row(
                        Modifier.fillMaxWidth().clickable {
                            val current = answers[index]
                            answers[index] = if (question.multiSelect) {
                                current.copy(chosen = if (chosen) current.chosen - option.label else current.chosen + option.label)
                            } else {
                                current.copy(chosen = if (current.chosen == listOf(option.label)) emptyList() else listOf(option.label))
                            }
                        }.padding(vertical = 4.dp),
                        verticalAlignment = Alignment.Top,
                    ) {
                        val icon = when {
                            question.multiSelect && chosen -> Icons.Outlined.CheckBox
                            question.multiSelect -> Icons.Outlined.CheckBoxOutlineBlank
                            chosen -> Icons.Outlined.RadioButtonChecked
                            else -> Icons.Outlined.RadioButtonUnchecked
                        }
                        Icon(icon, null, Modifier.size(20.dp), tint = if (chosen) MaterialTheme.colorScheme.primary else CedarColors.secondaryText)
                        Spacer(Modifier.width(8.dp))
                        Column {
                            Text(option.label, style = MaterialTheme.typography.bodyMedium)
                            option.description?.takeIf { it.isNotEmpty() }?.let {
                                Text(it, style = MaterialTheme.typography.bodySmall, color = CedarColors.secondaryText)
                            }
                        }
                    }
                }
                OutlinedTextField(
                    value = answers[index].own,
                    onValueChange = { answers[index] = answers[index].copy(own = it) },
                    modifier = Modifier.fillMaxWidth(),
                    placeholder = { Text(if (question.options.isEmpty()) "Your answer" else "Other") },
                    maxLines = 4,
                )
            }
        }
        questions.note?.takeIf { it.isNotEmpty() }?.let { Text(it, style = MaterialTheme.typography.bodySmall, color = CedarColors.secondaryText) }
        Button(onClick = { sent = true; submit(answers.toList()) }, enabled = !sent) { Text(if (sent) "Sent" else "Answer") }
    }
}

/**
 * A collapsible tool call: a one-line summary, and the input and output when expanded. The card a
 * card tool draws comes after it; when its result can't be drawn, this offers the web UI.
 */
@Composable
fun ToolCallView(call: ToolCall, webURL: String?) {
    val context = LocalContext.current
    var expanded by rememberSaveable(call.id) { mutableStateOf(false) }
    val shape = RoundedCornerShape(12.dp)
    Column(
        Modifier
            .fillMaxWidth()
            .clip(shape)
            .background(MaterialTheme.colorScheme.surfaceContainer)
            .border(1.dp, MaterialTheme.colorScheme.outlineVariant.copy(alpha = 0.6f), shape)
            .animateContentSize(),
    ) {
        Row(
            Modifier.fillMaxWidth().clickable { expanded = !expanded }.padding(horizontal = 12.dp, vertical = 10.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Icon(toolIcon(call.name), null, Modifier.size(18.dp), tint = CedarColors.secondaryText)
            Spacer(Modifier.width(8.dp))
            Text(call.name, style = MaterialTheme.typography.titleSmall)
            Spacer(Modifier.width(8.dp))
            Text(
                call.summary,
                Modifier.weight(1f),
                style = MaterialTheme.typography.bodySmall.let { if (call.isCard) it else it.copy(fontFamily = FontFamily.Monospace) },
                color = CedarColors.secondaryText,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
            )
            Spacer(Modifier.width(4.dp))
            when {
                !call.isFinished -> CircularProgressIndicator(Modifier.size(14.dp), strokeWidth = 2.dp)
                call.isError -> Icon(Icons.Outlined.Error, "Failed", Modifier.size(16.dp), tint = CedarColors.red)
                else -> Icon(Icons.Outlined.CheckCircle, "Done", Modifier.size(16.dp), tint = CedarColors.green)
            }
            Icon(Icons.Outlined.ChevronRight, null, Modifier.size(18.dp).rotate(if (expanded) 90f else 0f), tint = CedarColors.secondaryText)
        }
        if (expanded) {
            HorizontalDivider()
            Column(Modifier.padding(12.dp), verticalArrangement = Arrangement.spacedBy(10.dp)) {
                // A chart that can't be drawn is still shown as the picture it saved.
                if (call.isCard && call.name != "generate_chart" && call.isFinished && !call.isError && call.card == null && webURL != null) {
                    Row(Modifier.clickable { openUrl(context, webURL) }, verticalAlignment = Alignment.CenterVertically) {
                        Icon(Icons.Outlined.OpenInBrowser, null, Modifier.size(16.dp), tint = MaterialTheme.colorScheme.primary)
                        Spacer(Modifier.width(6.dp))
                        Text("Open the ${cardKind(call.name)} on the web", color = MaterialTheme.colorScheme.primary, style = MaterialTheme.typography.bodyMedium)
                    }
                }
                CodeBox("Input", inputText(call))
                call.output?.let { output ->
                    // Charts the tool printed are drawn under it, so they're left out here.
                    val text = if (call.charts.isEmpty()) output else ChartCard.inOutput(output).text
                    CodeBox(if (call.isError) "Error" else "Output", text.ifEmpty { "(no output)" }, isError = call.isError)
                }
                if (call.sources.isNotEmpty()) {
                    Column(verticalArrangement = Arrangement.spacedBy(2.dp)) {
                        call.sources.forEach { source ->
                            Text(
                                "[${source.n}] ${source.title ?: source.url}",
                                style = MaterialTheme.typography.bodySmall,
                                color = MaterialTheme.colorScheme.primary,
                                maxLines = 1,
                                overflow = TextOverflow.Ellipsis,
                                modifier = Modifier.clickable { openUrl(context, source.url) },
                            )
                        }
                    }
                }
                call.durationMs?.let { ms ->
                    Text(if (ms < 1000) "$ms ms" else "${"%.1f".format(ms / 1000.0)} s", style = MaterialTheme.typography.labelSmall, color = CedarColors.secondaryText)
                }
            }
        }
    }
}

private fun inputText(call: ToolCall): String = when (call.name) {
    "bash" -> call.input["command"].str ?: call.input.pretty()
    "run_python" -> call.input["code"].str ?: call.input.pretty()
    "write_file" -> "${call.input["path"].str.orEmpty()}\n\n${call.input["content"].str.orEmpty()}"
    else -> call.input.pretty()
}

private fun cardKind(name: String): String = when (name) {
    "show_map" -> "map"
    "show_recipe" -> "recipe"
    "plan_trip" -> "route"
    "compare_stocks" -> "comparison"
    "search_products" -> "products"
    "generate_chart" -> "chart"
    else -> "result"
}

private fun toolIcon(name: String): ImageVector = when (name) {
    "bash" -> Icons.Outlined.Terminal
    "run_python" -> Icons.Outlined.Code
    "read_file", "read_document", "generate_pdf" -> Icons.Outlined.Description
    "write_file" -> Icons.Outlined.EditNote
    "web_search" -> Icons.Outlined.Search
    "fetch_url", "download_file", "browser" -> Icons.Outlined.Language
    "generate_chart" -> Icons.Outlined.BarChart
    "weather_forecast" -> Icons.Outlined.Cloud
    "show_map" -> Icons.Outlined.Map
    "show_recipe" -> Icons.Outlined.Restaurant
    "plan_trip" -> Icons.Outlined.Train
    "compare_stocks" -> Icons.Outlined.ShowChart
    "search_products" -> Icons.Outlined.ShoppingBag
    "ask_user" -> Icons.AutoMirrored.Outlined.HelpOutline
    else -> Icons.Outlined.Build
}

/** Monospaced, selectable, horizontally scrolling text with a copy button. */
@Composable
fun CodeBox(label: String?, text: String, isError: Boolean = false) {
    val context = LocalContext.current
    // Very long outputs are cut for display; the copy button still copies everything.
    val limit = 20_000
    val shown = if (text.length > limit) text.take(limit) + "\n… (${text.length - limit} more characters)" else text
    Column(Modifier.fillMaxWidth().clip(RoundedCornerShape(8.dp)).background(MaterialTheme.colorScheme.surfaceContainerHighest)) {
        Row(Modifier.fillMaxWidth().padding(start = 10.dp, end = 6.dp, top = 6.dp), verticalAlignment = Alignment.CenterVertically) {
            if (label != null) {
                Text(label, style = MaterialTheme.typography.labelMedium, color = if (isError) CedarColors.red else CedarColors.secondaryText)
            }
            Spacer(Modifier.weight(1f))
            Icon(Icons.Outlined.ContentCopy, "Copy", Modifier.size(16.dp).clickable { copyText(context, text) }, tint = CedarColors.secondaryText)
        }
        SelectionContainer {
            Text(
                shown,
                Modifier.horizontalScroll(rememberScrollState()).padding(10.dp),
                style = MaterialTheme.typography.bodySmall.copy(fontFamily = FontFamily.Monospace),
                softWrap = false,
            )
        }
    }
}
