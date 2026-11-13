# Run a Rune app on iOS

Rune's experimental iOS target draws one `UIView` on a `CAMetalLayer` with the same Rust views,
layout, text and shaders as the desktop. A display link draws at up to 120 Hz while something
moves and stops when nothing does, so an idle app uses no CPU or GPU.

What works:

- **Drawing.** Light and dark appearance, the screen's own scale, up to 120 Hz, nothing drawn
  while idle or in the background.
- **Touch.** One finger taps, drags and pans with momentum; two fingers pinch (`on_magnify`)
  and turn (`on_rotate`); a long press is the secondary press, so it opens what a right-click
  opens on a desktop.
- **Pointer.** A mouse or trackpad on an iPad hovers, clicks, right-clicks and scrolls.
- **Safe area.** `Cx::safe_area` holds what the status bar, the home indicator and the soft
  keyboard take: pad a full-screen root by it.
- **Text input.** The soft keyboard shows while a text field has the focus. Typing, delete,
  return, predictions, autocorrection, dictation and marked text edit the field; a secure
  field shows the keyboard none of its text. The keyboard is told where the field's
  characters are, so the space bar's trackpad moves the caret.
- **Hardware keyboard.** Arrows, Return, Escape, Tab and the function keys with their
  modifiers, key repeat, and shortcuts with ⌘ as on a Mac. Every key is also a physical key,
  for `El::on_physical_input`.
- **Clipboard and menus.** Copy, cut and paste use the system pasteboard. A menu from
  `Tx::popup_menu` (a field's Cut, Copy and Paste among them) shows as the system's edit menu.
- **Accessibility.** VoiceOver, Switch Control and Voice Control read the same accessibility
  tree the desktop platforms do. The Reduce Motion setting is followed, and
  `Cx::system_text_scale` is the Dynamic Type size: `cx.zoom(cx.system_text_scale())` scales
  a view with it.
- **Handles.** `AppHandle::update` and `wake` from a background thread reach the app, as from
  a window; `is_visible` follows the app going to the background.
- **Links and files.** `Tx::open_url` opens a link in the app that owns its scheme.
  `Tx::open_file` shows the system's document picker and answers with a copy of the file;
  `Tx::open_folder` answers with a folder the app may use until it quits, and `Tx::save_file`
  asks for the folder and answers with the dialog's file name inside it.

What doesn't yet: the system's selection handles and loupe (a field draws its own selection).
`App::on_ready` runs once the first frame is on screen; its handle reaches no window.

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
put in. Export a function that returns it from the app's library, and declare it in a C header:

```rust,ignore
#[cfg(target_os = "ios")]
#[unsafe(no_mangle)]
pub extern "C" fn my_screen() -> *mut std::ffi::c_void {
    rune::ios::view(rune::App::new(AppSpec::new("Screen", (390.0, 760.0), State::default, view)))
}
```

```c
void *my_screen(void);
```

`scripts/ios-lib` builds the library for devices and Apple silicon simulators and writes an
XCFramework that holds both, the header and a module map:

```sh
scripts/ios-lib --package my-app --module MyApp --header include/my_app.h
```

```text
usage: ios-lib --package PACKAGE --module MODULE --header HEADER [--out OUT] [--debug]
```

It builds with `cargo rustc --crate-type staticlib`, so the package's manifest needs no
`staticlib` crate type. The module map links the system frameworks Rune uses, so the host app
adds no linker flags. In Xcode, add `target/ios/MyApp.xcframework` to the app target's
frameworks and add the `ios/RuneView` Swift package, then:

```swift
import SwiftUI
import RuneView
import MyApp

struct Screen: View {
    var body: some View {
        RuneView(my_screen)
    }
}
```

`RuneView` is a `UIViewRepresentable`; a UIKit app takes the pointer itself with
`Unmanaged<UIView>.fromOpaque(my_screen()).takeRetainedValue()`. Call the function on the main
thread: it returns null anywhere else. The view reports the safe area of where it sits, so one
that doesn't reach the screen's edges reads zero. `examples/web-hello` exports
`web_hello_view` this way (`examples/web-hello/include/web_hello.h`).

## What the `Info.plist` needs

`scripts/ios-app` writes these. An app with its own bundle needs the same:

- `UIApplicationSceneManifest`, with no configurations: Rune supplies the scene's in code.
- `UILaunchScreen` (an empty dictionary will do), or iOS letterboxes the app.
- `CADisableMinimumFrameDurationOnPhone` set to true, or an iPhone holds the app at 60 Hz.
- `MinimumOSVersion` 16.0 or later.
- `UISupportedInterfaceOrientations~ipad` with all four orientations, or an iPad runs the app
  full screen only, never in Split View or a Stage Manager window.
