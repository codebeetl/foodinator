-- Which weekday the meal-plan grid (and the wall display) treats as the
-- start of the week. 0=Monday .. 6=Sunday, matching chrono's
-- Weekday::num_days_from_monday() convention already used in
-- src/web/plan.rs. Defaults to 5 (Saturday), preserving today's hardcoded
-- Sat-Fri behavior.
ALTER TABLE app_settings ADD COLUMN week_start_weekday SMALLINT NOT NULL DEFAULT 5;
