# Web Docs — oxymake.noogram.dev

The OxyMake documentation site is an [mdBook](https://rust-lang.github.io/mdBook/)
built from `docs/book/` and published to **Cloudflare Pages** under the custom
domain **oxymake.noogram.dev**.

```
docs/book/src/**.md  ──mdbook build──▶  docs/book/book/  ──wrangler pages deploy──▶  Cloudflare Pages (oxymake-docs)  ──CNAME──▶  oxymake.noogram.dev
```

This document is the runbook. The in-repo pieces it describes:

| File | Role |
|------|------|
| `docs/book/book.toml` | mdBook config (title, theme, search, repo edit links) |
| `wrangler.toml` | Cloudflare Pages config (`name = oxymake-docs`, `pages_build_output_dir`) |
| `.github/workflows/docs-deploy.yml` | CI: build + deploy on push to `main` |
| `justfile` (`docs` group) | `just docs-build` / `docs-serve` / `docs-clean` / `docs-deploy` |

> **Deployment status.** The configuration is committed; the **live deploy is
> deferred to phase 2** (after the repo is recreated, so the Pages project points
> at the final repo and the Cloudflare account credentials are wired). Nothing in
> this repo deploys anything until the phase-2 prerequisites below are met.

---

## Build locally

```sh
cargo install mdbook        # one-time, if not present
just docs-build             # → docs/book/book/   (or: mdbook build docs/book)
just docs-serve             # live-reload preview at http://localhost:3000
```

The generated `docs/book/book/` directory is **git-ignored** — it is a build
artifact, rebuilt by CI on every deploy.

---

## Phase 2 — one-time setup (prerequisites)

These steps require access to the Cloudflare account and the final GitHub repo.
Do them once, in order.

### 1. Authenticate wrangler (operator gesture)

```sh
wrangler login          # already done by the operator; opens browser OAuth
wrangler whoami         # confirm the right account is active
```

### 2. Create the Pages project `oxymake-docs`

Direct-upload Pages project (no Git integration — we deploy from GitHub Actions):

```sh
# Build once so there is a directory to seed the project from:
just docs-build
wrangler pages project create oxymake-docs --production-branch=main
```

Or via the dashboard: **Workers & Pages → Create → Pages → Upload assets**,
name it exactly `oxymake-docs`.

### 3. Wire the GitHub Actions secrets

In the final repo: **Settings → Secrets and variables → Actions → New secret**

| Secret | Value |
|--------|-------|
| `CLOUDFLARE_API_TOKEN` | API token scoped to **Account · Cloudflare Pages · Edit**. Create at <https://dash.cloudflare.com/profile/api-tokens> (custom token, or the "Edit Cloudflare Workers" template). |
| `CLOUDFLARE_ACCOUNT_ID` | The target account ID (dash → any domain → *Account ID* in the right sidebar). |

These names are referenced verbatim by `.github/workflows/docs-deploy.yml`.
**Do not commit the token** — it lives only in GitHub Secrets.

### 4. Custom domain — oxymake.noogram.dev (DNS on Cloudflare)

`oxymake.noogram.dev` is hosted on the operator's Cloudflare account, so DNS and Pages
are the same account — attaching the domain is one step:

```sh
wrangler pages deployment list --project-name=oxymake-docs   # sanity check
```

Then in the dashboard: **Workers & Pages → oxymake-docs → Custom domains →
Set up a custom domain → `oxymake.noogram.dev`** (and optionally `www.oxymake.noogram.dev`).

Because the zone is on the same Cloudflare account, Cloudflare adds the required
`CNAME` (→ `oxymake-docs.pages.dev`) automatically and provisions the TLS
certificate. No manual DNS record editing is needed for an in-account zone.

If you prefer the apex naked-domain via API/Terraform later, the record is:

```
Type   Name           Content
CNAME  oxymake.noogram.dev    oxymake-docs.pages.dev   (proxied)
```

---

## Deploying

### Automatic (the normal path)

Once phase-2 setup is done, every push to `main` that touches `docs/book/**`,
`wrangler.toml`, or the workflow file rebuilds and redeploys via
`.github/workflows/docs-deploy.yml`. Trigger manually from the **Actions** tab
(*Deploy Docs → Run workflow*) when needed.

### Manual (from a workstation)

```sh
just docs-deploy
# equivalently:
mdbook build docs/book
wrangler pages deploy docs/book/book --project-name=oxymake-docs --branch=main
```

`--branch=main` publishes to the production deployment (the one mapped to
oxymake.noogram.dev). Omit it, or use another branch name, to get a preview URL instead.

---

## Troubleshooting

- **`wrangler pages deploy` says project not found** — the `oxymake-docs` project
  doesn't exist yet on this account; run step 2.
- **CI deploy fails with an auth error** — `CLOUDFLARE_API_TOKEN` /
  `CLOUDFLARE_ACCOUNT_ID` missing or under-scoped (needs *Pages: Edit*).
- **404 at oxymake.noogram.dev** — custom domain not attached, or DNS not yet propagated;
  re-check step 4. The raw `*.pages.dev` URL works before the custom domain does.
- **`mdbook: command not found`** — `cargo install mdbook` locally; CI installs a
  pinned binary (`MDBOOK_VERSION` in the workflow).
