import UIKit

/// Faces. Everything else on screen is the system's: its colours, its lists,
/// its bars, light or dark as the phone is set.
enum Avatar {
    /// The avatar hue for `seed`, 0..<1: FNV-1a over its lowercased bytes, as
    /// on the desktop and the web, so people keep their colour everywhere.
    static func hue(_ seed: String) -> CGFloat {
        var hash: UInt32 = 2_166_136_261
        for byte in seed.lowercased().utf8 {
            hash = (hash ^ UInt32(byte)) &* 16_777_619
        }
        return CGFloat(hash % 360) / 360
    }

    /// Up to two initials: "Anna Petrova" → "AP", "max" → "M".
    static func initials(_ name: String) -> String {
        let letters = name.split(whereSeparator: \.isWhitespace).compactMap(\.first).prefix(2)
        return String(letters).uppercased()
    }

    private static let cache = NSCache<NSString, UIImage>()

    /// A round face for a list row or a tile: the person's photo if the
    /// address book has one, or their initials on their colour. Drawn once
    /// per name and size, then kept.
    static func image(_ name: String, side: CGFloat, photo: Data? = nil) -> UIImage {
        let key = "\(name)|\(side)|\(photo?.count ?? 0)" as NSString
        if let cached = cache.object(forKey: key) {
            return cached
        }
        let size = CGSize(width: side, height: side)
        let image = UIGraphicsImageRenderer(size: size).image { context in
            let rect = CGRect(origin: .zero, size: size)
            UIBezierPath(ovalIn: rect).addClip()
            if let photo, let picture = UIImage(data: photo) {
                picture.draw(in: rect)
                return
            }
            let hue = hue(name)
            let colors = [
                UIColor(hue: hue, saturation: 0.55, lightness: 0.52).cgColor,
                UIColor(hue: (hue + 0.08).truncatingRemainder(dividingBy: 1), saturation: 0.6, lightness: 0.38).cgColor,
            ] as CFArray
            if let gradient = CGGradient(colorsSpace: CGColorSpaceCreateDeviceRGB(), colors: colors, locations: nil) {
                context.cgContext.drawLinearGradient(gradient, start: .zero, end: CGPoint(x: side, y: side), options: [])
            }
            let text = initials(name) as NSString
            let attributes: [NSAttributedString.Key: Any] = [
                .font: UIFont.systemFont(ofSize: side * 0.4, weight: .semibold),
                .foregroundColor: UIColor.white,
            ]
            let bounds = text.size(withAttributes: attributes)
            text.draw(
                at: CGPoint(x: (side - bounds.width) / 2, y: (side - bounds.height) / 2),
                withAttributes: attributes
            )
        }
        cache.setObject(image, forKey: key)
        return image
    }
}

extension UIColor {
    /// HSL, as CSS and the desktop's theme have it (UIKit's own is HSB).
    convenience init(hue: CGFloat, saturation: CGFloat, lightness: CGFloat) {
        let brightness = lightness + saturation * min(lightness, 1 - lightness)
        let s = brightness == 0 ? 0 : 2 * (1 - lightness / brightness)
        self.init(hue: hue, saturation: s, brightness: brightness, alpha: 1)
    }
}

extension UIListContentConfiguration {
    /// A person or a room as the system draws a list row: a round face, a
    /// name, a line under it.
    static func face(_ name: String, detail: String?, photo: Data? = nil) -> UIListContentConfiguration {
        var content = UIListContentConfiguration.subtitleCell()
        content.text = name
        content.secondaryText = detail
        content.textProperties.font = .preferredFont(forTextStyle: .headline)
        content.secondaryTextProperties.font = .preferredFont(forTextStyle: .subheadline)
        content.secondaryTextProperties.color = .secondaryLabel
        content.image = Avatar.image(name, side: 44, photo: photo)
        content.imageToTextPadding = 12
        content.textToSecondaryTextVerticalPadding = 2
        content.directionalLayoutMargins.top = 8
        content.directionalLayoutMargins.bottom = 8
        return content
    }
}

extension UIViewController {
    /// Something to say, the system's way.
    func say(_ message: String) {
        let alert = UIAlertController(title: nil, message: message, preferredStyle: .alert)
        alert.addAction(UIAlertAction(title: "OK", style: .default))
        present(alert, animated: true)
    }
}

extension Notification.Name {
    /// A call ended: the list of calls has a new entry.
    static let callsChanged = Notification.Name("quipcall.calls.changed")
}
