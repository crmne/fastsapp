//! Small formatting helpers shared by the views.

use jiff::civil::Date;
use jiff::{Timestamp, Zoned};

/// Local wall-clock time of a Unix timestamp, in the local time zone.
fn zoned(unix_seconds: i64) -> Option<Zoned> {
    let timestamp = Timestamp::from_second(unix_seconds).ok()?;
    Some(timestamp.to_zoned(jiff::tz::TimeZone::system()))
}

fn today() -> Date {
    Zoned::now().date()
}

/// "14:05", the time a message was sent, in local time.
pub fn clock(unix_seconds: i64) -> String {
    zoned(unix_seconds)
        .map(|when| format!("{:02}:{:02}", when.hour(), when.minute()))
        .unwrap_or_default()
}

/// The stamp on a chat row: the time today, "Yesterday", a weekday within
/// the week, and a date beyond it.
pub fn chat_stamp(unix_seconds: i64) -> String {
    let Some(when) = zoned(unix_seconds) else {
        return String::new();
    };
    stamp_relative_to(when.date(), today(), &when)
}

fn stamp_relative_to(date: Date, today: Date, when: &Zoned) -> String {
    let days = today
        .since(date)
        .map(|span| span.get_days())
        .unwrap_or(i32::MAX);
    match days {
        0 => format!("{:02}:{:02}", when.hour(), when.minute()),
        1 => "Yesterday".to_owned(),
        2..=6 => weekday_name(date.weekday()).to_owned(),
        _ => short_date(date),
    }
}

/// The label of a day separator in a conversation.
pub fn day_label(unix_seconds: i64) -> String {
    let Some(when) = zoned(unix_seconds) else {
        return String::new();
    };
    let date = when.date();
    let today = today();
    let days = today
        .since(date)
        .map(|span| span.get_days())
        .unwrap_or(i32::MAX);
    match days {
        0 => "Today".to_owned(),
        1 => "Yesterday".to_owned(),
        2..=6 => weekday_name(date.weekday()).to_owned(),
        _ => long_date(date),
    }
}

/// The local calendar day a timestamp falls on, for grouping messages.
pub fn day_key(unix_seconds: i64) -> Option<Date> {
    zoned(unix_seconds).map(|when| when.date())
}

fn weekday_name(weekday: jiff::civil::Weekday) -> &'static str {
    match weekday {
        jiff::civil::Weekday::Monday => "Monday",
        jiff::civil::Weekday::Tuesday => "Tuesday",
        jiff::civil::Weekday::Wednesday => "Wednesday",
        jiff::civil::Weekday::Thursday => "Thursday",
        jiff::civil::Weekday::Friday => "Friday",
        jiff::civil::Weekday::Saturday => "Saturday",
        jiff::civil::Weekday::Sunday => "Sunday",
    }
}

fn month_name(month: i8) -> &'static str {
    match month {
        1 => "January",
        2 => "February",
        3 => "March",
        4 => "April",
        5 => "May",
        6 => "June",
        7 => "July",
        8 => "August",
        9 => "September",
        10 => "October",
        11 => "November",
        _ => "December",
    }
}

fn short_date(date: Date) -> String {
    format!(
        "{} {} {}",
        date.day(),
        &month_name(date.month())[..3],
        date.year()
    )
}

fn long_date(date: Date) -> String {
    format!(
        "{}, {} {} {}",
        weekday_name(date.weekday()),
        date.day(),
        month_name(date.month()),
        date.year()
    )
}

/// The current time as a Unix timestamp.
pub fn now() -> i64 {
    Timestamp::now().as_second()
}

/// "0:12" for the length of a voice message.
pub fn duration(seconds: u32) -> String {
    format!("{}:{:02}", seconds / 60, seconds % 60)
}

/// "1.2 MB", for a document.
pub fn bytes(size: u64) -> String {
    const UNITS: [&str; 4] = ["B", "KB", "MB", "GB"];
    let mut value = size as f64;
    let mut unit = 0;
    while value >= 1000.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{size} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

/// Up to two initials, for an avatar without a picture.
pub fn initials(name: &str) -> String {
    let mut words = name
        .split(|character: char| character.is_whitespace() || character == '-')
        .filter(|word| word.chars().any(char::is_alphanumeric));
    let first = words.next();
    let last = words.next_back();
    let mut initials = String::new();
    for word in [first, last].into_iter().flatten() {
        if let Some(character) = word.chars().find(|character| character.is_alphanumeric()) {
            initials.extend(character.to_uppercase());
        }
    }
    if initials.is_empty() {
        initials.push('#');
    }
    initials
}

/// A phone number as people write it: a plus and the digits, spaced.
pub fn phone(digits: &str) -> String {
    let digits: String = digits.chars().filter(char::is_ascii_digit).collect();
    if digits.is_empty() {
        return String::new();
    }
    let mut out = String::from("+");
    for (index, character) in digits.chars().enumerate() {
        // Country code, then groups of three; imprecise for many countries
        // but readable for all of them.
        if index == 2 || (index > 2 && (index - 2) % 3 == 0) {
            out.push(' ');
        }
        out.push(character);
    }
    out
}

/// A stable hue for a name, so an avatar keeps its colour between runs.
pub fn hue(seed: &str) -> f32 {
    let mut hash: u32 = 2_166_136_261;
    for byte in seed.bytes() {
        hash ^= u32::from(byte);
        hash = hash.wrapping_mul(16_777_619);
    }
    (hash % 360) as f32
}

/// The window icon: the accent disc with a speech bubble cut out of it.
pub fn app_icon_rgba(size: usize) -> Vec<u8> {
    let mut rgba = vec![0u8; size * size * 4];
    let center = size as f32 / 2.0;
    let radius = center - 2.0;
    let bubble_center = (center, center - size as f32 * 0.03);
    let bubble_radius = size as f32 * 0.26;
    let tail = [
        (center - size as f32 * 0.14, center + size as f32 * 0.16),
        (center - size as f32 * 0.26, center + size as f32 * 0.30),
        (center - size as f32 * 0.03, center + size as f32 * 0.22),
    ];
    let sign = |a: (f32, f32), b: (f32, f32), c: (f32, f32)| {
        (a.0 - c.0) * (b.1 - c.1) - (b.0 - c.0) * (a.1 - c.1)
    };
    for y in 0..size {
        for x in 0..size {
            let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
            let distance = ((px - center).powi(2) + (py - center).powi(2)).sqrt();
            let coverage = (radius - distance + 0.5).clamp(0.0, 1.0);
            if coverage <= 0.0 {
                continue;
            }
            let bubble_distance =
                ((px - bubble_center.0).powi(2) + (py - bubble_center.1).powi(2)).sqrt();
            let in_bubble = bubble_distance <= bubble_radius;
            let d1 = sign((px, py), tail[0], tail[1]);
            let d2 = sign((px, py), tail[1], tail[2]);
            let d3 = sign((px, py), tail[2], tail[0]);
            let negative = d1 < 0.0 || d2 < 0.0 || d3 < 0.0;
            let positive = d1 > 0.0 || d2 > 0.0 || d3 > 0.0;
            let in_tail = !(negative && positive);
            let (r, g, b) = if in_bubble || in_tail {
                (255, 255, 255)
            } else {
                (0, 168, 132)
            };
            let index = (y * size + x) * 4;
            rgba[index] = r;
            rgba[index + 1] = g;
            rgba[index + 2] = b;
            rgba[index + 3] = (coverage * 255.0) as u8;
        }
    }
    rgba
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initials_take_first_and_last_word() {
        assert_eq!(initials("Ada Lovelace"), "AL");
        assert_eq!(initials("ada"), "A");
        assert_eq!(initials("  "), "#");
        assert_eq!(initials("🎉 Party Planning"), "PP");
    }

    #[test]
    fn phone_numbers_are_grouped() {
        assert_eq!(phone("393331234567"), "+39 333 123 456 7");
        assert_eq!(phone("15551234567"), "+15 551 234 567");
        assert_eq!(phone(""), "");
    }

    #[test]
    fn stamps_fall_back_to_dates() {
        let when = Timestamp::from_second(1_700_000_000)
            .expect("valid")
            .to_zoned(jiff::tz::TimeZone::UTC);
        let date = when.date();
        assert_eq!(stamp_relative_to(date, date, &when), "22:13");
        assert_eq!(
            stamp_relative_to(date, date.tomorrow().expect("date"), &when),
            "Yesterday"
        );
        assert_eq!(
            stamp_relative_to(
                date,
                date.checked_add(jiff::Span::new().days(3)).expect("date"),
                &when
            ),
            "Tuesday"
        );
        assert_eq!(
            stamp_relative_to(
                date,
                date.checked_add(jiff::Span::new().days(30)).expect("date"),
                &when
            ),
            "14 Nov 2023"
        );
    }

    #[test]
    fn sizes_and_durations_read_naturally() {
        assert_eq!(bytes(512), "512 B");
        assert_eq!(bytes(2_048), "2.0 KB");
        assert_eq!(bytes(5 * 1024 * 1024), "5.0 MB");
        assert_eq!(duration(75), "1:15");
    }

    #[test]
    fn icon_is_opaque_in_the_middle_and_clear_at_the_corners() {
        let icon = app_icon_rgba(32);
        assert_eq!(icon[3], 0);
        let middle = (16 * 32 + 16) * 4;
        assert_eq!(icon[middle + 3], 255);
    }
}
