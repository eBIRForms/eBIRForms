# bir-rules

`bir-rules` is the UI-agnostic runtime boundary for revision-pinned BIR form
rules. It contains the typed request/result contracts, static IR interpreter,
sealed provider boundary, exact-identity registry, closed serialization
primitives, and deterministic generated rule-set snapshots used by `bir-core`.

The canonical extracted evidence does **not** live in this crate. It remains in
[`../../rules`](../../rules/README.md), where source hashes, research notes,
schemas, fixtures, and extraction tools can be reviewed without being packaged
into the application.

## Dependency direction

```text
rules/ evidence
    |
    | explicit deterministic code generation
    v
bir-rules  <-  bir-core  <-  bir-desktop (GPUI)
```

- `bir-rules` must not depend on `bir-core`, GPUI, the database, networking, or
  submission code.
- `bir-core` owns form-specific adapters, checked XML/payload proof,
  persistence, final-copy checks, and submission revalidation.
- `bir-desktop` consumes the same report types through `bir-core`. It must not
  implement a second copy of a validation or tax formula.

## Intended source layout

```text
src/
  context.rs       validation phases and behavior profiles
  identity.rs      exact rule-set/revision identity
  value.rs         stable field instances and exact raw/canonical values
  issue.rs         complete, ordered validation reports
  evaluation.rs    fallible requests, results, coverage, and errors
  static_ir.rs      generated-static interpreter and fail-closed spec checks
  serialization.rs closed artifact identity, formatting, and body-codec primitives
  serialization_contract.rs generated borrowed artifact plans
  materialization.rs opaque ordered materialization trace and proof digests
  provider.rs      sealed compiled rule-set trait
  registry.rs      exact-identity packaged snapshot lookup
  generated/       tracked compiler output; currently an empty registry
```

The tracked `generated/` module is currently an empty reviewed registry plus a
hash-bound manifest. The 2550Q v2 skeleton is audited but is deliberately
excluded because it is not reviewed or executable; there is no generated
2550Q provider or production caller. Its 188-record v1 field inventory
contains 160 concrete fields plus 28 unbounded family-member descriptors that
must become members of seven logical v2 groups, not 28 group objects. The
skeleton still has no typed groups, fields, rules, calculations or executable
fixtures, and its source digest/profile/evaluation-policy/workflow blockers
remain unresolved.

For a reviewed snapshot, codegen emits static Rust and constructs a sealed
`StaticCompiledRuleSet`; the runtime does not read or parse `rules/` JSON.
Both profiles must carry an explicit reviewed `evaluation_policy` choosing
`ApplyAll` or `StopEffectsAfterFirstBlockingIssue`. The currently emitted safe
subset supports typed normalization/coercion, exact calculations and
expressions (including decimal division with mandatory expression-local scale
and rounding policy), explicit singleton/per-group calculation and rule
scopes, instance-aware derived/rule coverage, aggregate-over-row expressions,
repeated groups, logical/comparison/presence/membership predicates, and
`emit-issue`. Codegen rejects regex `matches` and
`set-derived`, `normalize-field`, and `set-workflow-state` effects.

Before inventory or evaluation, the static provider validates identifiers,
group membership, dependency order/availability, all field/context/derived/
group references, expression and predicate types, and effect targets/types
across both profiles and every declared phase/scope. Unsupported effects fail
closed after their static content is checked.

The public `serialization` module provides closed targets for editable save,
finalized save, encrypted Final Copy, submission payload, and
historical-import compatibility, each with an exact variant ID. It also
provides explicit absent and blank policies, exact typed formatting, and
deterministic raw, legacy JavaScript `escape`, and RFC 3986-unreserved UTF-8
percent codecs. Seventeen focused tests cover these primitives. The mandatory
v2 serialization subtree is now schema-checked, audited, digest-bound, emitted
as borrowed Rust statics, and exposed read-only by a generated provider. It
describes exact artifact/variant branches, ordered nodes, projections,
formatters, presence, groups, and codecs. The sealed materializer selects one
exact branch with no default, re-evaluates the request, validates the complete
plan, and produces a source/group/projection/omission/format/codec trace with
bound contract, raw-input, evaluation, and record-manifest digests. It does
not emit artifact bytes or authorize filing. Derived source records retain the
exact group instance, and the evaluation/record-manifest domains are v2 so an
older singleton-only proof cannot verify under group-scoped semantics.

`bir-core` additionally has an ordered, duplicate-preserving pseudo-XML
occurrence parser, an opaque `CheckedSerializationArtifact`, checked-payload v2
proof machinery, and v16 persistence. The checked serialization path
re-resolves the exact provider, independently walks the selected contract,
recomputes sources/formatters/codecs, renders the plaintext, parses every byte
back, and binds the plaintext and contract proofs. The older manifest coverage
path is test-only and exercises exact occurrence order, encoded bodies,
semantic sources, bindings, and hashes. Production
`CheckedFinalCopyPayload::try_new` deliberately returns
`MissingSerializationContract`; no adapter can construct a trusted payload
from independently supplied semantic and encoded values, and the checked
plaintext artifact has no conversion to that type. A reviewed form contract
must still account for pseudo-XML fields, dynamic groups, metadata, literals,
envelope bytes, codecs and outer-container binding before Final Copy or queue
wiring can open. None of this changes form capability or release status.

The crate does not claim that the prose-oriented v1 corpus is executable.

See
[`../../docs/validation-rules/architecture.md`](../../docs/validation-rules/architecture.md)
and
[`../../docs/validation-rules/implementation-plan.md`](../../docs/validation-rules/implementation-plan.md).
