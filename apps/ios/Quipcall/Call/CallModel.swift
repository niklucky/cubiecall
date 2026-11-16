import AVFoundation
import UIKit

/// Someone else in the call.
struct Person {
    let id: String
    var name: String
    var muted = true
    var speaking = false
    /// Their camera's track, while it's on.
    var camera: String?
    /// Their shared screen's track, while they share.
    var screen: String?
}

/// The call as the screen shows it, kept from the engine's events. Lives on
/// the main thread; the engine (Rust) does everything else.
final class CallModel {
    enum Change {
        /// Someone came or went, or a camera or screen did: lay out again.
        case people
        /// Only rings and badges: nothing moves.
        case speaking
        case controls
        case status
        /// Over. A message if it ended by itself; `signedOut` if the session did.
        case ended(String?, signedOut: Bool)
    }

    let room: String
    private(set) var roomName: String
    private(set) var call: Call?
    private(set) var you: String?
    private(set) var people: [Person] = []
    private(set) var youSpeak = false
    private(set) var micOn = false
    private(set) var cameraOn = false
    private(set) var frontCamera = true
    private(set) var speakerOn = true
    /// "Joining…", a permission prompt, a notice: shown under the name.
    private(set) var status: String? = "Joining…"
    private(set) var connectedAt: Date?
    /// Who records the call, if anyone.
    private(set) var recording: Set<String> = []

    var onChange: ((Change) -> Void)?

    init(room: String, name: String?) {
        self.room = room
        roomName = name ?? "Call"
    }

    func start() {
        Audio.prepare()
        do {
            call = try AppEnv.shared.client.join(room: room, listener: Relay(self))
        } catch {
            onChange?(.ended(error.shown, signedOut: false))
        }
    }

    // MARK: What you do

    func toggleMic() {
        micOn.toggle()
        call?.setMic(on: micOn)
        onChange?(.controls)
    }

    func toggleCamera() {
        call?.setCamera(on: !cameraOn)
    }

    func flipCamera() {
        frontCamera.toggle()
        call?.flipCamera(running: cameraOn)
        onChange?(.controls)
    }

    func toggleSpeaker() {
        speakerOn.toggle()
        Audio.setSpeaker(speakerOn)
        onChange?(.controls)
    }

    func leave() {
        call?.leave()
        call = nil
        Audio.finish()
    }

    func name(of id: String) -> String {
        people.first { $0.id == id }?.name ?? "Someone"
    }

    // MARK: What happens

    fileprivate func handle(_ event: CallEvent) {
        // Events of a call you've left are nobody's business.
        guard call != nil else { return }
        switch event {
        case let .waitingForPermission(camera):
            status = camera ? "Waiting for camera access…" : "Waiting for microphone access…"
            onChange?(.status)
        case let .connected(you, roomName):
            self.you = you
            if let roomName, !roomName.isEmpty { self.roomName = roomName }
            connectedAt = Date()
            status = nil
            Audio.setSpeaker(speakerOn)
            onChange?(.status)
        case let .joined(id, name):
            guard !people.contains(where: { $0.id == id }) else { return }
            people.append(Person(id: id, name: name.isEmpty ? "Guest" : name))
            onChange?(.people)
        case let .left(id):
            people.removeAll { $0.id == id }
            recording.remove(id)
            onChange?(.people)
        case let .speaking(ids):
            let speaking = Set(ids)
            youSpeak = you.map(speaking.contains) ?? false
            for index in people.indices {
                people[index].speaking = speaking.contains(people[index].id)
            }
            onChange?(.speaking)
        case let .mic(id, muted):
            update(id) { $0.muted = muted }
            onChange?(.speaking)
        case let .videoAdded(track, id, screen):
            update(id) { person in
                if screen { person.screen = track } else { person.camera = track }
            }
            onChange?(.people)
        case let .videoRemoved(track):
            for index in people.indices {
                if people[index].camera == track { people[index].camera = nil }
                if people[index].screen == track { people[index].screen = nil }
            }
            onChange?(.people)
        case let .yourMic(on):
            micOn = on
            onChange?(.controls)
        case let .yourCamera(on):
            cameraOn = on
            onChange?(.people)
            onChange?(.controls)
        case let .recording(id, on, _):
            if on { recording.insert(id) } else { recording.remove(id) }
            onChange?(.status)
        case .transcribing, .line:
            // Transcripts come to the phone already; showing them is next.
            break
        case let .notice(message):
            status = message
            onChange?(.status)
        case let .failed(message, signedOut):
            finish(message, signedOut: signedOut)
        case .ended:
            finish("The call ended.", signedOut: false)
        }
    }

    private func update(_ id: String, _ change: (inout Person) -> Void) {
        guard let index = people.firstIndex(where: { $0.id == id }) else { return }
        change(&people[index])
    }

    private func finish(_ message: String?, signedOut: Bool) {
        call?.leave()
        call = nil
        Audio.finish()
        onChange?(.ended(message, signedOut: signedOut))
    }
}

/// The engine calls this on its own thread, and keeps it for as long as the
/// call lives: so it holds the model weakly, and hops to the main thread.
private final class Relay: CallListener, @unchecked Sendable {
    private weak var model: CallModel?

    init(_ model: CallModel) {
        self.model = model
    }

    func onEvent(event: CallEvent) {
        DispatchQueue.main.async { [weak model] in
            model?.handle(event)
        }
    }
}

/// The phone's audio, around a call. The engine's voice processing (echo
/// cancellation, noise suppression, gain) is the system's.
enum Audio {
    static func prepare() {
        let session = AVAudioSession.sharedInstance()
        try? session.setCategory(
            .playAndRecord,
            mode: .videoChat,
            options: [.allowBluetoothHFP, .defaultToSpeaker]
        )
        UIApplication.shared.isIdleTimerDisabled = true
    }

    /// The loudspeaker, or the receiver (or whatever headset is connected).
    static func setSpeaker(_ on: Bool) {
        try? AVAudioSession.sharedInstance().overrideOutputAudioPort(on ? .speaker : .none)
    }

    static func finish() {
        UIApplication.shared.isIdleTimerDisabled = false
    }
}
