import UIKit

/// Your rooms, as a chat list: a face, the name, a line under it. Tap one to
/// call into it.
final class RoomsViewController: UITableViewController {
    private var rooms: [Room] = []
    private var loading: Task<Void, Never>?
    private let empty = UILabel()

    override func viewDidLoad() {
        super.viewDidLoad()
        title = "Rooms"
        view.backgroundColor = Theme.bg
        tableView.backgroundColor = Theme.bg
        tableView.separatorColor = Theme.line
        tableView.separatorInset = UIEdgeInsets(top: 0, left: 84, bottom: 0, right: 0)
        tableView.rowHeight = 76
        tableView.register(RoomCell.self, forCellReuseIdentifier: RoomCell.id)
        navigationItem.rightBarButtonItem = UIBarButtonItem(
            systemItem: .add,
            primaryAction: UIAction { [weak self] _ in self?.newRoom() }
        )
        refreshControl = UIRefreshControl()
        refreshControl?.addAction(UIAction { [weak self] _ in self?.load() }, for: .valueChanged)

        empty.text = "No rooms yet.\nMake one with + and send its link."
        empty.numberOfLines = 0
        empty.textAlignment = .center
        empty.textColor = Theme.textDim
        empty.font = .systemFont(ofSize: 16)

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
                self.tableView.reloadData()
                self.tableView.backgroundView = rooms.isEmpty ? self.empty : nil
            } catch AppError.SignedOut {
                SceneDelegate.signedOut()
            } catch {
                guard let self, !Task.isCancelled else { return }
                if self.rooms.isEmpty {
                    self.empty.text = error.shown
                    self.tableView.backgroundView = self.empty
                }
            }
            self?.refreshControl?.endRefreshing()
        }
    }

    private func newRoom() {
        let alert = UIAlertController(title: "New room", message: "Only people you invite get in.", preferredStyle: .alert)
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
                    self?.tableView.reloadData()
                    self?.tableView.backgroundView = nil
                } catch {
                    self?.say(error.shown)
                }
            }
        })
        present(alert, animated: true)
    }

    private func say(_ message: String) {
        let alert = UIAlertController(title: nil, message: message, preferredStyle: .alert)
        alert.addAction(UIAlertAction(title: "OK", style: .default))
        present(alert, animated: true)
    }

    // MARK: The list

    override func tableView(_ tableView: UITableView, numberOfRowsInSection section: Int) -> Int {
        rooms.count
    }

    override func tableView(_ tableView: UITableView, cellForRowAt indexPath: IndexPath) -> UITableViewCell {
        let cell = tableView.dequeueReusableCell(withIdentifier: RoomCell.id, for: indexPath) as! RoomCell
        cell.show(rooms[indexPath.row])
        return cell
    }

    override func tableView(_ tableView: UITableView, didSelectRowAt indexPath: IndexPath) {
        tableView.deselectRow(at: indexPath, animated: true)
        let room = rooms[indexPath.row]
        CallViewController.present(room: room.id, name: room.name, from: self)
    }

    override func tableView(
        _ tableView: UITableView,
        contextMenuConfigurationForRowAt indexPath: IndexPath,
        point: CGPoint
    ) -> UIContextMenuConfiguration? {
        let room = rooms[indexPath.row]
        let link = AppEnv.shared.client.roomLink(room: room.id)
        return UIContextMenuConfiguration(actionProvider: { [weak self] _ in
            UIMenu(children: [
                UIAction(title: "Copy link", image: UIImage(systemName: "link")) { _ in
                    UIPasteboard.general.string = link
                },
                UIAction(title: "Share…", image: UIImage(systemName: "square.and.arrow.up")) { [weak self] _ in
                    guard let self, let url = URL(string: link) else { return }
                    self.present(UIActivityViewController(activityItems: [url], applicationActivities: nil), animated: true)
                },
            ])
        })
    }
}

/// One room: its face, its name, what kind it is.
final class RoomCell: UITableViewCell {
    static let id = "room"
    private let avatar = AvatarView()
    private let name = UILabel()
    private let detail = UILabel()

    override init(style: UITableViewCell.CellStyle, reuseIdentifier: String?) {
        super.init(style: style, reuseIdentifier: reuseIdentifier)
        backgroundColor = Theme.bg
        let selected = UIView()
        selected.backgroundColor = Theme.raised
        selectedBackgroundView = selected
        name.font = .systemFont(ofSize: 17, weight: .semibold)
        name.textColor = Theme.text
        detail.font = .systemFont(ofSize: 15)
        detail.textColor = Theme.textDim
        for view in [avatar, name, detail] {
            contentView.addSubview(view)
        }
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError() }

    func show(_ room: Room) {
        avatar.name = room.name
        name.text = room.name
        switch room.kind {
        case .direct: detail.text = "Call for two"
        case .open: detail.text = "Open to anyone with the link"
        case .members: detail.text = room.mine ? "Private · yours" : "Private"
        }
    }

    // Laid out by hand: three views, no constraints to solve while scrolling.
    override func layoutSubviews() {
        super.layoutSubviews()
        let bounds = contentView.bounds
        let left = contentView.layoutMargins.left
        avatar.frame = CGRect(x: left, y: (bounds.height - 56) / 2, width: 56, height: 56)
        let x = avatar.frame.maxX + 12
        let width = bounds.width - x - contentView.layoutMargins.right
        name.frame = CGRect(x: x, y: bounds.midY - 23, width: width, height: 22)
        detail.frame = CGRect(x: x, y: bounds.midY + 1, width: width, height: 20)
    }
}
