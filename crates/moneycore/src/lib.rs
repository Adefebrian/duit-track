//! Duitrack core: storage and rule-based financial reasoning. No UI, no network.

pub mod analytics;
pub mod db;
pub mod debt;
pub mod goals;
pub mod intel;
pub mod model;
pub mod parse;
pub mod period;
pub mod store;
pub mod util;

pub use store::Store;
