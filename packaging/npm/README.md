# oxymake

This npm package **reserves the `oxymake` name** on the npm registry. It
contains no runnable code.

[OxyMake](https://oxymake.noogram.dev) is a content-addressed workflow
engine, distributed as a single static binary
named **`ox`** — not as a Node module.

## Install the `ox` binary

Prebuilt binaries for Linux and macOS are attached to every
[GitHub Release](https://github.com/noogram/oxymake/releases).

From source (requires a Rust toolchain):

```sh
cargo install --git https://github.com/noogram/oxymake ox-cli
```

See <https://oxymake.noogram.dev> for documentation.

---

*Why a placeholder?* The audience that adopts OxyMake (Snakemake / Nextflow
migrants) lives in pip and conda, with some overlap into the JS data tooling
ecosystem. Reserving `oxymake` on npm is cheap insurance against name-squatting
once the project is public. If a real npm entry point ever ships (e.g. a thin
launcher that downloads the `ox` binary, mirroring `packaging/pypi/`), it
replaces this README in a future minor.
