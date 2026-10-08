# Specification and design priorities

This directory describes the language's detailed semantics and design intent.
Start with [principles](principles.md) for priority and ambiguity tiebreakers, then
use the focused [syntax chapters](syntax/) for the relevant construct. The
[external guard protocol](external-guards.md) specifies the versioned callback
wire contract. [Commerce](examples/commerce/README.md) is a broad example schema.

For the current usable syntax surface, read the
[language reference](../docs/reference.md). For an executable first run, use the
[embedded or standalone walkthroughs](../docs/initializing.md). The
[support policy](../docs/support-policy.md) states the tested release boundary;
design intent alone is not evidence that a configuration is supported.

This index is navigation. It does not duplicate syntax or reinterpret the
principles. Implementation questions should be checked against the compiler,
focused tests, and the documented support evidence before making a release claim.
