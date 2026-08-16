use chrono::{DateTime, Datelike, Duration, NaiveDate, NaiveDateTime, NaiveTime, TimeZone, Utc};
use chrono_tz::Tz;

/// "Today" as seen by the household, in its configured `APP_TZ`.
pub fn today(household_tz: &Tz) -> NaiveDate {
    chrono::Utc::now().with_timezone(household_tz).date_naive()
}

/// Which week-start to resolve when today isn't itself the start weekday.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WeekStartDirection {
    /// The upcoming week start, never before today. Used by /plan, which
    /// plans the *upcoming* week.
    Forward,
    /// The start of the week containing today - so a status view like
    /// /display can always mark exactly one day as "today." Today counts when
    /// it matches, otherwise the most recent past occurrence.
    Backward,
}

/// The date whose weekday matches `week_start_weekday` (0=Monday..6=Sunday,
/// per app_settings.week_start_weekday) nearest to `today`, in the given
/// direction. Both directions count today itself as a match.
pub fn nearest_week_start(
    today: NaiveDate,
    week_start_weekday: i16,
    direction: WeekStartDirection,
) -> NaiveDate {
    let offset =
        (week_start_weekday as i64 - today.weekday().num_days_from_monday() as i64).rem_euclid(7);
    let offset = match direction {
        WeekStartDirection::Forward => offset,
        WeekStartDirection::Backward if offset == 0 => 0,
        WeekStartDirection::Backward => offset - 7,
    };
    today + Duration::days(offset)
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

    #[test]
    fn forward_finds_the_next_matching_weekday_inclusive_of_today() {
        // All three fall in the same calendar week, in forward order.
        let monday = NaiveDate::from_ymd_opt(2026, 8, 3).unwrap();
        let wednesday = NaiveDate::from_ymd_opt(2026, 8, 5).unwrap();
        let saturday = NaiveDate::from_ymd_opt(2026, 8, 8).unwrap();

        // week_start_weekday=5 (Saturday) - the default.
        assert_eq!(
            nearest_week_start(saturday, 5, WeekStartDirection::Forward),
            saturday,
            "today counts as a match"
        );
        assert_eq!(
            nearest_week_start(monday, 5, WeekStartDirection::Forward),
            saturday
        );

        // week_start_weekday=2 (Wednesday) - a household with a different planning day.
        assert_eq!(
            nearest_week_start(wednesday, 2, WeekStartDirection::Forward),
            wednesday
        );
        assert_eq!(
            nearest_week_start(monday, 2, WeekStartDirection::Forward),
            wednesday
        );
        assert_eq!(
            nearest_week_start(saturday, 2, WeekStartDirection::Forward),
            NaiveDate::from_ymd_opt(2026, 8, 12).unwrap(),
            "forward-only: the next Wednesday, not last week's"
        );
    }

    #[test]
    fn backward_finds_the_week_containing_today() {
        let monday = NaiveDate::from_ymd_opt(2026, 8, 3).unwrap();
        let wednesday = NaiveDate::from_ymd_opt(2026, 8, 5).unwrap();
        let saturday = NaiveDate::from_ymd_opt(2026, 8, 8).unwrap();

        // week_start_weekday=5 (Saturday) - the default.
        assert_eq!(
            nearest_week_start(saturday, 5, WeekStartDirection::Backward),
            saturday,
            "today counts as a match"
        );
        assert_eq!(
            nearest_week_start(monday, 5, WeekStartDirection::Backward),
            NaiveDate::from_ymd_opt(2026, 8, 1).unwrap(),
            "backward-only: last Saturday, not next week's"
        );

        // week_start_weekday=2 (Wednesday) - a household with a different planning day.
        assert_eq!(
            nearest_week_start(wednesday, 2, WeekStartDirection::Backward),
            wednesday
        );
        assert_eq!(
            nearest_week_start(saturday, 2, WeekStartDirection::Backward),
            wednesday,
            "today (Saturday) should fall within the week that started this Wednesday"
        );
    }
}
