# Run a Rune app on iOS

Rune's experimental iOS target draws one `UIView` on a `CAMetalLayer` with the same Rust views,
layout, text and shaders as the desktop. A display link draws at up to 120 Hz while something
moves and stops when nothing does, so an idle app uses no CPU or GPU.

What works: drawing, light and dark appearance, one finger (tap, drag, pan with momentum), and
stopping in the background. What doesn't yet: the soft keyboard and text
input, safe-area insets (`Cx::safe_area` returns zero, so content can sit under the status bar),
more than one finger, VoiceOver, the clipboard and `on_ready`.

## Build and run in a simulator

Install the targets and Xcode with an iOS simulator runtime, then build any Rune binary and
hand it to `scripts/ios-app`:

```sh
rustup target add aarch64-apple-ios aarch64-apple-ios-sim
cargo build -p hello --target aarch64-apple-ios-sim
scripts/ios-app --name Hello --id com.example.hello --bin target/aarch64-apple-ios-sim/debug/hello
```

The script stages `target/ios/Hello.app` (the binary and an `Info.plist`), signs it ad hoc,
installs it on the booted simulator and launches it. `--simulator NAME_OR_UDID` picks another
simulator and boots it first; `--no-launch` and `--no-install` stop earlier; arguments after
`--` go to the app. No Xcode project is involved.

```text
usage: ios-app --name NAME --id ID --bin BIN [--exe EXE] [--out OUT] [--simulator SIMULATOR]
               [--device DEVICE] [--profile PROFILE] [--no-install] [--no-launch] [-- ARGS...]
```

## Run on a device

A device runs only what a development provisioning profile covers: one that lists the device
and whose app identifier matches `--id` (a wildcard profile matches any).

```sh
cargo build -p hello --target aarch64-apple-ios --release
scripts/ios-app --name Hello --id com.example.hello --bin target/aarch64-apple-ios/release/hello \
    --device DEVICE_UDID --profile path/to/development.mobileprovision
```

The script embeds the profile and signs with the profile's entitlements and the keychain's
`Apple Development` identity; set `IOS_APP_SIGN` to name another. `xcrun devicectl list devices`
lists paired devices. Xcode keeps the profiles it manages in
`~/Library/Developer/Xcode/UserData/Provisioning Profiles`.

## The app's entry point

The usual one. On iOS `rune::run` and `App::run` start UIKit, make the window and fill it with
the app, and never return:

```rust,ignore
fn main() {
    rune::run(AppSpec::new("My app", (390.0, 760.0), State::default, view));
}
```

`AppSpec`'s size is ignored: the app is the size of the screen. Move desktop-only dependencies
(native file dialogs, hot reload, the system tray) behind target conditions.

## A Rune view in a UIKit or SwiftUI app

`rune::ios::view(app)` returns a retained `UIView *` that draws `app` and fills whatever it is
put in. Export a function that returns it from a `staticlib`, and wrap it on the Swift side:

```rust,ignore
#[unsafe(no_mangle)]
pub extern "C" fn my_screen() -> *mut std::ffi::c_void {
    rune::ios::view(rune::App::new(AppSpec::new("Screen", (390.0, 760.0), State::default, view)))
}
```

```swift
struct MyScreen: UIViewRepresentable {
    func makeUIView(context: Context) -> UIView {
        Unmanaged<UIView>.fromOpaque(my_screen()).takeRetainedValue()
    }
    func updateUIView(_ view: UIView, context: Context) {}
}
```

Call it on the main thread; it returns null anywhere else. This path compiles and shares `App::run`'s view, but
has not yet been run inside a host app.

## What the `Info.plist` needs

`scripts/ios-app` writes these. An app with its own bundle needs the same:

- `UIApplicationSceneManifest`, with no configurations: Rune supplies the scene's in code.
- `UILaunchScreen` (an empty dictionary will do), or iOS letterboxes the app.
- `CADisableMinimumFrameDurationOnPhone` set to true, or an iPhone holds the app at 60 Hz.
- `MinimumOSVersion` 16.0 or later.
