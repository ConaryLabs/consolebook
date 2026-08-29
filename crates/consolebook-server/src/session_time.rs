//! Local-time capture and resolution for training sessions (ADR 0009).
//!
//! Operators enter a business date, an IANA timezone name, and local
//! wall-clock times; the strings are stored verbatim and the UTC instants
//! are computed here, once, at entry — never re-derived later, so a
//! timezone-database change cannot rewrite what a historical session
//! said. Resolution uses the timezone database bundled into the binary
//! and RFC 5545 compatible disambiguation: a spring-forward gap rolls
//! forward, a fall-back fold takes the earlier offset.

/// Why entered time fields failed to resolve.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimeRefusal {
    /// The business date is not a real calendar date.
    InvalidBusinessDate,
    /// The timezone name is not in the IANA database.
    UnknownTimezone,
    /// A local time failed to parse or resolve.
    InvalidLocalTime,
    /// The end instant precedes the start instant.
    EndBeforeStart,
}

/// Validated time fields for a session write: the entered strings,
/// trimmed but otherwise verbatim, beside the instants resolved from
/// them.
pub struct ResolvedTimes {
    pub business_date: String,
    pub timezone: String,
    pub local_start: String,
    pub local_end: Option<String>,
    pub utc_start: i64,
    pub utc_end: Option<i64>,
}

/// Parses an operator-entered local time ("YYYY-MM-DDTHH:MM", seconds
/// optional — the `datetime-local` shape).
fn parse_local(value: &str) -> Option<jiff::civil::DateTime> {
    jiff::civil::DateTime::strptime("%Y-%m-%dT%H:%M:%S", value)
        .or_else(|_| jiff::civil::DateTime::strptime("%Y-%m-%dT%H:%M", value))
        .ok()
}

/// Resolves a local time in `tz` to a UTC unix second with RFC 5545
/// compatible disambiguation (ADR 0009).
fn resolve_local(tz: &jiff::tz::TimeZone, value: &str) -> Option<i64> {
    let local = parse_local(value)?;
    let zoned = tz.to_ambiguous_zoned(local).compatible().ok()?;
    Some(zoned.timestamp().as_second())
}

/// Validates and resolves the entered strings; the strings themselves are
/// stored verbatim (trimmed), never derived back from the instants.
pub fn resolve(
    business_date: &str,
    timezone: &str,
    local_start: &str,
    local_end: Option<&str>,
) -> Result<ResolvedTimes, TimeRefusal> {
    let business_date = business_date.trim();
    if jiff::civil::Date::strptime("%Y-%m-%d", business_date).is_err() {
        return Err(TimeRefusal::InvalidBusinessDate);
    }
    let timezone = timezone.trim();
    let Ok(tz) = jiff::tz::TimeZone::get(timezone) else {
        return Err(TimeRefusal::UnknownTimezone);
    };
    let local_start = local_start.trim();
    let Some(utc_start) = resolve_local(&tz, local_start) else {
        return Err(TimeRefusal::InvalidLocalTime);
    };
    let local_end = local_end.map(str::trim).filter(|value| !value.is_empty());
    let utc_end = match local_end {
        None => None,
        Some(value) => {
            let Some(instant) = resolve_local(&tz, value) else {
                return Err(TimeRefusal::InvalidLocalTime);
            };
            if instant < utc_start {
                return Err(TimeRefusal::EndBeforeStart);
            }
            Some(instant)
        }
    };
    Ok(ResolvedTimes {
        business_date: business_date.to_owned(),
        timezone: timezone.to_owned(),
        local_start: local_start.to_owned(),
        local_end: local_end.map(str::to_owned),
        utc_start,
        utc_end,
    })
}

/// Resolves an entered local end against a session's stored timezone
/// snapshot and start instant, for closing.
pub fn resolve_end(timezone: &str, local_end: &str, utc_start: i64) -> Result<i64, TimeRefusal> {
    let Ok(tz) = jiff::tz::TimeZone::get(timezone) else {
        return Err(TimeRefusal::UnknownTimezone);
    };
    let Some(instant) = resolve_local(&tz, local_end) else {
        return Err(TimeRefusal::InvalidLocalTime);
    };
    if instant < utc_start {
        return Err(TimeRefusal::EndBeforeStart);
    }
    Ok(instant)
}
