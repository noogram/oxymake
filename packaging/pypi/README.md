# OxyMake (Python launcher)

A content-addressable workflow engine. You `git checkout` an old branch, re-run
your pipeline, and it **does not** rebuild everything — change detection is a
BLAKE3 hash of file *content*, not timestamps.

```bash
uv tool install oxymake     # or: pipx install oxymake
ox --help
```

This package is a thin launcher: on first run it downloads the prebuilt `ox`
binary for your platform from the
[GitHub release](https://github.com/noogram/oxymake/releases/latest), verifies
its SHA-256, caches it, and execs it. No Rust toolchain required — which is the
point: bioinformatics and data-science users who live in conda/pip can try
OxyMake without a source build.

Full documentation: <https://github.com/noogram/oxymake>
