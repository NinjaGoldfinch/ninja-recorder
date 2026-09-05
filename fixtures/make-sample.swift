// Generates fixtures/sample.mp4 — a small, synthetic, obviously-fake clip
// whose only job is to give the review player something real to decode,
// seek, and frame-step. Uses AVFoundation because this machine has no
// ffmpeg, and a synthetic fixture beats a real game clip anyway: no
// copyright, tiny, and every frame states its own timestamp so a seek can
// be verified by eye.
import AVFoundation
import AppKit

let width = 640, height = 360, fps: Int32 = 30, seconds = 6
let out = URL(fileURLWithPath: CommandLine.arguments[1])
try? FileManager.default.removeItem(at: out)

let writer = try AVAssetWriter(outputURL: out, fileType: .mp4)
// The moov atom up front, so the player can seek without having read the
// whole file — the same thing `-movflags +faststart` does for the real
// recorder's output.
writer.shouldOptimizeForNetworkUse = true

let input = AVAssetWriterInput(mediaType: .video, outputSettings: [
    AVVideoCodecKey: AVVideoCodecType.h264,
    AVVideoWidthKey: width,
    AVVideoHeightKey: height,
    AVVideoCompressionPropertiesKey: [AVVideoAverageBitRateKey: 400_000],
])
input.expectsMediaDataInRealTime = false

let adaptor = AVAssetWriterInputPixelBufferAdaptor(
    assetWriterInput: input,
    sourcePixelBufferAttributes: [
        kCVPixelBufferPixelFormatTypeKey as String: Int(kCVPixelFormatType_32ARGB),
        kCVPixelBufferWidthKey as String: width,
        kCVPixelBufferHeightKey as String: height,
    ])

writer.add(input)
writer.startWriting()
writer.startSession(atSourceTime: .zero)

let space = CGColorSpaceCreateDeviceRGB()
let totalFrames = Int(fps) * seconds

for frame in 0..<totalFrames {
    var pb: CVPixelBuffer?
    CVPixelBufferCreate(kCFAllocatorDefault, width, height,
                        kCVPixelFormatType_32ARGB, nil, &pb)
    guard let buffer = pb else { fatalError("pixel buffer") }

    CVPixelBufferLockBaseAddress(buffer, [])
    let ctx = CGContext(data: CVPixelBufferGetBaseAddress(buffer),
                        width: width, height: height, bitsPerComponent: 8,
                        bytesPerRow: CVPixelBufferGetBytesPerRow(buffer),
                        space: space,
                        bitmapInfo: CGImageAlphaInfo.noneSkipFirst.rawValue)!

    let t = Double(frame) / Double(fps)

    // A distinct background per second, so scrubbing lands somewhere
    // visibly different rather than on an identical grey.
    let hue = CGFloat(Int(t) % seconds) / CGFloat(seconds)
    ctx.setFillColor(NSColor(hue: hue, saturation: 0.55, brightness: 0.30, alpha: 1).cgColor)
    ctx.fill(CGRect(x: 0, y: 0, width: width, height: height))

    // A sweep bar: continuous, so a single frame step is visible even
    // within one second's colour block.
    let x = CGFloat(frame) / CGFloat(totalFrames) * CGFloat(width)
    ctx.setFillColor(NSColor(white: 1, alpha: 0.85).cgColor)
    ctx.fill(CGRect(x: x - 2, y: 0, width: 4, height: CGFloat(height)))

    NSGraphicsContext.saveGraphicsState()
    NSGraphicsContext.current = NSGraphicsContext(cgContext: ctx, flipped: false)

    let big: [NSAttributedString.Key: Any] = [
        .font: NSFont.monospacedDigitSystemFont(ofSize: 84, weight: .bold),
        .foregroundColor: NSColor.white,
    ]
    let small: [NSAttributedString.Key: Any] = [
        .font: NSFont.monospacedSystemFont(ofSize: 20, weight: .regular),
        .foregroundColor: NSColor(white: 1, alpha: 0.75),
    ]
    // Frame number as well as seconds: frame-stepping is a ±1/30s nudge
    // in the player, and without a frame counter there is no way to tell
    // whether it actually moved.
    NSString(format: "%.2fs", t)
        .draw(at: NSPoint(x: 28, y: CGFloat(height) - 130), withAttributes: big)
    NSString(format: "frame %03d / %d", frame, totalFrames)
        .draw(at: NSPoint(x: 30, y: CGFloat(height) - 168), withAttributes: small)
    NSString(string: "ninja-recorder test fixture — synthetic, not gameplay")
        .draw(at: NSPoint(x: 30, y: 24), withAttributes: small)

    NSGraphicsContext.restoreGraphicsState()
    CVPixelBufferUnlockBaseAddress(buffer, [])

    while !input.isReadyForMoreMediaData { usleep(2000) }
    adaptor.append(buffer, withPresentationTime: CMTime(value: CMTimeValue(frame), timescale: fps))
}

input.markAsFinished()
let done = DispatchSemaphore(value: 0)
writer.finishWriting { done.signal() }
done.wait()

if writer.status != .completed {
    FileHandle.standardError.write("write failed: \(String(describing: writer.error))\n".data(using: .utf8)!)
    exit(1)
}
print("wrote \(out.path)")
