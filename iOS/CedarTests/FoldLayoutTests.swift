import XCTest
@testable import Cedar

final class FoldLayoutTests: XCTestCase {
    func testSplitsAtTheCreaseOnANearlySquareRegularCanvas() {
        // An unfolded book-style device, upright or on its side.
        XCTAssertTrue(FoldLayout.splitsAtCrease(CGSize(width: 760, height: 820)))
        XCTAssertTrue(FoldLayout.splitsAtCrease(CGSize(width: 820, height: 760)))
        XCTAssertEqual(FoldLayout.sidebarWidth(for: CGSize(width: 761, height: 800)), 381)
    }

    func testLeavesPhonesAndTabletsToTheSystem() {
        // A phone, or the folded outer display: one column, nothing to pin.
        XCTAssertFalse(FoldLayout.splitsAtCrease(CGSize(width: 375, height: 812)))
        XCTAssertFalse(FoldLayout.splitsAtCrease(CGSize(width: 812, height: 375)))
        // An iPad in either orientation is 4:3 or longer.
        XCTAssertFalse(FoldLayout.splitsAtCrease(CGSize(width: 820, height: 1180)))
        XCTAssertFalse(FoldLayout.splitsAtCrease(CGSize(width: 1180, height: 820)))
        XCTAssertFalse(FoldLayout.splitsAtCrease(CGSize(width: 1024, height: 1366)))
        XCTAssertNil(FoldLayout.sidebarWidth(for: CGSize(width: 1180, height: 820)))
        // A small square window is one column anyway.
        XCTAssertFalse(FoldLayout.splitsAtCrease(CGSize(width: 500, height: 500)))
        XCTAssertFalse(FoldLayout.splitsAtCrease(CGSize(width: 700, height: 0)))
    }

    func testComposerStaysShortOnACompactHeight() {
        XCTAssertEqual(FoldLayout.composerLineLimit(verticalSizeClass: .compact), 1...3)
        XCTAssertEqual(FoldLayout.composerLineLimit(verticalSizeClass: .regular), 1...8)
        XCTAssertEqual(FoldLayout.composerLineLimit(verticalSizeClass: nil), 1...8)
    }
}
