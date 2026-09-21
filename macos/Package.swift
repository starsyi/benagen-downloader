// swift-tools-version: 6.0
import PackageDescription

let package = Package(
    name: "BenagenDownloader",
    platforms: [.macOS(.v13)],
    products: [
        .executable(name: "BenagenDownloader", targets: ["BenagenDownloader"]),
        .library(name: "BenagenCoreKit", targets: ["BenagenCoreKit"]),
    ],
    dependencies: [],                       // 约束 9：不得有外部依赖
    targets: [
        .target(name: "BenagenCoreKit"),
        .executableTarget(name: "BenagenDownloader", dependencies: ["BenagenCoreKit"]),
        .testTarget(name: "BenagenCoreKitTests", dependencies: ["BenagenCoreKit"]),
    ]
)
