use chrono::{DateTime, Days, LocalResult, NaiveDate, TimeZone, Timelike};

/// Returns the next daily run in a caller-supplied timezone. `earliest()` resolves the
/// repeated DST hour deterministically; a nonexistent hour advances to the next valid day.
pub fn next_daily_run<Tz: TimeZone>(now: DateTime<Tz>, hour: u8) -> Option<DateTime<Tz>> {
    if hour > 23 {
        return None;
    }
    let mut date = now.date_naive();
    for _ in 0..4 {
        if let Some(candidate) =
            at_or_after_hour(&now.timezone(), date, hour).filter(|candidate| candidate > &now)
        {
            return Some(candidate);
        }
        date = date.checked_add_days(Days::new(1))?;
    }
    None
}

/// Most recent scheduled instant. Only this instant is considered for catch-up, so Web never
/// replays every interval missed while it was closed.
pub fn most_recent_daily_run<Tz: TimeZone>(now: DateTime<Tz>, hour: u8) -> Option<DateTime<Tz>> {
    if hour > 23 {
        return None;
    }
    let today = at_or_after_hour(&now.timezone(), now.date_naive(), hour);
    if let Some(candidate) = today.filter(|candidate| candidate <= &now) {
        return Some(candidate);
    }
    let yesterday = now.date_naive().checked_sub_days(Days::new(1))?;
    at_or_after_hour(&now.timezone(), yesterday, hour)
}

fn at_or_after_hour<Tz: TimeZone>(
    timezone: &Tz,
    date: NaiveDate,
    hour: u8,
) -> Option<DateTime<Tz>> {
    // On a nonexistent DST hour, advance within the same day rather than silently dropping it.
    for offset in 0..=3_u32 {
        let candidate_hour = u32::from(hour).saturating_add(offset);
        if candidate_hour > 23 {
            break;
        }
        let local = date.and_hms_opt(candidate_hour, 0, 0)?;
        match timezone.from_local_datetime(&local) {
            LocalResult::Single(value) => return Some(value),
            LocalResult::Ambiguous(first, _) => return Some(first),
            LocalResult::None => {}
        }
    }
    None
}

pub fn in_quiet_hours(hour: u32, start: u8, end: u8) -> bool {
    let start = u32::from(start);
    let end = u32::from(end);
    if start == end {
        return false;
    }
    if start < end {
        hour >= start && hour < end
    } else {
        hour >= start || hour < end
    }
}

pub fn should_run_one_catch_up(
    last_scheduled_ms: i64,
    last_success_ms: Option<i64>,
    now_ms: i64,
) -> bool {
    last_scheduled_ms <= now_ms && last_success_ms.is_none_or(|success| success < last_scheduled_ms)
}

pub fn next_eligible_run<Tz: TimeZone>(
    now: DateTime<Tz>,
    enabled: bool,
    schedule_hour: u8,
    quiet_start: u8,
    quiet_end: u8,
    last_success_ms: Option<i64>,
) -> Option<DateTime<Tz>> {
    if !enabled {
        return None;
    }
    let most_recent = most_recent_daily_run(now.clone(), schedule_hour)?;
    if should_run_one_catch_up(
        most_recent.timestamp_millis(),
        last_success_ms,
        now.timestamp_millis(),
    ) {
        return if in_quiet_hours(now.hour(), quiet_start, quiet_end) {
            quiet_end_after(now, quiet_start, quiet_end)
        } else {
            Some(now)
        };
    }
    let next = next_daily_run(now, schedule_hour)?;
    if in_quiet_hours(next.hour(), quiet_start, quiet_end) {
        quiet_end_after(next, quiet_start, quiet_end)
    } else {
        Some(next)
    }
}

fn quiet_end_after<Tz: TimeZone>(value: DateTime<Tz>, start: u8, end: u8) -> Option<DateTime<Tz>> {
    if start == end || !in_quiet_hours(value.hour(), start, end) {
        return Some(value);
    }
    let mut date = value.date_naive();
    if start > end && value.hour() >= u32::from(start) {
        date = date.checked_add_days(Days::new(1))?;
    }
    at_or_after_hour(&value.timezone(), date, end)
}

pub fn scheduled_due<Tz: TimeZone>(
    now: DateTime<Tz>,
    enabled: bool,
    schedule_hour: u8,
    quiet_start: u8,
    quiet_end: u8,
    last_success_ms: Option<i64>,
) -> Option<i64> {
    if !enabled || in_quiet_hours(now.hour(), quiet_start, quiet_end) {
        return None;
    }
    let scheduled = most_recent_daily_run(now.clone(), schedule_hour)?.timestamp_millis();
    should_run_one_catch_up(scheduled, last_success_ms, now.timestamp_millis()).then_some(scheduled)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{Datelike, FixedOffset, Offset};
    use chrono_tz::America::New_York;

    #[test]
    fn next_run_and_single_catch_up_use_only_nearest_instant() {
        let zone = FixedOffset::west_opt(5 * 3600).expect("offset");
        let now = zone
            .with_ymd_and_hms(2026, 7, 26, 9, 30, 0)
            .single()
            .expect("time");
        let next = next_daily_run(now, 8).expect("next");
        assert_eq!(next.date_naive().day(), 27);
        let due = scheduled_due(now, true, 8, 21, 7, None).expect("due");
        assert_eq!(
            due,
            zone.with_ymd_and_hms(2026, 7, 26, 8, 0, 0)
                .single()
                .expect("scheduled")
                .timestamp_millis()
        );
        assert!(scheduled_due(now, true, 8, 21, 7, Some(due)).is_none());
    }

    #[test]
    fn quiet_hours_wrap_midnight_and_disable_work() {
        assert!(in_quiet_hours(22, 21, 7));
        assert!(in_quiet_hours(2, 21, 7));
        assert!(!in_quiet_hours(12, 21, 7));
        let zone = FixedOffset::east_opt(0).expect("offset");
        let quiet = zone
            .with_ymd_and_hms(2026, 7, 26, 22, 0, 0)
            .single()
            .expect("time");
        assert!(scheduled_due(quiet, true, 8, 21, 7, None).is_none());
        let eligible = next_eligible_run(quiet, true, 22, 21, 7, None).expect("eligible");
        assert_eq!(eligible.hour(), 7);
        assert_eq!(eligible.date_naive().day(), 27);
    }

    #[test]
    fn timezone_dst_gap_and_repeat_have_deterministic_real_zone_semantics() {
        let before_gap = New_York
            .with_ymd_and_hms(2026, 3, 8, 1, 30, 0)
            .single()
            .expect("before gap");
        let gap = next_daily_run(before_gap, 2).expect("gap advances");
        assert_eq!(gap.hour(), 3);
        let before_repeat = New_York
            .with_ymd_and_hms(2026, 11, 1, 0, 30, 0)
            .single()
            .expect("before repeat");
        let repeated = next_daily_run(before_repeat, 1).expect("repeat");
        assert_eq!(repeated.hour(), 1);
        assert_eq!(repeated.offset().fix().local_minus_utc(), -4 * 3_600);
    }
}
