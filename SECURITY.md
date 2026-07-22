# Security Policy

## Supported versions

Only the [latest release](https://github.com/Stake-Dev-Tool/stake-dev-tool/releases/latest)
receives security fixes. The desktop app auto-updates, so staying current is
automatic for most users.

## Reporting a vulnerability

**Please do not open a public issue for security problems.**

Report privately via
[GitHub's private vulnerability reporting](https://github.com/Stake-Dev-Tool/stake-dev-tool/security/advisories/new).
Include what you found, how to reproduce it, and what impact you believe it
has.

We'll acknowledge your report as quickly as we can — usually within a few
days — keep you posted on the fix, and credit you in the release notes
unless you prefer otherwise.

## Scope

Anything that breaks a security boundary is in scope, for example:

- Local CA / certificate handling on the desktop app
- RGS or dashboard authentication bypass
- Session or workspace isolation (one tenant reading another's data)
- Share-link isolation (`*.play.` origin separation from the dashboard)
- File-system escape through math folder or upload handling
- Billing and quota enforcement bypass

Self-hosted deployments run the same code as the hosted instance, so a
vulnerability in one almost always affects the other — please report either
way.
