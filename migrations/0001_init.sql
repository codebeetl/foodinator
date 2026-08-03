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
    description TEXT,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- One row per recurring weekday+time slot. "Mon/Wed/Fri 08:00" = 3 rows.
-- Chosen over a weekday-bitmask/array column: trivial joins, trivial
-- "what happens every Monday" queries, no bit-twiddling in SQL or Rust.
CREATE TABLE eating_slots (
    id                BIGSERIAL PRIMARY KEY,
    consumer_id       BIGINT NOT NULL REFERENCES consumers(id) ON DELETE CASCADE,
    label             TEXT NOT NULL,
    weekday           SMALLINT NOT NULL CHECK (weekday BETWEEN 0 AND 6), -- 0=Mon
    start_local_time  TIME NOT NULL,
    duration_minutes  INTEGER NOT NULL DEFAULT 30,
    active            BOOLEAN NOT NULL DEFAULT true,
    created_at        TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at        TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (consumer_id, weekday, start_local_time)
);

-- Materialized occurrence: one meal assigned to one consumer's slot on one date.
CREATE TABLE meal_plan_entries (
    id             BIGSERIAL PRIMARY KEY,
    eating_slot_id BIGINT NOT NULL REFERENCES eating_slots(id) ON DELETE CASCADE,
    meal_id        BIGINT NOT NULL REFERENCES meals(id) ON DELETE RESTRICT,
    entry_date     DATE NOT NULL,
    notes          TEXT,
    deleted_at     TIMESTAMPTZ,
    created_at     TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at     TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (eating_slot_id, entry_date)
);

-- Sync ledger: source of truth for idempotency lives here, not in HA.
CREATE TABLE ha_calendar_sync (
    meal_plan_entry_id BIGINT PRIMARY KEY REFERENCES meal_plan_entries(id) ON DELETE CASCADE,
    ha_uid             TEXT,
    content_hash       TEXT NOT NULL,
    synced_at          TIMESTAMPTZ,
    last_error         TEXT
);

CREATE INDEX idx_eating_slots_consumer ON eating_slots(consumer_id);
CREATE INDEX idx_meal_plan_entries_slot_date ON meal_plan_entries(eating_slot_id, entry_date);
