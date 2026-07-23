#!/usr/bin/env python3

import argparse
import gzip
import io
import tarfile
import zipfile
from pathlib import Path


def arguments() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--target", required=True)
    parser.add_argument("--version", required=True)
    parser.add_argument("--format", choices=("tar.gz", "zip"), required=True)
    parser.add_argument("--output-dir", type=Path, required=True)
    return parser.parse_args()


def archive_files(binary: Path) -> list[tuple[str, bytes, int]]:
    return [
        (binary.name, binary.read_bytes(), 0o755),
        ("README.md", Path("README.md").read_bytes(), 0o644),
        ("LICENSE-MIT", Path("LICENSE-MIT").read_bytes(), 0o644),
        ("LICENSE-APACHE", Path("LICENSE-APACHE").read_bytes(), 0o644),
    ]


def tar_info(name: str, mode: int, size: int = 0) -> tarfile.TarInfo:
    info = tarfile.TarInfo(name)
    info.mode = mode
    info.size = size
    info.mtime = 0
    info.uid = 0
    info.gid = 0
    info.uname = ""
    info.gname = ""
    return info


def write_tar_gz(
    output: Path, package: str, files: list[tuple[str, bytes, int]]
) -> None:
    with output.open("wb") as raw_archive:
        with gzip.GzipFile(
            filename="", mode="wb", fileobj=raw_archive, mtime=0
        ) as compressed:
            with tarfile.open(
                fileobj=compressed, mode="w", format=tarfile.USTAR_FORMAT
            ) as archive:
                directory = tar_info(f"{package}/", 0o755)
                directory.type = tarfile.DIRTYPE
                archive.addfile(directory)
                for name, contents, mode in files:
                    info = tar_info(f"{package}/{name}", mode, len(contents))
                    archive.addfile(info, fileobj=io.BytesIO(contents))


def zip_info(name: str, mode: int) -> zipfile.ZipInfo:
    info = zipfile.ZipInfo(name, date_time=(1980, 1, 1, 0, 0, 0))
    info.compress_type = zipfile.ZIP_STORED
    info.create_system = 3
    info.external_attr = mode << 16
    return info


def write_zip(output: Path, package: str, files: list[tuple[str, bytes, int]]) -> None:
    with zipfile.ZipFile(output, mode="w") as archive:
        directory = zip_info(f"{package}/", 0o40755)
        archive.writestr(directory, b"")
        for name, contents, mode in files:
            archive.writestr(zip_info(f"{package}/{name}", 0o100000 | mode), contents)

def main() -> None:
    options = arguments()
    package = f"skill-manager-v{options.version}-{options.target}"
    options.output_dir.mkdir(parents=True, exist_ok=True)
    files = archive_files(options.binary)

    if options.format == "tar.gz":
        output = options.output_dir / f"{package}.tar.gz"
        write_tar_gz(output, package, files)
    else:
        output = options.output_dir / f"{package}.zip"
        write_zip(output, package, files)

    print(output.as_posix())


if __name__ == "__main__":
    main()
