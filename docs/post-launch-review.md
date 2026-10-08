# Bounded post-launch review

This is a maintainer checklist, not a scheduled monitor or telemetry system.
After the owner posts the reveal, review its public discussion and new GitHub
issues once after the first few days, then once after roughly two weeks. Adjust
later review frequency to actual activity; no daily status promise or support SLA.

For each voluntarily reported problem, classify the barrier:

| Barrier | First useful action |
|---|---|
| Could not install | Reproduce the exact version/platform path, fix the install instructions or artifact, and verify from a clean environment |
| Installed but could not reach useful work | Replay the documented first result/edit; fix missing steps or stale artifacts |
| Missing capability | Check the declared support boundary and record the concrete use case; do not promise a feature or silently expand scope |
| Genuine bug | Request the smallest safe reproduction, search duplicates, and prioritize data safety/authorization and reproducible abandonment barriers |

Keep version/repro/expected-actual fields small and route sensitive material privately.
Use the existing issue form rather than adding compiler diagnostics or private-data
requirements. Clarify a reproduction once when useful; do not repeatedly chase silent
users or recruit a committed volunteer cohort.

At each bounded review, record what installed/reached useful work, specific recurring
use reports, repeated barriers and concrete requests. Cite public/voluntarily provided
evidence, keeping sensitive details private. Distinguish observations from guesses.
Stars/views can indicate discovery, but recurring use and actionable reports are
stronger evidence of utility. Silence does not prove satisfaction; a quiet launch
is an outcome to learn from, not a failed adopter quota or fabricated success.

Choose a small follow-up priority set based on severity and repeated barriers,
link the relevant issues and recheck the affected first-use path after fixes. Do
not add mandatory analytics, database collection, outreach, growth promises or
support commitments. Future work is chosen explicitly rather than inferred from
a reaction count.
