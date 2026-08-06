-- Consumers flagged as default attend every new plan day automatically;
-- everyone else must be added explicitly via the plan-day picker.
ALTER TABLE consumers ADD COLUMN is_default BOOLEAN NOT NULL DEFAULT false;

-- Ad-hoc, unlinked attendees for a single day (no preference tracking,
-- unlike consumers). Plain text, not a consumer_id, on purpose.
ALTER TABLE meal_plan_entries ADD COLUMN guest_names TEXT[] NOT NULL DEFAULT '{}';

-- Powers fuzzy/substring ranking for the meal search picker.
CREATE EXTENSION IF NOT EXISTS pg_trgm;
CREATE INDEX meals_name_trgm_idx ON meals USING GIN (name gin_trgm_ops);
