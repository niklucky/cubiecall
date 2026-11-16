import UIKit

/// The calls you were in, newest first, as the Phone app lists its recents:
/// who, how long, when. Tap one to call that room again; swipe to delete.
///
/// Kept on this phone (the engine writes each call down as it ends); the
/// server has no call records yet.
final class CallsViewController: ListViewController {
    private var calls: [CallRecord] = []
    private lazy var clear = UIBarButtonItem(
        title: "Clear", primaryAction: UIAction { [weak self] _ in self?.askToClear() }
    )

    init() {
        super.init(style: .plain)
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError() }

    override func viewDidLoad() {
        super.viewDidLoad()
        title = "Calls"
        tableView.register(UITableViewCell.self, forCellReuseIdentifier: "call")
        navigationItem.rightBarButtonItem = editButtonItem
        NotificationCenter.default.addObserver(self, selector: #selector(reload), name: .callsChanged, object: nil)
    }

    override func viewWillAppear(_ animated: Bool) {
        super.viewWillAppear(animated)
        reload()
    }

    @objc private func reload() {
        calls = AppEnv.shared.client.calls()
        tableView.reloadData()
        editButtonItem.isEnabled = !calls.isEmpty
        if calls.isEmpty, isEditing { setEditing(false, animated: false) }
        setNeedsUpdateContentUnavailableConfiguration()
    }

    override func updateContentUnavailableConfiguration(using state: UIContentUnavailableConfigurationState) {
        guard calls.isEmpty else {
            showEmpty(nil)
            return
        }
        var empty = UIContentUnavailableConfiguration.empty()
        empty.image = UIImage(systemName: "phone")
        empty.text = "No Calls"
        empty.secondaryText = "Calls you make or join on this iPhone show up here."
        showEmpty(empty)
    }

    override func setEditing(_ editing: Bool, animated: Bool) {
        super.setEditing(editing, animated: animated)
        navigationItem.leftBarButtonItem = editing ? clear : nil
    }

    private func askToClear() {
        let alert = UIAlertController(title: nil, message: nil, preferredStyle: .actionSheet)
        alert.addAction(UIAlertAction(title: "Clear All Calls", style: .destructive) { [weak self] _ in
            AppEnv.shared.client.clearCalls()
            self?.reload()
        })
        alert.addAction(UIAlertAction(title: "Cancel", style: .cancel))
        alert.popoverPresentationController?.barButtonItem = clear
        present(alert, animated: true)
    }

    // MARK: The list

    override func tableView(_ tableView: UITableView, numberOfRowsInSection section: Int) -> Int {
        calls.count
    }

    override func tableView(_ tableView: UITableView, cellForRowAt indexPath: IndexPath) -> UITableViewCell {
        let cell = tableView.dequeueReusableCell(withIdentifier: "call", for: indexPath)
        let call = calls[indexPath.row]
        var content = UIListContentConfiguration.face(call.name, detail: Self.detail(call))
        if call.people.isEmpty {
            // Nobody came: the Phone app's red for a call that didn't happen.
            content.textProperties.color = .systemRed
        }
        cell.contentConfiguration = content
        let when = UILabel()
        when.text = Self.when(call.started)
        when.font = .preferredFont(forTextStyle: .subheadline)
        when.textColor = .secondaryLabel
        when.adjustsFontForContentSizeCategory = true
        when.sizeToFit()
        cell.accessoryView = when
        return cell
    }

    override func tableView(_ tableView: UITableView, didSelectRowAt indexPath: IndexPath) {
        tableView.deselectRow(at: indexPath, animated: true)
        let call = calls[indexPath.row]
        CallViewController.present(room: call.room, name: call.name, from: self)
    }

    override func tableView(_ tableView: UITableView, canEditRowAt indexPath: IndexPath) -> Bool { true }

    override func tableView(
        _ tableView: UITableView,
        commit editingStyle: UITableViewCell.EditingStyle,
        forRowAt indexPath: IndexPath
    ) {
        guard editingStyle == .delete else { return }
        AppEnv.shared.client.deleteCall(id: calls[indexPath.row].id)
        calls.remove(at: indexPath.row)
        tableView.deleteRows(at: [indexPath], with: .automatic)
        editButtonItem.isEnabled = !calls.isEmpty
        if calls.isEmpty { setEditing(false, animated: true) }
        setNeedsUpdateContentUnavailableConfiguration()
    }

    // MARK: Words

    /// "Anna, Max and Lena · 12 min", or that nobody came.
    private static func detail(_ call: CallRecord) -> String {
        guard !call.people.isEmpty else { return "Nobody joined" }
        let people = ListFormatter.localizedString(byJoining: call.people)
        return people + " · " + length(call.seconds)
    }

    private static let lengths: DateComponentsFormatter = {
        let formatter = DateComponentsFormatter()
        formatter.unitsStyle = .short
        formatter.maximumUnitCount = 2
        return formatter
    }()

    private static func length(_ seconds: UInt64) -> String {
        lengths.allowedUnits = seconds < 60 ? [.second] : [.hour, .minute]
        return lengths.string(from: TimeInterval(seconds)) ?? ""
    }

    /// The time today, "Yesterday", the weekday this week, the date before.
    private static func when(_ started: UInt64) -> String {
        let date = Date(timeIntervalSince1970: TimeInterval(started))
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
}
