use crate::model::*;
use crate::util::{OrStr, R};
use rusqlite::{params, Connection, Row};

const SCHEMA: &str = r#"
PRAGMA foreign_keys = ON;
CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS accounts (
  id INTEGER PRIMARY KEY,
  name TEXT NOT NULL,
  kind TEXT NOT NULL CHECK (kind IN ('bank','ewallet','cash','other')),
  color TEXT NOT NULL,
  opening INTEGER NOT NULL DEFAULT 0,
  keywords TEXT NOT NULL DEFAULT '',
  archived INTEGER NOT NULL DEFAULT 0,
  sort INTEGER NOT NULL DEFAULT 0
);
CREATE TABLE IF NOT EXISTS transfers (
  id INTEGER PRIMARY KEY,
  from_id INTEGER NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
  to_id INTEGER NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
  amount INTEGER NOT NULL CHECK (amount > 0),
  date TEXT NOT NULL,
  note TEXT NOT NULL DEFAULT '',
  CHECK (from_id <> to_id)
);
CREATE TABLE IF NOT EXISTS categories (
  id INTEGER PRIMARY KEY,
  name TEXT NOT NULL,
  kind TEXT NOT NULL CHECK (kind IN ('expense','income')),
  icon TEXT NOT NULL,
  color TEXT NOT NULL,
  keywords TEXT NOT NULL DEFAULT '',
  archived INTEGER NOT NULL DEFAULT 0,
  sort INTEGER NOT NULL DEFAULT 0
);
CREATE TABLE IF NOT EXISTS transactions (
  id INTEGER PRIMARY KEY,
  kind TEXT NOT NULL CHECK (kind IN ('expense','income')),
  amount INTEGER NOT NULL CHECK (amount > 0),
  category_id INTEGER REFERENCES categories(id) ON DELETE SET NULL,
  date TEXT NOT NULL,
  note TEXT NOT NULL DEFAULT '',
  recurring_id INTEGER REFERENCES recurring(id) ON DELETE SET NULL,
  debt_id INTEGER REFERENCES debts(id) ON DELETE SET NULL,
  created_at TEXT NOT NULL,
  account_id INTEGER REFERENCES accounts(id) ON DELETE SET NULL
);
CREATE INDEX IF NOT EXISTS idx_tx_date ON transactions(date);
CREATE INDEX IF NOT EXISTS idx_tx_cat ON transactions(category_id, date);
CREATE TABLE IF NOT EXISTS budgets (
  month TEXT NOT NULL,
  category_id INTEGER NOT NULL REFERENCES categories(id) ON DELETE CASCADE,
  amount INTEGER NOT NULL CHECK (amount >= 0),
  PRIMARY KEY (month, category_id)
);
CREATE TABLE IF NOT EXISTS recurring (
  id INTEGER PRIMARY KEY,
  kind TEXT NOT NULL CHECK (kind IN ('expense','income')),
  name TEXT NOT NULL,
  amount INTEGER NOT NULL CHECK (amount > 0),
  category_id INTEGER REFERENCES categories(id) ON DELETE SET NULL,
  day INTEGER NOT NULL CHECK (day BETWEEN 1 AND 31),
  active INTEGER NOT NULL DEFAULT 1,
  account_id INTEGER REFERENCES accounts(id) ON DELETE SET NULL
);
CREATE TABLE IF NOT EXISTS debts (
  id INTEGER PRIMARY KEY,
  name TEXT NOT NULL,
  lender TEXT NOT NULL DEFAULT '',
  start_balance INTEGER NOT NULL CHECK (start_balance > 0),
  apr REAL NOT NULL DEFAULT 0 CHECK (apr >= 0 AND apr <= 1000),
  min_payment INTEGER NOT NULL CHECK (min_payment >= 0),
  due_day INTEGER NOT NULL CHECK (due_day BETWEEN 1 AND 31),
  start_date TEXT NOT NULL,
  note TEXT NOT NULL DEFAULT '',
  closed INTEGER NOT NULL DEFAULT 0,
  tenor INTEGER CHECK (tenor IS NULL OR tenor BETWEEN 1 AND 600)
);
CREATE TABLE IF NOT EXISTS debt_payments (
  id INTEGER PRIMARY KEY,
  debt_id INTEGER NOT NULL REFERENCES debts(id) ON DELETE CASCADE,
  kind TEXT NOT NULL CHECK (kind IN ('payment','charge')),
  amount INTEGER NOT NULL CHECK (amount > 0),
  date TEXT NOT NULL,
  note TEXT NOT NULL DEFAULT '',
  tx_id INTEGER REFERENCES transactions(id) ON DELETE SET NULL
);
CREATE INDEX IF NOT EXISTS idx_dp_debt ON debt_payments(debt_id, date);
CREATE TABLE IF NOT EXISTS learn (
  token TEXT NOT NULL,
  category_id INTEGER NOT NULL REFERENCES categories(id) ON DELETE CASCADE,
  n INTEGER NOT NULL DEFAULT 0,
  PRIMARY KEY (token, category_id)
);
"#;

/// (name, kind, icon, color, keywords)
const SEED: &[(&str, &str, &str, &str, &str)] = &[
    ("Makan & Minum", "expense", "food", "#C4703D",
     "makan,nasi,kopi,coffee,ngopi,sarapan,lunch,dinner,gofood,grabfood,shopeefood,starbucks,bakso,mie,ayam,sate,warteg,padang,martabak,roti,snack,jajan,teh,boba,resto,restoran,cafe,kafe,mcd,kfc,pizza,sushi,minum,es,makan siang,makan malam"),
    ("Transportasi", "expense", "car", "#3E6E8E",
     "grab,gojek,ojol,ojek,taksi,taxi,bensin,pertamina,shell,bbm,tol,parkir,krl,mrt,lrt,transjakarta,busway,kereta,tiket,pesawat,bus,servis motor,servis mobil,maxim,bluebird"),
    ("Belanja", "expense", "bag", "#A8566F",
     "shopee,tokopedia,lazada,tiktok shop,indomaret,alfamart,supermarket,belanja,baju,sepatu,kaos,celana,skincare,sabun,shampo,uniqlo,ikea,groceries,sayur,buah,pasar"),
    ("Tagihan & Utilitas", "expense", "bolt", "#B08A2E",
     "listrik,pln,token,pulsa,kuota,paket data,internet,indihome,wifi,biznet,air,pdam,bpjs,asuransi,iuran,telkomsel,xl,tagihan,icloud,google one"),
    ("Hiburan", "expense", "play", "#6B7B3A",
     "netflix,spotify,youtube,disney,bioskop,xxi,cgv,nonton,game,steam,konser,karaoke,liburan,hotel,staycation,padel,futsal,badminton,hobi"),
    ("Kesehatan", "expense", "heart", "#B9534B",
     "obat,apotek,dokter,klinik,rumah sakit,rs,gym,fitness,vitamin,suplemen,lab,gigi,kacamata"),
    ("Pendidikan", "expense", "book", "#2F7A74",
     "buku,kursus,kuliah,ukt,spp,les,udemy,coursera,ielts,toefl,sertifikasi,seminar,workshop,kelas"),
    ("Rumah", "expense", "home", "#8A6A4B",
     "kos,kost,sewa,kontrakan,galon,gas,elpiji,laundry,perabot,renovasi,art,kebersihan,sampah"),
    ("Sosial & Hadiah", "expense", "gift", "#A0613A",
     "kado,hadiah,zakat,sedekah,infaq,donasi,nikahan,kondangan,arisan,traktir,amplop,sumbangan"),
    ("Cicilan Hutang", "expense", "card", "#55605A",
     "cicilan,angsuran,paylater,kartu kredit,kredit,pinjaman,hutang,utang,kpr"),
    ("Lainnya", "expense", "dots", "#858B85", ""),
    ("Gaji", "income", "wallet", "#1E5B3F", "gaji,gajian,salary,payroll,upah"),
    ("Bonus", "income", "star", "#3C8A5E", "bonus,thr,insentif,komisi,reward"),
    ("Freelance", "income", "briefcase", "#2F7A74", "freelance,project,proyek,fee,honor,klien,client,invoice"),
    ("Investasi", "income", "trend", "#4F7F3A", "dividen,bunga,deposito,saham,reksadana,crypto,profit,imbal"),
    ("Pemasukan Lain", "income", "plus", "#6E8B5C", "cashback,refund,hadiah,transfer masuk,jual,penjualan"),
];

pub fn open(path: &str) -> R<Connection> {
    let conn = Connection::open(path).s()?;
    init(&conn)?;
    Ok(conn)
}

pub fn open_memory() -> R<Connection> {
    let conn = Connection::open_in_memory().s()?;
    init(&conn)?;
    Ok(conn)
}

fn init(conn: &Connection) -> R<()> {
    conn.execute_batch(
        "PRAGMA journal_mode = WAL; PRAGMA synchronous = NORMAL; PRAGMA busy_timeout = 3000;",
    )
    .s()?;
    conn.execute_batch(SCHEMA).s()?;
    conn.execute_batch(crate::goals::GOAL_SCHEMA).s()?;
    migrate(conn)?;
    let seeded: i64 = conn
        .query_row("SELECT COUNT(*) FROM categories", [], |r| r.get(0))
        .s()?;
    if seeded == 0 {
        seed_categories(conn)?;
    }
    let accounts: i64 = conn.query_row("SELECT COUNT(*) FROM accounts", [], |r| r.get(0)).s()?;
    if accounts == 0 {
        seed_accounts(conn)?;
    }
    conn.execute("INSERT OR REPLACE INTO meta(key, value) VALUES ('schema_version', '2')", []).s()?;
    Ok(())
}

fn has_column(conn: &Connection, table: &str, col: &str) -> R<bool> {
    let mut st = conn.prepare(&format!("PRAGMA table_info({table})")).s()?;
    let names = st.query_map([], |r| r.get::<_, String>(1)).s()?;
    for n in names {
        if n.s()? == col {
            return Ok(true);
        }
    }
    Ok(false)
}

/// v1 -> v2: money sources on transactions and recurring rules.
fn migrate(conn: &Connection) -> R<()> {
    for table in ["transactions", "recurring"] {
        if !has_column(conn, table, "account_id")? {
            conn.execute_batch(&format!(
                "ALTER TABLE {table} ADD COLUMN account_id INTEGER REFERENCES accounts(id) ON DELETE SET NULL;"
            ))
            .s()?;
        }
    }
    if !has_column(conn, "debts", "tenor")? {
        conn.execute_batch("ALTER TABLE debts ADD COLUMN tenor INTEGER;").s()?;
    }
    conn.execute_batch(
        "CREATE INDEX IF NOT EXISTS idx_tx_acc ON transactions(account_id, date);
         CREATE INDEX IF NOT EXISTS idx_tr_date ON transfers(date);",
    )
    .s()
}

pub fn seed_accounts(conn: &Connection) -> R<()> {
    conn.execute(
        "INSERT INTO accounts(name, kind, color, opening, keywords, sort) VALUES ('Tunai', 'cash', '#6E8B5C', 0, 'tunai,cash,cas,dompet', 0)",
        [],
    )
    .s()?;
    Ok(())
}

pub fn seed_categories(conn: &Connection) -> R<()> {
    let mut stmt = conn
        .prepare("INSERT INTO categories(name, kind, icon, color, keywords, sort) VALUES (?1,?2,?3,?4,?5,?6)")
        .s()?;
    for (i, (name, kind, icon, color, kw)) in SEED.iter().enumerate() {
        stmt.execute(params![name, kind, icon, color, kw, i as i64]).s()?;
    }
    Ok(())
}

pub fn row_category(r: &Row) -> rusqlite::Result<Category> {
    Ok(Category {
        id: r.get(0)?,
        name: r.get(1)?,
        kind: r.get(2)?,
        icon: r.get(3)?,
        color: r.get(4)?,
        keywords: r.get(5)?,
        archived: r.get::<_, i64>(6)? != 0,
        sort: r.get(7)?,
    })
}
pub const CAT_COLS: &str = "id, name, kind, icon, color, keywords, archived, sort";

pub fn row_tx(r: &Row) -> rusqlite::Result<Transaction> {
    Ok(Transaction {
        id: r.get(0)?,
        kind: r.get(1)?,
        amount: r.get(2)?,
        category_id: r.get(3)?,
        date: r.get(4)?,
        note: r.get(5)?,
        recurring_id: r.get(6)?,
        debt_id: r.get(7)?,
        created_at: r.get(8)?,
        account_id: r.get(9)?,
    })
}
pub const TX_COLS: &str =
    "id, kind, amount, category_id, date, note, recurring_id, debt_id, created_at, account_id";

pub fn row_recurring(r: &Row) -> rusqlite::Result<Recurring> {
    Ok(Recurring {
        id: r.get(0)?,
        kind: r.get(1)?,
        name: r.get(2)?,
        amount: r.get(3)?,
        category_id: r.get(4)?,
        day: r.get(5)?,
        active: r.get::<_, i64>(6)? != 0,
        account_id: r.get(7)?,
    })
}
pub const REC_COLS: &str = "id, kind, name, amount, category_id, day, active, account_id";

pub fn row_debt(r: &Row) -> rusqlite::Result<Debt> {
    Ok(Debt {
        id: r.get(0)?,
        name: r.get(1)?,
        lender: r.get(2)?,
        start_balance: r.get(3)?,
        apr: r.get(4)?,
        min_payment: r.get(5)?,
        due_day: r.get(6)?,
        start_date: r.get(7)?,
        note: r.get(8)?,
        closed: r.get::<_, i64>(9)? != 0,
        tenor: r.get(10)?,
    })
}
pub const DEBT_COLS: &str =
    "id, name, lender, start_balance, apr, min_payment, due_day, start_date, note, closed, tenor";

pub fn row_payment(r: &Row) -> rusqlite::Result<DebtPayment> {
    Ok(DebtPayment {
        id: r.get(0)?,
        debt_id: r.get(1)?,
        kind: r.get(2)?,
        amount: r.get(3)?,
        date: r.get(4)?,
        note: r.get(5)?,
        tx_id: r.get(6)?,
    })
}
pub const PAY_COLS: &str = "id, debt_id, kind, amount, date, note, tx_id";

pub fn categories(conn: &Connection) -> R<Vec<Category>> {
    let mut st = conn
        .prepare(&format!("SELECT {CAT_COLS} FROM categories ORDER BY kind DESC, sort, id"))
        .s()?;
    let rows = st.query_map([], row_category).s()?;
    rows.collect::<Result<Vec<_>, _>>().s()
}

pub fn transactions_between(conn: &Connection, start: &str, end: &str) -> R<Vec<Transaction>> {
    let mut st = conn
        .prepare(&format!(
            "SELECT {TX_COLS} FROM transactions WHERE date >= ?1 AND date < ?2 ORDER BY date DESC, id DESC"
        ))
        .s()?;
    let rows = st.query_map(params![start, end], row_tx).s()?;
    rows.collect::<Result<Vec<_>, _>>().s()
}

pub fn recurring_all(conn: &Connection) -> R<Vec<Recurring>> {
    let mut st = conn
        .prepare(&format!("SELECT {REC_COLS} FROM recurring ORDER BY kind DESC, day, id"))
        .s()?;
    let rows = st.query_map([], row_recurring).s()?;
    rows.collect::<Result<Vec<_>, _>>().s()
}

pub fn debts_all(conn: &Connection) -> R<Vec<Debt>> {
    let mut st = conn
        .prepare(&format!("SELECT {DEBT_COLS} FROM debts ORDER BY closed, id"))
        .s()?;
    let rows = st.query_map([], row_debt).s()?;
    rows.collect::<Result<Vec<_>, _>>().s()
}

pub fn payments_for(conn: &Connection, debt_id: i64) -> R<Vec<DebtPayment>> {
    let mut st = conn
        .prepare(&format!(
            "SELECT {PAY_COLS} FROM debt_payments WHERE debt_id = ?1 ORDER BY date DESC, id DESC"
        ))
        .s()?;
    let rows = st.query_map([debt_id], row_payment).s()?;
    rows.collect::<Result<Vec<_>, _>>().s()
}

pub fn budgets_for(conn: &Connection, month: &str) -> R<Vec<(i64, i64)>> {
    let mut st = conn
        .prepare("SELECT category_id, amount FROM budgets WHERE month = ?1")
        .s()?;
    let rows = st.query_map([month], |r| Ok((r.get(0)?, r.get(1)?))).s()?;
    rows.collect::<Result<Vec<_>, _>>().s()
}

pub fn row_account(r: &Row) -> rusqlite::Result<Account> {
    Ok(Account {
        id: r.get(0)?,
        name: r.get(1)?,
        kind: r.get(2)?,
        color: r.get(3)?,
        opening: r.get(4)?,
        keywords: r.get(5)?,
        archived: r.get::<_, i64>(6)? != 0,
        sort: r.get(7)?,
    })
}
pub const ACC_COLS: &str = "id, name, kind, color, opening, keywords, archived, sort";

pub fn accounts(conn: &Connection) -> R<Vec<Account>> {
    let mut st = conn.prepare(&format!("SELECT {ACC_COLS} FROM accounts ORDER BY archived, sort, id")).s()?;
    let rows = st.query_map([], row_account).s()?;
    rows.collect::<Result<Vec<_>, _>>().s()
}

pub fn row_transfer(r: &Row) -> rusqlite::Result<Transfer> {
    Ok(Transfer { id: r.get(0)?, from_id: r.get(1)?, to_id: r.get(2)?, amount: r.get(3)?, date: r.get(4)?, note: r.get(5)? })
}
pub const TRF_COLS: &str = "id, from_id, to_id, amount, date, note";

pub fn transfers_between(conn: &Connection, start: &str, end: &str) -> R<Vec<Transfer>> {
    let mut st = conn
        .prepare(&format!("SELECT {TRF_COLS} FROM transfers WHERE date >= ?1 AND date < ?2 ORDER BY date DESC, id DESC"))
        .s()?;
    let rows = st.query_map(params![start, end], row_transfer).s()?;
    rows.collect::<Result<Vec<_>, _>>().s()
}
