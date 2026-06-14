import ComposableArchitecture
import SwiftUI
import SwiftTerm

struct TerminalSessionView: View {
    @Bindable var store: StoreOf<TerminalFeature>
    @Environment(\.dismiss) private var dismiss

    var body: some View {
        ZStack {
            FilTheme.void_.ignoresSafeArea()

            VStack(spacing: 0) {
                HStack {
                    Button {
                        dismiss()
                    } label: {
                        Image(systemName: "chevron.down")
                            .font(.system(size: 16, weight: .medium))
                            .foregroundStyle(FilTheme.cloud.opacity(0.6))
                    }

                    Spacer()

                    VStack(spacing: 2) {
                        Text(store.session.shell)
                            .font(.system(size: 14, weight: .semibold, design: .monospaced))
                            .foregroundStyle(FilTheme.cloud)

                        Text(store.session.cwd)
                            .font(.system(size: 11))
                            .foregroundStyle(FilTheme.cloud.opacity(0.4))
                    }

                    Spacer()

                    Circle()
                        .fill(store.isConnected ? FilTheme.online : FilTheme.offline)
                        .frame(width: 8, height: 8)
                }
                .padding(.horizontal, 16)
                .padding(.vertical, 10)
                .background(FilTheme.surface)

                SwiftTermWrapper()
                    .ignoresSafeArea(.keyboard)

                ExtraKeysBar { key in
                    store.send(.inputSent(key))
                }
            }
        }
        .onAppear { store.send(.onAppear) }
        .onDisappear { store.send(.onDisappear) }
    }
}

// MARK: - SwiftTerm UIKit Wrapper

struct SwiftTermWrapper: UIViewRepresentable {
    func makeUIView(context: Context) -> SwiftTerm.TerminalView {
        let tv = SwiftTerm.TerminalView(frame: .zero)
        tv.backgroundColor = UIColor(FilTheme.void_)
        tv.nativeForegroundColor = UIColor(FilTheme.cloud)
        tv.nativeBackgroundColor = UIColor(FilTheme.void_)
        tv.font = UIFont.monospacedSystemFont(ofSize: 14, weight: .regular)
        return tv
    }

    func updateUIView(_ uiView: SwiftTerm.TerminalView, context: Context) {}
}

// MARK: - Extra Keys Bar

struct ExtraKeysBar: View {
    let onKey: (Data) -> Void

    private let keys: [(label: String, data: Data)] = [
        ("esc", Data([0x1B])),
        ("tab", Data([0x09])),
        ("ctrl", Data()),
        ("↑", Data([0x1B, 0x5B, 0x41])),
        ("↓", Data([0x1B, 0x5B, 0x42])),
        ("←", Data([0x1B, 0x5B, 0x44])),
        ("→", Data([0x1B, 0x5B, 0x43])),
        ("⌥", Data()),
    ]

    var body: some View {
        HStack(spacing: 6) {
            ForEach(keys, id: \.label) { key in
                Button {
                    onKey(key.data)
                } label: {
                    Text(key.label)
                        .font(.system(size: 13, weight: .medium, design: .monospaced))
                        .foregroundStyle(FilTheme.cloud.opacity(0.7))
                        .frame(maxWidth: .infinity)
                        .frame(height: 36)
                        .background(FilTheme.elevated)
                        .clipShape(RoundedRectangle(cornerRadius: 6))
                }
            }
        }
        .padding(.horizontal, 8)
        .padding(.vertical, 6)
        .background(FilTheme.surface)
    }
}
