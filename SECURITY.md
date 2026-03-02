# Security Policy

## Supported Versions

imago is currently pre-1.0. At this stage, we only support the latest code on the `main` branch.

If a security fix is accepted, maintainers will apply it to `main`. Backports to older commits, tags, or forks are not guaranteed.

## Reporting a Vulnerability

Please report vulnerabilities through GitHub private advisories:

- https://github.com/yieldspace/imago/security/advisories/new

Do not open a public issue for undisclosed vulnerabilities.

## What to Include

Please include as much detail as possible:

- A clear description of the vulnerability and impact
- Affected component, path, or crate
- Reproduction steps or proof of concept
- Environment details (target, OS, architecture, version/commit)
- Any suggested mitigation, if available

## Response Timeline

Our target response windows are:

- Initial acknowledgement: within 3 business days
- Status updates: at least once per week while the issue is open

Resolution time depends on severity, complexity, and release timing.

## Disclosure Policy

After a fix or mitigation is available, maintainers coordinate public disclosure.

Disclosure may include:

- A security advisory
- A release note and/or changelog entry
- Credit to the reporter (with consent)

## Scope

This policy covers security issues in this repository, including:

- CLI, daemon, runtime, protocol, plugins, and supporting scripts

Issues that are only about third-party dependencies should still be reported if they affect imago users. We will coordinate remediation where possible.
