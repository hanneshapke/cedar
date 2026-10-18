import AVFoundation
import SwiftUI
import UIKit

/// Scans the QR code the Cedar Mac app shows under iPhone & iPad, and hands back what it holds.
struct ScanCodeView: View {
    var onScan: (ConnectionCode) -> Void
    @Environment(\.dismiss) private var dismiss
    @Environment(\.openURL) private var openURL
    @State private var access = AVCaptureDevice.authorizationStatus(for: .video)
    @State private var rejected = false

    var body: some View {
        NavigationStack {
            content
                .navigationTitle("Scan QR Code")
                .navigationBarTitleDisplayMode(.inline)
                .toolbar {
                    ToolbarItem(placement: .cancellationAction) {
                        Button("Cancel") { dismiss() }
                    }
                }
        }
    }

    @ViewBuilder
    private var content: some View {
        switch access {
        case .authorized:
            if AVCaptureDevice.default(for: .video) == nil {
                ContentUnavailableView(
                    "No camera",
                    systemImage: "camera",
                    description: Text("This device has no camera to scan with. Enter the address and token instead.")
                )
            } else {
                QRScanner { text in
                    guard let code = ConnectionCode(text) else {
                        rejected = true
                        return false
                    }
                    onScan(code)
                    dismiss()
                    return true
                }
                .ignoresSafeArea(edges: .bottom)
                .overlay(alignment: .bottom) {
                    Text(rejected
                         ? "That is not a Cedar code. In the Mac app, open iPhone & iPad and click Show QR code."
                         : "Point the camera at the code in the Cedar Mac app (iPhone & iPad → Show QR code).")
                        .font(.footnote)
                        .multilineTextAlignment(.center)
                        .padding(12)
                        .background(.regularMaterial, in: RoundedRectangle(cornerRadius: 12))
                        .padding()
                }
            }
        case .notDetermined:
            ProgressView()
                .task {
                    _ = await AVCaptureDevice.requestAccess(for: .video)
                    access = AVCaptureDevice.authorizationStatus(for: .video)
                }
        default:
            ContentUnavailableView {
                Label("Camera access is off", systemImage: "camera")
            } description: {
                Text("Allow Cedar to use the camera in Settings to scan the code, or enter the address and token by hand.")
            } actions: {
                Button("Open Settings") {
                    if let url = URL(string: UIApplication.openSettingsURLString) {
                        openURL(url)
                    }
                }
            }
        }
    }
}

/// The camera preview, reporting each QR code it reads. `onCode` returns whether the code was
/// taken; until one is, scanning goes on.
private struct QRScanner: UIViewControllerRepresentable {
    var onCode: (String) -> Bool

    func makeUIViewController(context: Context) -> QRScannerController {
        let controller = QRScannerController()
        controller.onCode = onCode
        return controller
    }

    func updateUIViewController(_ controller: QRScannerController, context: Context) {
        controller.onCode = onCode
    }
}

private final class QRScannerController: UIViewController, AVCaptureMetadataOutputObjectsDelegate {
    var onCode: ((String) -> Bool)?
    private let session = AVCaptureSession()
    private var preview: AVCaptureVideoPreviewLayer?
    private var done = false
    /// The last code that was turned down, so it isn't reported again every frame.
    private var lastRejected: String?

    override func viewDidLoad() {
        super.viewDidLoad()
        view.backgroundColor = .black
        guard let camera = AVCaptureDevice.default(for: .video),
              let input = try? AVCaptureDeviceInput(device: camera),
              session.canAddInput(input)
        else { return }
        session.addInput(input)
        let output = AVCaptureMetadataOutput()
        guard session.canAddOutput(output) else { return }
        session.addOutput(output)
        output.setMetadataObjectsDelegate(self, queue: .main)
        output.metadataObjectTypes = [.qr]
        let layer = AVCaptureVideoPreviewLayer(session: session)
        layer.videoGravity = .resizeAspectFill
        view.layer.addSublayer(layer)
        preview = layer
    }

    override func viewDidLayoutSubviews() {
        super.viewDidLayoutSubviews()
        preview?.frame = view.bounds
    }

    override func viewWillAppear(_ animated: Bool) {
        super.viewWillAppear(animated)
        // startRunning blocks until the camera is up; keep it off the main thread.
        let session = session
        DispatchQueue.global(qos: .userInitiated).async {
            if !session.isRunning { session.startRunning() }
        }
    }

    override func viewWillDisappear(_ animated: Bool) {
        super.viewWillDisappear(animated)
        let session = session
        DispatchQueue.global(qos: .userInitiated).async {
            if session.isRunning { session.stopRunning() }
        }
    }

    func metadataOutput(_ output: AVCaptureMetadataOutput, didOutput metadataObjects: [AVMetadataObject], from connection: AVCaptureConnection) {
        guard !done,
              let code = metadataObjects.first as? AVMetadataMachineReadableCodeObject,
              let text = code.stringValue,
              text != lastRejected
        else { return }
        if onCode?(text) == true {
            done = true
            UINotificationFeedbackGenerator().notificationOccurred(.success)
        } else {
            lastRejected = text
            UINotificationFeedbackGenerator().notificationOccurred(.warning)
        }
    }
}
