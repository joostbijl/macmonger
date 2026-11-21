//! Text formatting in the style of SpaceMonger 1.4.

/// "13.9 GB" style size. Decimal units, so the numbers match Finder.
pub fn size_string(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["kB", "MB", "GB", "TB", "PB"];
    if bytes < 1000 {
        return format!("{} bytes", bytes);
    }
    let mut unit = 0;
    let mut div: u64 = 1000;
    while bytes / div >= 1000 && unit < UNITS.len() - 1 {
        div *= 1000;
        unit += 1;
    }
    let whole = bytes / div;
    let tenth = (bytes % div) * 10 / div;
    format!("{}.{} {}", whole, tenth, UNITS[unit])
}

/// "1,878,581,248 bytes"
pub fn bytes_string(bytes: u64) -> String {
    format!("{} bytes", group_digits(bytes))
}

pub fn group_digits(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// "34.2%" (one decimal, truncated like the original).
pub fn percent_string(part: u64, total: u64) -> String {
    let total = total.max(1) as u128;
    let permille = (part as u128 * 1000 / total) as u64;
    format!("{}.{}%", permille / 10, permille % 10)
}

/// "14 Aug 2010   16:05:49" in local time.
pub fn date_string(mtime: i64) -> String {
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let t = mtime as libc::time_t;
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    // SAFETY: both pointers are valid for the duration of the call.
    if unsafe { libc::localtime_r(&t, &mut tm) }.is_null() {
        return String::new();
    }
    let month = MONTHS[(tm.tm_mon.clamp(0, 11)) as usize];
    format!(
        "{:02} {} {:04}   {}:{:02}:{:02}",
        tm.tm_mday,
        month,
        tm.tm_year + 1900,
        tm.tm_hour,
        tm.tm_min,
        tm.tm_sec
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes() {
        assert_eq!(size_string(0), "0 bytes");
        assert_eq!(size_string(999), "999 bytes");
        assert_eq!(size_string(1000), "1.0 kB");
        assert_eq!(size_string(1_500_000), "1.5 MB");
        assert_eq!(size_string(13_949_000_000), "13.9 GB");
        assert_eq!(size_string(999_999_999_999), "999.9 GB");
        assert_eq!(size_string(2_000_000_000_000), "2.0 TB");
    }

    #[test]
    fn grouping() {
        assert_eq!(group_digits(0), "0");
        assert_eq!(group_digits(999), "999");
        assert_eq!(group_digits(1000), "1,000");
        assert_eq!(bytes_string(1_878_581_248), "1,878,581,248 bytes");
    }

    #[test]
    fn percent() {
        assert_eq!(percent_string(342, 1000), "34.2%");
        assert_eq!(percent_string(1, 0), "100.0%");
        assert_eq!(percent_string(u64::MAX / 2, u64::MAX), "49.9%");
    }

    #[test]
    fn date_has_expected_shape() {
        let s = date_string(1_281_801_949);
        assert!(s.contains("Aug 2010"), "{s}");
    }
}
