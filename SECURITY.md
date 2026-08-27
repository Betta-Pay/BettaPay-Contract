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
