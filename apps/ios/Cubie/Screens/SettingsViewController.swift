import UIKit

/// You (tap to change your name), and signing out. More comes with the call
/// settings.
final class SettingsViewController: UITableViewController {
    init() {
        super.init(style: .insetGrouped)
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError() }

    override func viewDidLoad() {
        super.viewDidLoad()
        title = "Settings"
        tableView.register(UITableViewCell.self, forCellReuseIdentifier: "cell")
    }

    override func viewWillAppear(_ animated: Bool) {
        super.viewWillAppear(animated)
        tableView.reloadData()
    }

    override func numberOfSections(in tableView: UITableView) -> Int { 2 }

    override func tableView(_ tableView: UITableView, numberOfRowsInSection section: Int) -> Int { 1 }

    override func tableView(_ tableView: UITableView, cellForRowAt indexPath: IndexPath) -> UITableViewCell {
        let cell = tableView.dequeueReusableCell(withIdentifier: "cell", for: indexPath)
        if indexPath.section == 0 {
            let account = AppEnv.shared.client.account()
            // A new account may not have said its name yet.
            let name = account.flatMap { $0.name.isEmpty ? nil : $0.name } ?? "You"
            var content = UIListContentConfiguration.face(name, detail: account?.email)
            content.image = Avatar.image(name, side: 60)
            content.textProperties.font = .preferredFont(forTextStyle: .title3)
            cell.contentConfiguration = content
            cell.selectionStyle = .default
            cell.accessibilityHint = "Changes your name"
        } else {
            var content = cell.defaultContentConfiguration()
            content.text = "Sign Out"
            content.textProperties.color = .systemRed
            cell.contentConfiguration = content
            cell.selectionStyle = .default
        }
        return cell
    }

    override func tableView(_ tableView: UITableView, titleForFooterInSection section: Int) -> String? {
        guard section == 1 else { return nil }
        let info = Bundle.main.infoDictionary
        let version = info?["CFBundleShortVersionString"] as? String ?? ""
        let build = info?["CFBundleVersion"] as? String ?? ""
        return "Cubie \(version) (\(build))"
    }

    override func tableView(_ tableView: UITableView, didSelectRowAt indexPath: IndexPath) {
        tableView.deselectRow(at: indexPath, animated: true)
        guard indexPath.section == 1 else {
            rename()
            return
        }
        let alert = UIAlertController(
            title: nil,
            message: "Your list of calls on this iPhone goes too.",
            preferredStyle: .actionSheet
        )
        alert.addAction(UIAlertAction(title: "Sign Out", style: .destructive) { _ in
            Task {
                await AppEnv.shared.client.signOut()
                SceneDelegate.signedOut()
            }
        })
        alert.addAction(UIAlertAction(title: "Cancel", style: .cancel))
        alert.popoverPresentationController?.sourceView = tableView.cellForRow(at: indexPath)
        present(alert, animated: true)
    }

    /// What people should call you, asked the system's way.
    private func rename() {
        let alert = UIAlertController(title: "Your Name", message: "Shown on your face in calls.", preferredStyle: .alert)
        alert.addTextField { field in
            field.text = AppEnv.shared.client.account()?.name
            field.placeholder = "Your name"
            field.textContentType = .name
            field.autocapitalizationType = .words
        }
        alert.addAction(UIAlertAction(title: "Cancel", style: .cancel))
        alert.addAction(UIAlertAction(title: "Save", style: .default) { [weak self, weak alert] _ in
            let name = (alert?.textFields?.first?.text ?? "").trimmingCharacters(in: .whitespacesAndNewlines)
            guard !name.isEmpty else { return }
            Task {
                do {
                    _ = try await AppEnv.shared.client.setName(name: name)
                    self?.tableView.reloadData()
                } catch {
                    self?.say(error.shown)
                }
            }
        })
        present(alert, animated: true)
    }
}
