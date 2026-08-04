@preconcurrency import ActivityKit
import Foundation
import os

@available(iOS 16.2, *)
actor FilActivityManager {
    static let shared = FilActivityManager()

    private let logger = Logger(subsystem: "sh.fil.app", category: "LiveActivity")
    private var observationTasks: [String: [Task<Void, Never>]] = [:]

    func follow(
        session: Session,
        machineName: String,
        otherSessionCount: Int,
        status: FilActivityStatus
    ) async -> Bool {
        let preferences = FilActivityPreferences.current
        guard preferences.isEnabled, ActivityAuthorizationInfo().areActivitiesEnabled else {
            return false
        }

        let state = projectedState(
            session: session,
            status: status,
            otherSessionCount: otherSessionCount
        )

        for activity in Activity<FilActivityAttributes>.activities
        where activity.attributes.sessionId != session.id {
            await endActivity(activity, immediate: false)
        }

        if let existing = Activity<FilActivityAttributes>.activities.first(where: {
            $0.attributes.sessionId == session.id
        }) {
            await existing.update(content(for: state))
            startObserving(existing)
            return true
        }

        do {
            let pushUpdatesEnabled = (try? await HubClient().health().liveActivityPushEnabled)
                ?? false
            let pushType: PushType? = pushUpdatesEnabled ? .token : nil
            let activity = try Activity.request(
                attributes: FilActivityAttributes(
                    sessionId: session.id,
                    deviceId: session.deviceId,
                    machineName: machineName,
                    startedAt: session.createdAt ?? Date()
                ),
                content: content(for: state),
                pushType: pushType
            )
            startObserving(activity)
            return true
        } catch {
            logger.error("Unable to start Live Activity: \(error.localizedDescription, privacy: .public)")
            return false
        }
    }

    func isFollowing(sessionId: String) -> Bool {
        Activity<FilActivityAttributes>.activities.contains {
            $0.attributes.sessionId == sessionId
        }
    }

    func update(
        sessionId: String,
        status: FilActivityStatus,
        cwd: String,
        processName: String,
        otherSessionCount: Int
    ) async {
        let preferences = FilActivityPreferences.current
        let state = FilActivityProjection.content(
            status: status,
            cwd: cwd,
            shell: processName,
            otherSessionCount: otherSessionCount,
            privacy: preferences.privacy
        )
        for activity in Activity<FilActivityAttributes>.activities
        where activity.attributes.sessionId == sessionId {
            await activity.update(content(for: state))
        }
    }

    func stopFollowing(sessionId: String? = nil) async {
        for activity in Activity<FilActivityAttributes>.activities
        where sessionId == nil || activity.attributes.sessionId == sessionId {
            await endActivity(activity, immediate: false)
        }
    }

    func endAllImmediately() async {
        for activity in Activity<FilActivityAttributes>.activities {
            await endActivity(activity, immediate: true)
        }
    }

    private func projectedState(
        session: Session,
        status: FilActivityStatus,
        otherSessionCount: Int
    ) -> FilActivityAttributes.ContentState {
        FilActivityProjection.content(
            status: status,
            cwd: session.cwd,
            shell: session.processName,
            otherSessionCount: otherSessionCount,
            privacy: FilActivityPreferences.current.privacy
        )
    }

    private func content(
        for state: FilActivityAttributes.ContentState
    ) -> ActivityContent<FilActivityAttributes.ContentState> {
        ActivityContent(state: state, staleDate: Date().addingTimeInterval(20))
    }

    private func startObserving(_ activity: Activity<FilActivityAttributes>) {
        guard observationTasks[activity.id] == nil else { return }
        let activityId = activity.id
        let sessionId = activity.attributes.sessionId
        let deviceId = activity.attributes.deviceId
        nonisolated(unsafe) let pushActivity = activity
        nonisolated(unsafe) let stateActivity = activity

        let tokenTask = Task { [weak self] in
            for await pushToken in pushActivity.pushTokenUpdates {
                await self?.register(
                    activityId: activityId,
                    sessionId: sessionId,
                    deviceId: deviceId,
                    pushToken: pushToken
                )
            }
        }
        let stateTask = Task { [weak self] in
            for await state in stateActivity.activityStateUpdates {
                if state == .dismissed || state == .ended {
                    await self?.unregister(activityId)
                    await self?.stopObserving(activityId)
                    break
                }
            }
        }
        observationTasks[activityId] = [tokenTask, stateTask]
    }

    private func stopObserving(_ activityId: String) {
        observationTasks.removeValue(forKey: activityId)?.forEach { $0.cancel() }
    }

    private func register(
        activityId: String,
        sessionId: String,
        deviceId: String,
        pushToken: Data
    ) async {
        guard let token = TokenStorage.loadToken() else { return }
        let hex = pushToken.map { String(format: "%02x", $0) }.joined()
        #if DEBUG
        let environment = "sandbox"
        #else
        let environment = "production"
        #endif
        do {
            try await HubClient().registerLiveActivity(
                .init(
                    activityId: activityId,
                    sessionId: sessionId,
                    deviceId: deviceId,
                    pushToken: hex,
                    environment: environment
                ),
                token: token
            )
        } catch {
            logger.error("Unable to register Live Activity push token: \(error.localizedDescription, privacy: .public)")
        }
    }

    private func endActivity(
        _ activity: Activity<FilActivityAttributes>,
        immediate: Bool
    ) async {
        let activityId = activity.id
        var state = activity.content.state
        state.status = .ended
        state.lastUpdatedAt = Date()
        await unregister(activityId)
        nonisolated(unsafe) let endingActivity = activity
        await endingActivity.end(
            ActivityContent(state: state, staleDate: nil),
            dismissalPolicy: immediate ? .immediate : .after(Date().addingTimeInterval(8))
        )
        stopObserving(activityId)
    }

    private func unregister(_ activityId: String) async {
        guard let token = TokenStorage.loadToken() else { return }
        do {
            try await HubClient().deleteLiveActivity(activityId: activityId, token: token)
        } catch {
            logger.error("Unable to unregister Live Activity: \(error.localizedDescription, privacy: .public)")
        }
    }
}
