import UIKit

/// You, and signing out. More comes with the call settings.
final class SettingsViewController: UITableViewController {
    private let avatar = AvatarView()

    init() {
        super.init(style: .insetGrouped)
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError() }

    override func viewDidLoad() {
        super.viewDidLoad()
        title = "Settings"
        tableView.backgroundColor = Theme.bg
        tableView.register(UITableViewCell.self, forCellReuseIdentifier: "cell")
    }

    override func viewWillAppear(_ animated: Bool) {
        super.viewWillAppear(animated)
        tableView.reloadData()
    }

    override func numberOfSections(in tableView: UITableView) -> Int { 2 }

    override func tableView(_ tableView: UITableView, numberOfRowsInSection section: Int) -> Int { 1 }

    override func tableView(_ tableView: UITableView, heightForRowAt indexPath: IndexPath) -> CGFloat {
        indexPath.section == 0 ? 76 : 48
    }

    override func tableView(_ tableView: UITableView, cellForRowAt indexPath: IndexPath) -> UITableViewCell {
        let cell = tableView.dequeueReusableCell(withIdentifier: "cell", for: indexPath)
        cell.backgroundColor = Theme.raised
        var content = cell.defaultContentConfiguration()
        if indexPath.section == 0 {
            let account = AppEnv.shared.client.account()
            content.text = account?.name ?? "You"
            content.secondaryText = account?.email
            content.textProperties.font = .systemFont(ofSize: 20, weight: .semibold)
            content.textProperties.color = Theme.text
            content.secondaryTextProperties.color = Theme.textDim
            cell.selectionStyle = .none
        } else {
            content.text = "Sign out"
            content.textProperties.color = Theme.danger
        }
        cell.contentConfiguration = content
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
        let alert = UIAlertController(title: nil, message: "Sign out of Quipcall on this iPhone?", preferredStyle: .actionSheet)
        alert.addAction(UIAlertAction(title: "Sign out", style: .destructive) { _ in
            Task {
                await AppEnv.shared.client.signOut()
                SceneDelegate.signedOut()
            }
        })
        alert.addAction(UIAlertAction(title: "Cancel", style: .cancel))
        present(alert, animated: true)
    }
}
