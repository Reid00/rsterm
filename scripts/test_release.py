import hashlib
import json
import os
from pathlib import Path
import runpy
import subprocess
import sys
import tempfile
import unittest
from unittest import mock

import package
import release


SHA = "a" * 40
REPO = "fixture/rsterm"
TAG = "v0.0.1"


class SourceTests(unittest.TestCase):
    def test_manual_release_builds_explicit_tag_not_main(self):
        selected = release.select_source("workflow_dispatch", "refs/heads/main", SHA, TAG)
        self.assertEqual(selected, {
            "source_ref": "refs/tags/" + TAG,
            "release_tag": TAG,
            "version_tag": TAG,
            "expected_sha": "",
        })

    def test_manual_without_input_only_builds_artifacts(self):
        for ref in ("refs/heads/main", "refs/tags/" + TAG):
            selected = release.select_source("workflow_dispatch", ref, SHA)
            self.assertEqual(selected["source_ref"], SHA)
            self.assertEqual(selected["release_tag"], "")
            self.assertEqual(selected["expected_sha"], SHA)

    def test_push_tag_and_published_release_use_exact_tag(self):
        for event in ("push", "release"):
            selected = release.select_source(event, "refs/tags/" + TAG, SHA, event_tag=TAG)
            self.assertEqual(selected["source_ref"], "refs/tags/" + TAG)
            self.assertEqual(selected["release_tag"], TAG)
            self.assertEqual(selected["expected_sha"], SHA)

    def test_pr_and_branch_use_trigger_sha_without_publication(self):
        for event, ref in (("pull_request", "refs/pull/7/merge"), ("push", "refs/heads/main")):
            selected = release.select_source(event, ref, SHA, TAG, TAG)
            self.assertEqual(selected["source_ref"], SHA)
            self.assertEqual(selected["release_tag"], "")

    def test_invalid_release_tags_and_commits_fail(self):
        for tag in ("", "0.0.1", "v01.0.0", "v0.0.1\n", "v0.0.1/branch", "v0.0.1;false"):
            with self.subTest(tag=tag), self.assertRaises(ValueError):
                release.validate_tag(tag)
        with self.assertRaisesRegex(ValueError, "ref does not match"):
            release.select_source("release", "refs/heads/main", SHA, event_tag=TAG)
        for commit in ("main", "a" * 7, SHA + "\n"):
            with self.assertRaises(ValueError):
                release.select_source("push", "refs/heads/main", commit)

    def test_metadata_checks_version_toolchain_and_trigger_commit(self):
        with mock.patch.object(release.subprocess, "run") as run:
            run.return_value = subprocess.CompletedProcess([], 0, SHA + "\n")
            self.assertEqual(release.source_commit(
                Path("fixture"), "0.0.1", "1.99.1", SHA, TAG, "1.99.1",
            ), SHA)
            with self.assertRaisesRegex(ValueError, "package version"):
                release.source_commit(Path("fixture"), "0.0.2", "1.99.1", SHA, TAG, "1.99.1")
            with self.assertRaisesRegex(ValueError, "toolchain"):
                release.source_commit(Path("fixture"), "0.0.1", "1.98.0", SHA, TAG, "1.99.1")
            with self.assertRaisesRegex(ValueError, "triggering commit"):
                release.source_commit(Path("fixture"), "0.0.1", "1.99.1", "b" * 40, TAG, "1.99.1")

    def test_fully_qualified_tag_selects_old_commit_despite_same_named_branch(self):
        with tempfile.TemporaryDirectory(prefix="rsterm-source-test-") as temporary:
            root = Path(temporary)

            def git(*args):
                return subprocess.run(
                    ["git", "-C", str(root), *args], check=True,
                    capture_output=True, text=True,
                    env={**os.environ, "GIT_CONFIG_NOSYSTEM": "1", "GIT_CONFIG_GLOBAL": os.devnull},
                ).stdout.strip()

            git("init", "--initial-branch=main")
            (root / "fixture").write_text("old version", encoding="utf-8")
            git("add", "fixture")
            git("-c", "user.name=Fixture", "-c", "user.email=fixture@example.invalid",
                "commit", "-m", "fixture old")
            old = git("rev-parse", "HEAD")
            git("tag", TAG)
            (root / "fixture").write_text("new main", encoding="utf-8")
            git("add", "fixture")
            git("-c", "user.name=Fixture", "-c", "user.email=fixture@example.invalid",
                "commit", "-m", "fixture new")
            latest = git("rev-parse", "HEAD")
            git("branch", TAG)
            selected = release.select_source("workflow_dispatch", "refs/heads/main", latest, TAG)
            self.assertEqual(git("rev-parse", selected["source_ref"] + "^{commit}"), old)
            self.assertNotEqual(old, latest)

    def test_select_source_cli_uses_environment_without_network(self):
        with tempfile.TemporaryDirectory(prefix="rsterm-cli-test-") as temporary:
            output = Path(temporary) / "outputs"
            subprocess.run(
                [sys.executable, str(Path(release.__file__)), "select-source"], check=True,
                env={**os.environ, "GITHUB_EVENT_NAME": "workflow_dispatch",
                     "GITHUB_REF": "refs/heads/main", "GITHUB_SHA": SHA,
                     "MANUAL_RELEASE_TAG": TAG, "GITHUB_OUTPUT": str(output)},
            )
            values = dict(line.split("=", 1) for line in output.read_text().splitlines())
            self.assertEqual(values["source_ref"], "refs/tags/" + TAG)
            self.assertEqual(values["release_tag"], TAG)

    def test_metadata_cli_reads_source_manifest_and_writes_commit(self):
        try:
            import tomllib
        except ImportError:
            self.skipTest("metadata TOML parser requires Python 3.11+; CI uses 3.12")
        with tempfile.TemporaryDirectory(prefix="rsterm-metadata-cli-") as temporary:
            root = Path(temporary)
            source = root / "source"
            source.mkdir()
            (source / "Cargo.toml").write_text('[package]\nversion = "0.0.1"\n')
            (source / "rust-toolchain.toml").write_text('[toolchain]\nchannel = "1.99.1"\n')
            output = root / "outputs"
            with mock.patch.dict(os.environ, {
                "GITHUB_OUTPUT": str(output), "EXPECTED_SHA": SHA,
                "VERSION_TAG": TAG, "RUST_TOOLCHAIN": "1.99.1",
            }), mock.patch.object(sys, "argv", [str(Path(release.__file__)), "metadata", "--source", str(source)]), mock.patch(
                "subprocess.run", return_value=subprocess.CompletedProcess([], 0, SHA + "\n"),
            ):
                runpy.run_path(str(Path(release.__file__)), run_name="__main__")
            self.assertEqual(output.read_text(), "version=0.0.1\nsource_sha=" + SHA + "\n")

    def test_invalid_cli_input_exits_without_traceback_or_network(self):
        result = subprocess.run(
            [sys.executable, str(Path(release.__file__)), "select-source"],
            capture_output=True, text=True,
            env={**os.environ, "GITHUB_EVENT_NAME": "workflow_dispatch",
                 "GITHUB_REF": "refs/heads/main", "GITHUB_SHA": SHA,
                 "MANUAL_RELEASE_TAG": "v0.0.1;false"},
        )
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("Release workflow failed:", result.stderr)
        self.assertNotIn("Traceback", result.stderr)

    def test_outputs_are_single_line_and_complete(self):
        with tempfile.TemporaryDirectory(prefix="rsterm-outputs-test-") as temporary:
            output = Path(temporary) / "outputs"
            with mock.patch.dict(os.environ, {"GITHUB_OUTPUT": str(output)}):
                release.write_outputs({"source_sha": SHA, "release_tag": TAG})
            self.assertEqual(output.read_text(encoding="utf-8"),
                             "source_sha=" + SHA + "\nrelease_tag=" + TAG + "\n")


class AttachmentTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory(prefix="rsterm-release-test-")
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        self.names = [package.archive_name(target, "0.0.1") for target in package.TARGETS]
        for name in self.names:
            (self.root / name).write_bytes(("synthetic " + name).encode("ascii"))
            self.write_checksum(name)
        self.remote = {}
        self.release_record = {"id": 17, "tag_name": TAG, "draft": False, "immutable": False}
        self.exists = True
        self.commands = []
        patch = mock.patch.object(release, "gh", side_effect=self.fake_gh)
        patch.start()
        self.addCleanup(patch.stop)

    def write_checksum(self, name):
        content = (self.root / name).read_bytes()
        (self.root / (name + ".sha256")).write_text(
            hashlib.sha256(content).hexdigest() + "  " + name + "\n", encoding="ascii",
        )

    def fake_gh(self, *args):
        self.commands.append(args)
        if args[0] == "api":
            endpoint = args[-1]
            if "/git/ref/tags/" in endpoint or "/git/tags/" in endpoint:
                return json.dumps({"object": {"type": "commit", "sha": SHA}})
            if "/assets?" in endpoint:
                return json.dumps([[{"name": name} for name in self.remote]])
            return json.dumps([[self.release_record] if self.exists else []])
        if args[:2] == ("release", "download"):
            destination = Path(args[args.index("--dir") + 1])
            for index, arg in enumerate(args):
                if arg == "--pattern":
                    name = args[index + 1]
                    (destination / name).write_bytes(self.remote[name])
        elif args[:2] == ("release", "create"):
            self.exists = True
        elif args[:2] == ("release", "upload"):
            for asset in args[3:args.index("--repo")]:
                path = Path(asset)
                if path.name in self.remote:
                    raise AssertionError("attempted to replace existing asset")
                self.remote[path.name] = path.read_bytes()
        else:
            raise AssertionError("unexpected gh command: " + repr(args))
        return ""

    def attach(self):
        release.attach(self.root, "0.0.1", TAG, SHA, REPO)

    def writes(self):
        return [args for args in self.commands if args[:2] in (
            ("release", "create"), ("release", "upload"),
        )]

    def complete_pair(self, name, content=None):
        content = content if content is not None else (self.root / name).read_bytes()
        self.remote[name] = content
        self.remote[name + ".sha256"] = (
            hashlib.sha256(content).hexdigest() + "  " + name + "\n"
        ).encode("ascii")

    def test_existing_release_receives_all_eight_files_without_state_changes(self):
        before = self.release_record.copy()
        self.attach()
        self.assertEqual(len(self.remote), 8)
        self.assertEqual(self.release_record, before)
        self.assertEqual(len(self.writes()), 1)
        self.assertEqual(self.writes()[0][:3], ("release", "upload", TAG))

    def test_absent_release_creates_draft_with_verified_tag_then_uploads(self):
        self.exists = False
        self.attach()
        create, upload = self.writes()
        self.assertEqual(create[:3], ("release", "create", TAG))
        self.assertIn("--draft", create)
        self.assertIn("--verify-tag", create)
        self.assertEqual(upload[:3], ("release", "upload", TAG))
        self.assertEqual(len(self.remote), 8)

    def test_complete_existing_pairs_survive_different_rebuild_and_repeated_runs(self):
        self.complete_pair(self.names[0], b"earlier valid build with different bytes")
        self.attach()
        self.commands.clear()
        self.attach()
        self.assertEqual(self.writes(), [])
        self.assertEqual(self.remote[self.names[0]], b"earlier valid build with different bytes")

    def test_matching_partial_pairs_receive_only_missing_companions(self):
        first, second = self.names[:2]
        self.remote[first] = (self.root / first).read_bytes()
        self.remote[second + ".sha256"] = (self.root / (second + ".sha256")).read_bytes()
        self.attach()
        uploaded = self.writes()[0]
        self.assertNotIn(str(self.root / first), uploaded)
        self.assertNotIn(str(self.root / (second + ".sha256")), uploaded)
        self.assertEqual(len(self.remote), 8)

    def test_conflicting_partial_pairs_fail_before_any_upload(self):
        for checksum_only in (False, True):
            with self.subTest(checksum_only=checksum_only):
                self.remote.clear()
                self.commands.clear()
                name = self.names[-1]
                self.remote[name + (".sha256" if checksum_only else "")] = b"conflicting file"
                with self.assertRaisesRegex(ValueError, "conflicts"):
                    self.attach()
                self.assertEqual(self.writes(), [])

    def test_corrupt_existing_complete_pair_fails_without_upload(self):
        self.complete_pair(self.names[-1])
        self.remote[self.names[-1] + ".sha256"] = b"bad checksum\n"
        with self.assertRaisesRegex(ValueError, "invalid checksum"):
            self.attach()
        self.assertEqual(self.writes(), [])

    def test_immutable_release_missing_assets_fails_without_mutation(self):
        self.release_record["immutable"] = True
        with self.assertRaisesRegex(ValueError, "immutable"):
            self.attach()
        self.assertEqual(self.writes(), [])

    def test_complete_immutable_release_is_noop(self):
        self.release_record["immutable"] = True
        for name in self.names:
            self.complete_pair(name)
        self.attach()
        self.assertEqual(self.writes(), [])

    def test_unrelated_existing_asset_is_preserved(self):
        self.remote["manual-notes.txt"] = b"keep"
        self.attach()
        self.assertEqual(self.remote["manual-notes.txt"], b"keep")
        self.assertEqual(len(self.remote), 9)

    def test_invalid_local_assets_fail_before_network(self):
        checksum = self.root / (self.names[0] + ".sha256")
        checksum.write_text("invalid", encoding="ascii")
        with self.assertRaisesRegex(ValueError, "checksum mismatch"):
            self.attach()
        self.assertEqual(self.commands, [])

    def test_missing_extra_and_empty_files_fail_before_network(self):
        extra = self.root / "unexpected"
        extra.write_bytes(b"unexpected")
        with self.assertRaisesRegex(ValueError, "exactly four"):
            self.attach()
        extra.unlink()
        archive = self.root / self.names[0]
        archive.write_bytes(b"")
        with self.assertRaisesRegex(ValueError, "empty"):
            self.attach()
        archive.unlink()
        with self.assertRaisesRegex(ValueError, "exactly four"):
            self.attach()
        self.assertEqual(self.commands, [])

    @unittest.skipIf(os.name == "nt", "symlink creation needs Windows privileges")
    def test_symlink_assets_are_rejected_before_network(self):
        archive = self.root / self.names[0]
        archive.unlink()
        archive.symlink_to(self.root / self.names[1])
        with self.assertRaisesRegex(ValueError, "regular files"):
            self.attach()
        self.assertEqual(self.commands, [])

    def test_invalid_repo_tag_sha_or_version_fail_before_network(self):
        for version, tag, commit, repo in (
            ("0.0.1", TAG, SHA, "../fixture/rsterm"),
            ("0.0.1", TAG, "main", REPO),
            ("0.0.1", "v0.0.2", SHA, REPO),
            ("../0.0.1", TAG, SHA, REPO),
        ):
            with self.subTest(tag=tag, commit=commit, repo=repo), self.assertRaises(ValueError):
                release.attach(self.root, version, tag, commit, repo)
        self.assertEqual(self.commands, [])

    def test_list_failure_is_not_treated_as_absent_release(self):
        original = self.fake_gh

        def failed_list(*args):
            if args[-1].endswith("/releases?per_page=100"):
                raise subprocess.CalledProcessError(1, ["gh", *args], stderr="permission denied")
            return original(*args)

        with mock.patch.object(release, "gh", side_effect=failed_list):
            with self.assertRaises(subprocess.CalledProcessError):
                self.attach()
        self.assertEqual(self.writes(), [])

    def test_download_or_upload_failure_propagates_without_deletion(self):
        original = self.fake_gh
        for operation in ("download", "upload"):
            self.commands.clear()
            self.remote.clear()
            self.complete_pair(self.names[0])

            def fail(*args):
                if args[:2] == ("release", operation):
                    raise subprocess.CalledProcessError(1, ["gh", *args], stderr="network failed")
                return original(*args)

            with self.subTest(operation=operation), mock.patch.object(release, "gh", side_effect=fail):
                with self.assertRaises(subprocess.CalledProcessError):
                    self.attach()
            self.assertFalse(any("delete" in args or "--clobber" in args for args in self.commands))

    def test_tag_change_after_preflight_prevents_upload(self):
        with mock.patch.object(release, "verify_remote_tag", side_effect=[None, ValueError("tag moved")]):
            with self.assertRaisesRegex(ValueError, "tag moved"):
                self.attach()
        self.assertEqual(self.writes(), [])

    def test_annotated_tags_are_peeled_and_wrong_commit_fails(self):
        with mock.patch.object(release, "gh", side_effect=[
            json.dumps({"object": {"type": "tag", "sha": "b" * 40}}),
            json.dumps({"object": {"type": "commit", "sha": SHA}}),
        ]) as gh:
            release.verify_remote_tag(REPO, TAG, SHA)
            self.assertIn("/git/tags/", gh.call_args.args[-1])
        with mock.patch.object(release, "gh", return_value=json.dumps({
            "object": {"type": "commit", "sha": "b" * 40},
        })):
            with self.assertRaisesRegex(ValueError, "tag moved"):
                release.verify_remote_tag(REPO, TAG, SHA)

    def test_release_lookup_reads_all_pages(self):
        with mock.patch.object(release, "gh", return_value=json.dumps([
            [{"tag_name": "v0.0.2", "id": 18}], [self.release_record],
        ])):
            self.assertEqual(release.find_release(REPO, TAG), self.release_record)

    def test_interrupted_upload_can_resume_matching_files(self):
        original = self.fake_gh

        def interrupted(*args):
            if args[:2] == ("release", "upload"):
                path = Path(args[3])
                self.remote[path.name] = path.read_bytes()
                raise subprocess.CalledProcessError(1, ["gh", *args], stderr="interrupted")
            return original(*args)

        with mock.patch.object(release, "gh", side_effect=interrupted):
            with self.assertRaises(subprocess.CalledProcessError):
                self.attach()
        self.assertEqual(len(self.remote), 1)
        self.attach()
        self.assertEqual(len(self.remote), 8)


if __name__ == "__main__":
    unittest.main()
