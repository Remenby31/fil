import XCTest

@testable import Fil

final class TerminalConnectionSpeedTests: XCTestCase {
    /// A synchronous factory-order assertion, not a flaky wall-clock limit.
    /// QUIC must not be constructed on a healthy WSS cold open at all.
    func testEveryColdOpenSelectsWSSBeforeAnyQUICAttempt() {
        let attempts = SpeedAttempts()
        let registry = makeLiveRegistry(attempts)
        defer { registry.removeAllSessions() }

        for sid in ["speed-first", "speed-first", "speed-second"] {
            _ = registry.outputRelay(sessionId: sid)
            _ = registry.open(sessionId: sid, hubHost: "quic.example.invalid")
            XCTAssertEqual(attempts.all.last?.kind, .webSocket)
            registry.close(sessionId: sid)
        }
        XCTAssertEqual(attempts.all.map(\.kind), [.webSocket, .webSocket, .webSocket])
    }

    func testHealthyWSSConnectsAndSendsInputWithoutCreatingQUIC() async {
        let attempts = SpeedAttempts()
        let registry = makeLiveRegistry(attempts)
        defer { registry.removeAllSessions() }
        _ = registry.open(sessionId: "speed-healthy", hubHost: "quic.example.invalid")
        await speedEventually { attempts.all.first?.transport.connectCount == 1 }
        let primary = attempts.all[0].transport
        primary.onConnected?()
        registry.sendInput(sessionId: "speed-healthy", data: Data("one".utf8))
        XCTAssertEqual(registry.existingSession(sessionId: "speed-healthy")?.currentState, .connected)
        XCTAssertEqual(primary.sentInput, [Data("one".utf8)])
        XCTAssertEqual(primary.pinnedCertificate, Data([1, 2, 3]))
        XCTAssertEqual(attempts.all.map(\.kind), [.webSocket])
    }

    func testSequentialAlternateUsesRealHostAndFailedQUICReturnsToWSS() async {
        let attempts = SpeedAttempts()
        let registry = makeLiveRegistry(attempts)
        defer { registry.removeAllSessions() }
        let sid = "speed-failover"
        let relay = registry.outputRelay(sessionId: sid)
        XCTAssertTrue(attempts.all.isEmpty)
        _ = registry.open(sessionId: sid, hubHost: "quic.real-host.invalid")
        await speedEventually { attempts.all.first?.transport.connectCount == 1 }
        let primary = attempts.all[0].transport
        primary.onConnected?()
        primary.onOutputReceived?(Data("abc".utf8), 3)
        primary.onDisconnected?()
        XCTAssertEqual(primary.disconnectCount, 1)
        registry.existingSession(sessionId: sid)?.retryImmediatelyIfWaiting()
        await speedEventually { attempts.all.count == 2 && attempts.all[1].transport.connectCount == 1 }
        guard attempts.all.count == 2 else { return }
        let alternate = attempts.all[1].transport
        XCTAssertEqual(attempts.all[1].kind, .quic)
        XCTAssertEqual(attempts.all[1].host, "quic.real-host.invalid")
        XCTAssertEqual(alternate.resumeFrom, 3)
        alternate.onConnected?()
        registry.sendInput(sessionId: sid, data: Data("once".utf8))
        XCTAssertTrue(primary.sentInput.isEmpty)
        XCTAssertEqual(alternate.sentInput, [Data("once".utf8)])
        primary.onOutputReceived?(Data("stale".utf8), 999)
        primary.onDisconnected?()
        XCTAssertEqual(registry.existingSession(sessionId: sid)?.currentState, .connected)

        alternate.onDisconnected?()
        XCTAssertEqual(alternate.disconnectCount, 1)
        registry.existingSession(sessionId: sid)?.retryImmediatelyIfWaiting()
        await speedEventually { attempts.all.count == 3 && attempts.all[2].transport.connectCount == 1 }
        guard attempts.all.count == 3 else { return }
        XCTAssertEqual(attempts.all.map(\.kind), [.webSocket, .quic, .webSocket])
        XCTAssertEqual(attempts.all[2].transport.resumeFrom, 3)
        XCTAssertTrue(registry.outputRelay(sessionId: sid) === relay)
    }

    func testForegroundRetainsWorkingWSSAndDoesNotOpenUnusedRelay() async {
        let attempts = SpeedAttempts()
        let registry = makeLiveRegistry(attempts)
        defer { registry.removeAllSessions() }
        _ = registry.outputRelay(sessionId: "speed-unused")
        registry.applicationDidEnterBackground()
        registry.applicationWillEnterForeground()
        XCTAssertTrue(attempts.all.isEmpty)
        _ = registry.open(sessionId: "speed-resume", hubHost: "quic.example.invalid")
        await speedEventually { attempts.all.first?.transport.connectCount == 1 }
        attempts.all[0].transport.onConnected?()
        registry.applicationDidEnterBackground()
        XCTAssertEqual(attempts.all[0].transport.disconnectCount, 1)
        registry.applicationWillEnterForeground()
        await speedEventually { attempts.all.count == 2 && attempts.all[1].transport.connectCount == 1 }
        XCTAssertEqual(attempts.all.map(\.kind), [.webSocket, .webSocket])
    }

    func testAuthorizationFailuresNeverActivateEitherLiveTransport() async {
        for status in [401, 403] {
            let attempts = SpeedAttempts()
            let registry = TerminalConnectionRegistry.live(
                monitorsNetwork: false,
                makeWebSocket: { attempts.make(.webSocket) },
                makeQUIC: { attempts.make(.quic, host: $0) },
                ticketProvider: { _ in throw HubError.httpError(status) }
            )
            _ = registry.open(sessionId: "speed-auth", hubHost: "quic.example.invalid")
            await speedEventually { registry.existingSession(sessionId: "speed-auth")?.currentState.needsUserAction == true }
            registry.existingSession(sessionId: "speed-auth")?.retryImmediatelyIfWaiting()
            registry.applicationDidEnterBackground()
            registry.applicationWillEnterForeground()
            XCTAssertEqual(attempts.all.map(\.kind), [.webSocket])
            XCTAssertEqual(attempts.all.first?.transport.connectCount, 0)
            XCTAssertTrue(registry.existingSession(sessionId: "speed-auth")?.currentState.needsUserAction == true)
            registry.removeAllSessions()
        }
    }

    private func makeLiveRegistry(_ attempts: SpeedAttempts) -> TerminalConnectionRegistry {
        TerminalConnectionRegistry.live(
            monitorsNetwork: false,
            makeWebSocket: { attempts.make(.webSocket) },
            makeQUIC: { attempts.make(.quic, host: $0) },
            ticketProvider: { _ in
                SessionTicketResponse(ticket: "unit-test-ticket", quicCertificate: "AQID")
            }
        )
    }
}

private func speedEventually(
    file: StaticString = #filePath, line: UInt = #line,
    _ condition: @Sendable () -> Bool
) async {
    for _ in 0..<1_000 {
        if condition() { return }
        try? await Task.sleep(for: .milliseconds(1))
    }
    XCTFail("Connection state did not settle", file: file, line: line)
}

private enum SpeedTransportKind: Equatable, Sendable { case webSocket, quic }

private final class SpeedAttempts: @unchecked Sendable {
    struct Attempt: Sendable {
        let kind: SpeedTransportKind
        let host: String
        let transport: SpeedTransport
    }
    private let lock = NSLock()
    private var attempts: [Attempt] = []
    var all: [Attempt] { lock.filWithLock { attempts } }

    func make(_ kind: SpeedTransportKind, host: String = "") -> SpeedTransport {
        let transport = SpeedTransport()
        lock.filWithLock { attempts.append(Attempt(kind: kind, host: host, transport: transport)) }
        return transport
    }
}

/// No network or Keychain access. Callback registration happens before the
/// session publishes the transport; operational state is protected by lock.
private final class SpeedTransport: TerminalTransport, @unchecked Sendable {
    var onOutputReceived: (@Sendable (Data, UInt64) -> Void)?
    var onConnected: (@Sendable () -> Void)?
    var onDisconnected: (@Sendable () -> Void)?
    var onBetterPathAvailable: (@Sendable () -> Void)?

    private let lock = NSLock()
    private var connections = 0
    private var disconnections = 0
    private var input: [Data] = []
    private var cursor: UInt64 = 0
    private var certificate = Data()

    var connectCount: Int { lock.filWithLock { connections } }
    var disconnectCount: Int { lock.filWithLock { disconnections } }
    var sentInput: [Data] { lock.filWithLock { input } }
    var resumeFrom: UInt64 { lock.filWithLock { cursor } }
    var pinnedCertificate: Data { lock.filWithLock { certificate } }

    func connect(sessionId: String, ticket: String?, resumeFrom: UInt64) {
        lock.filWithLock { connections += 1; cursor = resumeFrom }
    }
    func setServerCertificate(_ certificate: Data) { lock.filWithLock { self.certificate = certificate } }
    func disconnect() { lock.filWithLock { disconnections += 1 } }
    func sendInput(_ data: Data) { lock.filWithLock { input.append(data) } }
    func sendResize(cols: UInt16, rows: UInt16) {}
}
