import UIKit

/// Your rooms, as the system draws a list: a face, the name, a line under
/// it. Tap one to call into it.
final class RoomsViewController: ListViewController, UISearchResultsUpdating {
    private var rooms: [Room] = []
    private var shown: [Room] = []
    private var loading: Task<Void, Never>?
    private var failure: String?

    init() {
        super.init(style: .plain)
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError() }

    override func viewDidLoad() {
        super.viewDidLoad()
        title = "Rooms"
        tableView.register(UITableViewCell.self, forCellReuseIdentifier: "room")
        navigationItem.rightBarButtonItem = UIBarButtonItem(
            systemItem: .add,
            primaryAction: UIAction { [weak self] _ in self?.newRoom() }
        )
        let search = UISearchController(searchResultsController: nil)
        search.searchResultsUpdater = self
        search.obscuresBackgroundDuringPresentation = false
        navigationItem.searchController = search
        tableView.refreshControl = UIRefreshControl()
        tableView.refreshControl?.addAction(UIAction { [weak self] _ in self?.load() }, for: .valueChanged)

        // Refreshed when the app comes back, not on a timer: nothing runs
        // while the list just sits there.
        NotificationCenter.default.addObserver(
            self, selector: #selector(load), name: UIApplication.willEnterForegroundNotification, object: nil
        )
        load()
    }

    @objc private func load() {
        loading?.cancel()
        loading = Task { [weak self] in
            let client = AppEnv.shared.client
            do {
                // Who you are first: it says which rooms are yours.
                if client.account() == nil { _ = try await client.refresh() }
                let rooms = try await client.rooms()
                guard let self, !Task.isCancelled else { return }
                self.rooms = rooms
                self.failure = nil
                self.filter()
            } catch AppError.SignedOut {
                SceneDelegate.signedOut()
            } catch {
                guard let self, !Task.isCancelled else { return }
                self.failure = error.shown
                self.filter()
            }
            self?.tableView.refreshControl?.endRefreshing()
        }
    }

    func updateSearchResults(for searchController: UISearchController) {
        filter()
    }

    private func filter() {
        let query = navigationItem.searchController?.searchBar.text ?? ""
        shown = query.isEmpty ? rooms : rooms.filter { $0.name.localizedCaseInsensitiveContains(query) }
        tableView.reloadData()
        setNeedsUpdateContentUnavailableConfiguration()
    }

    override func updateContentUnavailableConfiguration(using state: UIContentUnavailableConfigurationState) {
        guard shown.isEmpty else {
            showEmpty(nil)
            return
        }
        if !(navigationItem.searchController?.searchBar.text ?? "").isEmpty {
            showEmpty(UIContentUnavailableConfiguration.search())
            return
        }
        var empty = UIContentUnavailableConfiguration.empty()
        if let failure, rooms.isEmpty {
            empty.image = UIImage(systemName: "wifi.exclamationmark")
            empty.text = "Can't Load Rooms"
            empty.secondaryText = failure
        } else {
            empty.image = UIImage(systemName: "bubble.left.and.bubble.right")
            empty.text = "No Rooms"
            empty.secondaryText = "Make one with + and send its link."
        }
        showEmpty(empty)
    }

    private func newRoom() {
        let alert = UIAlertController(title: "New Room", message: "Only people you invite get in.", preferredStyle: .alert)
        alert.addTextField { field in
            field.placeholder = "Name"
            field.autocapitalizationType = .sentences
        }
        alert.addAction(UIAlertAction(title: "Cancel", style: .cancel))
        alert.addAction(UIAlertAction(title: "Create", style: .default) { [weak self, weak alert] _ in
            let name = alert?.textFields?.first?.text?.trimmingCharacters(in: .whitespaces) ?? ""
            guard !name.isEmpty else { return }
            Task {
                do {
                    let room = try await AppEnv.shared.client.createRoom(name: name, membersOnly: true)
                    self?.rooms.insert(room, at: 0)
                    self?.filter()
                } catch {
                    self?.say(error.shown)
                }
            }
        })
        present(alert, animated: true)
    }

    private func share(_ room: Room, from source: UIView?) {
        guard let url = URL(string: AppEnv.shared.client.roomLink(room: room.id)) else { return }
        let sheet = UIActivityViewController(activityItems: [url], applicationActivities: nil)
        sheet.popoverPresentationController?.sourceView = source ?? view
        present(sheet, animated: true)
    }

    // MARK: The list

    override func tableView(_ tableView: UITableView, numberOfRowsInSection section: Int) -> Int {
        shown.count
    }

    override func tableView(_ tableView: UITableView, cellForRowAt indexPath: IndexPath) -> UITableViewCell {
        let cell = tableView.dequeueReusableCell(withIdentifier: "room", for: indexPath)
        let room = shown[indexPath.row]
        let detail: String
        switch room.kind {
        case .direct: detail = "Call for two"
        case .open: detail = "Anyone with the link"
        case .members: detail = room.mine ? "Private · yours" : "Private"
        }
        cell.contentConfiguration = UIListContentConfiguration.face(room.name, detail: detail)
        return cell
    }

    override func tableView(_ tableView: UITableView, didSelectRowAt indexPath: IndexPath) {
        tableView.deselectRow(at: indexPath, animated: true)
        let room = shown[indexPath.row]
        CallViewController.present(room: room.id, name: room.name, from: self)
    }

    override func tableView(_ tableView: UITableView, canEditRowAt indexPath: IndexPath) -> Bool { true }

    override func tableView(
        _ tableView: UITableView,
        trailingSwipeActionsConfigurationForRowAt indexPath: IndexPath
    ) -> UISwipeActionsConfiguration? {
        let room = shown[indexPath.row]
        let share = UIContextualAction(style: .normal, title: "Share") { [weak self] _, view, done in
            self?.share(room, from: view)
            done(true)
        }
        share.image = UIImage(systemName: "square.and.arrow.up")
        share.backgroundColor = .systemBlue
        return UISwipeActionsConfiguration(actions: [share])
    }

    override func tableView(
        _ tableView: UITableView,
        contextMenuConfigurationForRowAt indexPath: IndexPath,
        point: CGPoint
    ) -> UIContextMenuConfiguration? {
        let room = shown[indexPath.row]
        let link = AppEnv.shared.client.roomLink(room: room.id)
        return UIContextMenuConfiguration(actionProvider: { [weak self] _ in
            UIMenu(children: [
                UIAction(title: "Call", image: UIImage(systemName: "video")) { _ in
                    guard let self else { return }
                    CallViewController.present(room: room.id, name: room.name, from: self)
                },
                UIAction(title: "Copy Link", image: UIImage(systemName: "link")) { _ in
                    UIPasteboard.general.string = link
                },
                UIAction(title: "Share…", image: UIImage(systemName: "square.and.arrow.up")) { _ in
                    self?.share(room, from: tableView.cellForRow(at: indexPath))
                },
            ])
        })
    }
}
