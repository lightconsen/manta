// swift-tools-version:5.3
// Syscity native iOS plugin (mobile-migration §4.4/§4.6).
//
// This package is compiled into the iOS static library by swift-rs at Rust
// build time (see desktop/build.rs `link_ios_swift`), mirroring how
// tauri-plugin crates link their `ios/` packages. The Tauri Swift framework
// is copied into `.tauri/tauri-api` by the build script before swift-rs
// compiles; that directory is a build artifact and is gitignored.

import PackageDescription

let package = Package(
  name: "syscity-device",
  platforms: [
    .macOS(.v10_13),
    .iOS(.v13),
  ],
  products: [
    // Products define the executables and libraries a package produces, and make them visible to other packages.
    .library(
      name: "syscity-device",
      type: .static,
      targets: ["syscity-device"])
  ],
  dependencies: [
    .package(name: "Tauri", path: ".tauri/tauri-api")
  ],
  targets: [
    // Targets are the basic building blocks of a package. A target can define a module or a test suite.
    .target(
      name: "syscity-device",
      dependencies: [
        .byName(name: "Tauri")
      ],
      path: "Sources")
  ]
)
