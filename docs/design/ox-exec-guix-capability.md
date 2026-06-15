# `ox-exec-guix` — capability design note

**Status:** draft · **Parent:** Guix-capability deliberation (4/4 Q-GUIX-1) · **Created:** 2026-05-28

Optional crate wrapping job execution inside a GNU Guix profile/container on Linux — bit-reproducible builds for users who already trust Guix, *without* elevating Guix to an OxyMake invariant.

## 1. Boundary

Capability of one optional crate `ox-exec-guix`, **NOT** a project invariant. OX-8 is explicitly refused at this stage (4/4 panel verdict, Guix-capability deliberation §C1). The cache key (OX-1) is **not** widened to include Guix store hashes — a Guix-wrapped job and a system-shell job sharing the same logical inputs share the same cache entry. Guix is a *runtime wrapper*, not a *content identity*. Promotion to invariant requires the R0 attestation suite to pass first.

## 2. Trait surface (sketch)

Implements the existing `ox-core::traits::executor::Executor` — like `ox-exec-local`, `-slurm`, `-ray`. No new trait, no widening of `ExecutorCapabilities`.

```rust
#[cfg(all(feature = "guix", target_os = "linux"))]
pub struct GuixExecutor {
    inner: LocalExecutor,           // delegate spawn / atomic-finalize
    manifest_path: PathBuf,         // ./manifest.scm
    container_mode: ContainerMode,  // Profile | Container { net: bool }
}
#[cfg(all(feature = "guix", target_os = "linux"))]
impl Executor for GuixExecutor {
    type Error = ExecGuixError;
    async fn execute(&self, job: &ConcreteJob, ws: &Workspace, ctx: &ExecContext)
        -> Result<JobResult, Self::Error>
    {
        // `guix shell [--container] -m manifest.scm -- <cmd>`
        self.inner.execute(&wrap_with_guix(job, &self.manifest_path, self.container_mode),
                           ws, ctx).await.map_err(Into::into)
    }
    // init / health_check / cleanup / capabilities / prepare_workspace /
    // finalize_workspace / cancel / poll_status / submit_dag — delegate to inner.
}
```

Pure-string wrapper, analogous to the existing `EnvSpec::Nix { expr }` arm in `ox-exec-local::resolve_environment`.

## 3. Feature-flag plan

`Cargo.toml`: `[features] guix = []` (off by default). Every type is gated `#[cfg(all(feature = "guix", target_os = "linux"))]` — the crate compiles as an empty stub on macOS and on Linux without the feature. CI: `cargo check --workspace` stays green on both OS; `cargo check -p ox-exec-guix --features guix` runs on Linux only; macOS CI skips the feature.

## 4. Integration point (single line)

```rust
// crates/ox-cli/src/run.rs
#[cfg(all(feature = "guix", target_os = "linux"))]
ExecutorKind::Guix => Box::new(GuixExecutor::new(cfg.guix)?),
```

One match arm. No deeper coupling.

## 5. Non-touch

- **OX-1** (cache key) — Guix store hashes do NOT enter it.
- **OX-2** (atomic outputs) — `GuixExecutor` delegates prepare/finalize to `LocalExecutor`; `.oxytmp` unchanged.
- **No Scheme / no Guile in the OxyMake runtime** — Guix is shelled out as a CLI, like `nix develop` today.
- **No new `EnvSpec` variant** — Guix lives at the executor layer, not env-spec (subtractive design; users can still nest `EnvSpec::Uv` inside a Guix profile).

## 6. Open questions

1. **R0 attestation harness shape.** What does a passing attestation look like for a Guix run? Owned by the R0 attestation task; must land before OX-8 re-opens.
2. **Default mode: profile vs container.** Recommended profile-mode default + container opt-in via `[executor.guix] container = true` — sized as a small ADR.

## 7. Falsifier

If `ox-exec-guix` ships and a cross-crate invariant violation (OX-1 cache-key conflict with a Guix store path) is found by a *user* rather than caught by a *spec*, the crate-local boundary was wrong and OX-8 should have been a project invariant. Zero such bugs across consecutive `v*` release reviews corroborates the verdict. *(Originally dated 2026-11-27; the calendar gate was voided 2026-06-10 — operator decision, premortem PM#5, no temporal gates.)*
