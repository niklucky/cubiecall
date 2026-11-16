import Foundation

/// What the whole app shares: the client (your account and rooms, in Rust).
///
/// Launch arguments, for development (`-key value`, read through
/// `UserDefaults`):
/// - `-server <url>` and `-web <url>`: another server than quipcall.com, and
///   its site (they differ only in development).
/// - `-session <token>`: debug builds only, use this session for the run.
/// - `-room <id>`: join this room at launch.
/// - `-mic YES`: let the Simulator open the microphone (it's the Mac's).
/// - `-log <file>`: debug builds only, write the engine's log there (in the
///   Simulator, a path on the Mac).
final class AppEnv {
    static let shared = AppEnv()

    let client: Client
    /// A room to join at launch (`-room`), once.
    var launchRoom: String?

    private init() {
        let defaults = UserDefaults.standard
        let server = defaults.string(forKey: "server") ?? "https://quipcall.com"
        let web = defaults.string(forKey: "web") ?? server

        #if DEBUG
        if let log = defaults.string(forKey: "log") {
            freopen(log, "a", stderr)
        }
        #endif

        #if targetEnvironment(simulator)
        // The Simulator's microphone is the Mac's real one: a test run must
        // not open it by accident. And it has no camera, so calls there send
        // the engine's test pattern.
        if !defaults.bool(forKey: "mic") {
            setenv("QUIPCALL_NO_MIC", "1", 1)
        }
        setenv("QUIPCALL_FAKE_CAMERA", "1", 1)
        #endif

        // Its own list of calls lives in Application Support (backed up with
        // the phone, out of the user's sight).
        let support = FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask).first
            ?? FileManager.default.temporaryDirectory
        client = Client(server: server, web: web, dataDir: support.path)
        #if DEBUG
        if let session = defaults.string(forKey: "session") {
            client.useSession(session: session)
        }
        #endif
        launchRoom = defaults.string(forKey: "room")
    }
}

extension AppError {
    /// The message, as a person should read it.
    var text: String {
        switch self {
        case let .SignedOut(message), let .Failed(message): return message
        }
    }
}

extension Error {
    /// What to show for any error: ours say it themselves.
    var shown: String {
        (self as? AppError)?.text ?? localizedDescription
    }
}
