import UIKit

/// The call, full screen: everyone as a tile, a shared screen on top, the
/// controls underneath. It only redraws when something changes; video is the
/// system's (see `VideoView`).
final class CallViewController: UIViewController {
    private let model: CallModel
    private let stage = UIView()
    private let nameLabel = UILabel()
    private let statusLabel = UILabel()
    private let controls = UIStackView()
    private let speaker = RoundButton(symbol: "speaker.wave.2.fill")
    private let camera = RoundButton(symbol: "video.fill")
    private let mic = RoundButton(symbol: "mic.fill")
    private let flip = RoundButton(symbol: "arrow.triangle.2.circlepath")
    private let leave = RoundButton(symbol: "phone.down.fill")

    /// Tiles by who they show: "you", someone's identity, or "screen:<track>".
    private var tiles: [String: Tile] = [:]
    private var order: [String] = []
    private var clock: Timer?

    static func present(room: String, name: String?, from presenter: UIViewController) {
        let call = CallViewController(model: CallModel(room: room, name: name))
        call.modalPresentationStyle = .fullScreen
        presenter.present(call, animated: true)
    }

    init(model: CallModel) {
        self.model = model
        super.init(nibName: nil, bundle: nil)
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError() }

    override var preferredStatusBarStyle: UIStatusBarStyle { .lightContent }

    override func viewDidLoad() {
        super.viewDidLoad()
        view.backgroundColor = Theme.stage
        view.addSubview(stage)

        nameLabel.font = .systemFont(ofSize: 17, weight: .semibold)
        nameLabel.textColor = Theme.text
        nameLabel.textAlignment = .center
        statusLabel.font = .monospacedDigitSystemFont(ofSize: 13, weight: .regular)
        statusLabel.textColor = Theme.textDim
        statusLabel.textAlignment = .center
        view.addSubview(nameLabel)
        view.addSubview(statusLabel)

        leave.tint(background: Theme.danger, symbol: .white)
        speaker.addAction(UIAction { [weak self] _ in self?.model.toggleSpeaker() }, for: .touchUpInside)
        camera.addAction(UIAction { [weak self] _ in self?.model.toggleCamera() }, for: .touchUpInside)
        mic.addAction(UIAction { [weak self] _ in self?.model.toggleMic() }, for: .touchUpInside)
        flip.addAction(UIAction { [weak self] _ in self?.model.flipCamera() }, for: .touchUpInside)
        leave.addAction(UIAction { [weak self] _ in self?.hangUp() }, for: .touchUpInside)
        speaker.accessibilityLabel = "Speaker"
        camera.accessibilityLabel = "Camera"
        mic.accessibilityLabel = "Microphone"
        flip.accessibilityLabel = "Flip camera"
        leave.accessibilityLabel = "Leave"
        controls.axis = .horizontal
        controls.distribution = .equalSpacing
        for button in [speaker, camera, mic, flip, leave] {
            controls.addArrangedSubview(button)
        }
        view.addSubview(controls)

        model.onChange = { [weak self] change in self?.apply(change) }
        model.start()
        syncPeople(animated: false)
        syncControls()
        syncStatus()
        // The time in the call: once a second, only while the screen is up.
        clock = Timer.scheduledTimer(withTimeInterval: 1, repeats: true) { [weak self] _ in
            self?.syncStatus()
        }
    }

    deinit {
        clock?.invalidate()
    }

    // MARK: Changes

    private func apply(_ change: CallModel.Change) {
        switch change {
        case .people: syncPeople(animated: true)
        case .speaking: syncRings()
        case .controls: syncControls()
        case .status: syncStatus()
        case let .ended(message, signedOut): finish(message, signedOut: signedOut)
        }
    }

    private func hangUp() {
        model.leave()
        close()
    }

    private func finish(_ message: String?, signedOut: Bool) {
        guard let message, !signedOut else {
            close(then: signedOut ? { SceneDelegate.signedOut() } : nil)
            return
        }
        let alert = UIAlertController(title: nil, message: message, preferredStyle: .alert)
        alert.addAction(UIAlertAction(title: "OK", style: .default) { [weak self] _ in self?.close() })
        present(alert, animated: true)
    }

    private func close(then: (() -> Void)? = nil) {
        clock?.invalidate()
        for tile in tiles.values {
            tile.show(track: nil, in: model.call)
        }
        presentingViewController?.dismiss(animated: true, completion: then)
    }

    /// Who has a tile, and what's in it.
    private func syncPeople(animated: Bool) {
        var wanted: [String] = []
        // Shared screens first: they get the big place.
        for person in model.people where person.screen != nil {
            wanted.append("screen:" + person.id)
        }
        wanted.append(contentsOf: model.people.map(\.id))
        wanted.append("you")

        for key in tiles.keys where !wanted.contains(key) {
            let tile = tiles.removeValue(forKey: key)
            tile?.show(track: nil, in: model.call)
            tile?.removeFromSuperview()
        }
        for key in wanted where tiles[key] == nil {
            let tile = Tile()
            tile.alpha = 0
            tiles[key] = tile
            stage.addSubview(tile)
        }
        order = wanted

        let you = AppEnv.shared.client.account()?.name ?? "You"
        for person in model.people {
            tiles[person.id]?.show(name: person.name, muted: person.muted)
            tiles[person.id]?.show(track: person.camera, in: model.call)
            if let screen = tiles["screen:" + person.id] {
                screen.isScreen = true
                screen.show(name: person.name + "'s screen", muted: false)
                screen.show(track: person.screen, in: model.call)
            }
        }
        if let tile = tiles["you"] {
            tile.show(name: you, muted: !model.micOn)
            tile.showPreview(model.cameraOn, mirrored: model.frontCamera, in: model.call)
        }
        syncRings()

        view.setNeedsLayout()
        let layout = {
            self.view.layoutIfNeeded()
            for tile in self.tiles.values {
                tile.alpha = 1
            }
        }
        if animated {
            UIView.animate(withDuration: 0.25, delay: 0, options: [.beginFromCurrentState, .curveEaseOut], animations: layout)
        } else {
            layout()
        }
    }

    private func syncRings() {
        for person in model.people {
            tiles[person.id]?.show(name: person.name, muted: person.muted)
            tiles[person.id]?.speaking = person.speaking && !person.muted
        }
        tiles["you"]?.speaking = model.youSpeak && model.micOn
    }

    private func syncControls() {
        mic.set(symbol: model.micOn ? "mic.fill" : "mic.slash.fill", lit: !model.micOn)
        camera.set(symbol: model.cameraOn ? "video.fill" : "video.slash.fill", lit: model.cameraOn)
        speaker.set(symbol: model.speakerOn ? "speaker.wave.2.fill" : "speaker.fill", lit: model.speakerOn)
        flip.isHidden = !model.cameraOn
        if let tile = tiles["you"] {
            let you = AppEnv.shared.client.account()?.name ?? "You"
            tile.show(name: you, muted: !model.micOn)
            tile.showPreview(model.cameraOn, mirrored: model.frontCamera, in: model.call)
        }
    }

    private func syncStatus() {
        nameLabel.text = model.roomName
        if let status = model.status {
            statusLabel.text = status
        } else if let recorder = model.recording.first {
            statusLabel.text = "● " + model.name(of: recorder) + " is recording"
        } else if let since = model.connectedAt {
            let seconds = Int(Date().timeIntervalSince(since))
            statusLabel.text = String(format: "%d:%02d", seconds / 60, seconds % 60)
        } else {
            statusLabel.text = nil
        }
    }

    // MARK: Layout

    override func viewDidLayoutSubviews() {
        super.viewDidLayoutSubviews()
        let safe = view.bounds.inset(by: view.safeAreaInsets)
        nameLabel.frame = CGRect(x: safe.minX + 16, y: safe.minY + 4, width: safe.width - 32, height: 22)
        statusLabel.frame = CGRect(x: safe.minX + 16, y: nameLabel.frame.maxY, width: safe.width - 32, height: 18)
        let controlsHeight: CGFloat = 60
        controls.frame = CGRect(
            x: safe.minX + 24, y: safe.maxY - controlsHeight - 8,
            width: safe.width - 48, height: controlsHeight
        )
        stage.frame = CGRect(
            x: safe.minX + 8, y: statusLabel.frame.maxY + 10,
            width: safe.width - 16, height: controls.frame.minY - statusLabel.frame.maxY - 26
        )
        let frames = Self.frames(for: order, in: stage.bounds)
        for (key, frame) in zip(order, frames) {
            guard let tile = tiles[key] else { continue }
            tile.frame = frame
            tile.isHidden = frame.isEmpty
            // Ask for no more video than the tile shows.
            tile.pickQuality(in: model.call, scale: view.window?.screen.scale ?? 3)
        }
    }

    /// Where each tile goes: a shared screen across the top with the faces in
    /// a row under it, or the faces alone (rows, then a grid two across).
    private static func frames(for order: [String], in bounds: CGRect) -> [CGRect] {
        let gap: CGFloat = 6
        let screens = order.filter { $0.hasPrefix("screen:") }.count
        let faces = order.count - screens
        var frames: [CGRect] = []
        var area = bounds
        if screens > 0 {
            // Screens share the top two thirds; the faces line up below.
            let top = bounds.height * 0.68
            let each = (top - gap * CGFloat(screens - 1)) / CGFloat(screens)
            for index in 0..<screens {
                frames.append(CGRect(x: 0, y: CGFloat(index) * (each + gap), width: bounds.width, height: each))
            }
            area = CGRect(x: 0, y: top + gap, width: bounds.width, height: bounds.height - top - gap)
            let shown = min(faces, 4)
            let width = (area.width - gap * CGFloat(shown - 1)) / CGFloat(max(shown, 1))
            for index in 0..<faces {
                frames.append(index < shown
                    ? CGRect(x: CGFloat(index) * (width + gap), y: area.minY, width: width, height: area.height)
                    : .zero)
            }
            return frames
        }
        // Up to three people get the full width each: most cameras are wide,
        // and a wide tile crops them least. From four, two across.
        let shown = min(faces, 8)
        let columns = shown <= 3 ? 1 : 2
        let rows = Int((Double(shown) / Double(columns)).rounded(.up))
        let width = (area.width - gap * CGFloat(columns - 1)) / CGFloat(columns)
        let height = (area.height - gap * CGFloat(rows - 1)) / CGFloat(max(rows, 1))
        for index in 0..<faces {
            guard index < shown else {
                frames.append(.zero)
                continue
            }
            let (row, column) = (index / columns, index % columns)
            frames.append(CGRect(
                x: CGFloat(column) * (width + gap), y: CGFloat(row) * (height + gap),
                width: width, height: height
            ))
        }
        return frames
    }
}

/// One person, or one shared screen: their video, or their face when the
/// camera is off, with a name and a ring while they speak.
private final class Tile: UIView {
    private let video = VideoView()
    private let avatar = AvatarView()
    private let label = UILabel()
    private let labelBack = UIView()
    private let mutedIcon = UIImageView(image: UIImage(systemName: "mic.slash.fill"))
    /// The track showing, if any.
    private var track: String?
    private var previewing = false
    private var quality: Quality?
    var isScreen = false {
        didSet { video.fill = !isScreen }
    }

    override init(frame: CGRect) {
        super.init(frame: frame)
        backgroundColor = Theme.raised
        layer.cornerRadius = 18
        layer.cornerCurve = .continuous
        layer.masksToBounds = true
        layer.borderColor = Theme.speaking.cgColor
        video.isHidden = true
        labelBack.backgroundColor = UIColor.black.withAlphaComponent(0.45)
        labelBack.layer.cornerRadius = 9
        label.font = .systemFont(ofSize: 13, weight: .medium)
        label.textColor = .white
        mutedIcon.tintColor = Theme.danger
        mutedIcon.contentMode = .scaleAspectFit
        for view in [video, avatar, labelBack, label, mutedIcon] {
            addSubview(view)
        }
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError() }

    var speaking = false {
        didSet {
            guard speaking != oldValue else { return }
            layer.borderWidth = speaking ? 3 : 0
        }
    }

    func show(name: String, muted: Bool) {
        if label.text != name {
            label.text = name
            avatar.name = name
            setNeedsLayout()
        }
        if mutedIcon.isHidden == muted {
            mutedIcon.isHidden = !muted
            setNeedsLayout()
        }
    }

    /// Show `track` here, or stop (nil): a tile without video asks for none.
    func show(track: String?, in call: Call?) {
        guard track != self.track else { return }
        if let old = self.track {
            call?.detachVideo(track: old)
        }
        self.track = track
        quality = nil
        if let track {
            call?.attachVideo(track: track, renderer: video.renderer)
        } else {
            video.clear()
        }
        video.isHidden = track == nil
        avatar.isHidden = track != nil
    }

    /// Your own camera, as others see it; mirrored for the front one, the
    /// way a mirror shows you.
    func showPreview(_ on: Bool, mirrored: Bool, in call: Call?) {
        video.transform = on && mirrored ? CGAffineTransform(scaleX: -1, y: 1) : .identity
        guard on != previewing else { return }
        previewing = on
        call?.showPreview(renderer: on ? video.renderer : 0)
        if !on { video.clear() }
        video.isHidden = !on
        avatar.isHidden = on
    }

    /// The smallest layer of a camera that still fills this tile.
    func pickQuality(in call: Call?, scale: CGFloat) {
        guard let track, !isScreen else { return }
        let pixels = bounds.width * scale
        let wanted: Quality = pixels >= 700 ? .high : pixels >= 300 ? .medium : .low
        guard wanted != quality else { return }
        quality = wanted
        call?.setQuality(track: track, quality: wanted)
    }

    override func layoutSubviews() {
        super.layoutSubviews()
        video.bounds = CGRect(origin: .zero, size: bounds.size)
        video.center = CGPoint(x: bounds.midX, y: bounds.midY)
        let side = min(96, min(bounds.width, bounds.height) * 0.5)
        avatar.frame = CGRect(x: bounds.midX - side / 2, y: bounds.midY - side / 2, width: side, height: side)
        let icon: CGFloat = mutedIcon.isHidden ? 0 : 18
        let text = min(label.intrinsicContentSize.width, bounds.width - 36 - icon)
        let width = text + icon + 14
        labelBack.frame = CGRect(x: 8, y: bounds.height - 30, width: max(width, 0), height: 22)
        mutedIcon.frame = CGRect(x: 14, y: bounds.height - 27, width: 14, height: 16)
        label.frame = CGRect(x: 15 + icon, y: bounds.height - 30, width: max(text, 0), height: 22)
        labelBack.isHidden = bounds.height < 60
        label.isHidden = labelBack.isHidden
    }
}

/// A round call control. Lit (white) says "this is on, and worth noticing".
private final class RoundButton: UIControl {
    private let icon = UIImageView()
    private var symbol: String

    init(symbol: String) {
        self.symbol = symbol
        super.init(frame: .zero)
        isAccessibilityElement = true
        accessibilityTraits = .button
        backgroundColor = UIColor.white.withAlphaComponent(0.14)
        icon.tintColor = .white
        icon.contentMode = .center
        icon.image = UIImage(systemName: symbol, withConfiguration: UIImage.SymbolConfiguration(pointSize: 22, weight: .medium))
        addSubview(icon)
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError() }

    override var intrinsicContentSize: CGSize { CGSize(width: 60, height: 60) }

    override func layoutSubviews() {
        super.layoutSubviews()
        layer.cornerRadius = bounds.height / 2
        icon.frame = bounds
    }

    override var isHighlighted: Bool {
        didSet { alpha = isHighlighted ? 0.6 : 1 }
    }

    func tint(background: UIColor, symbol: UIColor) {
        backgroundColor = background
        icon.tintColor = symbol
    }

    func set(symbol: String, lit: Bool) {
        if symbol != self.symbol {
            self.symbol = symbol
            icon.image = UIImage(systemName: symbol, withConfiguration: UIImage.SymbolConfiguration(pointSize: 22, weight: .medium))
        }
        tint(background: lit ? .white : UIColor.white.withAlphaComponent(0.14), symbol: lit ? Theme.stage : .white)
    }
}
