// Live push-to-talk using the same stack macOS Dictation uses: AVAudioEngine
// feeds Apple's Speech framework with partial results on stdout.
//
// Protocol (stdout, tab-separated):
//   ready
//   partial <text>
//   final <text>
//   wav <absolute-path>
//   error <message>
//
// Stdin: line "stop" ends capture and prints final + wav path.

import AVFoundation
import Foundation
import Speech

func emit(_ kind: String, _ payload: String = "") {
    if payload.isEmpty {
        fputs("\(kind)\n", stdout)
    } else {
        fputs("\(kind)\t\(payload)\n", stdout)
    }
    fflush(stdout)
}

func fail(_ message: String) -> Never {
    emit("error", message)
    exit(2)
}

// --- authorisation ---------------------------------------------------------

let authSem = DispatchSemaphore(value: 0)
var speechAuth: SFSpeechRecognizerAuthorizationStatus = .notDetermined
SFSpeechRecognizer.requestAuthorization { status in
    speechAuth = status
    authSem.signal()
}
_ = authSem.wait(timeout: .now() + 60)

switch speechAuth {
case .authorized:
    break
case .denied:
    fail("Speech recognition denied. Enable it in System Settings → Privacy & Security → Speech Recognition.")
default:
    fail("Speech recognition permission was not granted.")
}

switch AVCaptureDevice.authorizationStatus(for: .audio) {
case .authorized:
    break
case .notDetermined:
    let micSem = DispatchSemaphore(value: 0)
    AVCaptureDevice.requestAccess(for: .audio) { _ in micSem.signal() }
    _ = micSem.wait(timeout: .now() + 60)
default:
    fail("Microphone access denied. Enable it in System Settings → Privacy & Security → Microphone.")
}

let localeId = CommandLine.arguments.count > 1 && !CommandLine.arguments[1].isEmpty
    ? CommandLine.arguments[1]
    : Locale.current.identifier

guard let recognizer = SFSpeechRecognizer(locale: Locale(identifier: localeId)) else {
    fail("No speech recogniser for locale \(localeId).")
}
guard recognizer.isAvailable else {
    fail("Speech recogniser is unavailable right now.")
}

// --- audio + recognition ---------------------------------------------------

let engine = AVAudioEngine()
let request = SFSpeechAudioBufferRecognitionRequest()
request.shouldReportPartialResults = true
if recognizer.supportsOnDeviceRecognition {
    request.requiresOnDeviceRecognition = true
}

var pcmSamples: [Float] = []
let inputNode = engine.inputNode
let format = inputNode.outputFormat(forBus: 0)

inputNode.installTap(onBus: 0, bufferSize: 2048, format: format) { buffer, _ in
    request.append(buffer)
    if let channel = buffer.floatChannelData?[0] {
        let frames = Int(buffer.frameLength)
        for i in 0..<frames {
            pcmSamples.append(channel[i])
        }
    }
}

var lastPartial = ""
var finalText = ""
var recognitionError: String?
let done = DispatchSemaphore(value: 0)

let task = recognizer.recognitionTask(with: request) { result, error in
    if let error {
        recognitionError = error.localizedDescription
        done.signal()
        return
    }
    guard let result else { return }
    let text = result.bestTranscription.formattedString.trimmingCharacters(in: .whitespacesAndNewlines)
    if result.isFinal {
        finalText = text
        done.signal()
    } else if text != lastPartial {
        lastPartial = text
        emit("partial", text)
    }
}

do {
    try engine.start()
} catch {
    fail("Could not start the microphone: \(error.localizedDescription)")
}

emit("ready")

while let line = readLine(strippingNewline: true) {
    if line.contains("stop") { break }
}

engine.inputNode.removeTap(onBus: 0)
engine.stop()
request.endAudio()

_ = done.wait(timeout: .now() + 120)
task.cancel()

if let recognitionError, finalText.isEmpty, lastPartial.isEmpty {
    fail(recognitionError)
}

let transcript = !finalText.isEmpty ? finalText : lastPartial
emit("final", transcript)

// Write 16 kHz mono WAV for routing / fallback STT.
let wavPath = FileManager.default.temporaryDirectory
    .appendingPathComponent("caduceus-live-\(UUID().uuidString).wav")
writeWav(samples: pcmSamples, sampleRate: Int(format.sampleRate), to: wavPath)
emit("wav", wavPath.path)

func writeWav(samples: [Float], sampleRate: Int, to url: URL) {
    let targetRate = 16_000
    let mono = downmixResample(samples, fromRate: sampleRate, toRate: targetRate)
    var data = Data()
    let byteRate = targetRate * 2
    let dataSize = mono.count * 2
    data.append(contentsOf: "RIFF".utf8)
    data.append(contentsOf: uint32LE(UInt32(36 + dataSize)))
    data.append(contentsOf: "WAVE".utf8)
    data.append(contentsOf: "fmt ".utf8)
    data.append(contentsOf: uint32LE(16))
    data.append(contentsOf: uint16LE(1))
    data.append(contentsOf: uint16LE(1))
    data.append(contentsOf: uint32LE(UInt32(targetRate)))
    data.append(contentsOf: uint32LE(UInt32(byteRate)))
    data.append(contentsOf: uint16LE(2))
    data.append(contentsOf: uint16LE(16))
    data.append(contentsOf: "data".utf8)
    data.append(contentsOf: uint32LE(UInt32(dataSize)))
    for s in mono {
        let clamped = max(-1.0, min(1.0, s))
        let v = Int16(clamped * Float(Int16.max))
        data.append(contentsOf: int16LE(v))
    }
    try? data.write(to: url)
}

func downmixResample(_ input: [Float], fromRate: Int, toRate: Int) -> [Float] {
    guard fromRate > 0, toRate > 0, !input.isEmpty else { return [] }
    if fromRate == toRate { return input }
    let ratio = Double(toRate) / Double(fromRate)
    let outLen = Int((Double(input.count) * ratio).rounded())
    var out: [Float] = []
    out.reserveCapacity(outLen)
    for i in 0..<outLen {
        let src = Double(i) / ratio
        let idx = Int(src)
        let frac = Float(src - Double(idx))
        let a = input[min(idx, input.count - 1)]
        let b = input[min(idx + 1, input.count - 1)]
        out.append(a + (b - a) * frac)
    }
    return out
}

func uint16LE(_ v: UInt16) -> [UInt8] { [UInt8(v & 0xff), UInt8(v >> 8)] }
func uint32LE(_ v: UInt32) -> [UInt8] {
    [UInt8(v & 0xff), UInt8((v >> 8) & 0xff), UInt8((v >> 16) & 0xff), UInt8(v >> 24)]
}
func int16LE(_ v: Int16) -> [UInt8] { uint16LE(UInt16(bitPattern: v)) }
