# Context selection

## What it is

A selector that returns [knowledge records](../guide/00-concepts.md#assessment) for an agent's run and never blends
them: each record keeps its issuer, its attribution as stored, its standing (current, retracted, basis withdrawn,
expired) and the reader's verdict for its subject, computed over the records the caller can read, and is placed in
one class from the last two — the
[context selection](../guide/00-concepts.md#context-selection) rule of decision D14, *a summary is a selection*. The
caller's mandate scopes what it reads, inside the mandate's window: a record is readable when its subject lies under
the mandate's scope and every record it was derived from is readable too. A condensed text — an LLM's paragraph over a
selection — is written back only as a new claim signed by its own issuer, linked by `DerivedFrom` to every source, its
sources all in one class and its own class, under its subject, the same.

## When to use it, and when not

- **Use it when** an agent assembles context from the knowledge layer and the context must say who said what, and how
  each statement stands, rather than what a summariser concluded.
- **Not when** you need ranking or retrieval: the ranking is your closure and there is no vector store; nor for the
  wiki's pages, which carry no issuer or verdict (stated in the [module docs](../../src/knowledge/selection.rs)).

## Turn it on

This feature is turned on in code.

- [`ReadScope`](../../src/knowledge/selection.rs) — built from a current mandate that enumerates
  [`KNOWLEDGE_READ`](../../src/knowledge/selection.rs), its scope a plain path; its `view` is the direct read under the
  same rule, refused past the mandate's window.
- [`Selector`](../../src/knowledge/selection.rs) — built inside the window; `select` with your ranking closure and a
  [`VerdictSource`](../../src/knowledge/selection.rs) called with the readable records only; verified records only
  unless you call `admit_unchecked`; `condensation_draft` for the unsigned draft over one class of a selection;
  `write_condensed` (or `check_condensed` before a durable store's own `put_signed`).
- `a_scoped_selection_excludes_what_the_mandate_does_not_cover_and_what_derives_from_it`, in
  [`src/knowledge/selection.rs`](../../src/knowledge/selection.rs): a record outside the mandate's scope, one derived
  from it and one derived from a record the store does not hold are all left out, while the store holds all four.

## See it work

No runnable example exists: the [example index's capability matrix](../../examples/README.md#the-capability-matrix)
has no row for this feature. The tests, in [`src/knowledge/selection.rs`](../../src/knowledge/selection.rs):
`a_condensed_record_is_refused_without_its_issuers_signature`,
`a_condensed_record_is_refused_across_a_verdict_boundary`,
`a_condensation_is_refused_unless_its_own_class_is_its_sources_class`,
`a_condensation_over_expired_records_is_refused`, `a_condensation_signed_under_a_revoked_key_is_refused`,
`a_scoped_selection_excludes_what_the_mandate_does_not_cover_and_what_derives_from_it`,
`a_verdict_is_computed_over_what_the_scope_can_read`, `a_selection_past_the_mandates_window_reads_nothing`,
`the_class_is_re_derived_at_write_time` and `a_retracted_source_reaches_the_condensation_as_basis_withdrawn`.

## What it guarantees, and what it does not

- A selection reads only through the scoped view, inside the mandate's window, so it holds no record the caller could
  not read directly under the same mandate, and its verdicts are computed over those records only; a withheld record is
  reported by id with one reason whether its source is out of scope or not held. The scope matches by path segment —
  a selection-only rule; elsewhere a mandate's scope is exact — and never covers an empty, `.` or `..` segment.
- A condensation is refused unless it is signed by its own issuer under a key not revoked, a claim (never evidence), in
  scope, linked only by `DerivedFrom` to readable sources, none expired, all in one class, and its own class under its
  subject the same — re-derived when it is written, so a source retracted since the selection moves it out of its class.
  `InsufficientEvidence` and `Rejected` are two classes, and a `Retracted` or `BasisWithdrawn` record is never in a live
  one's.
- A selected record is returned unchanged: a `Supports`, `Challenges` or `Retracts` link names its target's id even
  where the target is not readable (the id, not the content).
- **Not built** ([plan row Y1](../plans/adopter-surface.md#y1--a-provenance-aware-context-selector-d14)): the wiki
  side; a read mandate's epoch and revocation standing (a knowledge store installs no epoch); re-verifying a selected
  record against the reader's current key view, which `classify_eligible` does for evidence; scoping the
  `KnowledgeStore`'s own methods, which are its holder's — no route or SDK verb reads knowledge records.

## Go deeper

- [Guide 23, knowledge](../guide/23-knowledge.md) and the [knowledge-layer design record](../design/knowledge-layer.md).
- No design record of its own; decision D14 and [plan row Y1](../plans/adopter-surface.md#y1--a-provenance-aware-context-selector-d14) are the record.
- Code: [`src/knowledge/selection.rs`](../../src/knowledge/selection.rs).
