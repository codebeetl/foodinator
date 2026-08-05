CREATE TABLE consumers (
    id          BIGSERIAL PRIMARY KEY,
    name        TEXT NOT NULL,
    active      BOOLEAN NOT NULL DEFAULT true,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE meals (
    id          BIGSERIAL PRIMARY KEY,
    name        TEXT NOT NULL UNIQUE,
    active      BOOLEAN NOT NULL DEFAULT true,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- Standing per-consumer preference for a meal, independent of any specific
-- occurrence. Only rows for an explicit like/dislike exist; absence means no
-- opinion. Feeds the "who's here today -> what's suitable" suggestion query.
CREATE TABLE consumer_meal_preferences (
    consumer_id BIGINT NOT NULL REFERENCES consumers(id) ON DELETE CASCADE,
    meal_id     BIGINT NOT NULL REFERENCES meals(id) ON DELETE CASCADE,
    preference  TEXT NOT NULL CHECK (preference IN ('like', 'dislike')),
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (consumer_id, meal_id)
);

-- One meal per calendar day, shared by the whole family. start/duration
-- overrides fall back to app_settings' default when unset.
CREATE TABLE meal_plan_entries (
    id                        BIGSERIAL PRIMARY KEY,
    entry_date                DATE NOT NULL UNIQUE,
    meal_id                   BIGINT NOT NULL REFERENCES meals(id) ON DELETE RESTRICT,
    notes                     TEXT,
    start_time_override       TIME,
    duration_minutes_override INTEGER,
    deleted_at                TIMESTAMPTZ,
    created_at                TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at                TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- Who ate a given day's meal. Set at planning time, editable afterward;
-- doubles as both the "who's here" suitability input and the historical record.
CREATE TABLE meal_attendance (
    meal_plan_entry_id BIGINT NOT NULL REFERENCES meal_plan_entries(id) ON DELETE CASCADE,
    consumer_id        BIGINT NOT NULL REFERENCES consumers(id) ON DELETE CASCADE,
    PRIMARY KEY (meal_plan_entry_id, consumer_id)
);

-- Single-row table of household-tunable defaults, editable from the admin UI
-- without a redeploy (unlike the rest of this app's env-var-only config).
CREATE TABLE app_settings (
    id                        SMALLINT PRIMARY KEY DEFAULT 1 CHECK (id = 1),
    default_start_time        TIME NOT NULL DEFAULT '18:30',
    default_duration_minutes  INTEGER NOT NULL DEFAULT 30,
    updated_at                TIMESTAMPTZ NOT NULL DEFAULT now()
);
INSERT INTO app_settings (id) VALUES (1);

-- Sync ledger: source of truth for idempotency lives here, not in HA.
CREATE TABLE ha_calendar_sync (
    meal_plan_entry_id BIGINT PRIMARY KEY REFERENCES meal_plan_entries(id) ON DELETE CASCADE,
    ha_uid             TEXT,
    content_hash       TEXT NOT NULL,
    synced_at          TIMESTAMPTZ,
    last_error         TEXT
);
