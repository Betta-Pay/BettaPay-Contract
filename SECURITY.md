# Security Policy

## Reporting a Vulnerability

**Do not open public GitHub issues, discussions, or pull requests for security
vulnerabilities.** Publicly disclosing a vulnerability before a fix is
available can put users and funds at risk.

Report suspected vulnerabilities through one of the following channels, in
order of preference:

1. **GitHub Security Advisories (preferred).** Use this repository's private
   ["Report a vulnerability"](https://github.com/Betta-Pay/BettaPay-Contract/security/advisories/new)
   form (repo → **Security** tab → **Advisories** → **Report a vulnerability**).
   This opens a private advisory visible only to maintainers and lets us
   collaborate with you on a fix before anything is disclosed.
2. **Email.** If you cannot use GitHub Security Advisories, email
   **security@bettapay.com** with the report template below. If the details
   are highly sensitive, say so in your first message and we will agree on a
   secure channel before you share exploit details.

We aim to **acknowledge reports within 48 hours** and to provide an initial
severity assessment and fix timeline within 5 business days.

### Report Template

Copy this template into your GitHub Security Advisory or email so we have
everything needed to triage quickly. The same template is available as a
standalone file at
[`.github/SECURITY_REPORT_TEMPLATE.md`](.github/SECURITY_REPORT_TEMPLATE.md)
for reuse.

```markdown
## Summary
<!-- One or two sentences describing the vulnerability. -->

## Affected Component(s)
- Contract(s): <!-- e.g. settlement_contract, governance_contract -->
- Version / commit: <!-- git commit hash, tag, or deployed contract address/network -->

## Impact / Severity Assessment
<!-- What can an attacker do? Funds at risk, unauthorized access, DoS, storage
     corruption, etc. Include your own severity estimate (Critical / High /
     Medium / Low) and reasoning. -->

## Steps to Reproduce
1.
2.
3.

## Proof of Concept
<!-- Optional: script, test case, transaction hash, or Soroban CLI invocation
     that demonstrates the issue. -->

## Suggested Fix
<!-- Optional: thoughts on remediation, mitigations, or patches. -->

## Reporter Contact
- Name / handle:
- Preferred contact method:
- Do you want to be credited in the advisory? (yes/no)
```

## Security Report Owners

Reports submitted through GitHub Security Advisories or email are triaged by:

- **@Betta-Pay/maintainers** (GitHub team — primary triage)
- **security@bettapay.com** (fallback contact)

> **Note for maintainers:** the placeholders above should be replaced with the
> real GitHub team handle(s) and/or individual maintainer usernames
> responsible for security triage.

## Responsible Disclosure

We request a **90-day disclosure window** from the time a fix is deployed
before any public disclosure of the vulnerability, so downstream integrators
and users have time to upgrade. We will coordinate with reporters on
disclosure timing and, where desired, credit reporters in the published
advisory and release notes.

## Scope

This policy covers the smart contracts, deployment scripts, and build tooling
in this repository (`settlement_contract`, `governance_contract`, `scripts/`).
Vulnerabilities discovered in third-party dependencies used by this project
should also be reported here so we can coordinate an upstream fix or a local
mitigation.

---

## Incident Runbook: Governance Trap

### Overview

A governance trap occurs when the configured governance contract is broken,
mis-deployed, or returns an unexpected error, causing every mutating entry
point in `settlement_contract` to abort with `GovernanceCallFailed`.  Admin
writes (fee-rule updates, merchant registration, pausing) become impossible
until the trap is resolved.

### Symptoms

- On-chain transaction invoking any mutating entry point returns a contract
  error whose `SettlementError` code corresponds to `GovernanceCallFailed`.
- `update_governance`, `set_settlement_rule`, `set_default_rule`, and similar
  admin calls all fail; read-only calls such as `calculate_fee_split` still
  succeed because they do not invoke governance.
- Monitoring dashboards for the governance contract show panics or missing
  `get_fee_config` export.

### Mitigation

1. **Identify the broken governance contract.**  Read the on-chain
   `DataKey::Governance` value from `settlement_contract` storage to confirm
   which address is misconfigured.

2. **Fix or replace the governance contract.**  If the contract can be patched
   in place, deploy the fix and verify `get_fee_config` returns a valid
   `GovFeeConfig`.  If it cannot, deploy a new governance contract that exports
   `get_fee_config` correctly.

3. **Update governance address via the recovery path.**  Because the multisig
   admin write is blocked, use the recovery mechanism:
   - Invoke `initiate_recovery` with the new-governance deployer address.
   - After the recovery timelock elapses, invoke `execute_recovery` to complete
     the admin transfer to an address that can call `update_governance`.
   - Alternatively, schedule `Operation::UpdateGovernance(new_address)` via the
     timelock path from a still-functional admin key and execute it after the
     delay.

4. **Verify recovery.**  Call `get_governance()` on-chain and confirm it returns
   the corrected address.  Then perform a dry-run of `calculate_fee_split` and a
   test `set_default_rule` transaction to confirm the trap is cleared.

### Verification Steps

```bash
# 1. Confirm governance address stored in the contract.
soroban contract read --id <settlement_contract_id> --key Governance

# 2. Probe the governance contract directly.
soroban contract invoke --id <governance_contract_id> -- get_fee_config

# 3. After updating governance, confirm the new address is stored.
soroban contract read --id <settlement_contract_id> --key Governance

# 4. Smoke-test a read-only path — should succeed even during the trap.
soroban contract invoke --id <settlement_contract_id> -- calculate_fee_split \
  --merchant <merchant_address> --amount 1000000

# 5. After resolving the trap, verify a mutating call succeeds.
soroban contract invoke --id <settlement_contract_id> -- set_default_rule \
  --admins '[<admin1>]' --rule '{"platform_fee_bps":100,"network_fee_bps":20,...}'
```
