import Foundation
import UniformTypeIdentifiers
import UIKit

/// A file picked to go with the next message. PDFs go with any model (cedar saves them in the
/// run's workspace for the agent to read); images only while the model takes them. The limits
/// mirror cedar-chat's (`clean_attachments` in chat/src/api.rs).
struct OutgoingAttachment: Identifiable, Hashable, Sendable {
    let id = UUID()
    let name: String
    let mediaType: String
    let data: Data

    static let types = ["image/png", "image/jpeg", "image/gif", "image/webp", "application/pdf"]
    static let pdf = "application/pdf"
    static let maxFiles = 10
    static let maxBytes = 20 << 20
    static let maxTotalBytes = 32 << 20
    /// Photos that have to be re-encoded (HEIC, for one) are scaled down to this longer side on
    /// the way; cedar scales them further to its own setting.
    static let maxConvertedSide: CGFloat = 4096

    var isImage: Bool { mediaType != Self.pdf }
    var info: AttachmentInfo { AttachmentInfo(name: name, mediaType: mediaType, size: data.count) }

    /// The file as it goes to the server, or nil when it is neither a PDF nor an image. Images
    /// in a type the server doesn't take (HEIC, TIFF, …) are re-encoded as JPEG.
    static func make(name: String, type: UTType?, data: Data) -> OutgoingAttachment? {
        guard let type else { return nil }
        if let mime = type.preferredMIMEType, types.contains(mime) {
            return OutgoingAttachment(name: name, mediaType: mime, data: data)
        }
        guard type.conforms(to: .image), let image = UIImage(data: data), let jpeg = jpeg(image) else { return nil }
        let base = (name as NSString).deletingPathExtension
        return OutgoingAttachment(name: (base.isEmpty ? "photo" : base) + ".jpg", mediaType: "image/jpeg", data: jpeg)
    }

    private static func jpeg(_ image: UIImage) -> Data? {
        let side = max(image.size.width, image.size.height)
        guard side > maxConvertedSide else { return image.jpegData(compressionQuality: 0.85) }
        let scale = maxConvertedSide / side
        let size = CGSize(width: (image.size.width * scale).rounded(), height: (image.size.height * scale).rounded())
        let format = UIGraphicsImageRendererFormat.default()
        format.scale = 1
        let resized = UIGraphicsImageRenderer(size: size, format: format).image { _ in
            image.draw(in: CGRect(origin: .zero, size: size))
        }
        return resized.jpegData(compressionQuality: 0.85)
    }

    /// Why `file` can't join `existing`, or nil when it can.
    static func problem(adding file: OutgoingAttachment, to existing: [OutgoingAttachment], imagesAllowed: Bool) -> String? {
        if file.isImage && !imagesAllowed { return L10n.format("%@: this model takes PDFs but not images", file.name) }
        if file.data.isEmpty { return L10n.format("%@ is empty", file.name) }
        if file.data.count > maxBytes { return L10n.format("%@ is larger than %@", file.name, format(maxBytes)) }
        if existing.count >= maxFiles { return L10n.format("At most %lld files per message", maxFiles) }
        if existing.reduce(0, { $0 + $1.data.count }) + file.data.count > maxTotalBytes {
            return L10n.format("The files would add up to more than %@", format(maxTotalBytes))
        }
        return nil
    }

    static func format(_ bytes: Int) -> String {
        Int64(bytes).formatted(.byteCount(style: .file).locale(L10n.locale))
    }
}
