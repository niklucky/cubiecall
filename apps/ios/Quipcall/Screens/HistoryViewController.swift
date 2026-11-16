import UIKit

/// A room's page, or one call's: what happened, and a Call button.
///
/// - A **room** shows every call you were in there, newest first, each under
///   its own header (when, how long), with what happened in it: who came and
///   went, screen shares, recordings, and what was said if someone's Mac
///   transcribed it.
/// - A **call** (from the Calls tab) shows its details, what happened in it,
///   and the earlier calls in the same room.
///
/// All of it is the phone's own record (the engine writes it as a call ends).
/// Messages and files will appear here too once Quipcall has them.
final class HistoryViewController: UITableViewController {
    enum Subject {
        case room(id: String, name: String, detail: String?)
        case call(CallRecord)
    }

    private let subject: Subject
    private var sections: [Section] = []

    private struct Section {
        var title: String?
        var rows: [Row]
    }

    private enum Row {
        /// A label and its value: "Length", "12 min".
        case fact(String, String)
        case moment(Moment)
        /// Another call: tap for its page.
        case call(CallRecord)
        /// The room's page.
        case room
        /// Nothing to show, said plainly.
        case note(String)
    }

    init(_ subject: Subject) {
        self.subject = subject
        super.init(style: .insetGrouped)
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError() }

    private var room: String {
        switch subject {
        case let .room(id, _, _): return id
        case let .call(call): return call.room
        }
    }

    private var name: String {
        switch subject {
        case let .room(_, name, _): return name
        case let .call(call): return call.name
        }
    }

    override func viewDidLoad() {
        super.viewDidLoad()
        navigationItem.largeTitleDisplayMode = .never
        tableView.register(UITableViewCell.self, forCellReuseIdentifier: "row")
        tableView.tableHeaderView = header()
        NotificationCenter.default.addObserver(self, selector: #selector(reload), name: .callsChanged, object: nil)
        reload()
    }

    override func viewDidLayoutSubviews() {
        super.viewDidLayoutSubviews()
        // A table's header doesn't size itself: fit it to the width.
        guard let header = tableView.tableHeaderView else { return }
        let fitted = header.systemLayoutSizeFitting(
            CGSize(width: tableView.bounds.width, height: 0),
            withHorizontalFittingPriority: .required, verticalFittingPriority: .fittingSizeLevel
        )
        if abs(header.frame.height - fitted.height) > 0.5 {
            header.frame.size = CGSize(width: tableView.bounds.width, height: fitted.height)
            tableView.tableHeaderView = header
        }
    }

    // MARK: The face, the name, Call and Share

    private func header() -> UIView {
        let face = UIImageView(image: Avatar.image(name, side: 88))
        let title = UILabel()
        title.text = name
        title.font = .preferredFont(forTextStyle: .title2).bold
        title.adjustsFontForContentSizeCategory = true
        title.numberOfLines = 2
        title.textAlignment = .center
        let detail = UILabel()
        detail.font = .preferredFont(forTextStyle: .subheadline)
        detail.textColor = .secondaryLabel
        detail.adjustsFontForContentSizeCategory = true
        switch subject {
        case let .room(_, _, text): detail.text = text
        case let .call(call): detail.text = call.fullDate
        }

        var callLook = UIButton.Configuration.borderedProminent()
        callLook.title = "Call"
        callLook.image = UIImage(systemName: "video.fill")
        callLook.imagePadding = 6
        callLook.buttonSize = .large
        callLook.cornerStyle = .capsule
        let call = UIButton(configuration: callLook, primaryAction: UIAction { [weak self] _ in
            guard let self else { return }
            CallViewController.present(room: self.room, name: self.name, from: self)
        })
        var shareLook = UIButton.Configuration.bordered()
        shareLook.title = "Share"
        shareLook.image = UIImage(systemName: "square.and.arrow.up")
        shareLook.imagePadding = 6
        shareLook.buttonSize = .large
        shareLook.cornerStyle = .capsule
        let share = UIButton(configuration: shareLook)
        share.addAction(UIAction { [weak self, weak share] _ in
            guard let self, let url = URL(string: AppEnv.shared.client.roomLink(room: self.room)) else { return }
            let sheet = UIActivityViewController(activityItems: [url], applicationActivities: nil)
            sheet.popoverPresentationController?.sourceView = share
            self.present(sheet, animated: true)
        }, for: .touchUpInside)
        let buttons = UIStackView(arrangedSubviews: [call, share])
        buttons.spacing = 12
        buttons.distribution = .fillEqually

        let stack = UIStackView(arrangedSubviews: [face, title, detail, buttons])
        stack.axis = .vertical
        stack.alignment = .center
        stack.spacing = 6
        stack.setCustomSpacing(12, after: face)
        stack.setCustomSpacing(18, after: detail)
        stack.isLayoutMarginsRelativeArrangement = true
        stack.directionalLayoutMargins = NSDirectionalEdgeInsets(top: 12, leading: 20, bottom: 8, trailing: 20)
        buttons.widthAnchor.constraint(equalTo: stack.layoutMarginsGuide.widthAnchor).isActive = true
        return stack
    }

    // MARK: What happened

    @objc private func reload() {
        let client = AppEnv.shared.client
        var sections: [Section] = []
        switch subject {
        case .room:
            let calls = client.roomCalls(room: room)
            if calls.isEmpty {
                sections.append(Section(title: nil, rows: [
                    .note("No calls here yet. Each call you make shows up with who joined, screen shares, recordings and transcripts."),
                ]))
            }
            // Each call under its own header: the line between calls.
            for call in calls {
                var rows: [Row] = [.fact("People", call.peopleText), .fact("Length", call.lengthText)]
                rows += client.callMoments(id: call.id).map(Row.moment)
                sections.append(Section(title: call.fullDate, rows: rows))
            }
        case let .call(call):
            sections.append(Section(title: nil, rows: [
                .fact("Date", call.fullDate),
                .fact("Length", call.lengthText),
                .fact("People", call.peopleText),
            ]))
            let moments = client.callMoments(id: call.id)
            sections.append(Section(
                title: "In This Call",
                rows: moments.isEmpty
                    ? [.note("Nothing else happened: nobody came or went, shared a screen, recorded or transcribed.")]
                    : moments.map(Row.moment)
            ))
            let earlier = client.roomCalls(room: call.room).filter { $0.started < call.started }
            if !earlier.isEmpty {
                sections.append(Section(title: "Previous Calls", rows: earlier.map(Row.call)))
            }
            sections.append(Section(title: nil, rows: [.room]))
        }
        self.sections = sections
        tableView.reloadData()
    }

    override func numberOfSections(in tableView: UITableView) -> Int {
        sections.count
    }

    override func tableView(_ tableView: UITableView, numberOfRowsInSection section: Int) -> Int {
        sections[section].rows.count
    }

    override func tableView(_ tableView: UITableView, titleForHeaderInSection section: Int) -> String? {
        sections[section].title
    }

    override func tableView(_ tableView: UITableView, cellForRowAt indexPath: IndexPath) -> UITableViewCell {
        let cell = tableView.dequeueReusableCell(withIdentifier: "row", for: indexPath)
        cell.accessoryType = .none
        cell.selectionStyle = .none
        switch sections[indexPath.section].rows[indexPath.row] {
        case let .fact(label, value):
            var content = UIListContentConfiguration.valueCell()
            content.text = label
            content.secondaryText = value
            cell.contentConfiguration = content
        case let .moment(moment):
            cell.contentConfiguration = moment.content
        case let .call(call):
            var content = UIListContentConfiguration.subtitleCell()
            content.text = call.fullDate
            content.secondaryText = call.summary
            content.secondaryTextProperties.color = .secondaryLabel
            cell.contentConfiguration = content
            cell.accessoryType = .disclosureIndicator
            cell.selectionStyle = .default
        case .room:
            var content = cell.defaultContentConfiguration()
            content.text = "Room History"
            content.image = UIImage(systemName: "clock.arrow.circlepath")
            cell.contentConfiguration = content
            cell.accessoryType = .disclosureIndicator
            cell.selectionStyle = .default
        case let .note(text):
            var content = cell.defaultContentConfiguration()
            content.text = text
            content.textProperties.color = .secondaryLabel
            content.textProperties.font = .preferredFont(forTextStyle: .subheadline)
            cell.contentConfiguration = content
        }
        return cell
    }

    override func tableView(_ tableView: UITableView, didSelectRowAt indexPath: IndexPath) {
        tableView.deselectRow(at: indexPath, animated: true)
        switch sections[indexPath.section].rows[indexPath.row] {
        case let .call(call):
            navigationController?.pushViewController(HistoryViewController(.call(call)), animated: true)
        case .room:
            let page = HistoryViewController(.room(id: room, name: name, detail: nil))
            navigationController?.pushViewController(page, animated: true)
        default:
            break
        }
    }
}

// MARK: Words for calls and what happens in them

extension CallRecord {
    private static let lengths: DateComponentsFormatter = {
        let formatter = DateComponentsFormatter()
        formatter.unitsStyle = .short
        formatter.maximumUnitCount = 2
        return formatter
    }()

    private var date: Date {
        Date(timeIntervalSince1970: TimeInterval(started))
    }

    /// "12 min", "1 hr, 4 min", "23 sec".
    var lengthText: String {
        Self.lengths.allowedUnits = seconds < 60 ? [.second] : [.hour, .minute]
        return Self.lengths.string(from: TimeInterval(seconds)) ?? ""
    }

    /// "Anna, Max and Lena", or that nobody came.
    var peopleText: String {
        people.isEmpty ? "Nobody joined" : ListFormatter.localizedString(byJoining: people)
    }

    /// One line for a list: who and how long.
    var summary: String {
        people.isEmpty ? peopleText : peopleText + " · " + lengthText
    }

    /// For a list's edge: the time today, "Yesterday", the weekday this week,
    /// the date before that.
    var whenText: String {
        let calendar = Calendar.current
        if calendar.isDateInToday(date) {
            return date.formatted(date: .omitted, time: .shortened)
        }
        if calendar.isDateInYesterday(date) {
            return "Yesterday"
        }
        if let days = calendar.dateComponents([.day], from: date, to: Date()).day, days < 7 {
            return date.formatted(.dateTime.weekday(.wide))
        }
        return date.formatted(date: .numeric, time: .omitted)
    }

    /// In full: "Today at 21:23", "16 Nov 2026 at 21:23".
    var fullDate: String {
        let time = date.formatted(date: .omitted, time: .shortened)
        let calendar = Calendar.current
        if calendar.isDateInToday(date) { return "Today at \(time)" }
        if calendar.isDateInYesterday(date) { return "Yesterday at \(time)" }
        return date.formatted(date: .abbreviated, time: .shortened)
    }
}

private extension Moment {
    var time: String {
        Date(timeIntervalSince1970: TimeInterval(at)).formatted(date: .omitted, time: .shortened)
    }

    private var actor: String {
        mine ? "You" : who
    }

    /// What happened, in words, and the symbol that goes with it.
    private var event: (text: String, symbol: String) {
        switch kind {
        case .joined: return ("\(actor) joined", "person.crop.circle.badge.plus")
        case .left: return ("\(actor) left", "person.crop.circle.badge.minus")
        case .shareStarted: return ("\(actor) started sharing a screen", "rectangle.inset.filled.and.person.filled")
        case .shareStopped: return ("\(actor) stopped sharing", "rectangle.slash")
        case .recordingStarted: return ("\(actor) started recording", "record.circle")
        case .recordingStopped: return ("\(actor) stopped recording", "stop.circle")
        case .transcriptStarted:
            return (mine ? "Transcript started" : "Transcript started on \(who)'s Mac", "captions.bubble")
        case .transcriptStopped: return ("Transcript stopped", "captions.bubble")
        case .said: return (text, "quote.bubble")
        }
    }

    /// A row of the history: a line of the transcript reads as a message
    /// (who, then what they said); anything else is a quiet line with a symbol.
    var content: UIListContentConfiguration {
        if kind == .said {
            var content = UIListContentConfiguration.subtitleCell()
            content.text = "\(actor) · \(time)"
            content.textProperties.font = .preferredFont(forTextStyle: .footnote)
            content.textProperties.color = .secondaryLabel
            content.secondaryText = text
            content.secondaryTextProperties.font = .preferredFont(forTextStyle: .body)
            content.secondaryTextProperties.color = .label
            content.image = Avatar.image(who, side: 28)
            content.textToSecondaryTextVerticalPadding = 2
            return content
        }
        var content = UIListContentConfiguration.valueCell()
        content.text = event.text
        content.textProperties.font = .preferredFont(forTextStyle: .subheadline)
        content.secondaryText = time
        content.secondaryTextProperties.font = .preferredFont(forTextStyle: .subheadline)
        content.image = UIImage(systemName: event.symbol)
        content.imageProperties.tintColor = .secondaryLabel
        return content
    }
}
