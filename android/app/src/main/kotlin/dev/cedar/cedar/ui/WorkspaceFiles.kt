package dev.cedar.cedar.ui

import android.content.Context
import android.content.Intent
import android.graphics.Bitmap
import android.graphics.BitmapFactory
import android.graphics.Color
import android.graphics.pdf.PdfRenderer
import android.os.ParcelFileDescriptor
import android.webkit.MimeTypeMap
import androidx.activity.compose.BackHandler
import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.gestures.awaitEachGesture
import androidx.compose.foundation.gestures.awaitFirstDown
import androidx.compose.foundation.gestures.calculateCentroid
import androidx.compose.foundation.gestures.calculatePan
import androidx.compose.foundation.gestures.calculateZoom
import androidx.compose.foundation.gestures.detectTapGestures
import androidx.compose.foundation.gestures.detectTransformGestures
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.aspectRatio
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.ArrowBack
import androidx.compose.material.icons.filled.Close
import androidx.compose.material.icons.outlined.Description
import androidx.compose.material.icons.outlined.FolderOpen
import androidx.compose.material.icons.outlined.Image
import androidx.compose.material.icons.outlined.InsertDriveFile
import androidx.compose.material.icons.outlined.PictureAsPdf
import androidx.compose.material.icons.outlined.Refresh
import androidx.compose.material.icons.outlined.Share
import androidx.compose.material.icons.outlined.WarningAmber
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.TopAppBar
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableFloatStateOf
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.ImageBitmap
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.input.pointer.PointerEventPass
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.window.Dialog
import androidx.compose.ui.window.DialogProperties
import androidx.core.content.FileProvider
import dev.cedar.cedar.R
import dev.cedar.core.ChatClient
import dev.cedar.core.OutgoingAttachment
import dev.cedar.core.WorkspaceFile
import dev.cedar.core.WorkspaceFileKind
import dev.cedar.core.WorkspaceFiles
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.NonCancellable
import kotlinx.coroutines.awaitCancellation
import kotlinx.coroutines.currentCoroutineContext
import kotlinx.coroutines.isActive
import kotlinx.coroutines.withContext
import java.io.Closeable
import java.io.File
import java.util.UUID

/**
 * The conversation's workspace files, full screen, as the web UI's files panel lists them. Images
 * and PDFs open in the viewer; other files are left to the web UI.
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun WorkspaceFilesDialog(client: ChatClient, conversationId: String, onDismiss: () -> Unit) {
    val context = LocalContext.current
    var listing by remember { mutableStateOf<WorkspaceFiles?>(null) }
    var error by remember { mutableStateOf<String?>(null) }
    var reloads by remember { mutableIntStateOf(0) }
    var viewing by remember { mutableStateOf<String?>(null) }

    LaunchedEffect(reloads) {
        try {
            listing = client.files(conversationId)
            error = null
        } catch (e: kotlinx.coroutines.CancellationException) {
            throw e
        } catch (e: Exception) {
            if (listing == null) error = context.describe(e)
        }
    }

    Dialog(onDismissRequest = onDismiss, properties = DialogProperties(usePlatformDefaultWidth = false)) {
        Surface(Modifier.fillMaxSize()) {
            val path = viewing
            if (path != null) {
                BackHandler { viewing = null }
                WorkspaceFileViewer(client, conversationId, path, back = true) { viewing = null }
            } else Scaffold(
                topBar = {
                    TopAppBar(
                        title = { Text(stringResource(R.string.files)) },
                        navigationIcon = { IconButton(onClick = onDismiss) { Icon(Icons.Filled.Close, stringResource(R.string.close)) } },
                        actions = { IconButton(onClick = { reloads++ }) { Icon(Icons.Outlined.Refresh, stringResource(R.string.refresh)) } },
                    )
                },
            ) { padding ->
                val current = listing
                Box(Modifier.padding(padding).fillMaxSize()) {
                    when {
                        current != null && current.files.isEmpty() ->
                            EmptyState(Icons.Outlined.FolderOpen, stringResource(R.string.no_files_yet), stringResource(R.string.no_files_text))
                        current != null -> LazyColumn(Modifier.fillMaxSize()) {
                            items(current.files, key = { it.path }) { file ->
                                FileRow(file) { viewing = file.path }
                                HorizontalDivider(Modifier.padding(start = 56.dp))
                            }
                            if (current.truncated) {
                                item { ErrorLine(stringResource(R.string.files_truncated)) }
                            }
                        }
                        error != null -> Failure(stringResource(R.string.could_not_load_files), error.orEmpty()) { reloads++ }
                        else -> Loading(Modifier.padding(top = 40.dp))
                    }
                }
            }
        }
    }
}

@Composable
private fun FileRow(file: WorkspaceFile, open: () -> Unit) {
    val kind = WorkspaceFileKind.of(file.path)
    Row(
        Modifier.fillMaxWidth().then(if (kind != null) Modifier.clickable(onClick = open) else Modifier).padding(horizontal = 16.dp, vertical = 10.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Icon(
            when (kind) {
                WorkspaceFileKind.IMAGE -> Icons.Outlined.Image
                WorkspaceFileKind.PDF -> Icons.Outlined.PictureAsPdf
                null -> Icons.Outlined.InsertDriveFile
            },
            null, Modifier.size(24.dp), tint = CedarColors.secondaryText,
        )
        Spacer(Modifier.width(16.dp))
        Column(Modifier.weight(1f)) {
            Text(file.path, maxLines = 2, overflow = TextOverflow.Ellipsis, style = MaterialTheme.typography.bodyLarge,
                color = if (kind != null) MaterialTheme.colorScheme.onSurface else CedarColors.secondaryText)
            Text(OutgoingAttachment.format(file.size), style = MaterialTheme.typography.bodySmall, color = CedarColors.secondaryText)
        }
    }
}

/** The PDFs a response produced, each opening in the viewer. */
@Composable
fun DocumentsView(paths: List<String>, conversationId: String, client: ChatClient) {
    var viewing by remember { mutableStateOf<String?>(null) }
    Column(verticalArrangement = Arrangement.spacedBy(6.dp)) {
        paths.forEach { path ->
            Row(
                Modifier.clip(RoundedCornerShape(10.dp)).background(MaterialTheme.colorScheme.surfaceVariant).clickable { viewing = path }
                    .padding(horizontal = 12.dp, vertical = 8.dp),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                Icon(Icons.Outlined.Description, null, Modifier.size(18.dp), tint = MaterialTheme.colorScheme.primary)
                Spacer(Modifier.width(8.dp))
                Text(path, style = MaterialTheme.typography.bodyMedium, maxLines = 1, overflow = TextOverflow.Ellipsis)
            }
        }
    }
    viewing?.let { path -> WorkspaceFileDialog(client, conversationId, path) { viewing = null } }
}

/** One workspace file in the viewer, full screen. */
@Composable
fun WorkspaceFileDialog(client: ChatClient, conversationId: String, path: String, onDismiss: () -> Unit) {
    Dialog(onDismissRequest = onDismiss, properties = DialogProperties(usePlatformDefaultWidth = false)) {
        Surface(Modifier.fillMaxSize()) {
            WorkspaceFileViewer(client, conversationId, path, back = false, onClose = onDismiss)
        }
    }
}

private sealed interface Shown {
    data object Loading : Shown

    class Failed(val message: String) : Shown

    /** A file downloaded to [file] and opened. */
    sealed interface Loaded : Shown, Closeable {
        val file: File
    }

    class Picture(val image: ImageBitmap, override val file: File) : Loaded {
        override fun close() {}
    }

    class Pdf(val pages: PdfPages, override val file: File) : Loaded {
        override fun close() = pages.close()
    }
}

/**
 * A workspace image or PDF at full size, loaded with the access token, with sharing. The file is
 * kept in a cache folder of its own while it is shown: PdfRenderer reads from it, and a share hands
 * it over under its own name.
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
private fun WorkspaceFileViewer(client: ChatClient, conversationId: String, path: String, back: Boolean, onClose: () -> Unit) {
    val context = LocalContext.current
    val name = path.substringAfterLast('/')
    var attempt by remember { mutableIntStateOf(0) }
    var shown by remember { mutableStateOf<Shown>(Shown.Loading) }
    LaunchedEffect(path, attempt) {
        shown = Shown.Loading
        val result = load(context, client, conversationId, path)
        shown = result
        // Kept open while shown; closed when the viewer goes away or loads again.
        try {
            awaitCancellation()
        } finally {
            if (result is Shown.Loaded) {
                result.close()
                result.file.parentFile?.deleteRecursively()
            }
        }
    }

    Scaffold(
        topBar = {
            TopAppBar(
                title = { Text(name, maxLines = 1, overflow = TextOverflow.Ellipsis) },
                navigationIcon = {
                    IconButton(onClick = onClose) {
                        if (back) Icon(Icons.AutoMirrored.Filled.ArrowBack, stringResource(R.string.back))
                        else Icon(Icons.Filled.Close, stringResource(R.string.close))
                    }
                },
                actions = {
                    val s = shown
                    if (s is Shown.Loaded) {
                        IconButton(onClick = { share(context, path, s.file) }) { Icon(Icons.Outlined.Share, stringResource(R.string.share_file, name)) }
                    }
                },
            )
        },
    ) { padding ->
        Box(Modifier.padding(padding).fillMaxSize(), contentAlignment = Alignment.Center) {
            when (val s = shown) {
                Shown.Loading -> Loading()
                is Shown.Failed -> Failure(stringResource(R.string.could_not_load, name), s.message) { attempt++ }
                is Shown.Picture -> ZoomableImage(s.image, path)
                is Shown.Pdf -> PdfPagesView(s.pages)
            }
        }
    }
}

/** What couldn't be loaded and why, centred, with a button to try again. */
@Composable
private fun Failure(title: String, message: String, retry: () -> Unit) {
    Column(Modifier.fillMaxSize().padding(32.dp), verticalArrangement = Arrangement.Center, horizontalAlignment = Alignment.CenterHorizontally) {
        Icon(Icons.Outlined.WarningAmber, null, Modifier.size(40.dp), tint = CedarColors.secondaryText)
        Spacer(Modifier.height(12.dp))
        Text(title, style = MaterialTheme.typography.titleMedium, textAlign = TextAlign.Center)
        Spacer(Modifier.height(6.dp))
        Text(message, style = MaterialTheme.typography.bodyMedium, color = CedarColors.secondaryText, textAlign = TextAlign.Center)
        TextButton(onClick = retry) { Text(stringResource(R.string.try_again)) }
    }
}

/** Larger files are left to the web UI's files panel. */
private const val MAX_VIEWED_BYTES = 32 * 1024 * 1024

private suspend fun load(context: Context, client: ChatClient, conversationId: String, path: String): Shown {
    val kind = WorkspaceFileKind.of(path) ?: return Shown.Failed(context.getString(R.string.viewer_unsupported))
    val bytes = try {
        client.file(conversationId, path)
    } catch (e: kotlinx.coroutines.CancellationException) {
        throw e
    } catch (e: Exception) {
        return Shown.Failed(context.describe(e))
    }
    if (bytes.size > MAX_VIEWED_BYTES) {
        return Shown.Failed(context.getString(R.string.viewer_too_large, OutgoingAttachment.format(MAX_VIEWED_BYTES.toLong())))
    }
    // Not cancelled halfway, so an opened PDF is never dropped without being closed; see below.
    val shown = withContext(NonCancellable + Dispatchers.IO) {
        val dir = File(context.cacheDir, "shared/viewer/${UUID.randomUUID()}").apply { mkdirs() }
        val file = File(dir, path.substringAfterLast('/')).apply { writeBytes(bytes) }
        val shown = when (kind) {
            WorkspaceFileKind.IMAGE -> decodeImage(bytes)?.let { Shown.Picture(it.asImageBitmap(), file) }
                ?: Shown.Failed(context.getString(R.string.viewer_not_an_image))
            WorkspaceFileKind.PDF -> try {
                Shown.Pdf(PdfPages(file), file)
            } catch (_: Exception) {
                // A damaged or password-protected PDF.
                Shown.Failed(context.getString(R.string.viewer_not_a_pdf))
            }
        }
        if (shown is Shown.Failed) dir.deleteRecursively()
        shown
    }
    if (!currentCoroutineContext().isActive) {
        // The viewer went away meanwhile.
        if (shown is Shown.Loaded) {
            shown.close()
            shown.file.parentFile?.deleteRecursively()
        }
        throw kotlinx.coroutines.CancellationException()
    }
    return shown
}

/** The image in [bytes], halved until neither side is over 4096 pixels, which a canvas draws. */
private fun decodeImage(bytes: ByteArray): Bitmap? {
    val bounds = BitmapFactory.Options().apply { inJustDecodeBounds = true }
    BitmapFactory.decodeByteArray(bytes, 0, bytes.size, bounds)
    if (bounds.outWidth <= 0 || bounds.outHeight <= 0) return null
    var sample = 1
    while (maxOf(bounds.outWidth, bounds.outHeight) / sample > 4096) sample *= 2
    return BitmapFactory.decodeByteArray(bytes, 0, bytes.size, BitmapFactory.Options().apply { inSampleSize = sample })
}

/** Hands the file to the Android share sheet, through the FileProvider. */
private fun share(context: Context, path: String, file: File) {
    val uri = FileProvider.getUriForFile(context, "${context.packageName}.files", file)
    val type = MimeTypeMap.getSingleton().getMimeTypeFromExtension(file.extension.lowercase()) ?: "application/octet-stream"
    val send = Intent(Intent.ACTION_SEND).setType(type).putExtra(Intent.EXTRA_STREAM, uri).addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)
    context.startActivity(Intent.createChooser(send, path).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK))
}

/** An image fitted to the screen that pinches to zoom up to 6×, pans when zoomed, and zooms in or back on a double tap. */
@Composable
private fun ZoomableImage(image: ImageBitmap, description: String) {
    var scale by remember { mutableFloatStateOf(1f) }
    var offset by remember { mutableStateOf(Offset.Zero) }
    BoxWithConstraints(Modifier.fillMaxSize()) {
        val width = constraints.maxWidth.toFloat()
        val height = constraints.maxHeight.toFloat()
        fun clamp(o: Offset, s: Float): Offset {
            val maxX = width * (s - 1) / 2
            val maxY = height * (s - 1) / 2
            return Offset(o.x.coerceIn(-maxX, maxX), o.y.coerceIn(-maxY, maxY))
        }
        Image(
            image, description,
            Modifier.fillMaxSize()
                .pointerInput(width, height) {
                    detectTapGestures(onDoubleTap = { at ->
                        if (scale > 1f) {
                            scale = 1f
                            offset = Offset.Zero
                        } else {
                            scale = 3f
                            offset = clamp((Offset(width / 2, height / 2) - at) * 2f, 3f)
                        }
                    })
                }
                .pointerInput(width, height) {
                    detectTransformGestures { centroid, pan, zoom, _ ->
                        val next = (scale * zoom).coerceIn(1f, 6f)
                        // Keep the point under the fingers where it is.
                        val center = Offset(width / 2, height / 2)
                        val moved = (offset + center - centroid) * (next / scale) + centroid - center + pan
                        scale = next
                        offset = clamp(moved, next)
                    }
                }
                .graphicsLayer(scaleX = scale, scaleY = scale, translationX = offset.x, translationY = offset.y),
            contentScale = ContentScale.Fit,
        )
    }
}

/**
 * A PDF opened for drawing. PdfRenderer draws one page at a time, so every use of it holds the
 * lock; [close] waits for a page being drawn.
 */
private class PdfPages(file: File) : Closeable {
    private val descriptor = ParcelFileDescriptor.open(file, ParcelFileDescriptor.MODE_READ_ONLY)
    private val renderer: PdfRenderer
    private var closed = false

    /** Each page's height over its width. */
    val ratios: List<Float>

    init {
        var opened: PdfRenderer? = null
        try {
            val pdf = PdfRenderer(descriptor)
            opened = pdf
            ratios = (0 until pdf.pageCount).map { i -> pdf.openPage(i).use { it.height.toFloat() / it.width.coerceAtLeast(1) } }
            renderer = pdf
        } catch (e: Exception) {
            opened?.close()
            descriptor.close()
            throw e
        }
    }

    /** Page `index` drawn `width` pixels wide on white, or null once closed. */
    fun render(index: Int, width: Int): Bitmap? = synchronized(this) {
        if (closed) return null
        renderer.openPage(index).use { page ->
            val height = (width * page.height.toFloat() / page.width.coerceAtLeast(1)).toInt().coerceAtLeast(1)
            Bitmap.createBitmap(width, height, Bitmap.Config.ARGB_8888).also {
                it.eraseColor(Color.WHITE)
                page.render(it, null, null, PdfRenderer.Page.RENDER_MODE_FOR_DISPLAY)
            }
        }
    }

    override fun close() {
        synchronized(this) {
            if (closed) return
            closed = true
            renderer.close()
            descriptor.close()
        }
    }
}

/**
 * A PDF's pages, one under another and fitted to the width. A pinch zooms up to 4× around the
 * fingers; zoomed in, one finger scrolls both ways. Pages are drawn again, sharper, once a pinch ends.
 */
@Composable
private fun PdfPagesView(pages: PdfPages) {
    val listState = rememberLazyListState()
    val horizontal = rememberScrollState()
    var scale by remember { mutableFloatStateOf(1f) }
    // The zoom the pages are drawn for, which follows `scale` when a pinch ends.
    var drawnScale by remember { mutableFloatStateOf(1f) }
    BoxWithConstraints(
        Modifier.fillMaxSize().background(MaterialTheme.colorScheme.surfaceVariant)
            .pointerInput(Unit) {
                awaitEachGesture {
                    awaitFirstDown(requireUnconsumed = false, pass = PointerEventPass.Initial)
                    var pinched = false
                    do {
                        // Seen before the scrolling lists below, so two fingers zoom instead of scrolling.
                        val event = awaitPointerEvent(PointerEventPass.Initial)
                        if (event.changes.count { it.pressed } >= 2) {
                            pinched = true
                            val zoom = event.calculateZoom()
                            val centroid = event.calculateCentroid()
                            val next = (scale * zoom).coerceIn(1f, 4f)
                            val factor = next / scale
                            scale = next
                            // Keep the point under the fingers where it is, then follow their pan.
                            val pan = event.calculatePan()
                            horizontal.dispatchRawDelta((horizontal.value + centroid.x) * (factor - 1) - pan.x)
                            // The list keeps its first visible page where it is, so measure from that page's top.
                            listState.dispatchRawDelta((listState.firstVisibleItemScrollOffset + centroid.y) * (factor - 1) - pan.y)
                            event.changes.forEach { it.consume() }
                        }
                    } while (event.changes.any { it.pressed })
                    if (pinched) drawnScale = scale
                }
            },
    ) {
        val density = LocalDensity.current
        val pageWidth = maxWidth * scale
        // Sharp enough for the zoom, but no wider than 2048 pixels: a page takes width × height × 4 bytes.
        val drawWidth = (with(density) { (maxWidth * drawnScale).roundToPx() }).coerceIn(1, 2048)
        Box(Modifier.fillMaxSize().horizontalScroll(horizontal)) {
            LazyColumn(
                Modifier.width(pageWidth).fillMaxHeight(),
                state = listState,
                contentPadding = PaddingValues(vertical = 8.dp),
                verticalArrangement = Arrangement.spacedBy(8.dp),
            ) {
                items(pages.ratios.size) { index ->
                    PdfPage(pages, index, drawWidth)
                }
            }
        }
    }
}

@Composable
private fun PdfPage(pages: PdfPages, index: Int, drawWidth: Int) {
    val ratio = pages.ratios[index]
    var bitmap by remember { mutableStateOf<ImageBitmap?>(null) }
    LaunchedEffect(index, drawWidth) {
        // The page drawn for the previous zoom stays until the sharper one is ready.
        bitmap = withContext(Dispatchers.IO) { pages.render(index, drawWidth) }?.asImageBitmap() ?: bitmap
    }
    Box(Modifier.fillMaxWidth().aspectRatio(1f / ratio).background(androidx.compose.ui.graphics.Color.White)) {
        bitmap?.let { Image(it, null, Modifier.fillMaxSize(), contentScale = ContentScale.FillBounds) }
    }
}
