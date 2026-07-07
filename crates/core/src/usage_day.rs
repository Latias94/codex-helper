use chrono::{DateTime, Duration, FixedOffset, Local, NaiveDate, TimeZone, Timelike, Utc};

const UTC_DAY_MS: u64 = 86_400_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UsageDayWindow {
    pub day: i32,
    pub start_ms: u64,
    pub end_ms: u64,
}

pub fn current_local_day() -> i32 {
    local_day_from_ms(now_ms())
}

pub fn local_day_from_ms(timestamp_ms: u64) -> i32 {
    local_datetime_from_ms(timestamp_ms)
        .and_then(|dt| day_from_date(dt.date_naive()))
        .unwrap_or_else(|| utc_day_from_ms(timestamp_ms))
}

pub fn local_hour_from_ms(timestamp_ms: u64) -> u8 {
    local_datetime_from_ms(timestamp_ms)
        .map(|dt| dt.hour() as u8)
        .unwrap_or_else(|| ((timestamp_ms / 3_600_000) % 24) as u8)
}

pub fn local_day_window(day: i32) -> Option<UsageDayWindow> {
    let date = date_from_day(day)?;
    let next_date = date.checked_add_signed(Duration::days(1))?;
    let start = Local
        .from_local_datetime(&date.and_hms_opt(0, 0, 0)?)
        .earliest()?
        .timestamp_millis();
    let end = Local
        .from_local_datetime(&next_date.and_hms_opt(0, 0, 0)?)
        .earliest()?
        .timestamp_millis();
    Some(UsageDayWindow {
        day,
        start_ms: u64::try_from(start).ok()?,
        end_ms: u64::try_from(end).ok()?,
    })
}

pub fn fixed_offset_day_from_ms(timestamp_ms: u64, offset_seconds: i32) -> Option<i32> {
    let offset = FixedOffset::east_opt(offset_seconds)?;
    let utc = utc_datetime_from_ms(timestamp_ms)?;
    day_from_date(utc.with_timezone(&offset).date_naive())
}

pub fn fixed_offset_hour_from_ms(timestamp_ms: u64, offset_seconds: i32) -> Option<u8> {
    let offset = FixedOffset::east_opt(offset_seconds)?;
    let utc = utc_datetime_from_ms(timestamp_ms)?;
    Some(utc.with_timezone(&offset).hour() as u8)
}

pub fn fixed_offset_day_window(day: i32, offset_seconds: i32) -> Option<UsageDayWindow> {
    let offset = FixedOffset::east_opt(offset_seconds)?;
    let date = date_from_day(day)?;
    let next_date = date.checked_add_signed(Duration::days(1))?;
    let start = offset
        .from_local_datetime(&date.and_hms_opt(0, 0, 0)?)
        .single()?
        .with_timezone(&Utc)
        .timestamp_millis();
    let end = offset
        .from_local_datetime(&next_date.and_hms_opt(0, 0, 0)?)
        .single()?
        .with_timezone(&Utc)
        .timestamp_millis();
    Some(UsageDayWindow {
        day,
        start_ms: u64::try_from(start).ok()?,
        end_ms: u64::try_from(end).ok()?,
    })
}

pub fn format_day(day: i32) -> String {
    date_from_day(day)
        .map(|date| date.format("%Y-%m-%d").to_string())
        .unwrap_or_else(|| "-".to_string())
}

pub fn utc_day_from_ms(timestamp_ms: u64) -> i32 {
    (timestamp_ms / UTC_DAY_MS) as i32
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn local_datetime_from_ms(timestamp_ms: u64) -> Option<DateTime<Local>> {
    let timestamp_ms = i64::try_from(timestamp_ms).ok()?;
    Local.timestamp_millis_opt(timestamp_ms).single()
}

fn utc_datetime_from_ms(timestamp_ms: u64) -> Option<DateTime<Utc>> {
    let timestamp_ms = i64::try_from(timestamp_ms).ok()?;
    Utc.timestamp_millis_opt(timestamp_ms).single()
}

pub fn day_from_date(date: NaiveDate) -> Option<i32> {
    let days = date.signed_duration_since(epoch_date()).num_days();
    i32::try_from(days).ok()
}

pub fn date_from_day(day: i32) -> Option<NaiveDate> {
    epoch_date().checked_add_signed(Duration::days(i64::from(day)))
}

fn epoch_date() -> NaiveDate {
    NaiveDate::from_ymd_opt(1970, 1, 1).expect("valid Unix epoch date")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixed_offset_day_uses_local_calendar_date() {
        let timestamp_ms = 1_704_038_400_000; // 2023-12-31T16:00:00Z
        let day = fixed_offset_day_from_ms(timestamp_ms, 8 * 60 * 60).expect("day");

        assert_eq!(format_day(day), "2024-01-01");
        assert_eq!(utc_day_from_ms(timestamp_ms), day - 1);
    }

    #[test]
    fn fixed_offset_hour_uses_local_clock_hour() {
        let timestamp_ms = 1_704_036_600_000; // 2023-12-31T15:30:00Z

        assert_eq!(
            fixed_offset_hour_from_ms(timestamp_ms, 8 * 60 * 60),
            Some(23)
        );
    }

    #[test]
    fn fixed_offset_day_window_converts_midnight_to_utc_bounds() {
        let day = day_from_date(NaiveDate::from_ymd_opt(2024, 1, 1).expect("date")).expect("day");
        let window = fixed_offset_day_window(day, 8 * 60 * 60).expect("window");

        assert_eq!(window.start_ms, 1_704_038_400_000);
        assert_eq!(window.end_ms, 1_704_124_800_000);
    }

    #[test]
    fn format_day_handles_epoch_and_before_epoch() {
        assert_eq!(format_day(0), "1970-01-01");
        assert_eq!(format_day(-1), "1969-12-31");
    }
}
