import SwiftUI

enum FilTheme {
    // MARK: - Colors
    static let void_ = Color(hex: 0x0A0A0F)
    static let filGreen = Color(hex: 0x00D4AA)
    static let surface = Color(hex: 0x1A1A2E)
    static let cloud = Color(hex: 0xFAFAFA)
    static let depth = Color(hex: 0x0D1117)
    static let elevated = Color(hex: 0x161B22)
    static let filDark = Color(hex: 0x00B894)
    static let filLight = Color(hex: 0x55EFC4)
    static let error = Color(hex: 0xFF6B6B)
    static let warning = Color(hex: 0xFECA57)

    // MARK: - Status Colors
    static let online = filGreen
    static let unreachable = warning
    static let offline = Color(white: 0.3)
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
