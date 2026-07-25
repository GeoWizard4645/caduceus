// Orbit's macOS speech-to-text helper.
//
// A ~90-line command-line tool that hands a WAV file to Apple's Speech
// framework and prints the transcript. It exists because `Speech.framework` has
// no C interface Rust can call directly, and because shelling out keeps the
// permission prompt, the framework link, and the Swift runtime entirely outside
// Orbit's main binary — if this helper is missing or fails, Orbit falls back to
// an HTTP speech-to-text endpoint and nothing else breaks.
//
// Built by `build.rs` into `src-tauri/bin/orbit-stt` and shipped as a bundle
// resource. Build it by hand with:
//
//     swiftc -O -o bin/orbit-stt macos/OrbitSTT.swift
//
// Usage: orbit-stt <path-to-audio> [locale-identifier]
// Exit codes: 0 success (transcript on stdout), 2 failure (reason on stderr).

import Foundation
import Speech

func fail(_ message: String) -> Never {
    FileHandle.standardError.write(Data((message + "\n").utf8))
    exit(2)
}

let arguments = CommandLine.arguments
guard arguments.count >= 2 else {
    fail("usage: orbit-stt <path-to-audio> [locale-identifier]")
}

let audioURL = URL(fileURLWithPath: arguments[1])
guard FileManager.default.fileExists(atPath: audioURL.path) else {
    fail("no such audio file: \(audioURL.path)")
}

let localeIdentifier: String = {
    if arguments.count >= 3, !arguments[2].isEmpty { return arguments[2] }
    return Locale.current.identifier
}()

// Authorisation. The first run shows the system prompt described by
// NSSpeechRecognitionUsageDescription in Orbit's Info.plist.
let authSemaphore = DispatchSemaphore(value: 0)
var authorization: SFSpeechRecognizerAuthorizationStatus = .notDetermined
SFSpeechRecognizer.requestAuthorization { status in
    authorization = status
    authSemaphore.signal()
}
_ = authSemaphore.wait(timeout: .now() + 60)

switch authorization {
case .authorized:
    break
case .denied:
    fail("speech recognition was denied. Enable it in System Settings > Privacy & Security > Speech Recognition.")
case .restricted:
    fail("speech recognition is restricted on this device.")
default:
    fail("speech recognition permission was not granted.")
}

guard let recognizer = SFSpeechRecognizer(locale: Locale(identifier: localeIdentifier)) else {
    fail("no speech recogniser is available for locale \(localeIdentifier).")
}
guard recognizer.isAvailable else {
    fail("the speech recogniser is currently unavailable.")
}

let request = SFSpeechURLRecognitionRequest(url: audioURL)
request.shouldReportPartialResults = false
// Prefer on-device recognition: it keeps audio off Apple's servers and works
// without a network connection. Falls back automatically when the language pack
// is not installed.
if recognizer.supportsOnDeviceRecognition {
    request.requiresOnDeviceRecognition = true
}

let done = DispatchSemaphore(value: 0)
var transcript = ""
var failure: String?

let task = recognizer.recognitionTask(with: request) { result, error in
    if let error {
        failure = error.localizedDescription
        done.signal()
        return
    }
    guard let result else { return }
    if result.isFinal {
        transcript = result.bestTranscription.formattedString
        done.signal()
    }
}

if done.wait(timeout: .now() + 120) == .timedOut {
    task.cancel()
    fail("speech recognition timed out.")
}

if let failure {
    fail(failure)
}

print(transcript)
