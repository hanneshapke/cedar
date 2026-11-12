package dev.cedar.cedar.ui

import androidx.compose.animation.animateColorAsState
import androidx.compose.animation.core.animateDpAsState
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.safeDrawingPadding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.pager.HorizontalPager
import androidx.compose.foundation.pager.rememberPagerState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.alpha
import androidx.compose.ui.draw.clip
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.heading
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import dev.cedar.cedar.R
import kotlinx.coroutines.launch

/** A page's words and picture. */
private class TourPage(val title: Int, val text: Int, val illustration: TourIllustration)

private val PAGES = listOf(
    TourPage(R.string.tour_server_title, R.string.tour_server_text, TourIllustration.SERVER),
    TourPage(R.string.tour_work_title, R.string.tour_work_text, TourIllustration.WORK),
    TourPage(R.string.tour_cards_title, R.string.tour_cards_text, TourIllustration.CARDS),
    TourPage(R.string.tour_plans_title, R.string.tour_plans_text, TourIllustration.PLANS),
    TourPage(R.string.tour_agents_title, R.string.tour_agents_text, TourIllustration.AGENTS),
    TourPage(R.string.tour_memory_title, R.string.tour_memory_text, TourIllustration.MEMORY),
    TourPage(R.string.tour_connect_title, R.string.tour_connect_text, TourIllustration.CONNECT),
)

/**
 * The feature tour a new install sees once, after picking a language, as the iOS app's
 * OnboardingTourView: seven pages, each a picture of the app doing the thing, a title and two
 * lines. Swipe or Continue; Skip ends it early. Settings → About shows it again.
 */
@Composable
fun OnboardingTour(onDone: () -> Unit) {
    val pager = rememberPagerState(pageCount = { PAGES.size })
    val scope = rememberCoroutineScope()
    val isLast = pager.currentPage == PAGES.size - 1
    Box(Modifier.fillMaxSize().background(MaterialTheme.colorScheme.background).safeDrawingPadding(), contentAlignment = Alignment.TopCenter) {
        Column(Modifier.widthIn(max = 620.dp).fillMaxSize()) {
            Row(
                Modifier.fillMaxWidth().padding(start = 20.dp, end = 8.dp, top = 8.dp),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                CedarWordmark(18.sp)
                Spacer(Modifier.weight(1f))
                TextButton(onClick = onDone, enabled = !isLast, modifier = Modifier.alpha(if (isLast) 0f else 1f)) {
                    Text(stringResource(R.string.tour_skip))
                }
            }
            HorizontalPager(pager, Modifier.weight(1f).fillMaxWidth()) { index ->
                val page = PAGES[index]
                TourPageView(page, active = pager.currentPage == index)
            }
            PageDots(PAGES.size, pager.currentPage, Modifier.align(Alignment.CenterHorizontally).padding(bottom = 16.dp))
            ProminentButton(
                onClick = { if (isLast) onDone() else scope.launch { pager.animateScrollToPage(pager.currentPage + 1) } },
                modifier = Modifier.fillMaxWidth().padding(horizontal = 16.dp).padding(bottom = 16.dp),
            ) {
                Text(stringResource(if (isLast) R.string.tour_get_started else R.string.language_continue))
            }
        }
    }
}

/** One page: the picture above, the words below. */
@Composable
private fun TourPageView(page: TourPage, active: Boolean) {
    Column(Modifier.fillMaxSize()) {
        Box(Modifier.weight(1f).fillMaxWidth().padding(horizontal = 12.dp), contentAlignment = Alignment.Center) {
            // The page beside the one on screen is built ahead of time; its picture waits.
            page.illustration.Draw(shown = active)
        }
        Column(
            Modifier.fillMaxWidth().heightIn(min = 120.dp).padding(horizontal = 28.dp, vertical = 12.dp).semantics(mergeDescendants = true) {},
            horizontalAlignment = Alignment.CenterHorizontally,
            verticalArrangement = Arrangement.spacedBy(10.dp),
        ) {
            Text(
                stringResource(page.title),
                style = MaterialTheme.typography.headlineSmall,
                fontWeight = FontWeight.Bold,
                textAlign = TextAlign.Center,
                modifier = Modifier.semantics { heading() },
            )
            Text(
                stringResource(page.text),
                style = MaterialTheme.typography.bodyLarge,
                color = CedarColors.secondaryText,
                textAlign = TextAlign.Center,
                modifier = Modifier.widthIn(max = 420.dp),
            )
        }
    }
}

/** The dots under the pages: the current one long and green. */
@Composable
private fun PageDots(count: Int, index: Int, modifier: Modifier = Modifier) {
    val label = stringResource(R.string.tour_page_of, index + 1, count)
    Row(modifier.semantics { contentDescription = label }, horizontalArrangement = Arrangement.spacedBy(6.dp)) {
        repeat(count) { i ->
            val width by animateDpAsState(if (i == index) 22.dp else 7.dp, label = "dot")
            val color by animateColorAsState(if (i == index) MaterialTheme.colorScheme.primary else MaterialTheme.colorScheme.outlineVariant, label = "dotColor")
            Box(Modifier.size(width, 7.dp).clip(RoundedCornerShape(50)).background(color))
        }
    }
}
