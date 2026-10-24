package dev.cedar.cedar.ui

import android.content.Context
import android.content.Intent
import android.graphics.BitmapFactory
import android.util.LruCache
import android.webkit.MimeTypeMap
import androidx.compose.foundation.Image
import androidx.compose.foundation.border
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.outlined.BrokenImage
import androidx.compose.material.icons.outlined.Share
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.produceState
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.ImageBitmap
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.core.content.FileProvider
import dev.cedar.core.ChatClient
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import java.io.File

/** The images a response produced, loaded from the conversation's workspace with the access token. */
@Composable
fun ImagesView(paths: List<String>, conversationId: String, client: ChatClient) {
    Column(verticalArrangement = Arrangement.spacedBy(10.dp)) {
        paths.forEach { path -> WorkspaceImage(client, conversationId, path) }
    }
}

private sealed interface Loaded {
    data object Loading : Loaded
    data object Failed : Loaded
    class Done(val image: ImageBitmap, val bytes: ByteArray) : Loaded
}

@Composable
private fun WorkspaceImage(client: ChatClient, conversationId: String, path: String) {
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    val key = "${client.baseURL}|$conversationId|$path"
    val state by produceState<Loaded>(ImageCache.get(key) ?: Loaded.Loading, key) {
        if (value is Loaded.Done) return@produceState
        value = try {
            val bytes = client.file(conversationId, path)
            if (bytes.size > ImageCache.MAX_BYTES) {
                Loaded.Failed
            } else {
                val bitmap = withContext(Dispatchers.Default) { BitmapFactory.decodeByteArray(bytes, 0, bytes.size) }
                bitmap?.let { Loaded.Done(it.asImageBitmap(), bytes).also { done -> ImageCache.put(key, done) } } ?: Loaded.Failed
            }
        } catch (e: kotlinx.coroutines.CancellationException) {
            throw e
        } catch (_: Exception) {
            Loaded.Failed
        }
    }
    Column(verticalArrangement = Arrangement.spacedBy(4.dp)) {
        when (val s = state) {
            is Loaded.Done -> {
                val shape = RoundedCornerShape(10.dp)
                Image(
                    s.image, path,
                    Modifier.fillMaxWidth().heightIn(max = 420.dp).clip(shape).border(1.dp, MaterialTheme.colorScheme.outlineVariant, shape),
                    contentScale = ContentScale.Fit,
                )
                Row(verticalAlignment = Alignment.CenterVertically) {
                    Text(path, Modifier.weight(1f), style = MaterialTheme.typography.bodySmall, color = CedarColors.secondaryText,
                        maxLines = 1, overflow = TextOverflow.Ellipsis)
                    IconButton(onClick = { scope.launch { share(context, path, s.bytes) } }) {
                        Icon(Icons.Outlined.Share, "Share $path", Modifier.size(18.dp))
                    }
                }
            }
            Loaded.Failed -> Row(verticalAlignment = Alignment.CenterVertically) {
                Icon(Icons.Outlined.BrokenImage, null, Modifier.size(16.dp), tint = CedarColors.secondaryText)
                Text(" Could not load $path", style = MaterialTheme.typography.bodySmall, color = CedarColors.secondaryText)
            }
            Loaded.Loading -> Loading()
        }
    }
}

/** Hands the image to the Android share sheet, through a cache file the FileProvider serves. */
private suspend fun share(context: Context, path: String, bytes: ByteArray) {
    val file = withContext(Dispatchers.IO) {
        val dir = File(context.cacheDir, "shared").apply { mkdirs() }
        File(dir, path.substringAfterLast('/')).apply { writeBytes(bytes) }
    }
    val uri = FileProvider.getUriForFile(context, "${context.packageName}.files", file)
    val type = MimeTypeMap.getSingleton().getMimeTypeFromExtension(file.extension.lowercase()) ?: "image/*"
    val send = Intent(Intent.ACTION_SEND).setType(type).putExtra(Intent.EXTRA_STREAM, uri).addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)
    context.startActivity(Intent.createChooser(send, path).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK))
}

/** Keeps the images of this session, so leaving and reopening a conversation doesn't reload them. */
private object ImageCache {
    /** Larger files are left to the web UI's files panel. */
    const val MAX_BYTES = 16 * 1024 * 1024
    private val cache = LruCache<String, Loaded.Done>(40)

    fun get(key: String): Loaded.Done? = cache.get(key)
    fun put(key: String, value: Loaded.Done) {
        cache.put(key, value)
    }
}
