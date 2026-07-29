// Screen and meeting recording, with system audio.
//
// The reason this exists: macOS's own recorder (⇧⌘5) cannot capture what your
// Mac is *playing*. It records the screen and the microphone, so a recording of
// a call is you talking to silence. Every app that does capture system audio
// used to ship a kernel extension or a virtual audio device for it.
//
// ScreenCaptureKit removed that requirement in macOS 13: a capture stream can
// include the system audio mix directly, with `excludesCurrentProcessAudio` so
// Caduceus's own sounds stay out of the recording. That is the whole trick, and
// it is why this is a separate Swift helper — there is no C interface for any
// of it that Rust could reach.
//
// Modes:
//   caduceus-record screen <out.mp4> [--mic] [--display N]
//       Video, system audio, optionally the microphone as a second track.
//   caduceus-record audio  <out.m4a> [--mic]
//       System audio only (plus microphone). For meetings, where the video is
//       forty minutes of somebody's slides and nobody wants the gigabyte.
//
// Protocol (stdout, tab separated):
//   ready                 capture has started
//   level <0..1>          rough input level, a few times a second, for a meter
//   partial <text>         rolling Parakeet transcript of system audio
//   transcription <text>  local-model preparation status
//   error <message>
//   done <path>
//
// Stdin: `pause`, `resume`, `stop`.
//
// Exit codes: 0 success, 2 failure, 3 nothing was captured.

import AVFoundation
import CoreMedia
import Foundation
import ScreenCaptureKit

// ---------------------------------------------------------------------------
// Protocol
// ---------------------------------------------------------------------------

let out = FileHandle.standardOutput
let emitLock = NSLock()

func emit(_ kind: String, _ payload: String = "") {
    emitLock.lock()
    defer { emitLock.unlock() }
    let line = payload.isEmpty ? "\(kind)\n" : "\(kind)\t\(payload)\n"
    out.write(Data(line.utf8))
}

func fail(_ message: String) -> Never {
    emit("error", message)
    exit(2)
}

// ---------------------------------------------------------------------------
// Arguments
// ---------------------------------------------------------------------------

let args = CommandLine.arguments
guard args.count >= 3 else {
    fail("usage: caduceus-record <screen|audio> <output-path> [--mic] [--display N]")
}

let mode = args[1]
let outputPath = args[2]
let wantsMic = args.contains("--mic")
let displayIndex: Int = {
    guard let at = args.firstIndex(of: "--display"), at + 1 < args.count else { return 0 }
    return Int(args[at + 1]) ?? 0
}()

guard mode == "screen" || mode == "audio" else {
    fail("mode must be 'screen' or 'audio', not '\(mode)'")
}

guard #available(macOS 13.0, *) else {
    fail(
        "Recording system audio needs macOS 13 or newer. On this version, macOS itself has no "
            + "way to capture what your Mac is playing without installing an audio driver, which "
            + "Caduceus will not do behind your back."
    )
}

// ---------------------------------------------------------------------------
// Writer
// ---------------------------------------------------------------------------

/// Wraps AVAssetWriter so the capture callbacks stay short.
///
/// Everything is serialised onto one queue. The screen and audio callbacks
/// arrive on different queues and `AVAssetWriterInput` is not safe to append to
/// from several at once — the failure mode is a corrupt file discovered at the
/// end of a forty-minute meeting.
@available(macOS 13.0, *)
final class Writer {
    private let writer: AVAssetWriter
    private var video: AVAssetWriterInput?
    private var systemAudio: AVAssetWriterInput?
    private var micAudio: AVAssetWriterInput?
    private let queue = DispatchQueue(label: "com.caduceus.record.writer")
    private var started = false
    private var samples = 0

    init(url: URL, video videoSize: CGSize?, micTrack: Bool) throws {
        try? FileManager.default.removeItem(at: url)
        writer = try AVAssetWriter(url: url, fileType: videoSize == nil ? .m4a : .mp4)

        if let size = videoSize {
            let input = AVAssetWriterInput(
                mediaType: .video,
                outputSettings: [
                    AVVideoCodecKey: AVVideoCodecType.h264,
                    AVVideoWidthKey: Int(size.width),
                    AVVideoHeightKey: Int(size.height),
                    AVVideoCompressionPropertiesKey: [
                        // ~8 Mbps at 1080p: sharp enough to read code in, small
                        // enough that an hour is not 20 GB.
                        AVVideoAverageBitRateKey: 8_000_000,
                        AVVideoMaxKeyFrameIntervalKey: 60,
                    ],
                ]
            )
            input.expectsMediaDataInRealTime = true
            if writer.canAdd(input) { writer.add(input) }
            video = input
        }

        let audioSettings: [String: Any] = [
            AVFormatIDKey: kAudioFormatMPEG4AAC,
            AVSampleRateKey: 48_000,
            AVNumberOfChannelsKey: 2,
            AVEncoderBitRateKey: 128_000,
        ]

        let system = AVAssetWriterInput(mediaType: .audio, outputSettings: audioSettings)
        system.expectsMediaDataInRealTime = true
        if writer.canAdd(system) { writer.add(system) }
        systemAudio = system

        if micTrack {
            // A separate track, not mixed. Keeping "them" and "you" apart is
            // what lets a transcript attribute lines to a speaker later, and
            // mixing is something you can always do afterwards — unmixing is not.
            let mic = AVAssetWriterInput(mediaType: .audio, outputSettings: audioSettings)
            mic.expectsMediaDataInRealTime = true
            if writer.canAdd(mic) { writer.add(mic) }
            micAudio = mic
        }
    }

    enum Track { case video, system, mic }

    func append(_ buffer: CMSampleBuffer, to track: Track) {
        queue.async { [self] in
            guard CMSampleBufferDataIsReady(buffer) else { return }

            if !started {
                guard writer.startWriting() else { return }
                writer.startSession(atSourceTime: CMSampleBufferGetPresentationTimeStamp(buffer))
                started = true
            }

            let input: AVAssetWriterInput? =
                switch track {
                case .video: video
                case .system: systemAudio
                case .mic: micAudio
                }

            guard let input, input.isReadyForMoreMediaData else { return }
            if input.append(buffer) { samples += 1 }
        }
    }

    /// Close the file. Returns false when nothing was ever written.
    func finish() -> Bool {
        var wrote = false
        let done = DispatchSemaphore(value: 0)
        queue.async { [self] in
            guard started, samples > 0 else {
                done.signal()
                return
            }
            wrote = true
            video?.markAsFinished()
            systemAudio?.markAsFinished()
            micAudio?.markAsFinished()
            writer.finishWriting { done.signal() }
        }
        _ = done.wait(timeout: .now() + 20)
        return wrote
    }
}

// ---------------------------------------------------------------------------
// Live system-audio transcription
// ---------------------------------------------------------------------------

/// Feeds the meeting's ScreenCaptureKit audio into the sibling Parakeet helper.
/// The helper owns CoreML inference; this recorder stays responsive and keeps
/// writing the durable audio file even if model preparation or preview fails.
@available(macOS 13.0, *)
final class SystemAudioTranscriber {
    private let process: Process
    private let input: FileHandle
    private let queue = DispatchQueue(label: "com.caduceus.record.live-transcript")

    private init(process: Process, input: FileHandle, output: Pipe) {
        self.process = process
        self.input = input
        DispatchQueue.global(qos: .utility).async {
            var pending = ""
            while true {
                let data = output.fileHandleForReading.availableData
                guard !data.isEmpty else { break }
                pending += String(decoding: data, as: UTF8.self)
                while let newline = pending.firstIndex(of: "\n") {
                    let line = String(pending[..<newline])
                    pending.removeSubrange(...newline)
                    let fields = line.split(separator: "\t", maxSplits: 1).map(String.init)
                    guard fields.count == 2 else { continue }
                    switch fields[0] {
                    case "partial": emit("partial", fields[1])
                    case "preparing": emit("transcription", fields[1])
                    case "error": emit("transcription-error", fields[1])
                    default: break
                    }
                }
            }
        }
    }

    static func startIfAvailable() -> SystemAudioTranscriber? {
        guard ProcessInfo.processInfo.operatingSystemVersion.majorVersion >= 14 else { return nil }
        let executable = URL(fileURLWithPath: CommandLine.arguments[0])
            .deletingLastPathComponent()
            .appendingPathComponent("caduceus-parakeet-live")
        guard FileManager.default.isExecutableFile(atPath: executable.path) else { return nil }

        let process = Process()
        let input = Pipe()
        let output = Pipe()
        process.executableURL = executable
        process.arguments = ["--stdin-pcm16"]
        process.standardInput = input
        process.standardOutput = output
        process.standardError = FileHandle.nullDevice
        do {
            try process.run()
            return SystemAudioTranscriber(
                process: process, input: input.fileHandleForWriting, output: output)
        } catch {
            emit("transcription-error", "Could not start live call transcription: \(error.localizedDescription)")
            return nil
        }
    }

    func append(_ sampleBuffer: CMSampleBuffer) {
        guard let pcm = Self.pcmBuffer(from: sampleBuffer),
              let samples = Self.samples16k(from: pcm), !samples.isEmpty else { return }
        queue.async { [input] in
            var words = samples.map {
                Int16(max(-1, min(1, $0)) * Float(Int16.max)).littleEndian
            }
            let data = words.withUnsafeMutableBytes { Data($0) }
            try? input.write(contentsOf: data)
        }
    }

    func stop() {
        queue.sync { try? input.close() }
        if process.isRunning { process.terminate() }
    }

    private static func pcmBuffer(from sampleBuffer: CMSampleBuffer) -> AVAudioPCMBuffer? {
        let count = CMSampleBufferGetNumSamples(sampleBuffer)
        guard count > 0,
              let description = CMSampleBufferGetFormatDescription(sampleBuffer),
              let stream = CMAudioFormatDescriptionGetStreamBasicDescription(description),
              let format = AVAudioFormat(streamDescription: stream),
              let buffer = AVAudioPCMBuffer(
                pcmFormat: format, frameCapacity: AVAudioFrameCount(count)) else { return nil }
        buffer.frameLength = AVAudioFrameCount(count)
        return CMSampleBufferCopyPCMDataIntoAudioBufferList(
            sampleBuffer, at: 0, frameCount: Int32(count), into: buffer.mutableAudioBufferList
        ) == noErr ? buffer : nil
    }

    private static func samples16k(from buffer: AVAudioPCMBuffer) -> [Float]? {
        let frames = Int(buffer.frameLength)
        let channels = Int(buffer.format.channelCount)
        guard frames > 0, channels > 0 else { return nil }
        var mono = [Float](repeating: 0, count: frames)
        if let values = buffer.floatChannelData {
            for channel in 0..<channels {
                for frame in 0..<frames { mono[frame] += values[channel][frame] }
            }
        } else if let values = buffer.int16ChannelData {
            for channel in 0..<channels {
                for frame in 0..<frames {
                    mono[frame] += Float(values[channel][frame]) / Float(Int16.max)
                }
            }
        } else if let values = buffer.int32ChannelData {
            for channel in 0..<channels {
                for frame in 0..<frames {
                    mono[frame] += Float(values[channel][frame]) / Float(Int32.max)
                }
            }
        } else {
            return nil
        }
        if channels > 1 {
            let scale = 1 / Float(channels)
            for index in mono.indices { mono[index] *= scale }
        }

        let sourceRate = Int(buffer.format.sampleRate.rounded())
        guard sourceRate != 16_000 else { return mono }
        let ratio = Double(sourceRate) / 16_000
        let outputCount = Int(Double(mono.count) / ratio)
        return (0..<outputCount).map { index in
            let source = Double(index) * ratio
            let lower = Int(source)
            let fraction = Float(source - Double(lower))
            let a = mono[min(lower, mono.count - 1)]
            let b = mono[min(lower + 1, mono.count - 1)]
            return a + (b - a) * fraction
        }
    }
}

// ---------------------------------------------------------------------------
// Capture
// ---------------------------------------------------------------------------

@available(macOS 13.0, *)
final class Recorder: NSObject, SCStreamOutput, SCStreamDelegate {
    private var stream: SCStream?
    private var writer: Writer?
    private var engine: AVAudioEngine?
    private var systemTranscriber: SystemAudioTranscriber?
    private let paused = NSLock()
    private var isPaused = false
    private let stopping = NSLock()
    private var isStopping = false
    private var lastLevelEmit = Date.distantPast

    func start() async throws {
        // `excludingDesktopWindows: false` keeps the wallpaper, which is what
        // people expect a screen recording to look like.
        let content = try await SCShareableContent.excludingDesktopWindows(
            false, onScreenWindowsOnly: true)

        guard !content.displays.isEmpty else {
            fail("No displays are available to record.")
        }
        let display = content.displays[min(displayIndex, content.displays.count - 1)]

        let configuration = SCStreamConfiguration()
        configuration.capturesAudio = true
        // Without this, Caduceus's own interface sounds end up in the recording
        // — including the click that started it.
        configuration.excludesCurrentProcessAudio = true
        configuration.sampleRate = 48_000
        configuration.channelCount = 2

        if mode == "screen" {
            configuration.width = display.width * 2
            configuration.height = display.height * 2
            configuration.minimumFrameInterval = CMTime(value: 1, timescale: 30)
            configuration.showsCursor = true
            configuration.queueDepth = 6
        } else {
            // Audio-only still needs a stream, and a stream needs a size. The
            // smallest legal one, so no time is spent encoding frames that are
            // thrown away.
            configuration.width = 2
            configuration.height = 2
            configuration.minimumFrameInterval = CMTime(value: 1, timescale: 1)
        }

        let filter = SCContentFilter(display: display, excludingWindows: [])

        writer = try Writer(
            url: URL(fileURLWithPath: outputPath),
            video: mode == "screen"
                ? CGSize(width: configuration.width, height: configuration.height) : nil,
            micTrack: wantsMic
        )

        let stream = SCStream(filter: filter, configuration: configuration, delegate: self)
        if mode == "screen" {
            try stream.addStreamOutput(
                self, type: .screen, sampleHandlerQueue: DispatchQueue(label: "cad.screen"))
        }
        try stream.addStreamOutput(
            self, type: .audio, sampleHandlerQueue: DispatchQueue(label: "cad.audio"))

        try await stream.startCapture()
        self.stream = stream

        if mode == "audio" {
            systemTranscriber = SystemAudioTranscriber.startIfAvailable()
        }

        if wantsMic { startMicrophone() }
        emit("ready")
    }

    /// The microphone, captured separately and written to its own track.
    private func startMicrophone() {
        let engine = AVAudioEngine()
        let input = engine.inputNode
        let format = input.outputFormat(forBus: 0)

        input.installTap(onBus: 0, bufferSize: 2048, format: format) { [weak self] buffer, when in
            guard let self, !self.held() else { return }
            if let sample = Self.sampleBuffer(from: buffer, at: when) {
                self.writer?.append(sample, to: .mic)
            }
            self.reportLevel(buffer)
        }

        do {
            try engine.start()
            self.engine = engine
        } catch {
            // Not fatal: a recording of the call without your side is far more
            // useful than no recording at all.
            emit("error", "The microphone could not be opened: \(error.localizedDescription)")
        }
    }

    /// A rough peak level, a few times a second, so the UI has a meter.
    private func reportLevel(_ buffer: AVAudioPCMBuffer) {
        guard Date().timeIntervalSince(lastLevelEmit) > 0.1 else { return }
        lastLevelEmit = Date()
        guard let channel = buffer.floatChannelData?[0] else { return }
        var peak: Float = 0
        for i in 0..<Int(buffer.frameLength) {
            peak = max(peak, abs(channel[i]))
        }
        emit("level", String(format: "%.3f", min(1, peak)))
    }

    private func held() -> Bool {
        paused.lock()
        defer { paused.unlock() }
        return isPaused
    }

    func setPaused(_ value: Bool) {
        paused.lock()
        isPaused = value
        paused.unlock()
        emit(value ? "paused" : "resumed")
    }

    // --- SCStreamOutput ---------------------------------------------------

    func stream(
        _ stream: SCStream, didOutputSampleBuffer buffer: CMSampleBuffer, of type: SCStreamOutputType
    ) {
        guard !held() else { return }
        switch type {
        case .screen:
            // Frames arrive even when nothing has changed; the ones marked
            // incomplete have no image attached and would corrupt the track.
            guard
                let attachments = CMSampleBufferGetSampleAttachmentsArray(
                    buffer, createIfNecessary: false) as? [[SCStreamFrameInfo: Any]],
                let raw = attachments.first?[.status] as? Int,
                SCFrameStatus(rawValue: raw) == .complete
            else { return }
            writer?.append(buffer, to: .video)
        case .audio:
            writer?.append(buffer, to: .system)
            systemTranscriber?.append(buffer)
        default:
            break
        }
    }

    func stream(_ stream: SCStream, didStopWithError error: Error) {
        emit("error", error.localizedDescription)
        // A stopped stream never resumes, so finish the file and go. Without
        // this the writer is never finalised — leaving an unplayable moov-less
        // file — and the process sits on the stream until something kills it.
        Task { await stop() }
    }

    // --- teardown ---------------------------------------------------------

    func stop() async {
        // The stream failing and the parent closing stdin can both arrive; the
        // second teardown would finish an already-finished writer.
        stopping.lock()
        if isStopping {
            stopping.unlock()
            return
        }
        isStopping = true
        stopping.unlock()

        engine?.inputNode.removeTap(onBus: 0)
        engine?.stop()
        systemTranscriber?.stop()
        systemTranscriber = nil
        if let stream { try? await stream.stopCapture() }

        if writer?.finish() == true {
            emit("done", outputPath)
            exit(0)
        }
        emit("error", "Nothing was captured. Check Screen Recording permission in System Settings.")
        exit(3)
    }

    /// Convert an `AVAudioPCMBuffer` from the engine into something the writer takes.
    static func sampleBuffer(from buffer: AVAudioPCMBuffer, at time: AVAudioTime) -> CMSampleBuffer?
    {
        let format = buffer.format
        var timing = CMSampleTimingInfo(
            duration: CMTime(value: 1, timescale: CMTimeScale(format.sampleRate)),
            presentationTimeStamp: CMTime(
                value: CMTimeValue(time.sampleTime), timescale: CMTimeScale(format.sampleRate)),
            decodeTimeStamp: .invalid
        )

        var description: CMFormatDescription?
        guard
            CMAudioFormatDescriptionCreate(
                allocator: kCFAllocatorDefault,
                asbd: format.streamDescription,
                layoutSize: 0, layout: nil, magicCookieSize: 0, magicCookie: nil,
                extensions: nil, formatDescriptionOut: &description) == noErr,
            let description
        else { return nil }

        var sample: CMSampleBuffer?
        guard
            CMSampleBufferCreate(
                allocator: kCFAllocatorDefault, dataBuffer: nil, dataReady: false,
                makeDataReadyCallback: nil, refcon: nil, formatDescription: description,
                sampleCount: CMItemCount(buffer.frameLength), sampleTimingEntryCount: 1,
                sampleTimingArray: &timing, sampleSizeEntryCount: 0, sampleSizeArray: nil,
                sampleBufferOut: &sample) == noErr,
            let sample,
            CMSampleBufferSetDataBufferFromAudioBufferList(
                sample, blockBufferAllocator: kCFAllocatorDefault,
                blockBufferMemoryAllocator: kCFAllocatorDefault, flags: 0,
                bufferList: buffer.mutableAudioBufferList) == noErr
        else { return nil }

        return sample
    }
}

// ---------------------------------------------------------------------------
// Run
// ---------------------------------------------------------------------------

@available(macOS 13.0, *)
func main() {
    let recorder = Recorder()

    Task {
        do {
            try await recorder.start()
        } catch {
            // The overwhelmingly common cause, and the one worth naming.
            fail(
                "Could not start recording: \(error.localizedDescription). If this is the first "
                    + "time, grant Caduceus Screen Recording in System Settings → Privacy & "
                    + "Security, then quit and reopen it — macOS requires a restart for that one."
            )
        }
    }

    // Commands arrive on stdin so the HUD can drive this without signals.
    DispatchQueue.global().async {
        while let line = readLine(strippingNewline: true) {
            switch line.trimmingCharacters(in: .whitespacesAndNewlines) {
            case "pause": recorder.setPaused(true)
            case "resume": recorder.setPaused(false)
            case "stop":
                Task { await recorder.stop() }
                return
            default: break
            }
        }
        // stdin closed: the parent is gone, so stop rather than record forever.
        Task { await recorder.stop() }
    }

    RunLoop.main.run()
}

if #available(macOS 13.0, *) {
    main()
} else {
    fail("Recording needs macOS 13 or newer.")
}
