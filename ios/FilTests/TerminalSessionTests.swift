import XCTest
import Network
import SwiftTerm
import ComposableArchitecture

@testable import Fil

@MainActor
final class TerminalScrollRetentionTests: XCTestCase {
    private func populatedView(scrollback: Int = 1000, count: Int = 200) -> FilTerminalView {
        let view = FilTerminalView(frame: CGRect(x: 0, y: 0, width: 390, height: 600))
        view.changeScrollback(scrollback)
        let bytes = Array((1...count).map { "line-\($0)\r\n" }.joined().utf8)
        view.receiveOutput(bytes[...])
        return view
    }

    func testNewOutputDoesNotPullHistoryReaderBackToBottom() async {
        let view = FilTerminalView(frame: CGRect(x: 0, y: 0, width: 390, height: 600))
        view.feed(text: (1...200).map { "line-\($0)\r\n" }.joined())
        await Task.yield()
        let position = view.contentSize.height - view.bounds.height - 350
        view.setContentOffset(CGPoint(x: 0, y: position), animated: false)
        let relay = TerminalOutputRelay()
        relay.attach(view)
        relay.enqueue(Data("new-output\r\n".utf8))
        try? await Task.sleep(for: .milliseconds(60))
        XCTAssertEqual(view.contentOffset.y, position, accuracy: 1,
                       "Reading history must survive new output")
    }

    func testOutputStillFollowsWhenAlreadyAtBottom() async {
        let view = FilTerminalView(frame: CGRect(x: 0, y: 0, width: 390, height: 600))
        view.feed(text: (1...200).map { "line-\($0)\r\n" }.joined())
        await Task.yield()
        let before = view.contentOffset.y
        let relay = TerminalOutputRelay()
        relay.attach(view)
        relay.enqueue(Data("new-output\r\n".utf8))
        try? await Task.sleep(for: .milliseconds(60))
        XCTAssertGreaterThan(view.contentOffset.y, before)
    }

    func testManyOutputBatchesKeepTheSameReadingPosition() {
        let view = populatedView()
        let position = view.contentSize.height - view.bounds.height - 350
        view.contentOffset.y = position
        for _ in 0..<100 { view.receiveOutput(Array("live\r\n".utf8)[...]) }
        XCTAssertEqual(view.contentOffset.y, position, accuracy: 1)
        XCTAssertTrue(view.isReadingHistory)
    }

    func testBackToLiveReEnablesOutputFollowing() {
        let view = populatedView()
        view.contentOffset.y = 400
        XCTAssertTrue(view.isReadingHistory)
        view.scrollToLatest()
        XCTAssertFalse(view.isReadingHistory)
        let before = view.contentOffset.y
        view.receiveOutput(Array("live\r\n".utf8)[...])
        XCTAssertGreaterThan(view.contentOffset.y, before)
        XCTAssertFalse(view.isReadingHistory)
    }

    func testCircularBufferTrimmingKeepsARetainedLineAnchored() {
        let view = populatedView(scrollback: 100, count: 200)
        let rowHeight = ceil(view.font.ascender - view.font.descender + view.font.leading)
        let before = view.contentSize.height - view.bounds.height - 150
        view.contentOffset.y = before
        view.receiveOutput(Array("one\r\ntwo\r\nthree\r\n".utf8)[...])
        XCTAssertEqual(view.contentOffset.y, before - 3 * rowHeight, accuracy: 1)
    }

    func testDiscardedHistoryClampsToOldestRemainingRow() {
        let view = populatedView(scrollback: 100, count: 200)
        view.contentOffset.y = 20
        view.receiveOutput(Array(String(repeating: "replacement\r\n", count: 250).utf8)[...])
        XCTAssertEqual(view.contentOffset.y, 0, accuracy: 1)
    }

    func testAlternateScreenIsNeverPinnedToNormalHistory() {
        let view = populatedView()
        view.contentOffset.y = 400
        view.receiveOutput(Array("\u{1b}[?1049h\u{1b}[Halternate".utf8)[...])
        XCTAssertTrue(view.getTerminal().isCurrentBufferAlternate)
        XCTAssertFalse(view.isReadingHistory)
        XCTAssertLessThanOrEqual(view.contentOffset.y, 1)
        view.receiveOutput(Array("\u{1b}[?1049l".utf8)[...])
        XCTAssertFalse(view.getTerminal().isCurrentBufferAlternate)
        XCTAssertFalse(view.isReadingHistory)
    }

    func testKeyboardHeightChangeKeepsHistoryPosition() async {
        let view = populatedView()
        view.contentOffset.y = 400
        view.frame.size.height = 320
        view.setNeedsLayout()
        view.layoutIfNeeded()
        try? await Task.sleep(for: .milliseconds(60))
        XCTAssertEqual(view.contentOffset.y, 400, accuracy: 1)
    }

    func testFontChangePreservesTheReadingRow() async {
        let view = populatedView()
        let height = ceil(view.font.ascender - view.font.descender + view.font.leading)
        view.contentOffset.y = height * 20
        view.preservingViewport {
            view.font = .monospacedSystemFont(ofSize: 20, weight: .regular)
        }
        view.layoutIfNeeded()
        try? await Task.sleep(for: .milliseconds(60))
        let newHeight = ceil(view.font.ascender - view.font.descender + view.font.leading)
        XCTAssertEqual(view.contentOffset.y / newHeight, 20, accuracy: 0.1)
    }
}

final class LiveActivityPresentationTests: XCTestCase {
    private func display(_ privacy: FilActivityPrivacy, stale: Bool = false, status: FilActivityStatus = .connected, revision: String = "current") -> FilActivityPresentation {
        let attributes = FilActivityAttributes(sessionId: "fixture", deviceId: "fixture-device", machineName: "Sensitive machine", startedAt: Date(timeIntervalSince1970: 100), privacy: .detailed, privacyRevision: revision)
        let state = FilActivityAttributes.ContentState(status: status, projectName: "/secret/client/project", shell: "Sensitive process", otherSessionCount: 7, lastUpdatedAt: Date(timeIntervalSince1970: 200))
        return FilActivityProjection.presentation(attributes: attributes, state: state, isStale: stale, privacy: privacy, privacyRevision: "current")
    }

    func testPrivatePresentationRedactsDelayedPushMetadata() {
        let result = display(.private_)
        XCTAssertEqual(result.machineName, "")
        XCTAssertEqual(result.shell, "")
        XCTAssertFalse(result.projectName.contains("project"))
        XCTAssertFalse(result.projectName.contains("secret"))
    }

    func testStaleContextOverridesConnectedWithoutInventingFreshness() {
        let result = display(.detailed, stale: true)
        XCTAssertEqual(result.status, .stale)
        XCTAssertEqual(result.lastUpdatedAt, Date(timeIntervalSince1970: 200))
    }

    func testStoppedFollowingNeverMeansShellEnded() {
        XCTAssertEqual(display(.private_, stale: true, status: .stopped).status, .stopped)
        XCTAssertEqual(display(.private_, stale: true, status: .ended).status, .ended)
    }

    func testOldPrivacyRevisionCannotRestoreDetails() {
        let result = display(.detailed, revision: "previous")
        XCTAssertEqual(result.machineName, "")
        XCTAssertEqual(result.shell, "")
        XCTAssertFalse(result.projectName.contains("project"))
    }

    func testStandardPresentationOnlyShowsShortProjectAndMachine() {
        let result = display(.standard)
        XCTAssertEqual(result.machineName, "Sensitive machine")
        XCTAssertEqual(result.projectName, "project")
        XCTAssertEqual(result.shell, "")
    }
}

#if DEBUG && targetEnvironment(simulator)
final class SimulatorLaunchConfigurationTests: XCTestCase {
    private let credentials = [
        "FIL_TEST_HUB_URL": "https://hub.example.invalid",
        "FIL_TEST_TOKEN": "unit-test-token-not-a-credential"
    ]

    func testAbsentOrPartialEnvironmentDoesNotConfigureLaunch() {
        XCTAssertNil(SimulatorLaunchConfiguration(environment: [:]))
        XCTAssertNil(SimulatorLaunchConfiguration(environment: ["FIL_TEST_HUB_URL": "https://hub.example.invalid"]))
        XCTAssertNil(SimulatorLaunchConfiguration(environment: ["FIL_TEST_TOKEN": "unit-test-token"]))
    }

    func testMachinesLaunchDoesNotSelectAnySession() throws {
        let configuration = try XCTUnwrap(SimulatorLaunchConfiguration(environment: credentials))
        XCTAssertEqual(configuration.hubURL, "https://hub.example.invalid")
        XCTAssertNil(configuration.sessionURL)
    }

    func testScratchSessionUsesExistingDeepLinkRoute() throws {
        var environment = credentials
        environment["FIL_TEST_SESSION_ID"] = "qa-scratch_123"
        let configuration = try XCTUnwrap(SimulatorLaunchConfiguration(environment: environment))
        XCTAssertEqual(configuration.sessionURL?.scheme, "fil")
        XCTAssertEqual(configuration.sessionURL?.host, "session")
        XCTAssertEqual(configuration.sessionURL?.pathComponents.last, "qa-scratch_123")
    }

    func testInsecureOrAmbiguousHubAndEmptyTokenAreRejected() {
        for hub in ["http://hub.example.invalid", "https://user:pass@hub.example.invalid", "https://hub.example.invalid?token=wrong", "https://hub.example.invalid#fragment", "not a URL"] {
            var environment = credentials
            environment["FIL_TEST_HUB_URL"] = hub
            XCTAssertNil(SimulatorLaunchConfiguration(environment: environment))
        }
        for token in ["", " ", "unit-test-token\n"] {
            var environment = credentials
            environment["FIL_TEST_TOKEN"] = token
            XCTAssertNil(SimulatorLaunchConfiguration(environment: environment))
        }
    }

    func testInvalidSessionDoesNotBecomeAnotherDeepLink() {
        for sessionId in ["", "../other", "scratch/other", "scratch?other", "scratch#other", "scratch other"] {
            var environment = credentials
            environment["FIL_TEST_SESSION_ID"] = sessionId
            XCTAssertNil(SimulatorLaunchConfiguration(environment: environment))
        }
    }
}
#endif

@MainActor
final class TerminalFollowTests: XCTestCase {
    func testDismissClosesTerminalBeforePresentationRemovesReducer() async {
        let closed = TestValue([String]())
        let store = TestStore(initialState: TerminalFeature.State(session: session("A"))) {
            TerminalFeature()
        } withDependencies: {
            $0.terminalClient.close = { id in closed.update { $0.append(id) } }
        }
        await store.send(.dismiss) { $0.followRevision = 1 }
        XCTAssertEqual(closed.value, ["A"])
    }

    func testLateConnectionResultForPreviousSessionIsIgnored() async {
        let store = TestStore(initialState: TerminalFeature.State(session: session("B"))) { TerminalFeature() }
        await store.send(.connectionStateChanged(sessionId: "A", state: .connected))
        XCTAssertFalse(store.state.isConnected)
    }

    private func session(_ id: String) -> Session {
        Session(id: id, deviceId: "test-device", shell: "zsh", command: nil, cwd: "/tmp",
                cols: 80, rows: 24, status: .online, createdAt: nil)
    }

    func testLateFollowResultsForAnotherSessionAreIgnored() async {
        var state = TerminalFeature.State(session: session("B"))
        state.isFollowRequestInFlight = true
        state.followRevision = 2
        let store = TestStore(initialState: state) { TerminalFeature() }
        await store.send(.followingChanged(sessionId: "A", revision: 2, isFollowing: true, requestedFollow: true))
        await store.send(.followingStatusLoaded(sessionId: "A", revision: 2, isFollowing: true))
        XCTAssertTrue(store.state.isFollowRequestInFlight)
        XCTAssertFalse(store.state.isFollowing)
    }

    func testOldResultsForSameSessionCannotOverwriteNewerOperation() async {
        var state = TerminalFeature.State(session: session("A"))
        state.followRevision = 3
        let store = TestStore(initialState: state) { TerminalFeature() }
        await store.send(.followingChanged(sessionId: "A", revision: 1, isFollowing: true, requestedFollow: true))
        await store.send(.followingStatusLoaded(sessionId: "A", revision: 2, isFollowing: true))
        XCTAssertFalse(store.state.isFollowing)
    }

    func testForegroundRefreshReconcilesSystemDismissal() async {
        var state = TerminalFeature.State(session: session("A"))
        state.isFollowing = true
        let store = TestStore(initialState: state) { TerminalFeature() } withDependencies: {
            $0.terminalFollowClient.isFollowing = { _ in false }
        }
        await store.send(.refreshFollowingStatus) { $0.followRevision = 1 }
        await store.receive(.followingStatusLoaded(sessionId: "A", revision: 1, isFollowing: false)) {
            $0.isFollowing = false
        }
    }

    func testSwitchResetsFollowSpinnerAndClosesPreviousBeforeOpeningNew() async {
        let a = session("A")
        let b = session("B")
        var state = TerminalFeature.State(session: a, availableSessions: [
            .init(session: a, machineName: "one"), .init(session: b, machineName: "two")
        ])
        state.isFollowRequestInFlight = true
        state.showLiveActivityUnavailableAlert = true
        let events = TestValue([String]())
        let store = TestStore(initialState: state) { TerminalFeature() } withDependencies: {
            $0.terminalClient.close = { id in events.update { $0.append("close-\(id)") } }
            $0.terminalClient.open = { id, _ in
                events.update { $0.append("open-\(id)") }
                return AsyncStream { $0.finish() }
            }
        }
        await store.send(.switchSession("B")) {
            $0.session = b
            $0.machineName = "two"
            $0.otherSessionCount = 1
            $0.isFollowRequestInFlight = false
            $0.showLiveActivityUnavailableAlert = false
            $0.followRevision = 1
        }
        XCTAssertEqual(events.value.first, "close-A")
        await store.receive(.onAppear) { $0.followRevision = 2 }
        await store.receive(.followingStatusLoaded(sessionId: "B", revision: 2, isFollowing: false))
        await store.finish()
        XCTAssertEqual(events.value, ["close-A", "open-B"])
    }
}

final class SessionEventCancellationTests: XCTestCase {
    func testCancellingStreamClosesSocketWhileReceiveIsPending() async {
        let socket = FakeWebSocket()
        let stream = SessionEventClient.updates(makeSocket: { socket })
        let consumer = Task { for await _ in stream {} }
        await eventually { socket.hasReceiver }
        consumer.cancel()
        await consumer.value
        await eventually { socket.cancels > 0 }
        // Cleanup also makes a red run release the intentionally stuck receive.
        socket.cancel(with: .goingAway, reason: nil)
    }
}

private final class FakeWebSocket: TerminalWebSocketTask, @unchecked Sendable {
    typealias Receiver = @Sendable (Result<URLSessionWebSocketTask.Message, Error>) -> Void
    private struct State {
        var receivers: [Receiver] = []
        var sent: [Data] = []
        var completions: [@Sendable (Error?) -> Void] = []
        var resumes = 0
        var cancels = 0
    }
    private let state = TestValue(State())
    var hasReceiver: Bool { !state.value.receivers.isEmpty }
    var frames: [Data] { state.value.sent }
    var resumes: Int { state.value.resumes }
    var cancels: Int { state.value.cancels }
    var lastCompletion: (@Sendable (Error?) -> Void)? { state.value.completions.last }
    func resume() { state.update { $0.resumes += 1 } }
    func cancel(with closeCode: URLSessionWebSocketTask.CloseCode, reason: Data?) {
        let receivers = state.update {
            $0.cancels += 1
            let receivers = $0.receivers
            $0.receivers.removeAll()
            return receivers
        }
        for receive in receivers { receive(.failure(URLError(.cancelled))) }
    }
    func send(_ message: URLSessionWebSocketTask.Message, completionHandler: @escaping @Sendable (Error?) -> Void) {
        state.update {
            if case .data(let data) = message { $0.sent.append(data) }
            $0.completions.append(completionHandler)
        }
    }
    func receive(completionHandler: @escaping Receiver) {
        let closed = state.update {
            if $0.cancels > 0 { return true }
            $0.receivers.append(completionHandler)
            return false
        }
        if closed { completionHandler(.failure(URLError(.cancelled))) }
    }
    func sendPing(pongReceiveHandler: @escaping @Sendable (Error?) -> Void) { pongReceiveHandler(nil) }
    func takeReceiver() -> Receiver? {
        state.update { $0.receivers.isEmpty ? nil : $0.receivers.removeFirst() }
    }
    func deliver(_ data: Data) { takeReceiver()?(.success(.data(data))) }
}

final class WebSocketTerminalClientTests: XCTestCase {
    func testRequestUsesWSSAndBearerHeaderWithoutCredentialInURL() throws {
        let request = try XCTUnwrap(WebSocketTerminalClient.request(
            hubURL: "https://hub.example.invalid:443/ignored?old=1#fragment",
            token: "test-only-token", sessionId: "sid", resumeFrom: UInt64.max
        ))
        XCTAssertEqual(request.url?.absoluteString, "wss://hub.example.invalid:443/ws/data/sid?role=client&resume_from=18446744073709551615")
        XCTAssertEqual(request.value(forHTTPHeaderField: "Authorization"), "Bearer test-only-token")
        for base in ["http://hub.example.invalid", "ws://hub.example.invalid", "https://user:pass@hub.example.invalid"] {
            XCTAssertNil(WebSocketTerminalClient.request(hubURL: base, token: "test", sessionId: "sid", resumeFrom: 0))
        }
        XCTAssertNil(WebSocketTerminalClient.request(hubURL: "https://hub.example.invalid", token: nil, sessionId: "sid", resumeFrom: 0))
    }

    func testFragmentedPrefixBinaryFramingAndLateCallbacksAfterClose() async {
        let socket = FakeWebSocket()
        let client = WebSocketTerminalClient(hubURL: "https://hub.example.invalid", token: "test", detachTimeout: 0.03, makeSocket: { _ in socket })
        let connected = TestValue(0)
        let outputs = TestValue([(Data, UInt64)]())
        client.onConnected = { connected.update { $0 += 1 } }
        client.onOutputReceived = { data, offset in outputs.update { $0.append((data, offset)) } }
        client.sendResize(cols: 90, rows: 30)
        client.sendInput(Data("x".utf8))
        client.connect(sessionId: "sid", ticket: nil, resumeFrom: 80)
        await eventually { socket.hasReceiver }
        XCTAssertEqual(connected.value, 0)
        XCTAssertTrue(socket.frames.isEmpty)
        socket.deliver(Data(cursorBytes(80).prefix(5)))
        await eventually { socket.hasReceiver }
        XCTAssertEqual(connected.value, 0)
        socket.deliver(Data(cursorBytes(80).dropFirst(5)) + Data("ab".utf8))
        await eventually { socket.frames.count == 2 && socket.hasReceiver }
        XCTAssertEqual(connected.value, 1)
        XCTAssertEqual(outputs.value.last?.0, Data("ab".utf8))
        XCTAssertEqual(outputs.value.last?.1, 82)
        XCTAssertEqual(socket.frames, [Data([1, 0, 90, 0, 30]), Data([0, 0, 0, 0, 1, 120])])
        socket.deliver(Data("cd".utf8))
        await eventually { outputs.value.last?.1 == 84 && socket.hasReceiver }
        let lateRead = socket.takeReceiver()
        let finished = TestValue(0)
        client.disconnect { finished.update { $0 += 1 } }
        await eventually { finished.value == 1 }
        socket.lastCompletion?(nil)
        lateRead?(.success(.data(Data("stale".utf8))))
        let drained = TestValue(false)
        client.disconnect { drained.update { $0 = true } }
        await eventually { drained.value }
        XCTAssertEqual(outputs.value.last?.1, 84)
        XCTAssertEqual(connected.value, 1)
        XCTAssertEqual(socket.frames.last, Data([2]))
        XCTAssertEqual(socket.cancels, 1)
        XCTAssertEqual(finished.value, 1)
    }

    func testMissingAuthorizationCannotCreateSocket() async {
        let made = TestValue(0)
        let failures = TestValue(0)
        let client = WebSocketTerminalClient(hubURL: "https://hub.example.invalid", token: nil, makeSocket: { _ in
            made.update { $0 += 1 }
            return FakeWebSocket()
        })
        client.onDisconnected = { failures.update { $0 += 1 } }
        client.connect(sessionId: "sid", ticket: nil, resumeFrom: 0)
        await eventually { failures.value == 1 }
        XCTAssertEqual(made.value, 0)
    }

    @MainActor
    func testQUICFailoverAndForegroundKeepRelayAndExactCursor() async {
        let quic = FakeTransport()
        let quicFactories = TestValue(0)
        let sockets = TestValue([FakeWebSocket]())
        let requests = TestValue([URLRequest]())
        let registry = TerminalConnectionRegistry(
            monitorsNetwork: false,
            makeTransport: { _, _ in quicFactories.update { $0 += 1 }; return quic },
            makeFallbackTransport: { _ in
                WebSocketTerminalClient(hubURL: "https://hub.example.invalid", token: "test-only", detachTimeout: 0.03, makeSocket: { request in
                    let socket = FakeWebSocket()
                    requests.update { $0.append(request) }
                    sockets.update { $0.append(socket) }
                    return socket
                })
            },
            ticketProvider: { _ in SessionTicketResponse(ticket: "ticket", quicCertificate: "AQID") }
        )
        let relay = registry.outputRelay(sessionId: "sid")
        let view = SwiftTerm.TerminalView(frame: CGRect(x: 0, y: 0, width: 400, height: 600))
        relay.attach(view)
        _ = registry.open(sessionId: "sid", hubHost: "quic.example.invalid")
        await eventually { quic.connectCount == 1 }
        quic.becomeConnected()
        quic.deliver("abc")
        quic.die()
        await eventually { sockets.value.first?.hasReceiver == true }
        XCTAssertEqual(requests.value[0].url?.query, "role=client&resume_from=3")
        sockets.value[0].deliver(cursorBytes(3) + Data("def".utf8))
        await eventually { registry.existingSession(sessionId: "sid")?.currentState == .connected }
        await withCheckedContinuation { c in DispatchQueue.main.async { c.resume() } }
        XCTAssertTrue(String(decoding: view.getTerminal().getBufferAsData(), as: UTF8.self).contains("abcdef"))
        XCTAssertTrue(registry.outputRelay(sessionId: "sid") === relay)
        let detached = TestValue(false)
        registry.applicationDidEnterBackground { detached.update { $0 = true } }
        await eventually { detached.value }
        XCTAssertEqual(sockets.value[0].frames.last, Data([2]))
        XCTAssertEqual(sockets.value[0].cancels, 1)
        registry.applicationWillEnterForeground()
        await eventually { sockets.value.count == 2 && sockets.value[1].hasReceiver }
        XCTAssertEqual(requests.value[1].url?.query, "role=client&resume_from=6")
        XCTAssertEqual(quicFactories.value, 1, "foreground must not retry a known-blocked UDP route")
        registry.close(sessionId: "sid")
    }

    func testFirstQUICWatchdogFailureSelectsFallbackWithoutUserAction() async {
        let sleeper = ManualSleeper()
        let quic = FakeTransport()
        let fallback = FakeTransport()
        let fallbackFactories = TestValue(0)
        let session = TerminalSession(
            sessionId: "sid", makeTransport: { _ in quic },
            makeFallbackTransport: { _ in fallbackFactories.update { $0 += 1 }; return fallback },
            mintTicket: { _ in nil }, sleep: { try await sleeper.sleep($0) }
        )
        session.connectIfNeeded()
        await eventually { quic.connectCount == 1 && sleeper.count == 1 }
        sleeper.releaseAll()
        await eventually { quic.disconnectCount == 1 && sleeper.count == 1 }
        sleeper.releaseAll()
        await eventually { fallback.connectCount == 1 && sleeper.count == 1 }
        fallback.becomeConnected()
        XCTAssertEqual(session.currentState, .connected)
        XCTAssertEqual(fallbackFactories.value, 1)
        session.shutdown()
        sleeper.releaseAll()
    }
}


// MARK: - Deterministic wire and lifecycle regression harness

private func cursorBytes(_ cursor: UInt64) -> Data {
    var value = cursor.bigEndian
    return Data(bytes: &value, count: 8)
}

private func eventually(
    file: StaticString = #filePath, line: UInt = #line,
    _ predicate: @Sendable () -> Bool
) async {
    for _ in 0..<2_000 {
        if predicate() { return }
        try? await Task.sleep(for: .milliseconds(1))
    }
    XCTFail("Condition did not become true", file: file, line: line)
}

/// Deliberately cancellation-insensitive: proves stale work is harmless even
/// when an underlying callback API cannot be cancelled.
private final class ManualSleeper: @unchecked Sendable {
    private let pending = TestValue([(TimeInterval, CheckedContinuation<Void, Error>)]())
    func sleep(_ duration: TimeInterval) async throws {
        try await withCheckedThrowingContinuation { continuation in
            pending.update { $0.append((duration, continuation)) }
        }
    }
    var count: Int { pending.value.count }
    var retryCount: Int { pending.value.filter { $0.0 != ReconnectPolicy.connectTimeout }.count }
    func releaseRetries() {
        let waits = pending.update { entries in
            let retries = entries.filter { $0.0 != ReconnectPolicy.connectTimeout }
            entries.removeAll { $0.0 != ReconnectPolicy.connectTimeout }
            return retries
        }
        for (_, continuation) in waits { continuation.resume() }
    }
    func releaseAll() {
        let waits = pending.update { let waits = $0; $0.removeAll(); return waits }
        for (_, continuation) in waits { continuation.resume() }
    }
}

final class TerminalTicketRecoveryTests: XCTestCase {
    /// Scripted HTTPS results, with a manually advanced retry clock. Nil
    /// means a valid authenticated credential; nothing uses the network.
    private final class Harness: Sendable {
        let sleeper = ManualSleeper()
        let recorder = TransportRecorder()
        let calls = TestValue(0)
        let session: TerminalSession

        init(status: @escaping @Sendable (Int) -> Int?) {
            let sleeper = sleeper
            let recorder = recorder
            let calls = calls
            session = TerminalSession(
                sessionId: "unit-test-404-budget",
                makeTransport: { _ in let t = FakeTransport(); recorder.record(t); return t },
                mintTicket: { _ in
                    let call = calls.update { $0 += 1; return $0 }
                    if let code = status(call) { throw HubError.httpError(code) }
                    return SessionTicketResponse(ticket: "unit-test-ticket", quicCertificate: "AQID")
                },
                sleep: { try await sleeper.sleep($0) }
            )
        }

        func waitForRetry(after call: Int) async {
            let session = session
            let calls = calls
            let sleeper = sleeper
            await eventually {
                calls.value == call && sleeper.count >= call + 1 && sleeper.retryCount == 1
                    && !session.currentState.needsUserAction && session.currentState != .connecting
            }
        }

        func close() { session.shutdown(); sleeper.releaseAll() }
    }

    func testMissingSessionThenValidTicketReconnectsWithoutManualTap() async {
        let sleeper = ManualSleeper()
        let recorder = TransportRecorder()
        let calls = TestValue(0)
        let session = TerminalSession(
            sessionId: "unit-test-rollout",
            makeTransport: { _ in let t = FakeTransport(); recorder.record(t); return t },
            mintTicket: { _ in
                let call = calls.update { $0 += 1; return $0 }
                if call == 1 { throw HubError.httpError(404) }
                return SessionTicketResponse(ticket: "unit-test-ticket", quicCertificate: "AQID")
            },
            sleep: { try await sleeper.sleep($0) }
        )
        defer { session.shutdown(); sleeper.releaseAll() }
        session.connectIfNeeded()
        await eventually { calls.value == 1 && session.currentState != .connecting && sleeper.count >= 1 }
        XCTAssertEqual(session.currentState, .reconnecting(attempt: 1), "A rollout's temporary 404 must not require a tap")
        guard !session.currentState.needsUserAction else { return }
        await eventually { sleeper.retryCount == 1 }
        sleeper.releaseRetries()
        await eventually { calls.value == 2 && recorder.all.last?.connectCount == 1 && sleeper.count >= 2 }
        recorder.all.last?.becomeConnected()
        XCTAssertEqual(session.currentState, .connected)
        XCTAssertEqual(recorder.all.last?.lastTicket, "unit-test-ticket")
        XCTAssertEqual(recorder.all.last?.certificate, Data([1, 2, 3]))
    }

    func testRepeated404StopsAtBudgetAndLifecycleCannotRestartIt() async {
        let h = Harness { _ in 404 }
        defer { h.close() }
        h.session.connectIfNeeded()
        for call in 1..<ReconnectPolicy.maxMissingSessionResponses {
            await h.waitForRetry(after: call)
            h.sleeper.releaseRetries()
        }
        await eventually { h.calls.value == 8 && h.session.currentState.needsUserAction && h.sleeper.count >= 8 }
        XCTAssertEqual(h.calls.value, 8)
        XCTAssertEqual(h.sleeper.retryCount, 0)
        h.session.retryImmediatelyIfWaiting()
        h.session.connectIfNeeded()
        h.session.suspend()
        h.session.resume()
        h.sleeper.releaseAll()
        XCTAssertTrue(h.session.currentState.needsUserAction)
        XCTAssertEqual(h.recorder.all.count, 8)
        XCTAssertTrue(h.recorder.all.allSatisfy { $0.connectCount == 0 })
    }

    func testPathAndForegroundRetriesDoNotReset404Budget() async {
        let h = Harness { _ in 404 }
        defer { h.close() }
        h.session.connectIfNeeded()
        for call in 1..<ReconnectPolicy.maxMissingSessionResponses {
            // Retain cancelled waits so this also exercises late timer wakes.
            await eventually { h.calls.value == call && h.session.currentState != .connecting && h.sleeper.count >= call * 2 }
            if call.isMultiple(of: 2) {
                h.session.suspend()
                h.session.resume()
            } else {
                h.session.retryImmediatelyIfWaiting()
            }
        }
        await eventually { h.calls.value == 8 && h.session.currentState.needsUserAction && h.sleeper.count >= 15 }
        h.sleeper.releaseAll()
        XCTAssertTrue(h.session.currentState.needsUserAction)
        XCTAssertEqual(h.recorder.all.count, 8)
    }

    func testActualConnectionResets404Budget() async {
        let h = Harness { call in call == 8 ? nil : 404 }
        defer { h.close() }
        h.session.connectIfNeeded()
        for call in 1..<8 {
            await h.waitForRetry(after: call)
            h.sleeper.releaseRetries()
        }
        await eventually { h.recorder.all.last?.connectCount == 1 && h.sleeper.count >= 8 }
        h.recorder.all.last?.becomeConnected()
        XCTAssertEqual(h.session.currentState, .connected)
        h.recorder.all.last?.die()
        await h.waitForRetry(after: 8)
        h.sleeper.releaseRetries()
        await h.waitForRetry(after: 9)
        XCTAssertEqual(h.session.currentState, .reconnecting(attempt: 2))
    }

    func testManualReconnectResetsExhausted404Budget() async {
        let h = Harness { _ in 404 }
        defer { h.close() }
        h.session.connectIfNeeded()
        for call in 1..<8 {
            await h.waitForRetry(after: call)
            h.sleeper.releaseRetries()
        }
        await eventually { h.session.currentState.needsUserAction && h.sleeper.count >= 8 }
        h.session.reconnectNow()
        await h.waitForRetry(after: 9)
        XCTAssertEqual(h.session.currentState, .reconnecting(attempt: 1))
    }

    func testAuthenticationFailureDuring404RecoveryRemainsTerminal() async {
        for status in [401, 403] {
            let h = Harness { call in call == 1 ? 404 : (call == 2 ? status : nil) }
            h.session.connectIfNeeded()
            await h.waitForRetry(after: 1)
            h.sleeper.releaseRetries()
            await eventually { h.calls.value == 2 && h.session.currentState.needsUserAction && h.sleeper.count >= 2 }
            h.session.retryImmediatelyIfWaiting()
            h.session.suspend()
            h.session.resume()
            h.recorder.all.last?.becomeConnected()
            h.sleeper.releaseAll()
            XCTAssertTrue(h.session.currentState.needsUserAction)
            XCTAssertEqual(h.recorder.all.count, 2)
            XCTAssertTrue(h.recorder.all.allSatisfy { $0.connectCount == 0 })
            h.close()
        }
    }
}

private final class TicketGate: @unchecked Sendable {
    private let continuation = TestValue<CheckedContinuation<SessionTicketResponse?, Error>?>(nil)
    func ticket() async throws -> SessionTicketResponse? {
        try await withCheckedThrowingContinuation { c in continuation.update { $0 = c } }
    }
    var isWaiting: Bool { continuation.value != nil }
    func finish(_ credential: SessionTicketResponse? = nil) {
        let c = continuation.update { let c = $0; $0 = nil; return c }
        c?.resume(returning: credential)
    }
}

private final class FakeQUICConnection: TerminalQUICConnection, @unchecked Sendable {
    typealias Receiver = @Sendable (Data?, Bool, NWError?) -> Void
    private struct State {
        var stateHandler: (@Sendable (NWConnection.State) -> Void)?
        var viableHandler: (@Sendable (Bool) -> Void)?
        var pathHandler: (@Sendable (Bool) -> Void)?
        var receivers: [Receiver] = []
        var sent: [Data] = []
        var sendCompletions: [@Sendable (NWError?) -> Void] = []
        var starts = 0
        var cancels = 0
    }
    private let state = TestValue(State())
    var stateUpdateHandler: (@Sendable (NWConnection.State) -> Void)? {
        get { state.value.stateHandler }
        set { state.update { $0.stateHandler = newValue } }
    }
    var viabilityUpdateHandler: (@Sendable (Bool) -> Void)? {
        get { state.value.viableHandler }
        set { state.update { $0.viableHandler = newValue } }
    }
    var betterPathUpdateHandler: (@Sendable (Bool) -> Void)? {
        get { state.value.pathHandler }
        set { state.update { $0.pathHandler = newValue } }
    }
    var starts: Int { state.value.starts }
    var cancels: Int { state.value.cancels }
    var frames: [Data] { state.value.sent }
    var hasReceiver: Bool { !state.value.receivers.isEmpty }
    var lastSendCompletion: (@Sendable (NWError?) -> Void)? { state.value.sendCompletions.last }
    func start(queue: DispatchQueue) { state.update { $0.starts += 1 } }
    func cancel() { state.update { $0.cancels += 1 } }
    func send(_ data: Data, isComplete: Bool, completion: @escaping @Sendable (NWError?) -> Void) {
        state.update { $0.sent.append(data); $0.sendCompletions.append(completion) }
    }
    func receive(completion: @escaping Receiver) { state.update { $0.receivers.append(completion) } }
    func takeReceiver() -> Receiver? {
        state.update { $0.receivers.isEmpty ? nil : $0.receivers.removeFirst() }
    }
    func deliver(_ data: Data?, complete: Bool = false, error: NWError? = nil) {
        takeReceiver()?(data, complete, error)
    }
}

final class TerminalStreamDecoderTests: XCTestCase {
    func testEveryPrefixAndPayloadFragmentBoundary() throws {
        let payload = Data("hello 🌍\u{1b}[31mred\u{1b}[0m".utf8)
        let wire = cursorBytes(0x0102030405060708) + payload
        for split in 0...wire.count {
            var decoder = TerminalStreamDecoder()
            let first = try decoder.receive(Data(wire.prefix(split)))
            if split < 8 {
                XCTAssertFalse(decoder.isReady)
                XCTAssertNil(decoder.offset)
                XCTAssertTrue(first.isEmpty)
            }
            let second = try decoder.receive(Data(wire.dropFirst(split)))
            XCTAssertEqual(first + second, payload, "split \(split)")
            XCTAssertEqual(decoder.offset, 0x0102030405060708 + UInt64(payload.count))
        }
    }

    func testSingleByteFragmentsAndLiveBytesAdvanceActualCursor() throws {
        var decoder = TerminalStreamDecoder()
        var output = Data()
        let wire = cursorBytes(100) + Data("catchup".utf8)
        for byte in wire { output.append(try decoder.receive(Data([byte]))) }
        XCTAssertEqual(output, Data("catchup".utf8))
        XCTAssertEqual(decoder.offset, 107)
        XCTAssertEqual(try decoder.receive(Data("live".utf8)), Data("live".utf8))
        XCTAssertEqual(decoder.offset, 111)
        XCTAssertTrue(try decoder.receive(Data()).isEmpty)
        XCTAssertEqual(decoder.offset, 111)
    }

    func testInterruptedReplayResumesFromDeliveredBytesNotSnapshotEnd() throws {
        var first = TerminalStreamDecoder()
        let all = Data("abcdefghij".utf8)
        let delivered = try first.receive(cursorBytes(900) + all.prefix(3))
        XCTAssertEqual(first.offset, 903)
        var resumed = TerminalStreamDecoder()
        let remainder = try resumed.receive(cursorBytes(first.offset!) + all.dropFirst(3))
        XCTAssertEqual(delivered + remainder, all)
        XCTAssertEqual(resumed.offset, 910)
    }

    func testCursorOverflowFailsWithoutDeliveringPayload() throws {
        var decoder = TerminalStreamDecoder()
        XCTAssertTrue(try decoder.receive(cursorBytes(UInt64.max)).isEmpty)
        XCTAssertThrowsError(try decoder.receive(Data([0xff])))
        XCTAssertEqual(decoder.offset, UInt64.max)
    }

    func testV3HeaderLayoutAndLengthValidation() {
        XCTAssertEqual(
            QUICTerminalClient.attachHeader(sessionId: "ab", ticket: "xyz", resumeFrom: 42),
            Data([0x13, 0, 2, 97, 98, 0, 3, 120, 121, 122]) + cursorBytes(42)
        )
        XCTAssertNil(QUICTerminalClient.attachHeader(sessionId: "", ticket: "x", resumeFrom: 0))
        XCTAssertNil(QUICTerminalClient.attachHeader(sessionId: "a", ticket: "", resumeFrom: 0))
        XCTAssertNil(QUICTerminalClient.attachHeader(sessionId: String(repeating: "é", count: 32768), ticket: "x", resumeFrom: 0))
        XCTAssertNil(QUICTerminalClient.attachHeader(sessionId: "a", ticket: String(repeating: "x", count: 65536), resumeFrom: 0))
    }
}

final class QUICTerminalClientTests: XCTestCase {
    private func client(_ connection: FakeQUICConnection, detachTimeout: TimeInterval = 0.03) -> QUICTerminalClient {
        QUICTerminalClient(hubHost: "example.invalid", detachTimeout: detachTimeout,
                           makeConnection: { _, _ in connection })
    }

    func testReadyRequiresFullResponsePrefixAndInputWaitsForAttach() async {
        let connection = FakeQUICConnection()
        let client = client(connection)
        let events = TestValue([String]())
        let offsets = TestValue([UInt64]())
        client.onConnected = { events.update { $0.append("connected") } }
        client.onOutputReceived = { data, offset in
            events.update { $0.append(String(decoding: data, as: UTF8.self)) }
            offsets.update { $0.append(offset) }
        }
        client.sendInput(Data("typed-before-ticket".utf8))
        client.sendResize(cols: 80, rows: 24)
        client.setServerCertificate(Data([1, 2, 3]))
        client.connect(sessionId: "s", ticket: "test-ticket", resumeFrom: 100)
        await eventually { connection.starts == 1 }
        connection.stateUpdateHandler?(.ready)
        await eventually { connection.hasReceiver }
        XCTAssertTrue(events.value.isEmpty, "QUIC ready is not an authenticated attach")
        XCTAssertEqual(connection.frames.count, 1, "only the attach header may precede acceptance")
        connection.deliver(Data(cursorBytes(100).prefix(3)))
        await eventually { connection.hasReceiver }
        XCTAssertTrue(events.value.isEmpty)
        connection.deliver(Data(cursorBytes(100).dropFirst(3)) + Data("abc".utf8))
        await eventually { connection.frames.count == 3 && connection.hasReceiver }
        XCTAssertEqual(events.value, ["abc", "connected"])
        XCTAssertEqual(offsets.value, [103])
        XCTAssertEqual(connection.frames[1], Data([1, 0, 80, 0, 24]))
        XCTAssertEqual(connection.frames[2].suffix("typed-before-ticket".utf8.count), Data("typed-before-ticket".utf8))
        connection.deliver(Data("live".utf8))
        await eventually { offsets.value.last == 107 }
        XCTAssertEqual(events.value, ["abc", "connected", "live"])
        client.disconnect()
    }

    func testMissingCredentialsNeverOpenANetworkConnection() async {
        for (ticket, certificate) in [(nil as String?, Data([1])), ("", Data([1])), ("ticket", Data())] {
            let connection = FakeQUICConnection()
            let client = client(connection)
            let failures = TestValue(0)
            client.onDisconnected = { failures.update { $0 += 1 } }
            client.setServerCertificate(certificate)
            client.connect(sessionId: "s", ticket: ticket)
            await eventually { failures.value == 1 }
            XCTAssertEqual(connection.starts, 0)
        }
    }

    func testCloseBeforeConnectPermanentlyRetiresTransport() async {
        let connection = FakeQUICConnection()
        let client = client(connection)
        let completions = TestValue(0)
        client.disconnect { completions.update { $0 += 1 } }
        client.setServerCertificate(Data([1]))
        client.connect(sessionId: "s", ticket: "ticket")
        client.disconnect { completions.update { $0 += 1 } }
        await eventually { completions.value == 2 }
        XCTAssertEqual(connection.starts, 0)
    }

    func testLateReadyReceiveAndFailureAfterDisconnectAreIgnored() async {
        let connection = FakeQUICConnection()
        let client = client(connection)
        let callbacks = TestValue(0)
        client.onConnected = { callbacks.update { $0 += 1 } }
        client.onDisconnected = { callbacks.update { $0 += 1 } }
        client.onOutputReceived = { _, _ in callbacks.update { $0 += 1 } }
        client.setServerCertificate(Data([1]))
        client.connect(sessionId: "s", ticket: "ticket")
        await eventually { connection.starts == 1 }
        let lateState = connection.stateUpdateHandler
        lateState?(.ready)
        await eventually { connection.hasReceiver }
        let lateReceive = connection.takeReceiver()
        client.disconnect()
        await eventually { connection.cancels == 1 }
        lateState?(.ready)
        lateReceive?(cursorBytes(0) + Data("stale".utf8), false, nil)
        lateState?(.failed(.posix(.ECONNRESET)))
        let drained = TestValue(false)
        client.disconnect { drained.update { $0 = true } }
        await eventually { drained.value }
        XCTAssertEqual(callbacks.value, 0)
        XCTAssertEqual(connection.frames.count, 1)
    }

    func testIncompletePrefixEOFDoesNotReportConnectedAndDisconnectsOnce() async {
        let connection = FakeQUICConnection()
        let client = client(connection)
        let connected = TestValue(0)
        let failures = TestValue(0)
        client.onConnected = { connected.update { $0 += 1 } }
        client.onDisconnected = { failures.update { $0 += 1 } }
        client.setServerCertificate(Data([1]))
        client.connect(sessionId: "s", ticket: "ticket")
        await eventually { connection.starts == 1 }
        let lateState = connection.stateUpdateHandler
        lateState?(.ready)
        await eventually { connection.hasReceiver }
        connection.deliver(Data([0, 0, 0]), complete: true)
        await eventually { failures.value == 1 }
        lateState?(.failed(.posix(.ECONNRESET)))
        client.disconnect()
        XCTAssertEqual(connected.value, 0)
        XCTAssertEqual(connection.cancels, 1)
        XCTAssertEqual(failures.value, 1)
    }

    func testDetachWatchdogAndLateCompletionCancelExactlyOnce() async {
        let connection = FakeQUICConnection()
        let client = client(connection)
        let connected = TestValue(false)
        let completions = TestValue(0)
        client.onConnected = { connected.update { $0 = true } }
        client.setServerCertificate(Data([1]))
        client.connect(sessionId: "s", ticket: "ticket")
        await eventually { connection.starts == 1 }
        connection.stateUpdateHandler?(.ready)
        await eventually { connection.hasReceiver }
        connection.deliver(cursorBytes(10))
        await eventually { connected.value }
        client.disconnect { completions.update { $0 += 1 } }
        await eventually { connection.frames.last == Data([2]) }
        let lateCompletion = connection.lastSendCompletion
        await eventually { completions.value == 1 }
        lateCompletion?(nil)
        lateCompletion?(.posix(.ECONNRESET))
        XCTAssertEqual(completions.value, 1)
        XCTAssertEqual(connection.cancels, 1)
    }

    func testSendFailureRetiresConnectionOnce() async {
        let connection = FakeQUICConnection()
        let client = client(connection)
        let failures = TestValue(0)
        client.onDisconnected = { failures.update { $0 += 1 } }
        client.setServerCertificate(Data([1]))
        client.connect(sessionId: "s", ticket: "ticket")
        await eventually { connection.starts == 1 }
        connection.stateUpdateHandler?(.ready)
        await eventually { !connection.frames.isEmpty }
        connection.lastSendCompletion?(.posix(.ECONNRESET))
        await eventually { failures.value == 1 }
        XCTAssertEqual(connection.cancels, 1)
    }
}

final class TerminalLifecycleRaceTests: XCTestCase {
    func testNeverOpenedAndClosedSessionsIgnoreAllPathAndLifecycleTriggers() {
        let calls = TestValue(0)
        let session = TerminalSession(sessionId: "unopened", makeTransport: { _ in
            calls.update { $0 += 1 }; return FakeTransport()
        }, mintTicket: { _ in nil })
        session.retryImmediatelyIfWaiting()
        session.reconnectNow()
        session.suspend()
        session.resume()
        XCTAssertEqual(calls.value, 0)
        session.shutdown()
        session.connectIfNeeded()
        session.retryImmediatelyIfWaiting()
        session.suspend()
        session.resume()
        XCTAssertEqual(calls.value, 0)
    }

    func testDelayedTicketAfterShutdownDoesNotConnectOrRetainSession() async {
        let gate = TicketGate()
        let transport = FakeTransport()
        var session: TerminalSession? = TerminalSession(
            sessionId: "ticket-close", makeTransport: { _ in transport },
            mintTicket: { _ in try await gate.ticket() }
        )
        weak let weakSession = session
        session?.connectIfNeeded()
        await eventually { gate.isWaiting }
        session?.shutdown()
        session = nil
        XCTAssertNil(weakSession, "an uncooperative ticket request must not retain the session")
        gate.finish()
        XCTAssertEqual(transport.connectCount, 0)
        XCTAssertEqual(transport.disconnectCount, 1)
    }

    func testShutdownDuringCredentialInstallationPreventsConnect() async {
        let transport = FakeTransport()
        let session = TerminalSession(
            sessionId: "install-close", makeTransport: { _ in transport },
            mintTicket: { _ in SessionTicketResponse(ticket: "ticket", quicCertificate: "AQID") }
        )
        transport.beforeCertificate = { [weak session] in session?.shutdown() }
        session.connectIfNeeded()
        await eventually { transport.disconnectCount == 1 }
        XCTAssertEqual(transport.connectCount, 0)
    }

    func testWatchdogCancelsStalledTicketAndLateResultCannotConnect() async {
        let gate = TicketGate()
        let sleeper = ManualSleeper()
        let transport = FakeTransport()
        let session = TerminalSession(
            sessionId: "stalled-ticket", makeTransport: { _ in transport },
            mintTicket: { _ in try await gate.ticket() },
            sleep: { try await sleeper.sleep($0) }
        )
        session.connectIfNeeded()
        await eventually { gate.isWaiting && sleeper.count == 1 }
        sleeper.releaseAll()
        await eventually { session.currentState == .reconnecting(attempt: 1) && sleeper.count == 1 }
        gate.finish(SessionTicketResponse(ticket: "too-late", quicCertificate: "AQID"))
        session.shutdown()
        sleeper.releaseAll()
        XCTAssertEqual(transport.connectCount, 0)
        XCTAssertEqual(transport.disconnectCount, 1)
    }

    func testCancelledWatchdogCannotDisconnectHealthyOrReplacementTransport() async {
        let sleeper = ManualSleeper()
        let recorder = TransportRecorder()
        let session = TerminalSession(
            sessionId: "cancelled-watchdog",
            makeTransport: { _ in let t = FakeTransport(); recorder.record(t); return t },
            mintTicket: { _ in nil }, sleep: { try await sleeper.sleep($0) }
        )
        session.connectIfNeeded()
        await eventually { sleeper.count == 1 }
        recorder.all[0].becomeConnected()
        session.reconnectNow()
        await eventually { sleeper.count == 2 }
        recorder.all[1].becomeConnected()
        sleeper.releaseAll()
        XCTAssertEqual(session.currentState, .connected)
        XCTAssertEqual(recorder.all[1].disconnectCount, 0)
        session.shutdown()
    }

    func testConcurrentConnectAndShutdownNeverProduceAnotherLiveTransport() async {
        let recorder = TransportRecorder()
        let session = TerminalSession(
            sessionId: "concurrent",
            makeTransport: { _ in let t = FakeTransport(); recorder.record(t); return t },
            mintTicket: { _ in nil }
        )
        DispatchQueue.concurrentPerform(iterations: 100) { index in
            if index.isMultiple(of: 3) { session.shutdown() }
            else { session.connectIfNeeded() }
        }
        session.shutdown()
        session.retryImmediatelyIfWaiting()
        XCTAssertLessThanOrEqual(recorder.all.count, 1)
        XCTAssertTrue(recorder.all.allSatisfy { $0.disconnectCount == 1 })
    }

    func testUnauthorizedTicketStaysUnreachableAcrossPathUpdates() async {
        let transport = FakeTransport()
        let session = TerminalSession(
            sessionId: "unauthorized", makeTransport: { _ in transport },
            mintTicket: { _ in throw HubError.httpError(401) }
        )
        session.connectIfNeeded()
        await eventually { session.currentState.needsUserAction }
        session.retryImmediatelyIfWaiting()
        XCTAssertTrue(session.currentState.needsUserAction)
        session.shutdown()
    }

    func testTicketEndpointRejectsHTTPBeforeSendingCredentials() async {
        let hub = HubClient(baseURL: URL(string: "http://example.invalid")!)
        do {
            _ = try await hub.sessionTicket(sessionId: "s", token: "not-a-real-token")
            XCTFail("HTTP ticket request must fail closed")
        } catch HubError.invalidURL {
            // Expected, before URLSession is invoked.
        } catch { XCTFail("Expected invalidURL, got \(error)") }
    }

    func testTicketResponseRequiresCertificateField() throws {
        let json = Data(#"{"ticket":"test","quic_certificate":"AQID"}"#.utf8)
        let credential = try JSONDecoder().decode(SessionTicketResponse.self, from: json)
        XCTAssertEqual(credential.ticket, "test")
        XCTAssertEqual(Data(base64Encoded: credential.quicCertificate), Data([1, 2, 3]))
        XCTAssertThrowsError(try JSONDecoder().decode(SessionTicketResponse.self, from: Data(#"{"ticket":"test"}"#.utf8)))
    }
}

/// A transport that never touches the network. Records what it was asked to do
/// and lets the test drive the callbacks by hand.
final class TestValue<Value>: @unchecked Sendable {
    private let lock = NSLock()
    private var storage: Value
    init(_ value: Value) { storage = value }
    var value: Value { lock.filWithLock { storage } }
    @discardableResult
    func update<T>(_ body: (inout Value) -> T) -> T { lock.filWithLock { body(&storage) } }
}

final class FakeTransport: TerminalTransport, @unchecked Sendable {
    private struct State {
        var output: (@Sendable (Data, UInt64) -> Void)?
        var connected: (@Sendable () -> Void)?
        var disconnected: (@Sendable () -> Void)?
        var betterPath: (@Sendable () -> Void)?
        var beforeResize: (@Sendable () -> Void)?
        var beforeCertificate: (@Sendable () -> Void)?
        var connectCount = 0
        var disconnectCount = 0
        var sentInput: [Data] = []
        var sentResizes: [(cols: UInt16, rows: UInt16)] = []
        var lastTicket: String?
        var lastResumeFrom: UInt64 = 0
        var certificate = Data()
        var offset: UInt64 = 0
    }
    private let state = TestValue(State())
    var onOutputReceived: (@Sendable (Data, UInt64) -> Void)? {
        get { state.value.output }
        set { state.update { $0.output = newValue } }
    }
    var onConnected: (@Sendable () -> Void)? {
        get { state.value.connected }
        set { state.update { $0.connected = newValue } }
    }
    var onDisconnected: (@Sendable () -> Void)? {
        get { state.value.disconnected }
        set { state.update { $0.disconnected = newValue } }
    }
    var onBetterPathAvailable: (@Sendable () -> Void)? {
        get { state.value.betterPath }
        set { state.update { $0.betterPath = newValue } }
    }
    var beforeResize: (@Sendable () -> Void)? {
        get { state.value.beforeResize }
        set { state.update { $0.beforeResize = newValue } }
    }
    var beforeCertificate: (@Sendable () -> Void)? {
        get { state.value.beforeCertificate }
        set { state.update { $0.beforeCertificate = newValue } }
    }
    var connectCount: Int { state.value.connectCount }
    var disconnectCount: Int { state.value.disconnectCount }
    var sentInput: [Data] { state.value.sentInput }
    var sentResizes: [(cols: UInt16, rows: UInt16)] { state.value.sentResizes }
    var lastTicket: String? { state.value.lastTicket }
    var lastResumeFrom: UInt64 { state.value.lastResumeFrom }
    var certificate: Data { state.value.certificate }
    func connect(sessionId: String, ticket: String?, resumeFrom: UInt64) {
        state.update {
            $0.connectCount += 1
            $0.lastTicket = ticket
            $0.lastResumeFrom = resumeFrom
        }
    }
    func setServerCertificate(_ certificate: Data) {
        state.update { $0.certificate = certificate }
        beforeCertificate?()
    }
    func disconnect() { state.update { $0.disconnectCount += 1 } }
    func sendInput(_ data: Data) { state.update { $0.sentInput.append(data) } }
    func sendResize(cols: UInt16, rows: UInt16) {
        state.update { $0.sentResizes.append((cols, rows)) }
        beforeResize?()
    }
    func becomeConnected() { onConnected?() }
    func die() { onDisconnected?() }
    func deliver(_ text: String) {
        let data = Data(text.utf8)
        let offset = state.update { $0.offset += UInt64(data.count); return $0.offset }
        onOutputReceived?(data, offset)
    }
}

/// Sendable box: the registry's factory is a `@Sendable` closure, so it cannot
/// capture the XCTestCase.
final class TransportRecorder: @unchecked Sendable {
    private let lock = NSLock()
    private var made: [FakeTransport] = []
    private var recordedHosts: [String] = []

    func record(_ t: FakeTransport, host: String = "") {
        lock.lock()
        defer { lock.unlock() }
        made.append(t)
        recordedHosts.append(host)
    }

    var hosts: [String] {
        lock.lock()
        defer { lock.unlock() }
        return recordedHosts
    }

    var all: [FakeTransport] {
        lock.lock()
        defer { lock.unlock() }
        return made
    }
}

final class TerminalSessionTests: XCTestCase {
    private var recorder = TransportRecorder()
    private var registry: TerminalConnectionRegistry!
    private var transports: [FakeTransport] { recorder.all }

    func testLateConnectedAfterDisconnectCannotReviveSession() {
        let registry = registry!
        _ = registry.open(sessionId: "late-ready", hubHost: "example.invalid")
        transports[0].die()
        transports[0].becomeConnected()
        XCTAssertEqual(registry.existingSession(sessionId: "late-ready")?.currentState, .reconnecting(attempt: 1))
    }

    func testDisconnectDuringGeometryReplayDoesNotPublishConnected() {
        let registry = registry!
        _ = registry.open(sessionId: "resize-race", hubHost: "example.invalid")
        registry.resize(sessionId: "resize-race", cols: 80, rows: 24)
        let transport = transports[0]
        transport.beforeResize = { [weak transport] in transport?.die() }
        transport.becomeConnected()
        XCTAssertEqual(registry.existingSession(sessionId: "resize-race")?.currentState, .reconnecting(attempt: 1))
    }

    func testOpenWhileBackgroundedWaitsForForeground() {
        let registry = registry!
        registry.applicationDidEnterBackground()
        _ = registry.open(sessionId: "background-open", hubHost: "example.invalid")
        XCTAssertEqual(transports.count, 0)
        XCTAssertEqual(registry.existingSession(sessionId: "background-open")?.currentState, .suspended)
        registry.applicationWillEnterForeground()
        XCTAssertEqual(transports.count, 1)
    }

    func testRelayCreatedBeforeOpenUsesTheRealHubHost() {
        let registry = registry!
        _ = registry.outputRelay(sessionId: "early-relay")
        _ = registry.open(sessionId: "early-relay", hubHost: "quic.example.invalid")
        XCTAssertEqual(recorder.hosts, ["quic.example.invalid"])
    }

    func testRetiredTransportCannotDisconnectItsReplacement() {
        let registry = registry!
        _ = registry.open(sessionId: "stale-callback", hubHost: "example.invalid")
        let old = transports[0]
        registry.reconnect(sessionId: "stale-callback")
        transports[1].becomeConnected()
        old.die()
        XCTAssertEqual(registry.existingSession(sessionId: "stale-callback")?.currentState, .connected)
    }

    func testLateConnectedCallbackCannotReviveSuspendedSession() {
        let registry = registry!
        _ = registry.open(sessionId: "suspended-callback", hubHost: "example.invalid")
        registry.applicationDidEnterBackground()
        transports[0].becomeConnected()
        XCTAssertEqual(registry.existingSession(sessionId: "suspended-callback")?.currentState, .suspended)
    }

    func testLatePathCallbackCannotReopenClosedSession() {
        let registry = registry!
        _ = registry.open(sessionId: "closed-callback", hubHost: "example.invalid")
        let session = registry.existingSession(sessionId: "closed-callback")
        registry.close(sessionId: "closed-callback")
        transports[0].onBetterPathAvailable?()
        session?.connectIfNeeded()
        XCTAssertEqual(transports.count, 1)
    }

    func testTransportIsReleasedAfterShutdown() {
        var transport: FakeTransport? = FakeTransport()
        weak let released = transport
        var session: TerminalSession? = TerminalSession(sessionId: "release", makeTransport: { [transport] _ in transport! }, mintTicket: { _ in nil })
        session?.connectIfNeeded()
        session?.shutdown()
        session = nil
        transport = nil
        XCTAssertNil(released, "callbacks must not retain their own transport")
    }

    override func setUp() {
        super.setUp()
        recorder = TransportRecorder()
        let recorder = self.recorder
        registry = TerminalConnectionRegistry(
            monitorsNetwork: false,
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

    /// The regression this whole refactor exists for.
    ///
    /// Before: the AsyncStream's onTermination destroyed `outputRelays[sid]`,
    /// and the next `connect` minted a fresh relay. The live SwiftTerm view
    /// still held the first one, so the terminal rendered nothing while the UI
    /// said "connected".
    func testRelayIdentitySurvivesAReconnect() async {
        let registry = registry!
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
        let registry = registry!
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
        let registry = registry!
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
    @MainActor
    func testOutputStillReachesTheOriginalRelayAfterReconnect() async {
        let registry = registry!
        let sid = "session-d"

        let relay = registry.outputRelay(sessionId: sid)
        let view = SwiftTerm.TerminalView(frame: CGRect(x: 0, y: 0, width: 400, height: 600))
        relay.attach(view)
        _ = registry.open(sessionId: sid, hubHost: "example.invalid")
        XCTAssertEqual(transports.count, 1)

        registry.reconnect(sessionId: sid)
        XCTAssertEqual(transports.count, 2, "reconnect opens a second transport")
        XCTAssertEqual(transports[0].disconnectCount, 1, "the old transport is closed")

        transports[1].deliver("hello")
        transports[0].deliver("STALE")
        // Flush the actual relay onto SwiftTerm, not just compare identities.
        await withCheckedContinuation { continuation in
            DispatchQueue.main.async { continuation.resume() }
        }
        let rendered = String(decoding: view.getTerminal().getBufferAsData(), as: UTF8.self)
        XCTAssertTrue(rendered.contains("hello"))
        XCTAssertFalse(rendered.contains("STALE"))

        XCTAssertTrue(
            registry.outputRelay(sessionId: sid) === relay,
            "post-reconnect output must land in the relay the view holds"
        )
    }

    /// A new connection must re-declare the geometry it inherited.
    func testGeometryIsReDeclaredOnEveryNewConnection() {
        let registry = registry!
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
        let registry = registry!
        let sid = "session-f"

        _ = registry.open(sessionId: sid, hubHost: "example.invalid")
        XCTAssertNotNil(registry.existingSession(sessionId: sid))

        registry.close(sessionId: sid)

        XCTAssertEqual(transports[0].disconnectCount, 1)
        XCTAssertNil(registry.existingSession(sessionId: sid))
    }

    /// A dropped connection enters the retry state (phase 2), not a dead end.
    func testTransportDeathEntersRetry() {
        let registry = registry!
        let sid = "session-g"

        _ = registry.open(sessionId: sid, hubHost: "example.invalid")
        transports[0].becomeConnected()
        XCTAssertEqual(registry.existingSession(sessionId: sid)?.currentState, .connected)

        transports[0].die()

        XCTAssertEqual(
            registry.existingSession(sessionId: sid)?.currentState,
            .reconnecting(attempt: 1)
        )
    }
}

/// Phase 2: the automatic retry policy.
final class ReconnectPolicyTests: XCTestCase {
    private var recorder = TransportRecorder()
    private var registry: TerminalConnectionRegistry!
    private var transports: [FakeTransport] { recorder.all }

    override func setUp() {
        super.setUp()
        recorder = TransportRecorder()
        let recorder = self.recorder
        registry = TerminalConnectionRegistry(
            monitorsNetwork: false,
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

    /// The core of the reported "stuck on Reconnecting" symptom: a dropped
    /// connection used to sit there until the user tapped the button.
    func testADroppedConnectionRetriesByItself() async {
        let registry = registry!
        let sid = "retry-a"

        _ = registry.open(sessionId: sid, hubHost: "example.invalid")
        transports[0].becomeConnected()
        transports[0].die()

        // First backoff is 0.25s +/- jitter.
        try? await Task.sleep(nanoseconds: 700_000_000)

        XCTAssertGreaterThanOrEqual(
            transports.count, 2,
            "a dropped connection must retry without the user tapping anything"
        )
    }

    /// The state must say "retrying", not "dead", while a retry is pending.
    func testRetryingStateIsNotUserActionable() async {
        let registry = registry!
        let sid = "retry-b"

        _ = registry.open(sessionId: sid, hubHost: "example.invalid")
        transports[0].becomeConnected()
        transports[0].die()

        let state = registry.existingSession(sessionId: sid)?.currentState
        if case .reconnecting = state {
            XCTAssertFalse(
                state!.needsUserAction,
                "a retry in flight must not raise the modal overlay"
            )
        } else {
            XCTFail("expected .reconnecting, got \(String(describing: state))")
        }
    }

    /// A successful connection must clear the backoff, or the delay ratchets up
    /// over a long session. This is the iOS twin of the daemon bug.
    func testBackoffResetsAfterASuccessfulConnection() async {
        let registry = registry!
        let sid = "retry-c"

        _ = registry.open(sessionId: sid, hubHost: "example.invalid")
        transports[0].die()
        try? await Task.sleep(nanoseconds: 700_000_000)
        guard transports.count >= 2 else { return XCTFail("no retry happened") }

        // Second attempt succeeds, then dies.
        transports[1].becomeConnected()
        XCTAssertEqual(registry.existingSession(sessionId: sid)?.currentState, .connected)
        transports[1].die()

        // If the backoff had not reset, the next state would report attempt 3.
        let state = registry.existingSession(sessionId: sid)?.currentState
        XCTAssertEqual(
            state, .reconnecting(attempt: 1),
            "a successful connection must reset the retry counter"
        )
    }

    /// suspend() must stop retrying; otherwise a backgrounded app keeps
    /// hammering the hub and re-taking the Mac's PTY size.
    func testSuspendStopsRetrying() async {
        let registry = registry!
        let sid = "retry-d"

        _ = registry.open(sessionId: sid, hubHost: "example.invalid")
        transports[0].becomeConnected()

        registry.applicationDidEnterBackground()
        XCTAssertEqual(registry.existingSession(sessionId: sid)?.currentState, .suspended)
        let countAtSuspend = transports.count

        try? await Task.sleep(nanoseconds: 700_000_000)
        XCTAssertEqual(
            transports.count, countAtSuspend,
            "a suspended session must not keep reconnecting in the background"
        )
    }

    /// Foregrounding must reconnect at once, with no backoff to wait out.
    func testForegroundResumesImmediately() {
        let registry = registry!
        let sid = "retry-e"

        _ = registry.open(sessionId: sid, hubHost: "example.invalid")
        transports[0].becomeConnected()
        registry.applicationDidEnterBackground()
        let countAtSuspend = transports.count

        registry.applicationWillEnterForeground()

        XCTAssertEqual(
            transports.count, countAtSuspend + 1,
            "returning to the foreground must reconnect immediately"
        )
    }

    /// A better network path means rebuilding: Network.framework QUIC does not
    /// migrate connections for us.
    func testBetterPathRebuildsTheConnection() {
        let registry = registry!
        let sid = "retry-f"

        _ = registry.open(sessionId: sid, hubHost: "example.invalid")
        transports[0].becomeConnected()
        transports[0].onBetterPathAvailable?()

        XCTAssertEqual(transports.count, 2, "a better path must produce a new connection")
        XCTAssertEqual(transports[0].disconnectCount, 1, "and retire the old one")
    }

    /// Backoff must be bounded and monotone up to the cap.
    func testBackoffProgressionIsBoundedAndJittered() {
        let delays = (1...12).map { ReconnectPolicy.delay(forAttempt: $0) }
        XCTAssertLessThanOrEqual(
            delays.max()!, ReconnectPolicy.maxDelay * 1.15,
            "backoff must stay within the cap plus jitter"
        )
        XCTAssertGreaterThan(delays[3], delays[0], "backoff must grow")
    }
}

@MainActor
final class AppAuthRecoveryTests: XCTestCase {
    func testUnauthorizedSnapshotReturnsToLogin() async {
        for status in [401, 403] {
            let store = TestStore(initialState: AppFeature.State.main(MachinesFeature.State())) { AppFeature() }
            store.exhaustivity = .off
            await store.send(.main(.sessionsLoaded(.failure(HubError.httpError(status)))))
            await store.receive(\.main.loginRequired)
            await store.finish()
            guard case .auth = store.state else {
                XCTFail("HTTP \(status) must require login, not leave the expired account on Machines")
                continue
            }
        }
    }

    func testNetworkAndServerFailuresDoNotLogOut() async {
        for error in [URLError(.notConnectedToInternet) as Error, HubError.httpError(500)] {
            let store = TestStore(initialState: AppFeature.State.main(MachinesFeature.State())) { AppFeature() }
            store.exhaustivity = .off
            await store.send(.main(.sessionsLoaded(.failure(error))))
            await store.finish()
            guard case .main(let main) = store.state else { return XCTFail("An outage must not log out") }
            XCTAssertFalse(main.isConnected)
            XCTAssertNotNil(main.errorMessage)
        }
    }

    func testLogoutSynchronouslyClearsTerminalRegistry() {
        let registry = TerminalConnectionRegistry.shared
        let sid = "unit-test-logout-unopened"
        _ = registry.outputRelay(sessionId: sid)
        defer { registry.close(sessionId: sid) }
        var state = AppFeature.State.main(MachinesFeature.State())
        // Deliberately do not execute the returned effect: teardown must not
        // depend on an effect surviving removal of the Machines reducer.
        _ = AppFeature().body.reduce(into: &state, action: .main(.logoutTapped))
        XCTAssertNil(registry.existingSession(sessionId: sid))
        guard case .auth = state else { return XCTFail("Expected login") }
    }

    func testAuthDeepLinkIsRetainedInsteadOfDiscarded() {
        let original = FilSharedStore.takePendingActivityURL()
        defer {
            _ = FilSharedStore.takePendingActivityURL()
            if let original { FilSharedStore.savePendingActivityURL(original) }
        }
        let url = FilActivityURL.session("unit-test-specific-session")!
        var state = AppFeature.State.auth(AuthFeature.State())
        _ = AppFeature().body.reduce(into: &state, action: .openURL(url))
        XCTAssertEqual(FilSharedStore.takePendingActivityURL()?.path, url.path)
    }

    func testSuccessfulSnapshotClearsStaleError() async {
        var state = MachinesFeature.State()
        state.errorMessage = "old outage"
        state.isLoading = true
        state.isConnected = false
        let store = TestStore(initialState: state) { MachinesFeature() }
        store.exhaustivity = .off
        await store.send(.sessionsLoaded(.success([])))
        await store.finish()
        XCTAssertNil(store.state.errorMessage)
        XCTAssertFalse(store.state.isLoading)
        XCTAssertTrue(store.state.isConnected)
    }

    func testSuccessfulLiveSnapshotClearsStaleError() async {
        var state = MachinesFeature.State()
        state.errorMessage = "old outage"
        state.isConnected = false
        let store = TestStore(initialState: state) { MachinesFeature() }
        store.exhaustivity = .off
        await store.send(.liveStatesReceived([]))
        await store.finish()
        XCTAssertNil(store.state.errorMessage)
        XCTAssertTrue(store.state.isConnected)
    }

    func testPendingLinkIsScopedToTheOriginalHub() throws {
        let url = FilActivityURL.session("unit-test-exact-session")!
        let scoped = try XCTUnwrap(AppFeature.scopedSessionURL(url, hubURL: "https://one.example.invalid"))
        XCTAssertNotNil(AppFeature.scopedSessionURL(scoped, hubURL: "https://ONE.example.invalid:443/"))
        XCTAssertNil(AppFeature.scopedSessionURL(scoped, hubURL: "https://two.example.invalid"))
        XCTAssertNil(AppFeature.scopedSessionURL(URL(string: "fil://session/extra/unit-test-exact-session")!, hubURL: "https://one.example.invalid"))
    }

    func testLoginReplaysOnlyTheExactSessionAfterFetchingOwnedSnapshot() async {
        let original = FilSharedStore.takePendingActivityURL()
        defer {
            _ = FilSharedStore.takePendingActivityURL()
            if let original { FilSharedStore.savePendingActivityURL(original) }
        }
        let wanted = Session(id: "unit-test-wanted", deviceId: "unit-test-device", shell: "zsh", command: nil,
                             cwd: "/tmp/wanted", cols: 80, rows: 24, status: .online, createdAt: nil)
        for isOwned in [true, false] {
            let fetched = TestValue(0)
            let other = Session(id: "unit-test-other-account-session", deviceId: "unit-test-device", shell: "zsh", command: nil,
                                cwd: "/tmp/other", cols: 80, rows: 24, status: .online, createdAt: nil)
            let machine = Machine(id: "unit-test-device", name: "test", status: .online, sessions: [isOwned ? wanted : other])
            let store = TestStore(initialState: AppFeature.State.auth(AuthFeature.State())) { AppFeature() } withDependencies: {
                $0.hubClient.fetchMachines = { fetched.update { $0 += 1 }; return [machine] }
            }
            store.exhaustivity = .off
            await store.send(.openURL(FilActivityURL.session(wanted.id)!))
            await store.send(.auth(.loginSucceeded))
            await store.receive(\.openURL)
            await store.receive(\.main.openSession)
            await store.receive(\.main.sessionsLoaded)
            await store.finish()
            guard case .main(let main) = store.state else { return XCTFail("Expected Machines") }
            XCTAssertEqual(fetched.value, 1)
            XCTAssertEqual(main.terminal?.session.id, isOwned ? wanted.id : nil)
            if !isOwned { XCTAssertNotNil(main.errorMessage) }
        }
    }

    func testSnapshotRefreshPreservesTerminalIdentityGeometryAndConnection() async {
        let old = Session(id: "unit-test-current", deviceId: "unit-test-device", shell: "zsh", command: nil,
                          cwd: "/tmp/old", cols: 80, rows: 24, status: .online, createdAt: nil)
        var updated = old
        updated.cwd = "/tmp/new"
        updated.cols = 200
        updated.rows = 60
        let machine = Machine(id: old.deviceId, name: "new-name", status: .online, sessions: [updated])
        var state = MachinesFeature.State()
        state.terminal = TerminalFeature.State(session: old, machineName: "old name")
        state.terminal?.isConnected = true
        state.terminal?.connectionState = .connected
        state.terminal?.isFollowing = true
        let connects = TestValue(0)
        let store = TestStore(initialState: state) { MachinesFeature() } withDependencies: {
            $0.terminalClient.open = { _, _ in connects.update { $0 += 1 }; return AsyncStream { $0.finish() } }
            $0.terminalClient.reconnect = { _ in connects.update { $0 += 1 } }
        }
        store.exhaustivity = .off
        await store.send(.sessionsLoaded(.success([machine])))
        await store.finish()
        XCTAssertEqual(store.state.terminal?.session.id, old.id)
        XCTAssertEqual(store.state.terminal?.session.cwd, "/tmp/new")
        XCTAssertEqual(store.state.terminal?.session.cols, 80)
        XCTAssertEqual(store.state.terminal?.machineName, "new name")
        XCTAssertEqual(store.state.terminal?.connectionState, .connected)
        XCTAssertEqual(store.state.terminal?.isFollowing, true)
        XCTAssertEqual(connects.value, 0)
    }

    func testLoginRequiredPreservesExactSelectionAndClearsRegistrySynchronously() throws {
        let original = FilSharedStore.takePendingActivityURL()
        defer {
            _ = FilSharedStore.takePendingActivityURL()
            if let original { FilSharedStore.savePendingActivityURL(original) }
        }
        let sid = "unit-test-expired-session"
        let registry = TerminalConnectionRegistry.shared
        _ = registry.outputRelay(sessionId: sid)
        defer { registry.close(sessionId: sid) }
        var machines = MachinesFeature.State()
        machines.pendingSessionId = sid
        var state = AppFeature.State.main(machines)
        _ = AppFeature().body.reduce(into: &state, action: .main(.loginRequired))
        XCTAssertNil(registry.existingSession(sessionId: sid))
        let pending = try XCTUnwrap(FilSharedStore.takePendingActivityURL())
        XCTAssertEqual(pending.pathComponents.last, sid)
        XCTAssertNotNil(URLComponents(url: pending, resolvingAgainstBaseURL: false)?.queryItems?.first { $0.name == "hub" })
        guard case .auth(let auth) = state else { return XCTFail("Expected login") }
        XCTAssertNotNil(auth.errorMessage)
    }
}
