# Contributing

Pick an issue, keep the change inside its scope, and follow the
[design principles](book/src/principles.md). Keep each file and function to one
responsibility.

Run the focused tests for what you touched, then the gate your change needs.
[Changing Based](book/src/contributing.md) lists which gate proves which kind of
change. The pull request must pass every triggered check in
[CI](.github/workflows/ci.yml).

Report bugs in [issues](https://github.com/oscscull/baseddsl/issues/new/choose)
with a small reproduction using made-up data. Never include connection URLs,
credentials, tokens, or private rows. Report vulnerabilities privately; see
[SECURITY.md](SECURITY.md).

Review and support are best effort, with no response-time commitment and no promise
to implement every request. Supported surfaces are listed in
[compatibility and reports](book/src/support.md).
