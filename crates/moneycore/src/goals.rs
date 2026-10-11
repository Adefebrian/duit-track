//! Savings plans: "Trip ke Jepang Rp25jt by March". Several at once, each with
//! its own deposits, required pace per day/week/month and an on-track verdict.

use crate::store::Store;
use crate::util::*;
use chrono::NaiveDate;
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Goal {
    #[serde(default)]
    pub id: i64,
    pub name: String,
    pub target: i64,
    /// YYYY-MM-DD
    pub deadline: String,
    /// when saving started, YYYY-MM-DD
    pub start: String,
    #[serde(default = "dots")]
    pub icon: String,
    #[serde(default = "green")]
    pub color: String,
    /// preferred rhythm: day | week | month
    #[serde(default = "month")]
    pub freq: String,
    #[serde(default)]
    pub archived: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GoalEntry {
    #[serde(default)]
    pub id: i64,
    pub goal_id: i64,
    /// positive = deposit, negative = withdrawal
    pub amount: i64,
    pub date: String,
    #[serde(default)]
    pub note: String,
}

fn dots() -> String {
    "target".into()
}
fn green() -> String {
    "#1E5B3F".into()
}
fn month() -> String {
    "month".into()
}

#[derive(Debug, Clone)]
pub struct GoalView {
    pub goal: Goal,
    pub saved: i64,
    pub remaining: i64,
    pub progress: f64,
    pub days_left: i64,
    pub per_day: i64,
    pub per_week: i64,
    pub per_month: i64,
    /// what a straight line from start to deadline expects by today
    pub expected_now: i64,
    /// ahead | on_track | behind | done | overdue
    pub status: &'static str,
    /// projected finish at the pace so far, None if no pace yet
    pub projected: Option<NaiveDate>,
    pub entries: Vec<GoalEntry>,
    /// (date, cumulative saved) sorted by date
    pub series: Vec<(NaiveDate, i64)>,
}

pub const GOAL_SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS goals (
  id INTEGER PRIMARY KEY,
  name TEXT NOT NULL,
  target INTEGER NOT NULL CHECK (target > 0),
  deadline TEXT NOT NULL,
  start TEXT NOT NULL,
  icon TEXT NOT NULL DEFAULT 'target',
  color TEXT NOT NULL DEFAULT '#1E5B3F',
  freq TEXT NOT NULL DEFAULT 'month' CHECK (freq IN ('day','week','month')),
  archived INTEGER NOT NULL DEFAULT 0
);
CREATE TABLE IF NOT EXISTS goal_entries (
  id INTEGER PRIMARY KEY,
  goal_id INTEGER NOT NULL REFERENCES goals(id) ON DELETE CASCADE,
  amount INTEGER NOT NULL CHECK (amount <> 0),
  date TEXT NOT NULL,
  note TEXT NOT NULL DEFAULT ''
);
CREATE INDEX IF NOT EXISTS idx_ge_goal ON goal_entries(goal_id, date);
"#;

fn ceil_div(a: i64, b: i64) -> i64 {
    if b <= 0 {
        a
    } else {
        (a + b - 1) / b
    }
}

impl Store {
    pub fn goals(&self) -> R<Vec<Goal>> {
        let mut st = self
            .conn
            .prepare("SELECT id, name, target, deadline, start, icon, color, freq, archived FROM goals ORDER BY archived, deadline, id")
            .s()?;
        let rows = st
            .query_map([], |r| {
                Ok(Goal {
                    id: r.get(0)?,
                    name: r.get(1)?,
                    target: r.get(2)?,
                    deadline: r.get(3)?,
                    start: r.get(4)?,
                    icon: r.get(5)?,
                    color: r.get(6)?,
                    freq: r.get(7)?,
                    archived: r.get::<_, i64>(8)? != 0,
                })
            })
            .s()?;
        rows.collect::<Result<Vec<_>, _>>().s()
    }

    pub fn save_goal(&self, g: &Goal) -> R<i64> {
        let name = g.name.trim();
        if name.is_empty() || name.chars().count() > 60 {
            return Err("nama rencana 1 sampai 60 karakter".into());
        }
        if !(1..=1_000_000_000_000).contains(&g.target) {
            return Err("target harus lebih dari 0".into());
        }
        let (s, d) = (parse_date(&g.start)?, parse_date(&g.deadline)?);
        if d <= s {
            return Err("tenggat harus setelah tanggal mulai".into());
        }
        if !["day", "week", "month"].contains(&g.freq.as_str()) {
            return Err("ritme tidak valid".into());
        }
        if g.id == 0 {
            self.conn
                .execute(
                    "INSERT INTO goals(name, target, deadline, start, icon, color, freq) VALUES (?1,?2,?3,?4,?5,?6,?7)",
                    params![name, g.target, g.deadline, g.start, g.icon, g.color, g.freq],
                )
                .s()?;
            Ok(self.conn.last_insert_rowid())
        } else {
            self.conn
                .execute(
                    "UPDATE goals SET name=?1, target=?2, deadline=?3, start=?4, icon=?5, color=?6, freq=?7, archived=?8 WHERE id=?9",
                    params![name, g.target, g.deadline, g.start, g.icon, g.color, g.freq, g.archived as i64, g.id],
                )
                .s()?;
            Ok(g.id)
        }
    }

    pub fn delete_goal(&self, id: i64) -> R<()> {
        self.conn.execute("DELETE FROM goals WHERE id=?1", [id]).s()?;
        Ok(())
    }

    pub fn add_goal_entry(&self, e: &GoalEntry) -> R<i64> {
        if e.amount == 0 || e.amount.abs() > 1_000_000_000_000 {
            return Err("nominal tidak valid".into());
        }
        parse_date(&e.date)?;
        let exists: Option<i64> = self
            .conn
            .query_row("SELECT id FROM goals WHERE id=?1", [e.goal_id], |r| r.get(0))
            .optional()
            .s()?;
        if exists.is_none() {
            return Err("rencana tidak ditemukan".into());
        }
        if e.amount < 0 {
            let saved: i64 = self
                .conn
                .query_row("SELECT COALESCE(SUM(amount),0) FROM goal_entries WHERE goal_id=?1", [e.goal_id], |r| r.get(0))
                .s()?;
            if saved + e.amount < 0 {
                return Err("penarikan melebihi yang terkumpul".into());
            }
        }
        let note: String = e.note.trim().chars().take(120).collect();
        self.conn
            .execute(
                "INSERT INTO goal_entries(goal_id, amount, date, note) VALUES (?1,?2,?3,?4)",
                params![e.goal_id, e.amount, e.date, note],
            )
            .s()?;
        Ok(self.conn.last_insert_rowid())
    }

    pub fn delete_goal_entry(&self, id: i64) -> R<()> {
        self.conn.execute("DELETE FROM goal_entries WHERE id=?1", [id]).s()?;
        Ok(())
    }

    fn goal_entries(&self, goal: i64) -> R<Vec<GoalEntry>> {
        let mut st = self
            .conn
            .prepare("SELECT id, goal_id, amount, date, note FROM goal_entries WHERE goal_id=?1 ORDER BY date DESC, id DESC")
            .s()?;
        let rows = st
            .query_map([goal], |r| Ok(GoalEntry { id: r.get(0)?, goal_id: r.get(1)?, amount: r.get(2)?, date: r.get(3)?, note: r.get(4)? }))
            .s()?;
        rows.collect::<Result<Vec<_>, _>>().s()
    }

    pub fn goal_views(&self) -> R<Vec<GoalView>> {
        let today = self.today();
        let mut out = vec![];
        for g in self.goals()? {
            let entries = self.goal_entries(g.id)?;
            let saved: i64 = entries.iter().map(|e| e.amount).sum::<i64>().max(0);
            let remaining = (g.target - saved).max(0);
            let start = parse_date(&g.start)?;
            let deadline = parse_date(&g.deadline)?;
            let days_left = (deadline - today).num_days().max(0);
            // Count today as a saving day; weeks and months round up so the
            // amount asked per period is enough.
            let d_left = days_left + 1;
            let per_day = ceil_div(remaining, d_left);
            let per_week = ceil_div(remaining, ceil_div(d_left, 7));
            let per_month = ceil_div(remaining, ceil_div(d_left, 30).max(1));
            let total_days = (deadline - start).num_days().max(1);
            let elapsed = (today - start).num_days().clamp(0, total_days);
            let expected_now = (g.target as f64 * elapsed as f64 / total_days as f64).round() as i64;
            let status = if saved >= g.target {
                "done"
            } else if today > deadline {
                "overdue"
            } else if elapsed < 3 || saved as f64 >= expected_now as f64 * 0.95 {
                if saved as f64 > expected_now as f64 * 1.1 && elapsed >= 3 { "ahead" } else { "on_track" }
            } else {
                "behind"
            };
            let projected = if saved >= g.target {
                None
            } else if elapsed > 0 && saved > 0 {
                let rate = saved as f64 / elapsed as f64;
                Some(plus_days(today, (remaining as f64 / rate).ceil() as i64))
            } else {
                None
            };
            let mut asc = entries.clone();
            asc.sort_by(|a, b| a.date.cmp(&b.date).then(a.id.cmp(&b.id)));
            let mut acc = 0;
            let mut series: Vec<(NaiveDate, i64)> = vec![(start, 0)];
            for e in &asc {
                acc += e.amount;
                if let Ok(d) = parse_date(&e.date) {
                    match series.last_mut() {
                        Some(last) if last.0 == d => last.1 = acc,
                        _ => series.push((d, acc)),
                    }
                }
            }
            out.push(GoalView {
                progress: (saved as f64 / g.target as f64).min(1.0),
                goal: g,
                saved,
                remaining,
                days_left,
                per_day,
                per_week,
                per_month,
                expected_now,
                status,
                projected,
                entries,
                series,
            });
        }
        Ok(out)
    }

    pub fn seed_demo_goals(&self) -> R<()> {
        let today = self.today();
        let japan = self.save_goal(&Goal {
            id: 0,
            name: "Trip ke Jepang".into(),
            target: 25_000_000,
            deadline: plus_days(today, 170).to_string(),
            start: plus_days(today, -75).to_string(),
            icon: "star".into(),
            color: "#A8566F".into(),
            freq: "week".into(),
            archived: false,
        })?;
        for (i, amt) in [1_500_000i64, 1_000_000, 2_000_000, 1_000_000, 1_500_000, 750_000].iter().enumerate() {
            self.add_goal_entry(&GoalEntry { id: 0, goal_id: japan, amount: *amt, date: plus_days(today, -70 + i as i64 * 12).to_string(), note: String::new() })?;
        }
        let laptop = self.save_goal(&Goal {
            id: 0,
            name: "MacBook baru".into(),
            target: 18_000_000,
            deadline: plus_days(today, 95).to_string(),
            start: plus_days(today, -40).to_string(),
            icon: "briefcase".into(),
            color: "#2F5D8A".into(),
            freq: "month".into(),
            archived: false,
        })?;
        for (i, amt) in [3_000_000i64, 2_500_000].iter().enumerate() {
            self.add_goal_entry(&GoalEntry { id: 0, goal_id: laptop, amount: *amt, date: plus_days(today, -35 + i as i64 * 30).to_string(), note: String::new() })?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> Store {
        let mut s = Store::open_memory().unwrap();
        s.set_today(NaiveDate::from_ymd_opt(2026, 10, 12).unwrap());
        s
    }

    #[test]
    fn pace_and_status() {
        let s = store();
        let id = s
            .save_goal(&Goal {
                id: 0,
                name: "Jepang".into(),
                target: 3_000_000,
                deadline: "2026-12-11".into(), // 60 days from today
                start: "2026-09-12".into(),   // 30 days ago
                icon: "star".into(),
                color: "#000000".into(),
                freq: "week".into(),
                archived: false,
            })
            .unwrap();
        // a third of the time gone (30 of 90 days), a fifth saved: behind
        s.add_goal_entry(&GoalEntry { id: 0, goal_id: id, amount: 600_000, date: "2026-10-01".into(), note: String::new() }).unwrap();
        let v = &s.goal_views().unwrap()[0];
        assert_eq!(v.expected_now, 1_000_000);
        assert_eq!(v.remaining, 2_400_000);
        assert_eq!(v.status, "behind");
        assert_eq!(v.per_day, 39_345); // 2,4jt over 61 days, rounded up
        assert_eq!(v.per_week, 266_667); // over 9 weeks
        assert_eq!(v.per_month, 800_000); // over 3 months
        assert!(v.projected.is_some());
        // cannot withdraw more than saved
        assert!(s.add_goal_entry(&GoalEntry { id: 0, goal_id: id, amount: -2_000_000, date: "2026-10-12".into(), note: String::new() }).is_err());
        s.add_goal_entry(&GoalEntry { id: 0, goal_id: id, amount: 2_400_000, date: "2026-10-12".into(), note: String::new() }).unwrap();
        assert_eq!(s.goal_views().unwrap()[0].status, "done");
    }

    #[test]
    fn rejects_bad_plans() {
        let s = store();
        let bad = Goal { id: 0, name: "".into(), target: 0, deadline: "2026-01-01".into(), start: "2026-02-01".into(), icon: "x".into(), color: "#000".into(), freq: "year".into(), archived: false };
        assert!(s.save_goal(&bad).is_err());
    }
}
