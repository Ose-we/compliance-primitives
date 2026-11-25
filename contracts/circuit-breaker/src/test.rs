extern crate std;

use super::*;
use soroban_sdk::testutils::{Address as _, Events as _};
use soroban_sdk::{Env, IntoVal};

fn setup(env: &Env) -> (Address, CircuitBreakerClient<'_>) {
    env.mock_all_auths();
    let admin = Address::generate(env);
    let contract_id = env.register(CircuitBreaker, ());
    let client = CircuitBreakerClient::new(env, &contract_id);
    client.initialize(&admin);
    (admin, client)
}

#[test]
fn test_is_frozen_defaults_to_false() {
    let env = Env::default();
    let (_admin, client) = setup(&env);
    assert!(!client.is_frozen());
}

#[test]
fn test_admin_can_freeze_and_unfreeze() {
    let env = Env::default();
    let (admin, client) = setup(&env);

    client.freeze(&admin, &None);
    assert!(client.is_frozen());

    client.unfreeze(&admin);
    assert!(!client.is_frozen());
}

#[test]
fn test_freeze_and_unfreeze_emit_events_with_admin() {
    let env = Env::default();
    let (admin, client) = setup(&env);

    client.freeze(&admin, &None);
    let freeze_events = env.events().all();
    let (_contract_id, topics, _data) = freeze_events.last().unwrap();
    assert_eq!(
        topics.get_unchecked(0),
        soroban_sdk::Symbol::new(&env, "frozen").into_val(&env)
    );
    assert_eq!(topics.get_unchecked(1), admin.clone().into_val(&env));

    client.unfreeze(&admin);
    let unfreeze_events = env.events().all();
    let (_contract_id, topics, _data) = unfreeze_events.last().unwrap();
    assert_eq!(
        topics.get_unchecked(0),
        soroban_sdk::Symbol::new(&env, "unfrozen").into_val(&env)
    );
    assert_eq!(topics.get_unchecked(1), admin.clone().into_val(&env));
}

#[test]
fn test_non_admin_cannot_freeze_or_unfreeze() {
    let env = Env::default();
    let (admin, client) = setup(&env);
    let impostor = Address::generate(&env);

    let freeze_result = client.try_freeze(&impostor, &None);
    assert_eq!(freeze_result, Err(Ok(Error::NotAuthorized)));
    assert!(!client.is_frozen());

    let unfreeze_result = client.try_unfreeze(&impostor);
    assert_eq!(unfreeze_result, Err(Ok(Error::NotAuthorized)));
    assert!(!client.is_frozen());

    client.freeze(&admin, &None);
    assert!(client.is_frozen());

    let unfreeze_result = client.try_unfreeze(&impostor);
    assert_eq!(unfreeze_result, Err(Ok(Error::NotAuthorized)));
    assert!(client.is_frozen());
}

#[test]
fn test_is_frozen_before_initialize_returns_not_initialized_error() {
    let env = Env::default();
    let contract_id = env.register(CircuitBreaker, ());
    let client = CircuitBreakerClient::new(&env, &contract_id);
    // Never called `initialize` on this instance.  `is_frozen` must now
    // return `Err(NotInitialized)` instead of silently returning `false`.
    // Callers (compliance-aggregator, policy-engine) must treat this as a
    // hard failure — failing open on an uninitialized breaker is a
    // security vulnerability (issue #607).
    let result = client.try_is_frozen();
    assert_eq!(result, Err(Ok(Error::NotInitialized)));
}

#[test]
fn test_double_freeze_is_idempotent() {
    let env = Env::default();
    let (admin, client) = setup(&env);

    // Calling `freeze` twice in a row is a no-op the second time: state
    // stays frozen and the call itself succeeds rather than erroring.
    client.freeze(&admin, &None);
    assert!(client.is_frozen());
    // Exactly one Frozen event should have been emitted so far.
    assert_eq!(env.events().all().len(), 1);

    client.freeze(&admin, &None);
    assert!(client.is_frozen());
    // Second freeze must NOT emit another event.
    assert_eq!(env.events().all().len(), 1);
}

#[test]
fn test_double_unfreeze_is_idempotent() {
    let env = Env::default();
    let (admin, client) = setup(&env);

    // Calling `unfreeze` twice in a row is a no-op the second time: state
    // stays unfrozen and the call itself succeeds rather than erroring.
    client.unfreeze(&admin);
    assert!(!client.is_frozen());
    // Already unfrozen at init; first unfreeze call also should not emit.
    assert_eq!(env.events().all().len(), 0);

    client.unfreeze(&admin);
    assert!(!client.is_frozen());
    // Still no events: both calls were no-ops.
    assert_eq!(env.events().all().len(), 0);
}

#[test]
fn test_freeze_then_unfreeze_emits_exactly_one_event_each() {
    let env = Env::default();
    let (admin, client) = setup(&env);

    client.freeze(&admin, &None);
    // One Frozen event.
    assert_eq!(env.events().all().len(), 1);

    client.unfreeze(&admin);
    // One Frozen + one Unfrozen = two events total.
    assert_eq!(env.events().all().len(), 2);
}

#[test]
fn test_propose_then_accept_admin_transfers_control() {
    let env = Env::default();
    let (admin, client) = setup(&env);
    let new_admin = Address::generate(&env);

    client.propose_admin(&admin, &new_admin);
    client.accept_admin(&new_admin);

    // Old admin no longer has control.
    let freeze_result = client.try_freeze(&admin, &None);
    assert_eq!(freeze_result, Err(Ok(Error::NotAuthorized)));

    // New admin does.
    client.freeze(&new_admin, &None);
    assert!(client.is_frozen());
}

#[test]
fn test_propose_never_accepted_old_admin_retains_control() {
    let env = Env::default();
    let (admin, client) = setup(&env);
    let new_admin = Address::generate(&env);

    client.propose_admin(&admin, &new_admin);
    // `new_admin` never calls accept_admin.

    // Old admin still has control.
    client.freeze(&admin, &None);
    assert!(client.is_frozen());

    // The proposed admin has no control yet.
    let unfreeze_result = client.try_unfreeze(&new_admin);
    assert_eq!(unfreeze_result, Err(Ok(Error::NotAuthorized)));
}

#[test]
fn test_metadata_after_initialization() {
    let env = Env::default();
    let (admin, client) = setup(&env);

    let metadata = client.metadata();
    assert_eq!(metadata.admin, admin);
    assert_eq!(
        metadata.version,
        soroban_sdk::String::from_str(&env, env!("CARGO_PKG_VERSION"))
    );
}

// ---------------------------------------------------------------------------
// Admin proposal events — #608
// ---------------------------------------------------------------------------

#[test]
fn test_propose_admin_emits_event() {
    let env = Env::default();
    let (admin, client) = setup(&env);
    let new_admin = Address::generate(&env);

    client.propose_admin(&admin, &new_admin);

    let events = env.events().all();
    let (_contract_id, topics, _data) = events.last().unwrap();
    assert_eq!(
        topics,
        (
            soroban_sdk::Symbol::new(&env, "admin_proposed"),
            admin.clone(),
            new_admin,
        )
            .into_val(&env)
    );
}

#[test]
fn test_accept_admin_emits_transferred_event() {
    let env = Env::default();
    let (admin, client) = setup(&env);
    let new_admin = Address::generate(&env);

    client.propose_admin(&admin, &new_admin);
    client.accept_admin(&new_admin);

    let events = env.events().all();
    let (_contract_id, topics, _data) = events.last().unwrap();
    assert_eq!(
        topics.get_unchecked(0),
        soroban_sdk::Symbol::new(&env, "admin_transferred").into_val(&env)
    );
    assert_eq!(topics.get_unchecked(1), new_admin.clone().into_val(&env));
}

// ---------------------------------------------------------------------------
// cancel_admin_proposal — #608
// ---------------------------------------------------------------------------

#[test]
fn test_cancel_admin_proposal_succeeds() {
    let env = Env::default();
    let (admin, client) = setup(&env);
    let new_admin = Address::generate(&env);

    client.propose_admin(&admin, &new_admin);
    assert_eq!(client.get_pending_admin(), Some(new_admin.clone()));

    client.cancel_admin_proposal(&admin);
    assert_eq!(client.get_pending_admin(), None);
}

#[test]
fn test_accept_after_cancel_returns_no_pending_admin() {
    let env = Env::default();
    let (admin, client) = setup(&env);
    let new_admin = Address::generate(&env);

    client.propose_admin(&admin, &new_admin);
    client.cancel_admin_proposal(&admin);

    let result = client.try_accept_admin(&new_admin);
    assert_eq!(result, Err(Ok(Error::NoPendingAdmin)));
}

#[test]
fn test_cancel_admin_proposal_no_pending_returns_error() {
    let env = Env::default();
    let (admin, client) = setup(&env);

    let result = client.try_cancel_admin_proposal(&admin);
    assert_eq!(result, Err(Ok(Error::NoPendingAdmin)));
}

#[test]
fn test_cancel_admin_proposal_non_admin_rejected() {
    let env = Env::default();
    let (admin, client) = setup(&env);
    let new_admin = Address::generate(&env);
    let impostor = Address::generate(&env);

    client.propose_admin(&admin, &new_admin);

    let result = client.try_cancel_admin_proposal(&impostor);
    assert_eq!(result, Err(Ok(Error::NotAuthorized)));

    // Proposal is still pending.
    assert_eq!(client.get_pending_admin(), Some(new_admin));
}

#[test]
fn test_get_pending_admin_none_when_no_proposal() {
    let env = Env::default();
    let (_admin, client) = setup(&env);

    assert_eq!(client.get_pending_admin(), None);
}

#[test]
fn test_get_pending_admin_returns_proposed() {
    let env = Env::default();
    let (admin, client) = setup(&env);
    let new_admin = Address::generate(&env);

    client.propose_admin(&admin, &new_admin);
    assert_eq!(client.get_pending_admin(), Some(new_admin));
}

#[test]
fn test_cancel_then_old_admin_retains_control() {
    let env = Env::default();
    let (admin, client) = setup(&env);
    let new_admin = Address::generate(&env);

    client.propose_admin(&admin, &new_admin);
    client.cancel_admin_proposal(&admin);

    // Old admin still has control after cancellation.
    client.freeze(&admin, &None);
    assert!(client.is_frozen());

    // Proposed admin does not have control.
    let result = client.try_unfreeze(&new_admin);
    assert_eq!(result, Err(Ok(Error::NotAuthorized)));
}

// ---------------------------------------------------------------------------
// Resource-fee benchmark (budget regression) — `is_frozen`
//
// `is_frozen` is the circuit-breaker's hottest entrypoint: consumers such as
// `compliance-aggregator` read it on every check. The measured CPU/memory
// cost is compared against the `[circuit-breaker.is_frozen]` entry in
// `budget-baselines.toml`; the test (and therefore the `budget-regression` CI
// job, which runs `cargo test --workspace budget_regression`) fails if either
// exceeds the baseline by more than 10%.
// ---------------------------------------------------------------------------

fn read_baseline(path: &std::path::Path, section: &str) -> (u64, u64) {
    let contents = std::fs::read_to_string(path).unwrap();
    let section_header = std::format!("[{section}]");
    let mut in_section = false;
    let mut cpu = None;
    let mut memory = None;

    for line in contents.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') && trimmed.ends_with(']') {
            in_section = trimmed == section_header;
            continue;
        }
        if !in_section {
            continue;
        }
        if let Some(value) = trimmed.strip_prefix("cpu = ") {
            cpu = Some(value.replace('_', "").parse::<u64>().unwrap());
        } else if let Some(value) = trimmed.strip_prefix("memory = ") {
            memory = Some(value.replace('_', "").parse::<u64>().unwrap());
        }
    }

    let cpu = cpu.expect("missing cpu baseline");
    let memory = memory.expect("missing memory baseline");
    (cpu, memory)
}

fn baseline_path() -> std::path::PathBuf {
    std::path::PathBuf::from(std::env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("budget-baselines.toml")
}

fn assert_budget_within_threshold(measured: (u64, u64), baseline: (u64, u64), label: &str) {
    let (measured_cpu, measured_memory) = measured;
    let (baseline_cpu, baseline_memory) = baseline;
    let cpu_limit = (baseline_cpu as f64 * 1.10).ceil() as u64;
    let memory_limit = (baseline_memory as f64 * 1.10).ceil() as u64;

    assert!(
        measured_cpu <= cpu_limit,
        "{label} CPU regression: measured {measured_cpu}, baseline {baseline_cpu}, limit {cpu_limit}"
    );
    assert!(
        measured_memory <= memory_limit,
        "{label} memory regression: measured {measured_memory}, baseline {baseline_memory}, limit {memory_limit}"
    );
}

// ---------------------------------------------------------------------------
// Fuzz target: guardian management — #817
//
// Random add/remove/freeze/unfreeze sequences by random callers. Invariants:
//   * only the admin (or a guardian, for freeze) can freeze/unfreeze;
//   * the guardian list never contains duplicates.
//
// The harness is bounded so it can run in CI. Iteration and per-iteration
// operation counts are configurable via `FUZZ_ITERATIONS` / `FUZZ_OPS`.
// ---------------------------------------------------------------------------

fn fuzz_env_u64(name: &str, default: u64) -> u64 {
    std::env::var(name)
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .filter(|v| *v > 0)
        .unwrap_or(default)
}

/// Deterministic xorshift64* PRNG so failures are reproducible.
struct FuzzRng(u64);

impl FuzzRng {
    fn new(seed: u64) -> Self {
        Self(seed | 1)
    }

    fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    fn below(&mut self, bound: u64) -> u64 {
        if bound == 0 {
            0
        } else {
            self.next_u64() % bound
        }
    }
}

fn fuzz_guardian_sequences(seed: u64, iterations: u64, ops: u64) {
    let env = Env::default();
    env.mock_all_auths();
    let admin = Address::generate(&env);
    let contract_id = env.register(CircuitBreaker, ());
    let client = CircuitBreakerClient::new(&env, &contract_id);
    client.initialize(&admin);

    // A small pool of actors keeps collisions (and thus duplicate-add paths)
    // likely without exhausting the test environment.
    let mut actors: std::vec::Vec<Address> = std::vec::Vec::new();
    for _ in 0..4 {
        actors.push(Address::generate(&env));
    }

    let mut rng = FuzzRng::new(seed);

    for _ in 0..iterations {
        for _ in 0..ops {
            let caller = actors[rng.below(actors.len() as u64) as usize].clone();
            let target = actors[rng.below(actors.len() as u64) as usize].clone();
            let is_admin = caller == admin;

            match rng.below(4) {
                0 => {
                    // add_guardian: only admin may add.
                    let result = client.try_add_guardian(&caller, &target);
                    if !is_admin {
                        assert_eq!(result, Err(Ok(Error::NotAuthorized)));
                    }
                }
                1 => {
                    // remove_guardian: only admin may remove.
                    let result = client.try_remove_guardian(&caller, &target);
                    if !is_admin {
                        assert_eq!(result, Err(Ok(Error::NotAuthorized)));
                    }
                }
                2 => {
                    // freeze: admin or a current guardian may freeze.
                    let allowed = is_admin || client.is_guardian(&caller);
                    let result = client.try_freeze(&caller, &None);
                    if allowed {
                        assert!(result.is_ok(), "authorized freeze must succeed");
                    } else {
                        assert_eq!(result, Err(Ok(Error::NotAuthorized)));
                    }
                }
                _ => {
                    // unfreeze: admin or a current guardian may unfreeze.
                    let allowed = is_admin || client.is_guardian(&caller);
                    let result = client.try_unfreeze(&caller);
                    if allowed {
                        assert!(result.is_ok(), "authorized unfreeze must succeed");
                    } else {
                        assert_eq!(result, Err(Ok(Error::NotAuthorized)));
                    }
                }
            }

            // Invariant: the guardian list has no duplicates.
            let guardians = client.get_guardians();
            let mut seen: std::vec::Vec<Address> = std::vec::Vec::new();
            for g in guardians.iter() {
                assert!(
                    !seen.contains(&g),
                    "duplicate guardian detected in list"
                );
                seen.push(g);
            }
        }
    }
}

#[test]
fn fuzz_guardian_management() {
    let iterations = fuzz_env_u64("FUZZ_ITERATIONS", 32);
    let ops = fuzz_env_u64("FUZZ_OPS", 16);
    fuzz_guardian_sequences(0x9E37_79B9_7F4A_7C15, iterations, ops);
}

#[test]
fn test_budget_regression_is_frozen() {
    let env = Env::default();
    let (_admin, client) = setup(&env);

    let mut budget = env.cost_estimate().budget();
    budget.reset_default();
    assert!(!client.is_frozen());

    let measured = (budget.cpu_instruction_cost(), budget.memory_bytes_cost());
    std::println!(
        "circuit-breaker.is_frozen: cpu = {}, memory = {}",
        measured.0, measured.1
    );

    let baseline = read_baseline(&baseline_path(), "circuit-breaker.is_frozen");
    assert_budget_within_threshold(measured, baseline, "circuit-breaker is_frozen");
}
