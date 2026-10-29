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
import androidx.compose.material.icons.outlined.Flight
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
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import dev.cedar.cedar.ChatModel
import dev.cedar.cedar.R
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
import dev.cedar.core.number
import dev.cedar.core.pretty
import dev.cedar.core.price
import dev.cedar.core.str
import kotlinx.coroutines.delay
import java.text.NumberFormat

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
        TurnStatus.QUEUED -> Working(stringResource(R.string.outcome_queued))
        TurnStatus.RUNNING -> Working(
            stringResource(
                when {
                    turn.pendingApproval != null -> R.string.waiting_approval
                    turn.awaitingPlan != null -> R.string.waiting_plan
                    else -> R.string.working
                },
            ),
        )
        TurnStatus.FAILED -> Row(verticalAlignment = Alignment.Top) {
            Icon(Icons.Outlined.WarningAmber, null, Modifier.size(16.dp), tint = CedarColors.red)
            Spacer(Modifier.width(6.dp))
            SelectionContainer { Text(turn.turn.error ?: stringResource(R.string.run_failed), style = small, color = CedarColors.red) }
        }
        TurnStatus.CANCELLED -> Row(verticalAlignment = Alignment.CenterVertically) {
            Icon(Icons.Outlined.StopCircle, null, Modifier.size(16.dp), tint = secondary)
            Spacer(Modifier.width(6.dp))
            Text(
                turn.cancelReason?.let { stringResource(R.string.stopped_reason, it) } ?: stringResource(R.string.outcome_stopped),
                style = small, color = secondary,
            )
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
                    stringResource(R.string.copy_answer),
                    Modifier.size(16.dp).clickable {
                        copyText(context, turn.answer)
                        copied = true
                    },
                    tint = secondary,
                )
                val usage = turn.usage
                if (usage.inputTokens != 0 || usage.outputTokens != 0 || usage.costUSD != null) {
                    val n = NumberFormat.getIntegerInstance()
                    Text(
                        stringResource(R.string.tokens_in_out, n.format(usage.inputTokens), n.format(usage.outputTokens)),
                        style = MaterialTheme.typography.labelSmall, color = secondary,
                    )
                }
                usage.costUSD?.let { cost ->
                    // In US dollars, as cedar counts it, written as the user's locale writes prices.
                    val digits = if (cost < 0.01) 4 else 2
                    Text(price(cost, "USD", digits, digits), style = MaterialTheme.typography.labelSmall, color = secondary)
                }
                if (turn.turn.needsAnswer) {
                    Row(verticalAlignment = Alignment.CenterVertically) {
                        Icon(Icons.AutoMirrored.Outlined.HelpOutline, null, Modifier.size(14.dp), tint = CedarColors.orange)
                        Spacer(Modifier.width(3.dp))
                        Text(stringResource(R.string.needs_your_answer), style = MaterialTheme.typography.labelSmall, color = CedarColors.orange)
                    }
                } else if (turn.turn.stopReason == "max_tokens") {
                    Text(stringResource(R.string.cut_short), style = MaterialTheme.typography.labelSmall, color = secondary)
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
            val title = if (durationMs != null && durationMs > 0) {
                stringResource(R.string.thought_for, Math.round(durationMs / 1000.0))
            } else {
                stringResource(R.string.thought_it_through)
            }
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
            stringResource(
                when {
                    plan.awaiting && plan.isProposed -> R.string.plan_proposed_waiting
                    plan.awaiting -> R.string.plan_waiting
                    plan.by == "timeout" -> R.string.plan_timeout
                    plan.isProposed -> R.string.plan_cedars
                    else -> R.string.plan
                },
            ),
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
                Text(stringResource(if (sent) R.string.sent else if (plan.isProposed) R.string.go_on else R.string.go_ahead))
            }
        }
    }
}

/** A tool call waiting for the user's say-so, with what it would do. */
@OptIn(ExperimentalLayoutApi::class)
@Composable
private fun ApprovalView(approval: Approval, canDecide: Boolean, decide: (String) -> Unit) {
    Panel(tint = if (approval.isPending) CedarColors.orange else CedarColors.orange.copy(alpha = 0.4f)) {
        PanelTitle(
            Icons.Outlined.BackHand,
            if (approval.isPending) stringResource(R.string.approval_question) else stringResource(R.string.approval_asked, approval.reason),
        )
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
                    Button(onClick = { decide("allow") }) { Text(stringResource(R.string.allow)) }
                    OutlinedButton(onClick = { decide("allow_always") }) {
                        // What "always" would remember: the host, the packages.
                        val scope = approval.scope
                        Text(if (scope.isNullOrEmpty()) stringResource(R.string.approval_always) else stringResource(R.string.approval_always_for, scope))
                    }
                    OutlinedButton(onClick = { decide("deny") }) { Text(stringResource(R.string.deny), color = CedarColors.red) }
                }
            }
        } else {
            val text = when (approval.decision) {
                "allow" -> stringResource(R.string.allowed)
                "allow_always" -> stringResource(R.string.allowed_remembered)
                "deny" -> stringResource(R.string.denied)
                "timeout" -> stringResource(R.string.approval_timeout)
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
        PanelTitle(Icons.AutoMirrored.Outlined.HelpOutline, stringResource(R.string.cedar_asks))
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
                    placeholder = { Text(stringResource(if (question.options.isEmpty()) R.string.your_answer else R.string.other)) },
                    maxLines = 4,
                )
            }
        }
        questions.note?.takeIf { it.isNotEmpty() }?.let { Text(it, style = MaterialTheme.typography.bodySmall, color = CedarColors.secondaryText) }
        Button(onClick = { sent = true; submit(answers.toList()) }, enabled = !sent) { Text(stringResource(if (sent) R.string.sent else R.string.answer)) }
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
                call.isError -> Icon(Icons.Outlined.Error, stringResource(R.string.failed), Modifier.size(16.dp), tint = CedarColors.red)
                else -> Icon(Icons.Outlined.CheckCircle, stringResource(R.string.outcome_done), Modifier.size(16.dp), tint = CedarColors.green)
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
                        Text(stringResource(openCardOnWeb(call.name)), color = MaterialTheme.colorScheme.primary, style = MaterialTheme.typography.bodyMedium)
                    }
                }
                CodeBox(stringResource(R.string.input), inputText(call))
                call.output?.let { output ->
                    // Charts the tool printed are drawn under it, so they're left out here.
                    val text = if (call.charts.isEmpty()) output else ChartCard.inOutput(output).text
                    CodeBox(
                        stringResource(if (call.isError) R.string.error else R.string.output),
                        text.ifEmpty { stringResource(R.string.no_output) },
                        isError = call.isError,
                    )
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
                    Text(
                        if (ms < 1000) "$ms ms" else "${number(ms / 1000.0, 1, 1)} s",
                        style = MaterialTheme.typography.labelSmall, color = CedarColors.secondaryText,
                    )
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

/** "Open the map on the web", for a card the app can't draw. */
private fun openCardOnWeb(name: String): Int = when (name) {
    "show_map" -> R.string.open_map_on_web
    "show_recipe" -> R.string.open_recipe_on_web
    "plan_trip" -> R.string.open_route_on_web
    "compare_stocks" -> R.string.open_comparison_on_web
    "search_products" -> R.string.open_products_on_web
    "search_flights" -> R.string.open_flights_on_web
    "generate_chart" -> R.string.open_chart_on_web
    else -> R.string.open_result_on_web
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
    "search_flights" -> Icons.Outlined.Flight
    "ask_user" -> Icons.AutoMirrored.Outlined.HelpOutline
    else -> Icons.Outlined.Build
}

/** Monospaced, selectable, horizontally scrolling text with a copy button. */
@Composable
fun CodeBox(label: String?, text: String, isError: Boolean = false) {
    val context = LocalContext.current
    // Very long outputs are cut for display; the copy button still copies everything.
    val limit = 20_000
    val more = text.length - limit
    val shown = if (more > 0) text.take(limit) + "\n" + plural(R.plurals.more_characters, more, more) else text
    Column(Modifier.fillMaxWidth().clip(RoundedCornerShape(8.dp)).background(MaterialTheme.colorScheme.surfaceContainerHighest)) {
        Row(Modifier.fillMaxWidth().padding(start = 10.dp, end = 6.dp, top = 6.dp), verticalAlignment = Alignment.CenterVertically) {
            if (label != null) {
                Text(label, style = MaterialTheme.typography.labelMedium, color = if (isError) CedarColors.red else CedarColors.secondaryText)
            }
            Spacer(Modifier.weight(1f))
            Icon(Icons.Outlined.ContentCopy, stringResource(R.string.copy), Modifier.size(16.dp).clickable { copyText(context, text) }, tint = CedarColors.secondaryText)
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
