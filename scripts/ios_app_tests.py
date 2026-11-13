"""Staged-bundle regressions: never boots a simulator."""
import contextlib
import io
from pathlib import Path
import plistlib
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import patch

import ios_app


def args(tmp, **over):
    binary = Path(tmp) / "hello"
    binary.write_text("binary")
    binary.chmod(0o755)
    base = dict(name="Hello", id="com.example.hello", bin=str(binary), exe="hello",
                simulator="booted", device=None, profile=None, no_launch=False, args=[])
    return SimpleNamespace(**{**base, **over})


class BundleRegression(unittest.TestCase):
    def test_a_staged_bundle_holds_the_binary_and_the_keys_a_full_screen_120hz_app_needs(self):
        with tempfile.TemporaryDirectory() as tmp, patch.object(ios_app, "command") as command:
            stage = Path(tmp) / "out/Hello.app"
            ios_app.build(stage, args(tmp))
            self.assertEqual((stage / "hello").read_text(), "binary")
            plist = plistlib.loads((stage / "Info.plist").read_bytes())
            self.assertEqual(plist["CFBundleExecutable"], "hello")
            self.assertEqual(plist["CFBundleIdentifier"], "com.example.hello")
            self.assertEqual(plist["UILaunchScreen"], {})
            self.assertIs(plist["CADisableMinimumFrameDurationOnPhone"], True)
            self.assertIn("UIApplicationSceneManifest", plist)
            self.assertEqual(len(plist["UISupportedInterfaceOrientations~ipad"]), 4, "an iPad app turns every way, so it can share the screen")
            self.assertEqual(command.call_args.args[:4], ("codesign", "--force", "--sign", "-"))

    def test_staging_again_replaces_the_old_bundle(self):
        with tempfile.TemporaryDirectory() as tmp, patch.object(ios_app, "command"):
            stage = Path(tmp) / "out/Hello.app"
            ios_app.build(stage, args(tmp))
            (stage / "stale").write_text("old")
            ios_app.build(stage, args(tmp))
            self.assertFalse((stage / "stale").exists())

    def test_the_booted_simulator_is_not_booted_again_and_the_app_is_replaced_then_launched(self):
        with tempfile.TemporaryDirectory() as tmp, patch.object(ios_app, "command") as command, \
                contextlib.redirect_stdout(io.StringIO()):
            ios_app.install_simulator(Path(tmp) / "Hello.app", args(tmp, args=["--flag"]))
            verbs = [call.args[2] for call in command.call_args_list]
            self.assertEqual(verbs, ["terminate", "install", "launch"])
            self.assertEqual(command.call_args.args[-1], "--flag")

    def test_a_named_simulator_is_booted_first_and_no_launch_stops_after_install(self):
        with tempfile.TemporaryDirectory() as tmp, patch.object(ios_app, "command") as command:
            ios_app.install_simulator(Path(tmp) / "Hello.app", args(tmp, simulator="iPhone 18 Pro", no_launch=True))
            verbs = [call.args[2] for call in command.call_args_list]
            self.assertEqual(verbs, ["boot", "bootstatus", "terminate", "install"])

    def test_a_device_bundle_carries_its_profile_and_is_signed_with_the_profiles_entitlements_for_this_app(self):
        granted = {"Entitlements": {"application-identifier": "TEAM123456.*", "com.apple.developer.team-identifier": "TEAM123456", "get-task-allow": True}}
        signed = {}

        def command(*argv, **_):
            if argv[0] == "security":
                return SimpleNamespace(stdout=plistlib.dumps(granted).decode())
            signed["identity"] = argv[argv.index("--sign") + 1]
            signed["entitlements"] = plistlib.loads(Path(argv[argv.index("--entitlements") + 1]).read_bytes())

        with tempfile.TemporaryDirectory() as tmp, patch.object(ios_app, "command", command), \
                patch.dict(ios_app.os.environ, {"IOS_APP_SIGN": "Apple Development: Someone"}):
            profile = Path(tmp) / "dev.mobileprovision"
            profile.write_text("profile")
            stage = Path(tmp) / "out/Hello.app"
            ios_app.build(stage, args(tmp, device="phone", profile=profile))
            self.assertEqual((stage / "embedded.mobileprovision").read_text(), "profile")
            self.assertEqual(signed["identity"], "Apple Development: Someone")
            self.assertEqual(signed["entitlements"]["application-identifier"], "TEAM123456.com.example.hello")
            self.assertIs(signed["entitlements"]["get-task-allow"], True)

    def test_a_device_install_replaces_the_app_and_launches_it_over_a_running_one(self):
        with tempfile.TemporaryDirectory() as tmp, patch.object(ios_app, "command") as command:
            ios_app.install_device(Path(tmp) / "Hello.app", args(tmp, device="phone"))
            install, launch = (call.args for call in command.call_args_list)
            self.assertEqual(install[3:7], ("install", "app", "--device", "phone"))
            self.assertEqual(launch[3:], ("process", "launch", "--device", "phone", "--terminate-existing", "com.example.hello"))


if __name__ == "__main__":
    unittest.main()
