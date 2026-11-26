//! `compliance-aggregator` is a `#![no_std]` Soroban contract that batches
//! multiple compliance checks into a single cross-contract call surface.
//!
//! ## Motivation
//!
//! A consuming contract running multiple compliance checks today pays
//! separate host-level cross-contract call overheads per transfer. This
//! aggregator reduces that to **one**
//! call from the consumer's perspective: the consumer calls
//! `check_address` (or the batched `check_all`) on this contract and gets
//! back a single aggregated pass/fail result plus a per-check breakdown for
//! audit purposes.
//!
//! ## Callers
//!
//! - **Consumer contracts** (the primary callers) — typically a token's
//!   `transfer` path — invoke `check_address` for one address, or `check_all` /
//!   `batch_check` for several, before moving value, and refuse the transfer
//!   when the result is `false`. These entrypoints are permissionless and
//!   require no auth.
//! - **The admin** (an issuer key or, preferably, a `multisig-admin` contract
//!   address) calls `initialize`, the check-source configuration methods
//!   (`set_denylist_gate`, `set_jurisdiction_flag`, `set_allowlist`,
//!   `set_circuit_breaker`), `set_admin`, pause controls, and `upgrade`.
//!   Every mutation requires the admin's auth.
//! - **Off-chain tooling and auditors** call the read-only views
//!   (`get_admin`, `get_config`, `get_checks_summary`, `is_paused`) and consume the
//!   configuration events to reconstruct which checks were registered when.
//!
//! ## Composition
//!
//! | Contract | Relationship to `compliance-aggregator` |
//! |---|---|
//! | `denylist-gate` | Registered as `DenylistGate`; `check(address)` runs for every address. |
//! | `jurisdiction-flag` | Registered as `JurisdictionFlag`; `is_permitted_jurisdiction(address, allowed_codes)` runs for every address. A failed or erroring call counts as a failure. |
//! | `allowlist-token` | Registered as `AllowlistToken`; `is_allowed(address)` runs for every address. |
//! | `circuit-breaker` | Optional emergency stop. When configured and frozen, every address is denied without running any check. |
//! | `multisig-admin` | Recommended `admin` value, so reconfiguration needs M-of-N approval. |
//! | `compliance-pausable` | Shared crate used to guard the pause flag. |
//! | `policy-engine` | Sibling, not a dependency. This contract is a fixed AND-only fan-out optimised for call overhead; `policy-engine` is the configurable AND / OR layer. Pick one per integration. |
//! | `audit-log`, `compliance-check` | Not called by this contract. `audit-log` is the place to record on-chain trails of configuration changes; this contract emits events only. `compliance-check` is a compile-time trait crate and is not used at runtime. |
//!
//! Use this contract when a consumer needs an AND-combination of the
//! denylist, jurisdiction, and/or allowlist primitives, optionally gated by
//! the emergency breaker. Use `policy-engine` when the policy needs OR
//! branches or weights.
//!
//! ## Contract interfaces used
//!
//! Cross-contract calls are made via `#[contractclient]` traits declared in
//! this file. This follows the same pattern as
//! `/examples/denylist-gate-consumer`: we describe only the shape of the
//! remote call and do **not** import the peer crates as `[dependencies]`
//! (doing so would pull both contracts' wasm exports into this binary and
//! cause linker conflicts). The peer crates appear only in
//! `[dev-dependencies]` so their test utilities are available in `cfg(test)`.
//!
//! ## Configured checks
//!
//! The admin registers up to one address for each of the three compliance
//! sources, plus an optional circuit breaker:
//!
//! - `DenylistCheck` — calls `denylist-gate.check(address) -> bool`
//! - `JurisdictionCheck` — calls
//!   `jurisdiction-flag.is_permitted_jurisdiction(address, allowed_codes) -> bool`
//! - `AllowlistCheck` — calls `allowlist-token.is_allowed(address) -> bool`
//! - `CircuitBreaker` — calls `circuit-breaker.is_frozen() -> bool`; the check
//!   passes only while the breaker is not frozen. A frozen breaker denies
//!   without consulting the other checks.
//!
//! All checks are optional; if a check type's contract address has not been
//! registered the check is skipped (treated as passing). Registering at least
//! one check is enforced at call time.
//!
//! The existing `check_address` and `check_all` entrypoints keep their return
//! shapes. `check_address_verbose` and `check_all_verbose` additionally report
//! the first failed source and its stable reason code for wallet-facing
//! diagnostics.
//!
//! ## Choosing `check_address` vs. `check_all`
//!
//! Both entry points run the same checks per address; they differ in how the
//! fixed per-invocation overhead is amortised.
//!
//! | Cost component | `check_address` × N | `check_all` (N addresses) |
//! |---|---|---|
//! | Consumer → aggregator invocations | N | 1 |
//! | Aggregator instance-storage reads (gate, flag, breaker, allowlist) | 4 × N | 4 |
//! | `circuit-breaker.is_frozen` calls | N | 1 |
//! | Downstream primitive calls (denylist + jurisdiction + allowlist) | up to 3 × N | up to 3 × N |
//! | Return payload | N × `(bool, Vec<CheckResult>)` | 1 × `Vec<AddressCheckResult>` (also carries each `Address`) |
//!
//! The recorded `check_address` baseline (denylist and jurisdiction checks,
//! no breaker or allowlist)
//! is a ceiling of **~1.5M CPU instructions / ~400 KB memory** per call
//! (`[compliance-aggregator.check_address]` in `budget-baselines.toml`).
//! Almost all of that is the downstream calls, so the cost model is:
//!
//! - `check_address` × N ≈ N × (aggregator frame + 4 reads + up to 3 downstream calls)
//! - `check_all` ≈ 1 × (aggregator frame + 4 reads) + N × (up to 3 downstream calls)
//!
//! Guidance for a token's `transfer` path, at representative batch sizes:
//!
//! | Batch size | Typical shape | Recommendation |
//! |---|---|---|
//! | 1 | Single sender check | `check_address`: smallest return value, no `Address` echoed back. |
//! | 2 | Sender + recipient (the common `transfer` case) | Either. `check_all` saves one aggregator frame, 3 storage reads and one breaker call; `check_address` × 2 is simpler to branch on. The difference is small next to the 4 downstream calls both make. |
//! | 5–10 | Multi-party settlement, batch payouts | `check_all` (or `batch_check` if you only need booleans). The saved fixed overhead grows linearly with N. |
//! | 25–45 | Bulk distribution, airdrop pre-screen | `batch_check`, which caps input at `MAX_BATCH_SIZE` (45). `check_all` has no cap, and its per-address `Vec<CheckResult>` breakdown makes the return payload the largest memory cost at this size. Split anything above 45 across transactions. |
//!
//! Total cost at every size is dominated by downstream calls, which
//! neither entry point avoids. Batching only removes the per-invocation
//! overhead. Per-size figures for the configuration above come from
//! `bench_check_address_vs_check_all_batch_sizes` in `src/test.rs`:
//!
//! ```text
//! cargo test -p compliance-aggregator bench_check_address_vs_check_all -- --nocapture
//! ```
//!
//! See the "check_address vs. check_all" section of `BENCHMARKS.md`.
//!
//! ## Upgradeability
//!
//! `upgrade(admin, new_wasm_hash)` lets the configured admin move the
//! contract's code to a new WASM hash via
//! `env.deployer().update_current_contract_wasm`, following the same
//! admin-gated pattern used by `jurisdiction-flag` and `policy-engine`.
//! Only the stored admin may call it; any other caller is rejected with
//! `Error::NotAuthorized`, and calling it before `initialize` returns
//! `Error::NotInitialized`. An `UpgradePerformed` event is emitted so the
//! upgrade is visible to indexers.
//!
//! The WASM swap does not touch storage: the admin, the registered
//! `denylist-gate` / `jurisdiction-flag` / `circuit-breaker` addresses, and
//! the pause flag all live in instance storage under the same `DataKey`
//! variants and are read unchanged by the new code, so the contract is
//! callable immediately after the upgrade. Upgrade procedure:
//!
//! 1. Build the new release WASM
//!    (`stellar contract build` / `make build`).
//! 2. Upload it: `stellar contract upload --wasm compliance_aggregator.wasm`
//!    — this prints the new WASM hash.
//! 3. Invoke `upgrade --admin <ADMIN> --new_wasm_hash <HASH>` on the
//!    deployed aggregator, signed by the admin (or the `multisig-admin`
//!    quorum if the admin is a multisig).
//!
//! A new version **must not** reorder or remove existing `DataKey` variants
//! or change the types stored under them; add new variants instead. See
//! `STORAGE_VERSIONING.md` for the repo-wide rules.
#![no_std]

use soroban_sdk::{
    contract, contracterror, contractevent, contractimpl, contracttype, Address, BytesN, Env,
    String, Symbol, Vec,
};

// ---------------------------------------------------------------------------
// Cross-contract client interfaces
// ---------------------------------------------------------------------------

/// Subset of the `denylist-gate` interface that this aggregator uses.
/// Must match the actual contract's exported function signature exactly.
#[soroban_sdk::contractclient(name = "DenylistGateClient")]
pub trait DenylistGateInterface {
    fn check(env: Env, address: Address) -> bool;
}

/// Subset of the `jurisdiction-flag` interface that this aggregator uses.
/// Must match the actual contract's exported function signature exactly.
#[soroban_sdk::contractclient(name = "JurisdictionFlagClient")]
pub trait JurisdictionFlagInterface {
    fn is_permitted_jurisdiction(env: Env, address: Address, allowed_codes: Vec<String>) -> bool;
}

/// Subset of the `circuit-breaker` interface that this aggregator uses.
/// Must match the actual contract's exported function signature exactly.
#[soroban_sdk::contractclient(name = "CircuitBreakerClient")]
pub trait CircuitBreakerInterface {
    fn is_frozen(env: Env) -> bool;
}

/// Subset of the `allowlist-token` interface used by this aggregator.
#[soroban_sdk::contractclient(name = "AllowlistTokenClient")]
pub trait AllowlistTokenInterface {
    fn is_allowed(env: Env, address: Address) -> bool;
}

// ---------------------------------------------------------------------------
// TTL policy
// ---------------------------------------------------------------------------

/// TTL policy for this contract's stored state.
///
/// Every piece of configuration the aggregator keeps (`Admin`,
/// `DenylistGate`, `JurisdictionFlag`, `CircuitBreaker`, and the pause flag)
/// lives in **instance** storage, which shares a single TTL with the contract
/// instance itself. If that TTL lapses the whole contract is archived and
/// every call fails until someone submits a manual restore.
///
/// To prevent that, every write path calls [`extend_instance_ttl`], which
/// bumps the instance TTL back up to [`INSTANCE_TTL_EXTEND_TO`] whenever the
/// remaining TTL has fallen below [`INSTANCE_TTL_THRESHOLD`]. The values
/// mirror `allowlist-token`'s persistent-entry policy:
///
/// | Constant                  | Ledgers     | ~Wall-clock (5s/ledger) |
/// |---------------------------|-------------|-------------------------|
/// | `INSTANCE_TTL_THRESHOLD`  | `120_960`   | ~7 days                 |
/// | `INSTANCE_TTL_EXTEND_TO`  | `1_555_200` | ~90 days                |
///
/// So as long as the contract is written to at least once every ~83 days
/// (90 − 7), its state never becomes archived. Extending only below the
/// threshold keeps the per-write rent cost to at most one bump per ~83 days.
///
/// Extend the instance TTL when its remaining TTL drops below this many
/// ledgers (~7 days at ~5s/ledger on mainnet).
pub(crate) const INSTANCE_TTL_THRESHOLD: u32 = 120_960; // ~7 days

/// Target remaining instance TTL after extension (~90 days at ~5s/ledger).
pub(crate) const INSTANCE_TTL_EXTEND_TO: u32 = 1_555_200; // ~90 days

/// Bump the contract instance TTL (and therefore every instance-storage
/// entry) according to the policy documented above. Called from every
/// storage write path.
fn extend_instance_ttl(env: &Env) {
    env.storage()
        .instance()
        .extend_ttl(INSTANCE_TTL_THRESHOLD, INSTANCE_TTL_EXTEND_TO);
}

// ---------------------------------------------------------------------------
// Storage keys
// ---------------------------------------------------------------------------

#[contracttype]
#[derive(Clone)]
enum DataKey {
    /// The admin address that may reconfigure the registered checks.
    Admin,
    /// Address of the `denylist-gate` contract to call, if any.
    DenylistGate,
    /// Address of the `jurisdiction-flag` contract to call, if any.
    JurisdictionFlag,
    /// Address of the `circuit-breaker` contract to consult, if any. When
    /// set and frozen, all checks short-circuit to deny.
    CircuitBreaker,
    /// Address of the `allowlist-token` contract to call, if any.
    AllowlistToken,
}

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// The result of a single compliance check on a single address.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CheckResult {
    /// Human-readable label identifying which check produced this result.
    pub check: CheckKind,
    /// `true` if the address passed the check, `false` if it failed.
    pub passed: bool,
}

/// Identifies which compliance primitive produced a `CheckResult`.
#[contracttype]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum CheckKind {
    Denylist = 0,
    Jurisdiction = 1,
    CircuitBreaker = 2,
    Allowlist = 3,
}

/// Stable reason codes returned by the verbose check endpoints.
#[contracttype]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum CheckFailureCode {
    Denylisted = 0,
    JurisdictionNotPermitted = 1,
    CircuitBreakerFrozen = 2,
    NotAllowlisted = 3,
}

/// Identifies the first check that caused an aggregated check to fail.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CheckFailure {
    pub source: CheckKind,
    pub code: CheckFailureCode,
}

/// Per-address aggregated result returned by `check_all`.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AddressCheckResult {
    pub address: Address,
    /// `true` iff every registered check passed for this address.
    pub all_passed: bool,
    /// Individual results in deterministic order (breaker, denylist,
    /// jurisdiction, then allowlist).
    pub checks: Vec<CheckResult>,
}

/// Per-address result returned by `check_all_verbose`.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VerboseAddressCheckResult {
    pub address: Address,
    pub all_passed: bool,
    pub checks: Vec<CheckResult>,
    pub failure_source: Option<Symbol>,
    pub failure_code: Option<Symbol>,
}

/// Configuration of this aggregator instance, returned by `get_config`.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AggregatorConfig {
    /// Address of the `denylist-gate` contract, if configured.
    pub denylist_gate: Option<Address>,
    /// Address of the `jurisdiction-flag` contract, if configured.
    pub jurisdiction_flag: Option<Address>,
    /// Address of the `circuit-breaker` contract, if configured.
    pub circuit_breaker: Option<Address>,
    /// Address of the `allowlist-token` contract, if configured.
    pub allowlist_token: Option<Address>,
}

// ---------------------------------------------------------------------------
// Events
// ---------------------------------------------------------------------------

#[contractevent]
pub struct AdminSet {
    #[topic]
    pub admin: Address,
}

#[contractevent]
pub struct DenylistGateSet {
    #[topic]
    pub gate: Address,
}

#[contractevent]
pub struct JurisdictionFlagSet {
    #[topic]
    pub flag: Address,
}

/// Emitted when `clear_denylist_gate` unregisters the denylist-gate check.
/// `previous` is the address that was registered before the clear.
#[contractevent]
pub struct DenylistGateCleared {
    #[topic]
    pub previous: Address,
}

/// Emitted when `clear_jurisdiction_flag` unregisters the jurisdiction check.
/// `previous` is the address that was registered before the clear.
#[contractevent]
pub struct JurisdictionFlagCleared {
    #[topic]
    pub previous: Address,
}

#[contractevent]
pub struct CircuitBreakerSet {
    #[topic]
    pub breaker: Address,
}

#[contractevent]
pub struct AllowlistTokenSet {
    #[topic]
    pub allowlist: Address,
}

/// Emitted whenever the contract is upgraded to a new WASM implementation,
/// for on-chain auditability of the upgrade path.
#[contractevent]
pub struct UpgradePerformed {
    #[topic]
    pub admin: Address,
    pub new_wasm_hash: BytesN<32>,
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
#[repr(u32)]
pub enum Error {
    NotInitialized = 1,
    AlreadyInitialized = 2,
    NotAuthorized = 3,
    NoChecksRegistered = 4,
    EmptyAddressList = 5,
    BatchTooLarge = 6,
    ContractPaused = 7,
}

// ---------------------------------------------------------------------------
// Contract
// ---------------------------------------------------------------------------

#[contract]
pub struct ComplianceAggregator;

#[contractimpl]
impl ComplianceAggregator {
    /// Maximum number of addresses accepted by `batch_check` in a single
    /// call. Bounds the per-transaction cross-contract call fan-out (each
    /// address costs up to three nested calls) so a single invocation cannot
    /// exceed the host's resource budget. Set to 45 to fit comfortably within
    /// Soroban's default per-invocation resource limits.
    pub const MAX_BATCH_SIZE: u32 = 45;

    // -----------------------------------------------------------------------
    // Lifecycle
    // -----------------------------------------------------------------------

    /// One-time setup. `admin` is the only address that may later call
    /// `set_denylist_gate`, `set_jurisdiction_flag`, or `set_admin`.
    ///
    /// `denylist_gate` and `jurisdiction_flag` are optional at initialization;
    /// pass `None` for either one you don't want to register yet.
    pub fn initialize(
        env: Env,
        admin: Address,
        denylist_gate: Option<Address>,
        jurisdiction_flag: Option<Address>,
        circuit_breaker: Option<Address>,
    ) -> Result<(), Error> {
        if env.storage().instance().has(&DataKey::Admin) {
            return Err(Error::AlreadyInitialized);
        }
        admin.require_auth();
        env.storage().instance().set(&DataKey::Admin, &admin);
        AdminSet {
            admin: admin.clone(),
        }
        .publish(&env);

        if let Some(gate) = denylist_gate {
            env.storage().instance().set(&DataKey::DenylistGate, &gate);
            DenylistGateSet { gate }.publish(&env);
        }
        if let Some(flag) = jurisdiction_flag {
            env.storage()
                .instance()
                .set(&DataKey::JurisdictionFlag, &flag);
            JurisdictionFlagSet { flag }.publish(&env);
        }
        if let Some(breaker) = circuit_breaker {
            env.storage()
                .instance()
                .set(&DataKey::CircuitBreaker, &breaker);
            CircuitBreakerSet { breaker }.publish(&env);
        }
        extend_instance_ttl(&env);
        Ok(())
    }

    /// Pause configuration mutations. Admin-only.
    pub fn pause(env: Env, admin: Address) -> Result<(), Error> {
        Self::require_admin(&env, &admin)?;
        compliance_pausable::pause(&env);
        extend_instance_ttl(&env);
        env.events().publish((), soroban_sdk::symbol_short!("Paused"));
        Ok(())
    }

    /// Resume configuration mutations after a pause. Admin-only.
    pub fn unpause(env: Env, admin: Address) -> Result<(), Error> {
        Self::require_admin(&env, &admin)?;
        compliance_pausable::unpause(&env);
        extend_instance_ttl(&env);
        env.events().publish((), soroban_sdk::symbol_short!("Unpaused"));
        Ok(())
    }

    /// Check if the contract is currently paused.
    pub fn is_paused(env: Env) -> bool {
        compliance_pausable::is_paused(&env)
    }

    // -----------------------------------------------------------------------
    // Admin management
    // -----------------------------------------------------------------------

    /// Replace the admin. Old admin must authorize.
    pub fn set_admin(env: Env, admin: Address, new_admin: Address) -> Result<(), Error> {
        compliance_pausable::require_not_paused_or(&env, Error::ContractPaused)?;
        Self::require_admin(&env, &admin)?;
        env.storage().instance().set(&DataKey::Admin, &new_admin);
        extend_instance_ttl(&env);
        AdminSet { admin: new_admin }.publish(&env);
        Ok(())
    }

    /// Register or replace the `denylist-gate` contract address. Admin-only.
    pub fn set_denylist_gate(env: Env, admin: Address, gate: Address) -> Result<(), Error> {
        compliance_pausable::require_not_paused_or(&env, Error::ContractPaused)?;
        Self::require_admin(&env, &admin)?;
        env.storage().instance().set(&DataKey::DenylistGate, &gate);
        extend_instance_ttl(&env);
        DenylistGateSet { gate }.publish(&env);
        Ok(())
    }

    /// Register or replace the `jurisdiction-flag` contract address. Admin-only.
    pub fn set_jurisdiction_flag(env: Env, admin: Address, flag: Address) -> Result<(), Error> {
        compliance_pausable::require_not_paused_or(&env, Error::ContractPaused)?;
        Self::require_admin(&env, &admin)?;
        env.storage()
            .instance()
            .set(&DataKey::JurisdictionFlag, &flag);
        extend_instance_ttl(&env);
        JurisdictionFlagSet { flag }.publish(&env);
        Ok(())
    }

    /// Unregister the `denylist-gate` check. Admin-only. Afterwards the
    /// denylist check is skipped, as if it had never been registered.
    pub fn clear_denylist_gate(env: Env, admin: Address) -> Result<(), Error> {
        compliance_pausable::require_not_paused_or(&env, Error::ContractPaused)?;
        Self::require_admin(&env, &admin)?;
        if let Some(previous) = env
            .storage()
            .instance()
            .get::<_, Address>(&DataKey::DenylistGate)
        {
            env.storage().instance().remove(&DataKey::DenylistGate);
            DenylistGateCleared { previous }.publish(&env);
        }
        extend_instance_ttl(&env);
        Ok(())
    }

    /// Unregister the `jurisdiction-flag` check. Admin-only. Afterwards the
    /// jurisdiction check is skipped, as if it had never been registered.
    pub fn clear_jurisdiction_flag(env: Env, admin: Address) -> Result<(), Error> {
        compliance_pausable::require_not_paused_or(&env, Error::ContractPaused)?;
        Self::require_admin(&env, &admin)?;
        if let Some(previous) = env
            .storage()
            .instance()
            .get::<_, Address>(&DataKey::JurisdictionFlag)
        {
            env.storage().instance().remove(&DataKey::JurisdictionFlag);
            JurisdictionFlagCleared { previous }.publish(&env);
        }
        extend_instance_ttl(&env);
        Ok(())
    }

    /// Register or replace the `circuit-breaker` contract address. Admin-only.
    /// Pass this to enable emergency-freeze short-circuiting of all checks.
    pub fn set_circuit_breaker(env: Env, admin: Address, breaker: Address) -> Result<(), Error> {
        compliance_pausable::require_not_paused_or(&env, Error::ContractPaused)?;
        Self::require_admin(&env, &admin)?;
        env.storage()
            .instance()
            .set(&DataKey::CircuitBreaker, &breaker);
        extend_instance_ttl(&env);
        CircuitBreakerSet { breaker }.publish(&env);
        Ok(())
    }

    /// Register or replace the `allowlist-token` contract address. Admin-only.
    pub fn set_allowlist(env: Env, admin: Address, allowlist: Address) -> Result<(), Error> {
        compliance_pausable::require_not_paused_or(&env, Error::ContractPaused)?;
        Self::require_admin(&env, &admin)?;
        env.storage()
            .instance()
            .set(&DataKey::AllowlistToken, &allowlist);
        extend_instance_ttl(&env);
        AllowlistTokenSet { allowlist }.publish(&env);
        Ok(())
    }

    /// Unregister the `allowlist-token` check. Admin-only.
    pub fn clear_allowlist(env: Env, admin: Address) -> Result<(), Error> {
        compliance_pausable::require_not_paused_or(&env, Error::ContractPaused)?;
        Self::require_admin(&env, &admin)?;
        env.storage().instance().remove(&DataKey::AllowlistToken);
        extend_instance_ttl(&env);
        Ok(())
    }

    /// Upgrade the contract WASM. Admin-only.
    ///
    /// Swaps the code behind this contract ID to `new_wasm_hash` (which must
    /// already be uploaded to the network). All instance storage — admin,
    /// registered check addresses, circuit-breaker, pause flag — is
    /// preserved across the upgrade. See the module-level "Upgradeability"
    /// section for the full procedure.
    pub fn upgrade(env: Env, admin: Address, new_wasm_hash: BytesN<32>) -> Result<(), Error> {
        Self::require_admin(&env, &admin)?;
        UpgradePerformed {
            admin,
            new_wasm_hash: new_wasm_hash.clone(),
        }
        .publish(&env);
        env.deployer().update_current_contract_wasm(new_wasm_hash);
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Read helpers
    // -----------------------------------------------------------------------

    /// Returns the configured admin address.
    ///
    /// Returns `Error::NotInitialized` if `initialize` has not been called.
    pub fn get_admin(env: Env) -> Result<Address, Error> {
        env.storage()
            .instance()
            .get(&DataKey::Admin)
            .ok_or(Error::NotInitialized)
    }

    /// Returns the currently registered `denylist-gate` address, if any.
    pub fn denylist_gate(env: Env) -> Option<Address> {
        env.storage().instance().get(&DataKey::DenylistGate)
    }

    /// Returns the currently registered `jurisdiction-flag` address, if any.
    pub fn jurisdiction_flag(env: Env) -> Option<Address> {
        env.storage().instance().get(&DataKey::JurisdictionFlag)
    }

    /// Returns the currently registered `allowlist-token` address, if any.
    pub fn allowlist_token(env: Env) -> Option<Address> {
        env.storage().instance().get(&DataKey::AllowlistToken)
    }

    /// Returns whether the denylist, jurisdiction, and allowlist checks are
    /// configured without requiring separate view calls.
    pub fn get_checks_summary(env: Env) -> (bool, bool, bool) {
        let denylist_gate: Option<Address> = env
            .storage()
            .instance()
            .get::<DataKey, Address>(&DataKey::DenylistGate);
        let jurisdiction_flag: Option<Address> = env
            .storage()
            .instance()
            .get::<DataKey, Address>(&DataKey::JurisdictionFlag);
        let allowlist_token: Option<Address> = env
            .storage()
            .instance()
            .get::<DataKey, Address>(&DataKey::AllowlistToken);
        (
            denylist_gate.is_some(),
            jurisdiction_flag.is_some(),
            allowlist_token.is_some(),
        )
    }

    /// Returns the currently registered `circuit-breaker` address, if any.
    pub fn circuit_breaker(env: Env) -> Option<Address> {
        env.storage().instance().get(&DataKey::CircuitBreaker)
    }

    /// Returns the current configuration of this aggregator instance, including
    /// all registered check contract addresses.
    pub fn get_config(env: Env) -> AggregatorConfig {
        AggregatorConfig {
            denylist_gate: env.storage().instance().get(&DataKey::DenylistGate),
            jurisdiction_flag: env.storage().instance().get(&DataKey::JurisdictionFlag),
            circuit_breaker: env.storage().instance().get(&DataKey::CircuitBreaker),
            allowlist_token: env.storage().instance().get(&DataKey::AllowlistToken),
        }
    }

    /// Evaluates the circuit-breaker check if one is configured. The result
    /// passes only while the breaker is not frozen.
    fn circuit_breaker_result(env: &Env) -> Option<CheckResult> {
        let breaker_addr: Address = env
            .storage()
            .instance()
            .get::<DataKey, Address>(&DataKey::CircuitBreaker)?;
        let frozen = CircuitBreakerClient::new(env, &breaker_addr).is_frozen();
        Some(CheckResult {
            check: CheckKind::CircuitBreaker,
            passed: !frozen,
        })
    }

    // -----------------------------------------------------------------------
    // Compliance checks
    // -----------------------------------------------------------------------

    /// Run all registered compliance checks against `address` in a **single**
    /// call from the consumer's perspective.
    ///
    /// - `allowed_jurisdictions`: the set of permitted jurisdiction codes
    ///   passed through to `jurisdiction-flag.is_permitted_jurisdiction`.
    ///   Ignored (and not forwarded) if no jurisdiction-flag contract is
    ///   registered.
    ///
    /// Returns `(all_passed, checks)` where:
    /// - `all_passed` is `true` iff every registered check returned `true`.
    /// - `checks` is the per-primitive result list in deterministic order
    ///   (breaker first when configured, then denylist, jurisdiction, and
    ///   allowlist) for auditability.
    ///
    /// Returns `Error::NoChecksRegistered` if no check has been configured.
    ///
    /// # Example
    ///
    /// ```ignore
    /// // `aggregator` is a `ComplianceAggregatorClient` for the deployed contract.
    /// let allowed = soroban_sdk::vec![&env, String::from_str(&env, "US")];
    /// let (all_passed, checks) = aggregator.check_address(&holder, &allowed);
    /// ```
    pub fn check_address(
        env: Env,
        address: Address,
        allowed_jurisdictions: Vec<String>,
    ) -> Result<(bool, Vec<CheckResult>), Error> {
        let mut results: Vec<CheckResult> = Vec::new(&env);
        let mut all_passed = true;

        // -- circuit-breaker check --
        // Emergency freeze short-circuit: if a configured circuit-breaker is
        // frozen, deny outright without evaluating the underlying checks.
        if let Some(breaker) = Self::circuit_breaker_result(&env) {
            let passed = breaker.passed;
            results.push_back(breaker);
            if !passed {
                return Ok((false, results));
            }
        }

        // -- denylist-gate check --
        if let Some(gate_addr) = env
            .storage()
            .instance()
            .get::<DataKey, Address>(&DataKey::DenylistGate)
        {
            let client = DenylistGateClient::new(&env, &gate_addr);
            let passed = client.check(&address);
            all_passed = all_passed && passed;
            results.push_back(CheckResult {
                check: CheckKind::Denylist,
                passed,
            });
        }

        // -- jurisdiction-flag check --
        if let Some(flag_addr) = env
            .storage()
            .instance()
            .get::<DataKey, Address>(&DataKey::JurisdictionFlag)
        {
            let client = JurisdictionFlagClient::new(&env, &flag_addr);
            let passed = matches!(
                    client.try_is_permitted_jurisdiction(&address, &allowed_jurisdictions),
                    Ok(Ok(true))
                );
            all_passed = all_passed && passed;
            results.push_back(CheckResult {
                check: CheckKind::Jurisdiction,
                passed,
            });
        }

        // -- allowlist-token check --
        if let Some(allowlist_addr) = env
            .storage()
            .instance()
            .get::<DataKey, Address>(&DataKey::AllowlistToken)
        {
            let client = AllowlistTokenClient::new(&env, &allowlist_addr);
            let passed = client.is_allowed(&address);
            all_passed = all_passed && passed;
            results.push_back(CheckResult {
                check: CheckKind::Allowlist,
                passed,
            });
        }

        if results.is_empty() {
            return Err(Error::NoChecksRegistered);
        }

        Ok((all_passed, results))
    }

    /// Like `check_address`, with the source and code for the first failed
    /// check. `check_address` remains available with its original return type.
    pub fn check_address_verbose(
        env: Env,
        address: Address,
        allowed_jurisdictions: Vec<String>,
    ) -> Result<(bool, Vec<CheckResult>, Option<CheckFailure>), Error> {
        let (all_passed, checks) =
            Self::check_address(env, address, allowed_jurisdictions)?;
        let failure = Self::failure_for_checks(&checks);
        Ok((all_passed, checks, failure))
    }

    /// Batched variant of `check_address` for multiple addresses in one call.
    ///
    /// Returns one `AddressCheckResult` per input address, in the same order.
    /// `Error::EmptyAddressList` is returned if `addresses` is empty.
    ///
    /// **Overhead note**: each address still requires its own cross-contract
    /// calls to the underlying primitives; Soroban's execution model does not
    /// allow deferring those across addresses. The saving here is the single
    /// invocation from the *consumer* to this aggregator, versus the consumer
    /// having to call each primitive per address itself. For the per-address
    /// primitive calls, Soroban's host caches contract instance reads within
    /// the same transaction so repeated calls to the same gate or flag address
    /// re-use the already-loaded instance.
    pub fn check_all(
        env: Env,
        addresses: Vec<Address>,
        allowed_jurisdictions: Vec<String>,
    ) -> Result<Vec<AddressCheckResult>, Error> {
        if addresses.is_empty() {
            return Err(Error::EmptyAddressList);
        }

        // Resolve contract addresses once, outside the per-address loop, to
        // avoid redundant storage reads.
        let breaker: Option<CheckResult> = Self::circuit_breaker_result(&env);
        let gate_addr: Option<Address> = env
            .storage()
            .instance()
            .get::<DataKey, Address>(&DataKey::DenylistGate);
        let flag_addr: Option<Address> = env
            .storage()
            .instance()
            .get::<DataKey, Address>(&DataKey::JurisdictionFlag);
        let allowlist_addr: Option<Address> = env
            .storage()
            .instance()
            .get::<DataKey, Address>(&DataKey::AllowlistToken);

        if breaker.is_none()
            && gate_addr.is_none()
            && flag_addr.is_none()
            && allowlist_addr.is_none()
        {
            return Err(Error::NoChecksRegistered);
        }

        let mut batch: Vec<AddressCheckResult> = Vec::new(&env);

        for address in addresses.iter() {
            let mut results: Vec<CheckResult> = Vec::new(&env);
            let mut all_passed = true;

            // Emergency freeze short-circuit: if a configured circuit-breaker
            // is frozen, deny this address without evaluating the underlying
            // checks.
            if let Some(ref b) = breaker {
                all_passed = b.passed;
                results.push_back(b.clone());
            }

            if all_passed {
                if let Some(ref ga) = gate_addr {
                    let client = DenylistGateClient::new(&env, ga);
                    let passed = client.check(&address);
                    all_passed = all_passed && passed;
                    results.push_back(CheckResult {
                        check: CheckKind::Denylist,
                        passed,
                    });
                }

                if let Some(ref fa) = flag_addr {
                    let client = JurisdictionFlagClient::new(&env, fa);
                    let passed =
                        matches!(
                        client.try_is_permitted_jurisdiction(&address, &allowed_jurisdictions),
                        Ok(Ok(true))
                    );
                    all_passed = all_passed && passed;
                    results.push_back(CheckResult {
                        check: CheckKind::Jurisdiction,
                        passed,
                    });
                }

                if let Some(ref aa) = allowlist_addr {
                    let client = AllowlistTokenClient::new(&env, aa);
                    let passed = client.is_allowed(&address);
                    all_passed = all_passed && passed;
                    results.push_back(CheckResult {
                        check: CheckKind::Allowlist,
                        passed,
                    });
                }
            }

            batch.push_back(AddressCheckResult {
                address,
                all_passed,
                checks: results,
            });
        }

        Ok(batch)
    }

    /// Batched variant of `check_all` that adds first-failure detail for each
    /// address. `check_all` retains its existing result shape.
    pub fn check_all_verbose(
        env: Env,
        addresses: Vec<Address>,
        allowed_jurisdictions: Vec<String>,
    ) -> Result<Vec<VerboseAddressCheckResult>, Error> {
        let results = Self::check_all(env.clone(), addresses, allowed_jurisdictions)?;
        let mut verbose = Vec::new(&env);
        for result in results.iter() {
            let failure = Self::failure_for_checks(&result.checks);
            verbose.push_back(VerboseAddressCheckResult {
                address: result.address,
                all_passed: result.all_passed,
                failure_source: failure
                    .clone()
                    .map(|failure| Self::check_kind_symbol(&env, failure.source)),
                failure_code: failure
                    .map(|failure| Self::check_failure_code_symbol(&env, failure.code)),
                checks: result.checks,
            });
        }
        Ok(verbose)
    }

    /// Lightweight batched entrypoint: evaluates the configured policy for
    /// each address in `addresses` and returns only the pass/fail booleans,
    /// in the same order as the input, for issuers who don't need the
    /// per-check breakdown that `check_all` provides.
    ///
    /// `allowed_jurisdictions` is forwarded to the `jurisdiction-flag` check
    /// exactly as in `check_address`/`check_all`. All configured sources,
    /// including the circuit breaker and allowlist, are applied.
    ///
    /// Guards against unbounded batches (and the associated cross-contract
    /// call fan-out) with `MAX_BATCH_SIZE`. Returns
    /// `Error::EmptyAddressList` for an empty input and
    /// `Error::BatchTooLarge` if `addresses.len() > MAX_BATCH_SIZE`.
    pub fn batch_check(
        env: Env,
        addresses: Vec<Address>,
        allowed_jurisdictions: Vec<String>,
    ) -> Result<Vec<bool>, Error> {
        if addresses.is_empty() {
            return Err(Error::EmptyAddressList);
        }
        if addresses.len() > Self::MAX_BATCH_SIZE {
            return Err(Error::BatchTooLarge);
        }

        let breaker = Self::circuit_breaker_result(&env);
        let gate_addr: Option<Address> = env
            .storage()
            .instance()
            .get::<DataKey, Address>(&DataKey::DenylistGate);
        let flag_addr: Option<Address> = env
            .storage()
            .instance()
            .get::<DataKey, Address>(&DataKey::JurisdictionFlag);
        let allowlist_addr: Option<Address> = env
            .storage()
            .instance()
            .get::<DataKey, Address>(&DataKey::AllowlistToken);

        if breaker.is_none()
            && gate_addr.is_none()
            && flag_addr.is_none()
            && allowlist_addr.is_none()
        {
            return Err(Error::NoChecksRegistered);
        }

        let mut results: Vec<bool> = Vec::new(&env);

        for address in addresses.iter() {
            let mut all_passed = breaker.as_ref().map(|check| check.passed).unwrap_or(true);

            if let Some(ref ga) = gate_addr {
                let client = DenylistGateClient::new(&env, ga);
                all_passed = all_passed && client.check(&address);
            }

            if let Some(ref fa) = flag_addr {
                let client = JurisdictionFlagClient::new(&env, fa);
                all_passed =
                    all_passed && matches!(
                    client.try_is_permitted_jurisdiction(&address, &allowed_jurisdictions),
                    Ok(Ok(true))
                );
            }

            if let Some(ref aa) = allowlist_addr {
                let client = AllowlistTokenClient::new(&env, aa);
                all_passed = all_passed && client.is_allowed(&address);
            }

            results.push_back(all_passed);
        }

        Ok(results)
    }

    // -----------------------------------------------------------------------
    // Internal helpers
    // -----------------------------------------------------------------------

    fn require_admin(env: &Env, admin: &Address) -> Result<(), Error> {
        admin.require_auth();
        let stored: Address = env
            .storage()
            .instance()
            .get(&DataKey::Admin)
            .ok_or(Error::NotInitialized)?;
        if stored != *admin {
            return Err(Error::NotAuthorized);
        }
        Ok(())
    }

    fn failure_for_checks(checks: &Vec<CheckResult>) -> Option<CheckFailure> {
        for check in checks.iter() {
            if !check.passed {
                let code = match check.check {
                    CheckKind::Denylist => CheckFailureCode::Denylisted,
                    CheckKind::Jurisdiction => CheckFailureCode::JurisdictionNotPermitted,
                    CheckKind::CircuitBreaker => CheckFailureCode::CircuitBreakerFrozen,
                    CheckKind::Allowlist => CheckFailureCode::NotAllowlisted,
                };
                return Some(CheckFailure {
                    source: check.check,
                    code,
                });
            }
        }
        None
    }

    fn check_kind_symbol(env: &Env, source: CheckKind) -> Symbol {
        match source {
            CheckKind::Denylist => Symbol::new(env, "Denylist"),
            CheckKind::Jurisdiction => Symbol::new(env, "Jurisdiction"),
            CheckKind::CircuitBreaker => Symbol::new(env, "CircuitBreaker"),
            CheckKind::Allowlist => Symbol::new(env, "Allowlist"),
        }
    }

    fn check_failure_code_symbol(env: &Env, code: CheckFailureCode) -> Symbol {
        match code {
            CheckFailureCode::Denylisted => Symbol::new(env, "Denylisted"),
            CheckFailureCode::JurisdictionNotPermitted => {
                Symbol::new(env, "JurisdictionNotPermitted")
            }
            CheckFailureCode::CircuitBreakerFrozen => Symbol::new(env, "CircuitBreakerFrozen"),
            CheckFailureCode::NotAllowlisted => Symbol::new(env, "NotAllowlisted"),
        }
    }
}

#[cfg(test)]
mod test;

#[cfg(test)]
mod fuzz;
