// Duitrack: personal finance for macOS. Pure Rust, offline, no AI.

slint::include_modules!();

mod view;

use chrono::{Datelike, Local, NaiveDate};
use moneycore::model::{Account, Category, Debt, DebtPayment, Recurring, Transaction, Transfer};
use moneycore::parse::amount_from_text;
use moneycore::util::{add_months, clamp_day, date_label, month_start, parse_date, plus_days, rupiah, R};
use moneycore::Store;
use slint::{ComponentHandle, Model, ModelRc, SharedString, Timer, TimerMode, VecModel};
use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;
use view::*;

enum Undo {
    Deleted(Transaction),
    Added(i64),
}

struct State {
    store: Store,
    y: i32,
    m: u32,
    span: u32,
    period: i32,
    period_offset: i32,
    goal_sel: i64,
    plan_extra: i64,
    undo: Option<Undo>,
    toast_timer: Timer,
    db_path: String,
}

fn today() -> NaiveDate {
    std::env::var("DUITRACK_TODAY")
        .ok()
        .and_then(|s| parse_date(&s).ok())
        .unwrap_or_else(|| Local::now().date_naive())
}

fn model<T: Clone + 'static>(v: Vec<T>) -> ModelRc<T> {
    ModelRc::new(VecModel::from(v))
}

fn db_path() -> String {
    if let Ok(p) = std::env::var("DUITRACK_DB") {
        return p;
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
    let dir = format!("{home}/Library/Application Support/Duitrack");
    let _ = std::fs::create_dir_all(&dir);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700));
    }
    format!("{dir}/duitrack.db")
}

fn system_reduce_motion() -> bool {
    std::process::Command::new("defaults")
        .args(["read", "com.apple.universalaccess", "reduceMotion"])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim() == "1")
        .unwrap_or(false)
}

fn apply_theme(ui: &AppWindow, mode: i32) {
    ui.global::<T>().set_theme_mode(mode.clamp(0, 2));
    ui.global::<D>().set_theme_mode(mode.clamp(0, 2));
}

/// Mirrors T.palette in theme.slint: user-pickable colors.
const PALETTE: [&str; 14] = ["#1E5B3F", "#2E7D52", "#6E8B5C", "#6B7B3A", "#2F7A74", "#3E6E8E", "#2F5D8A", "#55605A", "#8A6A4B", "#B08A2E", "#C4703D", "#A0613A", "#B9534B", "#A8566F"];

fn palette_index(hex: &str) -> i32 {
    PALETTE.iter().position(|c| c.eq_ignore_ascii_case(hex)).unwrap_or(0) as i32
}

const DEBT_TINTS: [&str; 7] = ["#2F5D8A", "#C4703D", "#2F7A74", "#A8566F", "#6B7B3A", "#B08A2E", "#55605A"];

fn apply_motion(ui: &AppWindow, mode: i32) {
    let on = match mode {
        2 => false,
        1 => !system_reduce_motion(),
        _ => true,
    };
    ui.global::<T>().set_motion(on);
}

// ---------------------------------------------------------------- refresh

fn refresh_all(ui: &AppWindow, s: &mut State) {
    let d = ui.global::<D>();
    d.set_month_label(month_label(s.y, s.m).into());
    let t = today();
    d.set_is_current((s.y, s.m) == (t.year(), t.month()));
    d.set_has_data(!s.store.is_empty().unwrap_or(true));
    for r in [
        refresh_cats(ui, s),
        refresh_overview(ui, s),
        refresh_period(ui, s),
        refresh_tx(ui, s),
        refresh_wallets(ui, s),
        refresh_budget(ui, s),
        refresh_income(ui, s),
        refresh_debts(ui, s),
        refresh_plan(ui, s),
        refresh_analytics(ui, s),
        refresh_goals(ui, s),
    ] {
        if let Err(e) = r {
            toast(ui, s, &format!("Gagal memuat: {e}"), None);
        }
    }
}

fn refresh_cats(ui: &AppWindow, s: &State) -> R<()> {
    let cats = s.store.categories()?;
    let d = ui.global::<D>();
    let live = |k: &str| cats.iter().filter(|c| c.kind == k && !c.archived).map(cat_opt).collect::<Vec<_>>();
    d.set_exp_cats(model(live("expense")));
    d.set_inc_cats(model(live("income")));
    let accs = s.store.accounts()?;
    let def = s.store.default_account()?;
    let mut opts: Vec<&Account> = accs.iter().filter(|a| !a.archived).collect();
    opts.sort_by_key(|a| (Some(a.id) != def, a.sort, a.id));
    d.set_acc_opts(model(
        opts.iter()
            .map(|a| AccOpt { id: a.id as i32, name: a.name.clone().into(), kind: a.kind.clone().into(), icon: acc_icon(&a.kind).into(), tint: color(&a.color) })
            .collect(),
    ));
    Ok(())
}

fn refresh_overview(ui: &AppWindow, s: &State) -> R<()> {
    let dash = s.store.dashboard(s.y, s.m)?;
    let cats = s.store.categories()?;
    let accs = s.store.accounts()?;
    let t = today();
    let d = ui.global::<D>();

    d.set_income(dash.income as f32);
    d.set_income_text(rp(dash.income));
    d.set_expense(dash.expense as f32);
    d.set_expense_text(rp(dash.expense));
    d.set_projected_text(
        if dash.is_current {
            format!("Prediksi akhir bulan {}", rupiah(dash.projected_expense))
        } else if dash.is_future {
            "Bulan belum berjalan".into()
        } else {
            format!("Rata-rata {} per hari", rupiah(dash.expense / dash.days_in_month.max(1) as i64))
        }
        .into(),
    );
    if dash.is_current {
        d.set_safe_today(dash.safe_today as f32);
        d.set_safe_today_text(rp(dash.safe_today));
        d.set_safe_caption(
            match dash.safe_basis {
                "budget" => format!("Sisa anggaran untuk {} hari", dash.days_left),
                "income" => format!("Sisa pemasukan dibagi {} hari", dash.days_left),
                "pool" => format!("Dibatasi total uang, {} hari tersisa", dash.days_left),
                _ => "Buat anggaran untuk menghitung".into(),
            }
            .into(),
        );
    } else {
        let net = dash.income - dash.expense;
        d.set_safe_today(net as f32);
        d.set_safe_today_text(rp(net));
        d.set_safe_caption("Masuk dikurangi keluar".into());
    }
    d.set_budget_used(if dash.budget_total > 0 { dash.expense as f32 / dash.budget_total as f32 } else { -1.0 });
    d.set_savings_rate(dash.savings_rate as f32);
    d.set_savings_text(if dash.income > 0 { pct(dash.savings_rate).into() } else { "-".into() });
    d.set_budget_text(
        if dash.budget_total > 0 {
            format!("{} direncanakan untuk belanja", rupiah(dash.budget_total))
        } else {
            "Belum ada anggaran".into()
        }
        .into(),
    );

    // Cumulative spend chart.
    let shown = if dash.is_current { dash.day as usize } else if dash.is_future { 0 } else { dash.days_in_month as usize };
    let actual: Vec<f64> = dash.cumulative[..shown].iter().map(|v| *v as f64).collect();
    let typical: Vec<f64> = dash.typical_cumulative.iter().map(|v| *v as f64).collect();
    let max = [
        dash.budget_total as f64,
        typical.iter().cloned().fold(0.0, f64::max),
        actual.iter().cloned().fold(0.0, f64::max),
    ]
    .into_iter()
    .fold(0.0, f64::max)
        * 1.1;
    let slots = dash.days_in_month as usize;
    d.set_chart_line(line_path(&actual, max, slots).into());
    d.set_chart_area(area_path(&actual, max, slots).into());
    d.set_chart_typical(if typical.iter().any(|v| *v > 0.0) { line_path(&typical, max, slots).into() } else { SharedString::new() });
    d.set_chart_budget_y(if dash.budget_total > 0 && max > 0.0 { (1.0 - dash.budget_total as f64 / max) as f32 } else { -1.0 });
    d.set_chart_max_text(compact(max as i64).into());
    d.set_chart_mid_text(compact((max / 2.0) as i64).into());
    d.set_chart_end_label(dash.days_in_month.to_string().into());
    d.set_chart_caption(if dash.is_current { "kumulatif sampai hari ini".into() } else { "kumulatif sebulan".into() });

    d.set_cat_stats(model(
        dash.categories
            .iter()
            .map(|c| CatStat {
                id: c.id as i32,
                name: c.name.clone().into(),
                icon: c.icon.clone().into(),
                tint: color(&c.color),
                planned_text: rp(c.planned),
                spent_text: rp(c.spent),
                projected_text: rp(c.projected),
                ratio: if c.planned > 0 { c.spent as f32 / c.planned as f32 } else { 0.0 },
                proj_ratio: if c.planned > 0 { c.projected as f32 / c.planned as f32 } else { 0.0 },
                status: c.status.into(),
                share: c.share as f32,
                has_plan: c.planned > 0,
            })
            .collect(),
    ));
    d.set_insights(model(
        dash.insights
            .iter()
            .map(|i| InsightRow { tone: i.tone.into(), title: i.title.clone().into(), body: i.body.clone().into() })
            .collect(),
    ));
    d.set_due(model(
        dash.due_recurring
            .iter()
            .map(|r| DueRow {
                id: r.rec.id as i32,
                kind: r.rec.kind.clone().into(),
                name: r.rec.name.clone().into(),
                amount_text: rp(r.rec.amount),
                date_text: (if r.overdue { format!("Lewat, {}", short_date(&r.date.to_string())) } else { friendly_date(&r.date.to_string(), t) }).into(),
                overdue: r.overdue,
            })
            .collect(),
    ));
    d.set_history(model(bars(&dash.history, true)));
    d.set_weekday(model(bars(
        &moneycore::util::WEEKDAYS_ID.iter().zip(dash.weekday_avg.iter()).map(|(n, v)| (n[..3].to_string(), 0, *v)).collect::<Vec<_>>(),
        false,
    )));
    d.set_recent(model(dash.recent.iter().map(|x| tx_row(x, &cats, &accs, t)).collect()));
    d.set_pool(dash.pool as f32);
    d.set_pool_text(rp(dash.pool));
    let load = s.store.debt_load(1)?;
    d.set_inst_month_text(rp(load.first().map(|m| m.1.iter().map(|x| x.1).sum()).unwrap_or(0)));
    d.set_has_debt(dash.debt_balance > 0);
    d.set_debt_balance(dash.debt_balance as f32);
    d.set_debt_balance_text(rp(dash.debt_balance));
    d.set_debt_progress(dash.debt_progress as f32);
    d.set_debt_next_text(
        match &dash.next_debt {
            Some((name, days, amt)) => match days {
                0 => format!("{name} jatuh tempo hari ini, {}", rupiah(*amt)),
                1 => format!("{name} jatuh tempo besok, {}", rupiah(*amt)),
                n => format!("{name} jatuh tempo {n} hari lagi, {}", rupiah(*amt)),
            },
            None => "Semua hutang lunas".into(),
        }
        .into(),
    );
    Ok(())
}

fn delta(cur: i64, prev: i64, up_is_good: bool) -> (SharedString, SharedString) {
    if prev <= 0 {
        return if cur > 0 { ("baru".into(), "neutral".into()) } else { (SharedString::new(), "neutral".into()) };
    }
    let ch = (cur - prev) as f64 / prev as f64;
    if ch.abs() < 0.03 {
        return ("stabil".into(), "neutral".into());
    }
    let good = (ch > 0.0) == up_is_good;
    (
        format!("{}{}%", if ch > 0.0 { "+" } else { "-" }, (ch.abs() * 100.0).round().min(999.0) as i64).into(),
        if good { "pos" } else { "neg" }.into(),
    )
}

fn refresh_period(ui: &AppWindow, s: &State) -> R<()> {
    use moneycore::period::Period;
    let d = ui.global::<D>();
    let p = Period::from_index(s.period);
    let v = s.store.period_view(p, s.period_offset)?;
    d.set_period(s.period);
    d.set_period_label(v.label.clone().into());
    d.set_period_current(s.period_offset >= 0);
    d.set_compare_label(v.compare_label.into());
    d.set_line_label(if p == Period::Day { "minggu lalu".into() } else { v.compare_label.into() });

    let net = v.income - v.expense;
    d.set_p_net(net as f32);
    d.set_p_net_text(format!("{}{}", if net > 0 { "+" } else { "" }, rupiah(net)).into());
    d.set_p_net_tone(if net < 0 { "neg" } else { "pos" }.into());
    d.set_p_in_text(rp(v.income));
    d.set_p_out_text(rp(v.expense));
    let (t, tone) = delta(v.income, v.prev_income, true);
    d.set_p_in_delta(t);
    d.set_p_in_tone(tone);
    let (t, tone) = delta(v.expense, v.prev_expense, false);
    d.set_p_out_delta(t);
    d.set_p_out_tone(tone);
    d.set_p_avg_text(rp(if v.days_elapsed > 0 { v.expense / v.days_elapsed } else { 0 }));

    let max = v.buckets.iter().map(|b| b.expense.max(b.prev_expense)).max().unwrap_or(0).max(1) as f32 * 1.1;
    d.set_p_buckets(model(
        v.buckets
            .iter()
            .map(|b| PBucket {
                label: b.label.clone().into(),
                inc: (b.income as f32 / max).min(1.0),
                exp: b.expense as f32 / max,
                prev: b.prev_expense as f32 / max,
                current: b.current,
                future: b.future,
                inc_text: rp(b.income),
                exp_text: rp(b.expense),
                prev_text: rp(b.prev_expense),
            })
            .collect(),
    ));
    let has_spend = v.buckets.iter().any(|b| b.expense > 0 || b.prev_expense > 0);
    d.set_p_max_text(if has_spend { compact(max as i64).into() } else { SharedString::new() });
    d.set_p_mid_text(if has_spend { compact((max / 2.0) as i64).into() } else { SharedString::new() });
    let prev: Vec<f32> = v.buckets.iter().map(|b| b.prev_expense as f32 / max).collect();
    d.set_p_prev_path(if prev.iter().any(|x| *x > 0.0) { centered_line(&prev).into() } else { SharedString::new() });

    let spent: Vec<_> = v.categories.iter().filter(|c| c.amount > 0).collect();
    let top = spent.first().map(|c| c.amount).unwrap_or(1).max(1) as f32;
    d.set_p_cats(model(
        spent
            .iter()
            .take(6)
            .map(|c| {
                let (t, tone) = delta(c.amount, c.prev, false);
                PCat {
                    name: c.name.clone().into(),
                    tint: color(&c.color),
                    icon: c.icon.clone().into(),
                    amount_text: rp(c.amount),
                    ratio: c.amount as f32 / top,
                    share_text: pct(c.amount as f64 / v.expense.max(1) as f64).into(),
                    delta_text: t,
                    delta_tone: tone,
                }
            })
            .collect(),
    ));
    let mut shares: Vec<(String, String, i64)> = spent.iter().take(5).map(|c| (c.name.clone(), c.color.clone(), c.amount)).collect();
    let rest: i64 = spent.iter().skip(5).map(|c| c.amount).sum();
    if rest > 0 {
        shares.push(("Lainnya".into(), "#A7ACA6".into(), rest));
    }
    let total: i64 = shares.iter().map(|x| x.2).sum();
    let paths = donut_paths(&shares.iter().map(|x| x.2 as f64).collect::<Vec<_>>());
    d.set_p_slices(model(
        shares
            .iter()
            .zip(paths)
            .map(|(x, path)| Slice {
                name: x.0.clone().into(),
                tint: color(&x.1),
                path: path.into(),
                share_text: pct(x.2 as f64 / total.max(1) as f64).into(),
                amount_text: rp(x.2),
            })
            .collect(),
    ));
    d.set_p_total_text(compact(total).into());
    refresh_status(ui, s)
}

/// Condition tile + the "what to do now" list, always about today.
fn refresh_status(ui: &AppWindow, s: &State) -> R<()> {
    let d = ui.global::<D>();
    let t = today();
    let dash = s.store.dashboard(t.year(), t.month())?;
    let a = s.store.analytics(6)?;
    let bad = dash.insights.iter().filter(|i| i.tone == "bad").count();
    let warn = dash.insights.iter().filter(|i| i.tone == "warn").count();
    let score = a.health as f32;
    let tone = if bad > 0 && score < 65.0 || score < 45.0 { "neg" } else if bad + warn > 0 || score < 65.0 { "warn" } else { "pos" };
    if s.store.is_empty()? {
        // No data is not a bad score: stay neutral until there is something to judge.
        d.set_status_score(0.0);
        d.set_status_label("Mulai".into());
        d.set_status_tone("pos".into());
        d.set_status_line("Catat transaksi pertamamu".into());
        d.set_safe_tone("pos".into());
        d.set_actions(model(vec![]));
        return Ok(());
    }
    d.set_status_score(score);
    d.set_status_label(match tone { "neg" => "Kritis", "warn" => "Waspada", _ => "Sehat" }.into());
    d.set_status_tone(tone.into());
    d.set_status_line(
        if bad + warn == 0 { "Tidak ada yang mengkhawatirkan".to_string() } else { format!("{} hal perlu perhatian", bad + warn) }.into(),
    );
    // How generous is today's allowance compared with a normal day?
    let typical_day = a.avg_expense as f64 / 30.4;
    let ratio = if typical_day > 0.0 { dash.safe_today as f64 / typical_day } else { 1.0 };
    d.set_safe_tone(if dash.safe_today <= 0 || ratio < 0.4 { "neg" } else if ratio < 0.8 { "warn" } else { "pos" }.into());

    let mut acts: Vec<ActionRow> = vec![];
    for r in &dash.due_recurring {
        acts.push(ActionRow {
            kind: "rec".into(),
            id: r.rec.id as i32,
            title: (if r.rec.kind == "income" { format!("Catat {}", r.rec.name) } else { format!("Bayar {}", r.rec.name) }).into(),
            value: format!("{}, {}", rupiah(r.rec.amount), if r.overdue { "lewat".to_string() } else { friendly_date(&r.date.to_string(), t).to_lowercase() }).into(),
            tone: if r.overdue { "warn" } else { "neutral" }.into(),
            button: "Catat".into(),
        });
    }
    for v in s.store.debt_views()?.iter().filter(|v| v.balance > 0 && !v.debt.closed) {
        let due_amt = if v.debt.tenor.is_some() {
            v.schedule.iter().find(|i| i.status != "paid").map(|i| (i.amount, i.status))
        } else if v.paid_this_month < v.debt.min_payment {
            Some((v.debt.min_payment.min(v.balance), if v.days_to_due < 0 { "late" } else { "due" }))
        } else {
            None
        };
        if let Some((amt, status)) = due_amt {
            if v.days_to_due <= 7 || status == "late" {
                let when = match v.days_to_due { d if d < 0 => "telat".to_string(), 0 => "hari ini".into(), 1 => "besok".into(), n => format!("{n} hari lagi") };
                acts.push(ActionRow {
                    kind: "debt".into(),
                    id: v.debt.id as i32,
                    title: format!("Bayar {}", v.debt.name).into(),
                    value: format!("{}, {}", rupiah(amt), when).into(),
                    tone: if status == "late" || v.days_to_due <= 1 { "neg" } else { "warn" }.into(),
                    button: "Bayar".into(),
                });
            }
        }
    }
    for c in dash.categories.iter().filter(|c| c.status == "over") {
        acts.push(ActionRow {
            kind: "budget".into(),
            id: c.id as i32,
            title: format!("Rem {}", c.name).into(),
            value: format!("lewat {}", rupiah(c.spent - c.planned)).into(),
            tone: "neg".into(),
            button: "Atur".into(),
        });
    }
    for c in dash.categories.iter().filter(|c| c.status == "risk").take(2) {
        let per_day = (c.planned - c.spent).max(0) / dash.days_left.max(1) as i64;
        acts.push(ActionRow {
            kind: "budget".into(),
            id: c.id as i32,
            title: format!("Hemat {}", c.name).into(),
            value: format!("maks {} / hari", rupiah(per_day)).into(),
            tone: "warn".into(),
            button: "Lihat".into(),
        });
    }
    let bals = s.store.balances_now()?;
    for acc in s.store.accounts()?.iter().filter(|a| !a.archived) {
        let b = bals.get(&acc.id).copied().unwrap_or(0);
        if b < 0 {
            acts.push(ActionRow {
                kind: "transfer".into(),
                id: acc.id as i32,
                title: format!("Isi saldo {}", acc.name).into(),
                value: format!("minus {}", rupiah(-b)).into(),
                tone: "neg".into(),
                button: "Pindah".into(),
            });
        }
    }
    if dash.budget_total == 0 {
        acts.push(ActionRow { kind: "budget".into(), id: -1, title: "Buat anggaran".into(), value: "pakai saran otomatis".into(), tone: "neutral".into(), button: "Atur".into() });
    }
    let rank = |a: &ActionRow| match a.tone.as_str() { "neg" => 0, "warn" => 1, _ => 2 };
    acts.sort_by_key(rank);
    acts.truncate(6);
    d.set_actions(model(acts));
    Ok(())
}

fn refresh_tx(ui: &AppWindow, s: &State) -> R<()> {
    let d = ui.global::<D>();
    let cats = s.store.categories()?;
    let accs = s.store.accounts()?;
    let filter = d.get_tx_filter();
    let q = d.get_tx_search().to_lowercase();
    let txs: Vec<Transaction> = s
        .store
        .transactions_month(s.y, s.m)?
        .into_iter()
        .filter(|t| match filter {
            1 => t.kind == "income",
            2 => t.kind == "expense",
            _ => true,
        })
        .filter(|t| {
            q.is_empty()
                || t.note.to_lowercase().contains(&q)
                || cats.iter().any(|c| Some(c.id) == t.category_id && c.name.to_lowercase().contains(&q))
                || accs.iter().any(|a| Some(a.id) == t.account_id && a.name.to_lowercase().contains(&q))
        })
        .collect();
    let inc: i64 = txs.iter().filter(|t| t.kind == "income").map(|t| t.amount).sum();
    let out: i64 = txs.iter().filter(|t| t.kind == "expense").map(|t| t.amount).sum();
    d.set_tx_in_text(rp(inc));
    d.set_tx_out_text(rp(out));
    d.set_tx_count_text(format!("{} catatan", txs.len()).into());
    d.set_tx_rows(model(grouped(&txs, &cats, &accs, today())));
    Ok(())
}

fn refresh_budget(ui: &AppWindow, s: &State) -> R<()> {
    let d = ui.global::<D>();
    let rows = s.store.budget_rows(s.y, s.m)?;
    let income: i64 = s.store.transactions_month(s.y, s.m)?.iter().filter(|t| t.kind == "income").map(|t| t.amount).sum();
    let planned: i64 = rows.iter().map(|r| r.planned).sum();
    let spent: i64 = rows.iter().map(|r| r.spent).sum();
    d.set_budget_rows(model(
        rows.iter()
            .map(|r| {
                let over = r.planned > 0 && r.spent > r.planned;
                BudgetRow {
                    cat_id: r.category.id as i32,
                    name: r.category.name.clone().into(),
                    icon: r.category.icon.clone().into(),
                    tint: color(&r.category.color),
                    planned_raw: if r.planned > 0 { raw_amount(r.planned) } else { SharedString::new() },
                    planned_text: raw_amount(r.planned),
                    spent_text: rp(r.spent),
                    ratio: if r.planned > 0 { r.spent as f32 / r.planned as f32 } else { 0.0 },
                    remaining_text: rp((r.planned - r.spent).abs()),
                    over,
                    suggested: (r.suggested > 0) as i32,
                    suggested_text: raw_amount(r.suggested),
                    hint: (if r.prev_spent > 0 {
                        format!("Bulan lalu {}", rupiah(r.prev_spent))
                    } else if r.suggested > 0 {
                        "Belum direncanakan".into()
                    } else {
                        "Belum ada riwayat".into()
                    })
                    .into(),
                }
            })
            .collect(),
    ));
    d.set_b_planned_text(rp(planned));
    d.set_b_spent_text(rp(spent));
    d.set_b_income_text(rp(income));
    d.set_b_alloc(if income > 0 { (planned as f32 / income as f32).min(1.0) } else { 0.0 });
    d.set_b_alloc_text(if income > 0 { pct(planned as f64 / income as f64).into() } else { "-".into() });
    d.set_b_over_income(income > 0 && planned > income);
    d.set_b_has_suggest(rows.iter().any(|r| r.suggested > 0));
    d.set_b_free_text(
        if income == 0 {
            "Catat pemasukan bulan ini untuk melihat sisa yang belum dialokasikan.".to_string()
        } else if planned > income {
            format!("Anggaran melebihi pemasukan sebesar {}.", rupiah(planned - income))
        } else {
            format!("{} belum dialokasikan. Arahkan ke tabungan atau pelunasan hutang.", rupiah(income - planned))
        }
        .into(),
    );
    Ok(())
}

fn refresh_income(ui: &AppWindow, s: &State) -> R<()> {
    let d = ui.global::<D>();
    let cats = s.store.categories()?;
    let accs = s.store.accounts()?;
    let t = today();
    let txs: Vec<Transaction> = s.store.transactions_month(s.y, s.m)?.into_iter().filter(|t| t.kind == "income").collect();
    d.set_income_rows(model(txs.iter().map(|x| tx_row(x, &cats, &accs, t)).collect()));
    d.set_inc_month_text(rp(txs.iter().map(|t| t.amount).sum()));
    let mut prev = 0;
    for k in 1..=3 {
        let (py, pm) = add_months(s.y, s.m, -k);
        prev += s.store.transactions_month(py, pm)?.iter().filter(|t| t.kind == "income").map(|t| t.amount).sum::<i64>();
    }
    d.set_inc_avg_text(rp(prev / 3));
    let recs = s.store.recurring()?;
    let posted = s.store.posted_recurring(s.y, s.m)?;
    let sum = |k: &str| recs.iter().filter(|r| r.kind == k && r.active).map(|r| r.amount).sum::<i64>();
    d.set_inc_fixed_text(rp(sum("income")));
    d.set_exp_fixed_text(rp(sum("expense")));
    d.set_rec_rows(model(
        recs.iter()
            .map(|r| {
                let c = cats.iter().find(|c| Some(c.id) == r.category_id);
                RecRow {
                    id: r.id as i32,
                    kind: r.kind.clone().into(),
                    name: r.name.clone().into(),
                    amount_text: rp(r.amount),
                    amount_raw: raw_amount(r.amount),
                    day: r.day as i32,
                    cat_id: c.map(|c| c.id as i32).unwrap_or(-1),
                    cat_name: c.map(|c| c.name.clone()).unwrap_or_else(|| "Tanpa kategori".into()).into(),
                    icon: c.map(|c| c.icon.clone()).unwrap_or_else(|| "repeat".into()).into(),
                    tint: c.map(|c| color(&c.color)).unwrap_or(color("#64748B")),
                    posted: posted.contains(&r.id),
                    acc_id: r.account_id.map(|a| a as i32).unwrap_or(-1),
                }
            })
            .collect(),
    ));
    Ok(())
}

fn debt_row(v: &moneycore::store::DebtView, tint: &str) -> DebtRow {
    let is_inst = v.debt.tenor.is_some();
    let months = match v.months_left {
        Some(n) if n >= 12 => format!("{} ({} thn {} bln)", v.payoff_label, n / 12, n % 12),
        Some(n) if n > 0 => format!("{} ({} bln)", v.payoff_label, n),
        _ => v.payoff_label.clone(),
    };
    let next = if v.balance == 0 {
        "Lunas".to_string()
    } else if is_inst {
        match v.schedule.iter().find(|i| i.status != "paid") {
            Some(i) => format!("Cicilan ke-{} {}, {}", i.n, date_label(i.date), rupiah(i.amount)),
            None => "Lunas".into(),
        }
    } else {
        format!("Lunas {}", months)
    };
    DebtRow {
        id: v.debt.id as i32,
        name: v.debt.name.clone().into(),
        lender: v.debt.lender.clone().into(),
        balance: v.balance as f32,
        balance_text: rp(v.balance),
        start_text: rp(v.debt.start_balance + v.charges),
        paid_text: rp(v.paid),
        progress: v.progress as f32,
        apr_text: format!("{}%/thn", format!("{}", v.debt.apr).replace('.', ",")).into(),
        min_text: rp(v.debt.min_payment),
        due_text: v.debt.due_day.to_string().into(),
        days_to_due: v.days_to_due as i32,
        payoff_text: months.into(),
        interest_text: rp(v.monthly_interest),
        settled: v.balance == 0,
        trapped: v.balance > 0 && v.months_left.is_none(),
        raw_start: raw_amount(v.balance),
        raw_apr: format!("{}", v.debt.apr).replace('.', ",").into(),
        raw_min: raw_amount(v.debt.min_payment),
        raw_due: v.debt.due_day.to_string().into(),
        raw_lender: v.debt.lender.clone().into(),
        tint: color(tint),
        is_inst,
        tenor: v.debt.tenor.unwrap_or(0) as i32,
        paid_n: v.installments_paid as i32,
        next_text: next.into(),
        segs: model(v.schedule.iter().map(|i| InstSeg { status: i.status.into() }).collect()),
        sched: model(
            v.schedule
                .iter()
                .map(|i| SchedRow { n: i.n as i32, date_text: date_label(i.date).into(), amount_text: rp(i.amount), status: i.status.into() })
                .collect(),
        ),
    }
}

fn refresh_debts(ui: &AppWindow, s: &State) -> R<()> {
    let d = ui.global::<D>();
    let views = s.store.debt_views()?;
    let tint_of = |id: i64| DEBT_TINTS[views.iter().position(|v| v.debt.id == id).unwrap_or(0) % DEBT_TINTS.len()];
    let active: Vec<_> = views.iter().filter(|v| !v.debt.closed).collect();
    let bal: i64 = active.iter().map(|v| v.balance).sum();
    let paid: i64 = active.iter().map(|v| v.paid).sum();
    let total: i64 = active.iter().map(|v| v.debt.start_balance + v.charges).sum();
    let rows: Vec<DebtRow> = views.iter().map(|v| debt_row(v, tint_of(v.debt.id))).collect();
    d.set_inst_rows(model(rows.iter().filter(|r| r.is_inst && !r.settled).cloned().collect()));
    d.set_debts(model(rows));
    d.set_d_total_text(rp(bal));
    d.set_d_paid_text(rp(paid));
    d.set_d_progress(if total > 0 { paid as f32 / total as f32 } else { 0.0 });
    let min_total: i64 = active.iter().filter(|v| v.balance > 0).map(|v| v.debt.min_payment.min(v.balance)).sum();
    d.set_d_min_text(rp(min_total));
    d.set_d_interest_text(rp(active.iter().map(|v| v.monthly_interest).sum()));
    let max = ((min_total * 4).max(2_000_000) + 499_999) / 500_000 * 500_000;
    d.set_plan_extra_max(max as f32);
    d.set_plan_max_text(compact(max).into());

    // Monthly obligations, stacked per debt.
    let load = s.store.debt_load(6)?;
    let totals: Vec<i64> = load.iter().map(|m| m.1.iter().map(|x| x.1).sum()).collect();
    let peak = totals.iter().cloned().max().unwrap_or(0).max(1) as f32;
    d.set_load_bars(model(
        load.iter()
            .enumerate()
            .map(|(k, (label, items))| {
                let mut acc = 0f32;
                let segs: Vec<Seg> = items
                    .iter()
                    .map(|(id, amt)| {
                        let len = *amt as f32 / peak;
                        let seg = Seg { start: acc, len, tint: color(tint_of(*id)) };
                        acc += len;
                        seg
                    })
                    .collect();
                LoadBar { label: label.clone().into(), total_text: compact(totals[k]).into(), segs: model(segs), current: k == 0 }
            })
            .collect(),
    ));
    let mut legend_ids: Vec<i64> = load.iter().flat_map(|m| m.1.iter().map(|x| x.0)).collect();
    legend_ids.sort_by_key(|id| views.iter().position(|v| v.debt.id == *id));
    legend_ids.dedup();
    d.set_load_legend(model(
        legend_ids
            .iter()
            .filter_map(|id| views.iter().find(|v| v.debt.id == *id))
            .map(|v| LegendItem { name: v.debt.name.clone().into(), tint: color(tint_of(v.debt.id)), value_text: SharedString::new() })
            .collect(),
    ));
    let now = totals.first().copied().unwrap_or(0);
    d.set_load_now_text(rp(now));
    d.set_load_drop_text(
        totals
            .iter()
            .enumerate()
            .skip(1)
            .find(|(_, t)| **t < now)
            .map(|(k, t)| format!("Turun ke {} mulai {}", compact(*t), load[k].0))
            .unwrap_or_default()
            .into(),
    );

    let sel = d.get_debt_selected() as i64;
    if let Some(v) = views.iter().find(|v| v.debt.id == sel) {
        d.set_debt_current(debt_row(v, tint_of(v.debt.id)));
        d.set_payments(model(
            v.payments
                .iter()
                .map(|p| PaymentRow {
                    id: p.id as i32,
                    kind: p.kind.clone().into(),
                    amount_text: rp(p.amount),
                    date_text: short_date(&p.date).into(),
                    note: p.note.clone().into(),
                })
                .collect(),
        ));
    } else {
        d.set_debt_detail(false);
    }
    Ok(())
}

fn refresh_wallets(ui: &AppWindow, s: &State) -> R<()> {
    let d = ui.global::<D>();
    let accs = s.store.accounts()?;
    let bals = s.store.balances_now()?;
    let def = s.store.default_account()?;
    let txs = s.store.transactions_month(s.y, s.m)?;
    let live: Vec<&Account> = accs.iter().filter(|a| !a.archived).collect();
    let positive: i64 = live.iter().map(|a| bals.get(&a.id).copied().unwrap_or(0).max(0)).sum();
    d.set_accounts(model(
        live.iter()
            .map(|a| {
                let b = bals.get(&a.id).copied().unwrap_or(0);
                let flow = |k: &str| txs.iter().filter(|t| t.kind == k && t.account_id == Some(a.id)).map(|t| t.amount).sum::<i64>();
                AccRow {
                    id: a.id as i32,
                    name: a.name.clone().into(),
                    kind: a.kind.clone().into(),
                    kind_label: acc_kind_label(&a.kind).into(),
                    icon: acc_icon(&a.kind).into(),
                    tint: color(&a.color),
                    balance: b as f32,
                    balance_text: rp(b),
                    raw_balance: raw_amount(b.max(0)),
                    in_text: compact(flow("income")).into(),
                    out_text: compact(flow("expense")).into(),
                    share: if positive > 0 { b.max(0) as f32 / positive as f32 } else { 0.0 },
                    is_default: Some(a.id) == def,
                    keywords: a.keywords.clone().into(),
                    negative: b < 0,
                }
            })
            .collect(),
    ));
    let inc: i64 = txs.iter().filter(|t| t.kind == "income").map(|t| t.amount).sum();
    let out: i64 = txs.iter().filter(|t| t.kind == "expense").map(|t| t.amount).sum();
    d.set_w_in_text(rp(inc));
    d.set_w_out_text(rp(out));

    let series = s.store.pool_series(30)?;
    let vals: Vec<f64> = series.iter().map(|v| *v as f64).collect();
    let n = normalise(&vals);
    d.set_pool_line(line_path(&n, 1.0, n.len()).into());
    d.set_pool_area(area_path(&n, 1.0, n.len()).into());
    let change = series.last().copied().unwrap_or(0) - series.first().copied().unwrap_or(0);
    d.set_pool_change_text(format!("{}{}", if change >= 0 { "+" } else { "" }, compact(change)).into());
    d.set_pool_change_tone(if change >= 0 { "pos" } else { "neg" }.into());
    d.set_pool_min_text(compact(series.iter().cloned().min().unwrap_or(0)).into());
    d.set_pool_max_text(compact(series.iter().cloned().max().unwrap_or(0)).into());

    let names = |id: i64| accs.iter().find(|a| a.id == id).map(|a| a.name.clone()).unwrap_or_else(|| "?".into());
    d.set_transfers(model(
        s.store
            .transfers_month(s.y, s.m)?
            .iter()
            .map(|t| TransferRow {
                id: t.id as i32,
                from_name: names(t.from_id).into(),
                to_name: names(t.to_id).into(),
                amount_text: rp(t.amount),
                date_text: short_date(&t.date).into(),
                note: t.note.clone().into(),
            })
            .collect(),
    ));
    Ok(())
}

fn refresh_plan(ui: &AppWindow, s: &State) -> R<()> {
    let d = ui.global::<D>();
    let plan = s.store.debt_plan(s.plan_extra)?;
    let tints = ["#A7ACA6", "#2F5D8A", "#1E5B3F"];
    let feasible_len = plan.options.iter().filter(|o| o.feasible).map(|o| o.timeline.len()).max().unwrap_or(0);
    let len = if feasible_len > 1 { feasible_len } else { 121 };
    let max = plan.options.iter().flat_map(|o| o.timeline.iter()).cloned().max().unwrap_or(0) as f64 * 1.05;
    d.set_plan(model(
        plan.options
            .iter()
            .enumerate()
            .map(|(i, o)| {
                let vals: Vec<f64> = o.timeline.iter().take(len).map(|v| *v as f64).collect();
                let months = if !o.feasible {
                    "Tidak lunas".to_string()
                } else if o.months >= 12 {
                    format!("{} thn {} bln", o.months / 12, o.months % 12)
                } else {
                    format!("{} bulan", o.months)
                };
                PlanOpt {
                    key: o.key.into(),
                    label: o.label.into(),
                    months_text: months.into(),
                    interest_text: rp(o.total_interest),
                    payoff_text: o.payoff_label.clone().into(),
                    order_text: if o.order.len() > 1 { format!("Urutan lunas: {}", o.order.join(", ")).into() } else { SharedString::new() },
                    feasible: o.feasible,
                    recommended: i == plan.recommended,
                    path: line_path(&vals, max, len).into(),
                    tint: color(tints[i]),
                }
            })
            .collect(),
    ));
    d.set_plan_message(plan.message.into());
    d.set_plan_extra_text(format!("+{}", rupiah(s.plan_extra)).into());
    Ok(())
}

fn refresh_analytics(ui: &AppWindow, s: &State) -> R<()> {
    let d = ui.global::<D>();
    let a = s.store.analytics(s.span)?;
    d.set_health(a.health as f32);
    d.set_health_label(a.health_label.into());
    d.set_health_tone(if a.health >= 65.0 { "pos" } else if a.health >= 50.0 { "accent" } else if a.health >= 35.0 { "warn" } else { "neg" }.into());
    d.set_health_parts(model(
        a.health_parts
            .iter()
            .map(|p| HealthPart { name: p.name.into(), score: p.score as f32, max: p.max as f32, detail: p.detail.clone().into() })
            .collect(),
    ));
    d.set_an_summary(a.summary.into());
    d.set_an_avg_income(rp(a.avg_income));
    d.set_an_avg_expense(rp(a.avg_expense));
    d.set_an_avg_savings(pct(a.avg_savings_rate).into());
    d.set_an_net_total(rp(a.net_total));
    let fmt_month = |m: &Option<(String, i64)>| match m {
        Some((l, n)) => format!("{l}, {}{}", if *n >= 0 { "+" } else { "" }, compact(*n)),
        None => "-".into(),
    };
    d.set_an_best(fmt_month(&a.best).into());
    d.set_an_worst(fmt_month(&a.worst).into());
    d.set_an_dti(pct(a.debt_to_income).into());
    d.set_an_volatility(pct(a.volatility).into());
    // Start at the first month with data so a new user does not see empty bars.
    let first = a.months.iter().position(|m| m.income > 0 || m.expense > 0).unwrap_or(0).min(a.months.len().saturating_sub(3));
    let shown = &a.months[first..];
    let rows: Vec<(String, i64, i64)> = shown.iter().map(|m| (m.label.clone(), m.income, m.expense)).collect();
    d.set_an_months(model(bars(&rows, true)));

    let rates: Vec<f64> = shown.iter().map(|m| if m.income > 0 { m.savings_rate.clamp(-1.0, 1.0) } else { 0.0 }).collect();
    let lo = rates.iter().cloned().fold(-0.2, f64::min);
    let hi = rates.iter().cloned().fold(0.5, f64::max);
    d.set_an_savings_path(signed_path(&rates, lo, hi).into());
    d.set_an_zero_y((1.0 - ((0.0 - lo) / (hi - lo))) as f32);
    d.set_an_savings_last(pct(a.avg_savings_rate).into());

    d.set_an_trends(model(
        a.categories
            .iter()
            .map(|c| {
                let vals: Vec<f64> = c.values.iter().map(|v| *v as f64).collect();
                let max = vals.iter().cloned().fold(0.0, f64::max) * 1.1;
                let (text, tone) = match c.change {
                    Some(ch) if ch > 0.1 => (format!("+{}", pct(ch)), "warn"),
                    Some(ch) if ch < -0.1 => (format!("-{}", pct(-ch)), "pos"),
                    Some(_) => ("Stabil".to_string(), "neutral"),
                    None => ("Baru".to_string(), "neutral"),
                };
                TrendRow {
                    name: c.name.clone().into(),
                    tint: color(&c.color),
                    path: line_path(&vals, max, vals.len()).into(),
                    total_text: format!("Total {}", rupiah(c.total)).into(),
                    change_text: text.into(),
                    change_tone: tone.into(),
                }
            })
            .collect(),
    ));
    let top_max = a.top_spend.first().map(|t| t.1).unwrap_or(1).max(1) as f32;
    d.set_an_top(model(
        a.top_spend
            .iter()
            .map(|(n, amt, cnt)| TopRow {
                name: n.clone().into(),
                amount_text: rp(*amt),
                count_text: format!("{cnt}x").into(),
                ratio: *amt as f32 / top_max,
            })
            .collect(),
    ));

    // Spending mix donut.
    let shares: Vec<f64> = a.composition.iter().map(|c| c.3).collect();
    let paths = donut_paths(&shares);
    d.set_an_slices(model(
        a.composition
            .iter()
            .zip(paths)
            .map(|(c, path)| Slice {
                name: c.0.clone().into(),
                tint: color(&c.1),
                path: path.into(),
                share_text: pct(c.3).into(),
                amount_text: rp(c.2),
            })
            .collect(),
    ));
    d.set_an_spend_total(compact(a.composition.iter().map(|c| c.2).sum()).into());

    // Weekday habit.
    let wd_max = a.weekday_avg.iter().cloned().max().unwrap_or(0).max(1) as f32;
    let peak = a.weekday_avg.iter().enumerate().max_by_key(|(_, v)| **v).map(|(i, _)| i).unwrap_or(0);
    d.set_an_weekday(model(
        moneycore::util::WEEKDAYS_ID
            .iter()
            .enumerate()
            .map(|(i, n)| BarDatum {
                label: n[..3].to_string().into(),
                a: 0.0,
                b: a.weekday_avg[i] as f32 / wd_max,
                a_text: SharedString::new(),
                b_text: compact(a.weekday_avg[i]).into(),
                current: i == peak && a.weekday_avg[i] > 0,
            })
            .collect(),
    ));
    d.set_an_weekday_peak(if a.weekday_avg[peak] > 0 { format!("Paling boros {}", moneycore::util::WEEKDAYS_ID[peak]).into() } else { SharedString::new() });

    // Total money per month end.
    let pools: Vec<f64> = a.pool_months[first..].iter().map(|v| *v as f64).collect();
    let n = normalise(&pools);
    d.set_an_pool_path(line_path(&n, 1.0, n.len()).into());
    d.set_an_pool_area(area_path(&n, 1.0, n.len()).into());
    let pool_now = a.pool_months.last().copied().unwrap_or(0);
    let pool_change = pool_now - a.pool_months[first];
    d.set_an_pool_now(rp(pool_now));
    d.set_an_pool_change(format!("{}{}", if pool_change >= 0 { "+" } else { "" }, compact(pool_change)).into());
    d.set_an_pool_tone(if pool_change >= 0 { "pos" } else { "neg" }.into());
    d.set_an_month_labels(model(shown.iter().map(|m| SharedString::from(m.label.as_str())).collect()));

    // Budget adherence per finished month.
    d.set_an_adherence(model(
        shown
            .iter()
            .filter_map(|m| m.adherence.map(|x| (m, x)))
            .map(|(m, x)| BarDatum {
                label: m.label.clone().into(),
                a: x as f32,
                b: 0.0,
                a_text: pct(x).into(),
                b_text: SharedString::new(),
                current: false,
            })
            .collect(),
    ));
    Ok(())
}

fn freq_index(f: &str) -> i32 {
    match f {
        "day" => 0,
        "week" => 1,
        _ => 2,
    }
}

fn long_date(d: NaiveDate) -> String {
    format!("{} {}", date_label(d), d.year())
}

fn goal_row(v: &moneycore::goals::GoalView, today: NaiveDate) -> GoalRow {
    let g = &v.goal;
    let (status_text, tone) = match v.status {
        "done" => ("Tercapai", "pos"),
        "ahead" => ("Lebih cepat", "pos"),
        "on_track" => ("Sesuai jalur", "pos"),
        "behind" => ("Tertinggal", "warn"),
        _ => ("Lewat tenggat", "neg"),
    };
    let deadline = parse_date(&g.deadline).unwrap_or(today);
    GoalRow {
        id: g.id as i32,
        name: g.name.clone().into(),
        icon: g.icon.clone().into(),
        tint: color(&g.color),
        saved: v.saved as f32,
        saved_text: rp(v.saved),
        target_text: rp(g.target),
        remaining_text: rp(v.remaining),
        progress: v.progress as f32,
        deadline_text: long_date(deadline).into(),
        days_left_text: (if v.status == "done" { "tercapai".to_string() } else if v.days_left == 0 { "tenggat hari ini".into() } else { format!("{} hari lagi", v.days_left) }).into(),
        per_day_text: rp(v.per_day),
        per_week_text: rp(v.per_week),
        per_month_text: rp(v.per_month),
        freq: freq_index(&g.freq),
        status: v.status.into(),
        status_text: status_text.into(),
        status_tone: tone.into(),
        projected_text: (match (v.status, v.projected) {
            ("done", _) => "Selesai".to_string(),
            (_, Some(d)) => month_label(d.year(), d.month()),
            _ => "Belum ada setoran".into(),
        })
        .into(),
        raw_target: raw_amount(g.target),
        raw_deadline: g.deadline.clone().into(),
    }
}

fn refresh_goals(ui: &AppWindow, s: &mut State) -> R<()> {
    let d = ui.global::<D>();
    let t = today();
    let views: Vec<_> = s.store.goal_views()?.into_iter().filter(|v| !v.goal.archived).collect();
    if !views.iter().any(|v| v.goal.id == s.goal_sel) {
        s.goal_sel = views.first().map(|v| v.goal.id).unwrap_or(0);
    }
    d.set_goals(model(views.iter().map(|v| goal_row(v, t)).collect()));
    let Some(v) = views.iter().find(|v| v.goal.id == s.goal_sel) else {
        d.set_has_goal(false);
        return Ok(());
    };
    d.set_has_goal(true);
    d.set_goal_current(goal_row(v, t));
    d.set_goal_entries(model(
        v.entries
            .iter()
            .map(|e| GoalEntryRow {
                id: e.id as i32,
                amount_text: format!("{}{}", if e.amount > 0 { "+" } else { "-" }, rupiah(e.amount.abs())).into(),
                positive: e.amount > 0,
                date_text: friendly_date(&e.date, t).into(),
                note: e.note.clone().into(),
            })
            .collect(),
    ));
    // Time runs left to right from start to deadline; height is the target.
    let start = parse_date(&v.goal.start).unwrap_or(t);
    let end = parse_date(&v.goal.deadline).unwrap_or(t);
    let span = (end - start).num_days().max(1) as f64;
    let target = v.goal.target.max(1) as f64;
    let xy = |d: NaiveDate, val: i64| {
        let x = ((d - start).num_days() as f64 / span).clamp(0.0, 1.0) * VW;
        let y = VH - (val as f64 / target).clamp(0.0, 1.0) * (VH - 3.0) - 1.5;
        format!("{x:.1} {y:.1}")
    };
    let mut path = String::new();
    let mut last = 0;
    for (i, (day, val)) in v.series.iter().enumerate() {
        // step shape: flat until a deposit lands, then up
        if i > 0 {
            path.push_str(&format!(" L {}", xy(*day, last)));
        }
        path.push_str(&format!("{}{}", if i == 0 { "M " } else { " L " }, xy(*day, *val)));
        last = *val;
    }
    let now = t.min(end).max(start);
    path.push_str(&format!(" L {}", xy(now, last)));
    let x_now = ((now - start).num_days() as f64 / span).clamp(0.0, 1.0) * VW;
    d.set_goal_area(format!("{path} L {x_now:.1} {VH} L 0 {VH} Z").into());
    d.set_goal_path(path.into());
    // Projection: keep the pace so far until the deadline.
    let elapsed = (now - start).num_days().max(1) as f64;
    let rate = v.saved as f64 / elapsed;
    let at_deadline = (v.saved as f64 + rate * (end - now).num_days() as f64).round() as i64;
    d.set_goal_proj(if v.saved > 0 && v.status != "done" { format!("M {} L {}", xy(now, v.saved), xy(end, at_deadline.min(v.goal.target))) } else { String::new() }.into());
    let (proj_text, proj_tone) = if v.status == "done" {
        ("Target tercapai".to_string(), "pos")
    } else if v.saved == 0 {
        ("Mulai setor untuk melihat proyeksi".to_string(), "pos")
    } else if at_deadline >= v.goal.target {
        (format!("Dengan ritme sekarang tercapai {}", v.projected.map(|d| long_date(d)).unwrap_or_default()), "pos")
    } else {
        (format!("Dengan ritme sekarang terkumpul {} di tenggat, kurang {}", rupiah(at_deadline), rupiah(v.goal.target - at_deadline)), if at_deadline as f64 >= v.goal.target as f64 * 0.8 { "warn" } else { "neg" })
    };
    d.set_goal_proj_text(proj_text.into());
    d.set_goal_proj_tone(proj_tone.into());
    d.set_goal_ymax(compact(v.goal.target).into());
    d.set_goal_ymid(compact(v.goal.target / 2).into());
    d.set_goal_ideal(format!("M 0 {VH} L {VW} 0").into());
    d.set_goal_today_x((((t - start).num_days() as f64 / span).clamp(0.0, 1.0)) as f32);
    d.set_goal_start_text(long_date(start).into());
    d.set_goal_end_text(long_date(end).into());
    Ok(())
}

fn shift_month(iso: &str, delta: i32) -> String {
    let base = parse_date(iso).unwrap_or_else(|_| today());
    let (y, m) = add_months(base.year(), base.month(), delta);
    // deadlines sit on the last day of their month
    clamp_day(y, m, 31).to_string()
}

// ---------------------------------------------------------------- feedback

fn toast(ui: &AppWindow, s: &mut State, text: &str, undo: Option<Undo>) {
    let d = ui.global::<D>();
    d.set_toast_undo(undo.is_some());
    s.undo = undo;
    d.set_toast(text.into());
    d.set_toast_show(true);
    let weak = ui.as_weak();
    s.toast_timer.start(TimerMode::SingleShot, Duration::from_millis(4500), move || {
        if let Some(ui) = weak.upgrade() {
            ui.global::<D>().set_toast_show(false);
        }
    });
}

fn parsed_view(s: &State, ui: &AppWindow) -> R<(Parsed, Option<Transaction>)> {
    let d = ui.global::<D>();
    let text = d.get_quick_text().to_string();
    let cats = s.store.categories()?;
    let kind = if d.get_quick_kind() == 0 { "income" } else { "expense" };
    let mut p = s.store.parse(&text, Some(kind))?;
    let over = d.get_quick_cat();
    if over >= 0 {
        if let Some(c) = cats.iter().find(|c| c.id == over as i64) {
            p.category_id = Some(c.id);
            p.kind = c.kind.clone();
        }
    }
    let acc_over = d.get_quick_acc();
    if acc_over >= 0 {
        p.account_id = Some(acc_over as i64);
    }
    let accs = s.store.accounts()?;
    let acc = accs.iter().find(|a| Some(a.id) == p.account_id);
    let c: Option<&Category> = cats.iter().find(|c| Some(c.id) == p.category_id);
    let view = Parsed {
        valid: p.amount.is_some() && !text.trim().is_empty(),
        kind: p.kind.clone().into(),
        amount_text: p.amount.map(rp).unwrap_or_default(),
        amount_raw: p.amount.map(raw_amount).unwrap_or_default(),
        assumed: p.assumed_thousands,
        date: p.date.to_string().into(),
        date_text: p.date_label.clone().into(),
        note: p.note.clone().into(),
        cat_id: c.map(|c| c.id as i32).unwrap_or(-1),
        cat_name: c.map(|c| c.name.clone()).unwrap_or_default().into(),
        icon: c.map(|c| c.icon.clone()).unwrap_or_else(|| "dots".into()).into(),
        tint: c.map(|c| color(&c.color)).unwrap_or(color("#64748B")),
        confidence: if over >= 0 { 1.0 } else { p.confidence as f32 },
        acc_id: acc.map(|a| a.id as i32).unwrap_or(-1),
        acc_name: acc.map(|a| a.name.clone()).unwrap_or_default().into(),
    };
    let tx = p.amount.map(|amount| Transaction {
        id: 0,
        kind: p.kind.clone(),
        amount,
        category_id: p.category_id,
        date: p.date.to_string(),
        note: p.note.clone(),
        recurring_id: None,
        debt_id: None,
        created_at: String::new(),
        account_id: p.account_id,
    });
    Ok((view, tx))
}

fn default_acc(s: &State) -> i32 {
    s.store.default_account().ok().flatten().map(|a| a as i32).unwrap_or(-1)
}

fn shift_date(iso: &str, delta: i32) -> String {
    let t = today();
    match parse_date(iso) {
        Ok(d) => plus_days(d, delta as i64).min(t).to_string(),
        Err(_) => t.to_string(),
    }
}

// ---------------------------------------------------------------- main

#[derive(Clone)]
struct Ctx {
    ui: slint::Weak<AppWindow>,
    st: Rc<RefCell<State>>,
}

impl Ctx {
    fn run<O: Default>(&self, f: impl FnOnce(&AppWindow, &mut State) -> O) -> O {
        let Some(ui) = self.ui.upgrade() else { return O::default() };
        let Ok(mut s) = self.st.try_borrow_mut() else { return O::default() };
        f(&ui, &mut s)
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = db_path();
    let store = Store::open(&path)?;
    let ui = AppWindow::new()?;
    let t = today();
    let st = Rc::new(RefCell::new(State {
        store,
        y: t.year(),
        m: t.month(),
        span: 12,
        period: 2,
        period_offset: 0,
        goal_sel: 0,
        plan_extra: 0,
        undo: None,
        toast_timer: Timer::default(),
        db_path: path.clone(),
    }));
    let cx = Ctx { ui: ui.as_weak(), st: st.clone() };

    // Formatting helpers used by bindings.
    let api = ui.global::<Api>();
    api.on_rp(|v| rp(v.round() as i64));
    api.on_rp_compact(|v| compact(v.round() as i64).into());
    api.on_date_label(|iso| friendly_date(&iso, today()).into());
    api.on_shift_date(|iso, delta| shift_date(&iso, delta).into());
    api.on_today(|| today().to_string().into());
    api.on_fit(|p, w, h| fit_path(&p, w, h).into());
    api.on_shift_month(|iso, delta| shift_month(&iso, delta).into());
    api.on_format_amount(|t| moneycore::parse::format_amount_input(&t).into());
    api.on_amount_suffix(|t| {
        let shorthand = t.chars().any(|c| c.is_alphabetic() && c != 'R' && c != 'p') || t.contains(',');
        match (shorthand, amount_from_text(&t)) {
            (true, Some(v)) => format!("= {}", raw_amount(v)).into(),
            _ => "Rp".into(),
        }
    });
    api.on_month_text(|iso| parse_date(&iso).map(|d| month_label(d.year(), d.month())).unwrap_or_default().into());

    let d = ui.global::<D>();
    d.set_font(std::env::var("DUITRACK_FONT").unwrap_or_else(|_| ".SF NS".into()).into());
    {
        let s = st.borrow();
        let mode: i32 = s.store.meta("motion").ok().flatten().and_then(|v| v.parse().ok()).unwrap_or(0);
        d.set_motion_mode(mode);
        apply_motion(&ui, mode);
        let theme: i32 = s.store.meta("theme").ok().flatten().and_then(|v| v.parse().ok()).unwrap_or(0);
        apply_theme(&ui, theme);
        d.set_db_path(s.db_path.clone().into());
    }

    // navigation
    let c = cx.clone();
    d.on_month_shift(move |delta| c.run(|ui, s| {
        (s.y, s.m) = add_months(s.y, s.m, delta);
        refresh_all(ui, s);
    }));
    let c = cx.clone();
    d.on_month_today(move || c.run(|ui, s| {
        let t = today();
        (s.y, s.m) = (t.year(), t.month());
        refresh_all(ui, s);
    }));

    // period dashboard
    let c = cx.clone();
    d.on_period_changed(move |i| c.run(|ui, s| {
        s.period = i;
        s.period_offset = 0;
        let _ = refresh_period(ui, s);
    }));
    let c = cx.clone();
    d.on_period_shift(move |delta| c.run(|ui, s| {
        s.period_offset = (s.period_offset + delta).min(0);
        let _ = refresh_period(ui, s);
    }));
    let weak = ui.as_weak();
    d.on_do_action(move |kind, id| {
        // Dispatches to other callbacks, so it must not hold the state borrow.
        let Some(ui) = weak.upgrade() else { return };
        let d = ui.global::<D>();
        match kind.as_str() {
            "rec" => d.invoke_rec_post(id),
            "debt" => d.invoke_pay_open(id),
            "transfer" => d.invoke_trf_new(),
            _ => d.set_page(5),
        }
    });

    // quick add
    let c = cx.clone();
    d.on_quick_parse(move || c.run(|ui, s| {
        let d = ui.global::<D>();
        let t = d.get_quick_text();
        match t.trim_start().chars().next() {
            Some('+') => d.set_quick_kind(0),
            Some('-') => d.set_quick_kind(1),
            _ => {}
        }
        if let Ok((view, _)) = parsed_view(s, ui) {
            ui.global::<D>().set_parsed(view);
        }
    }));
    let c = cx.clone();
    d.on_quick_save(move || c.run(|ui, s| {
        let Ok((_, Some(tx))) = parsed_view(s, ui) else { return false };
        match s.store.add_tx(&tx) {
            Ok(id) => {
                let d = ui.global::<D>();
                d.set_quick_text(SharedString::new());
                d.set_quick_cat(-1);
                d.set_quick_acc(-1);
                d.set_parsed(Parsed::default());
                let label = if tx.note.is_empty() { "Transaksi".to_string() } else { tx.note.clone() };
                refresh_all(ui, s);
                toast(ui, s, &format!("Tersimpan: {} {}", label, rupiah(tx.amount)), Some(Undo::Added(id)));
                true
            }
            Err(e) => {
                toast(ui, s, &e, None);
                false
            }
        }
    }));

    // transactions
    let c = cx.clone();
    d.on_tx_query(move || c.run(|ui, s| {
        let _ = refresh_tx(ui, s);
    }));
    let c = cx.clone();
    d.on_tx_open(move |row| c.run(|ui, _| {
        let d = ui.global::<D>();
        d.set_f_id(row.id);
        d.set_f_kind(row.kind.clone());
        d.set_f_amount(row.amount_raw.clone());
        d.set_f_cat(row.cat_id);
        d.set_f_acc(row.acc_id);
        d.set_f_date(row.date.clone());
        d.set_f_note(row.note.clone());
        d.set_f_error(SharedString::new());
        d.set_tx_sheet(true);
    }));
    let c = cx.clone();
    d.on_tx_new(move || c.run(|ui, s| {
        let d = ui.global::<D>();
        d.set_f_acc(default_acc(s));
        d.set_f_id(0);
        d.set_f_kind("expense".into());
        d.set_f_amount(SharedString::new());
        d.set_f_cat(d.get_exp_cats().row_data(0).map(|c| c.id).unwrap_or(-1));
        d.set_f_date(today().to_string().into());
        d.set_f_note(SharedString::new());
        d.set_f_error(SharedString::new());
        d.set_tx_sheet(true);
    }));
    let c = cx.clone();
    d.on_tx_save(move || c.run(|ui, s| {
        let d = ui.global::<D>();
        let Some(amount) = amount_from_text(&d.get_f_amount()) else {
            d.set_f_error("Nominal belum valid. Contoh: 25000, 25rb, 1,5jt".into());
            return;
        };
        let tx = Transaction {
            id: d.get_f_id() as i64,
            kind: d.get_f_kind().to_string(),
            amount,
            category_id: (d.get_f_cat() >= 0).then(|| d.get_f_cat() as i64),
            date: d.get_f_date().to_string(),
            note: d.get_f_note().to_string(),
            recurring_id: None,
            debt_id: None,
            created_at: String::new(),
            account_id: (d.get_f_acc() >= 0).then(|| d.get_f_acc() as i64),
        };
        let r = if tx.id == 0 { s.store.add_tx(&tx).map(|_| ()) } else { s.store.update_tx(&tx) };
        match r {
            Ok(()) => {
                d.set_tx_sheet(false);
                refresh_all(ui, s);
                toast(ui, s, "Transaksi disimpan", None);
            }
            Err(e) => d.set_f_error(e.into()),
        }
    }));
    let c = cx.clone();
    d.on_tx_delete(move |id| c.run(|ui, s| {
        match s.store.delete_tx(id as i64) {
            Ok(old) => {
                ui.global::<D>().set_tx_sheet(false);
                refresh_all(ui, s);
                toast(ui, s, "Transaksi dihapus", old.map(Undo::Deleted));
            }
            Err(e) => ui.global::<D>().set_f_error(e.into()),
        }
    }));
    let c = cx.clone();
    d.on_undo(move || c.run(|ui, s| {
        let r = match s.undo.take() {
            Some(Undo::Deleted(t)) => s.store.restore_tx(&t),
            Some(Undo::Added(id)) => s.store.delete_tx(id).map(|_| ()),
            None => Ok(()),
        };
        refresh_all(ui, s);
        match r {
            Ok(()) => toast(ui, s, "Diurungkan", None),
            Err(e) => toast(ui, s, &e, None),
        }
    }));

    // budget
    let c = cx.clone();
    d.on_budget_set(move |cat, text| c.run(|ui, s| {
        let amount = if text.trim().is_empty() { Some(0) } else { amount_from_text(&text) };
        match amount {
            Some(a) => match s.store.set_budget(s.y, s.m, cat as i64, a) {
                Ok(()) => {
                    refresh_all(ui, s);
                    toast(ui, s, &format!("Anggaran {}", if a == 0 { "dikosongkan".into() } else { rupiah(a) }), None);
                }
                Err(e) => toast(ui, s, &e, None),
            },
            None => toast(ui, s, "Nominal anggaran belum valid", None),
        }
    }));
    let c = cx.clone();
    d.on_budget_copy_prev(move || c.run(|ui, s| {
        match s.store.copy_budget_from_prev(s.y, s.m) {
            Ok(0) => toast(ui, s, "Bulan lalu belum punya anggaran", None),
            Ok(n) => {
                refresh_all(ui, s);
                toast(ui, s, &format!("{n} kategori disalin dari bulan lalu"), None);
            }
            Err(e) => toast(ui, s, &e, None),
        }
    }));
    let c = cx.clone();
    d.on_budget_apply_suggest(move || c.run(|ui, s| {
        match s.store.apply_suggestions(s.y, s.m) {
            Ok(n) => {
                refresh_all(ui, s);
                toast(ui, s, &format!("Saran diterapkan ke {n} kategori"), None);
            }
            Err(e) => toast(ui, s, &e, None),
        }
    }));

    // recurring
    let c = cx.clone();
    d.on_rec_new(move |kind| c.run(|ui, s| {
        let d = ui.global::<D>();
        d.set_r_acc(default_acc(s));
        let cats = if kind == "income" { d.get_inc_cats() } else { d.get_exp_cats() };
        d.set_r_id(0);
        d.set_r_kind(kind);
        d.set_r_name(SharedString::new());
        d.set_r_amount(SharedString::new());
        d.set_r_cat(cats.row_data(0).map(|c| c.id).unwrap_or(-1));
        d.set_r_day(today().day().to_string().into());
        d.set_r_error(SharedString::new());
        d.set_rec_sheet(true);
    }));
    let c = cx.clone();
    d.on_rec_open(move |r| c.run(|ui, _| {
        let d = ui.global::<D>();
        d.set_r_id(r.id);
        d.set_r_kind(r.kind.clone());
        d.set_r_name(r.name.clone());
        d.set_r_amount(r.amount_raw.clone());
        d.set_r_cat(r.cat_id);
        d.set_r_acc(r.acc_id);
        d.set_r_day(r.day.to_string().into());
        d.set_r_error(SharedString::new());
        d.set_rec_sheet(true);
    }));
    let c = cx.clone();
    d.on_rec_save(move || c.run(|ui, s| {
        let d = ui.global::<D>();
        let Some(amount) = amount_from_text(&d.get_r_amount()) else {
            d.set_r_error("Nominal belum valid".into());
            return;
        };
        let Ok(day) = d.get_r_day().trim().parse::<u32>() else {
            d.set_r_error("Tanggal harus angka 1 sampai 31".into());
            return;
        };
        let r = Recurring {
            id: d.get_r_id() as i64,
            kind: d.get_r_kind().to_string(),
            name: d.get_r_name().to_string(),
            amount,
            category_id: (d.get_r_cat() >= 0).then(|| d.get_r_cat() as i64),
            day,
            active: true,
            account_id: (d.get_r_acc() >= 0).then(|| d.get_r_acc() as i64),
        };
        match s.store.save_recurring(&r) {
            Ok(_) => {
                d.set_rec_sheet(false);
                refresh_all(ui, s);
                toast(ui, s, "Transaksi rutin disimpan", None);
            }
            Err(e) => d.set_r_error(e.into()),
        }
    }));
    let c = cx.clone();
    d.on_rec_delete(move |id| c.run(|ui, s| {
        let _ = s.store.delete_recurring(id as i64);
        ui.global::<D>().set_rec_sheet(false);
        refresh_all(ui, s);
        toast(ui, s, "Transaksi rutin dihapus", None);
    }));
    let c = cx.clone();
    d.on_rec_post(move |id| c.run(|ui, s| {
        match s.store.post_recurring(id as i64, s.y, s.m) {
            Ok(tx) => {
                refresh_all(ui, s);
                toast(ui, s, "Tercatat", Some(Undo::Added(tx)));
            }
            Err(e) => toast(ui, s, &e, None),
        }
    }));

    // debts
    let c = cx.clone();
    d.on_debt_new(move || c.run(|ui, _| {
        let d = ui.global::<D>();
        d.set_g_id(0);
        for f in [D::set_g_name, D::set_g_lender, D::set_g_start, D::set_g_min, D::set_g_error] {
            f(&d, SharedString::new());
        }
        d.set_g_apr("0".into());
        d.set_g_due(today().day().to_string().into());
        d.set_g_inst(true);
        d.set_g_tenor("6".into());
        d.set_g_paid("0".into());
        d.set_debt_sheet(true);
    }));
    let c = cx.clone();
    d.on_debt_edit(move |id| c.run(|ui, _| {
        let d = ui.global::<D>();
        let Some(row) = d.get_debts().iter().find(|r| r.id == id) else { return };
        d.set_g_id(row.id);
        d.set_g_name(row.name.clone());
        d.set_g_lender(row.raw_lender.clone());
        d.set_g_start(row.raw_start.clone());
        d.set_g_apr(row.raw_apr.clone());
        d.set_g_min(row.raw_min.clone());
        d.set_g_due(row.raw_due.clone());
        d.set_g_inst(row.is_inst);
        d.set_g_tenor(row.tenor.to_string().into());
        d.set_g_paid(row.paid_n.to_string().into());
        d.set_g_error(SharedString::new());
        d.set_debt_detail(false);
        d.set_debt_sheet(true);
    }));
    let c = cx.clone();
    d.on_debt_save(move || c.run(|ui, s| {
        let d = ui.global::<D>();
        let err = |m: &str| d.set_g_error(m.into());
        let Ok(due) = d.get_g_due().trim().parse::<u32>() else { return err("Tanggal jatuh tempo 1 sampai 31") };
        let id = d.get_g_id() as i64;
        let existing = s.store.debt_views().ok().and_then(|v| v.into_iter().find(|v| v.debt.id == id));
        let t = today();
        let (debt, paid_before) = if d.get_g_inst() {
            let Some(per) = amount_from_text(&d.get_g_min()) else { return err("Cicilan per bulan belum valid") };
            let Ok(tenor) = d.get_g_tenor().trim().parse::<u32>() else { return err("Tenor harus angka bulan") };
            let paid_before = if id == 0 { d.get_g_paid().trim().parse::<u32>().unwrap_or(0) } else { 0 };
            if tenor == 0 || paid_before >= tenor {
                return err("Sudah dibayar harus lebih kecil dari tenor");
            }
            // First unpaid installment lands in the current month.
            let (sy, sm) = add_months(t.year(), t.month(), -(paid_before as i32));
            let start_date = existing.as_ref().map(|v| v.debt.start_date.clone()).unwrap_or_else(|| month_start(sy, sm).to_string());
            (
                Debt {
                    id,
                    name: d.get_g_name().to_string(),
                    lender: d.get_g_lender().to_string(),
                    start_balance: per * tenor as i64,
                    apr: 0.0,
                    min_payment: per,
                    due_day: due,
                    start_date,
                    note: String::new(),
                    closed: false,
                    tenor: Some(tenor),
                },
                paid_before,
            )
        } else {
            let Some(bal) = amount_from_text(&d.get_g_start()) else { return err("Sisa hutang belum valid") };
            let min = if d.get_g_min().trim().is_empty() { Some(0) } else { amount_from_text(&d.get_g_min()) };
            let Some(min) = min else { return err("Bayar minimum belum valid") };
            let apr_text = d.get_g_apr().trim().replace(',', ".");
            let Ok(apr) = (if apr_text.is_empty() { Ok(0.0) } else { apr_text.parse::<f64>() }) else { return err("Bunga harus angka, misal 21") };
            // Editing a flexible debt sets what is left now, so keep paid history intact.
            let start_balance = match &existing {
                Some(v) if v.debt.tenor.is_none() => (bal + v.paid - v.charges).max(1),
                _ => bal,
            };
            (
                Debt {
                    id,
                    name: d.get_g_name().to_string(),
                    lender: d.get_g_lender().to_string(),
                    start_balance,
                    apr,
                    min_payment: min,
                    due_day: due,
                    start_date: existing.as_ref().map(|v| v.debt.start_date.clone()).unwrap_or_else(|| t.to_string()),
                    note: String::new(),
                    closed: false,
                    tenor: None,
                },
                0,
            )
        };
        match s.store.save_debt(&debt) {
            Ok(new_id) => {
                // Installments already paid before tracking: history only, not this month's spending.
                if let Ok(start) = parse_date(&debt.start_date) {
                    for n in 0..paid_before {
                        let (y, m) = add_months(start.year(), start.month(), n as i32);
                        let p = DebtPayment {
                            id: 0,
                            debt_id: new_id,
                            kind: "payment".into(),
                            amount: debt.min_payment,
                            date: clamp_day(y, m, due).min(t).to_string(),
                            note: "Sebelum dicatat".into(),
                            tx_id: None,
                        };
                        let _ = s.store.add_debt_payment(&p, false, None);
                    }
                }
                d.set_debt_sheet(false);
                refresh_all(ui, s);
                toast(ui, s, "Hutang disimpan", None);
            }
            Err(e) => d.set_g_error(e.into()),
        }
    }));
    let c = cx.clone();
    d.on_debt_delete(move |id| c.run(|ui, s| {
        let _ = s.store.delete_debt(id as i64);
        let d = ui.global::<D>();
        d.set_debt_sheet(false);
        d.set_debt_detail(false);
        refresh_all(ui, s);
        toast(ui, s, "Hutang dihapus", None);
    }));
    let c = cx.clone();
    d.on_debt_open(move |id| c.run(|ui, s| {
        let d = ui.global::<D>();
        d.set_debt_selected(id);
        let _ = refresh_debts(ui, s);
        d.set_debt_detail(true);
    }));
    let c = cx.clone();
    d.on_pay_open(move |id| c.run(|ui, s| {
        let d = ui.global::<D>();
        d.set_p_acc(default_acc(s));
        let Some(row) = d.get_debts().iter().find(|r| r.id == id) else { return };
        d.set_p_debt(id);
        d.set_p_debt_name(row.name.clone());
        d.set_p_kind("payment".into());
        d.set_p_amount(row.raw_min.clone());
        d.set_p_date(today().to_string().into());
        d.set_p_note(SharedString::new());
        d.set_p_record(true);
        d.set_p_error(SharedString::new());
        d.set_pay_sheet(true);
    }));
    let c = cx.clone();
    d.on_pay_save(move || c.run(|ui, s| {
        let d = ui.global::<D>();
        let Some(amount) = amount_from_text(&d.get_p_amount()) else {
            d.set_p_error("Nominal belum valid".into());
            return;
        };
        let p = DebtPayment {
            id: 0,
            debt_id: d.get_p_debt() as i64,
            kind: d.get_p_kind().to_string(),
            amount,
            date: d.get_p_date().to_string(),
            note: d.get_p_note().to_string(),
            tx_id: None,
        };
        match s.store.add_debt_payment(&p, d.get_p_record(), (d.get_p_acc() >= 0).then(|| d.get_p_acc() as i64)) {
            Ok(_) => {
                d.set_pay_sheet(false);
                refresh_all(ui, s);
                let msg = if p.kind == "payment" { format!("Pembayaran {} dicatat", rupiah(amount)) } else { format!("Tagihan {} ditambahkan", rupiah(amount)) };
                toast(ui, s, &msg, None);
            }
            Err(e) => d.set_p_error(e.into()),
        }
    }));
    let c = cx.clone();
    d.on_payment_delete(move |id| c.run(|ui, s| {
        let _ = s.store.delete_debt_payment(id as i64);
        refresh_all(ui, s);
        toast(ui, s, "Riwayat dihapus", None);
    }));
    let c = cx.clone();
    d.on_plan_changed(move |v| c.run(|ui, s| {
        let stepped = ((v as f64 / 50_000.0).round() * 50_000.0) as i64;
        if stepped != s.plan_extra {
            s.plan_extra = stepped;
            let _ = refresh_plan(ui, s);
        }
    }));

    // analytics
    let c = cx.clone();
    d.on_an_span_changed(move |i| c.run(|ui, s| {
        s.span = if i == 0 { 6 } else { 12 };
        let _ = refresh_analytics(ui, s);
    }));

    // settings
    let c = cx.clone();
    d.on_set_motion(move |mode| c.run(|ui, s| {
        let _ = s.store.set_meta("motion", &mode.to_string());
        apply_motion(ui, mode);
    }));
    let c = cx.clone();
    d.on_export_data(move || c.run(|ui, s| {
        let name = format!("duitrack-cadangan-{}.json", today());
        let Some(file) = rfd::FileDialog::new().set_file_name(&name).add_filter("JSON", &["json"]).save_file() else { return };
        let r = s.store.export_json().and_then(|j| std::fs::write(&file, j).map_err(|e| e.to_string()));
        match r {
            Ok(()) => toast(ui, s, "Cadangan tersimpan", None),
            Err(e) => toast(ui, s, &format!("Gagal ekspor: {e}"), None),
        }
    }));
    let c = cx.clone();
    d.on_import_data(move || c.run(|ui, s| {
        let Some(file) = rfd::FileDialog::new().add_filter("JSON", &["json"]).pick_file() else { return };
        let r = std::fs::read_to_string(&file).map_err(|e| e.to_string()).and_then(|j| {
            if j.len() > 50 * 1024 * 1024 {
                return Err("file terlalu besar".into());
            }
            s.store.import_json(&j)
        });
        refresh_all(ui, s);
        match r {
            Ok(()) => toast(ui, s, "Data dipulihkan dari cadangan", None),
            Err(e) => toast(ui, s, &format!("Gagal impor: {e}"), None),
        }
    }));
    let c = cx.clone();
    d.on_seed_demo(move || c.run(|ui, s| {
        if !s.store.is_empty().unwrap_or(false) {
            return toast(ui, s, "Data contoh hanya untuk database kosong", None);
        }
        match s.store.seed_demo() {
            Ok(()) => {
                refresh_all(ui, s);
                toast(ui, s, "Data contoh dimuat", None);
            }
            Err(e) => toast(ui, s, &e, None),
        }
    }));
    let c = cx.clone();
    d.on_reset_all(move || c.run(|ui, s| {
        let r = s.store.reset_all();
        ui.global::<D>().set_reset_sheet(false);
        refresh_all(ui, s);
        match r {
            Ok(()) => toast(ui, s, "Semua data dihapus", None),
            Err(e) => toast(ui, s, &e, None),
        }
    }));

    // money sources
    let c = cx.clone();
    d.on_acc_new(move || c.run(|ui, _| {
        let d = ui.global::<D>();
        d.set_a_id(0);
        d.set_a_name(SharedString::new());
        d.set_a_kind("bank".into());
        d.set_a_balance(SharedString::new());
        d.set_a_keywords(SharedString::new());
        d.set_a_default(false);
        d.set_a_color(6);
        d.set_a_error(SharedString::new());
        d.set_acc_sheet(true);
    }));
    let c = cx.clone();
    d.on_acc_open(move |a| c.run(|ui, _| {
        let d = ui.global::<D>();
        d.set_a_id(a.id);
        d.set_a_name(a.name.clone());
        d.set_a_kind(a.kind.clone());
        d.set_a_balance(a.raw_balance.clone());
        d.set_a_keywords(a.keywords.clone());
        d.set_a_default(a.is_default);
        d.set_a_color(PALETTE.iter().position(|c| color(c) == a.tint).unwrap_or(0) as i32);
        d.set_a_error(SharedString::new());
        d.set_acc_sheet(true);
    }));
    let c = cx.clone();
    d.on_acc_save(move || c.run(|ui, s| {
        let d = ui.global::<D>();
        let text = d.get_a_balance();
        let target = if text.trim().is_empty() {
            0
        } else if let Some(neg) = text.trim().strip_prefix('-') {
            match amount_from_text(neg) { Some(v) => -v, None => return d.set_a_error("Saldo belum valid".into()) }
        } else {
            match amount_from_text(&text) { Some(v) => v, None => return d.set_a_error("Saldo belum valid".into()) }
        };
        let id = d.get_a_id() as i64;
        let accs = s.store.accounts().unwrap_or_default();
        let existing = accs.iter().find(|a| a.id == id);
        // "Saldo sekarang" is what the user sees; store the opening that produces it.
        let flows = existing.map(|a| s.store.balances_now().ok().and_then(|b| b.get(&a.id).copied()).unwrap_or(0) - a.opening).unwrap_or(0);
        let acc = Account {
            id,
            name: d.get_a_name().to_string(),
            kind: d.get_a_kind().to_string(),
            color: PALETTE[(d.get_a_color().max(0) as usize).min(PALETTE.len() - 1)].into(),
            opening: target - flows,
            keywords: d.get_a_keywords().split(',').map(|k| k.trim().to_lowercase()).filter(|k| !k.is_empty()).collect::<Vec<_>>().join(","),
            archived: false,
            sort: existing.map(|a| a.sort).unwrap_or(0),
        };
        match s.store.save_account(&acc) {
            Ok(new_id) => {
                if d.get_a_default() {
                    let _ = s.store.set_default_account(new_id);
                }
                d.set_acc_sheet(false);
                refresh_all(ui, s);
                toast(ui, s, "Sumber dana disimpan", None);
            }
            Err(e) => d.set_a_error(e.into()),
        }
    }));
    let c = cx.clone();
    d.on_acc_archive(move |id| c.run(|ui, s| {
        let d = ui.global::<D>();
        let accs = s.store.accounts().unwrap_or_default();
        if accs.iter().filter(|a| !a.archived).count() <= 1 {
            return d.set_a_error("Minimal satu sumber dana".into());
        }
        let Some(mut a) = accs.into_iter().find(|a| a.id == id as i64) else { return };
        a.archived = true;
        let _ = s.store.save_account(&a);
        d.set_acc_sheet(false);
        refresh_all(ui, s);
        toast(ui, s, "Diarsipkan, riwayat tetap aman", None);
    }));
    let c = cx.clone();
    d.on_trf_new(move || c.run(|ui, s| {
        let d = ui.global::<D>();
        let opts = d.get_acc_opts();
        let from = default_acc(s);
        let to = opts.iter().map(|o| o.id).find(|i| *i != from).unwrap_or(-1);
        d.set_t_from(from);
        d.set_t_to(to);
        d.set_t_amount(SharedString::new());
        d.set_t_date(today().to_string().into());
        d.set_t_note(SharedString::new());
        d.set_t_error(SharedString::new());
        d.set_trf_sheet(true);
    }));
    let c = cx.clone();
    d.on_trf_save(move || c.run(|ui, s| {
        let d = ui.global::<D>();
        let Some(amount) = amount_from_text(&d.get_t_amount()) else { return d.set_t_error("Nominal belum valid".into()) };
        let t = Transfer {
            id: 0,
            from_id: d.get_t_from() as i64,
            to_id: d.get_t_to() as i64,
            amount,
            date: d.get_t_date().to_string(),
            note: d.get_t_note().to_string(),
        };
        match s.store.add_transfer(&t) {
            Ok(_) => {
                d.set_trf_sheet(false);
                refresh_all(ui, s);
                toast(ui, s, &format!("{} dipindahkan", rupiah(amount)), None);
            }
            Err(e) => d.set_t_error(e.into()),
        }
    }));
    let c = cx.clone();
    d.on_trf_delete(move |id| c.run(|ui, s| {
        let _ = s.store.delete_transfer(id as i64);
        refresh_all(ui, s);
        toast(ui, s, "Transfer dihapus", None);
    }));
    let c = cx.clone();
    d.on_set_theme(move |mode| c.run(|ui, s| {
        let _ = s.store.set_meta("theme", &mode.to_string());
        apply_theme(ui, mode);
    }));

    // savings plans
    let c = cx.clone();
    d.on_goal_select(move |id| c.run(|ui, s| {
        s.goal_sel = id as i64;
        let _ = refresh_goals(ui, s);
    }));
    let c = cx.clone();
    d.on_goal_freq(move |i| c.run(|ui, s| {
        let Some(mut g) = s.store.goals().ok().and_then(|gs| gs.into_iter().find(|g| g.id == s.goal_sel)) else { return };
        g.freq = ["day", "week", "month"][i.clamp(0, 2) as usize].into();
        let _ = s.store.save_goal(&g);
        let _ = refresh_goals(ui, s);
    }));
    let c = cx.clone();
    d.on_goal_new(move || c.run(|ui, _| {
        let d = ui.global::<D>();
        d.set_gl_id(0);
        d.set_gl_name(SharedString::new());
        d.set_gl_target(SharedString::new());
        d.set_gl_deadline(shift_month(&today().to_string(), 6).into());
        d.set_gl_freq(1);
        d.set_gl_icon("target".into());
        d.set_gl_color(0);
        d.set_gl_error(SharedString::new());
        d.set_goal_sheet(true);
    }));
    let c = cx.clone();
    d.on_goal_edit(move || c.run(|ui, s| {
        let d = ui.global::<D>();
        let Some(g) = s.store.goals().ok().and_then(|gs| gs.into_iter().find(|g| g.id == s.goal_sel)) else { return };
        d.set_gl_id(g.id as i32);
        d.set_gl_name(g.name.clone().into());
        d.set_gl_target(raw_amount(g.target));
        d.set_gl_deadline(g.deadline.clone().into());
        d.set_gl_freq(freq_index(&g.freq));
        d.set_gl_icon(g.icon.clone().into());
        d.set_gl_color(palette_index(&g.color));
        d.set_gl_error(SharedString::new());
        d.set_goal_sheet(true);
    }));
    let c = cx.clone();
    d.on_goal_save(move || c.run(|ui, s| {
        let d = ui.global::<D>();
        let Some(target) = amount_from_text(&d.get_gl_target()) else { return d.set_gl_error("Target belum valid, misal 25jt".into()) };
        let id = d.get_gl_id() as i64;
        let existing = s.store.goals().ok().and_then(|gs| gs.into_iter().find(|g| g.id == id));
        let g = moneycore::goals::Goal {
            id,
            name: d.get_gl_name().to_string(),
            target,
            deadline: d.get_gl_deadline().to_string(),
            start: existing.as_ref().map(|g| g.start.clone()).unwrap_or_else(|| today().to_string()),
            icon: d.get_gl_icon().to_string(),
            color: PALETTE[(d.get_gl_color().max(0) as usize).min(PALETTE.len() - 1)].into(),
            freq: ["day", "week", "month"][d.get_gl_freq().clamp(0, 2) as usize].into(),
            archived: false,
        };
        match s.store.save_goal(&g) {
            Ok(new_id) => {
                s.goal_sel = new_id;
                d.set_goal_sheet(false);
                refresh_all(ui, s);
                toast(ui, s, "Rencana disimpan", None);
            }
            Err(e) => d.set_gl_error(e.into()),
        }
    }));
    let c = cx.clone();
    d.on_goal_delete(move |id| c.run(|ui, s| {
        let _ = s.store.delete_goal(id as i64);
        ui.global::<D>().set_goal_sheet(false);
        refresh_all(ui, s);
        toast(ui, s, "Rencana dihapus", None);
    }));
    let c = cx.clone();
    d.on_entry_open(move |kind| c.run(|ui, _| {
        let d = ui.global::<D>();
        d.set_ge_kind(kind);
        d.set_ge_amount(SharedString::new());
        d.set_ge_date(today().to_string().into());
        d.set_ge_note(SharedString::new());
        d.set_ge_error(SharedString::new());
        d.set_entry_sheet(true);
    }));
    let c = cx.clone();
    d.on_entry_save(move || c.run(|ui, s| {
        let d = ui.global::<D>();
        let Some(amount) = amount_from_text(&d.get_ge_amount()) else { return d.set_ge_error("Nominal belum valid".into()) };
        let e = moneycore::goals::GoalEntry {
            id: 0,
            goal_id: s.goal_sel,
            amount: if d.get_ge_kind() == 1 { -amount } else { amount },
            date: d.get_ge_date().to_string(),
            note: d.get_ge_note().to_string(),
        };
        match s.store.add_goal_entry(&e) {
            Ok(_) => {
                d.set_entry_sheet(false);
                let _ = refresh_goals(ui, s);
                toast(ui, s, &format!("{} {}", if e.amount > 0 { "Disetor" } else { "Ditarik" }, rupiah(amount)), None);
            }
            Err(err) => d.set_ge_error(err.into()),
        }
    }));
    let c = cx.clone();
    d.on_entry_delete(move |id| c.run(|ui, s| {
        let _ = s.store.delete_goal_entry(id as i64);
        let _ = refresh_goals(ui, s);
        toast(ui, s, "Setoran dihapus", None);
    }));

    // categories
    let c = cx.clone();
    d.on_cat_new(move || c.run(|ui, _| {
        let d = ui.global::<D>();
        d.set_c_id(0);
        d.set_c_name(SharedString::new());
        d.set_c_keywords(SharedString::new());
        d.set_c_kind("expense".into());
        d.set_c_icon("dots".into());
        d.set_c_color(10);
        d.set_c_error(SharedString::new());
        d.set_cat_sheet(true);
    }));
    let c = cx.clone();
    d.on_cat_open(move |cat| c.run(|ui, s| {
        let d = ui.global::<D>();
        if let Some(full) = s.store.categories().ok().and_then(|cs| cs.into_iter().find(|c| c.id == cat.id as i64)) {
            d.set_c_icon(full.icon.clone().into());
            d.set_c_color(palette_index(&full.color));
        }
        d.set_c_id(cat.id);
        d.set_c_name(cat.name.clone());
        d.set_c_keywords(cat.keywords.clone());
        d.set_c_kind(cat.kind.clone());
        d.set_c_error(SharedString::new());
        d.set_cat_sheet(true);
    }));
    let c = cx.clone();
    d.on_cat_save(move || c.run(|ui, s| {
        let d = ui.global::<D>();
        let id = d.get_c_id() as i64;
        let cat = Category {
            id,
            name: d.get_c_name().to_string(),
            kind: d.get_c_kind().to_string(),
            icon: d.get_c_icon().to_string(),
            color: PALETTE[(d.get_c_color().max(0) as usize).min(PALETTE.len() - 1)].into(),
            keywords: d
                .get_c_keywords()
                .split(',')
                .map(|k| k.trim().to_lowercase())
                .filter(|k| !k.is_empty())
                .collect::<Vec<_>>()
                .join(","),
            archived: false,
            sort: 0,
        };
        match s.store.save_category(&cat) {
            Ok(_) => {
                d.set_cat_sheet(false);
                refresh_all(ui, s);
                toast(ui, s, "Kategori disimpan", None);
            }
            Err(e) => d.set_c_error(e.into()),
        }
    }));
    let c = cx.clone();
    d.on_cat_archive(move |id| c.run(|ui, s| {
        let d = ui.global::<D>();
        let Some(mut cat) = s.store.categories().ok().and_then(|cs| cs.into_iter().find(|c| c.id == id as i64)) else { return };
        let same_kind = s.store.categories().map(|cs| cs.iter().filter(|c| c.kind == cat.kind && !c.archived).count()).unwrap_or(0);
        if same_kind <= 1 {
            return d.set_c_error("Minimal satu kategori harus tersisa".into());
        }
        cat.archived = true;
        let _ = s.store.save_category(&cat);
        d.set_cat_sheet(false);
        refresh_all(ui, s);
        toast(ui, s, "Kategori diarsipkan, riwayatnya tetap aman", None);
    }));

    // Dev affordances: `--demo` seeds an empty database, `--page N` opens a page.
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|a| a == "--demo") && st.borrow().store.is_empty().unwrap_or(false) {
        st.borrow().store.seed_demo()?;
    }
    if let Some(p) = args.iter().position(|a| a == "--page").and_then(|i| args.get(i + 1)).and_then(|v| v.parse().ok()) {
        d.set_page(p);
    }
    let flag = |name: &str| args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).and_then(|v| v.parse::<i32>().ok());
    if let Some(t) = flag("--theme") {
        apply_theme(&ui, t);
    }
    if let Some(t) = flag("--tab") {
        d.set_debt_tab(t);
    }
    if let Some(p) = flag("--period") {
        st.borrow_mut().period = p;
    }
    refresh_all(&ui, &mut st.borrow_mut());
    d.set_parsed(Parsed::default());
    if let Some(q) = args.iter().position(|a| a == "--quick").and_then(|i| args.get(i + 1)) {
        d.set_quick_text(q.as_str().into());
        d.invoke_quick_parse();
        d.set_quick_open(true);
    }
    #[cfg(feature = "snap")]
    let _snap_timer = snap::schedule(&ui, &args);
    ui.run()?;
    Ok(())
}

#[cfg(feature = "snap")]
mod snap {
    use super::*;

    /// `--snap out.png [--after ms]` renders the window into a PNG, then quits.
    pub fn schedule(ui: &AppWindow, args: &[String]) -> Option<Timer> {
        let i = args.iter().position(|a| a == "--snap")?;
        let out = args.get(i + 1)?.clone();
        let after: u64 = args.iter().position(|a| a == "--after").and_then(|i| args.get(i + 1)).and_then(|v| v.parse().ok()).unwrap_or(2500);
        let weak = ui.as_weak();
        let t = Timer::default();
        t.start(TimerMode::SingleShot, Duration::from_millis(after), move || {
            let Some(ui) = weak.upgrade() else { return };
            match ui.window().take_snapshot() {
                Ok(buf) => {
                    let file = std::fs::File::create(&out).expect("create snapshot file");
                    let mut enc = png::Encoder::new(std::io::BufWriter::new(file), buf.width(), buf.height());
                    enc.set_color(png::ColorType::Rgba);
                    enc.set_depth(png::BitDepth::Eight);
                    let mut w = enc.write_header().expect("png header");
                    w.write_image_data(buf.as_bytes()).expect("png data");
                }
                Err(e) => eprintln!("snapshot failed: {e}"),
            }
            let _ = slint::quit_event_loop();
        });
        Some(t)
    }
}
