//! Turns core results into UI rows and chart paths.

use crate::{BarDatum, CatOpt, TxRow};
use chrono::{Datelike, NaiveDate};
use moneycore::model::{Account, Category, Transaction};
use moneycore::parse::describe_date;
use moneycore::util::{date_label, parse_date, rupiah, MONTHS_ID};
use slint::{Color, SharedString};

pub fn color(hex: &str) -> Color {
    let h = hex.trim_start_matches('#');
    let v = u32::from_str_radix(h, 16).unwrap_or(0x64748B);
    Color::from_rgb_u8((v >> 16) as u8, (v >> 8) as u8, v as u8)
}

pub fn rp(n: i64) -> SharedString {
    rupiah(n).into()
}

/// "1,2 jt", "850 rb"
pub fn compact(n: i64) -> String {
    let a = n.unsigned_abs() as f64;
    let sign = if n < 0 { "-" } else { "" };
    if a >= 1e9 {
        format!("{sign}{} M", trim_dec(a / 1e9))
    } else if a >= 1e6 {
        format!("{sign}{} jt", trim_dec(a / 1e6))
    } else if a >= 1e3 {
        format!("{sign}{} rb", (a / 1e3).round() as i64)
    } else {
        format!("{sign}{}", a as i64)
    }
}

fn trim_dec(v: f64) -> String {
    let s = format!("{:.1}", v);
    let s = s.trim_end_matches('0').trim_end_matches('.').to_string();
    s.replace('.', ",")
}

pub fn pct(x: f64) -> String {
    format!("{}%", (x * 100.0).round() as i64)
}

/// Raw digits for prefilling an amount field: 1250000 -> "1.250.000"
pub fn raw_amount(n: i64) -> SharedString {
    rupiah(n).trim_start_matches("Rp").to_string().into()
}

pub fn friendly_date(iso: &str, today: NaiveDate) -> String {
    match parse_date(iso) {
        Ok(d) => {
            let base = describe_date(d, today);
            if d.year() != today.year() {
                format!("{} {}", base, d.year())
            } else {
                base
            }
        }
        Err(_) => iso.to_string(),
    }
}

pub fn cat_opt(c: &Category) -> CatOpt {
    CatOpt {
        id: c.id as i32,
        name: c.name.clone().into(),
        kind: c.kind.clone().into(),
        icon: c.icon.clone().into(),
        tint: color(&c.color),
        keywords: c.keywords.clone().into(),
    }
}

pub fn acc_icon(kind: &str) -> &'static str {
    match kind {
        "bank" => "bank",
        "ewallet" => "ewallet",
        "cash" => "cash",
        _ => "wallet",
    }
}

pub fn acc_kind_label(kind: &str) -> &'static str {
    match kind {
        "bank" => "Bank",
        "ewallet" => "E-wallet",
        "cash" => "Tunai",
        _ => "Lainnya",
    }
}

pub fn tx_row(t: &Transaction, cats: &[Category], accs: &[Account], today: NaiveDate) -> TxRow {
    let c = cats.iter().find(|c| Some(c.id) == t.category_id);
    let acc = accs.iter().find(|a| Some(a.id) == t.account_id);
    TxRow {
        header: false,
        header_text: "".into(),
        header_total: "".into(),
        id: t.id as i32,
        kind: t.kind.clone().into(),
        note: t.note.clone().into(),
        cat_id: c.map(|c| c.id as i32).unwrap_or(-1),
        cat_name: c.map(|c| c.name.clone()).unwrap_or_else(|| "Tanpa kategori".into()).into(),
        icon: c.map(|c| c.icon.clone()).unwrap_or_else(|| "dots".into()).into(),
        tint: c.map(|c| color(&c.color)).unwrap_or(color("#64748B")),
        amount_text: rp(t.amount),
        amount_raw: raw_amount(t.amount),
        date: t.date.clone().into(),
        date_text: friendly_date(&t.date, today).into(),
        tag: acc.map(|a| a.name.clone()).unwrap_or_default().into(),
        acc_id: acc.map(|a| a.id as i32).unwrap_or(-1),
    }
}

/// Transactions grouped by day with a header row carrying the day's totals.
pub fn grouped(txs: &[Transaction], cats: &[Category], accs: &[Account], today: NaiveDate) -> Vec<TxRow> {
    let mut out = vec![];
    let mut i = 0;
    while i < txs.len() {
        let day = &txs[i].date;
        let mut j = i;
        let (mut out_sum, mut in_sum) = (0, 0);
        while j < txs.len() && &txs[j].date == day {
            if txs[j].kind == "income" {
                in_sum += txs[j].amount;
            } else {
                out_sum += txs[j].amount;
            }
            j += 1;
        }
        let mut total = String::new();
        if in_sum > 0 {
            total.push_str(&format!("+{}", rupiah(in_sum)));
        }
        if out_sum > 0 {
            if !total.is_empty() {
                total.push_str("   ");
            }
            total.push_str(&format!("-{}", rupiah(out_sum)));
        }
        let mut header = tx_row(&txs[i], cats, accs, today);
        header.header = true;
        header.header_text = friendly_date(day, today).to_uppercase().into();
        header.header_total = total.into();
        out.push(header);
        for t in &txs[i..j] {
            out.push(tx_row(t, cats, accs, today));
        }
        i = j;
    }
    out
}

pub fn month_label(y: i32, m: u32) -> String {
    format!("{} {}", MONTHS_ID[(m - 1) as usize], y)
}

pub fn short_date(iso: &str) -> String {
    parse_date(iso).map(date_label).unwrap_or_else(|_| iso.to_string())
}

// ---------- chart paths (viewbox 1000 x 300) ----------

pub const VW: f64 = 1000.0;
pub const VH: f64 = 300.0;

fn pt(x: f64, y: f64) -> String {
    format!("{:.1} {:.1}", x, y)
}

pub fn line_path(values: &[f64], max: f64, slots: usize) -> String {
    if values.is_empty() || max <= 0.0 {
        return String::new();
    }
    let step = VW / (slots.max(2) - 1) as f64;
    let mut s = String::new();
    for (i, v) in values.iter().enumerate() {
        let y = VH - (v / max).clamp(0.0, 1.0) * (VH - 4.0) - 2.0;
        s.push_str(if i == 0 { "M " } else { " L " });
        s.push_str(&pt(i as f64 * step, y));
    }
    if values.len() == 1 {
        s.push_str(&format!(" L {}", pt(step * 0.5, VH - (values[0] / max).clamp(0.0, 1.0) * (VH - 4.0) - 2.0)));
    }
    s
}

pub fn area_path(values: &[f64], max: f64, slots: usize) -> String {
    let line = line_path(values, max, slots);
    if line.is_empty() {
        return line;
    }
    let step = VW / (slots.max(2) - 1) as f64;
    let last_x = (values.len().max(2) - 1) as f64 * step;
    let last_x = if values.len() == 1 { step * 0.5 } else { last_x };
    format!("{line} L {} L {} Z", pt(last_x, VH), pt(0.0, VH))
}

/// Line over a signed range: maps [lo, hi] onto the box.
pub fn signed_path(values: &[f64], lo: f64, hi: f64) -> String {
    if values.is_empty() || hi <= lo {
        return String::new();
    }
    let step = VW / (values.len().max(2) - 1) as f64;
    let mut s = String::new();
    for (i, v) in values.iter().enumerate() {
        let y = VH - ((v - lo) / (hi - lo)).clamp(0.0, 1.0) * (VH - 8.0) - 4.0;
        s.push_str(if i == 0 { "M " } else { " L " });
        s.push_str(&pt(i as f64 * step, y));
    }
    s
}

pub fn bars(rows: &[(String, i64, i64)], current_last: bool) -> Vec<BarDatum> {
    let max = rows.iter().map(|r| r.1.max(r.2)).max().unwrap_or(0).max(1) as f32;
    let n = rows.len();
    rows.iter()
        .enumerate()
        .map(|(i, (label, a, b))| BarDatum {
            label: label.clone().into(),
            a: *a as f32 / max,
            b: *b as f32 / max,
            a_text: compact(*a).into(),
            b_text: compact(*b).into(),
            current: current_last && i == n - 1,
        })
        .collect()
}

/// Normalises a series into [0, 1] between its own min and max (with padding)
/// so small movements of a large balance stay visible.
pub fn normalise(values: &[f64]) -> Vec<f64> {
    let lo = values.iter().cloned().fold(f64::INFINITY, f64::min);
    let hi = values.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
    if !lo.is_finite() || !hi.is_finite() {
        return vec![];
    }
    let pad = ((hi - lo) * 0.15).max(hi.abs() * 0.02).max(1.0);
    values.iter().map(|v| (v - lo + pad) / (hi - lo + 2.0 * pad)).collect()
}

/// Donut slices as arcs on a 100 x 100 box, radius 40, small gaps between.
pub fn donut_paths(shares: &[f64]) -> Vec<String> {
    let (cx, cy, r) = (50.0f64, 50.0f64, 40.0f64);
    let total: f64 = shares.iter().sum();
    if total <= 0.0 {
        return vec![];
    }
    if shares.iter().filter(|s| **s > 0.0).count() == 1 {
        return shares
            .iter()
            .map(|s| if *s > 0.0 { format!("M {cx} {} A {r} {r} 0 1 1 {} {} A {r} {r} 0 1 1 {cx} {}", cy - r, cx - 0.01, cy - r, cy - r) } else { String::new() })
            .collect();
    }
    let gap = 0.035; // radians
    let mut a = -std::f64::consts::FRAC_PI_2;
    shares
        .iter()
        .map(|s| {
            let sweep = s / total * std::f64::consts::TAU;
            let (a0, a1) = (a + gap / 2.0, a + sweep - gap / 2.0);
            a += sweep;
            if a1 <= a0 {
                return String::new();
            }
            let large = if a1 - a0 > std::f64::consts::PI { 1 } else { 0 };
            format!(
                "M {:.2} {:.2} A {r} {r} 0 {large} 1 {:.2} {:.2}",
                cx + r * a0.cos(),
                cy + r * a0.sin(),
                cx + r * a1.cos(),
                cy + r * a1.sin()
            )
        })
        .collect()
}

/// Maps a path on the 1000 x 300 chart grid to `w` x `h` pixels. Slint keeps
/// the viewbox aspect ratio, so charts are drawn at their real size instead.
pub fn fit_path(path: &str, w: f32, h: f32) -> String {
    if path.is_empty() || w <= 0.0 || h <= 0.0 {
        return String::new();
    }
    let (sx, sy) = (w as f64 / VW, h as f64 / VH);
    let mut out = String::with_capacity(path.len());
    let mut is_x = true;
    for tok in path.split_whitespace() {
        if let Ok(v) = tok.parse::<f64>() {
            let scaled = if is_x { v * sx } else { v * sy };
            out.push_str(&format!("{scaled:.1} "));
            is_x = !is_x;
        } else {
            out.push_str(tok);
            out.push(' ');
            is_x = true;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    #[test]
    fn fit_scales_axes_independently() {
        assert_eq!(super::fit_path("M 0 300 L 1000 0 Z", 500.0, 150.0).trim(), "M 0.0 150.0 L 500.0 0.0 Z");
    }
}

/// Line through bar centres on the 1000 x 300 grid; values are already
/// normalised to [0, 1] of the bar scale so the line sits on the same axis.
pub fn centered_line(values: &[f32]) -> String {
    let n = values.len().max(1) as f64;
    let mut s = String::new();
    for (i, v) in values.iter().enumerate() {
        let x = (i as f64 + 0.5) / n * VW;
        let y = VH - (*v as f64).clamp(0.0, 1.0) * VH;
        s.push_str(if i == 0 { "M " } else { " L " });
        s.push_str(&format!("{x:.1} {y:.1}"));
    }
    s
}
