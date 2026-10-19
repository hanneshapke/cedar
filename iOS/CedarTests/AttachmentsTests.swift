import UIKit
import UniformTypeIdentifiers
import XCTest
@testable import Cedar

final class AttachmentsTests: XCTestCase {
    func testKeepsTypesTheServerTakesAndReencodesOthers() throws {
        let pdf = try XCTUnwrap(OutgoingAttachment.make(name: "a.pdf", type: .pdf, data: Data("%PDF".utf8)))
        XCTAssertEqual(pdf.mediaType, "application/pdf")
        XCTAssertFalse(pdf.isImage)

        let png = UIGraphicsImageRenderer(size: CGSize(width: 4, height: 4)).pngData { ctx in
            UIColor.red.setFill()
            ctx.fill(CGRect(x: 0, y: 0, width: 4, height: 4))
        }
        let kept = try XCTUnwrap(OutgoingAttachment.make(name: "dot.png", type: .png, data: png))
        XCTAssertEqual(kept.mediaType, "image/png")
        XCTAssertEqual(kept.data, png)

        // A type the server doesn't take, but an image all the same: sent as JPEG.
        let converted = try XCTUnwrap(OutgoingAttachment.make(name: "dot.tiff", type: .tiff, data: png))
        XCTAssertEqual(converted.mediaType, "image/jpeg")
        XCTAssertEqual(converted.name, "dot.jpg")

        XCTAssertNil(OutgoingAttachment.make(name: "notes.txt", type: .plainText, data: Data("hi".utf8)))
        XCTAssertNil(OutgoingAttachment.make(name: "x", type: nil, data: Data("hi".utf8)))
    }

    func testLimits() {
        let pdf = OutgoingAttachment(name: "a.pdf", mediaType: "application/pdf", data: Data(count: 10))
        let image = OutgoingAttachment(name: "a.png", mediaType: "image/png", data: Data(count: 10))
        XCTAssertNil(OutgoingAttachment.problem(adding: pdf, to: [], imagesAllowed: false))
        XCTAssertNotNil(OutgoingAttachment.problem(adding: image, to: [], imagesAllowed: false))
        XCTAssertNil(OutgoingAttachment.problem(adding: image, to: [], imagesAllowed: true))

        let empty = OutgoingAttachment(name: "e.pdf", mediaType: "application/pdf", data: Data())
        XCTAssertNotNil(OutgoingAttachment.problem(adding: empty, to: [], imagesAllowed: true))

        let full = Array(repeating: pdf, count: OutgoingAttachment.maxFiles)
        XCTAssertNotNil(OutgoingAttachment.problem(adding: pdf, to: full, imagesAllowed: true))

        let big = OutgoingAttachment(name: "b.pdf", mediaType: "application/pdf", data: Data(count: OutgoingAttachment.maxBytes))
        XCTAssertNil(OutgoingAttachment.problem(adding: big, to: [], imagesAllowed: true))
        XCTAssertNotNil(OutgoingAttachment.problem(adding: big, to: [big], imagesAllowed: true))
    }

    func testEncodesAttachmentsTheWayTheServerReadsThem() throws {
        let message = ChatClient.SendMessage(content: "look", model: nil, reasoning: nil,
                                             attachments: [.init(name: "a.pdf", mediaType: "application/pdf", data: "JVBERg==")])
        let json = try XCTUnwrap(JSONSerialization.jsonObject(with: JSONEncoder().encode(message)) as? [String: Any])
        XCTAssertNil(json["model"])
        let files = try XCTUnwrap(json["attachments"] as? [[String: String]])
        XCTAssertEqual(files, [["name": "a.pdf", "media_type": "application/pdf", "data": "JVBERg=="]])

        let plain = try JSONSerialization.jsonObject(with: JSONEncoder().encode(ChatClient.SendMessage(content: "hi"))) as? [String: Any]
        XCTAssertNil(plain?["attachments"])
    }
}
