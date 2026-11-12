// swift-tools-version: 5.9
import PackageDescription

let package = Package(
    name: "RuneView",
    platforms: [.iOS(.v16)],
    products: [.library(name: "RuneView", targets: ["RuneView"])],
    targets: [.target(name: "RuneView")]
)
