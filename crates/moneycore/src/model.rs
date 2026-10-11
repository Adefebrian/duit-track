use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Category {
    #[serde(default)]
    pub id: i64,
    pub name: String,
    pub kind: String,
    pub icon: String,
    pub color: String,
    #[serde(default)]
    pub keywords: String,
    #[serde(default)]
    pub archived: bool,
    #[serde(default)]
    pub sort: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Transaction {
    #[serde(default)]
    pub id: i64,
    pub kind: String,
    pub amount: i64,
    pub category_id: Option<i64>,
    pub date: String,
    #[serde(default)]
    pub note: String,
    #[serde(default)]
    pub recurring_id: Option<i64>,
    #[serde(default)]
    pub debt_id: Option<i64>,
    #[serde(default)]
    pub created_at: String,
    #[serde(default)]
    pub account_id: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Recurring {
    #[serde(default)]
    pub id: i64,
    pub kind: String,
    pub name: String,
    pub amount: i64,
    pub category_id: Option<i64>,
    pub day: u32,
    #[serde(default = "yes")]
    pub active: bool,
    #[serde(default)]
    pub account_id: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Debt {
    #[serde(default)]
    pub id: i64,
    pub name: String,
    #[serde(default)]
    pub lender: String,
    pub start_balance: i64,
    /// Annual interest rate in percent, e.g. 12.5
    #[serde(default)]
    pub apr: f64,
    pub min_payment: i64,
    pub due_day: u32,
    pub start_date: String,
    #[serde(default)]
    pub note: String,
    #[serde(default)]
    pub closed: bool,
    /// Fixed-term installment plan: number of months. None = flexible debt
    /// (credit card, open loan) where the balance and interest drive payoff.
    #[serde(default)]
    pub tenor: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DebtPayment {
    #[serde(default)]
    pub id: i64,
    pub debt_id: i64,
    /// "payment" reduces balance, "charge" adds to it (interest, fee, new draw)
    pub kind: String,
    pub amount: i64,
    pub date: String,
    #[serde(default)]
    pub note: String,
    #[serde(default)]
    pub tx_id: Option<i64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Insight {
    pub id: String,
    /// good | warn | bad | info
    pub tone: &'static str,
    pub title: String,
    pub body: String,
    pub priority: i32,
}

/// A money source: bank, e-wallet, cash.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Account {
    #[serde(default)]
    pub id: i64,
    pub name: String,
    /// bank | ewallet | cash | other
    pub kind: String,
    pub color: String,
    /// balance when you started tracking
    #[serde(default)]
    pub opening: i64,
    #[serde(default)]
    pub keywords: String,
    #[serde(default)]
    pub archived: bool,
    #[serde(default)]
    pub sort: i64,
}

/// Money moved between two of your own sources. Not income, not spending.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Transfer {
    #[serde(default)]
    pub id: i64,
    pub from_id: i64,
    pub to_id: i64,
    pub amount: i64,
    pub date: String,
    #[serde(default)]
    pub note: String,
}

fn yes() -> bool {
    true
}
