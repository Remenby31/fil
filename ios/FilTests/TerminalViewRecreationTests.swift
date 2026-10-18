import XCTest
import SwiftTerm
import ComposableArchitecture

@testable import Fil

/// SwiftUI recreates the terminal view on a size-class change (Pro Max/iPad
/// rotation, and the app-switcher snapshots iOS takes while the app is in the
/// background). The replacement's `onAppear` can run before the old view's
/// `onDisappear`. Reproduced live: about one rotation in two left a blank
/// terminal that still showed "connected" and never printed another byte.
final class TerminalViewRecreationTests: XCTestCase {
    private var recorder = TransportRecorder()
    private var registry: TerminalConnectionRegistry!
    private var transports: [FakeTransport] { recorder.all }

    override func setUp() {
        super.setUp()
        recorder = TransportRecorder()
        registry = makeRegistry(releaseGrace: 0)
    }

    private func makeRegistry(releaseGrace: TimeInterval) -> TerminalConnectionRegistry {
        let recorder = self.recorder
        return TerminalConnectionRegistry(
            monitorsNetwork: false,
            releaseGrace: releaseGrace,
            makeTransport: { _, host in
                let t = FakeTransport()
                recorder.record(t, host: host)
                return t
            },
            ticketProvider: { _ in nil }
        )
    }

    override func tearDown() {
        registry.removeAllSessions()
        super.tearDown()
    }

    func testOldViewsLateDisappearCannotTearDownItsReplacement() {
        let sid = "recreated-late-disappear"
        let oldView = UUID(), newView = UUID()
        _ = registry.open(sessionId: sid, hubHost: "example.invalid", holder: oldView)
        transports[0].becomeConnected()

        _ = registry.open(sessionId: sid, hubHost: "example.invalid", holder: newView)
        registry.release(holder: oldView)

        XCTAssertEqual(transports.count, 1, "the live connection is reused, not rebuilt")
        XCTAssertEqual(transports[0].disconnectCount, 0)
        XCTAssertEqual(registry.existingSession(sessionId: sid)?.currentState, .connected)

        registry.release(holder: newView)
        XCTAssertNil(registry.existingSession(sessionId: sid), "the last view to leave closes the session")
        XCTAssertEqual(transports[0].disconnectCount, 1)
    }

    /// The other ordering: the old view goes first. The replacement must adopt
    /// the same session, not cold-attach a new one.
    func testReplacementArrivingAfterTheOldViewLeftAdoptsTheSameSession() async {
        let registry = makeRegistry(releaseGrace: 0.2)
        defer { registry.removeAllSessions() }
        let sid = "recreated-old-first"
        let oldView = UUID(), newView = UUID()
        _ = registry.open(sessionId: sid, hubHost: "example.invalid", holder: oldView)
        transports[0].becomeConnected()

        registry.release(holder: oldView)
        _ = registry.open(sessionId: sid, hubHost: "example.invalid", holder: newView)
        try? await Task.sleep(nanoseconds: 400_000_000)

        XCTAssertEqual(transports.count, 1)
        XCTAssertEqual(transports[0].disconnectCount, 0)
        XCTAssertEqual(registry.existingSession(sessionId: sid)?.currentState, .connected)

        registry.release(holder: newView)
        try? await Task.sleep(nanoseconds: 400_000_000)
        XCTAssertNil(registry.existingSession(sessionId: sid), "an unheld session still closes after the grace")
        XCTAssertEqual(transports[0].disconnectCount, 1)
    }

    func testStaleHolderFromAPreviousOpeningCannotCloseANewOne() {
        let sid = "recreated-stale-holder"
        let first = UUID(), second = UUID()
        _ = registry.open(sessionId: sid, hubHost: "example.invalid", holder: first)
        registry.close(sessionId: sid)
        _ = registry.open(sessionId: sid, hubHost: "example.invalid", holder: second)

        registry.release(holder: first)

        XCTAssertNotNil(registry.existingSession(sessionId: sid))
        XCTAssertEqual(transports.last?.disconnectCount, 0)
    }

    func testMovingAHolderToAnotherSessionReleasesThePreviousOne() {
        let view = UUID()
        _ = registry.open(sessionId: "split-a", hubHost: "example.invalid", holder: view)
        _ = registry.open(sessionId: "split-b", hubHost: "example.invalid", holder: view)

        XCTAssertNil(registry.existingSession(sessionId: "split-a"))
        XCTAssertNotNil(registry.existingSession(sessionId: "split-b"))
    }

    /// The resume cursor describes what the *renderer* holds. A fresh SwiftTerm
    /// view holds nothing, so it must be given the screen back locally; the
    /// network will only ever send it the delta.
    @MainActor
    func testRecreatedViewShowsWhatTheSessionAlreadyDisplayed() async {
        let sid = "recreated-replay"
        let relay = registry.outputRelay(sessionId: sid)
        let first = SwiftTerm.TerminalView(frame: CGRect(x: 0, y: 0, width: 400, height: 600))
        relay.attach(first)
        _ = registry.open(sessionId: sid, hubHost: "example.invalid", holder: UUID())
        transports[0].becomeConnected()
        transports[0].deliver("before-background\r\n")
        await mainTurn()

        let replacement = SwiftTerm.TerminalView(frame: CGRect(x: 0, y: 0, width: 400, height: 600))
        relay.attach(replacement)
        relay.detach(first)
        await mainTurn()
        transports[0].deliver("after-return\r\n")
        await mainTurn()

        let shown = text(replacement)
        XCTAssertTrue(shown.contains("before-background"), "the replacement view must not come up blank")
        XCTAssertTrue(shown.contains("after-return"))
        XCTAssertEqual(shown.components(separatedBy: "before-background").count, 2, "replayed exactly once")
    }

    @MainActor
    func testReattachingTheSameViewDoesNotDuplicateItsContent() async {
        let relay = registry.outputRelay(sessionId: "recreated-same-view")
        let view = SwiftTerm.TerminalView(frame: CGRect(x: 0, y: 0, width: 400, height: 600))
        relay.attach(view)
        relay.enqueue(Data("only-once\r\n".utf8))
        await mainTurn()

        relay.detach(view)
        relay.attach(view)
        await mainTurn()

        XCTAssertEqual(text(view).components(separatedBy: "only-once").count, 2)
    }

    /// History contains terminal queries (DA, kitty keyboard) that the Mac
    /// terminal already answered. Answering them again types `65;4;1;2;6...`
    /// into the user's shell; this was visible in the live repro.
    @MainActor
    func testReplayedHistoryNeverAnswersStaleTerminalQueries() async {
        let relay = registry.outputRelay(sessionId: "recreated-queries")
        let first = SwiftTerm.TerminalView(frame: CGRect(x: 0, y: 0, width: 400, height: 600))
        relay.attach(first)
        relay.enqueue(Data("prompt \u{1b}[c\u{1b}[?u\r\n".utf8))
        await mainTurn()

        let replacement = SwiftTerm.TerminalView(frame: CGRect(x: 0, y: 0, width: 400, height: 600))
        let replies = ReplyRecorder()
        replacement.terminalDelegate = replies
        relay.attach(replacement)
        await mainTurn()

        XCTAssertTrue(text(replacement).contains("prompt"))
        XCTAssertTrue(replies.sent.isEmpty, "stale queries were answered: \(replies.sent)")
        XCTAssertTrue(replacement.terminalDelegate === replies, "the delegate is restored after replay")

        relay.enqueue(Data("\u{1b}[c".utf8))
        await mainTurn()
        XCTAssertFalse(replies.sent.isEmpty, "live queries are still answered")
    }

    @MainActor
    private func mainTurn() async {
        await withCheckedContinuation { continuation in
            DispatchQueue.main.async { continuation.resume() }
        }
    }

    @MainActor
    private func text(_ view: SwiftTerm.TerminalView) -> String {
        String(decoding: view.getTerminal().getBufferAsData(), as: UTF8.self)
    }
}

private final class ReplyRecorder: NSObject, SwiftTerm.TerminalViewDelegate {
    var sent: [Data] = []
    func send(source: SwiftTerm.TerminalView, data: ArraySlice<UInt8>) { sent.append(Data(data)) }
    func sizeChanged(source: SwiftTerm.TerminalView, newCols: Int, newRows: Int) {}
    func setTerminalTitle(source: SwiftTerm.TerminalView, title: String) {}
    func hostCurrentDirectoryUpdate(source: SwiftTerm.TerminalView, directory: String?) {}
    func scrolled(source: SwiftTerm.TerminalView, position: Double) {}
    func requestOpenLink(source: SwiftTerm.TerminalView, link: String, params: [String: String]) {}
    func bell(source: SwiftTerm.TerminalView) {}
    func clipboardCopy(source: SwiftTerm.TerminalView, content: Data) {}
    func iTermContent(source: SwiftTerm.TerminalView, content: ArraySlice<UInt8>) {}
    func rangeChanged(source: SwiftTerm.TerminalView, startY: Int, endY: Int) {}
}

@MainActor
final class TerminalViewLifecycleFeatureTests: XCTestCase {
    func testReplacementViewKeepsTheSessionAndItsSubscription() async {
        let events = TestValue([String]())
        let (stream, continuation) = AsyncStream<TerminalConnectionState>.makeStream()
        let oldView = UUID(), newView = UUID()
        let store = TestStore(initialState: TerminalFeature.State(session: session("A"))) {
            TerminalFeature()
        } withDependencies: {
            $0.terminalClient.open = { id, _, holder in
                events.update { $0.append("open-\(id)-\(holder == oldView ? "old" : "new")") }
                return holder == newView ? stream : AsyncStream { $0.finish() }
            }
            $0.terminalClient.release = { holder in
                events.update { $0.append("release-\(holder == oldView ? "old" : "new")") }
            }
            $0.terminalClient.close = { id in events.update { $0.append("close-\(id)") } }
        }
        store.exhaustivity = .off

        await store.send(.onAppear(viewID: oldView))
        await store.send(.onAppear(viewID: newView))
        await store.send(.onDisappear(viewID: oldView))

        continuation.yield(.connected)
        await store.receive(\.connectionStateChanged) { $0.isConnected = true }
        XCTAssertEqual(events.value, ["open-A-old", "open-A-new", "release-old"])

        await store.send(.onDisappear(viewID: newView))
        continuation.finish()
        await store.finish()
        XCTAssertEqual(events.value.last, "release-new")
        XCTAssertFalse(events.value.contains("close-A"), "a disappearing view releases; only dismiss closes")
    }

    private func session(_ id: String) -> Session {
        Session(id: id, deviceId: "test-device", shell: "zsh", command: nil, cwd: "/tmp",
                cols: 80, rows: 24, status: .online, createdAt: nil)
    }
}
