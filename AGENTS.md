# AGENTS.md

Operational guidance for AI coding agents working in this repository. Human-facing
docs (features, deployment) live in [README.md](README.md); the data model and the
Home Assistant API workaround live in [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md).

## Build, lint, test

Don't assume a local Rust toolchain is available. If one isn't, run everything
through a `rust:1-slim-bookworm` container with cached volumes so repeated invocations stay
fast (requires `docker compose up -d db` first, so the `db` hostname resolves on
that network):

```bash
docker run --rm \
  --network foodinator_default \
  -e DATABASE_URL=postgres://foodinator:foodinator@db:5432/foodinator \
  -v "$(pwd)":/app -w /app \
  -v foodinator-cargo-home:/usr/local/cargo \
  -v foodinator-rustup-home:/usr/local/rustup \
  -v foodinator-target:/app/target \
  rust:1-slim-bookworm bash -c "<command>"
```

If a local toolchain **is** available, the same `cargo` commands work directly.

These are the exact checks CI (`.github/workflows/ci.yml`) runs - match them, don't
invent stricter ones:

| Check | Command |
|---|---|
| Format | `cargo fmt --check` |
| Lint | `SQLX_OFFLINE=true cargo clippy --all-targets -- -D warnings` |
| Build | `SQLX_OFFLINE=true cargo build --release` |
| sqlx query cache | `cargo sqlx prepare --check --database-url postgresql://foodinator:foodinator@db:5432/foodinator` |
| Tests | `cargo test` |

All must pass before a commit. `cargo fmt` and `cargo test` need no live database;
the sqlx and `test` checks do.

### After adding or changing any `sqlx::query!`/`query_as!` call

The offline cache in `.sqlx/` (committed to git) goes stale. Apply migrations to a
live, reachable Postgres, then regenerate and commit the new `.sqlx/*.json` files:

```bash
sqlx migrate run --source ./migrations --database-url postgresql://foodinator:foodinator@db:5432/foodinator
cargo sqlx prepare --database-url postgresql://foodinator:foodinator@db:5432/foodinator
```

Skipping this passes a local `cargo test` (which doesn't set `SQLX_OFFLINE`) but
fails CI's `build`, `clippy`, and `sqlx-check` jobs.

## Project layout

- `src/web/` - one file per route group (`plan.rs`, `settings.rs`, `display.rs`,
  ...), each exposing `pub fn router() -> Router<AppState>`, merged in
  `src/web/mod.rs`. Two router trees: `protected` (wrapped in HTTP Basic Auth) and
  everything else (health checks, the `/display` kiosk view, `/static`,
  `/favicon.ico`) - a route belongs outside `protected` only if it has its own auth
  story or is genuinely public.
- `src/db/` - one file per table/domain concept; thin `sqlx::query!`/`query_as!`
  wrappers, no business logic.
- `src/ha/` - the Home Assistant REST client and the sync/idempotency logic that
  works around HA having no update/delete service for calendar events (see
  `docs/ARCHITECTURE.md`).
- `templates/` - Askama templates. Askama 0.12's macro-call syntax is
  `{% call scope::name(args) %}` (self-closing, no `{% endcall %}`) -
  `{{ scope::name(args) }}` as a plain expression does not compile.
- Tests live inline (`#[cfg(test)] mod tests` at the bottom of each file):
  `#[sqlx::test(migrations = "./migrations")]` for anything touching the database,
  a `connect_lazy` pool + `crate::state::test_app_state(pool)` for routes that
  don't.

## Conventions specific to this repo

- Any `ORDER BY` on a free-text (name/search) column needs `COLLATE "C"` -
  Postgres's default collation differs between the Alpine image used in local dev
  and the Debian-based image GitHub Actions' Postgres service uses, so an un-pinned
  sort order can pass locally and fail in CI.
- Env-var mutation in tests (`env::set_var`/`remove_var`) must go through a
  Drop-based guard that restores the original value - `std::env` is process-global,
  so an unrestored mutation in one test file can break an unrelated test elsewhere
  in the same binary, and only when the full suite runs together.
- Optional integrations/features (Home Assistant, the `/display` kiosk view) follow
  one pattern throughout: an optional env var provides a default, per-field config
  in the database can override it live (no restart), and the feature is simply
  *absent* (route 404s, nav link hidden, admin action returns 503) rather than
  erroring when unconfigured. Follow this pattern for any new optional feature.
- Commit messages: one line, imperative mood, no body (e.g. `Add /display week data
  and kiosk template`) - matches every commit in this repo's history.

## Releasing

The app's version (shown on the Settings page and baked into the GHCR image tag) is
read at compile time from `Cargo.toml`'s `version` field
(`env!("CARGO_PKG_VERSION")` in `src/web/settings.rs`) - there's no separate place to
update. To cut a release:

1. Bump `version` in `Cargo.toml` following semver (patch for fixes, minor for new
   backward-compatible features, major for breaking changes) and commit it.
2. Push, then `gh release create vX.Y.Z` (tag must match the bumped version, prefixed
   with `v`) with release notes.
3. `.github/workflows/docker-publish.yml` triggers on the release being published and
   pushes `ghcr.io/codebeetl/foodinator` tagged both `:latest` and `:vX.Y.Z` - no
   separate version-bump step needed there, it reads the tag straight off the
   GitHub Release event.
