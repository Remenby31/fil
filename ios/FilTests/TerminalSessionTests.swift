import XCTest

@testable import Fil

/// A transport that never touches the network. Records what it was asked to do
/// and lets the test drive the callbacks by hand.
final class FakeTransport: TerminalTransport, @unchecked Sendable {
    var onDataReceived: (@Sendable (Data) -> Void)?
    var onConnected: (@Sendable () -> Void)?
    var onDisconnected: (@Sendable () -> Void)?

    private(set) var connectCount = 0
    private(set) var disconnectCount = 0
    private(set) var sentInput: [Data] = []
    private(set) var sentResizes: [(cols: UInt16, rows: UInt16)] = []

    func connect(sessionId: String) { connectCount += 1 }
    func disconnect() { disconnectCount += 1 }
    func sendInput(_ data: Data) { sentInput.append(data) }
    func sendResize(cols: UInt16, rows: UInt16) { sentResizes.append((cols, rows)) }

    /// Simulate the transport reaching .ready.
    func becomeConnected() { onConnected?() }
    /// Simulate the transport dying.
    func die() { onDisconnected?() }
    /// Simulate bytes arriving from the hub.
    func deliver(_ text: String) { onDataReceived?(Data(text.utf8)) }
}

/// Sendable box: the registry's factory is a `@Sendable` closure, so it cannot
/// capture the XCTestCase.
final class TransportRecorder: @unchecked Sendable {
    private let lock = NSLock()
    private var made: [FakeTransport] = []

    func record(_ t: FakeTransport) {
        lock.lock()
        defer { lock.unlock() }
        made.append(t)
    }

    var all: [FakeTransport] {
        lock.lock()
        defer { lock.unlock() }
        return made
    }
}

final class TerminalSessionTests: XCTestCase {
    private var recorder = TransportRecorder()
    private var transports: [FakeTransport] { recorder.all }

    override func setUp() {
        super.setUp()
        recorder = TransportRecorder()
        let recorder = self.recorder
        TerminalConnectionRegistry.shared.removeAllSessions()
        TerminalConnectionRegistry.shared.makeTransport = { _, _ in
            let t = FakeTransport()
            recorder.record(t)
            return t
        }
    }

    override func tearDown() {
        TerminalConnectionRegistry.shared.removeAllSessions()
        super.tearDown()
    }

    /// The regression this whole refactor exists for.
    ///
    /// Before: the AsyncStream's onTermination destroyed `outputRelays[sid]`,
    /// and the next `connect` minted a fresh relay. The live SwiftTerm view
    /// still held the first one, so the terminal rendered nothing while the UI
    /// said "connected".
    func testRelayIdentitySurvivesAReconnect() async {
        let registry = TerminalConnectionRegistry.shared
        let sid = "session-a"

        // What makeUIView captures.
        let relayHeldByTheView = registry.outputRelay(sessionId: sid)

        var stream: AsyncStream<TerminalConnectionState>? = registry.open(
            sessionId: sid, hubHost: "example.invalid")
        XCTAssertNotNil(stream)

        // Cancel the subscription the way a TCA effect cancellation would.
        stream = nil

        // Reconnect.
        _ = registry.open(sessionId: sid, hubHost: "example.invalid")
        let relayAfterReconnect = registry.outputRelay(sessionId: sid)

        XCTAssertTrue(
            relayHeldByTheView === relayAfterReconnect,
            "the view's relay must survive a reconnect; a new one strands the terminal"
        )
    }

    /// Cancelling one subscriber must not disconnect the transport.
    func testDroppingASubscriberDoesNotTearDownTheConnection() async {
        let registry = TerminalConnectionRegistry.shared
        let sid = "session-b"

        var stream: AsyncStream<TerminalConnectionState>? = registry.open(
            sessionId: sid, hubHost: "example.invalid")
        _ = stream
        XCTAssertEqual(transports.count, 1, "one connection attempt")

        stream = nil
        try? await Task.sleep(nanoseconds: 50_000_000)

        XCTAssertEqual(
            transports[0].disconnectCount, 0,
            "unsubscribing must not disconnect; only close() may"
        )
    }

    /// Re-opening an already-live session must reuse the connection.
    func testOpenIsIdempotent() {
        let registry = TerminalConnectionRegistry.shared
        let sid = "session-c"

        _ = registry.open(sessionId: sid, hubHost: "example.invalid")
        _ = registry.open(sessionId: sid, hubHost: "example.invalid")
        _ = registry.open(sessionId: sid, hubHost: "example.invalid")

        XCTAssertEqual(
            transports.count, 1,
            "three subscribers must share one connection, not open three"
        )
    }

    /// Bytes must reach the relay the view is attached to, after a reconnect.
    func testOutputStillReachesTheOriginalRelayAfterReconnect() async {
        let registry = TerminalConnectionRegistry.shared
        let sid = "session-d"

        let relay = registry.outputRelay(sessionId: sid)
        _ = registry.open(sessionId: sid, hubHost: "example.invalid")
        XCTAssertEqual(transports.count, 1)

        registry.reconnect(sessionId: sid)
        XCTAssertEqual(transports.count, 2, "reconnect opens a second transport")
        XCTAssertEqual(transports[0].disconnectCount, 1, "the old transport is closed")

        transports[1].deliver("hello")

        XCTAssertTrue(
            registry.outputRelay(sessionId: sid) === relay,
            "post-reconnect output must land in the relay the view holds"
        )
    }

    /// A new connection must re-declare the geometry it inherited.
    func testGeometryIsReDeclaredOnEveryNewConnection() {
        let registry = TerminalConnectionRegistry.shared
        let sid = "session-e"

        _ = registry.open(sessionId: sid, hubHost: "example.invalid")
        transports[0].becomeConnected()
        registry.resize(sessionId: sid, cols: 100, rows: 40)
        XCTAssertEqual(transports[0].sentResizes.last?.cols, 100)

        registry.reconnect(sessionId: sid)
        transports[1].becomeConnected()

        XCTAssertEqual(
            transports[1].sentResizes.first?.cols, 100,
            "a reconnected session must tell the PTY its size again"
        )
        XCTAssertEqual(transports[1].sentResizes.first?.rows, 40)
    }

    /// close() is the only thing that may destroy a session.
    func testCloseTearsDownAndDropsTheSession() {
        let registry = TerminalConnectionRegistry.shared
        let sid = "session-f"

        _ = registry.open(sessionId: sid, hubHost: "example.invalid")
        XCTAssertNotNil(registry.existingSession(sessionId: sid))

        registry.close(sessionId: sid)

        XCTAssertEqual(transports[0].disconnectCount, 1)
        XCTAssertNil(registry.existingSession(sessionId: sid))
    }

    /// A dropped connection must not be reported as something the user can ignore.
    func testTransportDeathSurfacesAsUnreachable() {
        let registry = TerminalConnectionRegistry.shared
        let sid = "session-g"

        _ = registry.open(sessionId: sid, hubHost: "example.invalid")
        transports[0].becomeConnected()
        XCTAssertEqual(registry.existingSession(sessionId: sid)?.currentState, .connected)

        transports[0].die()

        XCTAssertEqual(
            registry.existingSession(sessionId: sid)?.currentState,
            .unreachable("Connection lost")
        )
    }
}
