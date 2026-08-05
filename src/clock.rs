use chrono::NaiveDate;

/// "Today" as seen by the household. Currently a plain UTC-clock read; once
/// `APP_TZ` gets a real caller (household-local calendar timestamps in the HA
/// sync job) this should route through the same conversion so there's one
/// place to fix, not two.
pub fn today() -> NaiveDate {
    chrono::Utc::now().date_naive()
}
