import UIKit

@main
final class AppDelegate: UIResponder, UIApplicationDelegate {
    func application(
        _ application: UIApplication,
        didFinishLaunchingWithOptions launchOptions: [UIApplication.LaunchOptionsKey: Any]?
    ) -> Bool {
        true
    }
}

/// The one window: signing in, or your rooms. A call covers it full screen.
final class SceneDelegate: UIResponder, UIWindowSceneDelegate {
    var window: UIWindow?

    func scene(
        _ scene: UIScene,
        willConnectTo session: UISceneSession,
        options connectionOptions: UIScene.ConnectionOptions
    ) {
        guard let scene = scene as? UIWindowScene else { return }
        let window = UIWindow(windowScene: scene)
        self.window = window
        showRoot()
        window.makeKeyAndVisible()

        if let room = connectionOptions.urlContexts.first.flatMap({ Self.room(in: $0.url) }) {
            AppEnv.shared.launchRoom = room
        }
        joinLaunchRoom()
    }

    func scene(_ scene: UIScene, openURLContexts contexts: Set<UIOpenURLContext>) {
        guard let room = contexts.first.flatMap({ Self.room(in: $0.url) }) else { return }
        AppEnv.shared.launchRoom = room
        joinLaunchRoom()
    }

    /// Signing in, or the tabs: whichever the session says.
    func showRoot() {
        let root: UIViewController = AppEnv.shared.client.signedIn() ? MainTabs() : SignInViewController()
        window?.rootViewController = root
    }

    /// The session ended (or you signed out): back to signing in.
    static func signedOut() {
        for case let scene as UIWindowScene in UIApplication.shared.connectedScenes {
            (scene.delegate as? SceneDelegate)?.showRoot()
        }
    }

    /// Signed in: on to the rooms, and into the room a link asked for.
    static func signedIn() {
        for case let scene as UIWindowScene in UIApplication.shared.connectedScenes {
            guard let delegate = scene.delegate as? SceneDelegate else { continue }
            delegate.showRoot()
            delegate.joinLaunchRoom()
        }
    }

    private func joinLaunchRoom() {
        guard AppEnv.shared.client.signedIn(), let room = AppEnv.shared.launchRoom,
              let root = window?.rootViewController, root.presentedViewController == nil
        else { return }
        AppEnv.shared.launchRoom = nil
        CallViewController.present(room: room, name: nil, from: root)
    }

    /// `quipcall://join/<room>` and `quipcall://open/<room>`, as on the Mac.
    private static func room(in url: URL) -> String? {
        guard url.scheme == "quipcall", url.host == "join" || url.host == "open" else { return nil }
        let room = url.lastPathComponent
        let allowed = CharacterSet.alphanumerics.union(CharacterSet(charactersIn: "-_."))
        let valid = (1...64).contains(room.utf8.count)
            && room.unicodeScalars.allSatisfy { $0.isASCII && allowed.contains($0) }
        return valid ? room : nil
    }
}

/// Contacts, Calls, Rooms, Settings: the system's tab bar, in a messenger's
/// order. Rooms is where the app opens.
final class MainTabs: UITabBarController {
    override func viewDidLoad() {
        super.viewDidLoad()
        func tab(_ root: UIViewController, _ title: String, _ symbol: String) -> UIViewController {
            let navigation = UINavigationController(rootViewController: root)
            navigation.navigationBar.prefersLargeTitles = true
            navigation.tabBarItem = UITabBarItem(title: title, image: UIImage(systemName: symbol), selectedImage: nil)
            return navigation
        }
        viewControllers = [
            tab(ContactsViewController(), "Contacts", "person.crop.circle.fill"),
            tab(CallsViewController(), "Calls", "phone.fill"),
            tab(RoomsViewController(), "Rooms", "bubble.left.and.bubble.right.fill"),
            tab(SettingsViewController(), "Settings", "gear"),
        ]
        selectedIndex = 2
    }
}
