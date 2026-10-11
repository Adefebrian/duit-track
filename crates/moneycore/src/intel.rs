//! Rule-based financial reasoning: pace, projection, anomalies, patterns.
//! Every number here is explainable; nothing is a black box.

use crate::model::*;
use crate::store::Store;
use crate::util::*;
use chrono::{Datelike, NaiveDate};
use std::collections::HashMap;

#[derive(Debug, Clone)]
pub struct CatStat {
    pub id: i64,
    pub name: String,
    pub color: String,
    pub icon: String,
    pub planned: i64,
    pub spent: i64,
    pub projected: i64,
    /// none | ok | watch | risk | over
    pub status: &'static str,
    pub share: f64,
}

#[derive(Debug, Clone)]
pub struct DueRecurring {
    pub rec: Recurring,
    pub date: NaiveDate,
    pub overdue: bool,
}

#[derive(Debug, Clone)]
pub struct Dashboard {
    pub year: i32,
    pub month: u32,
    pub is_current: bool,
    pub is_future: bool,
    pub day: u32,
    pub days_in_month: u32,
    pub days_left: u32,
    pub income: i64,
    pub expense: i64,
    pub savings_rate: f64,
    pub budget_total: i64,
    pub safe_today: i64,
    pub safe_basis: &'static str,
    pub projected_expense: i64,
    pub categories: Vec<CatStat>,
    pub daily: Vec<i64>,
    pub cumulative: Vec<i64>,
    /// average cumulative spend by day over the previous 3 months
    pub typical_cumulative: Vec<i64>,
    pub history: Vec<(String, i64, i64)>,
    pub weekday_avg: [i64; 7],
    pub recent: Vec<Transaction>,
    pub insights: Vec<Insight>,
    pub due_recurring: Vec<DueRecurring>,
    pub debt_balance: i64,
    pub debt_paid: i64,
    pub debt_progress: f64,
    pub next_debt: Option<(String, i64, i64)>,
    pub no_spend_streak: u32,
    pub pool: i64,
}

/// Transactions that land once a month on a known day; they should not be
/// extrapolated like daily spending.
fn is_fixed(t: &Transaction) -> bool {
    t.recurring_id.is_some() || t.debt_id.is_some()
}

fn cumulative_by_day(txs: &[Transaction], dim: u32) -> Vec<i64> {
    let mut daily = vec![0i64; dim as usize];
    for t in txs.iter().filter(|t| t.kind == "expense") {
        if let Ok(d) = parse_date(&t.date) {
            daily[(d.day() - 1).min(dim - 1) as usize] += t.amount;
        }
    }
    let mut acc = 0;
    daily.iter().map(|v| {
        acc += v;
        acc
    }).collect()
}

fn norm_note(s: &str) -> String {
    s.to_lowercase().chars().filter(|c| c.is_alphabetic() || *c == ' ').collect::<String>().split_whitespace().collect::<Vec<_>>().join(" ")
}

fn pct(x: f64) -> String {
    format!("{}%", (x * 100.0).round() as i64)
}

impl Store {
    pub fn dashboard(&self, y: i32, m: u32) -> R<Dashboard> {
        let today = self.today();
        let dim = days_in_month(y, m);
        let first = month_start(y, m);
        let cur_key = (today.year(), today.month());
        let is_current = (y, m) == cur_key;
        let is_future = (y, m) > cur_key;
        let day = if is_current { today.day() } else if is_future { 0 } else { dim };
        let days_left = if is_current { dim - today.day() + 1 } else if is_future { dim } else { 0 };
        let elapsed_frac = day as f64 / dim as f64;

        let cats = self.categories()?;
        let cat_by_id: HashMap<i64, &Category> = cats.iter().map(|c| (c.id, c)).collect();
        let txs = self.transactions_month(y, m)?;
        let budgets: HashMap<i64, i64> =
            crate::db::budgets_for(&self.conn, &month_key(y, m))?.into_iter().collect();
        let budget_total: i64 = budgets.values().sum();

        let income: i64 = txs.iter().filter(|t| t.kind == "income").map(|t| t.amount).sum();
        let expense: i64 = txs.iter().filter(|t| t.kind == "expense").map(|t| t.amount).sum();

        // History: 3 previous months for rates, 6 for anomalies/charts.
        let hist_start = {
            let (hy, hm) = add_months(y, m, -6);
            month_start(hy, hm)
        };
        let hist = self.transactions_between(hist_start, first)?;
        let (p3y, p3m) = add_months(y, m, -3);
        let p3_start = month_start(p3y, p3m).to_string();
        let hist3: Vec<&Transaction> = hist.iter().filter(|t| t.date >= p3_start).collect();
        let hist_months_present: usize = {
            let mut ms: Vec<&str> = hist3.iter().map(|t| &t.date[..7]).collect();
            ms.sort();
            ms.dedup();
            ms.len()
        };

        // Recurring still to come this month, per category.
        let recurring = self.recurring()?;
        let posted = self.posted_recurring(y, m)?;
        let mut due_recurring = vec![];
        let mut pending_by_cat: HashMap<i64, i64> = HashMap::new();
        let mut pending_expense = 0;
        if !is_future || (y, m) == add_months(cur_key.0, cur_key.1, 1) {
            for r in recurring.iter().filter(|r| r.active && !posted.contains(&r.id)) {
                let date = clamp_day(y, m, r.day);
                if r.kind == "expense" {
                    pending_expense += r.amount;
                    if let Some(c) = r.category_id {
                        *pending_by_cat.entry(c).or_default() += r.amount;
                    }
                }
                if is_current && date <= plus_days(today, 3) {
                    due_recurring.push(DueRecurring { overdue: date < today, rec: r.clone(), date });
                }
            }
        }
        due_recurring.sort_by_key(|d| d.date);

        // Category stats with projection.
        let mut spent_by: HashMap<i64, i64> = HashMap::new();
        let mut variable_by: HashMap<i64, i64> = HashMap::new();
        for t in txs.iter().filter(|t| t.kind == "expense") {
            let c = t.category_id.unwrap_or(0);
            *spent_by.entry(c).or_default() += t.amount;
            if !is_fixed(t) {
                *variable_by.entry(c).or_default() += t.amount;
            }
        }
        let mut hist_variable: HashMap<i64, i64> = HashMap::new();
        let mut hist_count_by: HashMap<i64, usize> = HashMap::new();
        for t in hist3.iter().filter(|t| t.kind == "expense" && !is_fixed(t)) {
            *hist_variable.entry(t.category_id.unwrap_or(0)).or_default() += t.amount;
            *hist_count_by.entry(t.category_id.unwrap_or(0)).or_default() += 1;
        }

        let w = elapsed_frac.clamp(0.2, 0.85);
        let mut categories = vec![];
        let mut projected_expense = 0i64;
        let mut ids: Vec<i64> = spent_by.keys().chain(budgets.keys()).copied().collect();
        ids.sort();
        ids.dedup();
        for id in ids {
            let spent = spent_by.get(&id).copied().unwrap_or(0);
            let planned = budgets.get(&id).copied().unwrap_or(0);
            let pending = pending_by_cat.get(&id).copied().unwrap_or(0);
            let projected = if is_current {
                let cur_rate = variable_by.get(&id).copied().unwrap_or(0) as f64 / day.max(1) as f64;
                let hist_rate = if hist_months_present > 0 {
                    hist_variable.get(&id).copied().unwrap_or(0) as f64 / (hist_months_present as f64 * 30.4)
                } else {
                    cur_rate
                };
                let hist_count = hist_count_by.get(&id).copied().unwrap_or(0) as f64 / hist_months_present.max(1) as f64;
                if hist_months_present > 0 && hist_count <= 3.0 {
                    // Lumpy category (a few bills a month): expect the usual total, not a daily rate.
                    let usual = (hist_rate * 30.4).round() as i64;
                    let variable = variable_by.get(&id).copied().unwrap_or(0);
                    (spent - variable) + variable.max(usual) + pending
                } else {
                    let rate = w * cur_rate + (1.0 - w) * hist_rate;
                    spent + (rate * (dim - day) as f64).round() as i64 + pending
                }
            } else if is_future {
                pending
            } else {
                spent
            };
            projected_expense += projected;
            let expected_now = planned as f64 * elapsed_frac;
            let status = if planned == 0 {
                "none"
            } else if spent > planned {
                "over"
            } else if is_current && projected as f64 > planned as f64 * 1.02 {
                "risk"
            } else if is_current && projected > spent && projected as f64 > planned as f64 * 0.95 && spent as f64 > expected_now * 1.1 {
                "watch"
            } else {
                "ok"
            };
            let (name, color, icon) = match cat_by_id.get(&id) {
                Some(c) => (c.name.clone(), c.color.clone(), c.icon.clone()),
                None => ("Tanpa kategori".into(), "#64748B".into(), "dots".into()),
            };
            categories.push(CatStat {
                id,
                name,
                color,
                icon,
                planned,
                spent,
                projected,
                status,
                share: if expense > 0 { spent as f64 / expense as f64 } else { 0.0 },
            });
        }
        categories.sort_by(|a, b| b.spent.cmp(&a.spent).then(b.planned.cmp(&a.planned)));

        // Daily series.
        let mut daily = vec![0i64; dim as usize];
        for t in txs.iter().filter(|t| t.kind == "expense") {
            if let Ok(d) = parse_date(&t.date) {
                daily[(d.day() - 1) as usize] += t.amount;
            }
        }
        let cumulative = cumulative_by_day(&txs, dim);
        let mut typical_cumulative = vec![0i64; dim as usize];
        if hist_months_present > 0 {
            for k in 1..=3 {
                let (hy, hm) = add_months(y, m, -k);
                let key = month_key(hy, hm);
                let mtx: Vec<Transaction> = hist3.iter().filter(|t| t.date.starts_with(&key)).map(|t| (*t).clone()).collect();
                if mtx.is_empty() {
                    continue;
                }
                let hdim = days_in_month(hy, hm);
                let c = cumulative_by_day(&mtx, hdim);
                for d in 0..dim as usize {
                    typical_cumulative[d] += c[d.min(hdim as usize - 1)];
                }
            }
            for v in typical_cumulative.iter_mut() {
                *v /= hist_months_present as i64;
            }
        }

        // Six-month history (oldest first, current last).
        let mut history = vec![];
        for k in (0..6).rev() {
            let (hy, hm) = add_months(y, m, -k);
            let key = month_key(hy, hm);
            let src: Vec<&Transaction> = if k == 0 { txs.iter().collect() } else { hist.iter().filter(|t| t.date.starts_with(&key)).collect() };
            let inc = src.iter().filter(|t| t.kind == "income").map(|t| t.amount).sum();
            let exp = src.iter().filter(|t| t.kind == "expense").map(|t| t.amount).sum();
            history.push((MONTHS_ID[(hm - 1) as usize][..3].to_string(), inc, exp));
        }

        // Weekday pattern over the last 8 weeks of variable spending.
        let pattern_end = if is_current { today } else { plus_days(month_start(add_months(y, m, 1).0, add_months(y, m, 1).1), -1) };
        let pattern_start = plus_days(pattern_end, -55);
        let pattern_tx = self.transactions_between(pattern_start, plus_days(pattern_end, 1))?;
        let mut wd_sum = [0i64; 7];
        for t in pattern_tx.iter().filter(|t| t.kind == "expense" && !is_fixed(t)) {
            if let Ok(d) = parse_date(&t.date) {
                wd_sum[d.weekday().num_days_from_monday() as usize] += t.amount;
            }
        }
        let weekday_avg = wd_sum.map(|s| s / 8);

        // Safe-to-spend.
        let (safe_today, safe_basis) = if !is_current {
            (0, "none")
        } else if budget_total > 0 {
            let left: i64 = budgets
                .iter()
                .map(|(c, p)| (p - spent_by.get(c).copied().unwrap_or(0) - pending_by_cat.get(c).copied().unwrap_or(0)).max(0))
                .sum();
            (left / days_left.max(1) as i64, "budget")
        } else if income > 0 {
            ((income - expense - pending_expense).max(0) / days_left.max(1) as i64, "income")
        } else {
            (0, "none")
        };

        // Real money caps what is "safe": a budget is a plan, the pool is the fact.
        let pool = self.pool_total()?;
        let (safe_today, safe_basis) = if is_current && safe_basis != "none" && pool / (days_left.max(1) as i64) < safe_today {
            ((pool.max(0)) / days_left.max(1) as i64, "pool")
        } else {
            (safe_today, safe_basis)
        };

        // No-spend streak (variable spending only).
        let mut no_spend_streak = 0;
        if is_current {
            let recent_tx = self.transactions_between(plus_days(today, -60), plus_days(today, 1))?;
            let mut d = today;
            loop {
                let ds = d.to_string();
                if recent_tx.iter().any(|t| t.kind == "expense" && !is_fixed(t) && t.date == ds) || no_spend_streak > 60 {
                    break;
                }
                no_spend_streak += 1;
                d = d.pred_opt().unwrap();
            }
        }

        // Debts.
        let debts = self.debt_views()?;
        let active: Vec<_> = debts.iter().filter(|d| !d.debt.closed).collect();
        let debt_balance: i64 = active.iter().map(|d| d.balance).sum();
        let debt_paid: i64 = active.iter().map(|d| d.paid).sum();
        let debt_total: i64 = active.iter().map(|d| d.debt.start_balance + d.charges).sum();
        let next_debt = active
            .iter()
            .filter(|d| d.balance > 0)
            .min_by_key(|d| d.days_to_due)
            .map(|d| (d.debt.name.clone(), d.days_to_due, d.debt.min_payment.min(d.balance)));

        let mut dash = Dashboard {
            year: y,
            month: m,
            is_current,
            is_future,
            day,
            days_in_month: dim,
            days_left,
            income,
            expense,
            savings_rate: if income > 0 { (income - expense) as f64 / income as f64 } else { 0.0 },
            budget_total,
            safe_today,
            safe_basis,
            projected_expense: projected_expense.max(expense),
            categories,
            daily,
            cumulative,
            typical_cumulative,
            history,
            weekday_avg,
            recent: txs.iter().take(6).cloned().collect(),
            insights: vec![],
            due_recurring,
            debt_balance,
            debt_paid,
            debt_progress: if debt_total > 0 { debt_paid as f64 / debt_total as f64 } else { 0.0 },
            next_debt,
            no_spend_streak,
            pool,
        };
        dash.insights = self.insights(&dash, &txs, &hist, &recurring, &cats, &debts)?;
        Ok(dash)
    }

    fn insights(
        &self,
        d: &Dashboard,
        txs: &[Transaction],
        hist: &[Transaction],
        recurring: &[Recurring],
        cats: &[Category],
        debts: &[crate::store::DebtView],
    ) -> R<Vec<Insight>> {
        let mut out: Vec<Insight> = vec![];
        let mut push = |id: String, tone: &'static str, priority: i32, title: String, body: String| {
            out.push(Insight { id, tone, title, body, priority });
        };

        // Budget breaches and projected breaches.
        for c in &d.categories {
            match c.status {
                "over" => push(
                    format!("over-{}", c.id), "bad", 100,
                    format!("{} lewat anggaran", c.name),
                    format!("Terpakai {} dari {}, lebih {} ({}).", rupiah(c.spent), rupiah(c.planned), rupiah(c.spent - c.planned), pct((c.spent - c.planned) as f64 / c.planned as f64)),
                ),
                "risk" => {
                    let per_day = (c.planned - c.spent).max(0) / d.days_left.max(1) as i64;
                    push(
                        format!("risk-{}", c.id), "warn", 90,
                        format!("{} diprediksi tembus anggaran", c.name),
                        format!("Dengan ritme sekarang akhir bulan sekitar {}, lebih {}. Jaga di bawah {} per hari agar aman.", rupiah(c.projected), rupiah(c.projected - c.planned), rupiah(per_day)),
                    )
                }
                _ => {}
            }
        }

        // Pace versus your own typical month at the same day.
        if d.is_current && d.day >= 5 {
            let now = d.cumulative[(d.day - 1) as usize];
            let typ = d.typical_cumulative[(d.day - 1) as usize];
            if typ > 0 {
                let diff = (now - typ) as f64 / typ as f64;
                if diff > 0.15 {
                    push("pace".into(), "warn", 80, format!("Pengeluaran {} lebih tinggi dari biasanya", pct(diff)),
                        format!("Sampai tanggal {} sudah {}, biasanya {}.", d.day, rupiah(now), rupiah(typ)));
                } else if diff < -0.1 {
                    push("pace".into(), "good", 60, format!("Lebih hemat {} dari biasanya", pct(-diff)),
                        format!("Sampai tanggal {} baru {}, biasanya {}. Pertahankan.", d.day, rupiah(now), rupiah(typ)));
                }
            }
        }

        // Unusual transactions: robust outlier against that category's history.
        let mut by_cat: HashMap<i64, Vec<f64>> = HashMap::new();
        for t in hist.iter().filter(|t| t.kind == "expense" && !is_fixed(t)) {
            by_cat.entry(t.category_id.unwrap_or(0)).or_default().push(t.amount as f64);
        }
        let mut anomalies: Vec<(&Transaction, f64)> = vec![];
        for t in txs.iter().filter(|t| t.kind == "expense" && !is_fixed(t)) {
            if let Some(h) = by_cat.get(&t.category_id.unwrap_or(0)) {
                if h.len() >= 6 {
                    let (mean, sd) = mean_sd(h);
                    let mut hc = h.clone();
                    let med = median(&mut hc);
                    let a = t.amount as f64;
                    if a > (3.0 * med).max(mean + 2.5 * sd) && a >= 50_000.0 {
                        anomalies.push((t, a / med.max(1.0)));
                    }
                }
            }
        }
        anomalies.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        for (t, ratio) in anomalies.into_iter().take(2) {
            let cname = cats.iter().find(|c| Some(c.id) == t.category_id).map(|c| c.name.as_str()).unwrap_or("kategorinya");
            push(format!("odd-{}", t.id), "info", 70,
                format!("Transaksi tidak biasa: {}", if t.note.is_empty() { cname } else { &t.note }),
                format!("{} pada {}, sekitar {}x transaksi {} biasanya.", rupiah(t.amount), date_label(parse_date(&t.date)?), ratio.round() as i64, cname.to_lowercase()));
        }

        // Income vs spending.
        if d.income > 0 && !d.is_future {
            if d.expense > d.income {
                push("deficit".into(), "bad", 95, "Pengeluaran melebihi pemasukan".into(),
                    format!("Selisih {} bulan ini. Cek kategori terbesar untuk dipangkas.", rupiah(d.expense - d.income)));
            } else if d.savings_rate >= 0.2 && (!d.is_current || d.day > 20) {
                push("save".into(), "good", 50, format!("Menabung {} dari pemasukan", pct(d.savings_rate)),
                    format!("Sisa {} bulan ini. Target sehat adalah 20% ke atas.", rupiah(d.income - d.expense)));
            }
            if d.budget_total > d.income {
                push("overplan".into(), "warn", 75, "Anggaran melebihi pemasukan".into(),
                    format!("Rencana {} sedangkan pemasukan {}.", rupiah(d.budget_total), rupiah(d.income)));
            } else if d.budget_total > 0 && d.income - d.budget_total >= 100_000 {
                push("unalloc".into(), "info", 40, format!("{} belum dialokasikan", rupiah(d.income - d.budget_total)),
                    "Arahkan ke tabungan atau pelunasan hutang supaya tidak habis tanpa rencana.".into());
            }
        }
        if d.budget_total == 0 && !d.is_future {
            push("nobudget".into(), "info", 65, "Belum ada anggaran bulan ini".into(),
                "Buka Anggaran dan pakai saran otomatis dari riwayat belanjamu.".into());
        }

        // Weekend habit.
        let wk: f64 = d.weekday_avg[..5].iter().sum::<i64>() as f64 / 5.0;
        let we: f64 = d.weekday_avg[5..].iter().sum::<i64>() as f64 / 2.0;
        if wk > 0.0 && we / wk >= 1.5 {
            push("weekend".into(), "info", 45, format!("Akhir pekan {:.1}x lebih boros", we / wk).replace('.', ","),
                format!("Rata-rata {} per hari Sabtu dan Minggu, {} di hari kerja.", rupiah(we as i64), rupiah(wk as i64)));
        }

        // Concentration.
        if let Some(top) = d.categories.first() {
            if top.share > 0.4 && d.expense > 0 {
                push("top".into(), "info", 35, format!("{} menyerap {} pengeluaran", top.name, pct(top.share)),
                    "Kategori terbesar adalah tempat paling efektif untuk berhemat.".into());
            }
        }

        // Debts.
        for v in debts.iter().filter(|v| v.balance > 0 && !v.debt.closed) {
            if v.months_left.is_none() {
                push(format!("trap-{}", v.debt.id), "bad", 98, format!("Cicilan {} tidak menutup bunga", v.debt.name),
                    format!("Bunga sekitar {} per bulan, minimum {}. Saldo akan terus naik.", rupiah(v.monthly_interest), rupiah(v.debt.min_payment)));
            } else if d.is_current && v.days_to_due <= 5 && v.paid_this_month < v.debt.min_payment {
                let when = match v.days_to_due { 0 => "hari ini".to_string(), 1 => "besok".to_string(), n => format!("{n} hari lagi") };
                push(format!("due-{}", v.debt.id), "warn", 92, format!("{} jatuh tempo {}", v.debt.name, when),
                    format!("Minimal {} pada {}.", rupiah(v.debt.min_payment.min(v.balance)), date_label(v.next_due)));
            }
        }

        // Sources below zero.
        let bals = self.balances_now()?;
        for a in self.accounts()?.iter().filter(|a| !a.archived) {
            let b = bals.get(&a.id).copied().unwrap_or(0);
            if b < 0 {
                push(format!("neg-{}", a.id), "bad", 97, format!("Saldo {} minus", a.name),
                    format!("{}. Cek transaksi yang salah sumber atau catat top up.", rupiah(b)));
            }
        }

        // Recurring income that has not shown up.
        for r in d.due_recurring.iter().filter(|r| r.overdue && r.rec.kind == "income") {
            push(format!("late-{}", r.rec.id), "info", 55, format!("{} belum dicatat", r.rec.name),
                format!("Biasanya masuk tanggal {}. Catat jika sudah diterima.", r.rec.day));
        }

        // Spot subscriptions you have not turned into a recurring rule.
        let mut groups: HashMap<String, Vec<(String, i64)>> = HashMap::new();
        for t in hist.iter().chain(txs.iter()).filter(|t| t.kind == "expense" && t.recurring_id.is_none() && t.debt_id.is_none()) {
            let n = norm_note(&t.note);
            if n.len() >= 3 {
                groups.entry(n).or_default().push((t.date[..7].to_string(), t.amount));
            }
        }
        let known: Vec<String> = recurring.iter().map(|r| norm_note(&r.name)).collect();
        let mut found = 0;
        for (n, v) in groups.iter() {
            let mut months: Vec<&str> = v.iter().map(|x| x.0.as_str()).collect();
            months.sort();
            months.dedup();
            if months.len() < 3 || months.len() * 2 < v.len() || known.contains(n) {
                continue;
            }
            let amts: Vec<f64> = v.iter().map(|x| x.1 as f64).collect();
            let (mean, sd) = mean_sd(&amts);
            if mean > 0.0 && sd / mean < 0.12 && found < 1 {
                found += 1;
                let label = v.iter().map(|_| n.clone()).next().unwrap_or_default();
                push(format!("pattern-{n}"), "info", 30, format!("Pola rutin: {}", capitalize(&label)),
                    format!("Muncul tiap bulan sekitar {}. Jadikan transaksi rutin agar prediksi lebih akurat.", rupiah(mean as i64)));
            }
        }

        if d.no_spend_streak >= 2 {
            push("streak".into(), "good", 20, format!("{} hari tanpa jajan", d.no_spend_streak),
                "Tidak ada pengeluaran harian tercatat. Bagus, atau ada yang lupa dicatat?".into());
        }

        // Many projected breaches read as noise: fold them into one line.
        let risks: Vec<Insight> = out.iter().filter(|i| i.id.starts_with("risk-")).cloned().collect();
        if risks.len() > 2 {
            out.retain(|i| !i.id.starts_with("risk-"));
            let names: Vec<String> = risks.iter().map(|i| i.title.trim_end_matches(" diprediksi tembus anggaran").to_string()).collect();
            out.push(Insight {
                id: "risk-many".into(),
                tone: "warn",
                priority: 90,
                title: format!("{} kategori diprediksi tembus anggaran", risks.len()),
                body: format!("{}. Ritme belanja harian perlu diturunkan.", names.join(", ")),
            });
        }
        out.sort_by(|a, b| b.priority.cmp(&a.priority));
        out.truncate(6);
        Ok(out)
    }
}

fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use crate::store::Store;
    use chrono::NaiveDate;

    #[test]
    fn demo_dashboard_is_coherent() {
        let mut s = Store::open_memory().unwrap();
        s.set_today(NaiveDate::from_ymd_opt(2026, 10, 12).unwrap());
        s.seed_demo().unwrap();
        let d = s.dashboard(2026, 10).unwrap();
        assert!(d.is_current);
        assert!(d.expense > 0);
        assert!(s.dashboard(2026, 9).unwrap().income > 0);
        assert!(d.projected_expense >= d.expense);
        assert_eq!(d.cumulative.len(), 31);
        assert_eq!(*d.cumulative.last().unwrap(), d.expense);
        assert!(!d.insights.is_empty());
        // The planted 1.25jt shoe purchase must be flagged.
        assert!(d.insights.iter().any(|i| i.id.starts_with("odd-")), "{:#?}", d.insights);
        // Fixed-term plans: 6x to family started July, 3 paid; PayLater 3x, 2 paid.
        let views = s.debt_views().unwrap();
        let kakak = views.iter().find(|v| v.debt.name == "Cicilan laptop").unwrap();
        assert_eq!((kakak.schedule.len(), kakak.installments_paid), (6, 3));
        assert_eq!(kakak.months_left, Some(3));
        let pl = views.iter().find(|v| v.debt.name == "PayLater HP").unwrap();
        assert_eq!(pl.installments_paid, 2);
        let load = s.debt_load(6).unwrap();
        let has = |k: usize, id: i64| load[k].1.iter().any(|(d, _)| *d == id);
        assert!(has(0, kakak.debt.id) && has(2, kakak.debt.id) && !has(3, kakak.debt.id));
        assert!(has(1, pl.debt.id) && !has(2, pl.debt.id));
        assert!(s.pool_total().unwrap() > 0, "demo money should stay positive");
        let plan = s.debt_plan(500_000).unwrap();
        assert_eq!(plan.options.len(), 3);
        assert!(plan.options[1].months <= plan.options[0].months);
        let rows = s.budget_rows(2026, 10).unwrap();
        assert!(rows.iter().any(|r| r.suggested > 0));
        // backup round-trip
        let json = s.export_json().unwrap();
        let mut s2 = Store::open_memory().unwrap();
        s2.import_json(&json).unwrap();
        assert_eq!(s2.export_json().unwrap(), json);
    }
}
