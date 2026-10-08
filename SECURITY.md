# Security reporting

Use GitHub's [private vulnerability report](https://github.com/oscscull/baseddsl/security/advisories/new)
for a suspected authorization bypass, credential disclosure, unsafe mutation or
other security-sensitive behavior. Private vulnerability reporting is enabled for
this repository. Sign in to GitHub; the repository's Security → Advisories page
also provides **Report a vulnerability**. Do not open a public issue with an
exploit, connection secret, private database row or unapproved source excerpt.

Provide the affected version/commit, deployment mode/database, expected and actual
behavior, impact and the smallest safe reproduction. Use fabricated data and
redacted configuration. Explain which component derives trusted context and which
network callers can reach the listener when relevant. Do not send live credentials
or a database dump. Keep further details in the private report until a disclosure
plan is agreed with the maintainer.

Ordinary bugs and feature requests belong in
[public issues](https://github.com/oscscull/baseddsl/issues/new/choose), with secrets
removed. If unsure whether a report is sensitive, start privately.

Support is best effort, with no acknowledgement/remediation SLA, security
certification or guaranteed support for every historical version. The
[support and compatibility policy](docs/support-policy.md) describes the current
candidate and v1 target; it does not declare v1 released. Give the exact version
so the maintainer can assess exposure and supported upgrade options. Never
replace a published version silently; remediation/upgrade guidance belongs with
the affected release and its advisory.
