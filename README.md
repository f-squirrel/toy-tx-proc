# Toy Payments Engine

A Rust CLI that reads transaction rows from a CSV file, applies them to client
accounts, handles disputes/resolves/chargebacks, and writes final account
balances as CSV.

```bash
cargo run -- transactions.csv > accounts.csv
```

The input file is the first and only CLI argument. `stdout` is reserved for the
account CSV. Malformed rows and rule violations are logged to `stderr` and
skipped so valid later rows can still be processed.

## Basics

The project is a standard Cargo crate and exposes the required interface:

```bash
cargo build
cargo run -- transactions.csv > accounts.csv
```

Input columns are `type`, `client`, `tx`, and `amount`. Output columns are
`client`, `available`, `held`, `total`, and `locked`.

## Completeness

All required transaction types are implemented:

| Type         | Behavior                                                                                              |
|--------------|-------------------------------------------------------------------------------------------------------|
| `deposit`    | Increases `available`; `total` increases because it is derived from `available + held`.               |
| `withdrawal` | Decreases `available` if sufficient funds exist; otherwise the row is skipped.                        |
| `dispute`    | References an earlier deposit, moves its amount from `available` to `held`, leaves `total` unchanged. |
| `resolve`    | Releases a currently disputed deposit, moving its amount from `held` back to `available`.             |
| `chargeback` | Finalizes a dispute, removes the held amount from the account, and locks the account.                 |

## Correctness

Money uses `rust_decimal::Decimal`, not floating point. Balance arithmetic is
checked and applied atomically: if an operation would overflow, balances and
stored transaction state are left unchanged and the row is skipped.

`total` is never stored separately; it is derived as `available + held` when
rendering output. This prevents `total`, `available`, and `held` from drifting
out of sync.

The type system carries part of the validation:

- `ClientId` and `TxId` are distinct newtypes over `u16` and `u32`.
- `Operation` is a tagged enum, so deposits and withdrawals require an amount,
  while dispute lifecycle rows are represented without one.
- Stored transactions are tagged by lifecycle state:
  `Undisputed`, `Disputed`, or `ChargedBack`.

## Safety and Robustness

The production code uses no `unsafe`. Bad input is handled as data, not as a
reason to panic:

- malformed CSV rows, unknown transaction types, missing deposit/withdrawal
  amounts, negative amounts, duplicate accepted transaction IDs, insufficient
  funds, client mismatches, and invalid dispute transitions are skipped and
  logged to `stderr`;
- diagnostics never go to `stdout`, so redirected output remains valid CSV;
- missing CLI arguments, too many CLI arguments, unreadable input files, and
  output write failures are fatal and exit non-zero.

## Efficiency

Input processing is streaming. The CSV parser yields one row at a time and the
engine applies it immediately; the full input file is never loaded into memory.
The reader accepts any `std::io::Read`, so the same library path could be wired
to a file, stdin, an in-memory buffer, or a TCP stream.

The engine keeps only the state needed after each row:

- one account record per client;
- a transaction map for accepted deposits and withdrawals, keyed by `tx`, so
  future dispute/resolve/chargeback rows can find the original amount and owner.

The transaction map is the main memory tradeoff. It is necessary because dispute
rows do not carry an amount. If this were bundled into a long-running server
processing many concurrent streams, each stream would need its own engine state
or an external store; for production-scale retention, the transaction map should
be backed by a database, bounded cache, or partitioned ledger rather than an
unbounded in-process `HashMap`.

## Maintainability

The binary is thin and the processing logic is reusable:

- `src/main.rs` handles CLI arguments, logging, opening the input file, and
  writing to stdout.
- `src/lib.rs` exposes reusable modules.
- `src/model.rs` defines transaction, account, and output-record types.
- `src/engine.rs` owns the account state machine.
- `src/io.rs` handles CSV parsing and writing.

The core engine is independent of files and stdout, which keeps business rules
testable without spawning the CLI. Comments are reserved for invariants and
non-obvious choices rather than restating each line of code.

## Assumptions

These are the assumptions used by this implementation, including both the
challenge-provided assumptions and project-specific decisions where the
requirements leave room for interpretation.

1. Each client has a single asset account.
2. Accounts are created when a syntactically valid transaction row references a
   new client. Parse-level malformed rows do not create accounts; a first
   withdrawal that fails for insufficient funds may still leave a zero-balance
   account row because the client was referenced.
3. Client IDs are valid `u16` values and transaction IDs are valid `u32` values.
4. Rows are processed chronologically in file order; client IDs and transaction
   IDs do not need to be sorted.
5. Output row order is not semantically important, but this implementation sorts
   clients by ID for deterministic output.
6. Output always includes the account CSV header, even when the input is empty
   or every data row is skipped and there are no account rows.
7. Input amounts are expected to have at most four decimal places. Output is
   rendered with at most four decimal places and at least one decimal digit.
8. Deposits and withdrawals require an `amount`. Dispute, resolve, and
   chargeback rows get their amount from the referenced stored transaction.
9. Negative deposit and withdrawal amounts are rejected. Zero-value deposits and
   withdrawals are accepted as no-ops for balance math.
10. Only deposits are disputable. Withdrawals are stored only to detect duplicate
   accepted transaction IDs; a dispute referencing a withdrawal is ignored.
11. A dispute/resolve/chargeback must refer to a transaction owned by the same
    client. Cross-client references are ignored.
12. A transaction can have only one active dispute at a time. After a resolve it
    can be disputed again. After a chargeback it is terminal.
13. A chargeback locks the account. Locked accounts reject later deposits,
    withdrawals, and new disputes. Resolves and chargebacks on already-disputed
    transactions can still proceed. Once locked, the account remains locked.
14. Disputing a deposit can make `available` negative if the client has already
    spent or withdrawn part of that deposit. This preserves the full hold amount
    for the dispute.
15. A chargeback of a disputed deposit can make `total` negative in the fraud
    scenario where the client has already withdrawn funds from the disputed
    deposit.
16. If a duplicate accepted transaction ID appears, the later deposit or
    withdrawal is ignored. A failed row does not reserve its transaction ID.
17. Unknown transaction types, malformed rows, and rule violations are partner
    or input errors; they are skipped rather than aborting the whole file.
18. Whitespace around CSV fields is accepted.

## Testing

Run the full verification suite with:

```bash
cargo build
cargo test
cargo fmt --check
cargo clippy -- -D warnings
```

The project is tested at three levels:

- Unit tests in `src/engine.rs` cover account state transitions, insufficient
  funds, duplicate IDs, cross-client disputes, withdrawal disputes, redispute
  rules, locked accounts, negative amounts, and atomic overflow handling.
- Unit tests in `src/io.rs` and `src/model.rs` cover CSV deserialization,
  required amounts, unknown transaction types, output formatting, derived
  totals, and checked arithmetic.
- Integration tests in `tests/golden.rs` auto-discover every
  `tests/cases/*.input.csv` file, run the compiled binary end to end, and compare
  stdout directly with the matching `*.expected.csv` file, ignoring only a
  trailing-newline difference.

There are 30 golden cases documented in `tests/cases/README.md`. They include
the sample deposit/withdrawal scenario, all dispute lifecycle operations,
insufficient funds, missing and unknown rows, whitespace and precision, duplicate
IDs, cross-client mismatches, account locking, negative balances after fraud,
and zero/negative amount handling.

`tests/cli.rs` covers CLI behavior that golden CSV cases do not: missing
arguments, too many arguments, missing files, stderr logging, and ensuring that
warnings never pollute stdout.

## Dependencies

| Crate                 | Purpose                                            |
|-----------------------|----------------------------------------------------|
| `serde`               | CSV record serialization/deserialization.          |
| `csv`                 | Streaming CSV reader and writer.                   |
| `rust_decimal`        | Exact decimal arithmetic for monetary values.      |
| `thiserror`           | Typed errors for skipped rows and output failures. |
| `log` + `env_logger`  | Diagnostics routed to `stderr`.                    |
| `rust_decimal_macros` | Decimal literals in tests.                         |
