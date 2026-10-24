package dev.cedar.core

import java.util.UUID

/**
 * A file picked to go with the next message. PDFs go with any model (cedar saves them in the run's
 * workspace for the agent to read); images only while the model takes them. The limits mirror
 * cedar-chat's (`clean_attachments` in chat/src/api.rs) and iOS/Cedar/Models/Attachments.swift.
 * Re-encoding an image the server doesn't take is the app's job (it needs Android's bitmaps).
 */
class OutgoingAttachment(
    val name: String,
    val mediaType: String,
    val data: ByteArray,
    val id: String = UUID.randomUUID().toString(),
) {
    val isImage: Boolean get() = mediaType != PDF
    val info: AttachmentInfo get() = AttachmentInfo(name, mediaType, data.size)

    override fun equals(other: Any?) = other is OutgoingAttachment && other.id == id
    override fun hashCode() = id.hashCode()

    companion object {
        val TYPES = listOf("image/png", "image/jpeg", "image/gif", "image/webp", "application/pdf")
        const val PDF = "application/pdf"
        const val MAX_FILES = 10
        const val MAX_BYTES = 20 shl 20
        const val MAX_TOTAL_BYTES = 32 shl 20

        /**
         * Photos that have to be re-encoded (HEIC, for one) are scaled down to this longer side on the
         * way; cedar scales them further to its own setting.
         */
        const val MAX_CONVERTED_SIDE = 4096

        /** Whether the server takes `mime` as it is. */
        fun accepts(mime: String?): Boolean = mime != null && mime.lowercase() in TYPES

        /** The JPEG name a re-encoded image goes by: `IMG_1.heic` becomes `IMG_1.jpg`. */
        fun jpegName(name: String): String {
            val dot = name.lastIndexOf('.')
            val base = if (dot > 0) name.substring(0, dot) else if (dot == 0) "" else name
            return (base.ifEmpty { "photo" }) + ".jpg"
        }

        /** Why `file` can't join `existing`, or null when it can. */
        fun problem(file: OutgoingAttachment, existing: List<OutgoingAttachment>, imagesAllowed: Boolean): String? {
            if (file.isImage && !imagesAllowed) return "${file.name}: this model takes PDFs but not images"
            if (file.data.isEmpty()) return "${file.name} is empty"
            if (file.data.size > MAX_BYTES) return "${file.name} is larger than ${format(MAX_BYTES.toLong())}"
            if (existing.size >= MAX_FILES) return "At most $MAX_FILES files per message"
            if (existing.sumOf { it.data.size.toLong() } + file.data.size > MAX_TOTAL_BYTES) {
                return "The files would add up to more than ${format(MAX_TOTAL_BYTES.toLong())}"
            }
            return null
        }

        /** A size as people read it: "20 MB" (decimal, as Apple's file style counts). */
        fun format(bytes: Long): String = when {
            bytes >= 1_000_000_000 -> "${trim(bytes / 1e9)} GB"
            bytes >= 1_000_000 -> "${trim(bytes / 1e6)} MB"
            bytes >= 1_000 -> "${trim(bytes / 1e3)} KB"
            else -> "$bytes bytes"
        }

        private fun trim(v: Double): String = if (v >= 100 || v == Math.floor(v)) Math.round(v).toString() else CardParse.fixed(v, 1)
    }
}
