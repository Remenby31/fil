import SwiftUI

public struct FilThreadShape: Shape {
    public init() {}

    public func path(in rect: CGRect) -> Path {
        var path = Path()
        let left = CGPoint(x: rect.minX + rect.width * 0.08, y: rect.midY)
        let right = CGPoint(x: rect.maxX - rect.width * 0.08, y: rect.midY)
        path.move(to: left)
        path.addCurve(
            to: CGPoint(x: rect.midX, y: rect.midY),
            control1: CGPoint(x: rect.minX + rect.width * 0.2, y: rect.maxY * 0.88),
            control2: CGPoint(x: rect.minX + rect.width * 0.34, y: rect.minY + rect.height * 0.12)
        )
        path.addCurve(
            to: right,
            control1: CGPoint(x: rect.minX + rect.width * 0.66, y: rect.maxY * 0.88),
            control2: CGPoint(x: rect.minX + rect.width * 0.8, y: rect.minY + rect.height * 0.12)
        )
        return path
    }
}

public struct FilBrandMark: View {
    private let color: Color

    public init(color: Color = Color(red: 0, green: 0.83, blue: 0.67)) {
        self.color = color
    }

    public var body: some View {
        GeometryReader { proxy in
            let lineWidth = max(6, proxy.size.height * 0.16)
            ZStack {
                FilThreadShape()
                    .stroke(
                        color,
                        style: StrokeStyle(lineWidth: lineWidth, lineCap: .round, lineJoin: .round)
                    )

                HStack {
                    Circle().fill(color).frame(width: lineWidth, height: lineWidth)
                    Spacer()
                    Circle().fill(color).frame(width: lineWidth, height: lineWidth)
                }
                .padding(.horizontal, proxy.size.width * 0.055)
            }
        }
        .aspectRatio(1.8, contentMode: .fit)
        .accessibilityHidden(true)
    }
}
