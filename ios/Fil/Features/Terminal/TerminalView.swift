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
    let presentation: TerminalPresentation

    @State private var currentFontSize: CGFloat = 14
    @State private var isKeyboardVisible = false
    @State private var isSearching = false
    @State private var searchText = ""
    @State private var searchCommand: TerminalSearchCommand?

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
                if isSearching {
                    searchBar
                        .transition(.move(edge: .top).combined(with: .opacity))
                }
                terminalArea
            }

            if store.showDisconnectedAlert {
                disconnectedOverlay
            }
        }
        .gesture(
            MagnifyGesture()
                .onChanged { value in
                    let newSize = currentFontSize * value.magnification
                    store.send(.fontSizeChanged(newSize))
                }
                .onEnded { _ in
                    currentFontSize = store.fontSize
                }
        )
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
            Text("Allow Live Activities in Fil settings and in iOS Settings, then try again.")
        }
    }

    // MARK: - Top Bar

    private var topBar: some View {
        ZStack {
            terminalIdentity
                .padding(.horizontal, store.isFollowing ? 154 : 112)

            HStack(spacing: 8) {
                closeButton

                Spacer(minLength: 0)

                searchButton
                followButton
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
            if presentation == .embedded {
                store.send(.dismiss)
            } else {
                dismiss()
            }
        } label: {
            Image(systemName: "chevron.down")
                .font(.system(size: 14, weight: .semibold))
                .foregroundStyle(.secondary)
                .frame(width: 44, height: 44)
                .contentShape(Circle())
        }
        .buttonStyle(.plain)
        .modifier(GlassCircleControl())
        .accessibilityLabel("Close terminal")
    }

    private var searchButton: some View {
        Button {
            withAnimation(.easeOut(duration: 0.18)) {
                isSearching.toggle()
            }
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
                                "\(context.session.projectName) · \(context.session.processName)",
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
            "\(store.session.projectName), \(store.session.detail), \(store.connectionState.accessibilityDescription)"
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
                    .lineLimit(1)

                if showsDisclosure {
                    Image(systemName: "chevron.down")
                        .font(.caption2.weight(.semibold))
                        .foregroundStyle(.secondary)
                }
            }

            if !isKeyboardVisible {
                Text("\(store.session.processName) · \(store.machineName)")
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
                    .transition(.opacity.combined(with: .move(edge: .top)))
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
                } else if store.isFollowing {
                    HStack(spacing: 5) {
                        Image(systemName: "livephoto")
                        Text("Following")
                    }
                    .font(.caption.weight(.semibold))
                } else {
                    Image(systemName: "livephoto")
                        .font(.system(size: 15, weight: .medium))
                }
            }
            .foregroundStyle(store.isFollowing ? FilTheme.filGreen : Color(.secondaryLabel))
            .frame(minWidth: 44, minHeight: 44)
            .padding(.horizontal, store.isFollowing ? 8 : 0)
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
                .onSubmit { performSearch(.next) }

            Button {
                performSearch(.previous)
            } label: {
                Image(systemName: "chevron.up")
                    .frame(width: 36, height: 36)
            }
            .buttonStyle(.plain)
            .disabled(searchText.isEmpty)
            .accessibilityLabel("Previous match")

            Button {
                performSearch(.next)
            } label: {
                Image(systemName: "chevron.down")
                    .frame(width: 36, height: 36)
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
                store.send(.inputSent(data))
            },
            onSizeChanged: { cols, rows in
                store.send(.terminalSizeChanged(cols: cols, rows: rows))
            },
            searchCommand: searchCommand
        )
        .id(store.session.id)
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .padding(.horizontal, 8)
        .padding(.top, 4)
    }

    // MARK: - Disconnected Overlay

    private var disconnectedOverlay: some View {
        ZStack {
            Color.black.opacity(0.6).ignoresSafeArea()

            VStack(spacing: 20) {
                Image(systemName: "wifi.exclamationmark")
                    .font(.system(size: 32))
                    .foregroundStyle(FilTheme.error)

                VStack(spacing: 6) {
                    Text("Connection lost")
                        .font(.headline)
                        .foregroundStyle(FilTheme.terminalForeground)

                    // This used to say "Trying to reconnect..." unconditionally
                    // while nothing was retrying. The overlay is now only shown
                    // for `.unreachable`, i.e. once the retries have stopped.
                    Text("Your session is still running. Tap to reconnect.")
                        .font(.body)
                        .multilineTextAlignment(.center)
                        .foregroundStyle(FilTheme.terminalForeground.opacity(0.62))
                }

                Button {
                    store.send(.reconnectTapped)
                } label: {
                    Text("Reconnect")
                        .font(.body.weight(.semibold))
                        .foregroundStyle(FilTheme.terminalBackground)
                        .padding(.horizontal, 28)
                        .frame(minHeight: 44)
                        .background(FilTheme.filGreen)
                        .clipShape(Capsule())
                }
            }
            .padding(28)
            .background(FilTheme.terminalSurface)
            .clipShape(RoundedRectangle(cornerRadius: 20))
        }
        .transition(.opacity)
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
        withAnimation(.easeOut(duration: duration)) {
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

    func makeUIView(context: Context) -> SwiftTerm.TerminalView {
        let font = UIFont.monospacedSystemFont(ofSize: fontSize, weight: .regular)
        let tv = SwiftTerm.TerminalView(frame: .zero, font: font)
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
            uiView.font = UIFont.monospacedSystemFont(ofSize: fontSize, weight: .regular)
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
        terminalView.feed(byteArray: bytes[...])
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
        let y = max(
            -terminalView.adjustedContentInset.top,
            terminalView.contentSize.height
                - terminalView.bounds.height
                + terminalView.adjustedContentInset.bottom
        )
        terminalView.setContentOffset(CGPoint(x: terminalView.contentOffset.x, y: y), animated: true)
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
