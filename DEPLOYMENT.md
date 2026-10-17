# Deployment and initialization

Deploy both contract instances before initializing either one. The required
order is:

1. Deploy the settlement contract.
2. Deploy the governance contract.
3. Initialize settlement with the governance contract address.
4. Initialize governance.

Both `scripts/deploy_testnet.sh` and `scripts/simulate.sh` follow this order.

## Why settlement initializes first

Settlement initialization stores the governance address after checking only
that it is non-zero. It does not make a cross-contract call, so governance
does not need to be initialized or otherwise live when settlement is
initialized. Settlement reads governance fee configuration later, when that
configuration is needed.

## Reruns and partial deployments

Contract deployment creates new contract instances; rerunning an entire deploy
script is not an idempotent way to resume a failed deployment. Each contract's
`init` is also one-shot: a successful second call fails with
`AlreadyInitialized`. Do not blindly call either `init` again.

If a script stops before initialization, the deployed contract instances remain
but are uninitialized. If it stops after settlement initializes but before
governance initializes, settlement remains initialized with the deployed
governance address stored, while governance remains uninitialized. Settlement
payment paths can fall back to bootstrap fee behavior when governance is
unavailable, but governance-controlled configuration is not active and
settlement operations that validate against governance can fail. Do not treat
that fallback as a completed deployment. A failed initialization transaction
does not partially commit its writes, but a successful earlier initialization
is not undone by a later failure.

For recovery from an interrupted run, preserve the contract IDs printed by
`deploy_testnet.sh` (or stored by `simulate.sh` under `.soroban/`) and inspect
each contract's initialization state before taking action. Resume only an
initialization that is known not to have succeeded; do not redeploy or
reinitialize contracts whose state is uncertain.

Both scripts default `RECOVERY_ADDRESS` to the deployment admin/source address
when it is unset. Set `RECOVERY_ADDRESS` explicitly if a different recovery
address is intended.
