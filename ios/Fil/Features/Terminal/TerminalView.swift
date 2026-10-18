import ActivityKit
import Combine
import ComposableArchitecture
import SwiftUI
import SwiftTerm
import UIKit

enum TerminalPresentation: Equatable {
    case modal
    case embedded
}

struct TerminalSearchCommand: Equatable {
    enum Direction {
        case next
        case previous
        case clear
    }

    let id = UUID()
    let term: String
    let direction: Direction
}

struct TerminalSessionView: View {
    @Bindable var store: StoreOf<TerminalFeature>
    @Environment(\.dismiss) private var dismiss
    @Environment(\.dynamicTypeSize) private var dynamicTypeSize
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    let presentation: TerminalPresentation

    @State private var currentFontSize: CGFloat = 14
    @State private var isKeyboardVisible = false
    @State private var isSearching = false
    @State private var searchText = ""
    @State private var searchCommand: TerminalSearchCommand?
    @State private var isReadingHistory = false
    @State private var scrollToLatestRequest = 0
    @FocusState private var isSearchFocused: Bool

    init(
        store: StoreOf<TerminalFeature>,
        presentation: TerminalPresentation = .modal
    ) {
        self.store = store
        self.presentation = presentation
    }

    var body: some View {
        ZStack {
            FilTheme.terminalBackground.ignoresSafeArea()

            VStack(spacing: 0) {
                topBar
                if store.showDisconnectedAlert {
                    disconnectedOverlay
                }
                if isSearching {
                    searchBar
                        .transition(reduceMotion ? .opacity : .move(edge: .top).combined(with: .opacity))
                }
                terminalArea
            }
        }
        .environment(\.colorScheme, .dark)
        .onAppear {
            currentFontSize = store.fontSize
            store.send(.onAppear)
        }
        .onDisappear { store.send(.onDisappear) }
        .onReceive(NotificationCenter.default.publisher(for: UIResponder.keyboardWillShowNotification)) {
            updateKeyboardVisibility(true, notification: $0)
        }
        .onReceive(NotificationCenter.default.publisher(for: UIResponder.keyboardWillHideNotification)) {
            updateKeyboardVisibility(false, notification: $0)
        }
        .alert("Live Activities unavailable", isPresented: liveActivityAlertBinding) {
            Button("OK", role: .cancel) {
                store.send(.dismissLiveActivityUnavailableAlert)
            }
        } message: {
            if !FilActivityPreferences.current.isEnabled || !ActivityAuthorizationInfo().areActivitiesEnabled {
                Text("Allow Live Activities in Fil settings and in iOS Settings, then try again.")
            } else {
                Text("Following could not start. Try again; iOS may have reached its Live Activity limit.")
            }
        }
    }

    // MARK: - Top Bar

    private var topBar: some View {
        Group {
            if dynamicTypeSize.isAccessibilitySize {
                VStack(spacing: 6) {
                    HStack {
                        closeButton
                        Spacer()
                        searchButton
                        followButton
                    }
                    terminalIdentity
                }
            } else {
                HStack(spacing: 8) {
                    closeButton
                    terminalIdentity
                        .frame(maxWidth: .infinity)
                    searchButton
                    followButton
                }
            }
        }
        .padding(.horizontal, 8)
        .padding(.vertical, isKeyboardVisible ? 4 : 6)
        .background(.ultraThinMaterial)
        .overlay(alignment: .bottom) {
            Divider().overlay(FilTheme.terminalForeground.opacity(0.08))
        }
    }

    private var closeButton: some View {
        Button {
            closeTerminal()
        } label: {
            Image(systemName: "chevron.down")
                .font(.system(size: 14, weight: .semibold))
                .foregroundStyle(.secondary)
                .frame(width: 44, height: 44)
                .contentShape(Circle())
        }
        .buttonStyle(.plain)
        .modifier(GlassCircleControl())
        .accessibilityLabel("Return to terminals")
        .accessibilityHint("Leaves this view without closing the remote shell")
    }

    private var searchButton: some View {
        Button {
            withAnimation(reduceMotion ? nil : .easeOut(duration: 0.18)) {
                isSearching.toggle()
            }
            isSearchFocused = isSearching
            if !isSearching {
                searchCommand = .init(term: "", direction: .clear)
                searchText = ""
            }
        } label: {
            Image(systemName: isSearching ? "xmark" : "magnifyingglass")
                .font(.system(size: 14, weight: .semibold))
                .foregroundStyle(.secondary)
                .frame(width: 44, height: 44)
                .contentShape(Circle())
        }
        .buttonStyle(.plain)
        .modifier(GlassCircleControl())
        .keyboardShortcut("f", modifiers: .command)
        .accessibilityLabel(isSearching ? "Close search" : "Search terminal output")
    }

    private var terminalIdentity: some View {
        Group {
            if store.availableSessions.count > 1 {
                Menu {
                    ForEach(store.availableSessions) { context in
                        Button {
                            store.send(.switchSession(context.id))
                        } label: {
                            Label(
                                "\(context.machineName) · \(context.session.projectName) · \(context.session.processName)",
                                systemImage: context.id == store.session.id ? "checkmark" : "terminal"
                            )
                        }
                    }

                    Divider()

                    Button {
                        UIPasteboard.general.string = store.session.cwd
                    } label: {
                        Label("Copy Path", systemImage: "doc.on.doc")
                    }
                } label: {
                    terminalIdentityLabel(showsDisclosure: true)
                }
                .buttonStyle(.plain)
                .accessibilityHint("Shows other active terminals")
            } else {
                terminalIdentityLabel(showsDisclosure: false)
                    .contextMenu {
                        Button {
                            UIPasteboard.general.string = store.session.cwd
                        } label: {
                            Label("Copy Path", systemImage: "doc.on.doc")
                        }

                        Text(store.session.cwd)
                    }
            }
        }
        .accessibilityElement(children: .combine)
        .accessibilityLabel(
            "\(store.machineName), \(store.session.projectName), \(store.session.detail), \(store.connectionState.accessibilityDescription)"
        )
    }

    private func terminalIdentityLabel(showsDisclosure: Bool) -> some View {
        VStack(spacing: 2) {
            HStack(spacing: 7) {
                // Amber while a retry is in flight: a brief blip should read as
                // "hang on" rather than as the red of a dead session.
                Circle()
                    .fill(store.connectionState.indicatorColor)
                    .frame(width: 7, height: 7)
                    .accessibilityHidden(true)

                Text(store.session.projectName)
                    .font(.body.weight(.semibold))
                    .foregroundStyle(.primary)
                    .lineLimit(dynamicTypeSize.isAccessibilitySize ? 2 : 1)

                if showsDisclosure {
                    Image(systemName: "chevron.down")
                        .font(.caption2.weight(.semibold))
                        .foregroundStyle(.secondary)
                }
            }

            Text(store.machineName)
                .font(.caption)
                .foregroundStyle(.secondary)
                .lineLimit(dynamicTypeSize.isAccessibilitySize ? 2 : 1)

            if !store.isConnected && !store.showDisconnectedAlert {
                Text(connectionStatusLabel)
                    .font(.caption)
                    .foregroundStyle(FilTheme.warning)
            }
        }
        .frame(maxWidth: .infinity)
    }

    private var followButton: some View {
        Button {
            store.send(.followTapped)
        } label: {
            Group {
                if store.isFollowRequestInFlight {
                    ProgressView()
                        .controlSize(.small)
                } else {
                    Image(systemName: store.isFollowing ? "checkmark.circle.fill" : "livephoto")
                        .font(.system(size: 15, weight: .medium))
                }
            }
            .foregroundStyle(store.isFollowing ? FilTheme.filGreen : Color(.secondaryLabel))
            .frame(minWidth: 44, minHeight: 44)
            .contentShape(Capsule())
        }
        .buttonStyle(.plain)
        .modifier(GlassCapsuleControl(isActive: store.isFollowing))
        .disabled(store.isFollowRequestInFlight)
        .accessibilityLabel(
            store.isFollowing ? "Stop following on Lock Screen" : "Follow on Lock Screen"
        )
        .accessibilityValue(store.isFollowing ? "Following" : "Not following")
        .accessibilityHint(
            store.isFollowing
                ? "Ends the Live Activity for this terminal"
                : "Keeps this terminal visible on the Lock Screen and Dynamic Island"
        )
    }

    private var searchBar: some View {
        HStack(spacing: 8) {
            TextField("Search output", text: $searchText)
                .textFieldStyle(.roundedBorder)
                .textInputAutocapitalization(.never)
                .autocorrectionDisabled()
                .submitLabel(.search)
                .focused($isSearchFocused)
                .onAppear { isSearchFocused = true }
                .onSubmit { performSearch(.next) }

            Button {
                performSearch(.previous)
            } label: {
                Image(systemName: "chevron.up")
                    .frame(width: 44, height: 44)
            }
            .buttonStyle(.plain)
            .disabled(searchText.isEmpty)
            .accessibilityLabel("Previous match")

            Button {
                performSearch(.next)
            } label: {
                Image(systemName: "chevron.down")
                    .frame(width: 44, height: 44)
            }
            .buttonStyle(.plain)
            .disabled(searchText.isEmpty)
            .accessibilityLabel("Next match")
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 7)
        .background(.ultraThinMaterial)
    }

    /*
     The header uses native iOS 26 glass when available and falls back to
     material-backed controls on iOS 17–18.
     */
    private struct GlassCircleControl: ViewModifier {
        @ViewBuilder
        func body(content: Content) -> some View {
            if #available(iOS 26.0, *) {
                content.glassEffect(.regular.interactive(), in: Circle())
            } else {
                content.background(.ultraThinMaterial, in: Circle())
            }
        }
    }

    private struct GlassCapsuleControl: ViewModifier {
        let isActive: Bool

        @ViewBuilder
        func body(content: Content) -> some View {
            if #available(iOS 26.0, *) {
                content.glassEffect(
                    isActive
                        ? .regular.tint(FilTheme.filGreen.opacity(0.16)).interactive()
                        : .regular.interactive(),
                    in: Capsule()
                )
            } else {
                content.background(
                    isActive
                        ? FilTheme.filGreen.opacity(0.12)
                        : FilTheme.terminalSurface.opacity(0.72),
                    in: Capsule()
                )
            }
        }
    }

    // MARK: - Terminal Area

    private var terminalArea: some View {
        SwiftTermWrapper(
            sessionId: store.session.id,
            fontSize: store.fontSize,
            onInput: { data in
                if store.isConnected && !isSearching {
                    store.send(.inputSent(data))
                }
            },
            onSizeChanged: { cols, rows in
                store.send(.terminalSizeChanged(cols: cols, rows: rows))
            },
            searchCommand: searchCommand,
            scrollToLatestRequest: scrollToLatestRequest,
            onReadingHistoryChanged: { isReadingHistory = $0 },
            onPinch: { state, scale in
                switch state {
                case .changed:
                    store.send(.fontSizeChanged(currentFontSize * scale))
                case .ended, .cancelled, .failed:
                    currentFontSize = store.fontSize
                default:
                    break
                }
            }
        )
        .id(store.session.id)
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .padding(.horizontal, 8)
        .padding(.top, 4)
        .overlay(alignment: .bottomTrailing) {
            if isReadingHistory && !isSearching {
                Button {
                    scrollToLatestRequest += 1
                } label: {
                    Label("Back to live output", systemImage: "arrow.down.to.line")
                        .font(.callout.weight(.semibold))
                        .padding(.horizontal, 14)
                        .frame(minHeight: 44)
                        .background(.regularMaterial, in: Capsule())
                }
                .buttonStyle(.plain)
                .padding(12)
            }
        }
    }

    // MARK: - Disconnected Overlay

    private var disconnectedOverlay: some View {
        VStack(alignment: .leading, spacing: 8) {
            Label("Connection lost", systemImage: "wifi.exclamationmark")
                .font(.headline)
            Text("The remote session state is unknown. Reconnect to check.")
                .font(.subheadline)
                .foregroundStyle(.secondary)
                .fixedSize(horizontal: false, vertical: true)
            ViewThatFits(in: .horizontal) {
                HStack(spacing: 12) { recoveryActions }
                VStack(alignment: .leading, spacing: 4) { recoveryActions }
            }
        }
        .padding(16)
        .frame(maxWidth: .infinity, alignment: .leading)
        .foregroundStyle(FilTheme.terminalForeground)
        .background(FilTheme.terminalSurface)
        .accessibilityElement(children: .contain)
    }

    @ViewBuilder
    private var recoveryActions: some View {
        Button("Reconnect") { store.send(.reconnectTapped) }
            .buttonStyle(.borderedProminent)
            .tint(FilTheme.filGreen)
            .foregroundStyle(FilTheme.terminalBackground)
            .frame(minHeight: 44)
        Button("Return to terminals") { closeTerminal() }
            .buttonStyle(.bordered)
            .frame(minHeight: 44)
    }

    private func closeTerminal() {
        if presentation == .embedded {
            store.send(.dismiss)
        } else {
            dismiss()
        }
    }

    private var connectionStatusLabel: LocalizedStringKey {
        switch store.connectionState {
        case .connected: "Connected"
        case .connecting: "Connecting…"
        case .reconnecting: "Reconnecting…"
        case .suspended: "Updates paused"
        case .unreachable: "Connection lost"
        }
    }

    private var liveActivityAlertBinding: Binding<Bool> {
        Binding(
            get: { store.showLiveActivityUnavailableAlert },
            set: { isPresented in
                if !isPresented {
                    store.send(.dismissLiveActivityUnavailableAlert)
                }
            }
        )
    }

    private func updateKeyboardVisibility(_ isVisible: Bool, notification: Notification) {
        let duration = notification.userInfo?[UIResponder.keyboardAnimationDurationUserInfoKey]
            as? Double ?? 0.25
        withAnimation(reduceMotion ? nil : .easeOut(duration: duration)) {
            isKeyboardVisible = isVisible
        }
    }

    private func performSearch(_ direction: TerminalSearchCommand.Direction) {
        guard !searchText.isEmpty else { return }
        searchCommand = .init(term: searchText, direction: direction)
    }
}

// MARK: - SwiftTerm UIKit Wrapper

struct SwiftTermWrapper: UIViewRepresentable {
    var sessionId: String
    var fontSize: CGFloat
    var onInput: ((Data) -> Void)?
    var onSizeChanged: ((Int, Int) -> Void)?
    var searchCommand: TerminalSearchCommand?
    var scrollToLatestRequest = 0
    var onReadingHistoryChanged: ((Bool) -> Void)?
    var onPinch: ((UIGestureRecognizer.State, CGFloat) -> Void)?

    func makeUIView(context: Context) -> SwiftTerm.TerminalView {
        let font = UIFont.monospacedSystemFont(ofSize: fontSize, weight: .regular)
        let tv = FilTerminalView(frame: .zero, font: font)
        tv.onReadingHistoryChanged = onReadingHistoryChanged
        tv.onPinch = onPinch
        tv.overrideUserInterfaceStyle = .dark
        tv.backgroundColor = UIColor(FilTheme.terminalBackground)
        tv.nativeForegroundColor = UIColor(FilTheme.terminalForeground)
        tv.nativeBackgroundColor = UIColor(FilTheme.terminalBackground)
        tv.contentInsetAdjustmentBehavior = .never
        tv.keyboardDismissMode = .interactive
        tv.inputAccessoryView = FilAccessoryView(terminalView: tv)
        context.coordinator.terminalView = tv
        context.coordinator.appliedFontSize = fontSize
        context.coordinator.onInput = onInput
        context.coordinator.onSizeChanged = onSizeChanged
        context.coordinator.apply(searchCommand, to: tv)
        let relay = TerminalConnectionRegistry.shared.outputRelay(sessionId: sessionId)
        context.coordinator.outputRelay = relay
        relay.attach(tv)
        tv.terminalDelegate = context.coordinator
        return tv
    }

    func updateUIView(_ uiView: SwiftTerm.TerminalView, context: Context) {
        if let terminal = uiView as? FilTerminalView {
            terminal.onReadingHistoryChanged = onReadingHistoryChanged
            terminal.onPinch = onPinch
            if context.coordinator.lastScrollToLatestRequest != scrollToLatestRequest {
                context.coordinator.lastScrollToLatestRequest = scrollToLatestRequest
                terminal.scrollToLatest()
            }
        }
        context.coordinator.onInput = onInput
        context.coordinator.onSizeChanged = onSizeChanged
        context.coordinator.apply(searchCommand, to: uiView)

        // Defence in depth. The relay is now stable for a session's lifetime,
        // but `makeUIView` used to be the only place it was ever resolved, so
        // any future lifecycle slip would silently leave this view wired to a
        // dead relay and the terminal would go blank while looking connected.
        let relay = TerminalConnectionRegistry.shared.outputRelay(sessionId: sessionId)
        if context.coordinator.outputRelay !== relay {
            context.coordinator.outputRelay?.detach(uiView)
            context.coordinator.outputRelay = relay
            relay.attach(uiView)
        }

        if context.coordinator.appliedFontSize != fontSize {
            context.coordinator.appliedFontSize = fontSize
            if let terminal = uiView as? FilTerminalView {
                terminal.preservingViewport {
                    terminal.font = UIFont.monospacedSystemFont(ofSize: fontSize, weight: .regular)
                }
            } else {
                uiView.font = UIFont.monospacedSystemFont(ofSize: fontSize, weight: .regular)
            }
        }
    }

    static func dismantleUIView(
        _ uiView: SwiftTerm.TerminalView,
        coordinator: Coordinator
    ) {
        coordinator.cancelPendingResize()
        coordinator.outputRelay?.detach(uiView)
        uiView.terminalDelegate = nil
    }

    func makeCoordinator() -> Coordinator {
        Coordinator()
    }

    class Coordinator: NSObject, SwiftTerm.TerminalViewDelegate {
        weak var terminalView: SwiftTerm.TerminalView?
        var outputRelay: TerminalOutputRelay?
        var appliedFontSize: CGFloat?
        var lastScrollToLatestRequest = 0
        var onInput: ((Data) -> Void)?
        var onSizeChanged: ((Int, Int) -> Void)?
        private var pendingResize: DispatchWorkItem?
        private var lastReportedSize: (cols: Int, rows: Int)?
        private var lastSearchCommandID: UUID?

        func scrolled(source: SwiftTerm.TerminalView, position: Double) {}
        func setTerminalTitle(source: SwiftTerm.TerminalView, title: String) {}
        func sizeChanged(source: SwiftTerm.TerminalView, newCols: Int, newRows: Int) {
            guard newCols > 0, newRows > 0 else { return }

            pendingResize?.cancel()
            let work = DispatchWorkItem { [weak self] in
                guard let self else { return }
                let size = (cols: newCols, rows: newRows)
                guard self.lastReportedSize?.cols != size.cols
                        || self.lastReportedSize?.rows != size.rows else {
                    return
                }
                self.lastReportedSize = size
                self.onSizeChanged?(size.cols, size.rows)
            }
            pendingResize = work
            DispatchQueue.main.asyncAfter(deadline: .now() + 0.15, execute: work)
        }
        func bell(source: SwiftTerm.TerminalView) {}
        func clipboardCopy(source: SwiftTerm.TerminalView, content: Data) {}
        func rangeChanged(source: SwiftTerm.TerminalView, startY: Int, endY: Int) {}
        func iTermContent(source: SwiftTerm.TerminalView, content: Data) {}

        func send(source: SwiftTerm.TerminalView, data: ArraySlice<UInt8>) {
            onInput?(Data(data))
        }

        func hostCurrentDirectoryUpdate(source: SwiftTerm.TerminalView, directory: String?) {}
        func requestOpenLink(source: SwiftTerm.TerminalView, link: String, params: [String : String]) {}

        func cancelPendingResize() {
            pendingResize?.cancel()
            pendingResize = nil
        }

        @MainActor
        func apply(_ command: TerminalSearchCommand?, to terminalView: SwiftTerm.TerminalView) {
            guard let command, command.id != lastSearchCommandID else { return }
            lastSearchCommandID = command.id
            switch command.direction {
            case .next:
                terminalView.findNext(command.term)
            case .previous:
                terminalView.findPrevious(command.term)
            case .clear:
                terminalView.clearSearch()
            }
        }
    }
}

// MARK: - Scroll Gesture

/// Where a one-finger vertical drag goes. Full-screen apps (Claude Code,
/// vim, less, tmux) run on the alternate screen, which has no local
/// scrollback, so the drag has to become input for the application.
enum TerminalScrollRoute: Equatable {
    /// UIScrollView scrolls SwiftTerm's local scrollback.
    case scrollback
    /// The application asked for mouse reports: send wheel events.
    case mouseWheel
    /// Alternate screen without mouse reporting: send cursor up/down, as
    /// xterm's alternateScroll mode and iTerm/Terminal.app do.
    case arrowKeys

    init(mouseReporting: Bool, alternateScreen: Bool) {
        if mouseReporting {
            self = .mouseWheel
        } else if alternateScreen {
            self = .arrowKeys
        } else {
            self = .scrollback
        }
    }

    static func arrowKeys(lines: Int, applicationCursor: Bool) -> [UInt8] {
        let key = switch (lines > 0, applicationCursor) {
        case (true, true): EscapeSequences.moveUpApp
        case (true, false): EscapeSequences.moveUpNormal
        case (false, true): EscapeSequences.moveDownApp
        case (false, false): EscapeSequences.moveDownNormal
        }
        return Array(repeating: key, count: abs(lines)).flatMap { $0 }
    }
}

/// Turns finger travel into whole terminal lines, one line per row height so
/// the content follows the finger. Claude Code scrolls one line per wheel
/// event in terminals it does not special-case.
struct TerminalLineAccumulator {
    var lineHeight: CGFloat {
        didSet { lineHeight = max(1, lineHeight) }
    }
    private(set) var remainder: CGFloat = 0

    init(lineHeight: CGFloat) {
        self.lineHeight = max(1, lineHeight)
    }

    /// `delta` > 0 means the finger moved down, revealing older content.
    /// Returns the lines to scroll back (> 0) or forward (< 0).
    mutating func lines(for delta: CGFloat) -> Int {
        remainder += delta
        let lines = Int(remainder / lineHeight)
        remainder -= CGFloat(lines) * lineHeight
        return lines
    }

    mutating func reset() {
        remainder = 0
    }
}

/// UIScrollView-style deceleration for drags that UIScrollView cannot
/// perform itself because they become application input.
struct TerminalScrollMomentum {
    static let decelerationRate = UIScrollView.DecelerationRate.normal.rawValue
    static let restingVelocity: CGFloat = 20

    private(set) var velocity: CGFloat

    init(velocity: CGFloat) {
        self.velocity = velocity
    }

    var isSettled: Bool {
        abs(velocity) < Self.restingVelocity
    }

    /// Distance travelled during `interval`, decaying velocity per ms like
    /// UIScrollView does.
    mutating func advance(by interval: TimeInterval) -> CGFloat {
        guard !isSettled, interval > 0 else { return 0 }
        let decay = pow(Self.decelerationRate, CGFloat(interval * 1000))
        let distance = velocity * (decay - 1) / (1000 * log(Self.decelerationRate))
        velocity *= decay
        return distance
    }
}

/// SwiftTerm 1.13 updates its scroller to the bottom on every output scroll.
/// Keep the user's top visible row instead, without stopping UIKit's drag or
/// deceleration. The dependency itself stays untouched.
///
/// It also owns the vertical drag: SwiftTerm turns a pan into a mouse
/// button-1 drag as soon as the application enables mouse reporting, and the
/// alternate screen has nothing for UIScrollView to scroll. Here a one-finger
/// drag scrolls local history on the normal screen and becomes wheel events
/// or arrow keys for full-screen applications, with momentum either way.
final class FilTerminalView: SwiftTerm.TerminalView {
    private var viewportReady = false
    var onReadingHistoryChanged: ((Bool) -> Void)?
    var onPinch: ((UIGestureRecognizer.State, CGFloat) -> Void)?
    private(set) var applicationScrollGesture: UIPanGestureRecognizer!
    private(set) var pinchGesture: UIPinchGestureRecognizer!
    private var lineAccumulator = TerminalLineAccumulator(lineHeight: 1)
    private var momentum: TerminalScrollMomentum?
    private var momentumLink: CADisplayLink?
    private var momentumRoute: TerminalScrollRoute?
    private var scrollLocation: CGPoint = .zero
    private var protectingViewport = false
    private var oldestLineIndex = 0
    private var observedBuffer: SwiftTerm.Buffer?
    private var bufferGeneration = 0
    private var lastReportedReadingHistory = false
    private var protectedSizeUpdates = 0

    override init(frame: CGRect, font: UIFont?) {
        super.init(frame: frame, font: font)
        viewportReady = true
        setupScrollGestures()
    }

    override init(frame: CGRect) {
        super.init(frame: frame)
        viewportReady = true
        setupScrollGestures()
    }

    required init?(coder: NSCoder) {
        super.init(coder: coder)
        viewportReady = true
        setupScrollGestures()
    }

    var rowHeight: CGFloat {
        max(1, ceil(CTFontGetAscent(font) + CTFontGetDescent(font) + CTFontGetLeading(font)))
    }

    var scrollRoute: TerminalScrollRoute {
        let terminal = getTerminal()
        return TerminalScrollRoute(
            mouseReporting: allowMouseReporting && terminal.mouseMode != .off,
            alternateScreen: terminal.isCurrentBufferAlternate
        )
    }

    // MARK: Gestures

    private func setupScrollGestures() {
        let pan = UIPanGestureRecognizer(target: self, action: #selector(handleApplicationScroll(_:)))
        pan.maximumNumberOfTouches = 1
        addGestureRecognizer(pan)
        applicationScrollGesture = pan

        // Pinch lives here rather than as a SwiftUI gesture on an ancestor,
        // so every terminal gesture is arbitrated by one UIKit view.
        let pinch = UIPinchGestureRecognizer(target: self, action: #selector(handlePinch(_:)))
        addGestureRecognizer(pinch)
        pinchGesture = pinch
    }

    override func mouseModeChanged(source: SwiftTerm.Terminal) {
        // Deliberately not calling super: SwiftTerm would install a pan that
        // reports a button-1 drag, which steals every scroll from Claude
        // Code and other mouse-aware apps. Taps still send clicks.
        if source.mouseMode == .off, momentumRoute == .mouseWheel {
            stopMomentum()
        }
    }

    override func gestureRecognizerShouldBegin(_ gestureRecognizer: UIGestureRecognizer) -> Bool {
        if gestureRecognizer === panGestureRecognizer {
            guard scrollRoute == .scrollback else { return false }
        } else if gestureRecognizer === applicationScrollGesture {
            guard scrollRoute != .scrollback, !selectionActive else { return false }
            let velocity = applicationScrollGesture.velocity(in: self)
            return abs(velocity.y) > abs(velocity.x)
        } else if type(of: gestureRecognizer) == UIPanGestureRecognizer.self {
            // SwiftTerm's selection pan (a plain UIPanGestureRecognizer) stays
            // installed after a selection is dismissed; it must not compete
            // with scrolling once the selection is gone.
            guard selectionActive else { return false }
        }
        return super.gestureRecognizerShouldBegin(gestureRecognizer)
    }

    override func touchesBegan(_ touches: Set<UITouch>, with event: UIEvent?) {
        // A touch catches a flick, as in any UIScrollView.
        stopMomentum()
        super.touchesBegan(touches, with: event)
    }

    override func didMoveToWindow() {
        super.didMoveToWindow()
        if window == nil { stopMomentum() }
    }

    @objc private func handlePinch(_ gesture: UIPinchGestureRecognizer) {
        onPinch?(gesture.state, gesture.scale)
    }

    @objc private func handleApplicationScroll(_ gesture: UIPanGestureRecognizer) {
        switch gesture.state {
        case .began:
            stopMomentum()
            lineAccumulator.reset()
            scrollLocation = gesture.location(in: self)
            applyApplicationScroll(gesture.translation(in: self).y, route: scrollRoute)
            gesture.setTranslation(.zero, in: self)
        case .changed:
            scrollLocation = gesture.location(in: self)
            applyApplicationScroll(gesture.translation(in: self).y, route: scrollRoute)
            gesture.setTranslation(.zero, in: self)
        case .ended:
            startMomentum(velocity: gesture.velocity(in: self).y)
        default:
            lineAccumulator.reset()
        }
    }

    /// Sends `distance` points of finger travel to the application as whole
    /// lines. Positive distance reveals older content.
    func applyApplicationScroll(_ distance: CGFloat, route: TerminalScrollRoute) {
        let terminal = getTerminal()
        lineAccumulator.lineHeight = rowHeight
        let lines = max(-terminal.rows, min(terminal.rows, lineAccumulator.lines(for: distance)))
        guard lines != 0 else { return }
        switch route {
        case .scrollback:
            return
        case .arrowKeys:
            send(TerminalScrollRoute.arrowKeys(lines: lines, applicationCursor: terminal.applicationCursor))
        case .mouseWheel:
            let col = min(max(0, Int(scrollLocation.x / max(1, bounds.width) * CGFloat(terminal.cols))),
                          terminal.cols - 1)
            let row = min(max(0, Int((scrollLocation.y - contentOffset.y) / rowHeight)), terminal.rows - 1)
            let flags = terminal.encodeButton(
                button: lines > 0 ? 4 : 5, release: false, shift: false, meta: false, control: false
            )
            for _ in 0..<abs(lines) {
                terminal.sendEvent(
                    buttonFlags: flags, x: col, y: row,
                    pixelX: Int(scrollLocation.x), pixelY: Int(scrollLocation.y - contentOffset.y)
                )
            }
        }
    }

    private func startMomentum(velocity: CGFloat) {
        let momentum = TerminalScrollMomentum(velocity: velocity)
        guard !momentum.isSettled else {
            lineAccumulator.reset()
            return
        }
        self.momentum = momentum
        momentumRoute = scrollRoute
        let link = CADisplayLink(target: DisplayLinkProxy(self), selector: #selector(DisplayLinkProxy.tick(_:)))
        link.add(to: .main, forMode: .common)
        momentumLink = link
    }

    fileprivate func momentumTick(_ link: CADisplayLink) {
        guard var momentum, let route = momentumRoute, route == scrollRoute else {
            stopMomentum()
            return
        }
        let distance = momentum.advance(by: link.targetTimestamp - link.timestamp)
        self.momentum = momentum
        applyApplicationScroll(distance, route: route)
        if momentum.isSettled { stopMomentum() }
    }

    private func stopMomentum() {
        momentumLink?.invalidate()
        momentumLink = nil
        momentum = nil
        momentumRoute = nil
    }

    var isReadingHistory: Bool {
        viewportReady && !getTerminal().isCurrentBufferAlternate
            && contentSize.height - bounds.height - contentOffset.y > rowHeight
    }

    override var contentOffset: CGPoint {
        get { super.contentOffset }
        set {
            // Do not set-and-restore: even a temporary snap cancels momentum.
            guard !protectingViewport, protectedSizeUpdates == 0 else { return }
            super.contentOffset = newValue
            reportReadingPosition()
        }
    }

    override func layoutSubviews() {
        preservingViewport { super.layoutSubviews() }
    }

    override func bufferActivated(source: SwiftTerm.Terminal) {
        bufferGeneration &+= 1
        // Alternate screens are a different viewport, never old scrollback.
        let wasProtecting = protectingViewport
        protectingViewport = false
        super.bufferActivated(source: source)
        protectingViewport = wasProtecting
    }

    override func sizeChanged(source: SwiftTerm.Terminal) {
        // SwiftTerm schedules another updateScroller on the next main turn
        // after a font resize. Keep that deferred update from undoing the
        // viewport restored synchronously below; still run its delegate and
        // geometry updates normally.
        let protect = viewportReady && (protectingViewport || isReadingHistory)
        if protect { protectedSizeUpdates += 1 }
        super.sizeChanged(source: source)
        if protect {
            DispatchQueue.main.async { [weak self] in
                guard let self else { return }
                self.protectedSizeUpdates -= 1
                self.reportReadingPosition()
            }
        }
    }

    func receiveOutput(_ bytes: ArraySlice<UInt8>) {
        preservingViewport(outputByteCount: bytes.count) { feed(byteArray: bytes) }
    }

    func scrollToLatest() {
        setContentOffset(CGPoint(x: 0, y: max(0, contentSize.height - bounds.height)), animated: false)
        reportReadingPosition()
    }

    func preservingViewport(outputByteCount: Int = 0, _ update: () -> Void) {
        guard viewportReady, !protectingViewport else { update(); return }
        let terminal = getTerminal()
        if observedBuffer !== terminal.buffer {
            observedBuffer = terminal.buffer
            oldestLineIndex = 0
        }
        let buffer = terminal.buffer
        let generation = bufferGeneration
        let preserve = isReadingHistory || (isDragging && !terminal.isCurrentBufferAlternate)
        let height = rowHeight
        let topRow = max(0, contentOffset.y / height)
        let absoluteRow = CGFloat(oldestLineIndex) + topRow
        let previousOffset = contentOffset
        protectingViewport = preserve
        update()
        protectingViewport = false

        // The public invariant-line API exposes which rows survived a full
        // circular buffer. At most one row can be discarded per output byte.
        // This keeps a retained line anchored when old history is evicted.
        if observedBuffer !== terminal.buffer {
            observedBuffer = terminal.buffer
            oldestLineIndex = 0
        }
        if terminal.getScrollInvariantLine(row: oldestLineIndex) == nil {
            if terminal.getScrollInvariantLine(row: 0) != nil {
                oldestLineIndex = 0
            } else if outputByteCount > 0 {
                for _ in 0...outputByteCount {
                    guard terminal.getScrollInvariantLine(row: oldestLineIndex) == nil else { break }
                    oldestLineIndex += 1
                }
            }
        }
        if preserve, buffer === terminal.buffer, generation == bufferGeneration,
           !terminal.isCurrentBufferAlternate {
            let y = min(max(0, (absoluteRow - CGFloat(oldestLineIndex)) * rowHeight),
                        max(0, contentSize.height - bounds.height))
            let restored = CGPoint(x: previousOffset.x, y: y)
            if super.contentOffset != restored { super.contentOffset = restored }
            setNeedsDisplay()
        }
        reportReadingPosition()
    }

    private func reportReadingPosition() {
        guard viewportReady else { return }
        let reading = isReadingHistory
        guard reading != lastReportedReadingHistory else { return }
        lastReportedReadingHistory = reading
        // Avoid publishing SwiftUI state in updateUIView/layoutSubviews.
        DispatchQueue.main.async { [weak self] in
            guard let self, self.lastReportedReadingHistory == reading else { return }
            self.onReadingHistoryChanged?(reading)
        }
    }
}

/// CADisplayLink retains its target; this keeps it from retaining the view.
@MainActor
private final class DisplayLinkProxy {
    private weak var view: FilTerminalView?

    init(_ view: FilTerminalView) {
        self.view = view
    }

    @objc func tick(_ link: CADisplayLink) {
        guard let view else {
            link.invalidate()
            return
        }
        view.momentumTick(link)
    }
}

/// Receives high-frequency QUIC output without invalidating the SwiftUI/TCA tree.
/// Chunks are coalesced and fed to SwiftTerm once per main-run-loop turn.
final class TerminalOutputRelay: @unchecked Sendable {
    private let lock = NSLock()
    private weak var terminalView: SwiftTerm.TerminalView?
    private var pending = Data()
    private var flushScheduled = false

    func attach(_ terminalView: SwiftTerm.TerminalView) {
        let shouldSchedule = withLock {
            self.terminalView = terminalView
            guard !pending.isEmpty, !flushScheduled else { return false }
            flushScheduled = true
            return true
        }
        if shouldSchedule {
            scheduleFlush()
        }
    }

    func detach(_ terminalView: SwiftTerm.TerminalView) {
        withLock {
            if self.terminalView === terminalView {
                self.terminalView = nil
            }
        }
    }

    func enqueue(_ data: Data) {
        guard !data.isEmpty else { return }
        let shouldSchedule = withLock {
            pending.append(data)
            guard terminalView != nil, !flushScheduled else { return false }
            flushScheduled = true
            return true
        }
        if shouldSchedule {
            scheduleFlush()
        }
    }

    private func scheduleFlush() {
        DispatchQueue.main.async { [weak self] in
            self?.flush()
        }
    }

    @MainActor
    private func flush() {
        let result: (SwiftTerm.TerminalView?, Data) = withLock {
            guard let terminalView = self.terminalView else {
                flushScheduled = false
                return (nil, Data())
            }
            let data = pending
            pending.removeAll(keepingCapacity: true)
            flushScheduled = false
            return (terminalView, data)
        }
        let (terminalView, data) = result

        guard let terminalView, !data.isEmpty else { return }
        let bytes = [UInt8](data)
        if let terminal = terminalView as? FilTerminalView {
            terminal.receiveOutput(bytes[...])
        } else {
            terminalView.feed(byteArray: bytes[...])
        }
    }

    private func withLock<T>(_ body: () -> T) -> T {
        lock.lock()
        defer { lock.unlock() }
        return body()
    }
}

// MARK: - Fil Accessory View (UIKit, replaces SwiftTerm's native accessory)

final class FilAccessoryView: UIInputView {
    private weak var terminalView: SwiftTerm.TerminalView?
    private weak var ctrlButton: UIButton?

    private var repeatTimer: Timer?
    private var repeatTask: Task<(), Never>?

    private let keyBg = UIColor.tertiarySystemFill
    private let keyFg = UIColor.label.withAlphaComponent(0.82)
    private let accentColor = UIColor(FilTheme.filGreen)

    init(terminalView: SwiftTerm.TerminalView) {
        self.terminalView = terminalView
        super.init(frame: CGRect(x: 0, y: 0, width: 0, height: 52), inputViewStyle: .keyboard)
        overrideUserInterfaceStyle = .dark
        allowsSelfSizing = true
        autoresizingMask = [.flexibleWidth]
        backgroundColor = .clear
        buildKeys()
    }

    required init?(coder: NSCoder) { fatalError() }

    override var intrinsicContentSize: CGSize {
        CGSize(width: UIView.noIntrinsicMetric, height: 52)
    }

    private func buildKeys() {
        let backdrop = UIVisualEffectView(effect: UIBlurEffect(style: .systemChromeMaterialDark))
        backdrop.translatesAutoresizingMaskIntoConstraints = false
        addSubview(backdrop)

        let scrollView = UIScrollView()
        scrollView.translatesAutoresizingMaskIntoConstraints = false
        scrollView.showsHorizontalScrollIndicator = false
        scrollView.alwaysBounceHorizontal = true
        scrollView.isDirectionalLockEnabled = true
        scrollView.delaysContentTouches = false
        scrollView.contentInsetAdjustmentBehavior = .never
        addSubview(scrollView)

        let stack = UIStackView()
        stack.axis = .horizontal
        stack.alignment = .center
        stack.spacing = 5
        stack.distribution = .fill
        stack.translatesAutoresizingMaskIntoConstraints = false
        scrollView.addSubview(stack)

        let hideKeyboardButton = makeKey(
            icon: "keyboard.chevron.compact.down",
            accessibilityLabel: String(localized: "Hide Keyboard"),
            action: #selector(hideKeyboard),
            width: 50
        )
        addSubview(hideKeyboardButton)

        addKey(
            to: stack,
            label: "esc",
            accessibilityLabel: String(localized: "Escape"),
            action: #selector(tapEsc)
        )
        addKey(
            to: stack,
            icon: "arrow.right.to.line.compact",
            accessibilityLabel: String(localized: "Tab"),
            action: #selector(tapTab)
        )
        ctrlButton = addKey(
            to: stack,
            label: "ctrl",
            accessibilityLabel: String(localized: "Control"),
            action: #selector(tapCtrl),
            width: 52
        )
        addKey(
            to: stack,
            icon: "doc.on.clipboard",
            accessibilityLabel: String(localized: "Paste"),
            action: #selector(pasteClipboard)
        )
        stack.addArrangedSubview(makeSeparator())

        addKey(to: stack, label: "~", accessibilityLabel: String(localized: "Tilde"), action: #selector(tapTilde))
        addKey(to: stack, label: "|", accessibilityLabel: String(localized: "Pipe"), action: #selector(tapPipe))
        addKey(to: stack, label: "/", accessibilityLabel: String(localized: "Slash"), action: #selector(tapSlash))
        addKey(to: stack, label: "-", accessibilityLabel: String(localized: "Dash"), action: #selector(tapDash))
        stack.addArrangedSubview(makeSeparator())

        addKey(
            to: stack,
            icon: "arrow.left",
            accessibilityLabel: String(localized: "Left Arrow"),
            action: #selector(tapLeft),
            autoRepeat: true
        )
        addKey(
            to: stack,
            icon: "arrow.down",
            accessibilityLabel: String(localized: "Down Arrow"),
            action: #selector(tapDown),
            autoRepeat: true
        )
        addKey(
            to: stack,
            icon: "arrow.up",
            accessibilityLabel: String(localized: "Up Arrow"),
            action: #selector(tapUp),
            autoRepeat: true
        )
        addKey(
            to: stack,
            icon: "arrow.right",
            accessibilityLabel: String(localized: "Right Arrow"),
            action: #selector(tapRight),
            autoRepeat: true
        )
        addKey(
            to: stack,
            icon: "arrow.down.to.line.compact",
            accessibilityLabel: String(localized: "Scroll to Bottom"),
            action: #selector(scrollToBottom)
        )

        NSLayoutConstraint.activate([
            backdrop.leadingAnchor.constraint(equalTo: leadingAnchor),
            backdrop.trailingAnchor.constraint(equalTo: trailingAnchor),
            backdrop.topAnchor.constraint(equalTo: topAnchor),
            backdrop.bottomAnchor.constraint(equalTo: bottomAnchor),

            scrollView.leadingAnchor.constraint(equalTo: leadingAnchor),
            scrollView.trailingAnchor.constraint(equalTo: hideKeyboardButton.leadingAnchor, constant: -4),
            scrollView.topAnchor.constraint(equalTo: topAnchor),
            scrollView.bottomAnchor.constraint(equalTo: bottomAnchor),

            hideKeyboardButton.trailingAnchor.constraint(equalTo: trailingAnchor, constant: -8),
            hideKeyboardButton.centerYAnchor.constraint(equalTo: centerYAnchor),

            stack.leadingAnchor.constraint(equalTo: scrollView.contentLayoutGuide.leadingAnchor, constant: 8),
            stack.trailingAnchor.constraint(equalTo: scrollView.contentLayoutGuide.trailingAnchor, constant: -8),
            stack.topAnchor.constraint(equalTo: scrollView.contentLayoutGuide.topAnchor, constant: 4),
            stack.bottomAnchor.constraint(equalTo: scrollView.contentLayoutGuide.bottomAnchor, constant: -4),
            stack.heightAnchor.constraint(equalToConstant: 44),
        ])
    }

    @discardableResult
    private func addKey(
        to stack: UIStackView,
        label: String? = nil,
        icon: String? = nil,
        accessibilityLabel: String,
        action: Selector,
        autoRepeat: Bool = false,
        width: CGFloat = 46
    ) -> UIButton {
        let button = makeKey(
            label: label,
            icon: icon,
            accessibilityLabel: accessibilityLabel,
            action: action,
            autoRepeat: autoRepeat,
            width: width
        )
        stack.addArrangedSubview(button)
        return button
    }

    private func makeKey(
        label: String? = nil,
        icon: String? = nil,
        accessibilityLabel: String,
        action: Selector,
        autoRepeat: Bool = false,
        width: CGFloat = 46
    ) -> UIButton {
        let btn = UIButton(type: .system)
        btn.translatesAutoresizingMaskIntoConstraints = false
        btn.layer.cornerRadius = 10
        btn.layer.masksToBounds = true
        btn.backgroundColor = keyBg
        btn.tintColor = keyFg
        btn.titleLabel?.font = .monospacedSystemFont(ofSize: 13, weight: .medium)
        btn.setTitleColor(keyFg, for: .normal)
        btn.accessibilityLabel = accessibilityLabel

        if let icon {
            let config = UIImage.SymbolConfiguration(pointSize: 13, weight: .medium)
            btn.setImage(UIImage(systemName: icon, withConfiguration: config), for: .normal)
        }
        if let label {
            btn.setTitle(label, for: .normal)
        }

        if autoRepeat {
            btn.addTarget(self, action: action, for: .touchDown)
            btn.addTarget(self, action: #selector(cancelRepeat), for: [.touchUpInside, .touchUpOutside, .touchCancel])
        } else {
            btn.addTarget(self, action: action, for: .touchUpInside)
        }

        NSLayoutConstraint.activate([
            btn.widthAnchor.constraint(equalToConstant: width),
            btn.heightAnchor.constraint(equalToConstant: 44),
        ])
        return btn
    }

    private func makeSeparator() -> UIView {
        let container = UIView()
        container.translatesAutoresizingMaskIntoConstraints = false

        let line = UIView()
        line.translatesAutoresizingMaskIntoConstraints = false
        line.backgroundColor = UIColor.separator.withAlphaComponent(0.45)
        container.addSubview(line)

        NSLayoutConstraint.activate([
            container.widthAnchor.constraint(equalToConstant: 8),
            container.heightAnchor.constraint(equalToConstant: 44),
            line.centerXAnchor.constraint(equalTo: container.centerXAnchor),
            line.centerYAnchor.constraint(equalTo: container.centerYAnchor),
            line.widthAnchor.constraint(equalToConstant: 1),
            line.heightAnchor.constraint(equalToConstant: 24),
        ])
        return container
    }

    // MARK: - Key Actions

    @objc private func tapEsc() {
        haptic()
        terminalView?.send([0x1b])
    }

    @objc private func tapTab() {
        haptic()
        terminalView?.send([0x09])
    }

    @objc private func tapCtrl() {
        haptic()
        guard let tv = terminalView else { return }
        tv.controlModifier.toggle()
        updateCtrlAppearance(isActive: tv.controlModifier)
    }

    @objc private func tapTilde() {
        haptic()
        terminalView?.send(txt: "~")
    }

    @objc private func tapPipe() {
        haptic()
        terminalView?.send(txt: "|")
    }

    @objc private func tapSlash() {
        haptic()
        terminalView?.send(txt: "/")
    }

    @objc private func tapDash() {
        haptic()
        terminalView?.send(txt: "-")
    }

    @objc private func tapLeft() {
        startRepeat { self.terminalView?.send([0x1b, 0x5b, 0x44]) }
    }

    @objc private func tapDown() {
        startRepeat { self.terminalView?.send([0x1b, 0x5b, 0x42]) }
    }

    @objc private func tapUp() {
        startRepeat { self.terminalView?.send([0x1b, 0x5b, 0x41]) }
    }

    @objc private func tapRight() {
        startRepeat { self.terminalView?.send([0x1b, 0x5b, 0x43]) }
    }

    @objc private func hideKeyboard() {
        haptic()
        _ = terminalView?.resignFirstResponder()
    }

    @objc private func pasteClipboard() {
        haptic()
        terminalView?.paste(nil)
    }

    @objc private func scrollToBottom() {
        haptic()
        guard let terminalView else { return }
        if let terminalView = terminalView as? FilTerminalView {
            terminalView.scrollToLatest()
            return
        }
        let y = max(
            -terminalView.adjustedContentInset.top,
            terminalView.contentSize.height
                - terminalView.bounds.height
                + terminalView.adjustedContentInset.bottom
        )
        terminalView.setContentOffset(
            CGPoint(x: terminalView.contentOffset.x, y: y),
            animated: !UIAccessibility.isReduceMotionEnabled
        )
    }

    // MARK: - Auto-Repeat

    private func startRepeat(_ action: @escaping @MainActor @Sendable () -> Void) {
        haptic()
        action()
        repeatTask = Task {
            try? await Task.sleep(nanoseconds: 500_000_000)
            guard !(repeatTask?.isCancelled ?? true) else { return }
            await MainActor.run {
                self.repeatTimer = Timer.scheduledTimer(withTimeInterval: 0.08, repeats: true) { _ in
                    Task { @MainActor in action() }
                }
            }
        }
    }

    @objc private func cancelRepeat() {
        repeatTimer?.invalidate()
        repeatTimer = nil
        repeatTask?.cancel()
        repeatTask = nil
    }

    // MARK: - Observe ctrl reset from SwiftTerm

    override func didMoveToWindow() {
        super.didMoveToWindow()
        NotificationCenter.default.removeObserver(
            self,
            name: .terminalViewControlModifierReset,
            object: terminalView
        )
        guard window != nil else { return }
        NotificationCenter.default.addObserver(
            self,
            selector: #selector(ctrlReset),
            name: .terminalViewControlModifierReset,
            object: terminalView
        )
    }

    @objc private func ctrlReset() {
        updateCtrlAppearance(isActive: false)
    }

    private func updateCtrlAppearance(isActive: Bool) {
        ctrlButton?.backgroundColor = isActive ? accentColor.withAlphaComponent(0.18) : keyBg
        ctrlButton?.setTitleColor(isActive ? accentColor : keyFg, for: .normal)
        ctrlButton?.accessibilityValue = isActive ? "On" : "Off"
    }

    private func haptic() {
        UIImpactFeedbackGenerator(style: .light).impactOccurred()
    }

    deinit {
        NotificationCenter.default.removeObserver(self)
    }
}

extension TerminalConnectionState {
    /// Green when live, amber while a retry is in flight, red only once the
    /// retries have given up. Previously any non-connected state was red,
    /// which made a one-second blip look like a dead session.
    // SwiftTerm also exports a `Color`, hence the qualification.
    var indicatorColor: SwiftUI.Color {
        switch self {
        case .connected: FilTheme.filGreen
        case .connecting, .reconnecting, .suspended: FilTheme.warning
        case .unreachable: FilTheme.error
        }
    }
}
