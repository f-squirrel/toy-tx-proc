use std::collections::HashMap;

use rust_decimal::Decimal;
use thiserror::Error;

use crate::model::{
    Account, AccountArithmeticError, ClientId, DepositState, Operation, StoredPayment,
    StoredTransaction, Transaction, TxId,
};

/// Reasons a single row was skipped.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum EngineError {
    #[error("tx {tx}: amount must be non-negative")]
    NegativeAmount { tx: TxId },

    #[error("tx {tx}: duplicate transaction id, ignoring")]
    DuplicateTx { tx: TxId },

    #[error("client {client}: account is locked, rejecting {kind}")]
    AccountLocked {
        client: ClientId,
        kind: &'static str,
    },

    #[error("tx {tx}: withdrawal of {amount} exceeds available funds")]
    InsufficientFunds { tx: TxId, amount: Decimal },

    #[error("tx {tx}: account arithmetic failed: {source}")]
    AccountArithmetic {
        tx: TxId,
        source: AccountArithmeticError,
    },

    #[error("tx {tx}: referenced transaction does not exist")]
    UnknownTx { tx: TxId },

    #[error("tx {tx}: dispute client {client} does not own the referenced transaction")]
    ClientMismatch { tx: TxId, client: ClientId },

    #[error("tx {tx}: only deposits can be disputed, not a withdrawal")]
    NotDisputable { tx: TxId },

    #[error("tx {tx}: transaction is not in a valid state for {kind}")]
    InvalidDisputeState { tx: TxId, kind: &'static str },
}

/// Per-client accounts plus accepted deposits/withdrawals.
///
/// Withdrawals are stored even though only deposits can be disputed, so
/// duplicate IDs are rejected and withdrawal disputes get a specific error.
#[derive(Debug, Default)]
pub struct PaymentsEngine {
    accounts: HashMap<ClientId, Account>,
    transactions: HashMap<TxId, StoredTransaction>,
}

impl PaymentsEngine {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn accounts(&self) -> &HashMap<ClientId, Account> {
        &self.accounts
    }

    /// Apply one transaction. On `Err`, balances and stored transactions are not
    /// updated.
    ///
    /// A syntactically valid first transaction can still create a zero-balance
    /// account before failing a business rule such as insufficient funds.
    pub fn process(&mut self, tx: Transaction) -> Result<(), EngineError> {
        // The operation names itself, so error messages stay in sync with the
        // `type` column without repeating string literals at each call site.
        let kind = tx.operation.name();
        match tx.operation {
            Operation::Deposit { amount } => self.deposit(tx, amount, kind),
            Operation::Withdrawal { amount } => self.withdrawal(tx, amount, kind),
            Operation::Dispute => self.dispute(tx.client, tx.id, kind),
            Operation::Resolve => self.resolve(tx.client, tx.id, kind),
            Operation::Chargeback => self.chargeback(tx.client, tx.id, kind),
        }
    }

    fn deposit(
        &mut self,
        tx: Transaction,
        amount: Decimal,
        kind: &'static str,
    ) -> Result<(), EngineError> {
        let account = self.begin_transaction(tx.client, tx.id, amount, kind)?;
        account
            .deposit(amount)
            .map_err(|source| EngineError::AccountArithmetic { tx: tx.id, source })?;
        self.transactions.insert(
            tx.id,
            StoredTransaction::Deposit {
                payment: StoredPayment::new(tx.client, amount),
                state: DepositState::Undisputed,
            },
        );
        Ok(())
    }

    fn withdrawal(
        &mut self,
        tx: Transaction,
        amount: Decimal,
        kind: &'static str,
    ) -> Result<(), EngineError> {
        let account = self.begin_transaction(tx.client, tx.id, amount, kind)?;
        if account.available() < amount {
            return Err(EngineError::InsufficientFunds { tx: tx.id, amount });
        }
        account
            .withdrawal(amount)
            .map_err(|source| EngineError::AccountArithmetic { tx: tx.id, source })?;
        self.transactions
            .insert(tx.id, StoredTransaction::Withdrawal { client: tx.client });
        Ok(())
    }

    fn dispute(
        &mut self,
        client: ClientId,
        id: TxId,
        kind: &'static str,
    ) -> Result<(), EngineError> {
        let (amount, next) = {
            let stored = self.referenced_tx(id, client)?;
            // Only deposits are disputable: charging back a withdrawal would remove
            // funds the client already received.
            match stored {
                StoredTransaction::Deposit {
                    payment,
                    state: DepositState::Undisputed,
                } => {
                    let amount = payment.amount();
                    (
                        amount,
                        StoredTransaction::Deposit {
                            payment: *payment,
                            state: DepositState::Disputed,
                        },
                    )
                }
                StoredTransaction::Withdrawal { .. } => {
                    return Err(EngineError::NotDisputable { tx: id });
                }
                _ => {
                    return Err(EngineError::InvalidDisputeState { tx: id, kind });
                }
            }
        };
        let account = self.accounts.entry(client).or_default();
        if account.is_locked() {
            return Err(EngineError::AccountLocked { client, kind });
        }
        account
            .dispute(amount)
            .map_err(|source| EngineError::AccountArithmetic { tx: id, source })?;
        self.transactions.insert(id, next);
        Ok(())
    }

    fn resolve(
        &mut self,
        client: ClientId,
        id: TxId,
        kind: &'static str,
    ) -> Result<(), EngineError> {
        let (amount, next) = {
            let stored = self.referenced_tx(id, client)?;
            match stored {
                StoredTransaction::Deposit {
                    payment,
                    state: DepositState::Disputed,
                } => {
                    let amount = payment.amount();
                    (
                        amount,
                        StoredTransaction::Deposit {
                            payment: *payment,
                            state: DepositState::Undisputed,
                        },
                    )
                }
                _ => {
                    return Err(EngineError::InvalidDisputeState { tx: id, kind });
                }
            }
        };
        let account = self.accounts.entry(client).or_default();
        account
            .resolve(amount)
            .map_err(|source| EngineError::AccountArithmetic { tx: id, source })?;
        self.transactions.insert(id, next);
        Ok(())
    }

    fn chargeback(
        &mut self,
        client: ClientId,
        id: TxId,
        kind: &'static str,
    ) -> Result<(), EngineError> {
        let (amount, next) = {
            let stored = self.referenced_tx(id, client)?;
            match stored {
                StoredTransaction::Deposit {
                    payment,
                    state: DepositState::Disputed,
                } => {
                    let amount = payment.amount();
                    (
                        amount,
                        StoredTransaction::Deposit {
                            payment: *payment,
                            state: DepositState::ChargedBack,
                        },
                    )
                }
                _ => {
                    return Err(EngineError::InvalidDisputeState { tx: id, kind });
                }
            }
        };
        let account = self.accounts.entry(client).or_default();
        account
            .chargeback(amount)
            .map_err(|source| EngineError::AccountArithmetic { tx: id, source })?;
        self.transactions.insert(id, next);
        Ok(())
    }

    fn begin_transaction(
        &mut self,
        client: ClientId,
        id: TxId,
        amount: Decimal,
        kind: &'static str,
    ) -> Result<&mut Account, EngineError> {
        if amount < Decimal::ZERO {
            return Err(EngineError::NegativeAmount { tx: id });
        }
        if self.transactions.contains_key(&id) {
            return Err(EngineError::DuplicateTx { tx: id });
        }
        let account = self.accounts.entry(client).or_default();
        if account.is_locked() {
            return Err(EngineError::AccountLocked { client, kind });
        }
        Ok(account)
    }

    fn referenced_tx(
        &mut self,
        tx: TxId,
        client: ClientId,
    ) -> Result<&mut StoredTransaction, EngineError> {
        let stored = self
            .transactions
            .get_mut(&tx)
            .ok_or(EngineError::UnknownTx { tx })?;
        if stored.client() != client {
            return Err(EngineError::ClientMismatch { tx, client });
        }
        Ok(stored)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal_macros::dec;

    fn raw(operation: Operation, client: u16, tx: u32) -> Transaction {
        Transaction {
            client: ClientId(client),
            id: TxId(tx),
            operation,
        }
    }

    fn deposit(engine: &mut PaymentsEngine, client: u16, tx: u32, amount: Decimal) {
        engine
            .process(raw(Operation::Deposit { amount }, client, tx))
            .unwrap();
    }

    fn account(engine: &PaymentsEngine, client: u16) -> &Account {
        &engine.accounts()[&ClientId(client)]
    }

    #[test]
    fn deposit_credits_available() {
        let mut engine = PaymentsEngine::new();
        deposit(&mut engine, 1, 1, dec!(10.0));
        let acct = account(&engine, 1);
        assert_eq!(acct.available(), dec!(10.0));
        assert_eq!(acct.total().unwrap(), dec!(10.0));
    }

    #[test]
    fn withdrawal_over_balance_fails() {
        let mut engine = PaymentsEngine::new();
        deposit(&mut engine, 1, 1, dec!(5.0));
        let err = engine
            .process(raw(Operation::Withdrawal { amount: dec!(10.0) }, 1, 2))
            .unwrap_err();
        assert!(matches!(err, EngineError::InsufficientFunds { .. }));
        assert_eq!(account(&engine, 1).available(), dec!(5.0));
    }

    #[test]
    fn dispute_holds_then_resolve_releases() {
        let mut engine = PaymentsEngine::new();
        deposit(&mut engine, 1, 1, dec!(10.0));
        engine.process(raw(Operation::Dispute, 1, 1)).unwrap();
        let acct = account(&engine, 1);
        assert_eq!(acct.available(), dec!(0.0));
        assert_eq!(acct.held(), dec!(10.0));
        engine.process(raw(Operation::Resolve, 1, 1)).unwrap();
        let acct = account(&engine, 1);
        assert_eq!(acct.available(), dec!(10.0));
        assert_eq!(acct.held(), dec!(0.0));
    }

    #[test]
    fn chargeback_locks_and_rejects_further_tx() {
        let mut engine = PaymentsEngine::new();
        deposit(&mut engine, 1, 1, dec!(10.0));
        engine.process(raw(Operation::Dispute, 1, 1)).unwrap();
        engine.process(raw(Operation::Chargeback, 1, 1)).unwrap();
        assert!(account(&engine, 1).is_locked());
        let err = engine
            .process(raw(Operation::Deposit { amount: dec!(5.0) }, 1, 2))
            .unwrap_err();
        assert!(matches!(err, EngineError::AccountLocked { .. }));
    }

    #[test]
    fn dispute_of_withdrawal_is_ignored() {
        let mut engine = PaymentsEngine::new();
        deposit(&mut engine, 1, 1, dec!(10.0));
        engine
            .process(raw(Operation::Withdrawal { amount: dec!(4.0) }, 1, 2))
            .unwrap();
        let err = engine.process(raw(Operation::Dispute, 1, 2)).unwrap_err();
        assert!(matches!(err, EngineError::NotDisputable { .. }));
        let acct = account(&engine, 1);
        assert_eq!(acct.available(), dec!(6.0));
        assert_eq!(acct.held(), dec!(0.0));
        assert_eq!(acct.total().unwrap(), dec!(6.0));
    }

    #[test]
    fn cross_client_dispute_is_ignored() {
        let mut engine = PaymentsEngine::new();
        deposit(&mut engine, 1, 1, dec!(5.0));
        let err = engine.process(raw(Operation::Dispute, 2, 1)).unwrap_err();
        assert!(matches!(err, EngineError::ClientMismatch { .. }));
        assert_eq!(account(&engine, 1).available(), dec!(5.0));
    }

    #[test]
    fn duplicate_tx_id_is_ignored() {
        let mut engine = PaymentsEngine::new();
        deposit(&mut engine, 1, 1, dec!(5.0));
        let err = engine
            .process(raw(
                Operation::Deposit {
                    amount: dec!(100.0),
                },
                1,
                1,
            ))
            .unwrap_err();
        assert!(matches!(err, EngineError::DuplicateTx { .. }));
        assert_eq!(account(&engine, 1).available(), dec!(5.0));
    }

    #[test]
    fn redispute_allowed_after_resolve() {
        let mut engine = PaymentsEngine::new();
        deposit(&mut engine, 1, 1, dec!(10.0));
        engine.process(raw(Operation::Dispute, 1, 1)).unwrap();
        engine.process(raw(Operation::Resolve, 1, 1)).unwrap();
        engine.process(raw(Operation::Dispute, 1, 1)).unwrap();
        assert_eq!(account(&engine, 1).held(), dec!(10.0));
    }

    #[test]
    fn no_dispute_after_chargeback() {
        let mut engine = PaymentsEngine::new();
        deposit(&mut engine, 1, 1, dec!(10.0));
        engine.process(raw(Operation::Dispute, 1, 1)).unwrap();
        engine.process(raw(Operation::Chargeback, 1, 1)).unwrap();
        let err = engine.process(raw(Operation::Dispute, 1, 1)).unwrap_err();
        assert!(matches!(err, EngineError::InvalidDisputeState { .. }));
    }

    #[test]
    fn locked_account_rejects_new_dispute() {
        let mut engine = PaymentsEngine::new();
        deposit(&mut engine, 1, 1, dec!(10.0));
        deposit(&mut engine, 1, 2, dec!(20.0));
        engine.process(raw(Operation::Dispute, 1, 1)).unwrap();
        engine.process(raw(Operation::Chargeback, 1, 1)).unwrap();

        let err = engine.process(raw(Operation::Dispute, 1, 2)).unwrap_err();

        assert!(matches!(err, EngineError::AccountLocked { .. }));
        let acct = account(&engine, 1);
        assert_eq!(acct.available(), dec!(20.0));
        assert_eq!(acct.held(), dec!(0.0));
        assert_eq!(acct.total().unwrap(), dec!(20.0));
        assert!(acct.is_locked());
    }

    #[test]
    fn negative_amount_is_rejected() {
        let mut engine = PaymentsEngine::new();
        let err = engine
            .process(raw(Operation::Deposit { amount: dec!(-1.0) }, 1, 1))
            .unwrap_err();
        assert!(matches!(err, EngineError::NegativeAmount { .. }));
        assert!(engine.accounts().get(&ClientId(1)).is_none());
    }

    #[test]
    fn deposit_overflowing_total_is_rejected_atomically() {
        let mut engine = PaymentsEngine::new();
        deposit(&mut engine, 1, 1, Decimal::MAX);
        engine.process(raw(Operation::Dispute, 1, 1)).unwrap();

        let err = engine
            .process(raw(Operation::Deposit { amount: dec!(1.0) }, 1, 2))
            .unwrap_err();

        assert!(matches!(err, EngineError::AccountArithmetic { .. }));
        let acct = account(&engine, 1);
        assert_eq!(acct.available(), Decimal::ZERO);
        assert_eq!(acct.held(), Decimal::MAX);
        assert_eq!(acct.total().unwrap(), Decimal::MAX);

        engine
            .process(raw(Operation::Deposit { amount: dec!(0.0) }, 1, 2))
            .unwrap();
    }
}
