//! Multi-month financial performance: trends, health score, category drift.

use crate::store::Store;
use crate::util::*;
use std::collections::HashMap;

#[derive(Debug, Clone)]
pub struct MonthPerf {
    pub label: String,
    pub key: String,
    pub income: i64,
    pub expense: i64,
    pub net: i64,
    pub savings_rate: f64,
    pub budget_total: i64,
    /// share of budgeted categories that stayed within plan, None if no budget
    pub adherence: Option<f64>,
    pub debt_paid: i64,
}

#[derive(Debug, Clone)]
pub struct HealthPart {
    pub name: &'static str,
    pub score: f64,
    pub max: f64,
    pub detail: String,
}

#[derive(Debug, Clone)]
pub struct CatTrend {
    pub name: String,
    pub color: String,
    pub values: Vec<i64>,
    pub total: i64,
    /// last full month vs average of the earlier months
    pub change: Option<f64>,
}

#[derive(Debug, Clone)]
pub struct Analytics {
    pub months: Vec<MonthPerf>,
    pub income_total: i64,
    pub expense_total: i64,
    pub net_total: i64,
    pub avg_savings_rate: f64,
    pub avg_expense: i64,
    pub avg_income: i64,
    pub best: Option<(String, i64)>,
    pub worst: Option<(String, i64)>,
    pub volatility: f64,
    pub debt_to_income: f64,
    pub health: f64,
    pub health_label: &'static str,
    pub health_parts: Vec<HealthPart>,
    pub categories: Vec<CatTrend>,
    pub top_spend: Vec<(String, i64, usize)>,
    pub summary: String,
    /// (name, color, amount, share) of spending over the period, top 5 + rest
    pub composition: Vec<(String, String, i64, f64)>,
    /// average variable spend per weekday, Monday first
    pub weekday_avg: [i64; 7],
    /// total money across sources at each month end (current month = now)
    pub pool_months: Vec<i64>,
}

impl Store {
    /// `span` months ending at the current month.
    pub fn analytics(&self, span: u32) -> R<Analytics> {
        use chrono::Datelike;
        let span = span.clamp(3, 24) as i32;
        let today = self.today();
        let (cy, cm) = (today.year(), today.month());
        let (sy, sm) = add_months(cy, cm, -(span - 1));
        let (ny, nm) = add_months(cy, cm, 1);
        let txs = self.transactions_between(month_start(sy, sm), month_start(ny, nm))?;
        let cats = self.categories()?;

        // Only months that actually contain data count toward averages.
        let mut months = vec![];
        let mut cat_vals: HashMap<i64, Vec<i64>> = HashMap::new();
        for k in 0..span {
            let (y, m) = add_months(sy, sm, k);
            let key = month_key(y, m);
            let mtx: Vec<_> = txs.iter().filter(|t| t.date.starts_with(&key)).collect();
            let income: i64 = mtx.iter().filter(|t| t.kind == "income").map(|t| t.amount).sum();
            let expense: i64 = mtx.iter().filter(|t| t.kind == "expense").map(|t| t.amount).sum();
            let debt_paid: i64 = mtx.iter().filter(|t| t.debt_id.is_some()).map(|t| t.amount).sum();
            let mut spent: HashMap<i64, i64> = HashMap::new();
            for t in mtx.iter().filter(|t| t.kind == "expense") {
                *spent.entry(t.category_id.unwrap_or(0)).or_default() += t.amount;
            }
            for (c, v) in &spent {
                cat_vals.entry(*c).or_insert_with(|| vec![0; span as usize])[k as usize] = *v;
            }
            let budgets = crate::db::budgets_for(&self.conn, &key)?;
            let budget_total = budgets.iter().map(|b| b.1).sum();
            let adherence = (!budgets.is_empty() && (y, m) != (cy, cm)).then(|| {
                let ok = budgets.iter().filter(|(c, p)| spent.get(c).copied().unwrap_or(0) <= *p).count();
                ok as f64 / budgets.len() as f64
            });
            months.push(MonthPerf {
                label: MONTHS_ID[(m - 1) as usize][..3].to_string(),
                key,
                income,
                expense,
                net: income - expense,
                savings_rate: if income > 0 { (income - expense) as f64 / income as f64 } else { 0.0 },
                budget_total,
                adherence,
                debt_paid,
            });
        }

        let active: Vec<&MonthPerf> = months.iter().filter(|m| m.income > 0 || m.expense > 0).collect();
        // Full months only for averages, the running month would drag them down.
        let full: Vec<&MonthPerf> = active.iter().copied().filter(|m| m.key != month_key(cy, cm)).collect();
        let basis: Vec<&MonthPerf> = if full.is_empty() { active.clone() } else { full.clone() };
        let n = basis.len().max(1) as f64;

        let income_total: i64 = active.iter().map(|m| m.income).sum();
        let expense_total: i64 = active.iter().map(|m| m.expense).sum();
        let avg_income = (basis.iter().map(|m| m.income).sum::<i64>() as f64 / n) as i64;
        let avg_expense = (basis.iter().map(|m| m.expense).sum::<i64>() as f64 / n) as i64;
        let avg_savings_rate = if avg_income > 0 { (avg_income - avg_expense) as f64 / avg_income as f64 } else { 0.0 };
        let best = basis.iter().max_by_key(|m| m.net).map(|m| (m.label.clone(), m.net));
        let worst = basis.iter().min_by_key(|m| m.net).map(|m| (m.label.clone(), m.net));
        let exp: Vec<f64> = basis.iter().map(|m| m.expense as f64).collect();
        let (mean, sd) = mean_sd(&exp);
        let volatility = if mean > 0.0 { sd / mean } else { 0.0 };

        let debts = self.debt_views()?;
        let min_total: i64 = debts.iter().filter(|d| d.balance > 0 && !d.debt.closed).map(|d| d.debt.min_payment.min(d.balance)).sum();
        let debt_balance: i64 = debts.iter().filter(|d| !d.debt.closed).map(|d| d.balance).sum();
        let debt_to_income = if avg_income > 0 { min_total as f64 / avg_income as f64 } else if min_total > 0 { 1.0 } else { 0.0 };
        let trapped = debts.iter().any(|d| d.balance > 0 && d.months_left.is_none());

        // Health score: five transparent parts, 100 total.
        let mut parts = vec![];
        let sr = avg_savings_rate;
        parts.push(HealthPart {
            name: "Tabungan",
            max: 30.0,
            score: (sr / 0.2).clamp(0.0, 1.0) * 30.0,
            detail: format!("Rata-rata menyisihkan {}% pemasukan, target 20%.", (sr * 100.0).round() as i64),
        });
        let adh: Vec<f64> = basis.iter().filter_map(|m| m.adherence).collect();
        let adh_avg = if adh.is_empty() { None } else { Some(adh.iter().sum::<f64>() / adh.len() as f64) };
        parts.push(HealthPart {
            name: "Disiplin anggaran",
            max: 20.0,
            score: adh_avg.map(|a| a * 20.0).unwrap_or(6.0),
            detail: match adh_avg {
                Some(a) => format!("{}% kategori tetap dalam rencana.", (a * 100.0).round() as i64),
                None => "Belum ada bulan lengkap dengan anggaran.".into(),
            },
        });
        parts.push(HealthPart {
            name: "Beban cicilan",
            max: 25.0,
            score: if trapped { 0.0 } else { (1.0 - ((debt_to_income - 0.1) / 0.3)).clamp(0.0, 1.0) * 25.0 },
            detail: if trapped {
                "Ada hutang yang cicilannya tidak menutup bunga.".into()
            } else if min_total == 0 {
                "Tidak ada cicilan aktif.".into()
            } else {
                format!("Cicilan minimum {}% dari pemasukan, aman di bawah 30%.", (debt_to_income * 100.0).round() as i64)
            },
        });
        parts.push(HealthPart {
            name: "Stabilitas belanja",
            max: 15.0,
            score: if basis.len() < 2 { 7.5 } else { (1.0 - (volatility - 0.1) / 0.4).clamp(0.0, 1.0) * 15.0 },
            detail: format!("Pengeluaran bulanan bergeser sekitar {}% dari rata-rata.", (volatility * 100.0).round() as i64),
        });
        let surplus_months = basis.iter().filter(|m| m.net >= 0).count();
        parts.push(HealthPart {
            name: "Konsistensi surplus",
            max: 10.0,
            score: if basis.is_empty() { 0.0 } else { surplus_months as f64 / basis.len() as f64 * 10.0 },
            detail: format!("{} dari {} bulan ditutup surplus.", surplus_months, basis.len()),
        });
        let health: f64 = parts.iter().map(|p| p.score).sum::<f64>().round();
        let health_label = match health as i64 {
            80.. => "Sangat sehat",
            65..=79 => "Sehat",
            50..=64 => "Cukup",
            35..=49 => "Perlu perhatian",
            _ => "Rawan",
        };

        // Category trends: top 6 by total.
        let mut categories: Vec<CatTrend> = cat_vals
            .into_iter()
            .map(|(id, values)| {
                let c = cats.iter().find(|c| c.id == id);
                let total = values.iter().sum();
                let idx_last_full = (span - 2).max(0) as usize;
                let earlier: Vec<i64> = values[..idx_last_full].iter().copied().filter(|v| *v > 0).collect();
                let change = (!earlier.is_empty() && values[idx_last_full] > 0).then(|| {
                    let avg = earlier.iter().sum::<i64>() as f64 / earlier.len() as f64;
                    (values[idx_last_full] as f64 - avg) / avg
                });
                CatTrend {
                    name: c.map(|c| c.name.clone()).unwrap_or_else(|| "Tanpa kategori".into()),
                    color: c.map(|c| c.color.clone()).unwrap_or_else(|| "#64748B".into()),
                    values,
                    total,
                    change,
                }
            })
            .collect();
        categories.sort_by(|a, b| b.total.cmp(&a.total));
        categories.truncate(6);

        // Spending mix.
        let mut all_cats: Vec<(i64, i64)> = vec![];
        for t in txs.iter().filter(|t| t.kind == "expense") {
            let c = t.category_id.unwrap_or(0);
            match all_cats.iter_mut().find(|x| x.0 == c) {
                Some(x) => x.1 += t.amount,
                None => all_cats.push((c, t.amount)),
            }
        }
        all_cats.sort_by(|a, b| b.1.cmp(&a.1));
        let spend_total: i64 = all_cats.iter().map(|x| x.1).sum();
        let mut composition: Vec<(String, String, i64, f64)> = all_cats
            .iter()
            .take(5)
            .map(|(id, v)| {
                let c = cats.iter().find(|c| c.id == *id);
                (
                    c.map(|c| c.name.clone()).unwrap_or_else(|| "Tanpa kategori".into()),
                    c.map(|c| c.color.clone()).unwrap_or_else(|| "#64748B".into()),
                    *v,
                    if spend_total > 0 { *v as f64 / spend_total as f64 } else { 0.0 },
                )
            })
            .collect();
        let rest: i64 = all_cats.iter().skip(5).map(|x| x.1).sum();
        if rest > 0 {
            composition.push(("Lainnya".into(), "#94A3B8".into(), rest, rest as f64 / spend_total.max(1) as f64));
        }

        // Weekday habit over the period (variable spending only).
        let mut wd_sum = [0i64; 7];
        let mut wd_n = [0i64; 7];
        let first_day = month_start(sy, sm);
        let mut d = first_day;
        while d <= today {
            wd_n[d.weekday().num_days_from_monday() as usize] += 1;
            d = d.succ_opt().unwrap();
        }
        let first_data = txs.iter().map(|t| t.date.as_str()).min().unwrap_or("9999").to_string();
        if let Ok(fd) = parse_date(&first_data) {
            // only count weekdays since data exists
            wd_n = [0; 7];
            let mut d = fd.max(first_day);
            while d <= today {
                wd_n[d.weekday().num_days_from_monday() as usize] += 1;
                d = d.succ_opt().unwrap();
            }
        }
        for t in txs.iter().filter(|t| t.kind == "expense" && t.recurring_id.is_none() && t.debt_id.is_none()) {
            if let Ok(d) = parse_date(&t.date) {
                wd_sum[d.weekday().num_days_from_monday() as usize] += t.amount;
            }
        }
        let mut weekday_avg = [0i64; 7];
        for i in 0..7 {
            weekday_avg[i] = if wd_n[i] > 0 { wd_sum[i] / wd_n[i] } else { 0 };
        }

        // Total money at each month end.
        let mut pool_months = vec![];
        for k in 0..span {
            let (y, m) = add_months(sy, sm, k);
            let v: i64 = if (y, m) == (cy, cm) {
                self.pool_total()?
            } else {
                let (ny, nm) = add_months(y, m, 1);
                let accs = self.accounts()?;
                let b = self.balances_before(&month_start(ny, nm).to_string())?;
                accs.iter().filter(|a| !a.archived).map(|a| b.get(&a.id).copied().unwrap_or(0)).sum()
            };
            pool_months.push(v);
        }

        // Top spending descriptions.
        let mut by_note: HashMap<String, (String, i64, usize)> = HashMap::new();
        for t in txs.iter().filter(|t| t.kind == "expense" && !t.note.is_empty()) {
            let key = t.note.to_lowercase();
            let e = by_note.entry(key).or_insert((t.note.clone(), 0, 0));
            e.1 += t.amount;
            e.2 += 1;
        }
        let mut top_spend: Vec<_> = by_note.into_values().collect();
        top_spend.sort_by(|a, b| b.1.cmp(&a.1));
        top_spend.truncate(6);

        let summary = if active.is_empty() {
            "Belum ada data. Catat transaksi untuk melihat performa.".to_string()
        } else {
            let mut s = format!(
                "Rata-rata pemasukan {} dan pengeluaran {} per bulan.",
                rupiah(avg_income),
                rupiah(avg_expense)
            );
            if let Some(c) = categories.iter().filter(|c| c.change.is_some()).max_by(|a, b| {
                a.change.unwrap().abs().partial_cmp(&b.change.unwrap().abs()).unwrap_or(std::cmp::Ordering::Equal)
            }) {
                let ch = c.change.unwrap();
                if ch.abs() >= 0.15 {
                    s.push_str(&format!(
                        " {} bulan lalu {} {}% dari biasanya.",
                        c.name,
                        if ch > 0.0 { "naik" } else { "turun" },
                        (ch.abs() * 100.0).round() as i64
                    ));
                }
            }
            if debt_balance > 0 {
                s.push_str(&format!(" Sisa hutang {}.", rupiah(debt_balance)));
            }
            s
        };

        Ok(Analytics {
            months,
            income_total,
            expense_total,
            net_total: income_total - expense_total,
            avg_savings_rate,
            avg_expense,
            avg_income,
            best,
            worst,
            volatility,
            debt_to_income,
            health,
            health_label,
            health_parts: parts,
            categories,
            top_spend,
            summary,
            composition,
            weekday_avg,
            pool_months,
        })
    }
}

#[cfg(test)]
mod tests {
    use crate::store::Store;
    use chrono::NaiveDate;

    #[test]
    fn analytics_on_demo() {
        let mut s = Store::open_memory().unwrap();
        s.set_today(NaiveDate::from_ymd_opt(2026, 10, 12).unwrap());
        s.seed_demo().unwrap();
        let a = s.analytics(12).unwrap();
        assert_eq!(a.months.len(), 12);
        assert!(a.avg_income > 0 && a.avg_expense > 0);
        assert!((0.0..=100.0).contains(&a.health));
        assert_eq!(a.health_parts.iter().map(|p| p.max).sum::<f64>(), 100.0);
        assert!(!a.categories.is_empty());
        let empty = Store::open_memory().unwrap().analytics(6).unwrap();
        assert_eq!(empty.income_total, 0);
    }
}
