-- Google Calendar OAuth2 config (nullable - integration is disabled until
-- all three are set, same pattern as HA).
ALTER TABLE app_settings ADD COLUMN gcal_client_id TEXT;
ALTER TABLE app_settings ADD COLUMN gcal_client_secret TEXT;
ALTER TABLE app_settings ADD COLUMN gcal_calendar_id TEXT;

-- OAuth2 refresh token (set by the callback, never shown in the UI).
ALTER TABLE app_settings ADD COLUMN gcal_refresh_token TEXT;

-- GCal sync ledger - mirrors ha_calendar_sync but with gcal_event_id
-- because Google Calendar supports update/delete natively.
CREATE TABLE gcal_calendar_sync (
    meal_plan_entry_id BIGINT PRIMARY KEY REFERENCES meal_plan_entries(id) ON DELETE CASCADE,
    gcal_event_id      TEXT NOT NULL,
    content_hash       TEXT NOT NULL,
    synced_at          TIMESTAMPTZ,
    last_error         TEXT
);
