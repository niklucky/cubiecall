import UIKit

/// Colours named by what they're for: the desktop's (`apps/desktop/src/theme.rs`).
/// Dark only, as there.
enum Theme {
    /// Screen background.
    static let bg = UIColor(rgb: 0x141519)
    /// Buttons, cards and fields on `bg`.
    static let raised = UIColor(rgb: 0x22242b)
    /// Behind video in a call.
    static let stage = UIColor(rgb: 0x0e0f12)
    static let text = UIColor(rgb: 0xf2f3f5)
    static let textDim = UIColor(rgb: 0x9ba1ac)
    static let textFaint = UIColor(rgb: 0x676d78)
    static let accent = UIColor(rgb: 0x4f8cff)
    static let danger = UIColor(rgb: 0xe5484d)
    /// The speaking ring, live calls.
    static let speaking = UIColor(rgb: 0x3fd07a)
    /// Hairlines between rows.
    static let line = UIColor(rgb: 0x24262d)

    /// The avatar hue for `seed`, 0..<1: FNV-1a over its lowercased bytes, as
    /// on the desktop and the web, so people keep their colour everywhere.
    static func hue(_ seed: String) -> CGFloat {
        var hash: UInt32 = 2_166_136_261
        for byte in seed.lowercased().utf8 {
            hash = (hash ^ UInt32(byte)) &* 16_777_619
        }
        return CGFloat(hash % 360) / 360
    }

    /// The two ends of someone's avatar gradient.
    static func avatarColors(_ seed: String) -> (UIColor, UIColor) {
        let hue = hue(seed)
        return (
            UIColor(hue: hue, saturation: 0.55, lightness: 0.52),
            UIColor(hue: (hue + 0.08).truncatingRemainder(dividingBy: 1), saturation: 0.6, lightness: 0.38)
        )
    }

    /// Up to two initials: "Anna Petrova" → "AP", "max" → "M".
    static func initials(_ name: String) -> String {
        let letters = name.split(whereSeparator: \.isWhitespace).compactMap(\.first).prefix(2)
        return String(letters).uppercased()
    }
}

extension UIColor {
    convenience init(rgb: UInt32) {
        self.init(
            red: CGFloat((rgb >> 16) & 0xff) / 255,
            green: CGFloat((rgb >> 8) & 0xff) / 255,
            blue: CGFloat(rgb & 0xff) / 255,
            alpha: 1
        )
    }

    /// HSL, as CSS and the desktop's theme have it (UIKit's own is HSB).
    convenience init(hue: CGFloat, saturation: CGFloat, lightness: CGFloat) {
        let brightness = lightness + saturation * min(lightness, 1 - lightness)
        let s = brightness == 0 ? 0 : 2 * (1 - lightness / brightness)
        self.init(hue: hue, saturation: s, brightness: brightness, alpha: 1)
    }
}
