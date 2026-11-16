import AVFoundation
import UIKit

/// A video, drawn by the system: the view's layer is an
/// `AVSampleBufferDisplayLayer`, and the engine puts decoded frames straight
/// into its renderer from its own thread. Nothing here runs per frame.
final class VideoView: UIView {
    override class var layerClass: AnyClass { AVSampleBufferDisplayLayer.self }

    private var displayLayer: AVSampleBufferDisplayLayer { layer as! AVSampleBufferDisplayLayer }

    override init(frame: CGRect) {
        super.init(frame: frame)
        isUserInteractionEnabled = false
        backgroundColor = .black
        // (Not `fill = true`: observers don't run from an initializer.)
        displayLayer.videoGravity = .resizeAspectFill
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError() }

    /// Fill the view, cropping (faces), or fit inside it (a shared screen).
    var fill: Bool = true {
        didSet { displayLayer.videoGravity = fill ? .resizeAspectFill : .resizeAspect }
    }

    /// The renderer's address, for the engine (`Call.attachVideo`). The engine
    /// keeps its own reference while it draws.
    var renderer: UInt64 {
        UInt64(UInt(bitPattern: Unmanaged.passUnretained(displayLayer.sampleBufferRenderer).toOpaque()))
    }

    /// Drop the picture: a camera that's off shouldn't leave its last frame.
    func clear() {
        displayLayer.sampleBufferRenderer.flush(removingDisplayedImage: true, completionHandler: nil)
    }
}
