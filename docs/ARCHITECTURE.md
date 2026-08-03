# Architecture

## System overview

```
+----------------+       REST (Bearer token)      +---------------------+
|  ha-foodinator |  ----------------------------->  |  Home Assistant     |
|  (Rust/axum,   |                                  |  Local Calendar     |
|  Postgres)     |  <-- GET /api/calendars/<id> --  |  integration        |
+----------------+                                  +---------------------+
```

ha-foodinator is a standalone container, not a Home Assistant custom_component - it
runs on its own network host and pushes calendar events into a remote HA instance over
HA's REST API. There is no code running inside HA.

## Data model

- `consumers` - people who eat meals.
- `meals` - a catalog of meal names/descriptions.
- `eating_slots` - per-consumer recurring weekday+time slots (e.g. "Alice eats
  breakfast Mon/Wed/Fri at 08:00"). One row per weekday+time, not a bitmask, to keep
  joins and "what happens every Monday" queries simple.
- `meal_plan_entries` - a meal assigned to a specific consumer's eating slot on a
  specific date. Soft-deleted (`deleted_at`), not hard-deleted, because a
  previously-pushed HA event needs to be reconciled against, and HA has no delete
  service to key that reconciliation off of (see below).
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
  directly from ha-foodinator, authenticated with the same long-lived token used for
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

This repository is a scaffold. Implemented: project skeleton, health/readiness
checks, Postgres schema + migrations, a full CRUD vertical slice for `consumers`
(DB + web UI), an HA REST client (`get_api_status`, `create_event`, `list_events`),
the sync idempotency primitives described above, HTTP Basic auth, and a manual
`/admin/test-event` smoke-test endpoint for the create_event call.

Deferred (not built yet, no code for these exists):

- CRUD UI for `meals`, `eating_slots`, `meal_plan_entries` (only `consumers` was
  built, as the reference pattern to copy).
- `domain/schedule.rs` - materializing `meal_plan_entries` from `eating_slots` +
  admin assignments. This is a prerequisite for any real sync job.
- A background sync loop that actually walks `meal_plan_entries` and calls
  `create_event`/`list_events` via the primitives in `src/ha/sync.rs`. Today those
  primitives are unit-tested in isolation but have no caller in `src/lib.rs::run`.
- Pagination, a stronger auth mechanism than HTTP Basic, and a broader test suite
  beyond the vertical slices already covered.

## Timezone handling

Not yet decided beyond storing `eating_slots.start_local_time` as a wall-clock
`TIME` with no attached zone. An `APP_TZ` (IANA zone name) config value will be
needed once `domain/schedule.rs` starts converting local slot times into the UTC
`DateTime`s the HA client and `content_hash` expect.
