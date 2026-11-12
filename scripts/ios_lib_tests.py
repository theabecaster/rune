"""Module-map and staging regressions: never runs cargo or xcodebuild."""
from pathlib import Path
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import patch

import ios_lib

RUSTC = """   Compiling demo v0.0.1
note: Link against the following native artifacts when linking against this static library.

note: native-static-libs: -lSystem -framework UIKit -framework QuartzCore -framework Foundation -lSystem -framework CoreText -framework UIKit -lc -lobjc -framework Metal -liconv -lSystem -lm

    Finished `release` profile [optimized] target(s) in 51.83s
"""


class ModuleRegression(unittest.TestCase):
    def test_links_are_read_once_each_in_order_without_what_every_app_links(self):
        self.assertEqual(ios_lib.native_libs(RUSTC), (["UIKit", "QuartzCore", "Foundation", "CoreText", "Metal"], ["iconv"]))

    def test_a_build_that_names_no_links_is_an_error(self):
        with self.assertRaises(RuntimeError):
            ios_lib.native_libs("    Finished `release` profile")

    def test_the_module_map_names_the_header_and_links_each_framework_and_library(self):
        text = ios_lib.module_map("Demo", "demo.h", ["UIKit", "Metal"], ["iconv"])
        self.assertEqual(text, 'module Demo {\n    header "demo.h"\n    link framework "UIKit"\n    link framework "Metal"\n    link "iconv"\n    export *\n}\n')

    def test_each_target_is_built_as_a_static_library_and_found_in_the_workspaces_target_folder(self):
        metadata = '{"packages": [{"name": "demo-app", "targets": [{"name": "demo-app", "kind": ["lib"]}]}], "target_directory": "/work/target"}'
        calls = []

        def command(*argv, **_):
            calls.append(argv)
            return SimpleNamespace(stdout=metadata, stderr=RUSTC)

        with patch.object(ios_lib, "command", command), patch.dict(ios_lib.os.environ, {"CARGO": "cargo"}):
            built, links = ios_lib.build(SimpleNamespace(package="demo-app", debug=False))
        self.assertEqual(built, [Path("/work/target/aarch64-apple-ios/release/libdemo_app.a"), Path("/work/target/aarch64-apple-ios-sim/release/libdemo_app.a")])
        self.assertEqual(links[1], ["iconv"])
        rustc = [argv for argv in calls if argv[1] == "rustc"]
        self.assertEqual([argv[argv.index("--target") + 1] for argv in rustc], list(ios_lib.TARGETS))
        self.assertTrue(all(argv[argv.index("--crate-type") + 1] == "staticlib" for argv in rustc))

    def test_the_framework_gets_one_slice_a_library_and_an_old_one_is_replaced(self):
        with tempfile.TemporaryDirectory() as tmp, patch.object(ios_lib, "command") as command:
            out = Path(tmp) / "ios"
            header = Path(tmp) / "demo.h"
            header.write_text("void *demo_view(void);\n")
            stale = out / "Demo.xcframework/stale"
            stale.parent.mkdir(parents=True)
            stale.write_text("old")
            seen = {}
            command.side_effect = lambda *argv, **_: seen.update(map=(Path(argv[argv.index("-headers") + 1]) / "module.modulemap").read_text())
            framework = ios_lib.assemble(out, "Demo", header, [Path("a.a"), Path("b.a")], (["Metal"], []))
            argv = command.call_args.args
            self.assertEqual(argv.count("-library"), 2)
            self.assertEqual(argv[-1], str(framework))
            self.assertIn('link framework "Metal"', seen["map"])
            self.assertFalse(stale.exists())
            self.assertFalse((out / "Demo-headers").exists())


if __name__ == "__main__":
    unittest.main()
