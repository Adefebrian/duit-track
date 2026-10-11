//! cargo run --release -p moneycore --example bench
//! Times the engine on four months of demo data (in-memory SQLite).

use chrono::NaiveDate;
use moneycore::period::Period;
use moneycore::Store;
use std::time::Instant;

fn time<T>(label: &str, runs: u32, mut f: impl FnMut() -> T) {
    let t = Instant::now();
    for _ in 0..runs {
        std::hint::black_box(f());
    }
    let per = t.elapsed().as_secs_f64() * 1000.0 / runs as f64;
    println!("{label:<34} {per:>8.3} ms");
}

fn main() {
    let mut s = Store::open_memory().unwrap();
    s.set_today(NaiveDate::from_ymd_opt(2026, 10, 12).unwrap());
    let t = Instant::now();
    s.seed_demo().unwrap();
    let n: usize = (0..4).map(|k| s.transactions_month(2026, 7 + k).unwrap().len()).sum();
    println!("demo data: {n} transactions, seeded in {:.1} ms\n", t.elapsed().as_secs_f64() * 1000.0);
    time("parse \"kopi 25rb gopay kemarin\"", 2000, || s.parse("kopi 25rb gopay kemarin", Some("expense")).unwrap());
    time("dashboard (month + insights)", 50, || s.dashboard(2026, 10).unwrap());
    for (p, l) in [(Period::Day, "day"), (Period::Week, "week"), (Period::Month, "month"), (Period::Year, "year")] {
        time(&format!("period view: {l}"), 50, || s.period_view(p, 0).unwrap());
    }
    time("analytics (12 months)", 50, || s.analytics(12).unwrap());
    time("debt views + payoff simulation", 50, || s.debt_plan(500_000).unwrap());
    time("savings plans", 200, || s.goal_views().unwrap());
    time("total money (all sources)", 200, || s.pool_total().unwrap());
}
