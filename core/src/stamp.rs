//! Dates in backup names and metadata: "2026-11-09T02:09:19.179Z", "shop_2026-11-09T02-08-46-592Z",
//! "db_2026-03-17_03-46-03", "name_20260921-160651". All are read as UTC.

/// Days since 1970-01-01 for a proleptic Gregorian date (Howard Hinnant's algorithm).
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let doy = (153 * ((m + 9) % 12) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// Seconds since 1970 from the digits of a date-time: YYYYMMDD, optionally followed by hhmm[ss].
pub fn parse(s: &str) -> Option<u64> {
    let d: Vec<i64> = s.bytes().filter(u8::is_ascii_digit).map(|b| i64::from(b - b'0')).take(14).collect();
    if d.len() < 8 {
        return None;
    }
    let num = |from: usize, len: usize| d.get(from..from + len).map(|x| x.iter().fold(0, |a, b| a * 10 + b));
    let (y, mo, day) = (num(0, 4)?, num(4, 2)?, num(6, 2)?);
    let (h, mi, sec) = (num(8, 2).unwrap_or(0), num(10, 2).unwrap_or(0), num(12, 2).unwrap_or(0));
    if !(1970..=9999).contains(&y) || !(1..=12).contains(&mo) || !(1..=31).contains(&day) || h > 23 || mi > 59 || sec > 60 {
        return None;
    }
    let secs = days_from_civil(y, mo, day) * 86_400 + h * 3600 + mi * 60 + sec;
    u64::try_from(secs).ok()
}

/// Splits a backup name into the part before its date and the date run itself.
/// "shop_2026-11-09T02-08-46-592Z" → ("shop", "2026-11-09T02-08-46-592Z").
pub fn split_name(stem: &str) -> Option<(&str, &str)> {
    let b = stem.as_bytes();
    let digit = |i: usize| b.get(i).is_some_and(u8::is_ascii_digit);
    let iso = |i: usize| {
        b[i] == b'2' && b.get(i + 1) == Some(&b'0') && digit(i + 2) && digit(i + 3) && b.get(i + 4) == Some(&b'-')
            && digit(i + 5) && digit(i + 6) && b.get(i + 7) == Some(&b'-') && digit(i + 8) && digit(i + 9)
    };
    let compact = |i: usize| b[i] == b'2' && b.get(i + 1) == Some(&b'0') && (i + 2..i + 8).all(digit) && !digit(i + 8);
    let start = (1..b.len()).filter(|&i| !digit(i - 1)).find(|&i| iso(i) || compact(i))?;
    let len = b[start..]
        .iter()
        .take_while(|c| c.is_ascii_digit() || matches!(c, b'-' | b'_' | b'T' | b'Z' | b':' | b'.'))
        .count();
    let prefix = stem[..start].trim_end_matches(['_', '-', ' ', '.']);
    (!prefix.is_empty()).then(|| (prefix, stem[start..start + len].trim_end_matches(['.', '-', '_'])))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_known_formats() {
        let t = 1_762_654_159; // 2025-11-09T02:09:19Z
        assert_eq!(parse("2025-11-09T02:09:19.179Z"), Some(t));
        assert_eq!(parse("2025-11-09T02-09-19-592Z"), Some(t));
        assert_eq!(parse("20251109-020919"), Some(t));
        assert_eq!(parse("2025-11-09"), Some(t - 2 * 3600 - 9 * 60 - 19));
        assert_eq!(parse("2025-13-01"), None);
        assert_eq!(parse("2025"), None);
    }

    #[test]
    fn splits_names() {
        assert_eq!(split_name("shop_2026-11-09T02-08-46-592Z"), Some(("shop", "2026-11-09T02-08-46-592Z")));
        assert_eq!(split_name("VietNguyen_PRO_2026-03-17_03-46-03"), Some(("VietNguyen_PRO", "2026-03-17_03-46-03")));
        assert_eq!(
            split_name("example_before_migration_20260921-160651"),
            Some(("example_before_migration", "20260921-160651"))
        );
        assert_eq!(split_name("north-shop_2026-10-21T02-00-57-581Z"), Some(("north-shop", "2026-10-21T02-00-57-581Z")));
        assert_eq!(split_name("2026-10-21 notes"), None, "a date alone has no tenant");
        assert_eq!(split_name("report"), None);
    }
}
