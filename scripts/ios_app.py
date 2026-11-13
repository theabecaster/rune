"""Bundle a Rune app built for iOS, install it on a simulator or a device and launch it."""
import argparse
import os
from pathlib import Path
import plistlib
import shutil
import sys
import tempfile

from mac_app_install import command

# Booting a simulator and installing an app on it take far longer than the Mac installer's steps.
SLOW = 180


def info(args):
    """The bundle's Info.plist: a full-screen app on one window scene Rune configures in code."""
    return dict(
        CFBundleName=args.name, CFBundleDisplayName=args.name, CFBundleIdentifier=args.id,
        CFBundleExecutable=args.exe, CFBundlePackageType="APPL",
        CFBundleShortVersionString="0.0.1", CFBundleVersion="1",
        # Rune's Metal backend writes MSL 3.0, which needs iOS 16.
        MinimumOSVersion="16.0",
        UIDeviceFamily=[1, 2],
        # An empty launch screen is what makes iOS give the app the whole display.
        UILaunchScreen={},
        # Without it an iPhone holds a display link at 60 Hz on a 120 Hz display.
        CADisableMinimumFrameDurationOnPhone=True,
        UIApplicationSceneManifest=dict(UIApplicationSupportsMultipleScenes=False),
        UISupportedInterfaceOrientations=[
            "UIInterfaceOrientationPortrait", "UIInterfaceOrientationLandscapeLeft",
            "UIInterfaceOrientationLandscapeRight"],
        # An iPad app that turns every way can share the screen (Split View, Stage Manager's
        # windows); one that leaves an orientation out runs full screen only.
        **{"UISupportedInterfaceOrientations~ipad": [
            "UIInterfaceOrientationPortrait", "UIInterfaceOrientationPortraitUpsideDown",
            "UIInterfaceOrientationLandscapeLeft", "UIInterfaceOrientationLandscapeRight"]},
    )


def entitlements(profile, bundle_id):
    """What a development profile grants, with its wildcard app identifier narrowed to this app."""
    granted = plistlib.loads(command("security", "cms", "-D", "-i", str(profile)).stdout.encode())["Entitlements"]
    team = granted["com.apple.developer.team-identifier"]
    return {**granted, "application-identifier": f"{team}.{bundle_id}"}


def sign(stage, args):
    """Ad hoc for a simulator; for a device, with the profile's entitlements and a development identity."""
    if not args.profile:
        command("codesign", "--force", "--sign", "-", str(stage), timeout=SLOW)
        return
    shutil.copy2(args.profile, stage / "embedded.mobileprovision")
    identity = os.environ.get("IOS_APP_SIGN") or "Apple Development"
    with tempfile.NamedTemporaryFile(suffix=".plist") as file:
        plistlib.dump(entitlements(args.profile, args.id), file)
        file.flush()
        command("codesign", "--force", "--sign", identity, "--entitlements", file.name, str(stage), timeout=SLOW)


def build(stage, args):
    """Stage `<out>/<name>.app` from the built binary and sign it."""
    if stage.exists():
        shutil.rmtree(stage)
    stage.mkdir(parents=True)
    shutil.copy2(args.bin, stage / args.exe)
    with (stage / "Info.plist").open("wb") as file:
        plistlib.dump(info(args), file)
    sign(stage, args)


def install_simulator(stage, args):
    """Boot the simulator if it isn't, replace the installed app and, unless told not to, launch it."""
    simctl = ("xcrun", "simctl")
    if args.simulator != "booted":
        # Booting a booted simulator fails; bootstatus then returns at once.
        command(*simctl, "boot", args.simulator, check=False, timeout=SLOW)
        command(*simctl, "bootstatus", args.simulator, timeout=SLOW)
    command(*simctl, "terminate", args.simulator, args.id, check=False, timeout=SLOW)
    command(*simctl, "install", args.simulator, str(stage), timeout=SLOW)
    if not args.no_launch:
        print(command(*simctl, "launch", args.simulator, args.id, *args.args, timeout=SLOW).stdout.strip())


def install_device(stage, args):
    """Replace the app on a paired device and, unless told not to, launch it in place of a running one."""
    device = ("xcrun", "devicectl", "device")
    command(*device, "install", "app", "--device", args.device, str(stage), timeout=SLOW)
    if not args.no_launch:
        command(*device, "process", "launch", "--device", args.device, "--terminate-existing", args.id, *args.args, timeout=SLOW)


def main():
    parser = argparse.ArgumentParser(prog="ios-app", description=__doc__)
    for name in ("name", "id", "bin"):
        parser.add_argument(f"--{name}", required=True)
    parser.add_argument("--exe", help="the executable's name in the bundle (default: the binary's)")
    parser.add_argument("--out", type=Path, default=Path("target/ios"), help="where the bundle is staged")
    parser.add_argument("--simulator", default="booted", help="a simulator's UDID or name (default: the booted one)")
    parser.add_argument("--device", help="a paired device's UDID or name, instead of a simulator; needs --profile")
    parser.add_argument("--profile", type=Path, help="a development .mobileprovision that covers --id and the device")
    parser.add_argument("--no-install", action="store_true", help="stage the bundle only")
    parser.add_argument("--no-launch", action="store_true", help="install without launching")
    parser.add_argument("args", nargs=argparse.REMAINDER, help="after --, passed to the app")
    args = parser.parse_args()
    if args.args[:1] == ["--"]:
        args.args = args.args[1:]
    if not os.access(args.bin, os.X_OK):
        parser.error(f"no executable binary at {args.bin}")
    if bool(args.device) != bool(args.profile):
        parser.error("--device and --profile go together: a device runs only what a profile covers")
    args.exe = args.exe or Path(args.bin).name
    stage = args.out / f"{args.name}.app"
    build(stage, args)
    print(stage)
    if not args.no_install:
        (install_device if args.device else install_simulator)(stage, args)


if __name__ == "__main__":
    try:
        main()
    except Exception as error:
        detail = getattr(error, "stderr", None)
        print(f"ios-app: {error}" + (f"\n{detail.strip()}" if detail else ""), file=sys.stderr)
        sys.exit(1)
