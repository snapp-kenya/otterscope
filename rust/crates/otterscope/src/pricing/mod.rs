//! Maintained pricing table and cost math.
//!
//! Rates are USD per million tokens. The built-in table is maintained by hand;
//! operators override or extend it with a JSON file via `serve -pricing`.
//! Unknown models yield no cost — never a fabricated one.

mod table;

#[cfg(test)]
mod tests;

pub use table::{Rate, Table, cost};
