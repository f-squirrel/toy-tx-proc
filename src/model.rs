//! Core data types for the payments engine.
//!
//! Amounts are always [`rust_decimal::Decimal`] — never floating point — so that
//! financial arithmetic is exact.

use std::fmt;

use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// A client identifier. Newtype over `u16` so it can't be confused with a `TxId`
/// or any other bare integer. Serializes transparently as the underlying number.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ClientId(pub u16);

impl fmt::Display for ClientId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

/// A globally-unique transaction identifier. Newtype over `u32`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct TxId(pub u32);

impl fmt::Display for TxId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

/// What a transaction does. Each variant carries exactly the data that kind of
/// operation needs — deposits and withdrawals own their (non-optional) amount,
/// while dispute/resolve/chargeback reference an earlier transaction and carry
/// none. This makes "a dispute operation with an amount" or "a deposit without
/// one" unrepresentable inside the engine.
///
/// Deserialized directly from the flat CSV: the `type` column is the tag that
/// selects the variant, and (for deposit/withdrawal) the `amount` column fills
/// it. Lifecycle rows ignore any CSV `amount` field because the enum variants
/// carry no amount. An unknown `type` or a deposit/withdrawal with no amount
/// fails to deserialize and the row is skipped as malformed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum Operation {
    Deposit { amount: Decimal },
    Withdrawal { amount: Decimal },
    Dispute,
    Resolve,
    Chargeback,
}

impl Operation {
    /// The operation's name as it appears in the CSV `type` column — the single
    /// source of truth for referring to this kind in logs and error messages.
    pub fn name(&self) -> &'static str {
        match self {
            Operation::Deposit { .. } => "deposit",
            Operation::Withdrawal { .. } => "withdrawal",
            Operation::Dispute => "dispute",
            Operation::Resolve => "resolve",
            Operation::Chargeback => "chargeback",
        }
    }
}

/// A single transaction: who, which id, and what to do.
///
/// Deserializes straight from a flat CSV row — `client` and `tx` are their own
/// columns, and `operation` is flattened so the `type`/`amount` columns fold
/// into the [`Operation`] enum inline. The engine consumes this directly.
#[derive(Debug, Clone, Copy, Deserialize)]
pub struct Transaction {
    pub client: ClientId,
    #[serde(rename = "tx")]
    pub id: TxId,
    #[serde(flatten)]
    pub operation: Operation,
}

/// A stored accepted deposit or withdrawal, tagged with its dispute lifecycle
/// state. Only deposits advance beyond `Undisputed`; withdrawals are retained so
/// duplicate transaction IDs are rejected and withdrawal disputes can be reported
/// as not disputable.
#[derive(Debug, Clone, Copy)]
pub enum StoredTransaction {
    /// Not currently disputed (initial state, or returned here by a resolve).
    Undisputed(Transaction),
    /// Under dispute — its amount is currently held.
    Disputed(Transaction),
    /// Terminal: a chargeback has occurred; cannot be disputed again.
    ChargedBack(Transaction),
}

impl StoredTransaction {
    /// The original accepted transaction, regardless of lifecycle state.
    pub fn transaction(&self) -> &Transaction {
        match self {
            StoredTransaction::Undisputed(tx)
            | StoredTransaction::Disputed(tx)
            | StoredTransaction::ChargedBack(tx) => tx,
        }
    }
}

/// A client's account. `total` is intentionally not stored — it is derived as
/// `available + held` at output time so the two can never drift apart.
#[derive(Debug, Clone, Copy, Default)]
pub struct Account {
    available: Decimal,
    held: Decimal,
    locked: bool,
}

impl Account {
    pub fn available(&self) -> Decimal {
        self.available
    }

    pub fn held(&self) -> Decimal {
        self.held
    }

    pub fn is_locked(&self) -> bool {
        self.locked
    }

    /// Derived total: available funds plus funds held for disputes.
    pub fn total(&self) -> Result<Decimal, AccountArithmeticError> {
        checked_add(self.available, self.held)
    }

    pub fn deposit(&mut self, amount: Decimal) -> Result<(), AccountArithmeticError> {
        self.update(|next| {
            next.available = checked_add(next.available, amount)?;
            Ok(())
        })
    }

    pub fn withdrawal(&mut self, amount: Decimal) -> Result<(), AccountArithmeticError> {
        self.update(|next| {
            next.available = checked_sub(next.available, amount)?;
            Ok(())
        })
    }

    pub fn dispute(&mut self, amount: Decimal) -> Result<(), AccountArithmeticError> {
        self.update(|next| {
            next.available = checked_sub(next.available, amount)?;
            next.held = checked_add(next.held, amount)?;
            Ok(())
        })
    }

    pub fn resolve(&mut self, amount: Decimal) -> Result<(), AccountArithmeticError> {
        self.update(|next| {
            next.held = checked_sub(next.held, amount)?;
            next.available = checked_add(next.available, amount)?;
            Ok(())
        })
    }

    pub fn chargeback(&mut self, amount: Decimal) -> Result<(), AccountArithmeticError> {
        self.update(|next| {
            next.held = checked_sub(next.held, amount)?;
            next.locked = true;
            Ok(())
        })
    }

    fn update(
        &mut self,
        mutate: impl FnOnce(&mut Self) -> Result<(), AccountArithmeticError>,
    ) -> Result<(), AccountArithmeticError> {
        let mut next = *self;
        mutate(&mut next)?;
        next.total()?;
        *self = next;
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
#[error("account balance arithmetic overflow")]
pub struct AccountArithmeticError;

fn checked_add(lhs: Decimal, rhs: Decimal) -> Result<Decimal, AccountArithmeticError> {
    lhs.checked_add(rhs).ok_or(AccountArithmeticError)
}

fn checked_sub(lhs: Decimal, rhs: Decimal) -> Result<Decimal, AccountArithmeticError> {
    lhs.checked_sub(rhs).ok_or(AccountArithmeticError)
}

/// The output representation of an account — one row of the result CSV, and the
/// mirror image of [`Transaction`] on the write side. Monetary values are
/// pre-formatted strings so the exact decimal rendering is fixed here rather
/// than left to serde; `total` is the derived `available + held`.
#[derive(Debug, Serialize)]
pub struct AccountRecord {
    client: ClientId,
    available: String,
    held: String,
    total: String,
    locked: bool,
}

impl AccountRecord {
    /// Render `account` (owned by `client`) into its output-CSV form, formatting
    /// each monetary value to at most 4 decimal places.
    pub fn new(client: ClientId, account: &Account) -> Result<Self, AccountArithmeticError> {
        Ok(Self {
            client,
            available: format_amount(account.available()),
            held: format_amount(account.held()),
            total: format_amount(account.total()?),
            locked: account.is_locked(),
        })
    }
}

/// Render a monetary value to at most 4 decimal places, dropping trailing zeros
/// but always keeping at least one (e.g. `0.0`, `1.5`, `3.2344`).
fn format_amount(value: Decimal) -> String {
    let rounded = value.round_dp(4).normalize();
    if rounded.scale() == 0 {
        format!("{rounded}.0")
    } else {
        rounded.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal_macros::dec;

    #[test]
    fn formats_keep_one_decimal_place() {
        assert_eq!(format_amount(dec!(0)), "0.0");
        assert_eq!(format_amount(dec!(2.0)), "2.0");
        assert_eq!(format_amount(dec!(1.5)), "1.5");
        assert_eq!(format_amount(dec!(-60.0)), "-60.0");
        assert_eq!(format_amount(dec!(3.2344)), "3.2344");
    }

    #[test]
    fn formats_round_to_four_places() {
        assert_eq!(format_amount(dec!(1.23456)), "1.2346");
    }

    #[test]
    fn account_deposit_overflow_is_fallible_and_atomic() {
        let mut account = Account::default();

        account.deposit(Decimal::MAX).unwrap();
        let err = account.deposit(dec!(1.0)).unwrap_err();

        assert_eq!(err, AccountArithmeticError);
        assert_eq!(account.available(), Decimal::MAX);
        assert_eq!(account.held(), Decimal::ZERO);
    }

    #[test]
    fn account_dispute_overflow_is_fallible_and_atomic() {
        let mut account = Account::default();
        account.deposit(Decimal::MAX).unwrap();
        account.dispute(Decimal::MAX).unwrap();

        let err = account.dispute(dec!(1.0)).unwrap_err();

        assert_eq!(err, AccountArithmeticError);
        assert_eq!(account.available(), Decimal::ZERO);
        assert_eq!(account.held(), Decimal::MAX);
        assert!(!account.is_locked());
    }

    #[test]
    fn account_chargeback_locks_in_the_same_atomic_update() {
        let mut account = Account {
            available: Decimal::ZERO,
            held: Decimal::MIN,
            locked: false,
        };

        let err = account.chargeback(dec!(1.0)).unwrap_err();

        assert_eq!(err, AccountArithmeticError);
        assert_eq!(account.held(), Decimal::MIN);
        assert!(!account.is_locked());
    }

    #[test]
    fn account_total_overflow_is_fallible() {
        let account = Account {
            available: Decimal::MAX,
            held: dec!(1.0),
            locked: false,
        };

        assert_eq!(account.total(), Err(AccountArithmeticError));
        assert!(AccountRecord::new(ClientId(1), &account).is_err());
    }
}
