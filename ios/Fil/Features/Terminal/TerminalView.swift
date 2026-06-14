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
                .onEnded { value in
                    currentFontSize = store.fontSize
                }
        )
        .gesture(
            DragGesture(minimumDistance: 50)
                .onEnded { value in
                    let horizontal = value.translation.width
                    let vertical = value.translation.height

                    if abs(horizontal) > abs(vertical) {
                        if horizontal > 0 {
                            store.send(.previousSession)
                        } else {
                            store.send(.nextSession)
                        }
                    } else if vertical > 50 {
                        dismiss()
                    }
                }
        )
        .onAppear {
            currentFontSize = store.fontSize
            store.send(.onAppear)
        }
        .onDisappear { store.send(.onDisappear) }
        .statusBarHidden(true)
    }

    // MARK: - Top Bar

    private var topBar: some View {
        HStack {
            Button {
                dismiss()
            } label: {
                Image(systemName: "chevron.down")
                    .font(.system(size: 15, weight: .semibold))
                    .foregroundStyle(FilTheme.cloud.opacity(0.6))
                    .frame(width: 36, height: 36)
                    .background(FilTheme.elevated)
                    .clipShape(Circle())
            }

            Spacer()

            VStack(spacing: 2) {
                Text(store.session.shell)
                    .font(.system(size: 14, weight: .semibold, design: .monospaced))
                    .foregroundStyle(FilTheme.cloud)

                HStack(spacing: 6) {
                    Text(store.session.cwd)
                        .font(.system(size: 11))
                        .foregroundStyle(FilTheme.cloud.opacity(0.4))

                    if let latency = store.latencyMs {
                        Text("\(latency)ms")
                            .font(.system(size: 10, design: .monospaced))
                            .foregroundStyle(latency < 100 ? FilTheme.filGreen : FilTheme.warning)
                    }
                }
            }

            Spacer()

            Circle()
                .fill(store.isConnected ? FilTheme.online : FilTheme.error)
                .frame(width: 8, height: 8)
                .frame(width: 36, height: 36)
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 6)
        .background(FilTheme.surface)
    }

    // MARK: - Terminal Area

    private var terminalArea: some View {
        SwiftTermWrapper(fontSize: store.fontSize)
            .ignoresSafeArea(.keyboard)
    }

    // MARK: - Disconnected Overlay

    private var disconnectedOverlay: some View {
        VStack(spacing: 16) {
            Image(systemName: "wifi.exclamationmark")
                .font(.system(size: 36))
                .foregroundStyle(FilTheme.error)

            Text("Connection lost")
                .font(.system(size: 18, weight: .semibold))
                .foregroundStyle(FilTheme.cloud)

            Text("The connection to this session was interrupted.")
                .font(.system(size: 14))
                .foregroundStyle(FilTheme.cloud.opacity(0.5))
                .multilineTextAlignment(.center)

            Button {
                store.send(.reconnectTapped)
            } label: {
                Text("Reconnect")
                    .font(.system(size: 15, weight: .medium))
                    .foregroundStyle(FilTheme.void_)
                    .padding(.horizontal, 24)
                    .padding(.vertical, 12)
                    .background(FilTheme.filGreen)
                    .clipShape(RoundedRectangle(cornerRadius: 10))
            }
        }
        .padding(32)
        .background(.ultraThinMaterial.opacity(0.95))
        .clipShape(RoundedRectangle(cornerRadius: 20))
        .padding(40)
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

    func makeUIView(context: Context) -> SwiftTerm.TerminalView {
        let tv = SwiftTerm.TerminalView(frame: .zero)
        tv.backgroundColor = UIColor(FilTheme.void_)
        tv.nativeForegroundColor = UIColor(FilTheme.cloud)
        tv.nativeBackgroundColor = UIColor(FilTheme.void_)
        tv.font = UIFont.monospacedSystemFont(ofSize: fontSize, weight: .regular)
        return tv
    }

    func updateUIView(_ uiView: SwiftTerm.TerminalView, context: Context) {
        uiView.font = UIFont.monospacedSystemFont(ofSize: fontSize, weight: .regular)
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
        HStack(spacing: 5) {
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
                        .font(.system(size: 13, weight: .medium, design: .monospaced))
                        .foregroundStyle(
                            (key.label == "ctrl" && ctrlActive) || (key.label == "alt" && altActive)
                                ? FilTheme.filGreen
                                : FilTheme.cloud.opacity(0.7)
                        )
                        .frame(maxWidth: .infinity)
                        .frame(height: 38)
                        .background(
                            (key.label == "ctrl" && ctrlActive) || (key.label == "alt" && altActive)
                                ? FilTheme.filGreen.opacity(0.15)
                                : FilTheme.elevated
                        )
                        .clipShape(RoundedRectangle(cornerRadius: 7))
                }
            }
        }
        .padding(.horizontal, 6)
        .padding(.vertical, 6)
        .background(FilTheme.surface)
    }
}
