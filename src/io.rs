//! CSV input (streaming deserialize) and account CSV output.

use std::fs::File;
use std::io::{self, Read, Write};
use std::path::Path;

use csv::{DeserializeRecordsIntoIter, ReaderBuilder, Trim, WriterBuilder};
use thiserror::Error;

use crate::model::{Account, AccountArithmeticError, AccountRecord, ClientId, Transaction};

const ACCOUNT_HEADERS: [&str; 5] = ["client", "available", "held", "total", "locked"];

/// Open `path` and stream its rows as [`Transaction`]s.
pub fn read_transactions_from_path(
    path: &Path,
) -> io::Result<DeserializeRecordsIntoIter<File, Transaction>> {
    let file = File::open(path)?;
    Ok(read_transactions(file))
}

/// Stream transactions from any [`Read`] source.
///
/// Each row deserializes directly into a `Transaction`, with the `type` column
/// tagging the operation. The `amount` column is required for deposits and
/// withdrawals; dispute lifecycle rows do not carry an amount internally, so a
/// CSV amount on those rows is ignored. The iterator yields one `Result` per
/// row: malformed rows (bad numbers, unknown `type`, a deposit/withdrawal
/// missing its amount) surface as `Err`, which the caller logs and skips.
/// Whitespace around every field is trimmed, and `flexible(true)` lets
/// dispute/resolve/chargeback rows omit the trailing `amount` field without a
/// column-count error.
pub fn read_transactions<R: Read>(input: R) -> DeserializeRecordsIntoIter<R, Transaction> {
    ReaderBuilder::new()
        .trim(Trim::All)
        .flexible(true)
        .from_reader(input)
        .into_deserialize::<Transaction>()
}

/// Write all accounts as CSV to `out`, sorted ascending by client id for a
/// stable, deterministic ordering.
pub fn write_accounts<'a, W: Write>(
    out: W,
    accounts: impl Iterator<Item = (&'a ClientId, &'a Account)>,
) -> Result<(), WriteAccountsError> {
    let mut rows: Vec<(ClientId, &Account)> = accounts.map(|(id, a)| (*id, a)).collect();
    rows.sort_by_key(|(id, _)| id.0);

    let mut writer = WriterBuilder::new().has_headers(false).from_writer(out);
    writer.write_record(ACCOUNT_HEADERS)?;
    for (client, account) in rows {
        let record = AccountRecord::new(client, account)
            .map_err(|source| WriteAccountsError::AccountArithmetic { client, source })?;
        writer.serialize(record)?;
    }
    writer.flush()?;
    Ok(())
}

#[derive(Debug, Error)]
pub enum WriteAccountsError {
    #[error(transparent)]
    Csv(#[from] csv::Error),

    #[error(transparent)]
    Io(#[from] io::Error),

    #[error("client {client}: account arithmetic failed while rendering output: {source}")]
    AccountArithmetic {
        client: ClientId,
        source: AccountArithmeticError,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Operation;
    use rust_decimal_macros::dec;

    /// Deserialize a CSV body (with header) into transactions, exactly as
    /// `read_transactions` does but from an in-memory string.
    fn parse(body: &str) -> Vec<csv::Result<Transaction>> {
        let header = "type, client, tx, amount\n";
        ReaderBuilder::new()
            .trim(Trim::All)
            .flexible(true)
            .from_reader(format!("{header}{body}").into_bytes().as_slice())
            .into_deserialize::<Transaction>()
            .collect()
    }

    #[test]
    fn deposit_row_becomes_deposit_operation() {
        let tx = parse("deposit, 1, 7, 5.0\n").pop().unwrap().unwrap();
        assert_eq!(tx.operation, Operation::Deposit { amount: dec!(5.0) });
    }

    #[test]
    fn dispute_row_has_no_amount() {
        let tx = parse("dispute, 1, 7,\n").pop().unwrap().unwrap();
        assert_eq!(tx.operation, Operation::Dispute);
    }

    #[test]
    fn deposit_without_amount_is_rejected() {
        assert!(parse("deposit, 1, 7,\n").pop().unwrap().is_err());
    }

    #[test]
    fn unknown_type_is_rejected() {
        assert!(parse("foobar, 1, 7, 5.0\n").pop().unwrap().is_err());
    }

    #[test]
    fn write_accounts_emits_header_for_empty_accounts() {
        let accounts: Vec<(ClientId, Account)> = Vec::new();
        let mut output = Vec::new();

        write_accounts(
            &mut output,
            accounts.iter().map(|(client, account)| (client, account)),
        )
        .unwrap();

        assert_eq!(
            String::from_utf8(output).unwrap(),
            "client,available,held,total,locked\n"
        );
    }
}
