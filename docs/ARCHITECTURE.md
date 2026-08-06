# Architecture

## System overview

```
+----------------+       REST (Bearer token)      +---------------------+
|   Foodinator   |  ----------------------------->  |  Home Assistant     |
|  (Rust/axum,   |                                  |  Local Calendar     |
|  Postgres)     |  <-- GET /api/calendars/<id> --  |  integration        |
+----------------+                                  +---------------------+
```

Foodinator is a standalone container, not a Home Assistant custom_component - it
runs on its own network host and pushes calendar events into a remote HA instance over
HA's REST API. There is no code running inside HA.

## Data model

The whole family shares one meal per day - there is no per-consumer recurring
schedule.

- `consumers` - people who eat meals.
- `meals` - a catalog of meal titles only (no recipes/descriptions). `active`
  lets a meal be retired from the picker without breaking history, mirroring
  `consumers.active`.
- `consumer_meal_preferences` - a standing like/dislike per consumer x meal,
  independent of any specific occurrence (a row exists only when a preference
  was explicitly set; absence means no opinion). Feeds the "who's here today ->
  what's suitable" suggestion query, which flags (does not exclude) meals
  disliked by a selected attendee.
- `meal_plan_entries` - one meal for one calendar date (`entry_date` is
  UNIQUE). `start_time_override`/`duration_minutes_override` fall back to
  `app_settings`' default when unset. Soft-deleted (`deleted_at`), not
  hard-deleted, because a previously-pushed HA event needs to be reconciled
  against, and HA has no delete service to key that reconciliation off of (see
  below).
- `meal_attendance` - which consumers ate a given day's meal. Set at planning
  time and editable afterward; doubles as both the suitability-query input and
  the historical record.
- `app_settings` - single-row table of the default meal start time/duration.
  The one setting that lives in the DB rather than an env var, since it's a
  household preference an admin may want to tweak without a redeploy.
- `ha_calendar_sync` - the idempotency ledger. Source of truth for "have we already
  pushed this entry, and has it changed since" lives here, not in HA.

See `migrations/0001_init.sql` for the full schema.

## Home Assistant's REST API: a real, load-bearing limitation

Verified against Home Assistant's developer docs and community discussion
(home-assistant/core discussion #3773; a 2025 PR attempting to add update/delete
support stalled and never merged):

- `POST /api/services/calendar/create_event` exists and works, but the response
  contains changed *states*, not the created event - **no UID comes back from
  create**.
- **There is no `calendar.update_event` or `calendar.delete_event` service.** Only the
  frontend's internal WebSocket command supports editing/deleting an event; it is not
  exposed over REST/services.
- `GET /api/calendars/<entity_id>?start=...&end=...` **does** return events with a
  `uid`, so already-created events can be read back even though they can't be
  created-and-read atomically.

Consequence: pushing new events and re-running the sync idempotently (no duplicates)
is fully solvable. **Updating or deleting an event already pushed to HA is not**
solvable through the public REST API today.

### How the design works around this

1. Every pushed event's description carries a marker: `foodinator:entry=<id>`
   (`src/ha/sync.rs::build_marker` / `extract_marker_entry_id`).
2. Before creating an event, compute a `content_hash` of
   (summary, description, start, end) and compare it against the `ha_calendar_sync`
   ledger row for that `meal_plan_entry_id`. Skip the create if the hash already
   matches a successful push - this is what makes re-running the sync job safe.
3. Only entries within a configurable **sync horizon** (e.g. the next N days) are
   eligible to push at all (`src/ha/sync.rs::is_within_sync_horizon`), so admins
   editing a meal-plan entry usually do so before anything has been pushed to HA -
   avoiding the need to update a live event in the first place.
4. On every read-back (`list_events`), a hash-matched ledger entry whose marker is
   missing or corrupted in the fetched description (e.g. a human hand-edited the
   event in HA's UI) should be surfaced as a "needs manual cleanup" case rather than
   silently ignored.

### Future options if this needs to improve

- If HA ships a public update/delete service, switch to it and retire the
  sync-horizon workaround.
- Call HA's WebSocket API (`calendar/event/delete` / a corresponding update command)
  directly from Foodinator, authenticated with the same long-lived token used for
  REST. This is not frontend-only: any external client that speaks the WS protocol
  can call it - a community workaround even drives it from HA's own `command_line`
  platform via `websocat` (see [home-assistant/core community thread
  #572499](https://community.home-assistant.io/t/add-delete-event-service-to-calendar-integration/572499/34)).
  It's still an internal, undocumented API with no stability guarantee - a HA
  release could change or remove it without notice - so this would need its own
  reqwest-or-tungstenite WebSocket client and defensive error handling, not a small
  addition to the existing REST client.
- Keep the current append-only-plus-manual-cleanup approach; it's honest about the
  constraint rather than papering over it.

## What's implemented vs. deferred

Implemented: project skeleton, health/readiness checks, Postgres schema +
migrations, full CRUD (DB + web UI) for `consumers` and `meals`, standing
per-consumer meal preferences editable from a meal's edit page, an `app_settings`
screen for the household's default meal time/duration, the suitability query
(given a set of attendees, flag - never exclude - meals disliked by any of them),
a navigable Sat-Fri week-grid meal planner (`/plan`), an HA REST client
(`get_api_status`, `create_event`, `list_events`), the sync idempotency
primitives described above plus the `ha_calendar_sync` ledger they feed, a
manual global "Sync to Home Assistant" trigger (`/sync`) that walks syncable
`meal_plan_entries` within the sync horizon, HTTP Basic auth, and a manual
`/admin/test-event` smoke-test endpoint for the create_event call in isolation.

Deferred (not built, no code for these exists):

- Automatic detection/surfacing of a hand-edited or marker-corrupted event on
  read-back (`list_events` is implemented and unit-tested, but nothing calls it
  from the sync job yet - today a corrupted marker is a silent no-op, not a
  surfaced "needs manual cleanup" case).
- Pagination, a stronger auth mechanism than HTTP Basic, and a broader test suite
  beyond the vertical slices already covered.
- A configurable sync horizon (currently a 14-day constant in `src/web/sync.rs`,
  not an env var).

## Timezone handling

A single household timezone, via a required `APP_TZ` (IANA zone name) config
value, converts `meal_plan_entries`' wall-clock local time into the UTC
`DateTime`s the HA client and `content_hash` expect. One timezone for the whole
app, not per-consumer or per-entry - this is a single-family meal planner.
