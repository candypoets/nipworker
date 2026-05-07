// swift-tools-version:5.9
import PackageDescription

let package = Package(
    name: "NipworkerSwift",
    platforms: [.iOS(.v14), .macOS(.v13)],
    products: [
        .library(
            name: "NipworkerSwift",
            targets: ["NipworkerSwift"]
        ),
    ],
    dependencies: [
        .package(url: "https://github.com/google/flatbuffers.git", from: "25.2.10")
    ],
    targets: [
        .binaryTarget(
            name: "NipworkerNativeFFI",
            path: "../crates/native-ffi/ios/NipworkerNativeFFI.xcframework"
        ),
        .target(
            name: "NipworkerSwift",
            dependencies: [
                "NipworkerNativeFFI",
                .product(name: "FlatBuffers", package: "flatbuffers")
            ],
            path: "Sources/NipworkerSwift",
            exclude: []
        ),
        .testTarget(
            name: "NipworkerSwiftTests",
            dependencies: ["NipworkerSwift"],
            path: "Tests/NipworkerSwiftTests"
        ),
    ]
)
