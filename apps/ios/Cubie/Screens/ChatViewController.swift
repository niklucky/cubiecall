import UIKit

/// A room, as a group chat: oldest on top, newest at the bottom.
///
/// - A call is bracketed by two centred lines: "Call started" and "Call
///   finished", as a messenger marks its service messages. The closing one
///   ends with small symbols for what the call had: a shared screen, a
///   recording, a transcript.
/// - What people said (the transcript, when someone's Mac made one) reads as
///   chat bubbles: yours on the right, everyone else's on the left under
///   their name, a face beside the last of a run.
/// - Who joined or left, screen shares, recordings and the transcript
///   starting are small centred lines between the bubbles, the last three
///   with their symbol.
///
/// Messages and files will be bubbles here too once Cubie has them; there
/// is nothing to type into yet. iOS has no chat-bubble component, so the two
/// cells below are ours, drawn in the system's colours and type.
final class ChatViewController: UIViewController, UITableViewDataSource, UITableViewDelegate {
    private let room: String
    private let name: String
    private let tableView = UITableView(frame: .zero, style: .plain)
    private var rows: [Row] = []
    /// Shown at its newest end once, when the list first has its size.
    private var openedAtEnd = false

    private enum Row {
        /// "Today", "Yesterday", "16 Nov": where a new day begins.
        case day(String)
        /// A centred line: a call starting or finishing (`strong`), someone
        /// joining. A symbol may lead it, and others close it.
        case service(String, lead: String?, trail: [String], strong: Bool)
        /// Something said. `first` and `last`: of a run by the same person.
        case said(Moment, first: Bool, last: Bool)
    }

    init(room: String, name: String) {
        self.room = room
        self.name = name
        super.init(nibName: nil, bundle: nil)
        hidesBottomBarWhenPushed = true
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError() }

    override func viewDidLoad() {
        super.viewDidLoad()
        view.backgroundColor = .systemBackground
        navigationItem.largeTitleDisplayMode = .never
        navigationItem.titleView = titleView()
        navigationItem.rightBarButtonItems = [
            UIBarButtonItem(
                image: UIImage(systemName: "video"),
                primaryAction: UIAction { [weak self] _ in self?.call() }
            ),
            UIBarButtonItem(
                image: UIImage(systemName: "square.and.arrow.up"),
                primaryAction: UIAction { [weak self] _ in self?.share() }
            ),
        ]
        navigationItem.rightBarButtonItems?[0].accessibilityLabel = "Call"
        navigationItem.rightBarButtonItems?[1].accessibilityLabel = "Share"

        tableView.frame = view.bounds
        tableView.autoresizingMask = [.flexibleWidth, .flexibleHeight]
        tableView.separatorStyle = .none
        tableView.allowsSelection = false
        tableView.dataSource = self
        tableView.delegate = self
        tableView.register(BubbleCell.self, forCellReuseIdentifier: "bubble")
        tableView.register(ServiceCell.self, forCellReuseIdentifier: "service")
        tableView.contentInset.top = 8
        tableView.contentInset.bottom = 8
        view.addSubview(tableView)

        NotificationCenter.default.addObserver(self, selector: #selector(reload), name: .callsChanged, object: nil)
        reload()
    }

    /// The room's face and name in the bar, as a chat's header.
    private func titleView() -> UIView {
        let face = UIImageView(image: Avatar.image(name, side: 30))
        let label = UILabel()
        label.text = name
        label.font = .preferredFont(forTextStyle: .headline)
        let stack = UIStackView(arrangedSubviews: [face, label])
        stack.spacing = 8
        stack.alignment = .center
        return stack
    }

    private func call() {
        CallViewController.present(room: room, name: name, from: self)
    }

    private func share() {
        guard let url = URL(string: AppEnv.shared.client.roomLink(room: room)) else { return }
        let sheet = UIActivityViewController(activityItems: [url], applicationActivities: nil)
        sheet.popoverPresentationController?.barButtonItem = navigationItem.rightBarButtonItems?.last
        present(sheet, animated: true)
    }

    // MARK: The history, as a chat

    @objc private func reload() {
        let client = AppEnv.shared.client
        var rows: [Row] = []
        var day: String?
        // Stored newest first; a chat reads the other way.
        for call in client.roomCalls(room: room).reversed() {
            let started = Date(timeIntervalSince1970: TimeInterval(call.started))
            let finished = started.addingTimeInterval(TimeInterval(call.seconds))
            let today = Self.day(started)
            if today != day {
                day = today
                rows.append(.day(today))
            }
            rows.append(.service(
                "Call started · \(Self.time(started))", lead: "phone.fill", trail: [], strong: true
            ))
            let moments = client.callMoments(id: call.id)
            for (index, moment) in moments.enumerated() {
                guard moment.kind == .said else {
                    rows.append(.service(moment.event.text, lead: Self.symbol(for: moment), trail: [], strong: false))
                    continue
                }
                // A run of lines by one person shares a name and a face.
                let same = { (other: Moment?) in other?.kind == .said && other?.who == moment.who && other?.mine == moment.mine }
                rows.append(.said(
                    moment,
                    first: !same(index > 0 ? moments[index - 1] : nil),
                    last: !same(index + 1 < moments.count ? moments[index + 1] : nil)
                ))
            }
            let ending = call.people.isEmpty
                ? "Nobody joined · finished \(Self.time(finished))"
                : "Call finished · \(Self.time(finished)) · \(call.lengthText)"
            rows.append(.service(ending, lead: "phone.down.fill", trail: Self.had(moments), strong: true))
        }
        self.rows = rows
        tableView.reloadData()
        showEmptyIfNeeded()
        // A call just ended while the chat was open: show its end.
        if openedAtEnd {
            scrollToEnd()
        }
    }

    override func viewDidLayoutSubviews() {
        super.viewDidLayoutSubviews()
        // A chat opens at its newest end. Not before now: until the list
        // has its size and insets, there's no "end" to scroll to.
        if !openedAtEnd, tableView.bounds.height > 0 {
            openedAtEnd = true
            scrollToEnd()
        }
    }

    private func scrollToEnd() {
        guard !rows.isEmpty else { return }
        tableView.layoutIfNeeded()
        tableView.scrollToRow(at: IndexPath(row: rows.count - 1, section: 0), at: .bottom, animated: false)
    }

    private func showEmptyIfNeeded() {
        guard rows.isEmpty else {
            contentUnavailableConfiguration = nil
            tableView.isHidden = false
            return
        }
        var empty = UIContentUnavailableConfiguration.empty()
        empty.image = UIImage(systemName: "bubble.left.and.bubble.right")
        empty.text = "No Calls Yet"
        empty.secondaryText = "Calls in this room show up here, with what was said when someone transcribes them."
        empty.button = .borderedProminent()
        empty.button.title = "Call"
        empty.button.image = UIImage(systemName: "video.fill")
        empty.button.imagePadding = 6
        empty.buttonProperties.primaryAction = UIAction { [weak self] _ in self?.call() }
        contentUnavailableConfiguration = empty
        tableView.isHidden = true
    }

    /// Screen shares, recordings and transcripts get their symbol in the
    /// line; comings and goings stay plain.
    private static func symbol(for moment: Moment) -> String? {
        switch moment.kind {
        case .shareStarted, .shareStopped, .recordingStarted, .recordingStopped,
             .transcriptStarted, .transcriptStopped:
            return moment.event.symbol
        default:
            return nil
        }
    }

    /// What a call had, as symbols for its closing line: a shared screen, a
    /// recording, a transcript.
    private static func had(_ moments: [Moment]) -> [String] {
        var symbols: [String] = []
        if moments.contains(where: { $0.kind == .shareStarted }) {
            symbols.append("rectangle.inset.filled.and.person.filled")
        }
        if moments.contains(where: { $0.kind == .recordingStarted }) {
            symbols.append("record.circle")
        }
        if moments.contains(where: { $0.kind == .transcriptStarted || $0.kind == .said }) {
            symbols.append("captions.bubble")
        }
        return symbols
    }

    private static func time(_ date: Date) -> String {
        date.formatted(date: .omitted, time: .shortened)
    }

    private static func day(_ date: Date) -> String {
        let calendar = Calendar.current
        if calendar.isDateInToday(date) { return "Today" }
        if calendar.isDateInYesterday(date) { return "Yesterday" }
        return date.formatted(.dateTime.day().month(.wide).year())
    }

    func tableView(_ tableView: UITableView, numberOfRowsInSection section: Int) -> Int {
        rows.count
    }

    func tableView(_ tableView: UITableView, cellForRowAt indexPath: IndexPath) -> UITableViewCell {
        switch rows[indexPath.row] {
        case let .day(text):
            let cell = tableView.dequeueReusableCell(withIdentifier: "service", for: indexPath) as! ServiceCell
            cell.show(text, lead: nil, trail: [], strong: true)
            return cell
        case let .service(text, lead, trail, strong):
            let cell = tableView.dequeueReusableCell(withIdentifier: "service", for: indexPath) as! ServiceCell
            cell.show(text, lead: lead, trail: trail, strong: strong)
            return cell
        case let .said(moment, first, last):
            let cell = tableView.dequeueReusableCell(withIdentifier: "bubble", for: indexPath) as! BubbleCell
            cell.show(moment, first: first, last: last)
            return cell
        }
    }
}

/// A centred line between bubbles: a day, a call starting or finishing,
/// someone joining. The messenger's "service message".
private final class ServiceCell: UITableViewCell {
    private let pill = UIView()
    private let label = UILabel()

    override init(style: UITableViewCell.CellStyle, reuseIdentifier: String?) {
        super.init(style: style, reuseIdentifier: reuseIdentifier)
        backgroundColor = .clear
        pill.backgroundColor = .tertiarySystemFill
        pill.layer.cornerRadius = 11
        pill.layer.cornerCurve = .continuous
        label.adjustsFontForContentSizeCategory = true
        label.numberOfLines = 0
        label.textAlignment = .center
        pill.translatesAutoresizingMaskIntoConstraints = false
        label.translatesAutoresizingMaskIntoConstraints = false
        contentView.addSubview(pill)
        pill.addSubview(label)
        NSLayoutConstraint.activate([
            pill.topAnchor.constraint(equalTo: contentView.topAnchor, constant: 5),
            pill.bottomAnchor.constraint(equalTo: contentView.bottomAnchor, constant: -5),
            pill.centerXAnchor.constraint(equalTo: contentView.centerXAnchor),
            pill.widthAnchor.constraint(lessThanOrEqualTo: contentView.widthAnchor, constant: -48),
            label.topAnchor.constraint(equalTo: pill.topAnchor, constant: 4),
            label.bottomAnchor.constraint(equalTo: pill.bottomAnchor, constant: -4),
            label.leadingAnchor.constraint(equalTo: pill.leadingAnchor, constant: 10),
            label.trailingAnchor.constraint(equalTo: pill.trailingAnchor, constant: -10),
        ])
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError() }

    /// `strong`: a day or a call's edge, which stands out a little more than
    /// someone joining. `lead` is a symbol before the words; `trail` are
    /// symbols after them (what a call had).
    func show(_ text: String, lead: String?, trail: [String], strong: Bool) {
        let font = UIFont.preferredFont(forTextStyle: .footnote)
        let line = NSMutableAttributedString()
        func symbol(_ name: String) -> NSAttributedString? {
            let configuration = UIImage.SymbolConfiguration(font: font)
            guard let image = UIImage(systemName: name, withConfiguration: configuration) else { return nil }
            return NSAttributedString(attachment: NSTextAttachment(image: image.withTintColor(.secondaryLabel)))
        }
        if let lead, let mark = symbol(lead) {
            line.append(mark)
            line.append(NSAttributedString(string: "  "))
        }
        line.append(NSAttributedString(string: text))
        for (index, name) in trail.enumerated() {
            guard let mark = symbol(name) else { continue }
            line.append(NSAttributedString(string: index == 0 ? "  ·  " : "  "))
            line.append(mark)
        }
        label.font = strong ? font.bold : font
        label.textColor = .secondaryLabel
        label.attributedText = line
        label.accessibilityLabel = trail.isEmpty ? text : text + ", " + Self.spoken(trail)
        pill.backgroundColor = strong ? .tertiarySystemFill : .clear
    }

    /// The closing symbols, in words, for VoiceOver.
    private static func spoken(_ symbols: [String]) -> String {
        let words = symbols.map { symbol -> String in
            switch symbol {
            case "record.circle": return "recorded"
            case "captions.bubble": return "transcribed"
            default: return "a screen was shared"
            }
        }
        return ListFormatter.localizedString(byJoining: words)
    }
}

/// Something someone said: a chat bubble, yours on the right in the tint
/// colour, everyone else's on the left in grey under their name.
private final class BubbleCell: UITableViewCell {
    private let face = UIImageView()
    private let bubble = UIView()
    private let name = UILabel()
    private let body = UILabel()
    private let meta = UILabel()
    private var theirs: [NSLayoutConstraint] = []
    private var mine: [NSLayoutConstraint] = []
    private var top: NSLayoutConstraint!

    override init(style: UITableViewCell.CellStyle, reuseIdentifier: String?) {
        super.init(style: style, reuseIdentifier: reuseIdentifier)
        backgroundColor = .clear
        bubble.layer.cornerRadius = 18
        bubble.layer.cornerCurve = .continuous
        name.font = .preferredFont(forTextStyle: .footnote).bold
        body.font = .preferredFont(forTextStyle: .body)
        body.numberOfLines = 0
        meta.font = .preferredFont(forTextStyle: .caption2)
        for label in [name, body, meta] {
            label.adjustsFontForContentSizeCategory = true
        }
        let text = UIStackView(arrangedSubviews: [name, body, meta])
        text.axis = .vertical
        text.spacing = 2
        text.setCustomSpacing(3, after: body)
        meta.textAlignment = .right

        for view in [face, bubble, text] {
            view.translatesAutoresizingMaskIntoConstraints = false
        }
        contentView.addSubview(face)
        contentView.addSubview(bubble)
        bubble.addSubview(text)
        top = bubble.topAnchor.constraint(equalTo: contentView.topAnchor, constant: 2)
        NSLayoutConstraint.activate([
            top,
            bubble.bottomAnchor.constraint(equalTo: contentView.bottomAnchor, constant: -2),
            bubble.widthAnchor.constraint(lessThanOrEqualTo: contentView.widthAnchor, multiplier: 0.76),
            text.topAnchor.constraint(equalTo: bubble.topAnchor, constant: 7),
            text.bottomAnchor.constraint(equalTo: bubble.bottomAnchor, constant: -7),
            text.leadingAnchor.constraint(equalTo: bubble.leadingAnchor, constant: 12),
            text.trailingAnchor.constraint(equalTo: bubble.trailingAnchor, constant: -12),
            face.leadingAnchor.constraint(equalTo: contentView.leadingAnchor, constant: 12),
            face.bottomAnchor.constraint(equalTo: bubble.bottomAnchor),
            face.widthAnchor.constraint(equalToConstant: 30),
            face.heightAnchor.constraint(equalToConstant: 30),
        ])
        theirs = [bubble.leadingAnchor.constraint(equalTo: face.trailingAnchor, constant: 8)]
        mine = [bubble.trailingAnchor.constraint(equalTo: contentView.trailingAnchor, constant: -12)]
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError() }

    func show(_ moment: Moment, first: Bool, last: Bool) {
        NSLayoutConstraint.deactivate(moment.mine ? theirs : mine)
        NSLayoutConstraint.activate(moment.mine ? mine : theirs)
        // A new speaker gets a little air above.
        top.constant = first ? 8 : 2

        bubble.backgroundColor = moment.mine ? .tintColor : .systemGray5
        body.text = moment.text
        body.textColor = moment.mine ? .white : .label
        // Their name over the first bubble of a run, in their own colour.
        name.isHidden = moment.mine || !first
        name.text = moment.who
        name.textColor = Self.nameColor(moment.who)
        // The time, and a mark that this was transcribed, not typed.
        let font = UIFont.preferredFont(forTextStyle: .caption2)
        let color: UIColor = moment.mine ? UIColor.white.withAlphaComponent(0.75) : .secondaryLabel
        let line = NSMutableAttributedString()
        if let image = UIImage(systemName: "captions.bubble", withConfiguration: UIImage.SymbolConfiguration(font: font)) {
            line.append(NSAttributedString(attachment: NSTextAttachment(image: image.withTintColor(color))))
            line.append(NSAttributedString(string: " "))
        }
        line.append(NSAttributedString(string: moment.time))
        meta.attributedText = line
        meta.textColor = color
        meta.accessibilityLabel = "transcribed at \(moment.time)"
        // Their face beside the last bubble of a run.
        face.isHidden = moment.mine || !last
        face.image = last && !moment.mine ? Avatar.image(moment.who, side: 30) : nil
    }

    /// The person's avatar colour, dark enough to read on a light bubble and
    /// light enough on a dark one.
    private static func nameColor(_ name: String) -> UIColor {
        let hue = Avatar.hue(name)
        return UIColor { traits in
            UIColor(hue: hue, saturation: 0.6, lightness: traits.userInterfaceStyle == .dark ? 0.68 : 0.4)
        }
    }
}
