import ComposableArchitecture
import SwiftUI
import SwiftTerm

struct TerminalSessionView: View {
    @Bindable var store: StoreOf<TerminalFeature>
    @Environment(\.dismiss) private var dismiss
    @State private var currentFontSize: CGFloat = 14

    var body: some View {
        ZStack {
            FilTheme.void_.ignoresSafeArea()

            VStack(spacing: 0) {
                topBar
                terminalArea
                extraKeysBar
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
        .statusBarHidden(true)
    }

    // MARK: - Top Bar (compact, modern)

    private var topBar: some View {
        HStack(spacing: 12) {
            Button { dismiss() } label: {
                Image(systemName: "chevron.down")
                    .font(.system(size: 13, weight: .semibold))
                    .foregroundStyle(FilTheme.cloud.opacity(0.5))
                    .frame(width: 32, height: 32)
                    .background(.ultraThinMaterial.opacity(0.3))
                    .clipShape(Circle())
            }

            // Session info
            HStack(spacing: 8) {
                RoundedRectangle(cornerRadius: 2)
                    .fill(store.isConnected ? FilTheme.filGreen : FilTheme.error)
                    .frame(width: 3, height: 18)

                VStack(alignment: .leading, spacing: 1) {
                    Text(store.session.shell)
                        .font(.system(size: 13, weight: .semibold, design: .monospaced))
                        .foregroundStyle(FilTheme.cloud)

                    Text(store.session.cwd)
                        .font(.system(size: 10))
                        .foregroundStyle(FilTheme.cloud.opacity(0.35))
                        .lineLimit(1)
                }
            }

            Spacer()

            if let latency = store.latencyMs {
                Text("\(latency)ms")
                    .font(.system(size: 10, weight: .medium, design: .monospaced))
                    .foregroundStyle(latency < 100 ? FilTheme.filGreen.opacity(0.6) : FilTheme.warning.opacity(0.6))
                    .padding(.horizontal, 8)
                    .padding(.vertical, 4)
                    .background(FilTheme.surface)
                    .clipShape(Capsule())
            }
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 8)
        .background(FilTheme.depth.opacity(0.8))
    }

    // MARK: - Terminal Area

    private var terminalArea: some View {
        SwiftTermWrapper(
            fontSize: store.fontSize,
            dataReceived: store.terminalData,
            onInput: { data in
                store.send(.inputSent(data))
            }
        )
        .ignoresSafeArea(.keyboard)
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
                        .font(.system(size: 18, weight: .semibold))
                        .foregroundStyle(FilTheme.cloud)

                    Text("Trying to reconnect...")
                        .font(.system(size: 14))
                        .foregroundStyle(FilTheme.cloud.opacity(0.4))
                }

                Button {
                    store.send(.reconnectTapped)
                } label: {
                    Text("Reconnect")
                        .font(.system(size: 14, weight: .semibold))
                        .foregroundStyle(FilTheme.void_)
                        .padding(.horizontal, 28)
                        .padding(.vertical, 10)
                        .background(FilTheme.filGreen)
                        .clipShape(Capsule())
                }
            }
            .padding(28)
            .background(FilTheme.surface)
            .clipShape(RoundedRectangle(cornerRadius: 20))
        }
        .transition(.opacity)
    }

    // MARK: - Extra Keys Bar

    private var extraKeysBar: some View {
        ExtraKeysBar { data in
            UIImpactFeedbackGenerator(style: .light).impactOccurred()
            store.send(.inputSent(data))
        }
    }
}

// MARK: - SwiftTerm UIKit Wrapper

struct SwiftTermWrapper: UIViewRepresentable {
    var fontSize: CGFloat
    var dataReceived: [UInt8]
    var onInput: ((Data) -> Void)?

    func makeUIView(context: Context) -> SwiftTerm.TerminalView {
        let tv = SwiftTerm.TerminalView(frame: .zero)
        tv.backgroundColor = UIColor(FilTheme.void_)
        tv.nativeForegroundColor = UIColor(FilTheme.cloud)
        tv.nativeBackgroundColor = UIColor(FilTheme.void_)
        tv.font = UIFont.monospacedSystemFont(ofSize: fontSize, weight: .regular)
        context.coordinator.terminalView = tv
        context.coordinator.onInput = onInput
        tv.terminalDelegate = context.coordinator
        return tv
    }

    func updateUIView(_ uiView: SwiftTerm.TerminalView, context: Context) {
        uiView.font = UIFont.monospacedSystemFont(ofSize: fontSize, weight: .regular)
        context.coordinator.onInput = onInput

        // Feed received bytes into SwiftTerm
        if !dataReceived.isEmpty {
            uiView.feed(byteArray: ArraySlice(dataReceived))
        }
    }

    func makeCoordinator() -> Coordinator {
        Coordinator()
    }

    class Coordinator: NSObject, SwiftTerm.TerminalViewDelegate {
        weak var terminalView: SwiftTerm.TerminalView?
        var onInput: ((Data) -> Void)?

        func scrolled(source: SwiftTerm.TerminalView, position: Double) {}
        func setTerminalTitle(source: SwiftTerm.TerminalView, title: String) {}
        func sizeChanged(source: SwiftTerm.TerminalView, newCols: Int, newRows: Int) {}
        func bell(source: SwiftTerm.TerminalView) {}
        func clipboardCopy(source: SwiftTerm.TerminalView, content: Data) {}
        func rangeChanged(source: SwiftTerm.TerminalView, startY: Int, endY: Int) {}
        func iTermContent(source: SwiftTerm.TerminalView, content: Data) {}

        func send(source: SwiftTerm.TerminalView, data: ArraySlice<UInt8>) {
            onInput?(Data(data))
        }

        func hostCurrentDirectoryUpdate(source: SwiftTerm.TerminalView, directory: String?) {}
        func requestOpenLink(source: SwiftTerm.TerminalView, link: String, params: [String : String]) {}
    }
}

// MARK: - Extra Keys Bar

struct ExtraKeysBar: View {
    let onKey: (Data) -> Void
    @State private var ctrlActive = false
    @State private var altActive = false

    private struct KeyDef: Identifiable {
        let id: String
        let label: String
        let data: Data
        let isModifier: Bool

        init(_ label: String, _ bytes: [UInt8], modifier: Bool = false) {
            self.id = label
            self.label = label
            self.data = Data(bytes)
            self.isModifier = modifier
        }
    }

    private let keys: [KeyDef] = [
        KeyDef("esc", [0x1B]),
        KeyDef("tab", [0x09]),
        KeyDef("ctrl", [], modifier: true),
        KeyDef("↑", [0x1B, 0x5B, 0x41]),
        KeyDef("↓", [0x1B, 0x5B, 0x42]),
        KeyDef("←", [0x1B, 0x5B, 0x44]),
        KeyDef("→", [0x1B, 0x5B, 0x43]),
        KeyDef("alt", [], modifier: true),
    ]

    var body: some View {
        HStack(spacing: 4) {
            ForEach(keys) { key in
                Button {
                    if key.label == "ctrl" {
                        ctrlActive.toggle()
                    } else if key.label == "alt" {
                        altActive.toggle()
                    } else {
                        onKey(key.data)
                        ctrlActive = false
                        altActive = false
                    }
                } label: {
                    Text(key.label)
                        .font(.system(size: 12, weight: .medium, design: .monospaced))
                        .foregroundStyle(isActive(key) ? FilTheme.filGreen : FilTheme.cloud.opacity(0.6))
                        .frame(maxWidth: .infinity)
                        .frame(height: 36)
                        .background(isActive(key) ? FilTheme.filGreen.opacity(0.12) : FilTheme.elevated.opacity(0.6))
                        .clipShape(RoundedRectangle(cornerRadius: 8))
                }
            }
        }
        .padding(.horizontal, 6)
        .padding(.vertical, 5)
        .background(FilTheme.depth)
    }

    private func isActive(_ key: KeyDef) -> Bool {
        (key.label == "ctrl" && ctrlActive) || (key.label == "alt" && altActive)
    }
}
