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
