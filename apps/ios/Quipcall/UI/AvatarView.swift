import UIKit

/// A round face: someone's initials on their colour. Drawn by two layers, so
/// a list of them costs nothing to scroll.
final class AvatarView: UIView {
    private let gradient = CAGradientLayer()
    private let label = UILabel()

    override init(frame: CGRect) {
        super.init(frame: frame)
        isUserInteractionEnabled = false
        gradient.startPoint = CGPoint(x: 0, y: 0)
        gradient.endPoint = CGPoint(x: 1, y: 1)
        gradient.masksToBounds = true
        layer.addSublayer(gradient)
        label.textColor = .white
        label.textAlignment = .center
        addSubview(label)
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError() }

    /// Whose face it is. The colour comes from the name.
    var name: String = "" {
        didSet {
            guard name != oldValue else { return }
            let (from, to) = Theme.avatarColors(name)
            gradient.colors = [from.cgColor, to.cgColor]
            label.text = Theme.initials(name)
        }
    }

    override func layoutSubviews() {
        super.layoutSubviews()
        // No implicit animation: a resized cell shouldn't morph its face.
        CATransaction.begin()
        CATransaction.setDisableActions(true)
        gradient.frame = bounds
        gradient.cornerRadius = bounds.width / 2
        CATransaction.commit()
        label.frame = bounds
        label.font = .systemFont(ofSize: bounds.width * 0.38, weight: .semibold)
    }
}
