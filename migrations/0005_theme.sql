-- Theme preference for the whole app: "light", "dark", or "auto" (follow the
-- browser's prefers-color-scheme, today's only behavior). Stored server-side
-- like every other setting here - Foodinator is a shared household app with
-- no per-user browser state.
ALTER TABLE app_settings ADD COLUMN theme TEXT NOT NULL DEFAULT 'auto'
  CHECK (theme IN ('light', 'dark', 'auto'));
