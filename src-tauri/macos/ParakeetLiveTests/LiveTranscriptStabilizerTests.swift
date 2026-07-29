@testable import CaduceusParakeetLive
import XCTest

final class LiveTranscriptStabilizerTests: XCTestCase {
    func testCumulativeRevisionsDoNotDuplicateWords() {
        var subject = LiveTranscriptStabilizer()
        XCTAssertEqual(subject.ingest("one two three four"), "one two three four")
        XCTAssertEqual(
            subject.ingest("one two three four five six"),
            "one two three four five six"
        )
    }

    func testSlidingTailAppendsOnlyNewWords() {
        var subject = LiveTranscriptStabilizer()
        _ = subject.ingest("alpha beta gamma delta epsilon zeta")
        XCTAssertEqual(
            subject.ingest("delta epsilon zeta eta theta iota"),
            "alpha beta gamma delta epsilon zeta eta theta iota"
        )
    }

    func testPunctuationDoesNotBreakTheOverlapAnchor() {
        var subject = LiveTranscriptStabilizer()
        _ = subject.ingest("hello there, this is Caduceus")
        XCTAssertEqual(
            subject.ingest("there this is Caduceus speaking live now"),
            "hello there, this is Caduceus speaking live now"
        )
    }
}
