# policy-engine

A configurable policy composition contract that evaluates denylist and jurisdiction checks together.

| Method | Purpose |
| --- | --- |
| `initialize(admin, combine_op)` | Initialize governance and combine semantics. |
| `propose_upgrade(admin, new_wasm, delay_ledgers)` | Propose an upgrade that is installable after the ledger delay. |
| `commit_upgrade(admin)` | Install the pending upgrade after its delay. |
| `cancel_upgrade(admin)` | Cancel a pending upgrade. |
| `add_check(admin, check)` | Add a configured compliance check. |
| `remove_check(admin, index)` | Remove a configured check. |
| `evaluate(from, to) -> bool` | Return the combined policy decision. |
| `evaluate_verbose(from, to) -> (bool, Option<CheckFailure>)` | Return the combined policy decision together with failure details when the policy rejects. |
| `get_checks()` | Read the configured checks. |
| `get_op()` | Read whether checks are combined with all/any semantics. |
| `is_paused()` | Read whether the policy engine is currently paused. |

## Failure reporting semantics

`evaluate_verbose` returns the combined decision plus an optional `CheckFailure` describing why the policy rejected. The reported failure depends on the combine operation:

- **`All`**: the policy fails as soon as a check fails, so the first failing check is the meaningful cause. The returned `CheckFailure` carries that check's `check_index` and `kind`.
- **`Any`**: the policy only fails when *every* check fails, so no single check is to blame. In this case the returned `CheckFailure` uses `check_index = u32::MAX` (a sentinel meaning "no specific check") and `kind = "AllFailed"`, indicating that all checks failed.

When the policy accepts, the `Option<CheckFailure>` is `None`.