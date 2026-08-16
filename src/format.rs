//! Small, locale-independent formatters used by the terminal view.
#![allow(dead_code)]

use std::time::Duration;

const UNITS: [&str; 4] = ["K", "M", "B", "T"];

fn finite(value: Option<f64>) -> Option<f64> {
    value.filter(|value| value.is_finite())
}

/// Format a USD price, retaining up to six decimal places for sub-cent prices.
pub fn format_price(value: Option<f64>) -> String {
    let Some(value) = finite(value) else {
        return "-".into();
    };
    let magnitude = value.abs();
    if magnitude == 0.0 {
        return "$0.00".into();
    }

    if magnitude < 0.01 {
        let rounded = round_to(magnitude, 6);
        if rounded == 0.0 {
            return signed(value, "$<0.000001");
        }
        let mut number = format!("{rounded:.6}");
        while number.ends_with('0') {
            number.pop();
        }
        return signed(value, &format!("${number}"));
    }

    // Rounding first makes 999.995 become $1K, rather than $1000.00.
    let rounded = round_to(magnitude, 2);
    if rounded >= 1_000.0 {
        return compact(value, "$");
    }
    signed(value, &format!("${rounded:.2}"))
}

fn signed(value: f64, positive: &str) -> String {
    if value.is_sign_negative() {
        format!("-{positive}")
    } else {
        positive.into()
    }
}

/// Format a percentage with an explicit sign for every non-zero value.
pub fn format_percentage(value: Option<f64>) -> String {
    let Some(value) = finite(value) else {
        return "-".into();
    };
    let rounded = round_to(value.abs(), 2);
    if rounded == 0.0 {
        return "0.00%".into();
    }
    let sign = if value.is_sign_negative() { '-' } else { '+' };
    if rounded >= 1_000_000.0 {
        return format!("{sign}999999.99%");
    }
    format!("{sign}{rounded:.2}%")
}

/// Format a USD amount using K, M, B, and T suffixes.
pub fn format_compact_money(value: Option<f64>) -> String {
    finite(value).map_or_else(|| "-".into(), |value| compact(value, "$"))
}

/// Format a supply using K, M, B, and T suffixes, without a currency sign.
pub fn format_compact_supply(value: Option<f64>) -> String {
    finite(value).map_or_else(|| "-".into(), |value| compact(value, ""))
}

/// Format elapsed time using whole seconds, minutes, hours, or days.
pub fn format_age(age: Duration) -> String {
    let seconds = age.as_secs();
    if seconds < 60 {
        format!("{seconds}s")
    } else if seconds < 3_600 {
        format!("{}m", seconds / 60)
    } else if seconds < 86_400 {
        format!("{}h", seconds / 3_600)
    } else {
        format!("{}d", seconds / 86_400)
    }
}

fn compact(value: f64, prefix: &str) -> String {
    let magnitude = value.abs();
    if magnitude == 0.0 {
        return format!("{prefix}0");
    }

    let mut scaled = magnitude;
    let mut unit = 0usize;
    while scaled >= 1_000.0 && unit < UNITS.len() {
        scaled /= 1_000.0;
        unit += 1;
    }

    // A rounded 999.5 of any unit belongs to the next unit. Recompute the
    // precision after promotion so K -> M -> B -> T cannot skip a unit.
    loop {
        let decimals = decimals(scaled);
        let rounded = round_to(scaled, decimals);
        if rounded == 0.0 {
            return format!("{prefix}0");
        }
        if rounded >= 1_000.0 && unit < UNITS.len() {
            scaled /= 1_000.0;
            unit += 1;
            continue;
        }
        if rounded >= 1_000.0 {
            let sign = if value.is_sign_negative() { "-" } else { "" };
            return format!("{sign}{prefix}999T+");
        }

        let sign = if value.is_sign_negative() { "-" } else { "" };
        let number = match decimals {
            0 => format!("{rounded:.0}"),
            1 => format!("{rounded:.1}"),
            _ => trim_zeroes(format!("{rounded:.2}")),
        };
        let suffix = if unit == 0 { "" } else { UNITS[unit - 1] };
        return format!("{sign}{prefix}{number}{suffix}");
    }
}

fn decimals(value: f64) -> usize {
    if value < 10.0 {
        2
    } else if value < 100.0 {
        1
    } else {
        0
    }
}

fn trim_zeroes(mut number: String) -> String {
    while number.ends_with('0') {
        number.pop();
    }
    if number.ends_with('.') {
        number.pop();
    }
    number
}

fn round_to(value: f64, decimals: usize) -> f64 {
    let factor = 10_f64.powi(decimals as i32);
    (value * factor).round() / factor
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prices_cover_rounding_signs_and_sub_cent_resolution() {
        let cases = [
            (None, "-"),
            (Some(f64::NAN), "-"),
            (Some(0.0), "$0.00"),
            (Some(-0.0), "$0.00"),
            (Some(0.0000004), "$<0.000001"),
            (Some(-0.0000004), "-$<0.000001"),
            (Some(0.000001), "$0.000001"),
            (Some(-0.009), "-$0.009"),
            (Some(12.5), "$12.50"),
            (Some(999.994), "$999.99"),
            (Some(999.995), "$1K"),
        ];
        for (input, expected) in cases {
            assert_eq!(format_price(input), expected);
        }
    }

    #[test]
    fn percentages_round_before_sign_and_cap_after_rounding() {
        let cases = [
            (Some(0.0), "0.00%"),
            (Some(-0.0), "0.00%"),
            (Some(0.0001), "0.00%"),
            (Some(-0.004), "0.00%"),
            (Some(0.005), "+0.01%"),
            (Some(-1.2), "-1.20%"),
            (Some(999_999.995), "+999999.99%"),
            (Some(-f64::MAX), "-999999.99%"),
            (Some(f64::INFINITY), "-"),
        ];
        for (input, expected) in cases {
            assert_eq!(format_percentage(input), expected);
        }
    }

    #[test]
    fn compact_values_promote_each_boundary_without_skipping_units() {
        let cases = [
            (999.49, "999"),
            (999.5, "1K"),
            (999_499.0, "999K"),
            (999_500.0, "1M"),
            (999_499_999.0, "999M"),
            (999_500_000.0, "1B"),
            (999_499_999_999.0, "999B"),
            (999_500_000_000.0, "1T"),
            (999_500_000_000_000.0, "999T+"),
            (-999.5, "-1K"),
            (-999_500.0, "-1M"),
            (-999_500_000.0, "-1B"),
            (-999_500_000_000.0, "-1T"),
            (f64::MAX, "999T+"),
            (-f64::MAX, "-999T+"),
        ];
        for (input, expected) in cases {
            assert_eq!(format_compact_supply(Some(input)), expected);
        }
        assert_eq!(format_compact_money(Some(0.0)), "$0");
        assert_eq!(format_compact_supply(Some(-0.0001)), "0");
        assert_eq!(format_compact_supply(None), "-");
    }

    #[test]
    fn ages_and_outputs_remain_bounded_at_extremes() {
        assert_eq!(format_age(Duration::MAX), "213503982334601d");
        for output in [
            format_price(Some(f64::MAX)),
            format_price(Some(-f64::MAX)),
            format_compact_money(Some(f64::MAX)),
            format_percentage(Some(f64::MAX)),
            format_age(Duration::MAX),
        ] {
            assert!(output.len() <= 32, "{output}");
        }
    }
}
