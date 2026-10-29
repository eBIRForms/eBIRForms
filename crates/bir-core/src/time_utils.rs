use chrono::{DateTime, Local};

/// Formats an RFC3339 string into a human-readable relative time string.
/// Examples:
/// "in 2 minutes (10:45 AM)"
/// "tomorrow at 9:00 AM (Apr 30)"
/// "in 3 hours (1:00 PM)"
pub fn format_next_run(rfc3339: &str) -> String {
    let Ok(dt) = DateTime::parse_from_rfc3339(rfc3339) else {
        return rfc3339.to_string(); // fallback to raw if parsing fails
    };

    let dt_local = dt.with_timezone(&Local);
    let now = Local::now();

    let duration = dt_local.signed_duration_since(now);
    let seconds = duration.num_seconds();
    let minutes = duration.num_minutes();
    let hours = duration.num_hours();
    let days = duration.num_days();

    let time_str = dt_local.format("%-I:%M %p").to_string();
    let date_str = dt_local.format("%b %-d").to_string();

    let is_tomorrow =
        dt_local.date_naive() == now.date_naive().succ_opt().unwrap_or(now.date_naive());
    let is_today = dt_local.date_naive() == now.date_naive();

    if seconds < 0 {
        return "past due".to_string();
    }

    if is_tomorrow {
        format!("tomorrow at {} ({})", time_str, date_str)
    } else if is_today {
        if hours >= 1 {
            let noun = if hours == 1 { "hour" } else { "hours" };
            format!("in {} {} ({})", hours, noun, time_str)
        } else if minutes >= 1 {
            let noun = if minutes == 1 { "minute" } else { "minutes" };
            format!("in {} {} ({})", minutes, noun, time_str)
        } else {
            let noun = if seconds == 1 { "second" } else { "seconds" };
            format!("in {} {} ({})", seconds, noun, time_str)
        }
    } else {
        if days >= 1 {
            let noun = if days == 1 { "day" } else { "days" };
            format!("in {} {} ({} at {})", days, noun, date_str, time_str)
        } else {
            format!("at {} ({})", time_str, date_str)
        }
    }
}
