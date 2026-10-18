import UserNotifications
import XCTest
@testable import Fil

final class AgentNotificationTests: XCTestCase {
    private let question: [AnyHashable: Any] = [
        "aps": ["alert": ["title": "Deploy"], "mutable-content": 1],
        "fil": [
            "kind": "ask",
            "notification_id": "6f1c2a9e-0d4b-4c38-9d2b-0d6f3c1e2a77",
            "session_id": "session-1",
            "options": ["Ship it", "Hold"],
            "expires_at": 1_900_000_000,
        ],
    ]

    func testQuestionPayloadIsParsed() throws {
        let payload = try XCTUnwrap(AgentPushPayload(userInfo: question))
        XCTAssertTrue(payload.isQuestion)
        XCTAssertEqual(payload.sessionId, "session-1")
        XCTAssertEqual(payload.options, ["Ship it", "Hold"])
        XCTAssertEqual(payload.expiresAt, Date(timeIntervalSince1970: 1_900_000_000))
        // Re-encoded for the "answer not sent" retry notification.
        XCTAssertEqual(AgentPushPayload(userInfo: payload.userInfo), payload)
    }

    func testNotificationWithoutSessionAndForeignPushesAreHandledSafely() throws {
        let notify = try XCTUnwrap(AgentPushPayload(userInfo: [
            "fil": ["kind": "notify", "notification_id": "n1", "session_id": NSNull(), "options": []],
        ]))
        XCTAssertFalse(notify.isQuestion)
        XCTAssertNil(notify.sessionId)
        XCTAssertNil(AgentPushPayload(userInfo: ["aps": ["alert": "x"]]))
        XCTAssertNil(AgentPushPayload(userInfo: ["fil": ["kind": "ask", "notification_id": ""]]))
        let capped = try XCTUnwrap(AgentPushPayload(userInfo: [
            "fil": ["kind": "ask", "notification_id": "n2", "options": ["a", "b", "c", "d", "e"]],
        ]))
        XCTAssertEqual(capped.options.count, 4)
    }

    func testCategoriesAreStablePerOptionSetAndMapActionsBack() {
        let id = AgentNotificationCategory.identifier(for: ["Ship it", "Hold"])
        XCTAssertEqual(id, AgentNotificationCategory.identifier(for: ["Ship it", "Hold"]))
        XCTAssertNotEqual(id, AgentNotificationCategory.identifier(for: ["Hold", "Ship it"]))
        XCTAssertTrue(id.hasPrefix(AgentNotificationCategory.dynamicPrefix))

        let category = AgentNotificationCategory.category(for: ["Ship it", "Hold"])
        XCTAssertEqual(category.actions.map(\.title), ["Ship it", "Hold"])
        XCTAssertTrue(category.actions.allSatisfy { $0.options.contains(.authenticationRequired) })
        XCTAssertEqual(AgentNotificationCategory.optionIndex(fromAction: category.actions[1].identifier), 1)
        XCTAssertNil(AgentNotificationCategory.optionIndex(fromAction: UNNotificationDefaultActionIdentifier))
    }

    func testMergeKeepsForeignCategoriesAndBoundsDynamicOnes() {
        let foreign = UNNotificationCategory(identifier: "COMMAND_FINISHED", actions: [], intentIdentifiers: [])
        var categories: Set<UNNotificationCategory> = [foreign]
        for index in 0..<40 {
            categories = AgentNotificationCategory.merged(
                categories, adding: [AgentNotificationCategory.category(for: ["a\(index)", "b"])]
            )
        }
        categories = AgentNotificationCategory.merged(categories, adding: AgentNotificationCategory.staticCategories)
        let ids = Set(categories.map(\.identifier))
        XCTAssertTrue(ids.contains("COMMAND_FINISHED"))
        XCTAssertTrue(ids.contains(AgentNotificationCategory.notify))
        XCTAssertTrue(ids.contains(AgentNotificationCategory.identifier(for: ["a39", "b"])))
        XCTAssertLessThanOrEqual(
            ids.filter { $0.hasPrefix(AgentNotificationCategory.dynamicPrefix) }.count,
            AgentNotificationCategory.maxDynamicCategories
        )
    }

    func testAnswerRequestsOnlyGoToTheHubOverHTTPS() throws {
        let request = try XCTUnwrap(AgentNotificationAPI.answerRequest(
            hubURL: "https://hub.example/", token: "test-only-secret",
            notificationId: "6f1c2a9e-0d4b", answer: "Hold"
        ))
        XCTAssertEqual(request.url?.absoluteString, "https://hub.example/agent-notifications/6f1c2a9e-0d4b/answer")
        XCTAssertEqual(request.httpMethod, "POST")
        XCTAssertEqual(request.value(forHTTPHeaderField: "Authorization"), "Bearer test-only-secret")
        let body = try XCTUnwrap(request.httpBody)
        XCTAssertEqual(try JSONSerialization.jsonObject(with: body) as? [String: String], ["answer": "Hold"])

        XCTAssertNil(AgentNotificationAPI.answerRequest(
            hubURL: "http://hub.example", token: "t", notificationId: "n", answer: "a"))
        XCTAssertNil(AgentNotificationAPI.answerRequest(
            hubURL: "https://hub.example", token: "t", notificationId: "../account", answer: "a"))
        XCTAssertNil(AgentNotificationAPI.unregisterRequest(
            hubURL: "https://hub.example", token: "t", deviceToken: "zz/.."))
        let register = try XCTUnwrap(AgentNotificationAPI.registerRequest(
            hubURL: "https://hub.example", token: "t", deviceToken: "abcd"))
        XCTAssertEqual(register.url?.path, "/push-tokens")
    }

    func testAnswerOutcomesMapHubStatuses() {
        XCTAssertEqual(AgentAnswerOutcome(status: 204), .delivered)
        XCTAssertEqual(AgentAnswerOutcome(status: 410), .closed)
        XCTAssertEqual(AgentAnswerOutcome(status: 503), .macOffline)
        XCTAssertEqual(AgentAnswerOutcome(status: 500), .failed)
    }
}
