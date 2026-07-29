import Foundation

// Adapted from MacParakeet's LiveTranscriptStabilizer.swift at
// 408d1bcd0b488c2363bc2de9d5dc62933478d413 (GPL-3.0).
// Copyright (C) 2026 Daniel Moon and MacParakeet contributors.
// Modified for Caduceus: this version is small, Sendable, and returns the full
// append-only transcript instead of only the overlay's short display tail.
struct LiveTranscriptStabilizer: Sendable {
    private(set) var committedWords: [String] = []
    private var hypothesisWords: [String] = []
    private let hypothesisHoldback = 3
    private let anchorLength = 6

    mutating func ingest(_ raw: String) -> String {
        let words = raw.split(whereSeparator: \.isWhitespace).map(String.init)
        guard !words.isEmpty else { return readout }

        let newFrom = newWordOffset(for: words)
        let newWords = Array(words[newFrom...])
        let commitCount = max(0, newWords.count - hypothesisHoldback)
        if commitCount > 0 {
            committedWords.append(contentsOf: newWords[0..<commitCount])
        }
        hypothesisWords = Array(newWords[commitCount...])
        return readout
    }

    mutating func finalize(_ raw: String) -> String {
        _ = ingest(raw)
        committedWords.append(contentsOf: hypothesisWords)
        hypothesisWords = []
        return readout
    }

    private var readout: String { (committedWords + hypothesisWords).joined(separator: " ") }

    private func newWordOffset(for words: [String]) -> Int {
        guard !committedWords.isEmpty else { return 0 }
        let incoming = words.map(Self.normalize)
        let committed = committedWords.map(Self.normalize)
        var anchor = min(anchorLength, incoming.count, committed.count)
        while anchor >= 1 {
            let suffix = committed.suffix(anchor)
            let match = anchor >= 2
                ? firstMatchEnd(of: suffix, in: incoming)
                : lastMatchEnd(of: suffix, in: incoming)
            if let match { return match }
            anchor -= 1
        }
        return recentlyContains(incoming, committed: committed) ? words.count : 0
    }

    private func recentlyContains(_ needle: [String], committed: [String]) -> Bool {
        let recent = Array(committed.suffix(max(anchorLength * 2, needle.count)))
        return firstMatchEnd(of: needle, in: recent) != nil
    }

    private func firstMatchEnd<P: Collection>(of pattern: P, in sequence: [String]) -> Int?
    where P.Element == String {
        guard !pattern.isEmpty, pattern.count <= sequence.count else { return nil }
        for start in 0...(sequence.count - pattern.count) {
            if sequence[start..<start + pattern.count].elementsEqual(pattern) {
                return start + pattern.count
            }
        }
        return nil
    }

    private func lastMatchEnd<P: Collection>(of pattern: P, in sequence: [String]) -> Int?
    where P.Element == String {
        guard !pattern.isEmpty, pattern.count <= sequence.count else { return nil }
        var start = sequence.count - pattern.count
        while start >= 0 {
            if sequence[start..<start + pattern.count].elementsEqual(pattern) {
                return start + pattern.count
            }
            start -= 1
        }
        return nil
    }

    private static func normalize(_ word: String) -> String {
        let trim = CharacterSet.punctuationCharacters.union(.symbols)
        let value = word.trimmingCharacters(in: trim)
        return (value.isEmpty ? word : value).lowercased()
    }
}
