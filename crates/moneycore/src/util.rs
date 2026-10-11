use chrono::{Datelike, Duration, NaiveDate};
use std::fmt::Display;

pub type R<T> = Result<T, String>;

pub trait OrStr<T> {
    fn s(self) -> R<T>;
}

impl<T, E: Display> OrStr<T> for Result<T, E> {
    fn s(self) -> R<T> {
        self.map_err(|e| e.to_string())
    }
}

pub fn parse_date(s: &str) -> R<NaiveDate> {
    NaiveDate::parse_from_str(s, "%Y-%m-%d").map_err(|_| format!("tanggal tidak valid: {s}"))
}

pub fn parse_month(m: &str) -> R<(i32, u32)> {
    let (y, mo) = m.split_once('-').ok_or_else(|| format!("bulan tidak valid: {m}"))?;
    let y: i32 = y.parse().map_err(|_| format!("bulan tidak valid: {m}"))?;
    let mo: u32 = mo.parse().map_err(|_| format!("bulan tidak valid: {m}"))?;
    if !(1..=12).contains(&mo) || !(1970..=2200).contains(&y) {
        return Err(format!("bulan tidak valid: {m}"));
    }
    Ok((y, mo))
}

pub fn month_start(y: i32, m: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(y, m, 1).expect("valid month")
}

pub fn add_months(y: i32, m: u32, delta: i32) -> (i32, u32) {
    let idx = y * 12 + (m as i32 - 1) + delta;
    (idx.div_euclid(12), (idx.rem_euclid(12) + 1) as u32)
}

pub fn days_in_month(y: i32, m: u32) -> u32 {
    let (ny, nm) = add_months(y, m, 1);
    (month_start(ny, nm) - month_start(y, m)).num_days() as u32
}

pub fn month_key(y: i32, m: u32) -> String {
    format!("{y:04}-{m:02}")
}

pub fn month_of(d: NaiveDate) -> String {
    month_key(d.year(), d.month())
}

/// [start, end) as ISO date strings, for `date >= ? AND date < ?`.
pub fn month_bounds(y: i32, m: u32) -> (String, String) {
    let (ny, nm) = add_months(y, m, 1);
    (month_start(y, m).to_string(), month_start(ny, nm).to_string())
}

/// Same day-of-month in a target month, clamped to that month's length.
pub fn clamp_day(y: i32, m: u32, day: u32) -> NaiveDate {
    let d = day.clamp(1, days_in_month(y, m));
    NaiveDate::from_ymd_opt(y, m, d).expect("clamped day")
}

pub fn next_due(today: NaiveDate, due_day: u32) -> NaiveDate {
    let this = clamp_day(today.year(), today.month(), due_day);
    if this >= today {
        this
    } else {
        let (y, m) = add_months(today.year(), today.month(), 1);
        clamp_day(y, m, due_day)
    }
}

pub fn days_between(a: NaiveDate, b: NaiveDate) -> i64 {
    (b - a).num_days()
}

pub fn plus_days(d: NaiveDate, n: i64) -> NaiveDate {
    d + Duration::days(n)
}

pub fn median(v: &mut [f64]) -> f64 {
    if v.is_empty() {
        return 0.0;
    }
    v.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let n = v.len();
    if n % 2 == 1 {
        v[n / 2]
    } else {
        (v[n / 2 - 1] + v[n / 2]) / 2.0
    }
}

pub fn mean_sd(v: &[f64]) -> (f64, f64) {
    if v.is_empty() {
        return (0.0, 0.0);
    }
    let n = v.len() as f64;
    let mean = v.iter().sum::<f64>() / n;
    let var = v.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / n;
    (mean, var.sqrt())
}

/// Round up to a friendly budgeting step (Rp 10rb under 500rb, Rp 50rb above).
pub fn round_budget(x: f64) -> i64 {
    if x <= 0.0 {
        return 0;
    }
    let step = if x < 500_000.0 { 10_000.0 } else { 50_000.0 };
    ((x / step).ceil() * step) as i64
}

pub fn rupiah(n: i64) -> String {
    let neg = n < 0;
    let s = n.unsigned_abs().to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i) % 3 == 0 {
            out.push('.');
        }
        out.push(c);
    }
    format!("{}Rp{}", if neg { "-" } else { "" }, out)
}

pub const MONTHS_ID: [&str; 12] = [
    "Januari", "Februari", "Maret", "April", "Mei", "Juni", "Juli", "Agustus", "September",
    "Oktober", "November", "Desember",
];

pub const WEEKDAYS_ID: [&str; 7] = ["Senin", "Selasa", "Rabu", "Kamis", "Jumat", "Sabtu", "Minggu"];

pub fn month_label(y: i32, m: u32) -> String {
    format!("{} {}", MONTHS_ID[(m - 1) as usize], y)
}

pub fn date_label(d: NaiveDate) -> String {
    format!("{} {}", d.day(), &MONTHS_ID[(d.month() - 1) as usize][..3])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn month_math() {
        assert_eq!(add_months(2026, 1, -1), (2025, 12));
        assert_eq!(add_months(2026, 12, 1), (2027, 1));
        assert_eq!(days_in_month(2024, 2), 29);
        assert_eq!(clamp_day(2026, 2, 31).day(), 28);
        assert_eq!(rupiah(1250000), "Rp1.250.000");
        assert_eq!(round_budget(432_100.0), 440_000);
    }
}
