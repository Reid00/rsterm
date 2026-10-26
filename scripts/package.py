#!/usr/bin/env python3
import argparse
import gzip
import hashlib
import os
from pathlib import Path
import plistlib
import re
import shutil
import stat
import subprocess
import sys
import tarfile
import tempfile
import zipfile


TARGETS = (
    "aarch64-apple-darwin",
    "x86_64-apple-darwin",
    "x86_64-pc-windows-msvc",
    "x86_64-unknown-linux-gnu",
)
MINIMUM_MACOS = "15.0"


def validate_version(version):
    match = re.fullmatch(
        r"(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)"
        r"(?:-([0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*))?"
        r"(?:\+([0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*))?",
        version,
    )
    if not match or len(version) > 128:
        raise ValueError("version must be a valid semantic version")
    if match.group(4) and any(
        part.isdigit() and len(part) > 1 and part.startswith("0")
        for part in match.group(4).split(".")
    ):
        raise ValueError("numeric prerelease identifiers cannot have leading zeroes")
    return version


def archive_name(target, version):
    if target not in TARGETS:
        raise ValueError("unsupported target: " + target)
    validate_version(version)
    suffix = ".app.zip" if target.endswith("apple-darwin") else (
        ".zip" if target.endswith("windows-msvc") else ".tar.gz"
    )
    return "rsterm-{}-{}{}".format(version, target, suffix)


def sha256(path):
    digest = hashlib.sha256()
    with path.open("rb") as source:
        for chunk in iter(lambda: source.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def write_zip(destination, files):
    with zipfile.ZipFile(destination, "w", compression=zipfile.ZIP_DEFLATED) as archive:
        for name, path, mode in files:
            info = zipfile.ZipInfo(name, date_time=(1980, 1, 1, 0, 0, 0))
            info.create_system = 3
            info.external_attr = (stat.S_IFREG | mode) << 16
            info.compress_type = zipfile.ZIP_DEFLATED
            with path.open("rb") as source, archive.open(info, "w") as output:
                shutil.copyfileobj(source, output)


def macos_files(binary, version, staging):
    if sys.platform != "darwin":
        raise OSError("macOS app signing requires a macOS host")
    app = staging / "RSTerm.app"
    contents = app / "Contents"
    executable = contents / "MacOS" / "rsterm"
    executable.parent.mkdir(parents=True)
    shutil.copyfile(binary, executable)
    executable.chmod(0o755)
    numeric_version = re.split(r"[-+]", version, maxsplit=1)[0]
    plist = contents / "Info.plist"
    with plist.open("wb") as output:
        plistlib.dump({
            "CFBundleDevelopmentRegion": "en",
            "CFBundleDisplayName": "RSTerm",
            "CFBundleExecutable": "rsterm",
            "CFBundleIdentifier": "io.github.reid00.rsterm",
            "CFBundleInfoDictionaryVersion": "6.0",
            "CFBundleName": "RSTerm",
            "CFBundlePackageType": "APPL",
            "CFBundleShortVersionString": numeric_version,
            "CFBundleVersion": numeric_version,
            "LSApplicationCategoryType": "public.app-category.developer-tools",
            "LSMinimumSystemVersion": MINIMUM_MACOS,
            "NSHighResolutionCapable": True,
        }, output)
    subprocess.run(
        ["codesign", "--force", "--sign", "-", "--timestamp=none", str(app)],
        check=True,
    )
    subprocess.run(["codesign", "--verify", "--strict", str(app)], check=True)
    signature = contents / "_CodeSignature" / "CodeResources"
    if not signature.is_file():
        raise OSError("codesign did not produce the app resource seal")
    return [
        ("RSTerm.app/Contents/Info.plist", plist, 0o644),
        ("RSTerm.app/Contents/MacOS/rsterm", executable, 0o755),
        ("RSTerm.app/Contents/_CodeSignature/CodeResources", signature, 0o644),
    ]


def package(binary, target, version, output_dir):
    binary = Path(binary)
    output_dir = Path(output_dir)
    name = archive_name(target, version)
    if binary.is_symlink() or not binary.is_file():
        raise ValueError("binary must be an existing regular file, not a symlink")
    if binary.stat().st_size == 0:
        raise ValueError("binary must not be empty")
    output_dir.mkdir(parents=True, exist_ok=True)
    destination = output_dir / name
    checksum = output_dir / (name + ".sha256")
    if os.path.lexists(destination) or os.path.lexists(checksum):
        raise FileExistsError("refusing to replace an existing package or checksum")

    with tempfile.TemporaryDirectory(prefix="rsterm-package-") as temporary:
        staging = Path(temporary)
        archive = staging / name
        if target.endswith("apple-darwin"):
            write_zip(archive, macos_files(binary, version, staging))
        elif target.endswith("windows-msvc"):
            write_zip(archive, [("rsterm.exe", binary, 0o755)])
        else:
            with archive.open("wb") as output, gzip.GzipFile(
                filename="", mode="wb", fileobj=output, mtime=0
            ) as compressed, tarfile.open(fileobj=compressed, mode="w") as tar:
                info = tarfile.TarInfo("rsterm")
                info.size = binary.stat().st_size
                info.mode = 0o755
                with binary.open("rb") as source:
                    tar.addfile(info, source)
        digest = sha256(archive)
        created = []
        try:
            with destination.open("xb") as output:
                created.append(destination)
                with archive.open("rb") as source:
                    shutil.copyfileobj(source, output)
            with checksum.open("x", encoding="ascii", newline="\n") as output:
                created.append(checksum)
                output.write("{}  {}\n".format(digest, name))
        except OSError:
            for path in created:
                path.unlink()
            raise
    return destination


def main():
    parser = argparse.ArgumentParser(description="Package a built RSTerm release binary")
    parser.add_argument("--target", required=True, choices=TARGETS)
    parser.add_argument("--version", required=True)
    parser.add_argument("--binary", required=True, type=Path)
    parser.add_argument("--output-dir", required=True, type=Path)
    args = parser.parse_args()
    try:
        result = package(args.binary, args.target, args.version, args.output_dir)
    except (OSError, ValueError, subprocess.CalledProcessError) as error:
        parser.exit(1, "Packaging failed: {}\n".format(error))
    print(result)
    print(str(result) + ".sha256")


if __name__ == "__main__":
    main()
