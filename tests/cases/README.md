# Test Cases

Golden test cases for the toy payments engine. Each case is a pair:

- `NN-name.input.csv` — transactions fed to the engine (`cargo run -- <input> > out.csv`)
- `NN-name.expected.csv` — the account states the engine must produce

Every case exercises a specific rule or one of the adopted assumptions documented
in [`docs/HANDOVER.md`](../../docs/HANDOVER.md).

## Note on comparison

Per the spec, **decimal formatting is flexible** (`2` and `2.0` are equivalent) and
**row ordering does not matter**. The expected files use a clean, minimal-decimal
form and sort clients ascending. A test harness should compare **semantically**:
parse both sides, key rows by `client`, and compare amounts as decimals — not by
byte-for-byte string equality. Precision is significant only up to **4 decimal
places**.

## Cases

| #  | Case | What it tests | Expected outcome |
| --- | --- | --- | --- |
| 01 | basic-deposits-withdrawals | Core deposit/withdrawal maths across two clients (the spec's verified example). | C1 `1.0+2.0-1.5=1.5`; C2's `3.0` withdrawal fails (only `2.0` available), stays `2.0`. |
| 02 | withdrawal-insufficient-funds | A withdrawal larger than `available` **silently fails** and leaves `total` unchanged. | Balance stays `5.0`. |
| 03 | dispute-holds-funds | `dispute` moves the referenced amount from `available` to `held`; `total` unchanged. | `available 0`, `held 10`, `total 10`. |
| 04 | resolve-releases-funds | `resolve` returns held funds to `available` and clears the dispute. | Back to `available 10`, `held 0`. |
| 05 | chargeback-freezes-account | `chargeback` withdraws held funds (`held` and `total` drop) and **locks** the account. | `available 0`, `held 0`, `total 0`, `locked true`. |
| 06 | dispute-nonexistent-tx-ignored | A `dispute` referencing an unknown `tx` is ignored (no such transaction). | Balance stays `5.0`. |
| 07 | resolve-without-dispute-ignored | A `resolve` on a tx that exists but is **not under dispute** is ignored. | Balance stays `5.0`. |
| 08 | chargeback-without-dispute-ignored | A `chargeback` on a tx that is **not under dispute** is ignored; account stays unlocked. | Balance stays `5.0`, `locked false`. |
| 09 | cross-client-dispute-mismatch-ignored | Dispute whose `client` doesn't own the referenced `tx` is ignored (Assumption #4). | Both clients unchanged at `5.0`. |
| 10 | dispute-withdrawal-ignored | Only deposits are disputable; a dispute referencing a withdrawal is ignored (Assumption #1). | Disputing the `4.0` withdrawal is a no-op: `available 6`, `held 0`, `total 6`. |
| 11 | redispute-after-resolve | A tx may be disputed again **after** it was resolved (Assumption #2). | Second dispute re-holds: `available 0`, `held 10`. |
| 12 | no-dispute-after-chargeback | A charged-back tx is final and cannot be re-disputed (Assumption #2). | Second dispute ignored; state identical to case 05. |
| 13 | locked-account-rejects-new-tx | A locked account rejects later deposits and withdrawals (Assumption #3). | Post-chargeback deposit/withdrawal ignored; `total 0`, `locked true`. |
| 14 | negative-available-during-dispute | Disputing a spent deposit may drive `available` negative — the bank holds the full amount (Assumption #5). | `available -5`, `held 10`, `total 5`. |
| 15 | duplicate-tx-id-ignored | A repeated `tx` ID is ignored on its second occurrence (Assumption #6). | Only the first `5.0` deposit counts. |
| 16 | whitespace-and-precision | Leading whitespace is trimmed and 4-dp amounts are handled exactly (no float drift). | `1.2345 + 2.0001 - 0.0002 = 3.2344`. |
| 17 | unknown-type-skipped | An unknown transaction `type` is skipped, run continues (Assumption #7). | Only the two valid deposits count: `7.0`. |
| 18 | deposit-missing-amount-skipped | A deposit with an empty `amount` is invalid and skipped, run continues. | Only the two valid deposits count: `8.0`. |
| 19 | fraud-chargeback-negative-balance | The spec's fraud scenario: deposit, withdraw, then chargeback the deposit. | Fraudster's account ends `available -60`, `total -60`, `locked true`. |
| 20 | multi-client-unordered-ids | Interleaved, non-sorted client/tx IDs processed independently; dispute+resolve mid-stream. | C1 `15.0`, C2 `20-5=15.0`, C3 `7.5`; all unlocked. |
| 21 | double-dispute-ignored | A tx already under dispute cannot be disputed again before resolve/chargeback. | Second dispute is ignored; `available 0`, `held 10`. |
| 22 | unknown-resolve-chargeback-ignored | `resolve`/`chargeback` referencing unknown tx IDs are ignored. | Balance stays `5.0`, account unlocked. |
| 23 | cross-client-resolve-chargeback-mismatch-ignored | `resolve`/`chargeback` rows must match the original tx owner, not just the tx ID. | Cross-client actions are ignored; the owner can still resolve normally. |
| 24 | locked-account-resolves-existing-dispute | A locked account rejects new money movement but can still resolve an existing dispute. | Remaining held funds are released; account stays locked with `total 20`. |
| 25 | negative-amounts-skipped | Negative deposits and withdrawals are rejected and skipped. | Only valid deposits count: `6.0`. |
| 26 | zero-amounts-accepted | Zero-value deposits and withdrawals are valid no-ops for balance math. | Balance stays `5.0`. |
| 27 | withdrawal-missing-amount-skipped | A withdrawal with an empty `amount` is invalid and skipped, run continues. | Valid deposits total `7.0`. |
| 28 | locked-account-rejects-new-dispute | A locked account rejects a new dispute on a previously undisputed deposit. | Post-lock dispute ignored; `available 20`, `held 0`, `locked true`. |
| 29 | empty-input-header-only | An input with only the transaction header still produces a valid account CSV. | Output contains only `client,available,held,total,locked`. |
| 30 | all-invalid-input-header-only | If every data row is malformed, valid later output still has the account CSV header. | Output contains only `client,available,held,total,locked`. |
