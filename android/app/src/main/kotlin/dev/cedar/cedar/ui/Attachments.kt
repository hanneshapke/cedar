package dev.cedar.cedar.ui

import android.content.Context
import android.graphics.Bitmap
import android.graphics.BitmapFactory
import android.net.Uri
import android.provider.OpenableColumns
import android.webkit.MimeTypeMap
import dev.cedar.cedar.R
import dev.cedar.core.OutgoingAttachment
import java.io.ByteArrayOutputStream
import java.io.IOException
import kotlin.math.max
import kotlin.math.roundToInt

/**
 * Reads a picked photo or document into an [OutgoingAttachment], as iOS's `OutgoingAttachment.make`:
 * a type the server takes is sent as it is; any other image (HEIC, …) is re-encoded as JPEG, at most
 * 4096 px on the longer side.
 */
object Attachments {
    /** The file's display name, if the provider gives one. */
    fun name(context: Context, uri: Uri): String? =
        context.contentResolver.query(uri, arrayOf(OpenableColumns.DISPLAY_NAME), null, null, null)?.use { cursor ->
            if (cursor.moveToFirst()) cursor.getString(0) else null
        }

    /** The attachment, or null when it is neither a PDF nor an image. Runs off the main thread. */
    fun read(context: Context, uri: Uri, index: Int, count: Int): OutgoingAttachment? {
        val resolver = context.contentResolver
        val mime = resolver.getType(uri)?.lowercase()
            ?: name(context, uri)?.substringAfterLast('.', "")?.let { MimeTypeMap.getSingleton().getMimeTypeFromExtension(it.lowercase()) }
        val ext = mime?.let { MimeTypeMap.getSingleton().getExtensionFromMimeType(it) } ?: "jpg"
        val name = name(context, uri) ?: if (count == 1) "photo.$ext" else "photo-${index + 1}.$ext"
        val data = resolver.openInputStream(uri)?.use { input ->
            // Read no more than the server takes, plus one byte to tell that it's too big.
            val out = ByteArrayOutputStream()
            val buffer = ByteArray(64 * 1024)
            var total = 0
            while (true) {
                val n = input.read(buffer)
                if (n < 0) break
                out.write(buffer, 0, n)
                total += n
                if (total > OutgoingAttachment.MAX_BYTES && OutgoingAttachment.accepts(mime)) break
            }
            out.toByteArray()
        } ?: throw IOException(context.getString(R.string.file_could_not_be_read, name))
        if (OutgoingAttachment.accepts(mime)) return OutgoingAttachment(name, mime!!, data)
        if (mime?.startsWith("image/") != true) return null
        val jpeg = jpeg(data) ?: return null
        return OutgoingAttachment(OutgoingAttachment.jpegName(name), "image/jpeg", jpeg)
    }

    private fun jpeg(data: ByteArray): ByteArray? = try {
        val bounds = BitmapFactory.Options().apply { inJustDecodeBounds = true }
        BitmapFactory.decodeByteArray(data, 0, data.size, bounds)
        val side = max(bounds.outWidth, bounds.outHeight)
        // Decode at a power-of-two fraction close to the limit, then scale the rest of the way.
        var sample = 1
        while (side / (sample * 2) >= OutgoingAttachment.MAX_CONVERTED_SIDE) sample *= 2
        val decoded = BitmapFactory.decodeByteArray(data, 0, data.size, BitmapFactory.Options().apply { inSampleSize = sample })
        val longer = max(decoded.width, decoded.height)
        val bitmap = if (longer > OutgoingAttachment.MAX_CONVERTED_SIDE) {
            val scale = OutgoingAttachment.MAX_CONVERTED_SIDE.toFloat() / longer
            Bitmap.createScaledBitmap(decoded, (decoded.width * scale).roundToInt(), (decoded.height * scale).roundToInt(), true)
        } else decoded
        ByteArrayOutputStream().use { out ->
            bitmap.compress(Bitmap.CompressFormat.JPEG, 85, out)
            out.toByteArray()
        }
    } catch (_: Exception) {
        null
    }
}
