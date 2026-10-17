import SwiftUI
import UIKit

/// The images a response produced, loaded from the conversation's workspace with the access
/// token (plain `AsyncImage` can't send one).
struct ImagesView: View {
    let paths: [String]
    let conversationId: String
    let client: ChatClient

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            ForEach(paths, id: \.self) { path in
                WorkspaceImageView(request: client.fileRequest(conversationId: conversationId, path: path), caption: path)
            }
        }
    }
}

struct WorkspaceImageView: View {
    let request: URLRequest
    let caption: String
    @State private var image: UIImage?
    @State private var failed = false

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            if let image {
                Image(uiImage: image)
                    .resizable()
                    .scaledToFit()
                    .frame(maxHeight: 420)
                    .clipShape(RoundedRectangle(cornerRadius: 10))
                    .overlay(RoundedRectangle(cornerRadius: 10).strokeBorder(Color(.separator).opacity(0.5)))
                HStack {
                    Text(caption)
                        .font(.caption)
                        .foregroundStyle(.secondary)
                        .lineLimit(1)
                        .truncationMode(.middle)
                    Spacer()
                    ShareLink(item: Image(uiImage: image), preview: SharePreview(caption, image: Image(uiImage: image))) {
                        Image(systemName: "square.and.arrow.up")
                            .font(.caption)
                    }
                    .accessibilityLabel("Share \(caption)")
                }
            } else if failed {
                Label("Could not load \(caption)", systemImage: "photo.badge.exclamationmark")
                    .font(.caption)
                    .foregroundStyle(.secondary)
            } else {
                ProgressView()
                    .frame(maxWidth: .infinity, minHeight: 80)
            }
        }
        .task(id: request.url) { await load() }
    }

    private func load() async {
        guard let url = request.url else { failed = true; return }
        if let cached = await ImageCache.shared.image(for: url) {
            image = cached
            return
        }
        do {
            let (data, response) = try await URLSession.shared.data(for: request)
            guard let http = response as? HTTPURLResponse, (200..<300).contains(http.statusCode),
                  data.count <= ImageCache.maxBytes, let loaded = UIImage(data: data)
            else { failed = true; return }
            await ImageCache.shared.store(loaded, for: url)
            image = loaded
        } catch {
            failed = true
        }
    }
}

/// Keeps the images of this session, so leaving and reopening a conversation doesn't reload them.
actor ImageCache {
    static let shared = ImageCache()
    /// Larger files are left to the web UI's files panel.
    static let maxBytes = 16 * 1024 * 1024

    private var images: [URL: UIImage] = [:]
    private var order: [URL] = []
    private let limit = 40

    func image(for url: URL) -> UIImage? {
        images[url]
    }

    func store(_ image: UIImage, for url: URL) {
        if images[url] == nil {
            order.append(url)
            if order.count > limit, let oldest = order.first {
                order.removeFirst()
                images[oldest] = nil
            }
        }
        images[url] = image
    }
}
