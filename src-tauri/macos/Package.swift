// swift-tools-version: 6.0

import PackageDescription
import Foundation

let helperPlist = URL(fileURLWithPath: #filePath)
    .deletingLastPathComponent()
    .appendingPathComponent("HelperInfo.plist")
    .path

// The on-device speech helper is intentionally its own package. Caduceus's
// Rust binary stays small, while the helper can link the same FluidAudio /
// Parakeet CoreML stack used by MacParakeet.
let package = Package(
    name: "CaduceusParakeetHelpers",
    platforms: [.macOS(.v14)],
    products: [
        .executable(name: "caduceus-parakeet-live", targets: ["CaduceusParakeetLive"]),
    ],
    dependencies: [
        // Match the version audited and pinned by MacParakeet 0.7.3.
        .package(url: "https://github.com/FluidInference/FluidAudio", exact: "0.15.4"),
    ],
    targets: [
        .executableTarget(
            name: "CaduceusParakeetLive",
            dependencies: [
                .product(name: "FluidAudio", package: "FluidAudio"),
            ],
            path: "ParakeetLive",
            linkerSettings: [
                // This executable asks for microphone access directly. TCC
                // reads the usage string from the binary's signed plist.
                .unsafeFlags([
                    "-Xlinker", "-sectcreate", "-Xlinker", "__TEXT",
                    "-Xlinker", "__info_plist", "-Xlinker", helperPlist,
                ]),
            ]
        ),
        .testTarget(
            name: "CaduceusParakeetLiveTests",
            dependencies: ["CaduceusParakeetLive"],
            path: "ParakeetLiveTests"
        ),
    ]
)
