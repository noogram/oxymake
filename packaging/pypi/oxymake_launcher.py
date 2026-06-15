"""Thin launcher for the OxyMake `ox` binary.

Installed via `uv tool install oxymake` / `pipx install oxymake`, this module
exposes the `ox` and `oxymake` console scripts. On first invocation it downloads
the prebuilt binary matching the host platform from the GitHub release, verifies
its SHA-256 against the `.sha256` sidecar, caches it under the user cache dir,
and execs it. No Rust toolchain is required.

This keeps the conda/pip onboarding path open: the audience that lives in Python
never has to `cargo install` from source.
"""

from __future__ import annotations

import hashlib
import os
import platform
import stat
import sys
import tarfile
import tempfile
import urllib.request
from pathlib import Path

__version__ = "0.1.0"

_REPO = "noogram/oxymake"
_BASE = f"https://github.com/{_REPO}/releases/download/v{__version__}"

# (sys.platform startswith, machine) -> release target triple
_TARGETS = {
    ("darwin", "arm64"): "aarch64-apple-darwin",
    ("darwin", "x86_64"): "x86_64-apple-darwin",
    ("linux", "x86_64"): "x86_64-unknown-linux-gnu",
    ("linux", "amd64"): "x86_64-unknown-linux-gnu",
}


def _target() -> str:
    key = (sys.platform, platform.machine().lower())
    target = _TARGETS.get(key)
    if target is None:
        raise SystemExit(
            f"oxymake: no prebuilt binary for {key}. "
            f"Build from source: https://github.com/{_REPO}#install"
        )
    return target


def _cache_dir() -> Path:
    root = os.environ.get("XDG_CACHE_HOME") or str(Path.home() / ".cache")
    d = Path(root) / "oxymake" / __version__
    d.mkdir(parents=True, exist_ok=True)
    return d


def _download(url: str, dest: Path) -> None:
    with urllib.request.urlopen(url) as resp, open(dest, "wb") as fh:  # noqa: S310
        fh.write(resp.read())


def _verify(tarball: Path, target: str) -> None:
    """Verify the tarball against its published .sha256 sidecar."""
    expected = _download_text(f"{_BASE}/ox-{target}.tar.gz.sha256").split()[0].strip()
    actual = hashlib.sha256(tarball.read_bytes()).hexdigest()
    if actual != expected:
        raise SystemExit(
            f"oxymake: checksum mismatch for ox-{target}.tar.gz "
            f"(expected {expected}, got {actual}). Refusing to run."
        )


def _download_text(url: str) -> str:
    with urllib.request.urlopen(url) as resp:  # noqa: S310
        return resp.read().decode()


def _ensure_binary() -> Path:
    target = _target()
    binary = _cache_dir() / "ox"
    if binary.exists():
        return binary

    url = f"{_BASE}/ox-{target}.tar.gz"
    with tempfile.TemporaryDirectory() as tmp:
        tarball = Path(tmp) / "ox.tar.gz"
        _download(url, tarball)
        _verify(tarball, target)
        with tarfile.open(tarball) as tf:
            tf.extract("ox", _cache_dir())  # noqa: S202 - controlled, verified archive
    binary.chmod(binary.stat().st_mode | stat.S_IEXEC)
    return binary


def main() -> "int":
    binary = _ensure_binary()
    os.execv(str(binary), [str(binary), *sys.argv[1:]])


if __name__ == "__main__":
    main()
