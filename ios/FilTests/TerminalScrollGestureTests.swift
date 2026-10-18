import XCTest
import SwiftTerm
import UIKit

@testable import Fil

final class TerminalScrollLogicTests: XCTestCase {
    func testRouteFollowsMouseReportingThenAlternateScreen() {
        XCTAssertEqual(TerminalScrollRoute(mouseReporting: false, alternateScreen: false), .scrollback)
        XCTAssertEqual(TerminalScrollRoute(mouseReporting: false, alternateScreen: true), .arrowKeys)
        XCTAssertEqual(TerminalScrollRoute(mouseReporting: true, alternateScreen: true), .mouseWheel)
        XCTAssertEqual(TerminalScrollRoute(mouseReporting: true, alternateScreen: false), .mouseWheel)
    }

    func testArrowKeysHonourCursorMode() {
        XCTAssertEqual(TerminalScrollRoute.arrowKeys(lines: 2, applicationCursor: false),
                       [0x1b, 0x5b, 0x41, 0x1b, 0x5b, 0x41])
        XCTAssertEqual(TerminalScrollRoute.arrowKeys(lines: -1, applicationCursor: false), [0x1b, 0x5b, 0x42])
        XCTAssertEqual(TerminalScrollRoute.arrowKeys(lines: 1, applicationCursor: true), [0x1b, 0x4f, 0x41])
        XCTAssertEqual(TerminalScrollRoute.arrowKeys(lines: -1, applicationCursor: true), [0x1b, 0x4f, 0x42])
        XCTAssertEqual(TerminalScrollRoute.arrowKeys(lines: 0, applicationCursor: false), [])
    }

    func testAccumulatorEmitsOneLinePerRowAndKeepsRemainder() {
        var accumulator = TerminalLineAccumulator(lineHeight: 17)
        XCTAssertEqual(accumulator.lines(for: 10), 0)
        XCTAssertEqual(accumulator.lines(for: 10), 1)
        XCTAssertEqual(accumulator.remainder, 3, accuracy: 0.001)
        XCTAssertEqual(accumulator.lines(for: 40), 2)
        XCTAssertEqual(accumulator.lines(for: -9), 0)
        XCTAssertEqual(accumulator.lines(for: -17), -1)
        accumulator.reset()
        XCTAssertEqual(accumulator.remainder, 0)
    }

    func testSlowJitterNeverScrolls() {
        var accumulator = TerminalLineAccumulator(lineHeight: 17)
        var total = 0
        for delta in [4, -3, 5, -6, 2, -2] as [CGFloat] {
            total += accumulator.lines(for: delta)
        }
        XCTAssertEqual(total, 0)
    }

    func testMomentumDecaysLikeUIScrollViewAndSettles() {
        var momentum = TerminalScrollMomentum(velocity: 2000)
        var distance: CGFloat = 0
        var frames = 0
        while !momentum.isSettled && frames < 10_000 {
            distance += momentum.advance(by: 1.0 / 120)
            frames += 1
        }
        let rate = TerminalScrollMomentum.decelerationRate
        let expected = 2000 / (1000 * -log(rate))
        XCTAssertEqual(distance, expected, accuracy: expected * 0.02)
        XCTAssertLessThan(frames, 10_000)
        XCTAssertEqual(momentum.advance(by: 1.0 / 120), 0)
    }

    func testMomentumKeepsDirection() {
        var momentum = TerminalScrollMomentum(velocity: -800)
        XCTAssertLessThan(momentum.advance(by: 1.0 / 60), 0)
        XCTAssertTrue(TerminalScrollMomentum(velocity: 5).isSettled)
    }
}

@MainActor
final class TerminalScrollGestureTests: XCTestCase {
    private var received = Data()
    private var coordinator: SwiftTermWrapper.Coordinator!

    private func makeView() -> FilTerminalView {
        let view = FilTerminalView(frame: CGRect(x: 0, y: 0, width: 390, height: 600))
        coordinator = SwiftTermWrapper.Coordinator()
        coordinator.onInput = { [unowned self] in self.received.append($0) }
        view.terminalDelegate = coordinator
        received = Data()
        return view
    }

    private func plainPans(on view: UIView) -> [UIGestureRecognizer] {
        (view.gestureRecognizers ?? []).filter { type(of: $0) == UIPanGestureRecognizer.self }
    }

    func testNormalScreenScrollsLocalHistoryNatively() {
        let view = makeView()
        view.feed(text: (1...200).map { "line-\($0)\r\n" }.joined())
        XCTAssertEqual(view.scrollRoute, .scrollback)
        XCTAssertTrue(view.isScrollEnabled)
        XCTAssertTrue(view.panGestureRecognizer.isEnabled)
    }

    // Claude Code 2.x enables ?1049h + ?1000/1002/1003h + ?1006h.
    func testClaudeCodeFullscreenRoutesDragToWheelEvents() {
        let view = makeView()
        view.feed(text: "\u{1b}[?1049h\u{1b}[?1000h\u{1b}[?1002h\u{1b}[?1003h\u{1b}[?1006h")
        XCTAssertEqual(view.scrollRoute, .mouseWheel)
        XCTAssertFalse(view.gestureRecognizerShouldBegin(view.panGestureRecognizer),
                       "UIScrollView must not swallow the drag on the alternate screen")
        XCTAssertEqual(plainPans(on: view).map { $0 === view.applicationScrollGesture }, [true],
                       "SwiftTerm's button-1 drag pan must not be installed")

        view.applyApplicationScroll(view.rowHeight * 2 + 1, route: .mouseWheel)
        let text = String(decoding: received, as: UTF8.self)
        XCTAssertEqual(text.components(separatedBy: "\u{1b}[<64;").count - 1, 2, text)
        XCTAssertTrue(text.hasSuffix("M"), text)

        received = Data()
        view.applyApplicationScroll(-(view.rowHeight * 3 + 1), route: .mouseWheel)
        let down = String(decoding: received, as: UTF8.self)
        XCTAssertEqual(down.components(separatedBy: "\u{1b}[<65;").count - 1, 3, down)
    }

    func testAlternateScreenWithoutMouseSendsArrows() {
        let view = makeView()
        view.feed(text: "\u{1b}[?1049h")
        XCTAssertEqual(view.scrollRoute, .arrowKeys)
        view.applyApplicationScroll(view.rowHeight, route: .arrowKeys)
        XCTAssertEqual([UInt8](received), [0x1b, 0x5b, 0x41])

        received = Data()
        view.feed(text: "\u{1b}[?1h")
        view.applyApplicationScroll(-view.rowHeight, route: .arrowKeys)
        XCTAssertEqual([UInt8](received), [0x1b, 0x4f, 0x42])
    }

    func testLeavingFullscreenRestoresNativeScrolling() {
        let view = makeView()
        view.feed(text: "\u{1b}[?1049h\u{1b}[?1000h\u{1b}[?1006h")
        view.feed(text: "\u{1b}[?1000l\u{1b}[?1006l\u{1b}[?1049l")
        XCTAssertEqual(view.scrollRoute, .scrollback)
    }

    func testOneBurstIsCappedToAScreenful() {
        let view = makeView()
        view.feed(text: "\u{1b}[?1049h")
        view.applyApplicationScroll(view.rowHeight * 10_000, route: .arrowKeys)
        XCTAssertEqual(received.count, view.getTerminal().rows * 3)
    }
}
