import UIKit

/// Signing in, as on the Mac: the app shows a code, the browser (your email's
/// link, then the code) approves this phone, and the app picks up its session.
final class SignInViewController: UIViewController {
    private let logo = UIImageView(image: UIImage(named: "Quby"))
    private let titleLabel = UILabel()
    private let message = UILabel()
    private let code = UILabel()
    private let button = UIButton(configuration: .filled())
    private let cancel = UIButton(configuration: .plain())

    /// The sign-in on its way: polled until the browser approves it.
    private var polling: Task<Void, Never>?

    override func viewDidLoad() {
        super.viewDidLoad()
        view.backgroundColor = .systemBackground

        logo.contentMode = .scaleAspectFit
        titleLabel.text = "Cubie"
        titleLabel.font = .preferredFont(forTextStyle: .largeTitle).bold
        titleLabel.textColor = .label
        message.font = .preferredFont(forTextStyle: .body)
        message.textColor = .secondaryLabel
        message.numberOfLines = 0
        message.textAlignment = .center
        code.font = .monospacedSystemFont(ofSize: 34, weight: .semibold)
        code.textColor = .label
        code.textAlignment = .center

        button.configuration?.cornerStyle = .large
        button.configuration?.buttonSize = .large
        button.addAction(UIAction { [weak self] _ in self?.primary() }, for: .touchUpInside)
        cancel.configuration?.title = "Cancel"
        cancel.addAction(UIAction { [weak self] _ in self?.stop() }, for: .touchUpInside)

        let stack = UIStackView(arrangedSubviews: [logo, titleLabel, message, code, button, cancel])
        stack.axis = .vertical
        stack.alignment = .center
        stack.spacing = 14
        stack.setCustomSpacing(6, after: logo)
        stack.setCustomSpacing(28, after: code)
        stack.translatesAutoresizingMaskIntoConstraints = false
        view.addSubview(stack)
        NSLayoutConstraint.activate([
            logo.widthAnchor.constraint(equalToConstant: 112),
            logo.heightAnchor.constraint(equalToConstant: 112),
            button.widthAnchor.constraint(equalTo: stack.widthAnchor),
            stack.centerYAnchor.constraint(equalTo: view.safeAreaLayoutGuide.centerYAnchor, constant: -24),
            stack.leadingAnchor.constraint(equalTo: view.layoutMarginsGuide.leadingAnchor, constant: 16),
            stack.trailingAnchor.constraint(equalTo: view.layoutMarginsGuide.trailingAnchor, constant: -16),
        ])
        show(.idle)
    }

    private enum Step {
        case idle
        case starting
        /// Waiting for the browser: `url` approves, `code` is typed there.
        case waiting(url: URL, code: String)
        case failed(String)
    }

    private var step = Step.idle

    private func show(_ step: Step) {
        self.step = step
        code.isHidden = true
        cancel.isHidden = true
        button.isEnabled = true
        switch step {
        case .idle:
            message.text = "Small, fast calls.\nSign in with your invited email."
            button.configuration?.title = "Sign in"
        case .starting:
            message.text = "Getting a code…"
            button.configuration?.title = "Sign in"
            button.isEnabled = false
        case let .waiting(_, digits):
            message.text = "Type this code in the browser to approve this iPhone. It's copied: paste it there."
            code.text = digits
            code.isHidden = false
            cancel.isHidden = false
            button.configuration?.title = "Open the browser"
        case let .failed(why):
            message.text = why
            button.configuration?.title = "Try again"
        }
    }

    private func primary() {
        if case let .waiting(url, _) = step {
            UIApplication.shared.open(url)
        } else {
            start()
        }
    }

    private func start() {
        show(.starting)
        polling?.cancel()
        polling = Task { [weak self] in
            let client = AppEnv.shared.client
            do {
                let signIn = try await client.signInStart()
                guard let url = URL(string: signIn.url) else { throw AppError.Failed(message: "The server sent a bad address") }
                guard !Task.isCancelled else { return }
                UIPasteboard.general.string = signIn.code
                self?.show(.waiting(url: url, code: signIn.code))
                await UIApplication.shared.open(url)
                // Every two seconds, for the ten minutes a code lasts. While
                // the app is in the background this sleeps with it.
                let deadline = Date().addingTimeInterval(600)
                // The screen isn't held meanwhile: once it's gone, so is this.
                while Date() < deadline, self != nil {
                    try await Task.sleep(for: .seconds(2))
                    if try await client.signInPoll() != nil {
                        SceneDelegate.signedIn()
                        return
                    }
                }
                self?.show(.failed("That code has expired. Try again."))
            } catch is CancellationError {
            } catch {
                guard !Task.isCancelled else { return }
                self?.show(.failed(error.shown))
            }
        }
    }

    private func stop() {
        polling?.cancel()
        polling = nil
        AppEnv.shared.client.signInCancel()
        show(.idle)
    }

    deinit {
        polling?.cancel()
    }
}
