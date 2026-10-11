use crate::db::{self, ACC_COLS, CAT_COLS, DEBT_COLS, PAY_COLS, REC_COLS, TRF_COLS, TX_COLS};
use crate::debt::{simulate, SimDebt, Strategy};
use crate::model::*;
use crate::parse::{self, Parsed};
use crate::util::*;
use chrono::{Datelike, Local, NaiveDate};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

pub struct Store {
    pub(crate) conn: Connection,
    today_override: Option<NaiveDate>,
}

#[derive(Debug, Clone)]
pub struct BudgetRow {
    pub category: Category,
    pub planned: i64,
    pub spent: i64,
    pub prev_spent: i64,
    pub avg3: i64,
    pub suggested: i64,
}

#[derive(Debug, Clone)]
pub struct Installment {
    pub n: u32,
    pub date: NaiveDate,
    pub amount: i64,
    /// paid | late | due | upcoming
    pub status: &'static str,
}

#[derive(Debug, Clone)]
pub struct DebtView {
    pub debt: Debt,
    pub paid: i64,
    pub charges: i64,
    pub balance: i64,
    pub progress: f64,
    pub monthly_interest: i64,
    pub months_left: Option<u32>,
    pub payoff_label: String,
    pub next_due: NaiveDate,
    pub days_to_due: i64,
    pub paid_this_month: i64,
    pub payments: Vec<DebtPayment>,
    /// installment schedule for fixed-term debts, empty for flexible ones
    pub schedule: Vec<Installment>,
    pub installments_paid: u32,
}

/// One month of upcoming obligations: (month label, [(debt id, amount)])
pub type LoadMonth = (String, Vec<(i64, i64)>);

#[derive(Debug, Clone)]
pub struct PlanOption {
    pub key: &'static str,
    pub label: &'static str,
    pub months: u32,
    pub total_interest: i64,
    pub payoff_label: String,
    pub feasible: bool,
    pub timeline: Vec<i64>,
    pub order: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct DebtPlan {
    pub options: Vec<PlanOption>,
    pub recommended: usize,
    pub message: String,
}

#[derive(Serialize, Deserialize)]
struct Backup {
    app: String,
    version: u32,
    categories: Vec<Category>,
    transactions: Vec<Transaction>,
    budgets: Vec<(String, i64, i64)>,
    recurring: Vec<Recurring>,
    debts: Vec<Debt>,
    debt_payments: Vec<DebtPayment>,
    learn: Vec<(String, i64, i64)>,
    #[serde(default)]
    accounts: Vec<Account>,
    #[serde(default)]
    transfers: Vec<Transfer>,
    #[serde(default)]
    goals: Vec<crate::goals::Goal>,
    #[serde(default)]
    goal_entries: Vec<crate::goals::GoalEntry>,
}

fn check_kind(k: &str) -> R<()> {
    if k == "income" || k == "expense" {
        Ok(())
    } else {
        Err("jenis harus pemasukan atau pengeluaran".into())
    }
}

fn check_amount(a: i64) -> R<()> {
    if a > 0 && a <= 1_000_000_000_000 {
        Ok(())
    } else {
        Err("nominal harus lebih dari 0".into())
    }
}

fn check_text(s: &str, max: usize, what: &str, required: bool) -> R<String> {
    let t = s.trim().to_string();
    if required && t.is_empty() {
        return Err(format!("{what} wajib diisi"));
    }
    if t.chars().count() > max {
        return Err(format!("{what} maksimal {max} karakter"));
    }
    Ok(t)
}

impl Store {
    pub fn open(path: &str) -> R<Self> {
        Ok(Self { conn: db::open(path)?, today_override: env_today() })
    }

    pub fn open_memory() -> R<Self> {
        Ok(Self { conn: db::open_memory()?, today_override: env_today() })
    }

    pub fn set_today(&mut self, d: NaiveDate) {
        self.today_override = Some(d);
    }

    pub fn today(&self) -> NaiveDate {
        self.today_override.unwrap_or_else(|| Local::now().date_naive())
    }

    pub fn meta(&self, key: &str) -> R<Option<String>> {
        self.conn
            .query_row("SELECT value FROM meta WHERE key=?1", [key], |r| r.get(0))
            .optional()
            .s()
    }

    pub fn set_meta(&self, key: &str, value: &str) -> R<()> {
        self.conn
            .execute("INSERT INTO meta(key, value) VALUES (?1,?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value", params![key, value])
            .s()?;
        Ok(())
    }

    pub fn is_empty(&self) -> R<bool> {
        let n: i64 = self
            .conn
            .query_row("SELECT (SELECT COUNT(*) FROM transactions) + (SELECT COUNT(*) FROM debts)", [], |r| r.get(0))
            .s()?;
        Ok(n == 0)
    }

    // ---------- categories ----------

    pub fn categories(&self) -> R<Vec<Category>> {
        db::categories(&self.conn)
    }

    pub fn save_category(&self, c: &Category) -> R<i64> {
        check_kind(&c.kind)?;
        let name = check_text(&c.name, 40, "Nama kategori", true)?;
        let kw = check_text(&c.keywords.to_lowercase(), 2000, "Kata kunci", false)?;
        if c.id == 0 {
            let sort: i64 = self
                .conn
                .query_row("SELECT COALESCE(MAX(sort), 0) + 1 FROM categories", [], |r| r.get(0))
                .s()?;
            self.conn
                .execute(
                    "INSERT INTO categories(name, kind, icon, color, keywords, sort) VALUES (?1,?2,?3,?4,?5,?6)",
                    params![name, c.kind, c.icon, c.color, kw, sort],
                )
                .s()?;
            Ok(self.conn.last_insert_rowid())
        } else {
            self.conn
                .execute(
                    "UPDATE categories SET name=?1, icon=?2, color=?3, keywords=?4, archived=?5 WHERE id=?6",
                    params![name, c.icon, c.color, kw, c.archived as i64, c.id],
                )
                .s()?;
            Ok(c.id)
        }
    }

    // ---------- transactions ----------

    pub fn transactions_month(&self, y: i32, m: u32) -> R<Vec<Transaction>> {
        let (a, b) = month_bounds(y, m);
        db::transactions_between(&self.conn, &a, &b)
    }

    pub fn transactions_between(&self, a: NaiveDate, b: NaiveDate) -> R<Vec<Transaction>> {
        db::transactions_between(&self.conn, &a.to_string(), &b.to_string())
    }

    pub fn add_tx(&self, t: &Transaction) -> R<i64> {
        check_kind(&t.kind)?;
        check_amount(t.amount)?;
        parse_date(&t.date)?;
        let note = check_text(&t.note, 200, "Catatan", false)?;
        self.conn
            .execute(
                "INSERT INTO transactions(kind, amount, category_id, date, note, recurring_id, debt_id, created_at, account_id)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9)",
                params![t.kind, t.amount, t.category_id, t.date, note, t.recurring_id, t.debt_id, Local::now().to_rfc3339(), t.account_id],
            )
            .s()?;
        let id = self.conn.last_insert_rowid();
        if let Some(c) = t.category_id {
            self.learn(&note, c)?;
        }
        Ok(id)
    }

    pub fn update_tx(&self, t: &Transaction) -> R<()> {
        check_kind(&t.kind)?;
        check_amount(t.amount)?;
        parse_date(&t.date)?;
        let note = check_text(&t.note, 200, "Catatan", false)?;
        let old_cat: Option<Option<i64>> = self
            .conn
            .query_row("SELECT category_id FROM transactions WHERE id=?1", [t.id], |r| r.get(0))
            .optional()
            .s()?;
        let Some(old_cat) = old_cat else { return Err("transaksi tidak ditemukan".into()) };
        self.conn
            .execute(
                "UPDATE transactions SET kind=?1, amount=?2, category_id=?3, date=?4, note=?5, account_id=?6 WHERE id=?7",
                params![t.kind, t.amount, t.category_id, t.date, note, t.account_id, t.id],
            )
            .s()?;
        // A manual re-categorisation is a strong signal: learn it twice.
        if let Some(c) = t.category_id {
            if old_cat != Some(c) {
                self.learn(&note, c)?;
                self.learn(&note, c)?;
            }
        }
        Ok(())
    }

    pub fn delete_tx(&self, id: i64) -> R<Option<Transaction>> {
        let t = self
            .conn
            .query_row(&format!("SELECT {TX_COLS} FROM transactions WHERE id=?1"), [id], db::row_tx)
            .optional()
            .s()?;
        self.conn.execute("DELETE FROM debt_payments WHERE tx_id=?1", [id]).s()?;
        self.conn.execute("DELETE FROM transactions WHERE id=?1", [id]).s()?;
        Ok(t)
    }

    /// Re-insert a deleted transaction with its original id (undo).
    pub fn restore_tx(&self, t: &Transaction) -> R<()> {
        self.conn
            .execute(
                "INSERT OR REPLACE INTO transactions(id, kind, amount, category_id, date, note, recurring_id, debt_id, created_at, account_id)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",
                params![t.id, t.kind, t.amount, t.category_id, t.date, t.note, t.recurring_id, t.debt_id, t.created_at, t.account_id],
            )
            .s()?;
        Ok(())
    }

    fn learn(&self, note: &str, cat: i64) -> R<()> {
        for tok in parse::tokens_for_learning(note) {
            self.conn
                .execute(
                    "INSERT INTO learn(token, category_id, n) VALUES (?1, ?2, 1)
                     ON CONFLICT(token, category_id) DO UPDATE SET n = n + 1",
                    params![tok, cat],
                )
                .s()?;
        }
        Ok(())
    }

    fn learned(&self) -> R<HashMap<(String, i64), i64>> {
        let mut st = self.conn.prepare("SELECT token, category_id, n FROM learn").s()?;
        let rows = st
            .query_map([], |r| Ok(((r.get::<_, String>(0)?, r.get::<_, i64>(1)?), r.get::<_, i64>(2)?)))
            .s()?;
        rows.collect::<Result<HashMap<_, _>, _>>().s()
    }

    pub fn parse(&self, text: &str, kind_hint: Option<&str>) -> R<Parsed> {
        let cats = self.categories()?;
        let mut p = parse::parse(text, self.today(), &cats, &self.learned()?, kind_hint);
        let accs = self.accounts()?;
        let (acc, note) = parse::detect_account(&p.note, &accs);
        if acc.is_some() {
            p.note = note;
        }
        p.account_id = acc.or(self.default_account()?);
        Ok(p)
    }

    // ---------- budgets ----------

    pub fn category_spend(&self, y: i32, m: u32) -> R<HashMap<i64, i64>> {
        let (a, b) = month_bounds(y, m);
        let mut st = self
            .conn
            .prepare(
                "SELECT category_id, SUM(amount) FROM transactions
                 WHERE kind='expense' AND date >= ?1 AND date < ?2 AND category_id IS NOT NULL GROUP BY category_id",
            )
            .s()?;
        let rows = st.query_map(params![a, b], |r| Ok((r.get(0)?, r.get(1)?))).s()?;
        rows.collect::<Result<HashMap<_, _>, _>>().s()
    }

    pub fn budget_rows(&self, y: i32, m: u32) -> R<Vec<BudgetRow>> {
        let month = month_key(y, m);
        let planned: HashMap<i64, i64> = db::budgets_for(&self.conn, &month)?.into_iter().collect();
        let spent = self.category_spend(y, m)?;
        let hist: Vec<HashMap<i64, i64>> = (1..=3)
            .map(|k| {
                let (py, pm) = add_months(y, m, -k);
                self.category_spend(py, pm)
            })
            .collect::<R<_>>()?;
        let mut out = vec![];
        for c in self.categories()?.into_iter().filter(|c| c.kind == "expense" && !c.archived) {
            let prev = hist[0].get(&c.id).copied().unwrap_or(0);
            let mut h: Vec<f64> = hist.iter().filter_map(|mo| mo.get(&c.id)).map(|v| *v as f64).collect();
            let months_with = h.len();
            let avg3 = if h.is_empty() { 0 } else { (h.iter().sum::<f64>() / 3.0) as i64 };
            // Median of the months it was used, discounted when it is rarely used.
            let suggested = if months_with == 0 {
                0
            } else {
                round_budget(median(&mut h) * (months_with as f64 / 3.0).max(0.67))
            };
            out.push(BudgetRow {
                planned: planned.get(&c.id).copied().unwrap_or(0),
                spent: spent.get(&c.id).copied().unwrap_or(0),
                prev_spent: prev,
                avg3,
                suggested,
                category: c,
            });
        }
        Ok(out)
    }

    pub fn set_budget(&self, y: i32, m: u32, cat: i64, amount: i64) -> R<()> {
        if !(0..=1_000_000_000_000).contains(&amount) {
            return Err("nominal anggaran tidak valid".into());
        }
        let month = month_key(y, m);
        if amount == 0 {
            self.conn
                .execute("DELETE FROM budgets WHERE month=?1 AND category_id=?2", params![month, cat])
                .s()?;
        } else {
            self.conn
                .execute(
                    "INSERT INTO budgets(month, category_id, amount) VALUES (?1,?2,?3)
                     ON CONFLICT(month, category_id) DO UPDATE SET amount=excluded.amount",
                    params![month, cat, amount],
                )
                .s()?;
        }
        Ok(())
    }

    pub fn copy_budget_from_prev(&self, y: i32, m: u32) -> R<usize> {
        let (py, pm) = add_months(y, m, -1);
        let prev = db::budgets_for(&self.conn, &month_key(py, pm))?;
        for (c, a) in &prev {
            self.set_budget(y, m, *c, *a)?;
        }
        Ok(prev.len())
    }

    pub fn apply_suggestions(&self, y: i32, m: u32) -> R<usize> {
        let mut n = 0;
        for r in self.budget_rows(y, m)? {
            if r.suggested > 0 {
                self.set_budget(y, m, r.category.id, r.suggested)?;
                n += 1;
            }
        }
        Ok(n)
    }

    // ---------- recurring ----------

    pub fn recurring(&self) -> R<Vec<Recurring>> {
        db::recurring_all(&self.conn)
    }

    pub fn save_recurring(&self, r: &Recurring) -> R<i64> {
        check_kind(&r.kind)?;
        check_amount(r.amount)?;
        let name = check_text(&r.name, 60, "Nama", true)?;
        if !(1..=31).contains(&r.day) {
            return Err("tanggal harus 1 sampai 31".into());
        }
        if r.id == 0 {
            self.conn
                .execute(
                    "INSERT INTO recurring(kind, name, amount, category_id, day, active, account_id) VALUES (?1,?2,?3,?4,?5,?6,?7)",
                    params![r.kind, name, r.amount, r.category_id, r.day, r.active as i64, r.account_id],
                )
                .s()?;
            Ok(self.conn.last_insert_rowid())
        } else {
            self.conn
                .execute(
                    "UPDATE recurring SET kind=?1, name=?2, amount=?3, category_id=?4, day=?5, active=?6, account_id=?7 WHERE id=?8",
                    params![r.kind, name, r.amount, r.category_id, r.day, r.active as i64, r.account_id, r.id],
                )
                .s()?;
            Ok(r.id)
        }
    }

    pub fn delete_recurring(&self, id: i64) -> R<()> {
        self.conn.execute("DELETE FROM recurring WHERE id=?1", [id]).s()?;
        Ok(())
    }

    pub fn posted_recurring(&self, y: i32, m: u32) -> R<Vec<i64>> {
        let (a, b) = month_bounds(y, m);
        let mut st = self
            .conn
            .prepare("SELECT DISTINCT recurring_id FROM transactions WHERE recurring_id IS NOT NULL AND date >= ?1 AND date < ?2")
            .s()?;
        let rows = st.query_map(params![a, b], |r| r.get(0)).s()?;
        rows.collect::<Result<Vec<_>, _>>().s()
    }

    pub fn post_recurring(&self, id: i64, y: i32, m: u32) -> R<i64> {
        let r = self
            .conn
            .query_row(&format!("SELECT {REC_COLS} FROM recurring WHERE id=?1"), [id], db::row_recurring)
            .optional()
            .s()?
            .ok_or("transaksi rutin tidak ditemukan")?;
        if self.posted_recurring(y, m)?.contains(&id) {
            return Err("sudah dicatat bulan ini".into());
        }
        let date = clamp_day(y, m, r.day).min(self.today().max(month_start(y, m)));
        self.add_tx(&Transaction {
            id: 0,
            kind: r.kind,
            amount: r.amount,
            category_id: r.category_id,
            date: date.to_string(),
            note: r.name,
            recurring_id: Some(r.id),
            debt_id: None,
            created_at: String::new(),
            account_id: r.account_id.or(self.default_account()?),
        })
    }

    // ---------- debts ----------

    pub fn save_debt(&self, d: &Debt) -> R<i64> {
        let name = check_text(&d.name, 60, "Nama hutang", true)?;
        let lender = check_text(&d.lender, 60, "Pemberi pinjaman", false)?;
        let note = check_text(&d.note, 200, "Catatan", false)?;
        check_amount(d.start_balance)?;
        if d.min_payment < 0 || !(0.0..=1000.0).contains(&d.apr) || !(1..=31).contains(&d.due_day) {
            return Err("data hutang tidak valid".into());
        }
        parse_date(&d.start_date)?;
        if let Some(t) = d.tenor {
            if !(1..=600).contains(&t) || d.min_payment <= 0 {
                return Err("cicilan tetap butuh nominal per bulan dan tenor 1 sampai 600 bulan".into());
            }
        }
        if d.id == 0 {
            self.conn
                .execute(
                    "INSERT INTO debts(name, lender, start_balance, apr, min_payment, due_day, start_date, note, closed, tenor)
                     VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",
                    params![name, lender, d.start_balance, d.apr, d.min_payment, d.due_day, d.start_date, note, d.closed as i64, d.tenor],
                )
                .s()?;
            Ok(self.conn.last_insert_rowid())
        } else {
            self.conn
                .execute(
                    "UPDATE debts SET name=?1, lender=?2, start_balance=?3, apr=?4, min_payment=?5, due_day=?6,
                     start_date=?7, note=?8, closed=?9, tenor=?10 WHERE id=?11",
                    params![name, lender, d.start_balance, d.apr, d.min_payment, d.due_day, d.start_date, note, d.closed as i64, d.tenor, d.id],
                )
                .s()?;
            Ok(d.id)
        }
    }

    pub fn delete_debt(&self, id: i64) -> R<()> {
        self.conn.execute("DELETE FROM debts WHERE id=?1", [id]).s()?;
        Ok(())
    }

    /// Records a payment or charge. A payment can also be booked as an expense
    /// in "Cicilan Hutang" so it shows in monthly spending.
    pub fn add_debt_payment(&self, p: &DebtPayment, record_expense: bool, account_id: Option<i64>) -> R<i64> {
        if p.kind != "payment" && p.kind != "charge" {
            return Err("jenis tidak valid".into());
        }
        check_amount(p.amount)?;
        parse_date(&p.date)?;
        let note = check_text(&p.note, 200, "Catatan", false)?;
        let debt = self
            .conn
            .query_row(&format!("SELECT {DEBT_COLS} FROM debts WHERE id=?1"), [p.debt_id], db::row_debt)
            .optional()
            .s()?
            .ok_or("hutang tidak ditemukan")?;
        let mut tx_id = None;
        if record_expense && p.kind == "payment" {
            let cat = self
                .conn
                .query_row("SELECT id FROM categories WHERE name='Cicilan Hutang' AND kind='expense'", [], |r| r.get(0))
                .optional()
                .s()?;
            tx_id = Some(self.add_tx(&Transaction {
                id: 0,
                kind: "expense".into(),
                amount: p.amount,
                category_id: cat,
                date: p.date.clone(),
                note: format!("Bayar {}", debt.name),
                recurring_id: None,
                debt_id: Some(debt.id),
                created_at: String::new(),
                account_id: account_id.or(self.default_account()?),
            })?);
        }
        self.conn
            .execute(
                "INSERT INTO debt_payments(debt_id, kind, amount, date, note, tx_id) VALUES (?1,?2,?3,?4,?5,?6)",
                params![p.debt_id, p.kind, p.amount, p.date, note, tx_id],
            )
            .s()?;
        Ok(self.conn.last_insert_rowid())
    }

    pub fn delete_debt_payment(&self, id: i64) -> R<()> {
        let tx: Option<Option<i64>> = self
            .conn
            .query_row("SELECT tx_id FROM debt_payments WHERE id=?1", [id], |r| r.get(0))
            .optional()
            .s()?;
        self.conn.execute("DELETE FROM debt_payments WHERE id=?1", [id]).s()?;
        if let Some(Some(t)) = tx {
            self.conn.execute("DELETE FROM transactions WHERE id=?1", [t]).s()?;
        }
        Ok(())
    }

    pub fn debt_views(&self) -> R<Vec<DebtView>> {
        let today = self.today();
        let this_month = month_of(today);
        let mut out = vec![];
        for d in db::debts_all(&self.conn)? {
            let payments = db::payments_for(&self.conn, d.id)?;
            let paid: i64 = payments.iter().filter(|p| p.kind == "payment").map(|p| p.amount).sum();
            let charges: i64 = payments.iter().filter(|p| p.kind == "charge").map(|p| p.amount).sum();
            let total = d.start_balance + charges;
            let balance = (total - paid).max(0);
            let paid_this_month = payments
                .iter()
                .filter(|p| p.kind == "payment" && p.date.starts_with(&this_month))
                .map(|p| p.amount)
                .sum();
            let sim = simulate(
                &[SimDebt { id: d.id, balance: balance as f64, apr: d.apr, min_payment: d.min_payment as f64 }],
                0.0,
                Strategy::Minimum,
            );
            let months_left = (sim.feasible).then_some(sim.months);
            let payoff_label = match months_left {
                _ if balance == 0 => "Lunas".to_string(),
                Some(n) => {
                    let (y, m) = add_months(today.year(), today.month(), n as i32);
                    month_label(y, m)
                }
                None => "Tidak akan lunas".to_string(),
            };
            let (schedule, installments_paid) = installment_schedule(&d, paid, today);
            let next_due = match schedule.iter().find(|i| i.status != "paid") {
                Some(i) => i.date,
                None if d.tenor.is_some() => today,
                None => next_due(today, d.due_day),
            };
            let (months_left, payoff_label) = if d.tenor.is_some() {
                let left = schedule.iter().filter(|i| i.status != "paid").count() as u32;
                let label = if balance == 0 || left == 0 {
                    "Lunas".to_string()
                } else {
                    schedule.last().map(|i| month_label(i.date.year(), i.date.month())).unwrap_or_default()
                };
                (Some(left), label)
            } else {
                (months_left, payoff_label)
            };
            out.push(DebtView {
                schedule,
                installments_paid,
                progress: if total > 0 { (paid as f64 / total as f64).min(1.0) } else { 1.0 },
                monthly_interest: (balance as f64 * d.apr / 100.0 / 12.0).round() as i64,
                days_to_due: days_between(today, next_due),
                next_due,
                months_left,
                payoff_label,
                paid,
                charges,
                balance,
                paid_this_month,
                payments,
                debt: d,
            });
        }
        Ok(out)
    }

    pub fn debt_plan(&self, extra: i64) -> R<DebtPlan> {
        let today = self.today();
        let views: Vec<DebtView> = self
            .debt_views()?
            .into_iter()
            .filter(|v| v.balance > 0 && !v.debt.closed)
            .collect();
        let sims: Vec<SimDebt> = views
            .iter()
            .map(|v| SimDebt {
                id: v.debt.id,
                balance: v.balance as f64,
                apr: if v.debt.tenor.is_some() { 0.0 } else { v.debt.apr },
                min_payment: v.debt.min_payment as f64,
            })
            .collect();
        let name_of = |id: i64| views.iter().find(|v| v.debt.id == id).map(|v| v.debt.name.clone()).unwrap_or_default();
        let mk = |key, label, s: Strategy| {
            let r = simulate(&sims, extra as f64, s);
            let (y, m) = add_months(today.year(), today.month(), r.months as i32);
            PlanOption {
                key,
                label,
                months: r.months,
                total_interest: r.total_interest.round() as i64,
                payoff_label: if !r.feasible {
                    "Tidak akan lunas".into()
                } else if r.months == 0 {
                    "Sudah lunas".into()
                } else {
                    month_label(y, m)
                },
                feasible: r.feasible,
                timeline: r.timeline.iter().map(|v| v.round() as i64).collect(),
                order: r.payoff.iter().map(|(id, _)| name_of(*id)).collect(),
            }
        };
        let options = vec![
            mk("minimum", "Bayar minimum", Strategy::Minimum),
            mk("avalanche", "Avalanche", Strategy::Avalanche),
            mk("snowball", "Snowball", Strategy::Snowball),
        ];
        let (av, sn, mi) = (&options[1], &options[2], &options[0]);
        let diff = sn.total_interest - av.total_interest;
        let recommended = if !av.feasible && sn.feasible {
            2
        } else if diff > 100_000 || (diff > 0 && sn.months > av.months) {
            1
        } else {
            2
        };
        let rec = &options[recommended];
        let message = if views.is_empty() {
            "Tidak ada hutang aktif.".to_string()
        } else if !rec.feasible {
            "Pembayaran belum cukup menutup bunga. Tambah setoran bulanan agar hutang bisa lunas.".to_string()
        } else {
            let save_i = (mi.total_interest - rec.total_interest).max(0);
            let save_m = mi.months as i64 - rec.months as i64;
            let why = if recommended == 1 {
                format!("hemat {} bunga dibanding Snowball", rupiah(diff.max(0)))
            } else {
                "bunganya hampir sama, dan hutang kecil lunas lebih dulu untuk menjaga semangat".to_string()
            };
            if extra > 0 && mi.feasible {
                format!(
                    "Pakai {}: {}. Tambahan {} per bulan mempercepat lunas {} bulan dan menghemat {} bunga.",
                    rec.label, why, rupiah(extra), save_m.max(0), rupiah(save_i)
                )
            } else if extra > 0 {
                format!("Pakai {}: {}. Lunas {}.", rec.label, why, rec.payoff_label)
            } else {
                format!("Pakai {}: {}. Coba tambah setoran untuk melihat berapa bulan bisa dipangkas.", rec.label, why)
            }
        };
        Ok(DebtPlan { options, recommended, message })
    }

    // ---------- money sources ----------

    pub fn accounts(&self) -> R<Vec<Account>> {
        db::accounts(&self.conn)
    }

    pub fn default_account(&self) -> R<Option<i64>> {
        let accs = self.accounts()?;
        let pref = self.meta("default_account")?.and_then(|v| v.parse::<i64>().ok());
        Ok(pref
            .filter(|id| accs.iter().any(|a| a.id == *id && !a.archived))
            .or_else(|| accs.iter().find(|a| !a.archived).map(|a| a.id)))
    }

    pub fn set_default_account(&self, id: i64) -> R<()> {
        self.set_meta("default_account", &id.to_string())
    }

    pub fn save_account(&self, a: &Account) -> R<i64> {
        let name = check_text(&a.name, 40, "Nama sumber dana", true)?;
        if !["bank", "ewallet", "cash", "other"].contains(&a.kind.as_str()) {
            return Err("jenis sumber dana tidak valid".into());
        }
        if a.opening.abs() > 1_000_000_000_000 {
            return Err("saldo awal tidak valid".into());
        }
        let kw = check_text(&a.keywords.to_lowercase(), 400, "Kata kunci", false)?;
        if a.id == 0 {
            let sort: i64 = self.conn.query_row("SELECT COALESCE(MAX(sort), 0) + 1 FROM accounts", [], |r| r.get(0)).s()?;
            self.conn
                .execute(
                    "INSERT INTO accounts(name, kind, color, opening, keywords, sort) VALUES (?1,?2,?3,?4,?5,?6)",
                    params![name, a.kind, a.color, a.opening, kw, sort],
                )
                .s()?;
            Ok(self.conn.last_insert_rowid())
        } else {
            self.conn
                .execute(
                    "UPDATE accounts SET name=?1, kind=?2, color=?3, opening=?4, keywords=?5, archived=?6 WHERE id=?7",
                    params![name, a.kind, a.color, a.opening, kw, a.archived as i64, a.id],
                )
                .s()?;
            Ok(a.id)
        }
    }

    pub fn add_transfer(&self, t: &Transfer) -> R<i64> {
        check_amount(t.amount)?;
        parse_date(&t.date)?;
        if t.from_id == t.to_id {
            return Err("sumber dan tujuan harus berbeda".into());
        }
        let note = check_text(&t.note, 120, "Catatan", false)?;
        self.conn
            .execute(
                "INSERT INTO transfers(from_id, to_id, amount, date, note) VALUES (?1,?2,?3,?4,?5)",
                params![t.from_id, t.to_id, t.amount, t.date, note],
            )
            .s()?;
        Ok(self.conn.last_insert_rowid())
    }

    pub fn delete_transfer(&self, id: i64) -> R<()> {
        self.conn.execute("DELETE FROM transfers WHERE id=?1", [id]).s()?;
        Ok(())
    }

    pub fn transfers_month(&self, y: i32, m: u32) -> R<Vec<Transfer>> {
        let (a, b) = month_bounds(y, m);
        db::transfers_between(&self.conn, &a, &b)
    }

    /// Balance of every source as of the end of `upto` (exclusive bound date).
    pub fn balances_before(&self, upto: &str) -> R<HashMap<i64, i64>> {
        let mut out: HashMap<i64, i64> = self.accounts()?.iter().map(|a| (a.id, a.opening)).collect();
        let mut st = self
            .conn
            .prepare(
                "SELECT account_id, SUM(CASE WHEN kind='income' THEN amount ELSE -amount END)
                 FROM transactions WHERE account_id IS NOT NULL AND date < ?1 GROUP BY account_id",
            )
            .s()?;
        for r in st.query_map([upto], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?))).s()? {
            let (id, v) = r.s()?;
            *out.entry(id).or_default() += v;
        }
        let mut st = self
            .conn
            .prepare("SELECT from_id, to_id, amount FROM transfers WHERE date < ?1")
            .s()?;
        for r in st.query_map([upto], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?, r.get::<_, i64>(2)?))).s()? {
            let (f, t, a) = r.s()?;
            *out.entry(f).or_default() -= a;
            *out.entry(t).or_default() += a;
        }
        Ok(out)
    }

    pub fn balances_now(&self) -> R<HashMap<i64, i64>> {
        // include everything up to far future so future-dated entries still count
        self.balances_before("9999-12-31")
    }

    pub fn pool_total(&self) -> R<i64> {
        let accs = self.accounts()?;
        let b = self.balances_now()?;
        Ok(accs.iter().filter(|a| !a.archived).map(|a| b.get(&a.id).copied().unwrap_or(0)).sum())
    }

    /// Daily total pool for the last `days` days, oldest first.
    pub fn pool_series(&self, days: i64) -> R<Vec<i64>> {
        let today = self.today();
        let start = plus_days(today, -(days - 1));
        let mut level: i64 = self.balances_before(&start.to_string())?.values().sum();
        let mut flows: HashMap<String, i64> = HashMap::new();
        for t in self.transactions_between(start, plus_days(today, 1))? {
            if t.account_id.is_some() {
                *flows.entry(t.date.clone()).or_default() += if t.kind == "income" { t.amount } else { -t.amount };
            }
        }
        let mut out = vec![];
        for i in 0..days {
            let d = plus_days(start, i).to_string();
            level += flows.get(&d).copied().unwrap_or(0);
            out.push(level);
        }
        Ok(out)
    }

    /// Obligations for the next `months` months, current month first. Fixed
    /// installments follow their schedule; flexible debts use the minimum
    /// payment until the simulated payoff month.
    pub fn debt_load(&self, months: u32) -> R<Vec<LoadMonth>> {
        let today = self.today();
        let views = self.debt_views()?;
        let mut out = vec![];
        for k in 0..months as i32 {
            let (y, m) = add_months(today.year(), today.month(), k);
            let key = month_key(y, m);
            let mut items = vec![];
            for v in views.iter().filter(|v| v.balance > 0 && !v.debt.closed) {
                let amt = if v.debt.tenor.is_some() {
                    v.schedule
                        .iter()
                        .filter(|i| i.status != "paid" && (month_of(i.date) == key || (k == 0 && i.date < month_start(y, m))))
                        .map(|i| i.amount)
                        .sum()
                } else if v.months_left.map(|n| (k as u32) < n.max(1)).unwrap_or(true) {
                    let due_this = if k == 0 { (v.debt.min_payment - v.paid_this_month).max(0) } else { v.debt.min_payment };
                    due_this.min(v.balance)
                } else {
                    0
                };
                if amt > 0 {
                    items.push((v.debt.id, amt));
                }
            }
            out.push((MONTHS_ID[(m - 1) as usize][..3].to_string(), items));
        }
        Ok(out)
    }

    // ---------- backup ----------

    pub fn export_json(&self) -> R<String> {
        let q3 = |sql: &str| -> R<Vec<(String, i64, i64)>> {
            let mut st = self.conn.prepare(sql).s()?;
            let rows = st.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).s()?;
            rows.collect::<Result<Vec<_>, _>>().s()
        };
        let mut payments = vec![];
        for d in db::debts_all(&self.conn)? {
            payments.extend(db::payments_for(&self.conn, d.id)?);
        }
        let b = Backup {
            app: "duitrack".into(),
            version: 1,
            categories: self.categories()?,
            transactions: db::transactions_between(&self.conn, "0000-01-01", "9999-12-31")?,
            budgets: q3("SELECT month, category_id, amount FROM budgets")?,
            recurring: self.recurring()?,
            debts: db::debts_all(&self.conn)?,
            debt_payments: payments,
            learn: q3("SELECT token, category_id, n FROM learn")?,
            accounts: self.accounts()?,
            transfers: db::transfers_between(&self.conn, "0000-01-01", "9999-12-31")?,
            goals: self.goals()?,
            goal_entries: self.goal_views()?.into_iter().flat_map(|v| v.entries).collect(),
        };
        serde_json::to_string_pretty(&b).s()
    }

    pub fn import_json(&mut self, json: &str) -> R<()> {
        let b: Backup = serde_json::from_str(json).map_err(|e| format!("file cadangan tidak valid: {e}"))?;
        if b.app != "duitrack" && b.app != "arus" {
            return Err("file ini bukan cadangan Duitrack".into());
        }
        let tx = self.conn.transaction().s()?;
        tx.execute_batch(
            "DELETE FROM debt_payments; DELETE FROM debts; DELETE FROM budgets; DELETE FROM learn;
             DELETE FROM transactions; DELETE FROM recurring; DELETE FROM categories;
             DELETE FROM transfers; DELETE FROM accounts; DELETE FROM goal_entries; DELETE FROM goals;",
        )
        .s()?;
        for g in &b.goals {
            tx.execute(
                "INSERT INTO goals(id, name, target, deadline, start, icon, color, freq, archived) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9)",
                params![g.id, g.name, g.target, g.deadline, g.start, g.icon, g.color, g.freq, g.archived as i64],
            )
            .s()?;
        }
        for e in &b.goal_entries {
            tx.execute(
                "INSERT INTO goal_entries(id, goal_id, amount, date, note) VALUES (?1,?2,?3,?4,?5)",
                params![e.id, e.goal_id, e.amount, e.date, e.note],
            )
            .s()?;
        }
        for a in &b.accounts {
            tx.execute(
                &format!("INSERT INTO accounts({ACC_COLS}) VALUES (?1,?2,?3,?4,?5,?6,?7,?8)"),
                params![a.id, a.name, a.kind, a.color, a.opening, a.keywords, a.archived as i64, a.sort],
            )
            .s()?;
        }
        if b.accounts.is_empty() {
            db::seed_accounts(&tx)?;
        }
        for c in &b.categories {
            tx.execute(
                &format!("INSERT INTO categories({CAT_COLS}) VALUES (?1,?2,?3,?4,?5,?6,?7,?8)"),
                params![c.id, c.name, c.kind, c.icon, c.color, c.keywords, c.archived as i64, c.sort],
            )
            .s()?;
        }
        for r in &b.recurring {
            tx.execute(
                &format!("INSERT INTO recurring({REC_COLS}) VALUES (?1,?2,?3,?4,?5,?6,?7,?8)"),
                params![r.id, r.kind, r.name, r.amount, r.category_id, r.day, r.active as i64, r.account_id],
            )
            .s()?;
        }
        for d in &b.debts {
            tx.execute(
                &format!("INSERT INTO debts({DEBT_COLS}) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)"),
                params![d.id, d.name, d.lender, d.start_balance, d.apr, d.min_payment, d.due_day, d.start_date, d.note, d.closed as i64, d.tenor],
            )
            .s()?;
        }
        for t in &b.transactions {
            tx.execute(
                &format!("INSERT INTO transactions({TX_COLS}) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)"),
                params![t.id, t.kind, t.amount, t.category_id, t.date, t.note, t.recurring_id, t.debt_id, t.created_at, t.account_id],
            )
            .s()?;
        }
        for p in &b.debt_payments {
            tx.execute(
                &format!("INSERT INTO debt_payments({PAY_COLS}) VALUES (?1,?2,?3,?4,?5,?6,?7)"),
                params![p.id, p.debt_id, p.kind, p.amount, p.date, p.note, p.tx_id],
            )
            .s()?;
        }
        for (m, c, a) in &b.budgets {
            tx.execute("INSERT INTO budgets(month, category_id, amount) VALUES (?1,?2,?3)", params![m, c, a]).s()?;
        }
        for t in &b.transfers {
            tx.execute(
                &format!("INSERT INTO transfers({TRF_COLS}) VALUES (?1,?2,?3,?4,?5,?6)"),
                params![t.id, t.from_id, t.to_id, t.amount, t.date, t.note],
            )
            .s()?;
        }
        for (t, c, n) in &b.learn {
            tx.execute("INSERT INTO learn(token, category_id, n) VALUES (?1,?2,?3)", params![t, c, n]).s()?;
        }
        tx.commit().s()
    }

    pub fn reset_all(&self) -> R<()> {
        self.conn
            .execute_batch(
                "DELETE FROM debt_payments; DELETE FROM debts; DELETE FROM budgets; DELETE FROM learn;
                 DELETE FROM transactions; DELETE FROM recurring; DELETE FROM categories;
                 DELETE FROM transfers; DELETE FROM accounts; DELETE FROM goal_entries; DELETE FROM goals;
                 DELETE FROM meta WHERE key = 'default_account';",
            )
            .s()?;
        db::seed_accounts(&self.conn)?;
        db::seed_categories(&self.conn)
    }

    // ---------- demo ----------

    /// Four months of believable history so every screen has something to show.
    pub fn seed_demo(&self) -> R<()> {
        let today = self.today();
        let cats = self.categories()?;
        let cat = |n: &str| cats.iter().find(|c| c.name == n).map(|c| c.id);
        let mut seed: u64 = 0x5eed_1234_abcd;
        let mut rnd = move || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            (seed % 10_000) as f64 / 10_000.0
        };
        let round = |v: f64, step: f64| ((v / step).round() * step) as i64;

        let acc = |name: &str, kind: &str, color: &str, opening: i64, kw: &str| -> R<i64> {
            self.save_account(&Account { id: 0, name: name.into(), kind: kind.into(), color: color.into(), opening, keywords: kw.into(), archived: false, sort: 0 })
        };
        let acc_bca = Some(acc("BCA", "bank", "#2F5D8A", 9_000_000, "bca,mbca,klikbca")?);
        let acc_gopay = Some(acc("GoPay", "ewallet", "#2F7A74", 150_000, "gopay,gojek pay")?);
        let acc_jago = Some(acc("Jago", "bank", "#C4703D", 12_000_000, "jago,bank jago")?);
        let acc_cash = self.accounts()?.into_iter().find(|a| a.kind == "cash").map(|a| a.id);
        if let Some(c) = acc_cash {
            self.conn.execute("UPDATE accounts SET opening = 300000 WHERE id = ?1", [c]).s()?;
        }
        self.set_meta("default_account", &acc_gopay.unwrap().to_string())?;

        let rec = |kind: &str, name: &str, amount: i64, c: &str, day: u32| -> R<i64> {
            self.save_recurring(&Recurring { id: 0, kind: kind.into(), name: name.into(), amount, category_id: cat(c), day, active: true, account_id: acc_bca })
        };
        let r_gaji = rec("income", "Gaji bulanan", 9_500_000, "Gaji", 25)?;
        let r_kos = rec("expense", "Sewa kos", 1_800_000, "Rumah", 1)?;
        let r_net = rec("expense", "Internet rumah", 350_000, "Tagihan & Utilitas", 10)?;
        let r_spot = rec("expense", "Spotify", 54_990, "Hiburan", 18)?;

        let food = ["Kopi susu", "Nasi padang", "Makan siang", "Bakso", "GoFood ayam geprek", "Sarapan bubur", "Martabak", "Starbucks", "Mie ayam", "Warteg"];
        let trans = ["Grab ke kantor", "Gojek", "Bensin", "Parkir", "KRL", "Tol"];
        let shop = ["Indomaret", "Shopee", "Belanja sayur", "Alfamart", "Tokopedia kabel"];
        let fun = ["Nonton XXI", "Padel", "Steam game", "Karaoke"];

        let start = {
            let (y, m) = add_months(today.year(), today.month(), -3);
            month_start(y, m)
        };
        let mut d = start;
        while d <= today {
            let (y, m) = (d.year(), d.month());
            for (rid, day) in [(r_gaji, 25u32), (r_kos, 1), (r_net, 10), (r_spot, 18)] {
                if d.day() == day.min(days_in_month(y, m)) {
                    self.post_recurring_on(rid, d)?;
                }
            }
            let weekend = d.weekday().num_days_from_monday() >= 5;
            let n_food = if weekend { 3 } else { 2 } + (rnd() < 0.3) as usize;
            for _ in 0..n_food {
                let f = food[(rnd() * food.len() as f64) as usize % food.len()];
                let amt = round(15_000.0 + rnd() * if weekend { 70_000.0 } else { 40_000.0 }, 500.0);
                let a = if rnd() < 0.55 { acc_gopay } else if rnd() < 0.6 { acc_cash } else { acc_bca };
                self.add_tx(&tx("expense", amt, cat("Makan & Minum"), d, f, a))?;
            }
            if !weekend || rnd() < 0.4 {
                let t = trans[(rnd() * trans.len() as f64) as usize % trans.len()];
                let amt = if t == "Bensin" { round(100_000.0 + rnd() * 80_000.0, 1000.0) } else { round(8_000.0 + rnd() * 40_000.0, 500.0) };
                self.add_tx(&tx("expense", amt, cat("Transportasi"), d, t, if t == "Bensin" || t == "Tol" { acc_bca } else { acc_gopay }))?;
            }
            if rnd() < 0.22 {
                let s = shop[(rnd() * shop.len() as f64) as usize % shop.len()];
                self.add_tx(&tx("expense", round(40_000.0 + rnd() * 260_000.0, 1000.0), cat("Belanja"), d, s, acc_bca))?;
            }
            if weekend && rnd() < 0.45 {
                let f = fun[(rnd() * fun.len() as f64) as usize % fun.len()];
                self.add_tx(&tx("expense", round(50_000.0 + rnd() * 200_000.0, 5000.0), cat("Hiburan"), d, f, acc_bca))?;
            }
            if [2, 9, 16, 23].contains(&d.day()) {
                self.add_transfer(&Transfer { id: 0, from_id: acc_bca.unwrap(), to_id: acc_gopay.unwrap(), amount: 600_000, date: d.to_string(), note: "Top up GoPay".into() })?;
            }
            if d.day() == 3 || d.day() == 18 {
                if let Some(c) = acc_cash {
                    self.add_transfer(&Transfer { id: 0, from_id: acc_bca.unwrap(), to_id: c, amount: 500_000, date: d.to_string(), note: "Tarik tunai".into() })?;
                }
            }
            if d.day() == 26 && d.month() % 2 == 0 {
                self.add_transfer(&Transfer { id: 0, from_id: acc_bca.unwrap(), to_id: acc_jago.unwrap(), amount: 500_000, date: d.to_string(), note: "Nabung".into() })?;
            }
            if d.day() == 5 {
                self.add_tx(&tx("expense", round(200_000.0 + rnd() * 80_000.0, 1000.0), cat("Tagihan & Utilitas"), d, "Token listrik PLN", acc_bca))?;
                self.add_tx(&tx("expense", 105_000, cat("Tagihan & Utilitas"), d, "Paket data Telkomsel", acc_gopay))?;
            }
            if d.day() == 14 && rnd() < 0.7 {
                self.add_tx(&tx("income", round(1_500_000.0 + rnd() * 2_000_000.0, 50_000.0), cat("Freelance"), d, "Project landing page klien", acc_bca))?;
            }
            if rnd() < 0.05 {
                self.add_tx(&tx("expense", round(60_000.0 + rnd() * 150_000.0, 1000.0), cat("Kesehatan"), d, "Apotek vitamin", acc_cash))?;
            }
            d = d.succ_opt().unwrap();
        }
        self.seed_demo_goals()?;
        // One unusual purchase this month for the anomaly detector.
        let odd = plus_days(today, -2).max(month_start(today.year(), today.month()));
        self.add_tx(&tx("expense", 1_250_000, cat("Belanja"), odd, "Sepatu lari", acc_bca))?;

        // Budgets for the shown months.
        for k in -3..=0 {
            let (y, m) = add_months(today.year(), today.month(), k);
            for (n, a) in [
                ("Makan & Minum", 3_000_000), ("Transportasi", 1_000_000), ("Belanja", 800_000),
                ("Tagihan & Utilitas", 800_000), ("Hiburan", 800_000), ("Rumah", 1_800_000),
                ("Kesehatan", 200_000), ("Cicilan Hutang", 1_300_000),
            ] {
                if let Some(c) = cat(n) {
                    self.set_budget(y, m, c, a)?;
                }
            }
        }

        let debt = |name: &str, lender: &str, bal: i64, apr: f64, min: i64, due: u32| -> R<i64> {
            self.save_debt(&Debt {
                id: 0, name: name.into(), lender: lender.into(), start_balance: bal, apr, min_payment: min,
                due_day: due, start_date: start.to_string(), note: String::new(), closed: false, tenor: None,
            })
        };
        let cc = debt("Kartu kredit", "BCA", 8_500_000, 21.0, 450_000, 15)?;
        let inst = |name: &str, lender: &str, per: i64, tenor: u32, due: u32, from: i32| -> R<i64> {
            let (y, m) = add_months(today.year(), today.month(), from);
            self.save_debt(&Debt {
                id: 0, name: name.into(), lender: lender.into(), start_balance: per * tenor as i64, apr: 0.0, min_payment: per,
                due_day: due, start_date: month_start(y, m).to_string(), note: String::new(), closed: false, tenor: Some(tenor),
            })
        };
        let kakak = inst("Cicilan laptop", "Kakak", 500_000, 6, 28, -3)?;
        let pl = inst("PayLater HP", "Shopee PayLater", 400_000, 3, 5, -1)?;
        for k in -3..=0 {
            let (y, m) = add_months(today.year(), today.month(), k);
            for (id, amt, day, from) in [(cc, 650_000, 14u32, -3), (kakak, 500_000, 27, -3), (pl, 400_000, 4, -1)] {
                if k < from {
                    continue;
                }
                let date = clamp_day(y, m, day);
                if date <= today {
                    self.add_debt_payment(&DebtPayment { id: 0, debt_id: id, kind: "payment".into(), amount: amt, date: date.to_string(), note: String::new(), tx_id: None }, true, acc_bca)?;
                }
            }
        }
        Ok(())
    }

    fn post_recurring_on(&self, id: i64, d: NaiveDate) -> R<()> {
        let r = self
            .conn
            .query_row(&format!("SELECT {REC_COLS} FROM recurring WHERE id=?1"), [id], db::row_recurring)
            .s()?;
        self.add_tx(&Transaction {
            id: 0, kind: r.kind, amount: r.amount, category_id: r.category_id, date: d.to_string(),
            note: r.name, recurring_id: Some(id), debt_id: None, created_at: String::new(), account_id: r.account_id,
        })?;
        Ok(())
    }
}

fn tx(kind: &str, amount: i64, category_id: Option<i64>, d: NaiveDate, note: &str, account_id: Option<i64>) -> Transaction {
    Transaction {
        id: 0,
        kind: kind.into(),
        amount,
        category_id,
        date: d.to_string(),
        note: note.into(),
        recurring_id: None,
        debt_id: None,
        created_at: String::new(),
        account_id,
    }
}

/// Schedule of a fixed-term plan. The first installment is due on `due_day`
/// of the month of `start_date`. Payments fill installments in order.
pub fn installment_schedule(d: &Debt, paid: i64, today: NaiveDate) -> (Vec<Installment>, u32) {
    let Some(tenor) = d.tenor else { return (vec![], 0) };
    let Ok(start) = parse_date(&d.start_date) else { return (vec![], 0) };
    let inst = d.min_payment.max(1);
    let total = d.start_balance;
    let mut out = vec![];
    let mut paid_n = 0;
    for n in 0..tenor {
        let (y, m) = add_months(start.year(), start.month(), n as i32);
        let date = clamp_day(y, m, d.due_day);
        // the last installment absorbs rounding so the plan sums to the total
        let amount = if n + 1 == tenor { (total - inst * (tenor as i64 - 1)).max(1) } else { inst };
        let covered = paid >= inst * n as i64 + amount.min(inst);
        let status = if covered || paid >= total {
            paid_n += 1;
            "paid"
        } else if date < today {
            "late"
        } else if days_between(today, date) <= 7 {
            "due"
        } else {
            "upcoming"
        };
        out.push(Installment { n: n + 1, date, amount, status });
    }
    (out, paid_n)
}

fn env_today() -> Option<NaiveDate> {
    std::env::var("DUITRACK_TODAY").ok().and_then(|s| parse_date(&s).ok())
}
