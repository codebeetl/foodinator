-- A day can be configured without a meal: the clear button next to the picker
-- drops the meal but keeps notes, guests, attendees, and the time/duration
-- overrides, so those can't live in a row that requires a meal. The foreign key
-- to meals(id) is unchanged - NULL simply isn't a reference, so the
-- "can't delete a meal that's been planned" guarantee still holds for every day
-- that actually names one.
ALTER TABLE meal_plan_entries ALTER COLUMN meal_id DROP NOT NULL;
