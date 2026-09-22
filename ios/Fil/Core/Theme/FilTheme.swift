import SwiftUI
import UIKit

enum FilTheme {
    // MARK: - Adaptive App Colors
    static let void_ = Color(uiColor: .systemBackground)
    static let filGreen = Color(hex: 0x00D4AA)
    /// Text/action accent, unlike the decorative brand green, also works on light surfaces.
    static let filGreenText = Color(uiColor: UIColor { traits in
        traits.userInterfaceStyle == .dark
            ? UIColor(red: 0, green: 0.83, blue: 0.67, alpha: 1)
            : UIColor(red: 0, green: 0.40, blue: 0.32, alpha: 1)
    })
    static let surface = Color(uiColor: .secondarySystemGroupedBackground)
    static let cloud = Color(uiColor: .label)
    static let depth = Color(uiColor: .secondarySystemBackground)
    static let elevated = Color(uiColor: .tertiarySystemBackground)
    static let filDark = Color(hex: 0x00B894)
    static let filLight = Color(hex: 0x55EFC4)
    static let error = Color(uiColor: .systemRed)
    static let warning = Color(uiColor: .systemOrange)

    // MARK: - Terminal Colors
    static let terminalBackground = Color(hex: 0x0A0A0F)
    static let terminalForeground = Color(hex: 0xFAFAFA)
    static let terminalSurface = Color(hex: 0x1A1A2E)

    // MARK: - Status Colors
    static let online = filGreen
    static let unreachable = warning
    static let offline = Color(uiColor: .tertiaryLabel)
}

extension Color {
    init(hex: UInt, alpha: Double = 1.0) {
        self.init(
            .sRGB,
            red: Double((hex >> 16) & 0xFF) / 255,
            green: Double((hex >> 8) & 0xFF) / 255,
            blue: Double(hex & 0xFF) / 255,
            opacity: alpha
        )
    }
}
