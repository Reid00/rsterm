#!/usr/bin/env python3
import argparse
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import tempfile

from package import TARGETS, archive_name, sha256, validate_version


def validate_tag(tag):
    if not tag.startswith("v"):
        raise ValueError("release tag must start with v")
    validate_version(tag[1:])
    return tag


def select_source(event, ref, commit, manual_tag="", event_tag=""):
    tag = ""
    if event == "release":
        tag = validate_tag(event_tag)
        if ref != "refs/tags/" + tag:
            raise ValueError("release event ref does not match its tag")
    elif event == "workflow_dispatch" and manual_tag:
        tag = validate_tag(manual_tag)
    elif event == "push" and ref.startswith("refs/tags/"):
        tag = validate_tag(ref.removeprefix("refs/tags/"))
    if not re.fullmatch(r"[0-9a-f]{40}", commit):
        raise ValueError("expected a full source commit SHA")
    version_tag = tag or (ref[len("refs/tags/"):] if ref.startswith("refs/tags/") else "")
    if version_tag:
        validate_tag(version_tag)
    return {
        "source_ref": "refs/tags/" + tag if tag else commit,
        "release_tag": tag,
        "version_tag": version_tag,
        "expected_sha": "" if event == "workflow_dispatch" and manual_tag else commit,
    }


def source_commit(source, version, toolchain, expected_sha, tag, expected_toolchain):
    validate_version(version)
    if tag and validate_tag(tag) != "v" + version:
        raise ValueError("release tag must equal v + Cargo.toml package version")
    if toolchain != expected_toolchain:
        raise ValueError("workflow and rust-toolchain.toml versions differ")
    commit = subprocess.run(
        ["git", "-C", str(source), "rev-parse", "HEAD^{commit}"],
        check=True, capture_output=True, text=True,
    ).stdout.strip()
    if expected_sha and commit != expected_sha:
        raise ValueError("source tag no longer matches the triggering commit")
    return commit


def validate_assets(root, version, tag):
    if validate_tag(tag) != "v" + validate_version(version):
        raise ValueError("release tag does not match package version")
    names = [archive_name(target, version) for target in TARGETS]
    expected = set(names + [name + ".sha256" for name in names])
    if {path.name for path in root.iterdir()} != expected:
        raise ValueError("expected exactly four archives and four checksums")
    for name in names:
        archive = root / name
        checksum = root / (name + ".sha256")
        if any(path.is_symlink() or not path.is_file() for path in (archive, checksum)):
            raise ValueError("release assets must be regular files")
        if not archive.stat().st_size:
            raise ValueError("release archives must not be empty")
        if checksum.read_text(encoding="ascii") != sha256(archive) + "  " + name + "\n":
            raise ValueError("checksum mismatch: " + name)
    return names


def gh(*args):
    return subprocess.run(
        ["gh", *args], check=True, capture_output=True, text=True,
    ).stdout


def verify_remote_tag(repo, tag, commit):
    reference = json.loads(gh("api", "repos/" + repo + "/git/ref/tags/" + tag))["object"]
    while reference["type"] == "tag":
        reference = json.loads(gh(
            "api", "repos/" + repo + "/git/tags/" + reference["sha"],
        ))["object"]
    if reference["type"] != "commit" or reference["sha"] != commit:
        raise ValueError("remote release tag moved after the build; refusing to upload")


def find_release(repo, tag):
    pages = json.loads(gh("api", "--paginate", "--slurp", "repos/" + repo + "/releases?per_page=100"))
    return next((item for page in pages for item in page if item["tag_name"] == tag), None)


def pending_assets(repo, tag, root, names, release):
    pages = json.loads(gh(
        "api", "--paginate", "--slurp",
        "repos/" + repo + "/releases/" + str(release["id"]) + "/assets?per_page=100",
    ))
    existing = {asset["name"] for page in pages for asset in page}
    pending = []
    with tempfile.TemporaryDirectory(prefix="rsterm-release-check-") as temporary:
        downloaded = Path(temporary)
        for name in names:
            checksum_name = name + ".sha256"
            present = [item for item in (name, checksum_name) if item in existing]
            if not present:
                pending.extend([root / name, root / checksum_name])
                continue
            command = ["release", "download", tag, "--repo", repo, "--dir", str(downloaded)]
            for item in present:
                command.extend(["--pattern", item])
            gh(*command)
            for item in present:
                path = downloaded / item
                if path.is_symlink() or not path.is_file():
                    raise ValueError("downloaded release asset is not a regular file: " + item)
            if len(present) == 2:
                if (downloaded / checksum_name).read_text(encoding="ascii") != (
                    sha256(downloaded / name) + "  " + name + "\n"
                ):
                    raise ValueError("existing release pair has an invalid checksum: " + name)
                print("Keeping existing verified assets: " + name)
            elif name in existing:
                if sha256(downloaded / name) != sha256(root / name):
                    raise ValueError("existing archive conflicts with this build: " + name)
                pending.append(root / checksum_name)
            else:
                if (downloaded / checksum_name).read_bytes() != (root / checksum_name).read_bytes():
                    raise ValueError("existing checksum conflicts with this build: " + name)
                pending.append(root / name)
    if pending and release.get("immutable", False):
        raise ValueError("release is immutable; attach binaries to a draft before publishing")
    return pending


def attach(root, version, tag, commit, repo):
    if not re.fullmatch(r"[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+", repo):
        raise ValueError("expected an owner/repository name")
    if not re.fullmatch(r"[0-9a-f]{40}", commit):
        raise ValueError("expected a full source commit SHA")
    names = validate_assets(root, version, tag)
    verify_remote_tag(repo, tag, commit)
    release = find_release(repo, tag)
    if release is None:
        pending = [root / item for name in names for item in (name, name + ".sha256")]
    else:
        pending = pending_assets(repo, tag, root, names, release)
    verify_remote_tag(repo, tag, commit)
    if release is None:
        gh(
            "release", "create", tag, "--repo", repo, "--draft", "--verify-tag",
            "--title", "RSTerm " + version,
            "--notes", "Native desktop binaries and SHA-256 checksums. macOS apps are ad-hoc signed, not Developer ID signed or notarized. Linux binaries require a compatible GNU desktop runtime. Review and test all platform downloads before publishing.",
        )
    if pending:
        gh("release", "upload", tag, *map(str, pending), "--repo", repo)
    print("Release assets are complete for " + tag)


def write_outputs(values):
    with open(os.environ["GITHUB_OUTPUT"], "a", encoding="utf-8") as output:
        for key, value in values.items():
            output.write(key + "=" + value + "\n")


def main():
    parser = argparse.ArgumentParser(description="Validate builds and attach RSTerm release binaries")
    commands = parser.add_subparsers(dest="command", required=True)
    commands.add_parser("select-source")
    metadata = commands.add_parser("metadata")
    metadata.add_argument("--source", type=Path, required=True)
    publish = commands.add_parser("attach")
    publish.add_argument("--packages", type=Path, required=True)
    args = parser.parse_args()
    try:
        if args.command == "select-source":
            write_outputs(select_source(
                os.environ["GITHUB_EVENT_NAME"], os.environ["GITHUB_REF"], os.environ["GITHUB_SHA"],
                os.environ.get("MANUAL_RELEASE_TAG", ""), os.environ.get("EVENT_RELEASE_TAG", ""),
            ))
        elif args.command == "metadata":
            import tomllib
            manifest = tomllib.loads((args.source / "Cargo.toml").read_text(encoding="utf-8"))
            toolchain = tomllib.loads((args.source / "rust-toolchain.toml").read_text(encoding="utf-8"))
            version = manifest["package"]["version"]
            commit = source_commit(
                args.source, version, toolchain["toolchain"]["channel"],
                os.environ["EXPECTED_SHA"], os.environ["VERSION_TAG"], os.environ["RUST_TOOLCHAIN"],
            )
            write_outputs({"version": version, "source_sha": commit})
        else:
            attach(
                args.packages, os.environ["PACKAGE_VERSION"], os.environ["RELEASE_TAG"],
                os.environ["SOURCE_SHA"], os.environ["GH_REPO"],
            )
    except (OSError, ValueError, KeyError, subprocess.CalledProcessError) as error:
        if isinstance(error, subprocess.CalledProcessError) and error.stderr:
            print(error.stderr.strip(), file=sys.stderr)
        parser.exit(1, "Release workflow failed: {}\n".format(error))


if __name__ == "__main__":
    main()
