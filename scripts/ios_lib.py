"""Build a Rune app's library for iOS as an XCFramework a UIKit or SwiftUI app imports as a module."""
import argparse
import json
import os
from pathlib import Path
import re
import shutil
import sys

from mac_app_install import command

# A release build of Rune from cold.
BUILD = 3600
# A device and an Apple silicon Mac's simulator. An XCFramework holds one library per platform.
TARGETS = ("aarch64-apple-ios", "aarch64-apple-ios-sim")


def native_libs(build_output):
    """The frameworks and libraries rustc says a static library needs, in order, without repeats.

    libSystem, libc, libm and libobjc are left out: every iOS app links them already.
    """
    line = re.search(r"native-static-libs: (.*)", build_output)
    if not line:
        raise RuntimeError("rustc did not say what the static library links (native-static-libs)")
    words = line[1].split()
    frameworks = [words[i + 1] for i, word in enumerate(words[:-1]) if word == "-framework"]
    libraries = [word[2:] for i, word in enumerate(words) if word.startswith("-l") and (i == 0 or words[i - 1] != "-framework")]
    libraries = [name for name in libraries if name not in ("System", "c", "m", "objc")]
    return list(dict.fromkeys(frameworks)), list(dict.fromkeys(libraries))


def module_map(module, header, frameworks, libraries):
    """A Clang module of the app's C header that links what the Rust library needs, so the host
    app imports the module and adds no linker flags of its own."""
    links = [f'    link framework "{name}"' for name in frameworks] + [f'    link "{name}"' for name in libraries]
    return "\n".join([f"module {module} {{", f'    header "{header}"', *links, "    export *", "}", ""])


def library(package):
    """The package's library target's file name, and the workspace's target folder."""
    meta = json.loads(command(cargo(), "metadata", "--format-version", "1", "--no-deps", timeout=BUILD).stdout)
    found = next((p for p in meta["packages"] if p["name"] == package), None)
    if found is None:
        raise RuntimeError(f"{package} is not a workspace member")
    target = next((t for t in found["targets"] if "lib" in t["kind"] or "staticlib" in t["kind"] or "cdylib" in t["kind"]), None)
    if target is None:
        raise RuntimeError(f"{package} has no library target")
    return f"lib{target['name'].replace('-', '_')}.a", Path(meta["target_directory"])


def cargo():
    return os.environ.get("CARGO") or "cargo"


def build(args):
    """Build the static library for each target; returns each one's path and what they link."""
    name, target_dir = library(args.package)
    profile = ("--release",) if not args.debug else ()
    built, links = [], None
    for target in TARGETS:
        # `cargo rustc --crate-type` builds a static library without the package asking for one
        # in its manifest, where it would be built for every other target too.
        output = command(cargo(), "rustc", "-p", args.package, "--lib", "--crate-type", "staticlib", "--target", target,
                         *profile, "--", "--print", "native-static-libs", timeout=BUILD).stderr
        links = native_libs(output)
        built.append(target_dir / target / ("debug" if args.debug else "release") / name)
    return built, links


def assemble(out, module, header, libraries, links):
    """`<out>/<module>.xcframework` from the built libraries, each with the header and module map."""
    headers = out / f"{module}-headers"
    if headers.exists():
        shutil.rmtree(headers)
    headers.mkdir(parents=True)
    shutil.copy2(header, headers / header.name)
    (headers / "module.modulemap").write_text(module_map(module, header.name, *links))
    framework = out / f"{module}.xcframework"
    if framework.exists():
        shutil.rmtree(framework)
    slices = [arg for path in libraries for arg in ("-library", str(path), "-headers", str(headers))]
    command("xcodebuild", "-create-xcframework", *slices, "-output", str(framework), timeout=BUILD)
    shutil.rmtree(headers)
    return framework


def main():
    parser = argparse.ArgumentParser(prog="ios-lib", description=__doc__)
    parser.add_argument("--package", required=True, help="the workspace package whose library exports the views")
    parser.add_argument("--module", required=True, help="the module's name in Swift (import MODULE)")
    parser.add_argument("--header", required=True, type=Path, help="a C header declaring the exported functions")
    parser.add_argument("--out", type=Path, default=Path("target/ios"), help="where the XCFramework is written")
    parser.add_argument("--debug", action="store_true", help="build the debug profile, not release")
    args = parser.parse_args()
    if not args.header.is_file():
        parser.error(f"no header at {args.header}")
    libraries, links = build(args)
    print(assemble(args.out, args.module, args.header, libraries, links))


if __name__ == "__main__":
    try:
        main()
    except Exception as error:
        detail = getattr(error, "stderr", None)
        print(f"ios-lib: {error}" + (f"\n{detail.strip()[-2000:]}" if detail else ""), file=sys.stderr)
        sys.exit(1)
