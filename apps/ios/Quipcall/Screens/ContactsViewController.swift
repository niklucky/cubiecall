import Contacts
import MessageUI
import UIKit

/// Someone in the phone's address book.
private struct Contact {
    /// The address book's identifier: what their room is filed under.
    let id: String
    let name: String
    let emails: [String]
    let phones: [String]
    let photo: Data?
    /// They have a Quipcall account (one of their emails does).
    var here = false
}

/// The phone's own contacts, searchable, with the people who already use
/// Quipcall on top. Tap someone who does to call them; tap anyone else to
/// text them an invitation.
///
/// Each contact gets a room of their own (a call for two, kept), and its link
/// is the invitation: it opens in a browser, no app or account needed.
/// Ringing comes with the server's presence (PLAN.md, M4); until then a call
/// starts by sending that link.
final class ContactsViewController: ListViewController, UISearchResultsUpdating,
    MFMessageComposeViewControllerDelegate
{
    private let store = CNContactStore()
    private var people: [Contact] = []
    private var sections: [(title: String, people: [Contact])] = []
    private var loading: Task<Void, Never>?
    /// What to do once a text has gone out (join the call).
    private var afterMessage: (() -> Void)?

    init() {
        super.init(style: .plain)
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError() }

    override func viewDidLoad() {
        super.viewDidLoad()
        title = "Contacts"
        tableView.register(UITableViewCell.self, forCellReuseIdentifier: "contact")
        let search = UISearchController(searchResultsController: nil)
        search.searchResultsUpdater = self
        search.obscuresBackgroundDuringPresentation = false
        navigationItem.searchController = search
        navigationItem.hidesSearchBarWhenScrolling = false
        // The address book changed (in Contacts, or by a sync): read it again.
        NotificationCenter.default.addObserver(
            self, selector: #selector(load), name: .CNContactStoreDidChange, object: nil
        )
        load()
    }

    // MARK: Reading the address book

    private var access: CNAuthorizationStatus {
        CNContactStore.authorizationStatus(for: .contacts)
    }

    @objc private func load() {
        guard access != .notDetermined, access != .denied, access != .restricted else {
            setNeedsUpdateContentUnavailableConfiguration()
            return
        }
        loading?.cancel()
        loading = Task { [weak self, store] in
            // Off the main thread: a big address book takes a moment.
            let people = await Task.detached(priority: .userInitiated) { Self.read(store) }.value
            guard let self, !Task.isCancelled else { return }
            self.people = people
            self.arrange()
            await self.findWhoIsHere()
        }
    }

    private nonisolated static func read(_ store: CNContactStore) -> [Contact] {
        let keys: [CNKeyDescriptor] = [
            CNContactFormatter.descriptorForRequiredKeys(for: .fullName),
            CNContactOrganizationNameKey as CNKeyDescriptor,
            CNContactEmailAddressesKey as CNKeyDescriptor,
            CNContactPhoneNumbersKey as CNKeyDescriptor,
            CNContactThumbnailImageDataKey as CNKeyDescriptor,
        ]
        let request = CNContactFetchRequest(keysToFetch: keys)
        request.sortOrder = .userDefault
        var people: [Contact] = []
        try? store.enumerateContacts(with: request) { contact, _ in
            let emails = contact.emailAddresses.map { $0.value as String }
            let phones = contact.phoneNumbers.map(\.value.stringValue)
            let name = CNContactFormatter.string(from: contact, style: .fullName)
                ?? [contact.organizationName, emails.first, phones.first].compactMap { $0 }.first { !$0.isEmpty }
            guard let name, !emails.isEmpty || !phones.isEmpty else { return }
            people.append(Contact(
                id: contact.identifier, name: name, emails: emails, phones: phones,
                photo: contact.thumbnailImageData
            ))
        }
        return people
    }

    /// Ask the server which of them have an account. Only a scramble
    /// (SHA-256) of each address leaves the phone. A server that can't say
    /// (an older one) leaves everyone as "invite".
    private func findWhoIsHere() async {
        let emails = people.flatMap(\.emails)
        guard !emails.isEmpty else { return }
        do {
            let matches = try await AppEnv.shared.client.matchContacts(emails: emails)
            let here = Set(matches.map { $0.email.lowercased() })
            guard !Task.isCancelled else { return }
            for index in people.indices {
                people[index].here = people[index].emails.contains { here.contains($0.lowercased()) }
            }
            arrange()
        } catch AppError.SignedOut {
            SceneDelegate.signedOut()
        } catch {
            // Not worth an alert: the list works without it.
        }
    }

    // MARK: Sections and search

    func updateSearchResults(for searchController: UISearchController) {
        arrange()
    }

    private var query: String {
        navigationItem.searchController?.searchBar.text ?? ""
    }

    /// People on Quipcall first, then everyone else under their letter.
    private func arrange() {
        let query = query
        let digits = query.filter(\.isNumber)
        let found = query.isEmpty ? people : people.filter { person in
            person.name.localizedCaseInsensitiveContains(query)
                || person.emails.contains { $0.localizedCaseInsensitiveContains(query) }
                || (!digits.isEmpty && person.phones.contains { $0.filter(\.isNumber).contains(digits) })
        }
        var sections: [(title: String, people: [Contact])] = []
        let here = found.filter(\.here)
        if !here.isEmpty {
            sections.append(("On Quipcall", here))
        }
        let others = found.filter { !$0.here }
            .sorted { $0.name.localizedCaseInsensitiveCompare($1.name) == .orderedAscending }
        for person in others {
            let first = person.name.first.map { String($0).uppercased() } ?? "#"
            let letter = first.first?.isLetter == true ? first : "#"
            if sections.last?.title == letter {
                sections[sections.count - 1].people.append(person)
            } else {
                sections.append((letter, [person]))
            }
        }
        self.sections = sections
        tableView.reloadData()
        setNeedsUpdateContentUnavailableConfiguration()
    }

    override func updateContentUnavailableConfiguration(using state: UIContentUnavailableConfigurationState) {
        var empty = UIContentUnavailableConfiguration.empty()
        empty.image = UIImage(systemName: "person.crop.circle")
        switch access {
        case .notDetermined:
            empty.text = "Find People You Know"
            empty.secondaryText = "See which of your contacts use Quipcall, and invite the rest."
            empty.button = .borderedProminent()
            empty.button.title = "Continue"
            empty.buttonProperties.primaryAction = UIAction { [weak self] _ in
                self?.store.requestAccess(for: .contacts) { _, _ in
                    DispatchQueue.main.async { self?.load() }
                }
            }
        case .denied, .restricted:
            empty.text = "No Access to Contacts"
            empty.secondaryText = "Allow Quipcall to see your contacts in Settings."
            empty.button = .bordered()
            empty.button.title = "Open Settings"
            empty.buttonProperties.primaryAction = UIAction { _ in
                if let url = URL(string: UIApplication.openSettingsURLString) {
                    UIApplication.shared.open(url)
                }
            }
        default:
            guard sections.isEmpty else {
                showEmpty(nil)
                return
            }
            if !query.isEmpty {
                showEmpty(UIContentUnavailableConfiguration.search())
                return
            }
            empty.text = "No Contacts"
            empty.secondaryText = "People in the Contacts app show up here."
        }
        showEmpty(empty)
    }

    // MARK: The list

    override func numberOfSections(in tableView: UITableView) -> Int {
        sections.count
    }

    override func tableView(_ tableView: UITableView, numberOfRowsInSection section: Int) -> Int {
        sections[section].people.count
    }

    override func tableView(_ tableView: UITableView, titleForHeaderInSection section: Int) -> String? {
        sections[section].title
    }

    override func sectionIndexTitles(for tableView: UITableView) -> [String]? {
        // A–Z down the side, as in Contacts; not while searching.
        query.isEmpty && people.count > 20 ? sections.map(\.title).filter { $0.count == 1 } : nil
    }

    override func tableView(_ tableView: UITableView, sectionForSectionIndexTitle title: String, at index: Int) -> Int {
        sections.firstIndex { $0.title == title } ?? 0
    }

    override func tableView(_ tableView: UITableView, cellForRowAt indexPath: IndexPath) -> UITableViewCell {
        let cell = tableView.dequeueReusableCell(withIdentifier: "contact", for: indexPath)
        let person = sections[indexPath.section].people[indexPath.row]
        var content = UIListContentConfiguration.face(
            person.name,
            detail: person.here ? "On Quipcall" : person.phones.first ?? person.emails.first,
            photo: person.photo
        )
        if person.here {
            content.secondaryTextProperties.color = .tintColor
        }
        cell.contentConfiguration = content

        // What a tap does, said by a small button on the right.
        var button = person.here ? UIButton.Configuration.tinted() : UIButton.Configuration.gray()
        button.buttonSize = .small
        button.cornerStyle = .capsule
        if person.here {
            button.image = UIImage(systemName: "video.fill")
        } else {
            button.title = "Invite"
        }
        let action = UIButton(configuration: button, primaryAction: UIAction { [weak self, weak cell] _ in
            self?.act(on: person, from: cell)
        })
        action.accessibilityLabel = person.here ? "Call \(person.name)" : "Invite \(person.name)"
        action.sizeToFit()
        cell.accessoryView = action
        return cell
    }

    override func tableView(_ tableView: UITableView, didSelectRowAt indexPath: IndexPath) {
        tableView.deselectRow(at: indexPath, animated: true)
        act(on: sections[indexPath.section].people[indexPath.row], from: tableView.cellForRow(at: indexPath))
    }

    override func tableView(
        _ tableView: UITableView,
        contextMenuConfigurationForRowAt indexPath: IndexPath,
        point: CGPoint
    ) -> UIContextMenuConfiguration? {
        let person = sections[indexPath.section].people[indexPath.row]
        return UIContextMenuConfiguration(actionProvider: { [weak self] _ in
            UIMenu(children: [
                UIAction(title: "Call", image: UIImage(systemName: "video")) { _ in
                    self?.room(of: person) { room, _ in
                        guard let self else { return }
                        CallViewController.present(room: room.id, name: person.name, from: self)
                    }
                },
                UIAction(title: "Send Invitation", image: UIImage(systemName: "message")) { _ in
                    self?.invite(person, thenCall: false, from: tableView.cellForRow(at: indexPath))
                },
                UIAction(title: "Copy Link", image: UIImage(systemName: "link")) { _ in
                    self?.room(of: person) { _, link in UIPasteboard.general.string = link }
                },
            ])
        })
    }

    // MARK: Calling and inviting

    /// Someone on Quipcall: call them. Anyone else: text them an invitation.
    private func act(on person: Contact, from source: UIView?) {
        invite(person, thenCall: person.here, from: source)
    }

    /// Their room and its link, made the first time.
    private func room(of person: Contact, then: @escaping (Room, String) -> Void) {
        Task { [weak self] in
            do {
                let client = AppEnv.shared.client
                let room = try await client.contactRoom(contact: person.id, name: person.name)
                then(room, client.roomLink(room: room.id))
            } catch AppError.SignedOut {
                SceneDelegate.signedOut()
            } catch {
                self?.say(error.shown)
            }
        }
    }

    /// Text `person` the link to their room: in Messages when the phone can
    /// (and they have a number), through the share sheet otherwise. With
    /// `thenCall`, go into the call once it's sent.
    private func invite(_ person: Contact, thenCall: Bool, from source: UIView?) {
        room(of: person) { [weak self] room, link in
            guard let self else { return }
            let text = thenCall
                ? "I'm calling you on Quipcall. Join me here: \(link)"
                : "Let's talk on Quipcall. Open this to call me, no app needed: \(link)"
            let call: (() -> Void)? = thenCall
                ? { [weak self] in
                    guard let self else { return }
                    CallViewController.present(room: room.id, name: person.name, from: self)
                }
                : nil
            if MFMessageComposeViewController.canSendText(), let phone = person.phones.first {
                let message = MFMessageComposeViewController()
                message.recipients = [phone]
                message.body = text
                message.messageComposeDelegate = self
                self.afterMessage = call
                self.present(message, animated: true)
            } else {
                let sheet = UIActivityViewController(activityItems: [text], applicationActivities: nil)
                sheet.popoverPresentationController?.sourceView = source ?? self.view
                sheet.completionWithItemsHandler = { _, sent, _, _ in
                    if sent { call?() }
                }
                self.present(sheet, animated: true)
            }
        }
    }

    func messageComposeViewController(
        _ controller: MFMessageComposeViewController,
        didFinishWith result: MessageComposeResult
    ) {
        let then = result == .sent ? afterMessage : nil
        afterMessage = nil
        controller.dismiss(animated: true) { then?() }
    }
}
