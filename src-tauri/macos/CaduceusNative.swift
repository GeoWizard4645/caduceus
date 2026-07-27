// Caduceus's macOS native helper.
//
// A command-line tool for the two capabilities that need frameworks with no C
// interface Rust can reach: Vision (on-device OCR) and the CoreAudio HAL
// (listing and switching sound devices).
//
// Deliberately *not* here: anything that touches the Accessibility API. TCC
// grants Accessibility per code signature, and this helper is ad-hoc signed —
// so it would need its own entry in System Settings, and every rebuild would
// invalidate it. Window management lives in the main binary for that reason.
//
// Built by `build.rs` into `src-tauri/bin/caduceus-native` and shipped as a
// bundle resource. Build it by hand with:
//
//     swiftc -O -o bin/caduceus-native macos/CaduceusNative.swift
//
// Usage:
//     caduceus-native ocr <image-path>       Recognised text on stdout
//     caduceus-native audio-list             JSON: every input and output device
//     caduceus-native audio-set in|out <uid> Make a device the system default
//
// Exit codes: 0 success, 2 failure (reason on stderr).

import AppKit
import CoreAudio
import Foundation
import Vision

func fail(_ message: String) -> Never {
    FileHandle.standardError.write(Data((message + "\n").utf8))
    exit(2)
}

func emit(_ value: Any) {
    guard
        let data = try? JSONSerialization.data(withJSONObject: value, options: [.sortedKeys]),
        let text = String(data: data, encoding: .utf8)
    else {
        fail("could not encode the result as JSON")
    }
    print(text)
}

// ---------------------------------------------------------------------------
// OCR
// ---------------------------------------------------------------------------

// Recognise text in an image file and print it, reading order preserved.
//
// Vision returns observations in no particular order, so they are sorted top to
// bottom and then left to right before joining. Without that, a two-column
// screenshot comes out interleaved line by line and is unreadable.
func runOCR(path: String) -> Never {
    let url = URL(fileURLWithPath: path)
    guard FileManager.default.fileExists(atPath: url.path) else {
        fail("no such image: \(url.path)")
    }
    guard
        let image = NSImage(contentsOf: url),
        let cgImage = image.cgImage(forProposedRect: nil, context: nil, hints: nil)
    else {
        fail("could not read \(url.lastPathComponent) as an image")
    }

    let request = VNRecognizeTextRequest()
    request.recognitionLevel = .accurate
    request.usesLanguageCorrection = true
    if let supported = try? request.supportedRecognitionLanguages() {
        // Ask for the user's own languages where Vision has them, so a German
        // or Japanese screenshot is not forced through an English model.
        let preferred = Locale.preferredLanguages.filter { supported.contains($0) }
        if !preferred.isEmpty { request.recognitionLanguages = preferred }
    }

    let handler = VNImageRequestHandler(cgImage: cgImage, options: [:])
    do {
        try handler.perform([request])
    } catch {
        fail("text recognition failed: \(error.localizedDescription)")
    }

    guard let observations = request.results, !observations.isEmpty else {
        fail("no text was found in that image")
    }

    // Vision's coordinate space is bottom-left origin, so a larger `maxY` means
    // higher up the page.
    let lines = observations
        .sorted { a, b in
            let ay = a.boundingBox.maxY
            let by = b.boundingBox.maxY
            // Within roughly one line height, treat as the same row.
            if abs(ay - by) > 0.01 { return ay > by }
            return a.boundingBox.minX < b.boundingBox.minX
        }
        .compactMap { $0.topCandidates(1).first?.string }

    print(lines.joined(separator: "\n"))
    exit(0)
}

// ---------------------------------------------------------------------------
// Audio devices
// ---------------------------------------------------------------------------

enum AudioScope {
    case input
    case output

    var element: AudioObjectPropertyScope {
        self == .input ? kAudioDevicePropertyScopeInput : kAudioDevicePropertyScopeOutput
    }

    var defaultSelector: AudioObjectPropertySelector {
        self == .input
            ? kAudioHardwarePropertyDefaultInputDevice
            : kAudioHardwarePropertyDefaultOutputDevice
    }
}

func address(
    _ selector: AudioObjectPropertySelector,
    _ scope: AudioObjectPropertyScope = kAudioObjectPropertyScopeGlobal
) -> AudioObjectPropertyAddress {
    AudioObjectPropertyAddress(
        mSelector: selector,
        mScope: scope,
        mElement: kAudioObjectPropertyElementMain
    )
}

func allDeviceIDs() -> [AudioDeviceID] {
    var addr = address(kAudioHardwarePropertyDevices)
    var size: UInt32 = 0
    guard
        AudioObjectGetPropertyDataSize(
            AudioObjectID(kAudioObjectSystemObject), &addr, 0, nil, &size
        ) == noErr, size > 0
    else { return [] }

    let count = Int(size) / MemoryLayout<AudioDeviceID>.size
    var ids = [AudioDeviceID](repeating: 0, count: count)
    guard
        AudioObjectGetPropertyData(
            AudioObjectID(kAudioObjectSystemObject), &addr, 0, nil, &size, &ids
        ) == noErr
    else { return [] }
    return ids
}

func stringProperty(_ device: AudioDeviceID, _ selector: AudioObjectPropertySelector) -> String? {
    var addr = address(selector)
    var size = UInt32(MemoryLayout<CFString?>.size)
    var value: CFString? = nil
    let status = withUnsafeMutablePointer(to: &value) { pointer -> OSStatus in
        AudioObjectGetPropertyData(device, &addr, 0, nil, &size, pointer)
    }
    guard status == noErr, let value else { return nil }
    return value as String
}

// A device is an input or an output depending on whether it has channels in
// that scope. Most devices have exactly one; aggregate devices have both.
func hasChannels(_ device: AudioDeviceID, scope: AudioScope) -> Bool {
    var addr = address(kAudioDevicePropertyStreamConfiguration, scope.element)
    var size: UInt32 = 0
    guard AudioObjectGetPropertyDataSize(device, &addr, 0, nil, &size) == noErr, size > 0 else {
        return false
    }
    let buffer = UnsafeMutableRawPointer.allocate(
        byteCount: Int(size), alignment: MemoryLayout<AudioBufferList>.alignment
    )
    defer { buffer.deallocate() }
    guard AudioObjectGetPropertyData(device, &addr, 0, nil, &size, buffer) == noErr else {
        return false
    }
    let list = UnsafeMutableAudioBufferListPointer(
        buffer.assumingMemoryBound(to: AudioBufferList.self)
    )
    return list.reduce(0) { $0 + Int($1.mNumberChannels) } > 0
}

func defaultDevice(_ scope: AudioScope) -> AudioDeviceID {
    var addr = address(scope.defaultSelector)
    var device = AudioDeviceID(0)
    var size = UInt32(MemoryLayout<AudioDeviceID>.size)
    AudioObjectGetPropertyData(
        AudioObjectID(kAudioObjectSystemObject), &addr, 0, nil, &size, &device
    )
    return device
}

func listAudioDevices() -> Never {
    let currentInput = defaultDevice(.input)
    let currentOutput = defaultDevice(.output)

    var devices: [[String: Any]] = []
    for id in allDeviceIDs() {
        guard let name = stringProperty(id, kAudioObjectPropertyName) else { continue }
        // The UID is the only stable handle across reboots and reconnections;
        // AudioDeviceID is reassigned freely, so it must never be persisted.
        guard let uid = stringProperty(id, kAudioDevicePropertyDeviceUID) else { continue }

        let isInput = hasChannels(id, scope: .input)
        let isOutput = hasChannels(id, scope: .output)
        if !isInput && !isOutput { continue }

        devices.append([
            "uid": uid,
            "name": name,
            "isInput": isInput,
            "isOutput": isOutput,
            "isDefaultInput": id == currentInput,
            "isDefaultOutput": id == currentOutput,
        ])
    }
    emit(devices)
    exit(0)
}

func setDefaultAudioDevice(scope: AudioScope, uid: String) -> Never {
    guard
        let match = allDeviceIDs().first(where: {
            stringProperty($0, kAudioDevicePropertyDeviceUID) == uid
        })
    else {
        fail("no audio device with UID \(uid) is connected")
    }
    guard hasChannels(match, scope: scope) else {
        let kind = scope == .input ? "an input" : "an output"
        fail("\(stringProperty(match, kAudioObjectPropertyName) ?? uid) is not \(kind) device")
    }

    var addr = address(scope.defaultSelector)
    var device = match
    let status = AudioObjectSetPropertyData(
        AudioObjectID(kAudioObjectSystemObject),
        &addr,
        0,
        nil,
        UInt32(MemoryLayout<AudioDeviceID>.size),
        &device
    )
    guard status == noErr else {
        fail("CoreAudio refused the change (status \(status))")
    }
    print(stringProperty(match, kAudioObjectPropertyName) ?? uid)
    exit(0)
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

let arguments = CommandLine.arguments
guard arguments.count >= 2 else {
    fail("usage: caduceus-native <ocr|audio-list|audio-set> [...]")
}

switch arguments[1] {
case "ocr":
    guard arguments.count >= 3 else { fail("usage: caduceus-native ocr <image-path>") }
    runOCR(path: arguments[2])

case "audio-list":
    listAudioDevices()

case "audio-set":
    guard arguments.count >= 4 else {
        fail("usage: caduceus-native audio-set <in|out> <device-uid>")
    }
    switch arguments[2] {
    case "in": setDefaultAudioDevice(scope: .input, uid: arguments[3])
    case "out": setDefaultAudioDevice(scope: .output, uid: arguments[3])
    default: fail("audio-set takes 'in' or 'out', not '\(arguments[2])'")
    }

default:
    fail("unknown command '\(arguments[1])'")
}
