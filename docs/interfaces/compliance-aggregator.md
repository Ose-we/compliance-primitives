# compliance-aggregator

The aggregator combines configured denylist, jurisdiction, and allowlist checks
with AND semantics. The circuit breaker, when configured, runs first and
short-circuits a frozen result.

| Method | Purpose |
| --- | --- |
| `initialize(admin, denylist_gate, jurisdiction_flag, circuit_breaker)` | One-time setup; each check address is optional. |
| `set_allowlist(admin, allowlist)` / `clear_allowlist(admin)` | Configure or remove the `allowlist-token` check. |
| `check_address(address, allowed_jurisdictions)` | Return the overall boolean and per-check results. |
| `check_address_verbose(address, allowed_jurisdictions)` | Return the existing results plus `Option<CheckFailure { source, code }>` for the first failure. |
| `check_all(addresses, allowed_jurisdictions)` | Evaluate a batch while preserving the original per-address result shape. |
| `check_all_verbose(addresses, allowed_jurisdictions)` | Include optional first-failure source and code Symbols for each address. |
| `batch_check(addresses, allowed_jurisdictions)` | Bounded boolean-only batch API. |
| `get_checks_summary()` | Return `(denylist_configured, jurisdiction_configured, allowlist_configured)`. |

`check_address` and `check_all` retain their existing return shapes. Use the
verbose variants when callers need to explain a rejection; use
`batch_check` when only pass/fail booleans are needed.

Use the deployed contract's generated interface for exact enum and address
encoding.
