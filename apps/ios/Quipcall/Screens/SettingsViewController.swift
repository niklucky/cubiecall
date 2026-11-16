import UIKit

/// You, and signing out. More comes with the call settings.
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
            var content = UIListContentConfiguration.face(account?.name ?? "You", detail: account?.email)
            content.image = Avatar.image(account?.name ?? "You", side: 60)
            content.textProperties.font = .preferredFont(forTextStyle: .title3)
            cell.contentConfiguration = content
            cell.selectionStyle = .none
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
        return "Quipcall \(version) (\(build))"
    }

    override func tableView(_ tableView: UITableView, didSelectRowAt indexPath: IndexPath) {
        tableView.deselectRow(at: indexPath, animated: true)
        guard indexPath.section == 1 else { return }
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
}
