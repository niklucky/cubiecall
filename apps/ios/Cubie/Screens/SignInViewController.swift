import UIKit

/// Signing in, without leaving the app: your email, then the six digits sent
/// to it (iOS offers them from Mail above the keyboard). A new account says
/// what to be called before the tabs open.
final class SignInViewController: UIViewController, UITextFieldDelegate {
    private enum Step {
        case email
        case code
        case name
    }

    private var step = Step.email
    /// The address the code went to.
    private var email = ""
    /// When: another code can be asked for half a minute later.
    private var sentAt = Date.distantPast
    /// The request on its way. Starting another, or leaving, cancels it.
    private var work: Task<Void, Never>?

    private let picture = UIImageView()
    private let heading = UILabel()
    private let message = UILabel()
    private let field = UITextField()
    private let button = UIButton(configuration: .filled())
    private let problem = UILabel()
    private let again = UIButton(configuration: .plain())
    private let other = UIButton(configuration: .plain())
    private let footnote = UILabel()

    private static let codeLength = 6
    private static let resendAfter: TimeInterval = 30

    override func viewDidLoad() {
        super.viewDidLoad()
        view.backgroundColor = .systemBackground

        picture.contentMode = .scaleAspectFit
        heading.font = .preferredFont(forTextStyle: .title1).bold
        heading.textColor = .label
        heading.numberOfLines = 0
        heading.textAlignment = .center
        heading.adjustsFontForContentSizeCategory = true
        message.font = .preferredFont(forTextStyle: .body)
        message.textColor = .secondaryLabel
        message.numberOfLines = 0
        message.textAlignment = .center
        message.adjustsFontForContentSizeCategory = true
        for label in [problem, footnote] {
            label.font = .preferredFont(forTextStyle: .footnote)
            label.numberOfLines = 0
            label.textAlignment = .center
            label.adjustsFontForContentSizeCategory = true
        }
        problem.textColor = .systemRed
        footnote.textColor = .secondaryLabel

        field.borderStyle = .roundedRect
        field.backgroundColor = .secondarySystemBackground
        field.adjustsFontForContentSizeCategory = true
        field.clearButtonMode = .whileEditing
        field.delegate = self
        field.addAction(UIAction { [weak self] _ in self?.typed() }, for: .editingChanged)

        button.configuration?.cornerStyle = .large
        button.configuration?.buttonSize = .large
        button.addAction(UIAction { [weak self] _ in self?.primary() }, for: .touchUpInside)
        again.configuration?.title = "Send a New Code"
        again.addAction(UIAction { [weak self] _ in self?.send() }, for: .touchUpInside)
        other.configuration?.title = "Use a Different Email"
        other.addAction(UIAction { [weak self] _ in self?.startOver() }, for: .touchUpInside)

        let stack = UIStackView(arrangedSubviews: [picture, heading, message, field, button, problem, again, other, footnote])
        stack.axis = .vertical
        stack.alignment = .center
        stack.spacing = 12
        stack.setCustomSpacing(8, after: heading)
        stack.setCustomSpacing(24, after: message)
        stack.setCustomSpacing(0, after: again)
        stack.translatesAutoresizingMaskIntoConstraints = false
        view.addSubview(stack)

        // What's left of the screen above the keyboard: the form sits in the
        // middle of it, and moves up as the keyboard does.
        let room = UILayoutGuide()
        view.addLayoutGuide(room)
        let centered = stack.centerYAnchor.constraint(equalTo: room.centerYAnchor)
        centered.priority = .defaultHigh
        NSLayoutConstraint.activate([
            room.topAnchor.constraint(equalTo: view.safeAreaLayoutGuide.topAnchor),
            room.bottomAnchor.constraint(equalTo: view.keyboardLayoutGuide.topAnchor),
            centered,
            stack.topAnchor.constraint(greaterThanOrEqualTo: room.topAnchor, constant: 12),
            stack.leadingAnchor.constraint(equalTo: view.layoutMarginsGuide.leadingAnchor, constant: 12),
            stack.trailingAnchor.constraint(equalTo: view.layoutMarginsGuide.trailingAnchor, constant: -12),
            picture.widthAnchor.constraint(equalToConstant: 104),
            picture.heightAnchor.constraint(equalToConstant: 104),
            field.widthAnchor.constraint(equalTo: stack.widthAnchor),
            field.heightAnchor.constraint(greaterThanOrEqualToConstant: 50),
            button.widthAnchor.constraint(equalTo: stack.widthAnchor),
        ])
        show(.email)
    }

    override func viewDidAppear(_ animated: Bool) {
        super.viewDidAppear(animated)
        field.becomeFirstResponder()
    }

    private var entered: String {
        (field.text ?? "").trimmingCharacters(in: .whitespacesAndNewlines)
    }

    private var digits: String {
        String((field.text ?? "").filter(\.isNumber).prefix(Self.codeLength))
    }

    /// Enough of an address to be worth sending: the server has the last word.
    private var plausibleEmail: Bool {
        let parts = entered.split(separator: "@", omittingEmptySubsequences: false)
        return parts.count == 2 && !parts[0].isEmpty && parts[1].contains(".") && !parts[1].hasSuffix(".")
            && !entered.contains(" ")
    }

    private func show(_ step: Step, keep text: String = "") {
        self.step = step
        problem.text = nil
        problem.isHidden = true
        field.text = text
        again.isHidden = step != .code
        other.isHidden = step != .code
        footnote.isHidden = step != .email
        switch step {
        case .email:
            picture.image = UIImage(named: "Quby")
            picture.isHidden = false
            heading.text = "Cubie"
            message.text = "Sign in with your email.\nNo password needed."
            footnote.text = "Cubie is invite-only for now."
            field.placeholder = "Email"
            field.keyboardType = .emailAddress
            field.textContentType = .emailAddress
            field.autocapitalizationType = .none
            field.autocorrectionType = .no
            field.returnKeyType = .continue
            field.textAlignment = .natural
            field.font = .preferredFont(forTextStyle: .body)
        case .code:
            picture.isHidden = true
            heading.text = "Enter the Code"
            message.text = "We emailed a 6-digit code to \(email)."
            field.placeholder = "6-digit code"
            field.keyboardType = .numberPad
            // The system offers the code from Mail or Messages by itself.
            field.textContentType = .oneTimeCode
            field.textAlignment = .center
            field.font = .monospacedDigitSystemFont(ofSize: 28, weight: .semibold)
        case .name:
            picture.isHidden = false
            heading.text = "What should people call you?"
            message.text = "Shown on your face in calls."
            field.placeholder = "Your name"
            field.keyboardType = .default
            field.textContentType = .name
            field.autocapitalizationType = .words
            field.autocorrectionType = .no
            field.returnKeyType = .done
            field.textAlignment = .natural
            field.font = .preferredFont(forTextStyle: .body)
        }
        field.reloadInputViews()
        typed()
        if view.window != nil {
            field.becomeFirstResponder()
        }
    }

    /// What's in the field decides what can be pressed (and, for a code,
    /// presses it: the sixth digit is enough).
    private func typed() {
        let busy = work != nil
        field.isEnabled = !busy
        again.isEnabled = !busy && Date().timeIntervalSince(sentAt) >= Self.resendAfter
        other.isEnabled = !busy
        switch step {
        case .email:
            button.configuration?.title = busy ? "Sending…" : "Continue"
            button.isEnabled = !busy && plausibleEmail
        case .code:
            if field.text != digits {
                field.text = digits
            }
            button.configuration?.title = busy ? "Checking…" : "Continue"
            button.isEnabled = !busy && digits.count == Self.codeLength
            if button.isEnabled {
                primary()
            }
        case .name:
            picture.image = entered.isEmpty
                ? UIImage(systemName: "person.crop.circle.fill")?.withTintColor(.tertiaryLabel, renderingMode: .alwaysOriginal)
                : Avatar.image(entered, side: 104)
            button.configuration?.title = busy ? "Saving…" : "Continue"
            button.isEnabled = !busy && !entered.isEmpty
        }
    }

    private func primary() {
        switch step {
        case .email:
            email = entered
            send()
        case .code:
            let code = digits
            run { client in
                let account = try await client.signInCode(code: code)
                // A new account has no name yet: ask, then go in.
                return account.name.isEmpty ? .name : nil
            }
        case .name:
            let name = entered
            run { client in
                _ = try await client.setName(name: name)
                return nil
            }
        }
    }

    /// Email a code to `email` (again, from the code screen).
    private func send() {
        let email = email
        run { [weak self] client in
            try await client.signInStart(email: email)
            self?.sentAt = Date()
            return .code
        }
    }

    /// One request at a time. It answers with the step to show next, or
    /// nothing once you're in.
    private func run(_ request: @escaping (Client) async throws -> Step?) {
        guard work == nil else { return }
        problem.isHidden = true
        work = Task { [weak self] in
            let result: Result<Step?, Error>
            do {
                result = .success(try await request(AppEnv.shared.client))
            } catch {
                result = .failure(error)
            }
            guard let self, !Task.isCancelled else { return }
            self.work = nil
            switch result {
            case .success(.some(let next)):
                self.show(next)
                if next == .code {
                    // "Send a New Code" wakes up by itself.
                    DispatchQueue.main.asyncAfter(deadline: .now() + Self.resendAfter) { [weak self] in self?.typed() }
                }
            case .success(.none):
                SceneDelegate.signedIn()
            case .failure(let error):
                // A wrong code is typed again from an empty field.
                if self.step == .code {
                    self.field.text = nil
                }
                self.problem.text = error.shown
                self.problem.isHidden = false
                self.typed()
                self.field.becomeFirstResponder()
            }
        }
        typed()
    }

    private func startOver() {
        work?.cancel()
        work = nil
        AppEnv.shared.client.signInCancel()
        show(.email, keep: email)
    }

    func textFieldShouldReturn(_ textField: UITextField) -> Bool {
        if button.isEnabled {
            primary()
        }
        return false
    }

    deinit {
        work?.cancel()
    }
}
