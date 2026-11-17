//! Proleptic Gregorian calendar arithmetic on plain numbers, shared by the
//! grammars and the emitter so neither needs a date library.

/// Whether `year` is a leap year.
fn is_leap_year(year: i32) -> bool {
    year.rem_euclid(4) == 0 && (year.rem_euclid(100) != 0 || year.rem_euclid(400) == 0)
}

/// The number of days in `month` (1 to 12) of `year`; 0 for any other
/// month.
pub(super) fn days_in_month(year: i32, month: u8) -> u8 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if is_leap_year(year) => 29,
        2 => 28,
        _ => 0,
    }
}

/// Days from 1970-01-01 to the given date, negative before it. The date must
/// be valid.
pub(super) fn days_from_civil(year: i32, month: u8, day: u8) -> i64 {
    let month = i64::from(month);
    let year = i64::from(year) - i64::from(month <= 2);
    let era = year.div_euclid(400);
    let year_of_era = year - era * 400;
    let shifted_month = if month > 2 { month - 3 } else { month + 9 };
    let day_of_year = (153 * shifted_month + 2) / 5 + i64::from(day) - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

/// The date `days` days after 1970-01-01 (before it when negative), as year,
/// month and day.
pub(super) fn civil_from_days(days: i64) -> (i64, u8, u8) {
    let shifted = days + 719_468;
    let era = shifted.div_euclid(146_097);
    let day_of_era = shifted - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let shifted_month = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * shifted_month + 2) / 5 + 1;
    let month = if shifted_month < 10 {
        shifted_month + 3
    } else {
        shifted_month - 9
    };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    (year, month as u8, day as u8)
}

/// The weekday of a date `days` days after 1970-01-01, counted from Monday
/// as 0 to Sunday as 6.
pub(super) fn weekday_from_monday(days: i64) -> u8 {
    // 1970-01-01 was a Thursday, 3 days after a Monday.
    (days + 3).rem_euclid(7) as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn days_round_trip_across_eras() {
        for days in (-200_000_000..200_000_000).step_by(9_973) {
            let (year, month, day) = civil_from_days(days);
            let year = i32::try_from(year).unwrap();
            assert!(day >= 1 && day <= days_in_month(year, month), "{days}");
            assert_eq!(days_from_civil(year, month, day), days);
        }
        assert_eq!(days_from_civil(1970, 1, 1), 0);
        assert_eq!(days_from_civil(2026, 1, 5), 20_458);
        assert_eq!(civil_from_days(-1), (1969, 12, 31));
        assert_eq!(weekday_from_monday(days_from_civil(2026, 1, 5)), 0);
        assert_eq!(weekday_from_monday(days_from_civil(2026, 12, 31)), 3);
        assert_eq!(days_in_month(2000, 2), 29);
        assert_eq!(days_in_month(1900, 2), 28);
        assert_eq!(days_in_month(-4, 2), 29);
        assert_eq!(days_in_month(2026, 13), 0);
    }
}
