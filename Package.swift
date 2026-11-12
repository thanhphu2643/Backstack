// swift-tools-version:5.9
import PackageDescription

let package = Package(
    name: "Backstack",
    platforms: [.macOS(.v14)],
    targets: [
        .systemLibrary(name: "CBackstackCore"),
        .executableTarget(name: "Backstack", dependencies: ["CBackstackCore"]),
    ]
)
