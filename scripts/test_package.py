import hashlib
from pathlib import Path
import plistlib
import stat
import subprocess
import sys
import tarfile
import tempfile
import unittest
from unittest import mock
import zipfile

import package


class PackageTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix="rsterm-package-test-")
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.binary = self.root / "synthetic binary"
        self.binary.write_bytes(b"synthetic executable\x00\xff\n")
        self.output = self.root / "dist"
        self.version = "1.2.3"

    def assert_checksum(self, archive):
        expected = hashlib.sha256(archive.read_bytes()).hexdigest()
        self.assertEqual(
            Path(str(archive) + ".sha256").read_text(encoding="ascii"),
            "{}  {}\n".format(expected, archive.name),
        )

    def fake_codesign(self, command, **kwargs):
        self.assertTrue(kwargs["check"])
        app = Path(command[-1])
        if "--sign" in command:
            signature = app / "Contents" / "_CodeSignature" / "CodeResources"
            signature.parent.mkdir()
            signature.write_bytes(b"synthetic resource seal")
        return subprocess.CompletedProcess(command, 0)

    def test_macos_architectures_and_bundle_contents(self):
        for target in ("aarch64-apple-darwin", "x86_64-apple-darwin"):
            with self.subTest(target=target), mock.patch.object(
                package.sys, "platform", "darwin"
            ), mock.patch.object(
                package.subprocess, "run", side_effect=self.fake_codesign
            ) as codesign:
                archive = package.package(self.binary, target, self.version, self.output)
                self.assertEqual(archive.name, "rsterm-1.2.3-" + target + ".app.zip")
                with zipfile.ZipFile(archive) as content:
                    self.assertEqual(set(content.namelist()), {
                        "RSTerm.app/Contents/MacOS/rsterm",
                        "RSTerm.app/Contents/Info.plist",
                        "RSTerm.app/Contents/_CodeSignature/CodeResources",
                    })
                    executable = content.getinfo("RSTerm.app/Contents/MacOS/rsterm")
                    self.assertEqual(stat.S_IMODE(executable.external_attr >> 16), 0o755)
                    self.assertEqual(content.read(executable), self.binary.read_bytes())
                    plist = plistlib.loads(content.read("RSTerm.app/Contents/Info.plist"))
                    self.assertEqual(plist["CFBundleExecutable"], "rsterm")
                    self.assertEqual(plist["CFBundleIdentifier"], "io.github.reid00.rsterm")
                    self.assertEqual(plist["CFBundlePackageType"], "APPL")
                    self.assertEqual(plist["CFBundleShortVersionString"], self.version)
                    self.assertEqual(plist["CFBundleVersion"], self.version)
                    self.assertEqual(plist["LSMinimumSystemVersion"], "15.0")
                self.assertEqual(codesign.call_count, 2)
                self.assertIn("--sign", codesign.call_args_list[0].args[0])
                self.assertIn("--verify", codesign.call_args_list[1].args[0])
                self.assert_checksum(archive)

    def test_windows_zip_contains_only_executable(self):
        (self.root / "credentials.json").write_text("synthetic secret", encoding="utf-8")
        archive = package.package(
            self.binary, "x86_64-pc-windows-msvc", self.version, self.output
        )
        self.assertEqual(archive.name, "rsterm-1.2.3-x86_64-pc-windows-msvc.zip")
        with zipfile.ZipFile(archive) as content:
            self.assertEqual(content.namelist(), ["rsterm.exe"])
            self.assertEqual(content.read("rsterm.exe"), self.binary.read_bytes())
        self.assert_checksum(archive)

    def test_linux_tar_contains_executable_and_permissions(self):
        archive = package.package(
            self.binary, "x86_64-unknown-linux-gnu", self.version, self.output
        )
        self.assertEqual(archive.name, "rsterm-1.2.3-x86_64-unknown-linux-gnu.tar.gz")
        with tarfile.open(archive, "r:gz") as content:
            self.assertEqual(content.getnames(), ["rsterm"])
            executable = content.getmember("rsterm")
            self.assertTrue(executable.isfile())
            self.assertEqual(executable.mode, 0o755)
            self.assertEqual(content.extractfile(executable).read(), self.binary.read_bytes())
        self.assert_checksum(archive)

    def test_archives_are_repeatable_for_unsigned_platforms(self):
        for target in ("x86_64-pc-windows-msvc", "x86_64-unknown-linux-gnu"):
            with self.subTest(target=target):
                first = package.package(self.binary, target, self.version, self.root / "first")
                second = package.package(self.binary, target, self.version, self.root / "second")
                self.assertEqual(first.read_bytes(), second.read_bytes())

    def test_version_accepts_semver(self):
        for version in ("0.0.1", "0.1.0", "1.2.3-rc.1", "1.2.3+build.01", "1.2.3-rc.1+build.2"):
            with self.subTest(version=version):
                self.assertEqual(package.validate_version(version), version)

    def test_version_rejects_paths_and_invalid_semver(self):
        for version in (
            "", "v1.2.3", "1.2", "01.2.3", "1.02.3", "1.2.03", "1.2.3-01",
            "1.2.3-rc..1", "1.2.3\n", "../1.2.3", "1.2.3/foo", "1.2.3\\foo",
            "1.2.3;touch bad", "1.2.3+" + "a" * 129,
        ):
            with self.subTest(version=version), self.assertRaises(ValueError):
                package.package(self.binary, "x86_64-unknown-linux-gnu", version, self.output)
        self.assertFalse(self.output.exists())

    def test_unsupported_target_fails_before_output(self):
        for target in ("aarch64-unknown-linux-gnu", "../target", "windows", ""):
            with self.subTest(target=target), self.assertRaises(ValueError):
                package.package(self.binary, target, self.version, self.output)
        self.assertFalse(self.output.exists())

    def test_missing_empty_or_directory_binary_fails(self):
        empty = self.root / "empty"
        empty.touch()
        for binary in (self.root / "missing", empty, self.root):
            with self.subTest(binary=binary), self.assertRaises(ValueError):
                package.package(binary, "x86_64-unknown-linux-gnu", self.version, self.output)
        self.assertFalse(self.output.exists())

    @unittest.skipIf(sys.platform == "win32", "symlink creation needs Windows privileges")
    def test_symlink_binary_fails(self):
        link = self.root / "link"
        link.symlink_to(self.binary)
        with self.assertRaises(ValueError):
            package.package(link, "x86_64-unknown-linux-gnu", self.version, self.output)
        self.assertFalse(self.output.exists())

    def test_existing_archive_is_never_replaced(self):
        self.output.mkdir()
        name = package.archive_name("x86_64-unknown-linux-gnu", self.version)
        existing = self.output / name
        existing.write_bytes(b"keep existing archive")
        with self.assertRaises(FileExistsError):
            package.package(self.binary, "x86_64-unknown-linux-gnu", self.version, self.output)
        self.assertEqual(existing.read_bytes(), b"keep existing archive")
        self.assertEqual(list(self.output.iterdir()), [existing])

    def test_existing_checksum_is_never_replaced(self):
        self.output.mkdir()
        name = package.archive_name("x86_64-unknown-linux-gnu", self.version)
        checksum = self.output / (name + ".sha256")
        checksum.write_text("keep existing checksum", encoding="ascii")
        with self.assertRaises(FileExistsError):
            package.package(self.binary, "x86_64-unknown-linux-gnu", self.version, self.output)
        self.assertEqual(checksum.read_text(encoding="ascii"), "keep existing checksum")
        self.assertEqual(list(self.output.iterdir()), [checksum])

    def test_different_targets_do_not_collide(self):
        first = package.package(
            self.binary, "x86_64-pc-windows-msvc", self.version, self.output
        )
        second = package.package(
            self.binary, "x86_64-unknown-linux-gnu", self.version, self.output
        )
        self.assertNotEqual(first, second)
        self.assertEqual(len(list(self.output.iterdir())), 4)

    def test_failed_codesign_does_not_publish_package(self):
        with mock.patch.object(package.sys, "platform", "darwin"), mock.patch.object(
            package.subprocess, "run", side_effect=subprocess.CalledProcessError(1, "codesign")
        ), self.assertRaises(subprocess.CalledProcessError):
            package.package(self.binary, "aarch64-apple-darwin", self.version, self.output)
        self.assertEqual(list(self.output.iterdir()), [])

    def test_missing_resource_seal_does_not_publish_package(self):
        with mock.patch.object(package.sys, "platform", "darwin"), mock.patch.object(
            package.subprocess, "run"
        ), self.assertRaises(OSError):
            package.package(self.binary, "aarch64-apple-darwin", self.version, self.output)
        self.assertEqual(list(self.output.iterdir()), [])

    def test_macos_package_requires_macos_host(self):
        with mock.patch.object(package.sys, "platform", "linux"), self.assertRaises(OSError):
            package.package(self.binary, "aarch64-apple-darwin", self.version, self.output)
        self.assertEqual(list(self.output.iterdir()), [])

    def test_cli_error_has_no_traceback(self):
        result = subprocess.run(
            [sys.executable, str(Path(package.__file__)), "--target", "x86_64-pc-windows-msvc",
             "--version", self.version, "--binary", str(self.root / "missing"),
             "--output-dir", str(self.output)],
            capture_output=True, text=True,
        )
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("Packaging failed:", result.stderr)
        self.assertNotIn("Traceback", result.stderr)


if __name__ == "__main__":
    unittest.main()
