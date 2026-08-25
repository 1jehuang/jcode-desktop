#!/usr/bin/env python3
import argparse, pathlib, tarfile, zipfile

EXPECTED_UNIX = {"jcode-desktop", "jcode", "jcode-harness-api-bridge", "jcode.desktop", "jcode.png"}
EXPECTED_WINDOWS = {"jcode-desktop.exe", "jcode.exe", "jcode-harness-api-bridge.exe", "Jcode.png", "jcode-desktop.exe.manifest"}

def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("artifact", type=pathlib.Path)
    args = parser.parse_args()
    artifact = args.artifact
    if artifact.name.endswith(".tar.gz"):
        with tarfile.open(artifact) as archive:
            names = {pathlib.PurePosixPath(n).name for n in archive.getnames() if n}
        missing = EXPECTED_UNIX - names
    elif artifact.suffix == ".zip":
        with zipfile.ZipFile(artifact) as archive:
            names = {pathlib.PurePosixPath(n).name for n in archive.namelist() if n}
        missing = EXPECTED_WINDOWS - names
    else:
        parser.error("expected .tar.gz or .zip")
    if missing:
        raise SystemExit(f"{artifact}: missing {', '.join(sorted(missing))}")
    print(f"verified {artifact}")

if __name__ == "__main__": main()
