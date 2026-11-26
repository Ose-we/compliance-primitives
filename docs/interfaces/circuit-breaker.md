# circuit-breaker

An emergency control that lets an authorized operator stop and resume protected flows.

| Method | Purpose |
| --- | --- |
| `initialize(admin)` | Configure the administrator. |
| `is_frozen() -> bool` | Read the current breaker state. |
| `frozen_since() -> Option<u32>` | Returns the ledger sequence when frozen, or `None` if not frozen. |
| `freeze(caller)` | Enter the emergency-frozen state (admin or guardian). |
| `unfreeze(admin)` | Resume normal operation (admin only). |
| `upgrade(admin, new_wasm_hash)` | Admin-only. Move the contract code to a new WASM hash; storage is preserved. See the "Upgradeability" section in `contracts/circuit-breaker/src/lib.rs`. |

## Guardian management

Guardians are additional authorized operators that may freeze and unfreeze protected flows. The administrator manages the guardian list.

| Method | Purpose |
| --- | --- |
| `add_guardian(admin, guardian)` | Admin-only. Add `guardian` to the guardian list. Adding an existing guardian is a no-op (the list never contains duplicates). |
| `remove_guardian(admin, guardian)` | Admin-only. Remove `guardian` from the guardian list. Removing an absent guardian is a no-op. |
| `is_guardian(guardian) -> bool` | Read whether `guardian` is currently in the guardian list. |
| `freeze(caller)` | Freeze protected flows. Callable only by the admin or a current guardian. |
| `unfreeze(caller)` | Unfreeze protected flows. Callable only by the admin or a current guardian. |

### Invariants

- Only the admin or a current guardian may `freeze` or `unfreeze`; any other caller is rejected.
- The guardian list contains no duplicates.

## Fuzzing

The `circuit_breaker_guardian_sequences` fuzz target exercises random sequences of `add_guardian`, `remove_guardian`, `freeze`, and `unfreeze` calls issued by random callers (admin, guardians, and arbitrary accounts). It asserts the invariants above after every step:

- `freeze`/`unfreeze` succeed only when the caller is the admin or a current guardian, and fail otherwise.
- The guardian list never contains duplicates.