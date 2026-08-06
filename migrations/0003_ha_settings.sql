-- Per-field override of the HA_URL/HA_TOKEN/HA_CALENDAR_ENTITY_ID env vars,
-- editable from the Settings page. NULL means "fall back to the env var" -
-- the Home Assistant integration is only enabled once all three resolve
-- (DB override or env default) to a value.
ALTER TABLE app_settings ADD COLUMN ha_url TEXT;
ALTER TABLE app_settings ADD COLUMN ha_token TEXT;
ALTER TABLE app_settings ADD COLUMN ha_calendar_entity_id TEXT;
