import XCTest
@testable import Fil

final class SessionEventSecurityTests: XCTestCase {
    func testSimulatorLoopbackRemainsAvailableForLocalDevelopment() {
        #if DEBUG && targetEnvironment(simulator)
        let request = SessionEventClient.request(hubURL: "http://127.0.0.1:3100", token: "fixture")
        XCTAssertEqual(request?.url?.absoluteString, "ws://127.0.0.1:3100/ws/client")
        XCTAssertEqual(request?.value(forHTTPHeaderField: "Authorization"), "Bearer fixture")
        #endif
    }
    func testEventCredentialIsOnlyInTheHeader() throws {
        let request = try XCTUnwrap(SessionEventClient.request(
            hubURL: "https://hub.example/old?token=stale#fragment", token: "test-only-secret"
        ))
        XCTAssertEqual(request.url?.absoluteString, "wss://hub.example/ws/client")
        XCTAssertEqual(request.value(forHTTPHeaderField: "Authorization"), "Bearer test-only-secret")
        XCTAssertNil(request.url?.query)
    }

    func testInsecureOriginsAndInvalidTokensAreRejectedBeforeOpeningASocket() {
        for hub in ["http://hub.example", "ws://hub.example", "https://user:password@hub.example"] {
            XCTAssertNil(SessionEventClient.request(hubURL: hub, token: "test-only"))
        }
        for token in ["", "bad\r\nheader"] {
            XCTAssertNil(SessionEventClient.request(hubURL: "https://hub.example", token: token))
        }
    }
}
