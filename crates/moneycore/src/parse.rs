//! Natural-language quick entry, e.g. "kopi 25rb kemarin", "+gaji 8,5jt tgl 25".
//! Pure rules + learned token counts, no model.

use crate::model::Category;
use chrono::{Datelike, NaiveDate, Weekday};
use std::collections::HashMap;

#[derive(Debug, Clone, Default)]
pub struct Parsed {
    pub kind: String,
    pub amount: Option<i64>,
    /// "kopi 25" read as Rp25.000
    pub assumed_thousands: bool,
    pub date: NaiveDate,
    pub date_label: String,
    pub note: String,
    pub category_id: Option<i64>,
    pub confidence: f64,
    /// money source mentioned in the text, else the default one
    pub account_id: Option<i64>,
}

const INCOME_CUES: &[&str] = &[
    "gaji", "gajian", "bonus", "thr", "dapat", "dapet", "terima", "diterima", "pemasukan",
    "masuk", "honor", "komisi", "cashback", "refund", "dividen", "dibayar", "salary",
];

const STOP: &[&str] = &[
    "di", "ke", "dari", "yang", "dan", "untuk", "buat", "sama", "utk", "beli", "bayar", "the",
    "rp", "pakai", "pake", "via", "lagi", "aja", "sm", "yg", "dgn", "dengan",
];

pub fn tokens_for_learning(text: &str) -> Vec<String> {
    let mut out: Vec<String> = text
        .split_whitespace()
        .map(norm)
        .filter(|t| t.chars().count() >= 3 && !STOP.contains(&t.as_str()))
        .filter(|t| !t.chars().all(|c| c.is_ascii_digit()))
        .collect();
    out.sort();
    out.dedup();
    out
}

fn norm(t: &str) -> String {
    t.to_lowercase()
        .chars()
        .filter(|c| c.is_alphanumeric())
        .collect()
}

fn unit_mult(s: &str) -> Option<f64> {
    match s {
        "k" | "rb" | "ribu" | "rbu" | "rban" => Some(1_000.0),
        "jt" | "juta" | "jtan" => Some(1_000_000.0),
        _ => None,
    }
}

/// Returns (amount, consumed_next_token, explicit)
fn parse_amount(tok: &str, next: Option<&str>) -> Option<(i64, bool, bool, bool)> {
    let mut t = tok.to_lowercase();
    t = t.trim_start_matches(['+', '-']).to_string();
    let mut explicit = false;
    for p in ["rp.", "rp"] {
        if let Some(rest) = t.strip_prefix(p) {
            t = rest.to_string();
            explicit = true;
            break;
        }
    }
    let num_len = t
        .char_indices()
        .take_while(|(_, c)| c.is_ascii_digit() || *c == '.' || *c == ',')
        .map(|(i, c)| i + c.len_utf8())
        .last()?;
    let (num, suffix) = t.split_at(num_len);
    if !num.chars().any(|c| c.is_ascii_digit()) {
        return None;
    }
    let suffix = suffix.trim_end_matches(['.', ',', '!']);
    let mut consumed = false;
    let mult = if suffix.is_empty() {
        match next.map(|n| n.to_lowercase()).as_deref().and_then(unit_mult) {
            Some(m) => {
                consumed = true;
                Some(m)
            }
            None => None,
        }
    } else {
        Some(unit_mult(suffix)?)
    };

    if let Some(m) = mult {
        let v: f64 = num.replace(',', ".").parse().ok()?;
        let amount = (v * m).round() as i64;
        return (amount > 0).then_some((amount, consumed, true, false));
    }

    // No unit: "25.000" / "25,000" are thousand groups, "2,5" is a decimal.
    let grouped = num.split(['.', ',']).skip(1).all(|g| g.len() == 3) && num.contains(['.', ',']);
    let v: f64 = if grouped {
        num.replace(['.', ','], "").parse().ok()?
    } else {
        num.replace(',', ".").parse().ok()?
    };
    if v <= 0.0 {
        return None;
    }
    // Bare small numbers are shorthand for thousands ("kopi 25").
    if v < 1000.0 {
        return Some(((v * 1000.0).round() as i64, false, explicit, true));
    }
    Some((v.round() as i64, false, explicit, false))
}

fn weekday_of(s: &str) -> Option<Weekday> {
    Some(match s {
        "senin" => Weekday::Mon,
        "selasa" => Weekday::Tue,
        "rabu" => Weekday::Wed,
        "kamis" => Weekday::Thu,
        "jumat" | "jum'at" => Weekday::Fri,
        "sabtu" => Weekday::Sat,
        "minggu" | "ahad" => Weekday::Sun,
        _ => return None,
    })
}

fn last_day_of_month_on_or_before(today: NaiveDate, day: u32) -> NaiveDate {
    let this = crate::util::clamp_day(today.year(), today.month(), day);
    if this <= today {
        this
    } else {
        let (y, m) = crate::util::add_months(today.year(), today.month(), -1);
        crate::util::clamp_day(y, m, day)
    }
}

pub fn parse(
    text: &str,
    today: NaiveDate,
    cats: &[Category],
    learned: &HashMap<(String, i64), i64>,
    kind_hint: Option<&str>,
) -> Parsed {
    let raw: Vec<&str> = text.split_whitespace().collect();
    let lower: Vec<String> = raw.iter().map(|t| t.to_lowercase()).collect();
    let mut used = vec![false; raw.len()];

    // Dates first so "tgl 5" never becomes Rp5.000.
    let mut date = today;
    let mut i = 0;
    while i < raw.len() {
        let w = lower[i].trim_end_matches([',', '.']);
        let next = lower.get(i + 1).map(|s| s.trim_end_matches([',', '.']));
        if w == "hari" && next == Some("ini") {
            used[i] = true;
            used[i + 1] = true;
            i += 2;
            continue;
        }
        if matches!(w, "kemarin" | "kmrn" | "kemaren" | "kmarin") {
            date = today.pred_opt().unwrap_or(today);
            used[i] = true;
        } else if let Some(wd) = weekday_of(w) {
            let back = (today.weekday().num_days_from_monday() + 7
                - wd.num_days_from_monday())
                % 7;
            date = crate::util::plus_days(today, -(back as i64));
            used[i] = true;
        } else if matches!(w, "tgl" | "tanggal") {
            if let Some(d) = next.and_then(|n| n.parse::<u32>().ok()).filter(|d| (1..=31).contains(d)) {
                date = last_day_of_month_on_or_before(today, d);
                used[i] = true;
                used[i + 1] = true;
                i += 2;
                continue;
            }
        } else if let Some((d, m)) = w.split_once('/') {
            if let (Ok(d), Ok(m)) = (d.parse::<u32>(), m.parse::<u32>()) {
                let mut y = today.year();
                if let Some(nd) = NaiveDate::from_ymd_opt(y, m, d) {
                    if nd > today {
                        y -= 1;
                    }
                    if let Some(nd) = NaiveDate::from_ymd_opt(y, m, d) {
                        date = nd;
                        used[i] = true;
                    }
                }
            }
        }
        i += 1;
    }

    // Amount: prefer explicit (unit or Rp), else first plain number.
    let mut best: Option<(usize, i64, bool, bool, bool)> = None;
    for i in 0..raw.len() {
        if used[i] {
            continue;
        }
        let next = if i + 1 < raw.len() && !used[i + 1] { Some(raw[i + 1]) } else { None };
        if let Some((amt, consumed, explicit, assumed)) = parse_amount(raw[i], next) {
            let better = match best {
                None => true,
                Some((_, _, _, bexp, _)) => explicit && !bexp,
            };
            if better {
                best = Some((i, amt, consumed, explicit, assumed));
            }
        }
    }
    let mut amount = None;
    let mut assumed_thousands = false;
    if let Some((i, amt, consumed, _, assumed)) = best {
        used[i] = true;
        if consumed {
            used[i + 1] = true;
        }
        amount = Some(amt);
        assumed_thousands = assumed;
    }

    let note_tokens: Vec<&str> = raw
        .iter()
        .enumerate()
        .filter(|(i, _)| !used[*i])
        .map(|(_, t)| t.trim_start_matches('+'))
        .filter(|t| !t.is_empty())
        .collect();
    let mut note = note_tokens.join(" ");
    if let Some(first) = note.chars().next() {
        note = first.to_uppercase().collect::<String>() + &note[first.len_utf8()..];
    }

    let norm_tokens: Vec<String> = note_tokens.iter().map(|t| norm(t)).filter(|t| !t.is_empty()).collect();
    let cue_income = text.trim_start().starts_with('+')
        || norm_tokens.iter().any(|t| INCOME_CUES.contains(&t.as_str()));

    let kind_filter: Option<&str> = match kind_hint {
        Some(k @ ("income" | "expense")) => Some(k),
        _ if cue_income => Some("income"),
        _ => None,
    };

    let (category_id, confidence, cat_kind) =
        categorize(&note.to_lowercase(), &norm_tokens, cats, learned, kind_filter);

    let kind = match kind_filter {
        Some(k) => k.to_string(),
        None => cat_kind.unwrap_or("expense").to_string(),
    };

    let category_id = category_id.or_else(|| fallback_category(cats, &kind));

    Parsed {
        date_label: describe_date(date, today),
        kind,
        amount,
        assumed_thousands,
        date,
        note,
        category_id,
        confidence,
        account_id: None,
    }
}

const VIA: &[&str] = &["pakai", "pake", "pakek", "via", "dari", "lewat", "pk", "dr", "ke"];

/// Finds a money source named in the note ("pakai gopay", "bca") and returns
/// it with the note cleaned of that mention.
pub fn detect_account(note: &str, accounts: &[crate::model::Account]) -> (Option<i64>, String) {
    let words: Vec<&str> = note.split_whitespace().collect();
    let lower: Vec<String> = words.iter().map(|w| norm(w)).collect();
    for a in accounts.iter().filter(|a| !a.archived) {
        let mut aliases: Vec<String> = a.keywords.split(',').map(|k| k.trim().to_lowercase()).filter(|k| !k.is_empty()).collect();
        aliases.push(a.name.to_lowercase());
        for alias in aliases {
            let parts: Vec<String> = alias.split_whitespace().map(norm).filter(|p| !p.is_empty()).collect();
            if parts.is_empty() || parts.len() > words.len() {
                continue;
            }
            for i in 0..=words.len() - parts.len() {
                if lower[i..i + parts.len()] == parts[..] {
                    let mut start = i;
                    if start > 0 && VIA.contains(&lower[start - 1].as_str()) {
                        start -= 1;
                    }
                    let kept: Vec<&str> = words[..start].iter().chain(words[i + parts.len()..].iter()).copied().collect();
                    let mut cleaned = kept.join(" ");
                    if let Some(f) = cleaned.chars().next() {
                        cleaned = f.to_uppercase().collect::<String>() + &cleaned[f.len_utf8()..];
                    }
                    return (Some(a.id), cleaned);
                }
            }
        }
    }
    (None, note.to_string())
}

pub fn fallback_category(cats: &[Category], kind: &str) -> Option<i64> {
    let want = if kind == "income" { "Pemasukan Lain" } else { "Lainnya" };
    cats.iter()
        .find(|c| c.kind == kind && c.name == want && !c.archived)
        .or_else(|| cats.iter().rev().find(|c| c.kind == kind && !c.archived))
        .map(|c| c.id)
}

fn categorize<'a>(
    lower_text: &str,
    tokens: &[String],
    cats: &'a [Category],
    learned: &HashMap<(String, i64), i64>,
    kind_filter: Option<&str>,
) -> (Option<i64>, f64, Option<&'a str>) {
    let mut scores: Vec<(f64, &Category)> = Vec::new();
    for c in cats.iter().filter(|c| !c.archived) {
        if let Some(k) = kind_filter {
            if c.kind != k {
                continue;
            }
        }
        let mut s = 0.0;
        for kw in c.keywords.split(',').map(str::trim).filter(|k| !k.is_empty()) {
            if kw.contains(' ') {
                if lower_text.contains(kw) {
                    s += 4.0;
                }
            } else if tokens.iter().any(|t| t == kw) {
                s += 3.0;
            } else if kw.len() >= 4 && tokens.iter().any(|t| t.starts_with(kw)) {
                s += 2.0;
            }
        }
        for t in tokens {
            if let Some(n) = learned.get(&(t.clone(), c.id)) {
                s += 2.0 * ((1 + *n) as f64).ln() + 1.0;
            }
        }
        if s > 0.0 {
            scores.push((s, c));
        }
    }
    scores.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    match scores.first() {
        None => (None, 0.0, None),
        Some((best, c)) => {
            let total: f64 = scores.iter().map(|x| x.0).sum();
            let conf = (best / total) * (1.0 - (-best / 3.0).exp());
            (Some(c.id), (conf * 100.0).round() / 100.0, Some(c.kind.as_str()))
        }
    }
}

pub fn describe_date(d: NaiveDate, today: NaiveDate) -> String {
    match crate::util::days_between(d, today) {
        0 => "Hari ini".into(),
        1 => "Kemarin".into(),
        2..=6 => format!(
            "{}, {}",
            crate::util::WEEKDAYS_ID[d.weekday().num_days_from_monday() as usize],
            crate::util::date_label(d)
        ),
        _ => crate::util::date_label(d),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;

    fn setup() -> (Vec<Category>, NaiveDate) {
        let conn = db::open_memory().unwrap();
        (db::categories(&conn).unwrap(), NaiveDate::from_ymd_opt(2026, 10, 12).unwrap())
    }

    fn cat_name(cats: &[Category], id: Option<i64>) -> String {
        cats.iter().find(|c| Some(c.id) == id).map(|c| c.name.clone()).unwrap_or_default()
    }

    #[test]
    fn amounts_and_units() {
        let (cats, today) = setup();
        let l = HashMap::new();
        let p = parse("kopi 25rb", today, &cats, &l, None);
        assert_eq!(p.amount, Some(25_000));
        assert_eq!(p.note, "Kopi");
        assert_eq!(cat_name(&cats, p.category_id), "Makan & Minum");
        assert_eq!(parse("bensin 1,5jt", today, &cats, &l, None).amount, Some(1_500_000));
        assert_eq!(parse("grab 32.500", today, &cats, &l, None).amount, Some(32_500));
        assert_eq!(parse("parkir 5 rb", today, &cats, &l, None).amount, Some(5_000));
        let p = parse("bakso 18", today, &cats, &l, None);
        assert_eq!(p.amount, Some(18_000));
        assert!(p.assumed_thousands);
        assert_eq!(parse("Rp150000 listrik", today, &cats, &l, None).amount, Some(150_000));
    }

    #[test]
    fn dates() {
        let (cats, today) = setup(); // Monday 12 Oct 2026
        let l = HashMap::new();
        assert_eq!(parse("makan 20k kemarin", today, &cats, &l, None).date.day(), 11);
        assert_eq!(parse("nonton 50k sabtu", today, &cats, &l, None).date.day(), 10);
        let p = parse("listrik 300k tgl 20", today, &cats, &l, None);
        assert_eq!((p.date.month(), p.date.day()), (9, 20));
        assert_eq!(p.amount, Some(300_000));
    }

    #[test]
    fn income_detection() {
        let (cats, today) = setup();
        let l = HashMap::new();
        let p = parse("gaji 8,5jt", today, &cats, &l, None);
        assert_eq!(p.kind, "income");
        assert_eq!(cat_name(&cats, p.category_id), "Gaji");
        let p = parse("+ jual sepeda 2jt", today, &cats, &l, None);
        assert_eq!(p.kind, "income");
    }

    #[test]
    fn learning_beats_unknown() {
        let (cats, today) = setup();
        let rumah = cats.iter().find(|c| c.name == "Rumah").unwrap().id;
        let mut l = HashMap::new();
        l.insert(("cakwe".to_string(), rumah), 3);
        let p = parse("cakwe 10k", today, &cats, &l, None);
        assert_eq!(p.category_id, Some(rumah));
        let p = parse("sesuatu 10k", today, &cats, &HashMap::new(), None);
        assert_eq!(cat_name(&cats, p.category_id), "Lainnya");
    }
}

/// Amount typed into a form field: "25000", "25.000", "25rb", "1,5 jt", "25" (= 25rb).
pub fn amount_from_text(s: &str) -> Option<i64> {
    let compact: String = s.split_whitespace().collect();
    if compact.is_empty() {
        return None;
    }
    // A form field shows thousand dots as you type, so plain digits mean
    // exactly that amount. "25" is Rp25 here; only shorthand scales.
    let core = compact.trim_start_matches("Rp").trim_start_matches("rp");
    if !core.is_empty() && core.chars().all(|c| c.is_ascii_digit() || c == '.') {
        let digits: String = core.chars().filter(|c| c.is_ascii_digit()).collect();
        return digits.parse::<i64>().ok().filter(|v| *v > 0);
    }
    parse_amount(&compact, None).map(|(a, ..)| a)
}

/// Live formatting for a money field: "1500000" -> "1.500.000". Shorthand
/// such as "1,5jt" or "500rb" is left as typed.
pub fn format_amount_input(s: &str) -> String {
    let t: String = s.chars().filter(|c| !c.is_whitespace()).collect();
    if t.is_empty() {
        return String::new();
    }
    let neg = t.starts_with('-');
    let core = t.trim_start_matches('-').trim_start_matches("Rp").trim_start_matches("rp");
    if core.chars().any(|c| c.is_alphabetic() || c == ',') {
        return s.to_string();
    }
    let digits: String = core.chars().filter(|c| c.is_ascii_digit()).collect::<String>();
    let digits = digits.trim_start_matches('0');
    if digits.is_empty() {
        return if core.contains('0') { "0".into() } else if neg { "-".into() } else { String::new() };
    }
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            out.push('.');
        }
        out.push(c);
    }
    if neg { format!("-{out}") } else { out }
}

#[cfg(test)]
mod account_tests {
    use crate::model::Account;
    fn acc(id: i64, name: &str, kw: &str) -> Account {
        Account { id, name: name.into(), kind: "bank".into(), color: "#000".into(), opening: 0, keywords: kw.into(), archived: false, sort: 0 }
    }
    #[test]
    fn finds_source() {
        let accs = vec![acc(1, "BCA", "bca"), acc(2, "GoPay", "gopay"), acc(3, "Bank Jago", "jago")];
        assert_eq!(super::detect_account("Kopi pakai gopay", &accs), (Some(2), "Kopi".to_string()));
        assert_eq!(super::detect_account("Bensin bca", &accs), (Some(1), "Bensin".to_string()));
        assert_eq!(super::detect_account("Transfer bank jago", &accs).0, Some(3));
        assert_eq!(super::detect_account("Makan siang", &accs).0, None);
    }
}

#[cfg(test)]
mod form_tests {
    #[test]
    fn form_amounts() {
        assert_eq!(super::amount_from_text("1.250.000"), Some(1_250_000));
        assert_eq!(super::amount_from_text("1,5 jt"), Some(1_500_000));
        assert_eq!(super::amount_from_text("Rp 75.000"), Some(75_000));
        assert_eq!(super::amount_from_text(""), None);
        assert_eq!(super::amount_from_text("abc"), None);
        assert_eq!(super::amount_from_text("25"), Some(25));
        assert_eq!(super::amount_from_text("12.500.000"), Some(12_500_000));
        assert_eq!(super::amount_from_text("1,5jt"), Some(1_500_000));
        assert_eq!(super::format_amount_input("1500000"), "1.500.000");
        assert_eq!(super::format_amount_input("12.5000"), "125.000");
        assert_eq!(super::format_amount_input("45000000"), "45.000.000");
        assert_eq!(super::format_amount_input("1,5jt"), "1,5jt");
        assert_eq!(super::format_amount_input("-250000"), "-250.000");
        assert_eq!(super::format_amount_input(""), "");
    }
}
