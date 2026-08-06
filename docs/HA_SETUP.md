# Home Assistant setup

Foodinator pushes events into an existing Home Assistant "Local Calendar"
integration over HA's REST API. It does not install anything inside HA.

## 1. Create a Local Calendar

In Home Assistant: **Settings -> Devices & Services -> Add Integration -> Local
Calendar**. Give it a name (e.g. "Foodinator") - this determines its entity ID,
typically `calendar.foodinator`. Note the entity ID; you'll need it for
`HA_CALENDAR_ENTITY_ID`.

## 2. Generate a long-lived access token

In Home Assistant: click your user profile (bottom left) -> **Security** tab ->
scroll to **Long-lived access tokens** -> **Create Token**. Copy it immediately;
HA only shows it once. This is your `HA_TOKEN`.

## 3. Configure Foodinator

Either fill in `.env` (copy from `.env.example`):

```
HA_URL=http://homeassistant.local:8123
HA_TOKEN=<the token from step 2>
HA_CALENDAR_ENTITY_ID=calendar.foodinator
```

or set the same three fields from the **Settings** page in the running app - that
takes effect immediately, no restart, and overrides the env vars per-field (a
blank field there falls back to its env var). Settings also has a **Test
connection** button to confirm a token/URL actually works before you rely on it.
Either way, all three need to resolve to a value for syncing to be enabled at all.

`HA_URL` has no trailing slash and must be reachable from wherever the
Foodinator container runs (the same LAN, or through whatever networking you use
between the two hosts).

## Known limitation: editing or cancelling an already-pushed event

Home Assistant's REST API can create calendar events but has no public service to
update or delete one (see `docs/ARCHITECTURE.md` for the verified details). If a
meal-plan entry changes or is cancelled *after* Foodinator has already pushed it
to HA, the stale event will **not** be automatically corrected or removed - it needs
to be edited or deleted manually in the HA calendar UI. Foodinator's sync horizon
is designed to minimize how often this happens, not eliminate it entirely.
