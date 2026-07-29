import AVFoundation
import CoreML
import FluidAudio
import Foundation

// Caduceus live Parakeet helper.
//
// Architecture and preview cadence are adapted from MacParakeet 0.7.3
// (GPL-3.0), commit 408d1bcd0b488c2363bc2de9d5dc62933478d413:
// record the complete WAV, transcribe a rolling 15-second tail every second for
// display, and run a separate final pass at Stop. FluidAudio is pinned to the
// same 0.15.4 release. Copyright (C) 2026 Daniel Moon and contributors.
//
// Protocol matches caduceus-stt-live so Rust can fall back to Apple Speech:
//   preparing <message> | ready | partial <text> | final <text> | wav <path>
// stdin: pause | resume | stop
//
// Everything executable lives in `run()` rather than in top-level statements.
// Top-level code in a Swift 6 script is implicitly @MainActor, and closures
// formed there inherit that isolation — including the AVAudioEngine tap block,
// which CoreAudio then invokes on its realtime thread. The runtime's dynamic
// isolation check kills the process with SIGTRAP the moment the first audio
// buffer arrives, which the app sees as "Broken pipe" on the next stdin write.
// The same inheritance parked the preview Task on a main actor that was
// blocked in readLine, so partials never came out either. A nonisolated
// `run()` gives the tap, the preview loop and the stdin loop plain
// backgrounds to live on.

private enum Out {
    static let lock = NSLock()

    static func emit(_ kind: String, _ payload: String = "") {
        lock.lock()
        defer { lock.unlock() }
        let safe = payload.replacingOccurrences(of: "\n", with: " ")
        fputs(safe.isEmpty ? "\(kind)\n" : "\(kind)\t\(safe)\n", stdout)
        fflush(stdout)
    }
}

private func emit(_ kind: String, _ payload: String = "") {
    Out.emit(kind, payload)
}

private func fail(_ message: String) -> Never {
    emit("error", message)
    exit(2)
}

private func modelIsReady() -> Bool {
    let directory = AsrModels.defaultCacheDirectory(for: .v3)
    return AsrModels.modelsExist(
        at: directory, version: .v3, encoderPrecision: .int8)
}

private final class AudioStore: @unchecked Sendable {
    private let lock = NSLock()
    private let maxRetainedSamples: Int?
    private var samples: [Float] = []
    private var totalSamples = 0
    private var paused = false

    init(maxRetainedSamples: Int? = nil) {
        self.maxRetainedSamples = maxRetainedSamples
    }

    func setPaused(_ value: Bool) {
        lock.withLock { paused = value }
    }

    func append(_ newSamples: [Float]) {
        guard !newSamples.isEmpty else { return }
        lock.withLock {
            guard !paused else { return }
            samples.append(contentsOf: newSamples)
            totalSamples += newSamples.count
            if let maxRetainedSamples, samples.count > maxRetainedSamples {
                samples.removeFirst(samples.count - maxRetainedSamples)
            }
        }
    }

    func snapshot(limit: Int? = nil) -> [Float] {
        lock.withLock {
            guard let limit, samples.count > limit else { return samples }
            return Array(samples.suffix(limit))
        }
    }

    var count: Int { lock.withLock { totalSamples } }
}

private actor ParakeetRuntime {
    private var manager: AsrManager?
    private var stabilizer = LiveTranscriptStabilizer()

    func prepare() async throws {
        guard manager == nil else { return }
        emit("preparing", "Preparing Caduceus on-device transcription…")
        let config = MLModelConfiguration()
        config.computeUnits = .cpuAndNeuralEngine
        let models = try await AsrModels.downloadAndLoad(
            configuration: config,
            version: .v3,
            encoderPrecision: .int8,
            progressHandler: { progress in
                emit(
                    "preparing",
                    "Downloading transcription model \(Int(progress.fractionCompleted * 100))%"
                )
            }
        )
        let loaded = AsrManager(config: .default)
        try await loaded.loadModels(models)
        manager = loaded
    }

    func preview(_ samples: [Float]) async throws -> String {
        try await prepare()
        guard let manager else { return "" }
        var state = TdtDecoderState.make(decoderLayers: await manager.decoderLayerCount)
        let result = try await manager.transcribe(samples, decoderState: &state)
        return stabilizer.ingest(result.text)
    }

    func final(_ samples: [Float]) async throws -> String {
        try await prepare()
        guard let manager else { return "" }
        // MacParakeet pads dictation finals so a word spoken against the stop
        // boundary still has enough decoder context to be emitted.
        let padded = samples + [Float](repeating: 0, count: 8_000)
        var state = TdtDecoderState.make(decoderLayers: await manager.decoderLayerCount)
        let result = try await manager.transcribe(padded, decoderState: &state)
        return stabilizer.finalize(result.text)
    }
}

private func mono16k(_ buffer: AVAudioPCMBuffer) -> [Float] {
    guard let channels = buffer.floatChannelData else { return [] }
    let frames = Int(buffer.frameLength)
    let channelCount = Int(buffer.format.channelCount)
    guard frames > 0, channelCount > 0 else { return [] }
    var mono = [Float](repeating: 0, count: frames)
    for channel in 0..<channelCount {
        for frame in 0..<frames { mono[frame] += channels[channel][frame] }
    }
    if channelCount > 1 {
        let scale = 1 / Float(channelCount)
        for i in mono.indices { mono[i] *= scale }
    }
    let sourceRate = Int(buffer.format.sampleRate.rounded())
    guard sourceRate != 16_000 else { return mono }
    let ratio = Double(sourceRate) / 16_000
    let count = Int(Double(mono.count) / ratio)
    return (0..<count).map { index in
        let position = Double(index) * ratio
        let lower = Int(position)
        let fraction = Float(position - Double(lower))
        let a = mono[min(lower, mono.count - 1)]
        let b = mono[min(lower + 1, mono.count - 1)]
        return a + (b - a) * fraction
    }
}

private func writeWav(_ samples: [Float]) -> URL {
    let url = FileManager.default.temporaryDirectory
        .appendingPathComponent("caduceus-parakeet-\(UUID().uuidString).wav")
    let format = AVAudioFormat(
        commonFormat: .pcmFormatInt16, sampleRate: 16_000, channels: 1, interleaved: true)!
    let buffer = AVAudioPCMBuffer(pcmFormat: format, frameCapacity: AVAudioFrameCount(samples.count))!
    buffer.frameLength = AVAudioFrameCount(samples.count)
    if let output = buffer.int16ChannelData?[0] {
        for i in samples.indices {
            output[i] = Int16(max(-1, min(1, samples[i])) * Float(Int16.max))
        }
    }
    if let file = try? AVAudioFile(forWriting: url, settings: format.settings) {
        try? file.write(from: buffer)
    }
    return url
}

private func run() async {
    let pcmStdinMode = CommandLine.arguments.contains("--stdin-pcm16")
    let modelStatusMode = CommandLine.arguments.contains("--model-ready")
    let prepareModelMode = CommandLine.arguments.contains("--prepare-model")

    // Meeting stdin mode only needs the rolling preview; its authoritative
    // final pass reads the saved file. Keep that path bounded for multi-hour
    // calls.
    let store = AudioStore(maxRetainedSamples: pcmStdinMode ? 15 * 16_000 : nil)
    let runtime = ParakeetRuntime()

    if modelStatusMode {
        exit(modelIsReady() ? 0 : 1)
    }

    if prepareModelMode {
        do {
            try await runtime.prepare()
            exit(0)
        } catch {
            fail("Could not prepare the local transcription model: \(error.localizedDescription)")
        }
    }

    let engine: AVAudioEngine? = pcmStdinMode ? nil : AVAudioEngine()

    if let engine {
        let input = engine.inputNode
        let format = input.outputFormat(forBus: 0)
        // A missing microphone grant shows up here as a 0 Hz / 0-channel
        // format, and installTap raises an uncatchable NSException on it.
        // Failing over stdout keeps the app's error path in charge instead.
        guard format.sampleRate > 0, format.channelCount > 0 else {
            fail(
                "The microphone is not available. Enable it for Caduceus in "
                    + "System Settings → Privacy & Security → Microphone.")
        }
        // Let Core Audio negotiate the active device's native stream format.
        // Feeding outputFormat back into the tap can be rejected at start with
        // kAudioUnitErr_FormatNotSupported (-10868) after route changes or for
        // virtual/Bluetooth input devices.
        input.installTap(onBus: 0, bufferSize: 2048, format: nil) { buffer, _ in
            store.append(mono16k(buffer))
        }
        do {
            try engine.start()
        } catch {
            fail("Could not start the microphone: \(error.localizedDescription)")
        }
    }

    emit("ready")

    // Load the model while the first second of audio accumulates, so the
    // first preview does not also pay the model-load cost. Meeting stdin mode
    // leaves downloading to the dictation helper, as before.
    if !pcmStdinMode {
        Task { try? await runtime.prepare() }
    }

    let previewTask = Task {
        var lastTotalSampleCount = 0
        while !Task.isCancelled {
            try? await Task.sleep(for: .seconds(1))
            guard !Task.isCancelled else { break }
            let totalSampleCount = store.count
            let tail = store.snapshot(limit: 15 * 16_000)
            guard tail.count >= 8_000, totalSampleCount != lastTotalSampleCount else { continue }
            // In a first-use meeting, the microphone/voice path prepares the
            // model in a separate helper. Keep buffering the bounded
            // system-audio tail and join live as soon as that download becomes
            // ready.
            if pcmStdinMode, !modelIsReady() { continue }
            lastTotalSampleCount = totalSampleCount
            do {
                let text = try await runtime.preview(tail)
                if !text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
                    emit("partial", text)
                }
            } catch {
                emit("error", "Live transcription is temporarily unavailable: \(error.localizedDescription)")
            }
        }
    }

    if pcmStdinMode {
        // Raw signed little-endian 16-bit mono, already resampled to 16 kHz by
        // the ScreenCaptureKit recorder. Binary input avoids base64 and
        // line-protocol overhead during hour-long calls.
        var pendingByte: UInt8?
        while let data = try? FileHandle.standardInput.read(upToCount: 16_384),
              !data.isEmpty {
            var bytes = Array(data)
            if let carriedByte = pendingByte {
                bytes.insert(carriedByte, at: 0)
                pendingByte = nil
            }
            if !bytes.count.isMultiple(of: 2) {
                pendingByte = bytes.removeLast()
            }
            let samples = stride(from: 0, to: bytes.count, by: 2).map { index in
                let value = UInt16(bytes[index]) | (UInt16(bytes[index + 1]) << 8)
                return Float(Int16(bitPattern: value)) / Float(Int16.max)
            }
            store.append(samples)
        }
    } else {
        while let line = readLine(strippingNewline: true) {
            switch line.trimmingCharacters(in: .whitespacesAndNewlines) {
            case "pause": store.setPaused(true)
            case "resume": store.setPaused(false)
            case "stop": break
            default: continue
            }
            if line.trimmingCharacters(in: .whitespacesAndNewlines) == "stop" { break }
        }
    }

    if let engine {
        engine.inputNode.removeTap(onBus: 0)
        engine.stop()
    }
    previewTask.cancel()
    _ = await previewTask.result

    let samples = store.snapshot()
    guard !samples.isEmpty else { fail("Nothing was said — hold the key a little longer.") }

    // Meeting system audio is finalised from the saved recording by Caduceus.
    // The helper's job in stdin mode is only the no-wait live preview.
    if pcmStdinMode { exit(0) }

    do {
        let text = try await runtime.final(samples)
        emit("final", text)
    } catch {
        fail("Caduceus could not transcribe this recording: \(error.localizedDescription)")
    }
    let wav = writeWav(samples)
    emit("wav", wav.path)
}

await run()
