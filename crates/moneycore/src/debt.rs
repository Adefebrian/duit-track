//! Month-by-month debt payoff simulation.

#[derive(Debug, Clone)]
pub struct SimDebt {
    pub id: i64,
    pub balance: f64,
    /// annual percent
    pub apr: f64,
    pub min_payment: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Strategy {
    /// Pay only the minimums, no rollover.
    Minimum,
    /// Extra money to the highest interest rate first.
    Avalanche,
    /// Extra money to the smallest balance first.
    Snowball,
}

#[derive(Debug, Clone, Default)]
pub struct SimResult {
    pub months: u32,
    pub total_interest: f64,
    pub total_paid: f64,
    /// (debt id, month index it reaches zero, 1-based)
    pub payoff: Vec<(i64, u32)>,
    /// total remaining balance at end of each month, starting with month 0
    pub timeline: Vec<f64>,
    pub feasible: bool,
}

const MAX_MONTHS: u32 = 600;

pub fn simulate(debts: &[SimDebt], extra: f64, strategy: Strategy) -> SimResult {
    let mut bal: Vec<f64> = debts.iter().map(|d| d.balance.max(0.0)).collect();
    let mut res = SimResult {
        timeline: vec![bal.iter().sum()],
        ..Default::default()
    };
    let monthly_budget: f64 = debts.iter().map(|d| d.min_payment).sum::<f64>()
        + if strategy == Strategy::Minimum { 0.0 } else { extra.max(0.0) };

    let mut month = 0;
    while bal.iter().any(|b| *b > 0.5) && month < MAX_MONTHS {
        month += 1;
        for (i, d) in debts.iter().enumerate() {
            if bal[i] > 0.5 {
                let interest = bal[i] * d.apr / 100.0 / 12.0;
                bal[i] += interest;
                res.total_interest += interest;
            }
        }
        let mut budget = if strategy == Strategy::Minimum {
            debts
                .iter()
                .enumerate()
                .filter(|(i, _)| bal[*i] > 0.5)
                .map(|(_, d)| d.min_payment)
                .sum()
        } else {
            monthly_budget
        };
        for (i, d) in debts.iter().enumerate() {
            if bal[i] > 0.5 {
                let p = d.min_payment.min(bal[i]).min(budget);
                bal[i] -= p;
                budget -= p;
                res.total_paid += p;
            }
        }
        if strategy != Strategy::Minimum {
            let mut order: Vec<usize> = (0..debts.len()).filter(|i| bal[*i] > 0.5).collect();
            match strategy {
                Strategy::Avalanche => order.sort_by(|a, b| {
                    debts[*b].apr.partial_cmp(&debts[*a].apr).unwrap_or(std::cmp::Ordering::Equal)
                        .then(bal[*a].partial_cmp(&bal[*b]).unwrap_or(std::cmp::Ordering::Equal))
                }),
                _ => order.sort_by(|a, b| bal[*a].partial_cmp(&bal[*b]).unwrap_or(std::cmp::Ordering::Equal)),
            }
            for i in order {
                if budget <= 0.0 {
                    break;
                }
                let p = budget.min(bal[i]);
                bal[i] -= p;
                budget -= p;
                res.total_paid += p;
            }
        }
        for (i, d) in debts.iter().enumerate() {
            if bal[i] <= 0.5 && !res.payoff.iter().any(|(id, _)| *id == d.id) {
                bal[i] = 0.0;
                res.payoff.push((d.id, month));
            }
        }
        res.timeline.push(bal.iter().sum());
    }
    res.months = month;
    res.feasible = bal.iter().all(|b| *b <= 0.5);
    res
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Vec<SimDebt> {
        vec![
            SimDebt { id: 1, balance: 10_000_000.0, apr: 24.0, min_payment: 500_000.0 },
            SimDebt { id: 2, balance: 2_000_000.0, apr: 6.0, min_payment: 200_000.0 },
        ]
    }

    #[test]
    fn zero_interest_is_exact() {
        let d = vec![SimDebt { id: 1, balance: 1_200_000.0, apr: 0.0, min_payment: 100_000.0 }];
        let r = simulate(&d, 0.0, Strategy::Minimum);
        assert_eq!(r.months, 12);
        assert!(r.feasible);
        assert!(r.total_interest.abs() < 1e-6);
    }

    #[test]
    fn avalanche_saves_interest_and_extra_is_faster() {
        let a = simulate(&sample(), 500_000.0, Strategy::Avalanche);
        let s = simulate(&sample(), 500_000.0, Strategy::Snowball);
        let m = simulate(&sample(), 0.0, Strategy::Minimum);
        assert!(a.total_interest <= s.total_interest);
        assert!(a.months < m.months);
        // snowball clears the small debt first
        assert_eq!(s.payoff[0].0, 2);
    }

    #[test]
    fn infeasible_when_min_below_interest() {
        let d = vec![SimDebt { id: 1, balance: 10_000_000.0, apr: 60.0, min_payment: 100_000.0 }];
        let r = simulate(&d, 0.0, Strategy::Minimum);
        assert!(!r.feasible);
    }
}
