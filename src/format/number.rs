//! Numbers with their thousands apart, and numbers of bytes as a size.

/// The binary units a size reads in, each 1024 times the one before.
const BINARY_UNITS: [&str; 7] = ["B", "KiB", "MiB", "GiB", "TiB", "PiB", "EiB"];
const BYTES_IN_KIBIBYTE: f64 = 1024.0;

/// `digits`, a run of ASCII digits, with its thousands apart: `1 732`, `120 000`.
pub(crate) fn grouped_digits(digits: &str) -> String {
    let first = digits.len() % 3;
    let groups = std::iter::once(&digits[..first]).chain((first..digits.len()).step_by(3).map(|at| &digits[at..at + 3]));
    groups.filter(|group| !group.is_empty()).collect::<Vec<_>>().join(" ")
}

pub(super) fn grouped_int(int: i64) -> String {
    let sign = if int < 0 { "-" } else { "" };
    format!("{sign}{}", grouped_digits(&int.unsigned_abs().to_string()))
}

/// A decimal's text with the thousands of its whole part apart, or `None` when it is not a plain decimal (`NaN`, `Infinity`).
pub(super) fn grouped_decimal(text: &str) -> Option<String> {
    let (sign, unsigned) = text.strip_prefix('-').map_or(("", text), |unsigned| ("-", unsigned));
    let (whole, fraction) = unsigned.find('.').map_or((unsigned, ""), |at| unsigned.split_at(at));
    let is_plain = !whole.is_empty() && whole.bytes().all(|byte| byte.is_ascii_digit());
    is_plain.then(|| format!("{sign}{}{fraction}", grouped_digits(whole)))
}

/// `bytes` in the largest binary unit that keeps it at 1 or more: `300 B`, `1.5 KiB`, `5.0 GiB`.
pub(super) fn size(bytes: f64) -> String {
    let unit = (0..BINARY_UNITS.len()).rev().find(|&step| bytes.abs() >= BYTES_IN_KIBIBYTE.powi(step as i32)).unwrap_or(0);
    match unit {
        0 => format!("{bytes:.0} B"),
        _ => format!("{:.1} {}", bytes / BYTES_IN_KIBIBYTE.powi(unit as i32), BINARY_UNITS[unit]),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn digits_show_their_thousands_apart() {
        assert_eq!(["7", "500", "1732", "120000", "1000000"].map(grouped_digits), ["7", "500", "1 732", "120 000", "1 000 000"]);
    }

    #[test]
    fn the_smallest_integer_keeps_its_sign_when_grouped() {
        assert_eq!(grouped_int(i64::MIN), "-9 223 372 036 854 775 808");
    }

    #[test]
    fn a_decimal_groups_its_whole_part_only() {
        assert_eq!(grouped_decimal("-1234.56789"), Some("-1 234.56789".to_owned()));
    }

    #[test]
    fn a_decimal_with_no_whole_digits_is_not_grouped() {
        assert_eq!([grouped_decimal("Infinity"), grouped_decimal(".5"), grouped_decimal("-")], [None, None, None]);
    }

    #[test]
    fn a_size_under_a_kibibyte_reads_in_bytes() {
        assert_eq!([size(0.0), size(1023.0)], ["0 B", "1023 B"]);
    }

    #[test]
    fn a_size_reads_in_its_largest_unit_with_one_decimal() {
        assert_eq!([size(1024.0), size(1_572_864.0), size(3.0 * 1024f64.powi(6))], ["1.0 KiB", "1.5 MiB", "3.0 EiB"]);
    }

    #[test]
    fn a_negative_size_keeps_its_sign() {
        assert_eq!(size(-2048.0), "-2.0 KiB");
    }
}
