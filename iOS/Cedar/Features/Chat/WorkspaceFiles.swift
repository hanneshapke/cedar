import PDFKit
import SwiftUI
import UIKit

/// The conversation's workspace files, as the web UI's files panel lists them. Images and PDFs
/// open in the viewer; other files are left to the web UI.
struct WorkspaceFilesView: View {
    let conversationId: String
    let client: ChatClient
    @Environment(\.dismiss) private var dismiss
    @State private var listing: WorkspaceFiles?
    @State private var error: String?

    var body: some View {
        NavigationStack {
            content
                .navigationTitle("Files")
                .navigationBarTitleDisplayMode(.inline)
                .navigationDestination(for: String.self) { path in
                    WorkspaceFileViewer(path: path, request: client.fileRequest(conversationId: conversationId, path: path))
                }
                .toolbar {
                    ToolbarItem(placement: .topBarLeading) {
                        Button {
                            Task { await load() }
                        } label: {
                            Label("Refresh", systemImage: "arrow.clockwise")
                        }
                    }
                    ToolbarItem(placement: .topBarTrailing) {
                        Button("Done") { dismiss() }
                    }
                }
        }
        .task { await load() }
    }

    @ViewBuilder
    private var content: some View {
        if let listing {
            if listing.files.isEmpty {
                ContentUnavailableView("No Files Yet", systemImage: "folder",
                                       description: Text("Files the agent writes in this conversation appear here."))
            } else {
                List {
                    Section {
                        ForEach(listing.files) { file in
                            if WorkspaceFileKind(path: file.path) != nil {
                                NavigationLink(value: file.path) { WorkspaceFileRow(file: file) }
                            } else {
                                WorkspaceFileRow(file: file)
                            }
                        }
                    } footer: {
                        if listing.truncated {
                            Text("Only the first 1,000 files are listed.")
                        }
                    }
                }
                .refreshable { await load() }
            }
        } else if let error {
            ContentUnavailableView {
                Label("Could Not Load the Files", systemImage: "exclamationmark.triangle")
            } description: {
                Text(error)
            } actions: {
                Button("Try Again") { Task { await load() } }
            }
        } else {
            ProgressView()
        }
    }

    private func load() async {
        do {
            listing = try await client.files(conversationId: conversationId)
            error = nil
        } catch is CancellationError {
        } catch {
            if listing == nil { self.error = error.localizedDescription }
        }
    }
}

private struct WorkspaceFileRow: View {
    let file: WorkspaceFile

    var body: some View {
        HStack(spacing: 12) {
            Image(systemName: WorkspaceFileKind.symbol(for: file.path))
                .foregroundStyle(.secondary)
                .frame(width: 24)
            VStack(alignment: .leading, spacing: 2) {
                Text(file.path)
                    .lineLimit(2)
                    .truncationMode(.middle)
                Text(OutgoingAttachment.format(file.size))
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }
        }
    }
}

extension WorkspaceFileKind {
    /// The SF Symbol for a file at `path`.
    static func symbol(for path: String) -> String {
        switch WorkspaceFileKind(path: path) {
        case .image: "photo"
        case .pdf: "doc.richtext"
        case nil: "doc"
        }
    }
}

/// The PDFs a response produced, each opening in the viewer.
struct DocumentsView: View {
    let paths: [String]
    let conversationId: String
    let client: ChatClient
    @State private var viewing: ViewedFile?

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            ForEach(paths, id: \.self) { path in
                Button {
                    viewing = ViewedFile(path: path)
                } label: {
                    Label {
                        Text(path)
                            .lineLimit(1)
                            .truncationMode(.middle)
                    } icon: {
                        Image(systemName: "doc.richtext")
                    }
                    .font(.subheadline)
                    .padding(.horizontal, 12)
                    .padding(.vertical, 8)
                    .background(Color(.secondarySystemBackground), in: RoundedRectangle(cornerRadius: 10))
                }
                .buttonStyle(.plain)
                .accessibilityLabel(Text("Open \(path)"))
            }
        }
        .workspaceFileSheet($viewing, conversationId: conversationId, client: client)
    }
}

/// A workspace file the viewer is showing.
struct ViewedFile: Identifiable, Hashable {
    let path: String
    var id: String { path }
}

extension View {
    /// Shows the workspace file `file` names in the viewer, in a sheet.
    func workspaceFileSheet(_ file: Binding<ViewedFile?>, conversationId: String, client: ChatClient) -> some View {
        sheet(item: file) { file in
            NavigationStack {
                WorkspaceFileViewer(path: file.path, request: client.fileRequest(conversationId: conversationId, path: file.path))
                    .toolbar {
                        ToolbarItem(placement: .topBarLeading) {
                            DoneButton()
                        }
                    }
            }
        }
    }
}

private struct DoneButton: View {
    @Environment(\.dismiss) private var dismiss

    var body: some View {
        Button("Done") { dismiss() }
    }
}

/// A workspace image or PDF at full size, loaded with the access token, with sharing. The file is
/// downloaded to a temporary file named after it, so a share hands over a file with its own name.
struct WorkspaceFileViewer: View {
    /// Larger files are left to the web UI's files panel.
    static let maxBytes = 32 * 1024 * 1024

    let path: String
    let request: URLRequest
    @State private var state: LoadState = .loading

    private enum LoadState {
        case loading
        case failed(String)
        case image(UIImage, URL)
        case pdf(PDFDocument, URL)
    }

    private var name: String { path.split(separator: "/").last.map(String.init) ?? path }

    var body: some View {
        Group {
            switch state {
            case .loading:
                ProgressView()
            case .failed(let message):
                ContentUnavailableView {
                    Label(L10n.format("Could not load %@", name), systemImage: "exclamationmark.triangle")
                } description: {
                    Text(message)
                } actions: {
                    Button("Try Again") { Task { await load() } }
                }
            case .image(let image, _):
                ZoomableImage(image: image)
                    .ignoresSafeArea(edges: .bottom)
                    .accessibilityLabel(Text(path))
            case .pdf(let document, _):
                PDFDocumentView(document: document)
                    .ignoresSafeArea(edges: .bottom)
            }
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .navigationTitle(name)
        .navigationBarTitleDisplayMode(.inline)
        .toolbar {
            if let url = sharedFile {
                ToolbarItem(placement: .topBarTrailing) {
                    ShareLink(item: url) {
                        Label(L10n.format("Share %@", name), systemImage: "square.and.arrow.up")
                    }
                }
            }
        }
        .task(id: request.url) { await load() }
    }

    private var sharedFile: URL? {
        switch state {
        case .image(_, let url), .pdf(_, let url): url
        case .loading, .failed: nil
        }
    }

    private func load() async {
        state = .loading
        guard let kind = WorkspaceFileKind(path: path) else {
            state = .failed(L10n.string("The app shows images and PDFs. Open this file on the web instead."))
            return
        }
        do {
            let (download, response) = try await URLSession.shared.download(for: request)
            guard let http = response as? HTTPURLResponse, (200..<300).contains(http.statusCode) else {
                try? FileManager.default.removeItem(at: download)
                let status = (response as? HTTPURLResponse)?.statusCode ?? 0
                state = .failed(L10n.format("The server answered %lld.", status))
                return
            }
            let size = (try? download.resourceValues(forKeys: [.fileSizeKey]).fileSize) ?? 0
            guard size <= Self.maxBytes else {
                try? FileManager.default.removeItem(at: download)
                state = .failed(L10n.format("This file is over %@, more than the app shows. Open it on the web instead.",
                                            OutgoingAttachment.format(Self.maxBytes)))
                return
            }
            let file = try Self.keep(download, named: name)
            switch kind {
            case .image:
                let image = await Task.detached { UIImage(contentsOfFile: file.path) }.value
                if let image {
                    state = .image(image, file)
                } else {
                    state = .failed(L10n.string("The file isn't an image the app can show."))
                }
            case .pdf:
                let document = await Task.detached { PDFDocument(url: file) }.value
                if let document {
                    state = .pdf(document, file)
                } else {
                    state = .failed(L10n.string("The file isn't a PDF the app can show."))
                }
            }
        } catch is CancellationError {
        } catch let error as URLError where error.code == .cancelled {
        } catch {
            state = .failed(error.localizedDescription)
        }
    }

    /// Moves a download to a folder of its own under the temporary directory, named `name`.
    private static func keep(_ download: URL, named name: String) throws -> URL {
        let folder = FileManager.default.temporaryDirectory
            .appendingPathComponent("workspace-files", isDirectory: true)
            .appendingPathComponent(UUID().uuidString, isDirectory: true)
        try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
        let file = folder.appendingPathComponent(name)
        try FileManager.default.moveItem(at: download, to: file)
        return file
    }
}

/// A PDF with PDFKit's own scrolling, zooming and text selection.
private struct PDFDocumentView: UIViewRepresentable {
    let document: PDFDocument

    func makeUIView(context: Context) -> PDFView {
        let view = PDFView()
        view.autoScales = true
        view.displayMode = .singlePageContinuous
        view.displayDirection = .vertical
        view.backgroundColor = .secondarySystemBackground
        view.document = document
        return view
    }

    func updateUIView(_ view: PDFView, context: Context) {
        if view.document !== document { view.document = document }
    }
}

/// An image fitted to the screen that pinches to zoom up to 6×, and zooms in or back on a double tap.
private struct ZoomableImage: UIViewRepresentable {
    let image: UIImage

    func makeUIView(context: Context) -> ZoomingScrollView {
        ZoomingScrollView(image: image)
    }

    func updateUIView(_ view: ZoomingScrollView, context: Context) {
        view.imageView.image = image
    }
}

private final class ZoomingScrollView: UIScrollView, UIScrollViewDelegate {
    let imageView: UIImageView

    init(image: UIImage) {
        imageView = UIImageView(image: image)
        super.init(frame: .zero)
        imageView.contentMode = .scaleAspectFit
        addSubview(imageView)
        delegate = self
        minimumZoomScale = 1
        maximumZoomScale = 6
        showsHorizontalScrollIndicator = false
        showsVerticalScrollIndicator = false
        backgroundColor = .systemBackground
        let doubleTap = UITapGestureRecognizer(target: self, action: #selector(toggleZoom(_:)))
        doubleTap.numberOfTapsRequired = 2
        addGestureRecognizer(doubleTap)
    }

    required init?(coder: NSCoder) {
        fatalError("init(coder:) is not used")
    }

    override func layoutSubviews() {
        super.layoutSubviews()
        // At 1× the image fills the view; zooming scales that frame.
        if zoomScale == minimumZoomScale {
            imageView.frame = bounds
            contentSize = bounds.size
        }
    }

    func viewForZooming(in scrollView: UIScrollView) -> UIView? {
        imageView
    }

    @objc private func toggleZoom(_ tap: UITapGestureRecognizer) {
        if zoomScale > minimumZoomScale {
            setZoomScale(minimumZoomScale, animated: true)
        } else {
            let point = tap.location(in: imageView)
            let size = CGSize(width: bounds.width / 3, height: bounds.height / 3)
            zoom(to: CGRect(x: point.x - size.width / 2, y: point.y - size.height / 2, width: size.width, height: size.height), animated: true)
        }
    }
}
