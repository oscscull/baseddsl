# principles.md

Tiebreakers, in priority order. Apply when a case isn't specified.

1. **Safe by default; dangerous is explicit + visible.** Safe state is the silent default. Dangerous/guarantee-forfeiting state requires a visible token.

2. **Nothing consequential is true by omission.** Elide a property only if: omission has one meaning, that meaning is safe, deviation is written visibly. Else make it explicit.

3. **No positional/whitespace meaning.** Structure via delimiters (`{}`, `:`, `()`). Banned: significant indentation; two adjacent bare tokens where the gap is the syntax.

4. **One source of truth; point at it.** A fact is declared once, referenced by name. Never declare the same fact from two places that can drift.

5. **Push in a semantic only if the compiler can know it → warn/guarantee something.** In: schema/access semantics (deletion, cardinality, indexing, traversal). Out: app logic (validation, business rules, computation) → host-language seam. Nothing Turing-complete in the DSL.

6. **Escape hatches: mandatory, minimal-scope, never silent.** Forfeit only guarantees needing comprehension (keep param-binding). Smallest scope. Greppable. Engine detects the gap and lints it even when it can't fill it.

7. **Own the brutal lifecycle; lend the intent.** Engine owns dangerous scaffolding (tx boundaries, batching, capture). Caller supplies intent. Reuse hardened external tools, don't rebuild.

8. **Show, don't write, for derived facts.** Engine-derivable facts (inverse names, inferred indexes) shown in editor (LSP), not forced into source.

## Hard priorities
1. Brevity (context windows).
2. File separation — one model per file, self-contained for its owner.
3. Readability — reviewer confirms design by reading.
