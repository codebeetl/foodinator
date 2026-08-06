use chrono::{DateTime, NaiveDate, NaiveDateTime, NaiveTime, TimeZone, Utc};
use chrono_tz::Tz;

/// "Today" as seen by the household, in its configured `APP_TZ`.
pub fn today(household_tz: &Tz) -> NaiveDate {
    chrono::Utc::now().with_timezone(household_tz).date_naive()
}

/// Converts a household-local wall-clock date/time into the UTC instant it
/// refers to. On the rare ambiguous local time (a DST fall-back repeat),
/// resolves to the earlier of the two possible instants; a DST spring-forward
/// gap can't occur for a household dinner time in practice, so this doesn't
/// try to be clever about it beyond picking the earliest match either way.
pub fn household_datetime_utc(
    household_tz: &Tz,
    date: NaiveDate,
    time: NaiveTime,
) -> DateTime<Utc> {
    let naive = NaiveDateTime::new(date, time);
    household_tz
        .from_local_datetime(&naive)
        .earliest()
        .expect("household local datetime should resolve to a UTC instant")
        .with_timezone(&Utc)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn household_datetime_utc_converts_using_the_given_timezone() {
        let sydney: Tz = "Australia/Sydney".parse().unwrap();
        let date = NaiveDate::from_ymd_opt(2026, 8, 8).unwrap();
        let time = NaiveTime::from_hms_opt(18, 30, 0).unwrap();

        let utc = household_datetime_utc(&sydney, date, time);

        // AEST is UTC+10 in August (outside Sydney's DST window).
        assert_eq!(utc.to_rfc3339(), "2026-08-08T08:30:00+00:00");
    }
}
