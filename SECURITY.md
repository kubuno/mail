# Security Policy

Kubuno is a self-hosted platform that people run to hold their own data. We take
security reports seriously and are grateful to those who disclose responsibly.

## Reporting a vulnerability

**Please do not open a public issue for security problems.**

Report privately through either channel:

- **GitHub Security Advisories** — use the *Report a vulnerability* button under
  the **Security** tab of this repository (preferred: it keeps the discussion
  private and lets us credit you).
- **Email** — `security@martinienolinga.com`. Encrypt with our PGP key if the details
  are sensitive (key fingerprint published at the project website).

Please include, as far as you can:

- the component and version (`/api/v1/health` reports the running version, or see
  `/etc/kubuno/VERSIONS` in a Docker install);
- a description of the issue and its impact;
- steps to reproduce, a proof of concept, or affected source locations;
- any suggested remediation.

## What to expect

| Stage | Target |
|---|---|
| Acknowledgement of your report | within **72 hours** |
| Initial assessment and severity triage | within **7 days** |
| Fix or mitigation plan communicated | within **30 days** |
| Public disclosure (coordinated) | after a fix is released, by agreement |

We will keep you informed of progress, credit you in the release notes and the
advisory unless you prefer to stay anonymous, and coordinate the disclosure
timeline with you.

## Scope

In scope: the code in this repository and the official distributions we publish
(`.deb`, `.rpm`, Windows/macOS installers, the Docker image).

Out of scope: vulnerabilities in third-party dependencies already tracked
upstream (report those upstream; tell us if a pinned version leaves us exposed),
issues that require a compromised host or physical access, and findings against a
misconfigured deployment that departs from the hardening guidance in the docs.

## Supported versions

Security fixes land on the latest released minor version. Because Kubuno is a
polyrepo, each component (core and each module) is versioned and released
independently; report against the component and version where you observed the
issue.
