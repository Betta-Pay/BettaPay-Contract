<!--
Vulnerability report template for BettaPay Contracts.

Do not open this as a GitHub issue, discussion, or pull request. Copy this
file's content into an email to security@bettapay.com instead — see
SECURITY.md at the repository root for the full reporting process and
disclosure timeline. Fill in every section you can; leave a section as
"Unknown" rather than guessing if you're not sure.
-->

## Summary

One or two sentences describing the vulnerability and its impact.

## Affected Component(s)

- [ ] `governance_contract`
- [ ] `settlement_contract`
- [ ] `bettapay_common`
- [ ] Deployment / CI tooling (`scripts/`, `.github/workflows/`)
- [ ] Other (describe):

Contract entry point(s) or function(s) involved, if applicable:

## Vulnerability Class

Examples: unauthorized state mutation, authorization bypass, integer
overflow/underflow, storage/TTL exhaustion (DoS), fee/rounding
manipulation, reentrancy-equivalent cross-call issue, event/topic
spoofing, upgrade/migration hazard, key management or CI-secret exposure,
other.

## Severity (your estimate)

- [ ] Critical — funds/admin control directly at risk, exploitable by anyone
- [ ] High — significant impact, requires specific preconditions
- [ ] Medium — limited impact or requires privileged access
- [ ] Low — best-practice gap, no direct exploit path found

## Steps to Reproduce / Proof of Concept

Minimal steps, test code, or a `cargo test` reproduction against the
contract's in-memory `Env` (preferred where possible) that demonstrates
the issue. Include the network (testnet/futurenet/local) and contract
version/commit hash if reproducing against a live deployment.

## Impact

What an attacker gains, and who is affected (all users of a deployed
instance, merchants, admins, ...).

## Suggested Fix (optional)

Any mitigation or fix you'd propose, if you have one.

## Disclosure Preferences

- Your name/handle for credit in the fix's release notes (or "anonymous"):
- Preferred contact method for follow-up questions:
- Any disclosure deadline constraints on your end (e.g. a conflicting
  public disclosure elsewhere):
