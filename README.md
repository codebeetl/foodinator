# Foodinator

*Name courtesy of Dr. Heinz Doofenshmirtz - if it doesn't end in "-inator," is it even
a household appliance?*

A standalone helper application that manages consumers (people), meals, standing
like/dislike preferences, and a week-grid meal plan (one shared family meal per
calendar day). It can optionally push the resulting plan into a remote Home
Assistant instance as calendar events - the HA integration is off by default and
only activates once it's configured (see [Configuration](#configuration) below).
It runs in its own container on the network - it is **not** a Home Assistant
custom_component/HACS integration.

## Architecture

See [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) for the data model, the HA REST API
integration approach, and a documented upstream limitation (HA has no public
service to update or delete a calendar event) along with how this project works
around it.

## Quick start (local trial run)

```bash
cp .env.example .env   # fill in ADMIN_*, APP_TZ - HA_* is optional, see below
docker compose up --build
```

The admin web UI (HTTP Basic auth, `ADMIN_USERNAME`/`ADMIN_PASSWORD` from `.env`) is
served at `http://localhost:8080/plan`. Other pages: `/consumers`, `/meals`,
`/settings`, and `/sync` (manual "push to Home Assistant" trigger, only shown once
HA is configured). Health checks are unauthenticated at `/healthz` (liveness) and
`/readyz` (checks the database connection).

For a persistent deployment on a home server (e.g. OpenMediaVault), see
[Deploying on a local server](#deploying-on-a-local-server-eg-openmediavault) below.

## Configuration

All configuration is via environment variables (see `.env.example`):

| Variable | Required | Description |
|---|---|---|
| `DATABASE_URL` | yes | Postgres connection string |
| `BIND_ADDR` | no (default `0.0.0.0:8080`) | Address the web server binds to |
| `HA_URL` | no | Base URL of your Home Assistant instance, no trailing slash |
| `HA_TOKEN` | no | HA long-lived access token |
| `HA_CALENDAR_ENTITY_ID` | no | Entity ID of the Local Calendar to push events to |
| `ADMIN_USERNAME` | yes | HTTP Basic auth username for the admin UI |
| `ADMIN_PASSWORD` | yes | HTTP Basic auth password for the admin UI |
| `APP_TZ` | yes | IANA timezone the household lives in, e.g. `Australia/Sydney` - used for "today" in the week-grid planner and to convert meal times to UTC when pushing to Home Assistant |
| `DISPLAY_TOKEN` | no | Enables the `/display` wall-kiosk view, gated by this token - see [Wall-display kiosk view](#wall-display-kiosk-view-optional) below |

The app **will not start** if any required variable is missing or, for `APP_TZ`, not
a valid IANA timezone name - it fails fast with a clear error rather than starting in
a broken state.

### Home Assistant integration (optional)

`HA_URL`/`HA_TOKEN`/`HA_CALENDAR_ENTITY_ID` are env-var *defaults*, not requirements.
Each one can also be set (and overridden per-field) from the **Settings** page, which
is stored in the database and takes effect immediately - no restart needed. The
integration is only active once all three resolve to a value, whichever source they
come from; a partially-configured combination (e.g. a URL but no token) is treated
the same as fully unconfigured. Settings also has a **Test connection** button that
checks the currently-typed values (or, for a blank field, whatever's already saved)
without persisting anything.

While HA is unconfigured, the `/sync` page, the "Sync" nav link, and the admin
`/admin/ha-check`/`/admin/test-event` smoke-test routes are all hidden or return
`503 Service Unavailable`.

See [docs/HA_SETUP.md](docs/HA_SETUP.md) for creating the Local Calendar integration
and generating a long-lived access token.

### Wall-display kiosk view (optional)

Setting `DISPLAY_TOKEN` enables `http://<host>:8080/display?token=<DISPLAY_TOKEN>` - a
full-screen, read-only week view meant for a tablet mounted on a wall or fridge. It
lives outside HTTP Basic auth (a tablet can't type a password) and is gated by the
token instead; a missing or wrong token returns `401`, and the route doesn't exist at
all (`404`) if `DISPLAY_TOKEN` is unset. It shows the same week grid `/plan` computes
(today's card large and accent-bordered, the rest smaller), reads-only, and refreshes
itself by polling every 5 minutes - no interaction needed once it's loaded. Leave
`DISPLAY_TOKEN` unset to disable the route entirely.

## Deploying on a local server (e.g. OpenMediaVault)

This section covers running Foodinator as a persistent service on a home server/NAS
via Docker Compose - the `docker-compose.yml` in this repo is the same one used for
the quick start above, just run detached with a restart policy.

### Prerequisites

- Docker Engine with the Compose plugin (`docker compose`, not the legacy
  `docker-compose` v1). On OMV this is commonly provided by the **OMV-Extras**
  "Docker Compose" plugin, or by installing Docker directly and managing it over SSH -
  either way, the commands below are plain `docker compose` and work the same.
- Network access from the OMV host to your Home Assistant instance (same LAN is the
  common case).
- A shared folder / dataset on the array to hold the project files, e.g. under
  `/srv/dev-disk-by-uuid-<your-uuid>/appdata/`. Find your actual mount points with
  `df -h` or in the OMV web UI under **Storage -> File Systems**.
- The published container image (`ghcr.io/codebeetl/foodinator`) is private, since
  the repo is private. The OMV host needs a one-time `docker login ghcr.io` using a
  GitHub personal access token with `read:packages` scope before it can pull it -
  see step 3 below.

### 1. Get the code onto the server

```bash
ssh <user>@<omv-host>
git clone https://github.com/codebeetl/foodinator.git \
  /srv/dev-disk-by-uuid-<your-uuid>/appdata/ha-foodinator
cd /srv/dev-disk-by-uuid-<your-uuid>/appdata/ha-foodinator
```

If you manage containers through OMV-Extras' Compose plugin instead of the CLI,
point a new Compose entry at this same folder (or at its `docker-compose.yml`) rather
than running `docker compose` by hand - the file doesn't need to change either way.

### 2. Configure environment

```bash
cp .env.example .env
nano .env
```

Fill in `HA_URL`, `HA_TOKEN`, `HA_CALENDAR_ENTITY_ID` (see
[docs/HA_SETUP.md](docs/HA_SETUP.md)), `ADMIN_USERNAME`/`ADMIN_PASSWORD`, and
`APP_TZ`. `.env` **must live in the same directory as `docker-compose.yml`** -
Compose loads it automatically from there; it is never read from anywhere else and
is already excluded from git via `.gitignore`, so it's safe to edit in place.

### 3. Pull and start

```bash
docker login ghcr.io -u <github-username>   # first time only
docker compose pull
docker compose up -d
```

`docker login` will prompt for a password - use a GitHub personal access token
with `read:packages` scope, not your account password. This pulls the prebuilt
image published to GHCR by `.github/workflows/docker-publish.yml` - note that a
new image is only published when a GitHub Release is cut, not on every push to
`main`, so "deploy the latest code" means cutting a release first. It starts
two containers:

- `app` - the web UI, published on host port `8080`.
- `db` - Postgres 16, **not** published to the host - only reachable from `app` over
  the Compose-managed internal network.

Both services have `restart: unless-stopped`, so they come back up automatically
after a host reboot or Docker restart, without needing a cron job or systemd unit.
Database migrations run automatically on `app` startup, so there's no separate
migration step.

If no release has been published yet, or you'd rather build from source directly
on the NAS, `docker compose up -d --build` still works exactly as before - it
builds from the `Dockerfile` in this repo instead of pulling from GHCR.

### 4. Verify

```bash
curl http://localhost:8080/healthz   # liveness - always 200 once the process is up
curl http://localhost:8080/readyz    # readiness - 200 only once the DB is reachable
```

Then open `http://<omv-host>:8080/plan` in a browser and log in with
`ADMIN_USERNAME`/`ADMIN_PASSWORD`.

### Data persistence and backups

Postgres data lives in the named Docker volume `<project-name>_db-data` (the project
name defaults to the containing directory's name, e.g. `ha-foodinator_db-data` for
the path above - confirm the exact name with `docker volume ls`). It survives
`docker compose down` and image rebuilds, but **not** `docker compose down -v`. Back
it up with:

```bash
docker run --rm -v ha-foodinator_db-data:/data -v "$(pwd)":/backup alpine \
  tar czf /backup/ha-foodinator-db-backup-$(date +%F).tar.gz -C /data .
```

If you'd rather have the database files land directly on an OMV share (for the
NAS's own backup/snapshot tooling to pick up) instead of inside a Docker-managed
volume, change the `db` service's volume in `docker-compose.yml` from
`db-data:/var/lib/postgresql/data` to a bind mount, e.g.
`/srv/dev-disk-by-uuid-<your-uuid>/appdata/ha-foodinator/pgdata:/var/lib/postgresql/data`.

### Updating

```bash
cd /srv/dev-disk-by-uuid-<your-uuid>/appdata/ha-foodinator
git pull
docker compose pull
docker compose up -d
```

`git pull` picks up any `docker-compose.yml`/README/migration changes; `docker
compose pull` fetches the latest published image (only present once a GitHub
Release has been cut - see step 3 above).

### Firewall / networking

Only port `8080` (the `app` service) needs to be reachable from wherever you use the
admin UI - open that on the OMV host's firewall if you access it from another
machine on the LAN. The `db` service publishes no host port at all. `HA_URL` must be
reachable from the OMV host's Docker network - typically the same LAN your Home
Assistant instance is on.

## Development

No local Rust toolchain is assumed - build and test through Docker:

```bash
docker compose up -d db   # start just Postgres
cargo build               # if you do have a local Rust toolchain
cargo test                 # requires DATABASE_URL pointing at a reachable Postgres
```

Or entirely through the container image:

```bash
docker build .
```

Query metadata for sqlx's compile-time checks is committed in `.sqlx/` so builds
work without a live database (`SQLX_OFFLINE=true`, set automatically in the
Dockerfile and CI). After changing any `sqlx::query*!` call, regenerate it:

```bash
cargo install sqlx-cli --no-default-features --features rustls,postgres
DATABASE_URL=postgres://... cargo sqlx prepare
```

## Known limitations

- Home Assistant's REST API has no service to update or delete a calendar event
  once created - see [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) for the verified
  details and the sync-horizon/ledger workaround this project uses. A meal-plan
  entry that changes *after* it's been synced needs manual cleanup in HA's own
  calendar UI.
- Syncing to Home Assistant is a manual, global action (the "Sync to Home
  Assistant" button on `/sync`), not a background loop, and only considers entries
  within a 14-day horizon - by design, not a current limitation to fix.
- Auth is HTTP Basic only, suitable for a LAN-only admin tool behind your own
  network boundary - don't expose port 8080 directly to the internet without a
  reverse proxy adding real authentication and TLS in front of it.

## License

MIT - see [LICENSE](LICENSE).
