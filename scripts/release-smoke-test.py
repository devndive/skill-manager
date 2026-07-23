#!/usr/bin/env python3

import argparse
import json
import os
import subprocess
import tarfile
import tempfile
import zipfile
from pathlib import Path


def arguments() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("--archive", type=Path, required=True)
    parser.add_argument("--binary", required=True)
    parser.add_argument("--version", required=True)
    return parser.parse_args()


def extract_archive(archive: Path, destination: Path) -> None:
    if archive.suffix == ".zip":
        with zipfile.ZipFile(archive) as package:
            package.extractall(destination)
            if os.name != "nt":
                for member in package.infolist():
                    mode = member.external_attr >> 16
                    if mode:
                        (destination / member.filename).chmod(mode)
    else:
        with tarfile.open(archive, mode="r:gz") as package:
            package.extractall(destination)


def run(command: list[Path | str], **options: object) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        [str(argument) for argument in command],
        check=True,
        text=True,
        **options,
    )


def main() -> None:
    options = arguments()
    archive = options.archive.resolve()

    with tempfile.TemporaryDirectory() as work_directory:
        work = Path(work_directory)
        extracted = work / "extracted"
        extracted.mkdir()
        extract_archive(archive, extracted)

        binaries = list(extracted.rglob(options.binary))
        if len(binaries) != 1:
            raise RuntimeError(
                f"expected one packaged {options.binary} binary, found {len(binaries)}"
            )
        binary = binaries[0]

        run([binary, "--help"], stdout=subprocess.DEVNULL)
        version = run([binary, "--version"], capture_output=True).stdout.strip()
        if version != f"skill-manager {options.version}":
            raise RuntimeError(f"packaged binary reported unexpected version {version!r}")

        repository = work / "source-repository"
        run(["git", "init", "--quiet", repository])
        run(["git", "-C", repository, "config", "user.email", "release-test@example.com"])
        run(["git", "-C", repository, "config", "user.name", "Release Test"])
        (repository / "SKILL.md").write_text("# Release smoke test\n")
        run(["git", "-C", repository, "add", "SKILL.md"])
        run(["git", "-C", repository, "commit", "--quiet", "-m", "add root Skill"])

        output = run(
            [binary, "discover", repository, "--json"], capture_output=True
        ).stdout
        discovery = json.loads(output)
        if discovery["schema_version"] != 1:
            raise RuntimeError("unexpected discovery schema version")
        if len(discovery["skills"]) != 1:
            raise RuntimeError("unexpected number of discovered Skills")
        if discovery["skills"][0]["path"] != ".":
            raise RuntimeError("root Skill was not discovered")
        if discovery["skills"][0]["name"] != "source-repository":
            raise RuntimeError(
                "root Skill name was not derived from the Source Repository"
            )


if __name__ == "__main__":
    main()
