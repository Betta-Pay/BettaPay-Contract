# Security Policy

## Scope

This policy covers the Soroban smart contracts in this repository
(`governance_contract`, `settlement_contract`, `bettapay_common`) and the
deployment/CI tooling under `scripts/` and `.github/workflows/`. It does not
cover the BettaPay frontend or backend services, which live in separate
repositories — report issues in those to their own repositories or contacts.

## Reporting a Vulnerability

Email **security@bettapay.com** with the details of the issue. Use the
[vulnerability report template](.github/SECURITY_REPORT_TEMPLATE.md) — copy
its sections into your email so we get the information we need (affected
component, vulnerability class, reproduction steps, impact) on the first
pass instead of a back-and-forth.

**Do not** open public GitHub issues, discussions, or pull requests for
undisclosed security vulnerabilities. Filing one publicly discloses the
issue before a fix exists, which puts every deployed instance of the
affected contract at risk. (Non-sensitive hardening suggestions — e.g. a
defense-in-depth improvement with no exploitable path today — are fine as
a normal issue or PR; when in doubt, email first and we'll tell you.)

We aim to:

- **Acknowledge** your report within **48 hours**.
- **Provide an initial severity assessment and fix timeline** within
  **5 business days** of acknowledgment.
- **Keep you updated** as the fix progresses, and credit you (unless you
  ask to stay anonymous) in the release notes once it ships.

## Report Owners

Vulnerability reports are triaged and coordinated by the contact(s) listed
below. This list is intentionally small and kept current in this file
rather than requiring reporters to guess who's active.

| Role | Contact |
|------|---------|
| Primary security contact | [@therealjhay](https://github.com/therealjhay) — reachable via security@bettapay.com |

If you don't get an acknowledgment within 48 hours, that's itself worth
flagging — try opening a normal (non-sensitive) issue asking someone to
check the security inbox, or reach out to any active maintainer directly.

## Responsible Disclosure

We request a **90-day disclosure window** from the time a fix is deployed
before any public write-up or disclosure of the vulnerability's details.
If a fix requires longer (e.g. it depends on a coordinated multi-party
deployment), we'll communicate a revised timeline before the 90 days are
up rather than let the window lapse silently.
