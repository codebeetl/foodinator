# ha-foodinator

A standalone helper application that manages meals, consumers, and recurring
weekday/time eating slots, and pushes the resulting meal-plan into a remote Home
Assistant instance as calendar events. It runs in its own container on the network -
it is **not** a Home Assistant custom_component/HACS integration.

## Architecture

See [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) for the data model, the HA REST API
integration approach, and a documented upstream limitation (HA has no public
service to update or delete a calendar event) along with how this project works
around it.

## Quick start

```bash
cp .env.example .env   # fill in HA_URL, HA_TOKEN, HA_CALENDAR_ENTITY_ID, ADMIN_*
docker compose up --build
```

The admin web UI (HTTP Basic auth, `ADMIN_USERNAME`/`ADMIN_PASSWORD` from `.env`) is
served at `http://localhost:8080/consumers`. Health checks are unauthenticated at
`/healthz` (liveness) and `/readyz` (checks the database connection).

## Configuration

All configuration is via environment variables (see `.env.example`):

| Variable | Required | Description |
|---|---|---|
| `DATABASE_URL` | yes | Postgres connection string |
| `BIND_ADDR` | no (default `0.0.0.0:8080`) | Address the web server binds to |
| `HA_URL` | yes | Base URL of your Home Assistant instance, no trailing slash |
| `HA_TOKEN` | yes | HA long-lived access token |
| `HA_CALENDAR_ENTITY_ID` | yes | Entity ID of the Local Calendar to push events to |
| `ADMIN_USERNAME` | yes | HTTP Basic auth username for the admin UI |
| `ADMIN_PASSWORD` | yes | HTTP Basic auth password for the admin UI |

See [docs/HA_SETUP.md](docs/HA_SETUP.md) for creating the Local Calendar integration
and generating a long-lived access token.

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
  details and the sync-horizon workaround this project uses.
- This is a scaffold: only `consumers` has a full CRUD vertical slice built out.
  `meals`, preferences, the week-grid meal planner, and the actual
  meal-plan-to-calendar sync trigger are not yet implemented (see the "What's
  implemented vs. deferred" section of `docs/ARCHITECTURE.md`).

## License

MIT - see [LICENSE](LICENSE).
