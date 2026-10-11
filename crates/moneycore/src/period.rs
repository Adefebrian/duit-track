//! Day / week / month / year views for the main dashboard, each compared with
//! the period before it.

use crate::store::Store;
use crate::util::*;
use chrono::{Datelike, NaiveDate};
use std::collections::HashMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Period {
    Day,
    Week,
    Month,
    Year,
}

impl Period {
    pub fn from_index(i: i32) -> Self {
        match i {
            0 => Period::Day,
            1 => Period::Week,
            3 => Period::Year,
            _ => Period::Month,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Bucket {
    pub label: String,
    pub income: i64,
    pub expense: i64,
    /// same slot in the previous period, for the comparison line
    pub prev_expense: i64,
    pub current: bool,
    pub future: bool,
}

#[derive(Debug, Clone)]
pub struct CatDelta {
    pub id: i64,
    pub name: String,
    pub color: String,
    pub icon: String,
    pub amount: i64,
    pub prev: i64,
}

#[derive(Debug, Clone)]
pub struct PeriodView {
    pub label: String,
    pub compare_label: &'static str,
    pub start: NaiveDate,
    pub end: NaiveDate,
    pub is_current: bool,
    pub income: i64,
    pub expense: i64,
    pub prev_income: i64,
    pub prev_expense: i64,
    pub buckets: Vec<Bucket>,
    pub categories: Vec<CatDelta>,
    pub tx_count: usize,
    pub days_elapsed: i64,
}

fn week_start(d: NaiveDate) -> NaiveDate {
    plus_days(d, -(d.weekday().num_days_from_monday() as i64))
}

/// [start, end) of the period containing `anchor`, shifted by `offset` periods.
pub fn bounds(p: Period, today: NaiveDate, offset: i32) -> (NaiveDate, NaiveDate) {
    match p {
        Period::Day => {
            let d = plus_days(today, offset as i64);
            (d, plus_days(d, 1))
        }
        Period::Week => {
            let s = plus_days(week_start(today), 7 * offset as i64);
            (s, plus_days(s, 7))
        }
        Period::Month => {
            let (y, m) = add_months(today.year(), today.month(), offset);
            let (ny, nm) = add_months(y, m, 1);
            (month_start(y, m), month_start(ny, nm))
        }
        Period::Year => {
            let y = today.year() + offset;
            (NaiveDate::from_ymd_opt(y, 1, 1).unwrap(), NaiveDate::from_ymd_opt(y + 1, 1, 1).unwrap())
        }
    }
}

fn label(p: Period, s: NaiveDate, e: NaiveDate, today: NaiveDate) -> String {
    match p {
        Period::Day => crate::parse::describe_date(s, today),
        Period::Week => {
            let last = plus_days(e, -1);
            if s.month() == last.month() {
                format!("{} - {} {}", s.day(), last.day(), &MONTHS_ID[(s.month() - 1) as usize][..3])
            } else {
                format!("{} - {}", date_label(s), date_label(last))
            }
        }
        Period::Month => month_label(s.year(), s.month()),
        Period::Year => s.year().to_string(),
    }
}

impl Store {
    pub fn period_view(&self, p: Period, offset: i32) -> R<PeriodView> {
        let today = self.today();
        let (s, e) = bounds(p, today, offset);
        let (ps, pe) = bounds(p, today, offset - 1);
        let cur = self.transactions_between(s, e)?;
        let prev = self.transactions_between(ps, pe)?;
        let sum = |v: &[crate::model::Transaction], k: &str| v.iter().filter(|t| t.kind == k).map(|t| t.amount).sum::<i64>();

        // Buckets: a day shows the last 14 days, a week its 7 days, a month its
        // days, a year its 12 months.
        let mut buckets = vec![];
        let day_map = |v: &[crate::model::Transaction]| {
            let mut m: HashMap<String, (i64, i64)> = HashMap::new();
            for t in v {
                let x = m.entry(t.date.clone()).or_default();
                if t.kind == "income" {
                    x.0 += t.amount
                } else {
                    x.1 += t.amount
                }
            }
            m
        };
        match p {
            Period::Day => {
                let from = plus_days(s, -13);
                let span = self.transactions_between(from, e)?;
                let span_prev = self.transactions_between(plus_days(from, -7), plus_days(e, -7))?;
                let m = day_map(&span);
                let mp = day_map(&span_prev);
                for i in 0..14 {
                    let d = plus_days(from, i);
                    let (inc, exp) = m.get(&d.to_string()).copied().unwrap_or_default();
                    let prev_exp = mp.get(&plus_days(d, -7).to_string()).map(|x| x.1).unwrap_or(0);
                    buckets.push(Bucket {
                        label: if i == 13 { "Ini".into() } else { d.day().to_string() },
                        income: inc,
                        expense: exp,
                        prev_expense: prev_exp,
                        current: d == s,
                        future: d > today,
                    });
                }
            }
            Period::Week | Period::Month => {
                let m = day_map(&cur);
                let mp = day_map(&prev);
                let n = (e - s).num_days();
                for i in 0..n {
                    let d = plus_days(s, i);
                    let (inc, exp) = m.get(&d.to_string()).copied().unwrap_or_default();
                    let pd = plus_days(ps, i.min((pe - ps).num_days() - 1));
                    let prev_exp = mp.get(&pd.to_string()).map(|x| x.1).unwrap_or(0);
                    buckets.push(Bucket {
                        label: if p == Period::Week { WEEKDAYS_ID[i as usize][..3].to_string() } else { d.day().to_string() },
                        income: inc,
                        expense: exp,
                        prev_expense: prev_exp,
                        current: d == today,
                        future: d > today,
                    });
                }
            }
            Period::Year => {
                for mo in 1..=12u32 {
                    let key = format!("{:04}-{:02}", s.year(), mo);
                    let pkey = format!("{:04}-{:02}", s.year() - 1, mo);
                    let inc = cur.iter().filter(|t| t.kind == "income" && t.date.starts_with(&key)).map(|t| t.amount).sum();
                    let exp = cur.iter().filter(|t| t.kind == "expense" && t.date.starts_with(&key)).map(|t| t.amount).sum();
                    let pexp = prev.iter().filter(|t| t.kind == "expense" && t.date.starts_with(&pkey)).map(|t| t.amount).sum();
                    let first = NaiveDate::from_ymd_opt(s.year(), mo, 1).unwrap();
                    buckets.push(Bucket {
                        label: MONTHS_ID[(mo - 1) as usize][..3].to_string(),
                        income: inc,
                        expense: exp,
                        prev_expense: pexp,
                        current: (s.year(), mo) == (today.year(), today.month()),
                        future: first > today,
                    });
                }
            }
        }

        // Categories with the previous period alongside.
        let cats = self.categories()?;
        let mut by: HashMap<i64, (i64, i64)> = HashMap::new();
        for t in cur.iter().filter(|t| t.kind == "expense") {
            by.entry(t.category_id.unwrap_or(0)).or_default().0 += t.amount;
        }
        for t in prev.iter().filter(|t| t.kind == "expense") {
            by.entry(t.category_id.unwrap_or(0)).or_default().1 += t.amount;
        }
        let mut categories: Vec<CatDelta> = by
            .into_iter()
            .map(|(id, (a, b))| {
                let c = cats.iter().find(|c| c.id == id);
                CatDelta {
                    id,
                    name: c.map(|c| c.name.clone()).unwrap_or_else(|| "Tanpa kategori".into()),
                    color: c.map(|c| c.color.clone()).unwrap_or_else(|| "#7C847E".into()),
                    icon: c.map(|c| c.icon.clone()).unwrap_or_else(|| "dots".into()),
                    amount: a,
                    prev: b,
                }
            })
            .filter(|c| c.amount > 0 || c.prev > 0)
            .collect();
        categories.sort_by(|a, b| b.amount.cmp(&a.amount).then(b.prev.cmp(&a.prev)));

        let is_current = s <= today && today < e;
        let days_elapsed = if is_current { (today - s).num_days() + 1 } else if e <= today { (e - s).num_days() } else { 0 };
        Ok(PeriodView {
            label: label(p, s, e, today),
            compare_label: match p {
                Period::Day => "kemarin",
                Period::Week => "minggu lalu",
                Period::Month => "bulan lalu",
                Period::Year => "tahun lalu",
            },
            start: s,
            end: e,
            is_current,
            income: sum(&cur, "income"),
            expense: sum(&cur, "expense"),
            prev_income: sum(&prev, "income"),
            prev_expense: sum(&prev, "expense"),
            buckets,
            categories,
            tx_count: cur.len(),
            days_elapsed,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bounds_cover_the_period() {
        let t = NaiveDate::from_ymd_opt(2026, 10, 14).unwrap(); // Wednesday
        let (s, e) = bounds(Period::Week, t, 0);
        assert_eq!((s.day(), e.day()), (12, 19));
        let (s, _) = bounds(Period::Month, t, -1);
        assert_eq!((s.month(), s.day()), (9, 1));
        let (s, e) = bounds(Period::Year, t, 0);
        assert_eq!((s.year(), e.year()), (2026, 2027));
    }

    #[test]
    fn views_on_demo() {
        let mut s = Store::open_memory().unwrap();
        s.set_today(NaiveDate::from_ymd_opt(2026, 10, 12).unwrap());
        s.seed_demo().unwrap();
        for (p, n) in [(Period::Day, 14), (Period::Week, 7), (Period::Month, 31), (Period::Year, 12)] {
            let v = s.period_view(p, 0).unwrap();
            assert_eq!(v.buckets.len(), n);
            assert!(v.expense > 0, "{p:?}");
        }
        let m = s.period_view(Period::Month, 0).unwrap();
        assert_eq!(m.buckets.iter().map(|b| b.expense).sum::<i64>(), m.expense);
    }
}
