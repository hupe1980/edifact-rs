# Changelog

All notable changes to `edifact-rs` and `edifact-rs-derive` are documented here.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

---

## [Unreleased]

---

## [0.15.0] — 2026-08-09

A second audit pass over the same pipeline, plus the feature that made a whole
class of real-world interchanges parseable at all. Three of the fixes below are
cases where the crate produced or rejected data **incorrectly**; one of them —
the derive dropping components on write — was silent data loss on the way out.

### Added

- **Character repertoire support (`UNB` S001 DE 0001).** UTF-8 is not a superset
  of `UNOC`: `UNOC` is ISO 8859-1, where `ü` is the single byte `0xFC`, so a
  conformant German interchange was rejected outright as
  [`EdifactError::InvalidText`] (`E003`). The same held for every
  `UNOD`…`UNOK` payload — a whole class of real, standards-compliant EDIFACT the
  crate simply could not read.
  New [`Charset`] covers `UNOA`, `UNOB`, `UNOC`–`UNOK`, and `UNOY`.
  `decode_interchange` reads the repertoire out of the interchange's own `UNB`
  and transcodes; it **borrows** — copying nothing — when the payload is already
  ASCII or `UNOY`, so the zero-copy path is untouched for everyone it does not
  affect. `Charset::decoding_reader` is the streaming counterpart, so the
  constant-memory guarantee survives for `UNOC` input too. `sniff_charset` reads
  the identifier byte-wise, without parsing a `UNB` whose sender name may itself
  be Latin-1, and `decode_reader` combines the two: it buffers only enough of the
  stream to find the `UNB`, then streams the rest through the right decoder — so
  the repertoire never has to be known in advance.
  `UNOX` and `KECA` report the new [`EdifactError::UnsupportedCharset`] (`E039`)
  rather than being silently mis-decoded: both are stateful or multi-byte, which
  would make byte-level delimiter scanning unsound.
  The ISO 8859 tables are generated from the reference codecs, not hand-typed,
  and a byte in a slot the standard leaves undefined is an error rather than a
  substituted replacement character.
- **`Writer::with_charset`.** Binds a writer to a repertoire so values are
  encoded into it rather than emitted as UTF-8 — a `UNOC` header over a UTF-8
  body arrives as mojibake with nothing in the file to explain why. A character
  the repertoire cannot carry is refused with
  [`EdifactError::CharacterNotInRepertoire`] (`E038`), and a `UNB` declaring a
  repertoire the writer does not encode is refused with
  [`EdifactError::CharacterRepertoireMismatch`] (`E041`).
- **`CharsetValidator`**, wired up by
  `ValidationContextBuilder::with_charset_validation` /
  `with_charset_validation_for`. Reports payload that its own `UNB` says should
  not be there — a `UNOA` interchange carrying a lower-case letter is the check
  partners run *after* you have sent the file. Decoding stays permissive so an
  encoding finding never hides the rest of the report.
- **ISO 9735 service-segment layouts** in the new `edifact_rs::service` module:
  `UNB`, `UNG`, `UNH`, `UNT`, `UNE`, `UNZ`, `UNS`, plus composites `S001`–`S018`.
  Code-addressed access needs a [`SegmentDefinition`] to resolve against, and
  requiring every consumer to hand-author `UNB` first put the crate's headline
  safety feature out of reach for the segments *every* EDIFACT program touches.
  These are fixed by the syntax standard rather than by a directory release, so
  there is one correct answer and no version to choose; they are not the
  separately-licensed directory data the crate still does not ship.
  A test pins them against the indices `envelope.rs` reads by hand, so the two
  cannot drift.
- **`ComponentRef::repeated`** (and `OwnedComponentRef::repeated`), for the
  composites that repeat a data element by design — `C080 PARTY NAME` is `3036`
  five times, `C059 STREET` is `3042` four times. Declaring those faithfully as
  five `ComponentRef::new` entries made `code_positions` count five positions, so
  the component came back [`AmbiguousDataElement`] and could not be code-addressed
  **at all**: a faithful declaration was punished, and the only way to use named
  access was to declare the composite incompletely and disagree with the
  directory it claims to model. One `repeated` entry is one addressable position
  that still records how many slots belong to it.
- **`OwnedElement::of` / `OwnedElement::and_repeat` / `OwnedSegment::new`**, plus
  `with_span` / `with_spans`. Synthesising an `OwnedSegment` from a non-EDIFACT
  source previously had no constructor and required a struct literal.
- **`MessageWindow::body` / `OwnedMessageWindow::body`** — the segments between
  `UNH` and `UNT`. `segments` deliberately includes the service segments, but
  `group_segments_indexed` is driven by trigger tags alone, so a trailing `UNT`
  landed inside whichever group ran last. `body()` is the one-call answer.

### Breaking Changes

- **`Segment`, `Element`, `OwnedSegment`, and `OwnedElement` are
  `#[non_exhaustive]`.** Adding `repeats` in 0.14 broke every downstream struct
  literal, and the next field would have done it again. Each now has a
  constructor (`Segment::new`, `Element::of`, `OwnedSegment::new`,
  `OwnedElement::of`) with builder-style setters for spans, so further fields are
  additive. Fields stay public: reading and `..` destructuring are unaffected.
  This is the pattern [`SegmentDefinition`] has always used.
- **Group schemas are no longer forced to be `'static`.** `GroupDef` and
  `SegmentGroupIndexed` gained a lifetime parameter, so a schema deserialized from
  a MIG at startup works exactly like a `static` table. `GroupDef<'static>` is what
  a `static` table already is, so existing schemas — and every `static SCHEMA:
  &[GroupDef]` in the wild — compile unchanged; only explicit type annotations
  naming the bare types need `<'_>`. This closes an inconsistency with the
  directory side of the crate, where [`OwnedSegmentDef`] and
  [`DirectoryValidatorBuilder`] have always supported runtime-loaded definitions.
  `GroupDef::new(name, trigger)` and `GroupDef::with_children(name, trigger,
  children)` are `const` constructors; the fields stay public.
- **Serializing a non-finite float is an error** ([`EdifactError::NonFiniteNumber`],
  `E040`) rather than the text `NaN` / `inf`, which is not an EDIFACT numeric data
  element, which no receiver can parse, and which this crate's own reader hands
  back as an ordinary string. An arithmetic bug now fails at the boundary instead
  of days later.
- A segment struct with no data elements now serializes as `UNS'` rather than
  `UNS+'`, matching `Writer::write_elements(tag, elements![])`.

### Fixed

- **The derive dropped every component but the last when writing.**
  `EdifactSerialize` laid fields out on a map keyed by the data element index
  alone, so two fields sharing an element collapsed into one entry:
  `#[edifact(component = N)]` was honoured on read and silently ignored on write.
  A `DTM` mapped to three component fields round-tripped `137:20260101:102` out as
  `DTM+102'`, and a `NAD` lost its party identifier. The layout is now a
  two-dimensional (element × component) grid resolved at macro-expansion time, so
  the generated code is still straight-line event emission with no runtime
  allocation, and gaps in either dimension emit the empty slot they should.
  Only the positional path was affected; the code-addressed path already went
  through `emit_sparse_segment`.
- **Repetition spans were not rebased onto the stream on the reader path.**
  `OwnedSegment::offset` walked each element's `components` but silently skipped
  its `repeats`, so under a syntax-version-4 `UNA` every occurrence after the
  first carried a span relative to the *start of its own segment*. Diagnostics
  pointed into unrelated bytes and any caller slicing the input by span read the
  wrong value. `from_bytes` was unaffected; `from_reader` and every API built on
  it were not. `OwnedElement::offset_in_place` is the shared implementation both
  paths now use.
- **A rejected repeating element left a half-written segment in the sink.**
  `Writer::write_segment` emitted the tag and the first element separator before
  discovering that the active service string advice declares no repetition
  separator, so a caller that logged
  [`EdifactError::RepetitionSeparatorNotDeclared`] and continued produced an
  interchange with a dangling `RFF+` spliced in front of the next segment. The
  check now runs before the first write: a rejected segment writes **nothing**
  and the writer stays usable.
- **A segment tagged `UNA` parsed differently through a reader than through a
  slice.** `UNA` is three ASCII uppercase letters and therefore a legal segment
  tag. The reader re-tokenizes each segment from its own slice, where the
  whole-interchange rule "a leading `UNA` is a nine-byte service string advice"
  is wrong — it ate the tag and its first element, and bytes that parsed cleanly
  through `from_bytes` came back as `InvalidSegmentTag` (`E006`) through
  `from_reader`. Both reader paths now use the new `Tokenizer::for_segment`,
  which parses a bare segment with no header heuristic.
- `InterchangeEnvelope::sender_routing_address` was documented as `UNB` S002
  **DE 0014**. It is **DE 0008**; 0014 is the recipient-side component in S003.
  The field and its behaviour are unchanged — only the documentation was wrong,
  which is worse for a reference than an outright gap.
- `DirectoryValidator` counted a composite's declared components by entry rather
  than by slot, so a composite using `ComponentRef::repeated` would have capped
  its arity far too low.

### Changed

- `SegmentDefinition::element_slot` / `component_slot` now report *which* problem
  occurred — unknown identifier versus declared at several positions — instead of
  one message covering both. A `const` panic message cannot be formatted, so
  naming the identifier is impossible, but naming the problem decides what the
  author has to change.
- `ProfileRulePack` message-type and release scoping share one implementation
  across the flat and the group pass instead of two copies, and the `UNH` lookup
  now happens only for the scopes a pack actually configures — a pack bound with
  `for_release` used to rescan for `UNH` even when the message type had already
  been extracted.
- `qualifier_matches_pattern` drops a redundant overlap check from its
  single-wildcard fast path; the length test it duplicated already decides the
  same question.

### Documentation

- New guide: [Character Sets](https://hupe1980.github.io/edifact-rs/docs/character-sets/).
  *Core Concepts* no longer claims UTF-8 input as an unqualified requirement, and
  the `E003` entry no longer describes `UNOA` as a "Latin-1 subset" (it is an
  ASCII subset) or suggests transcoding as the only remedy.
- The README's **Scope** section now distinguishes the service segments the crate
  ships from the directory data it does not, with a worked example.
- The group-validation sections of *Validation* and *Profile Packs* no longer
  tell readers that a schema "must be a static", and *Validation* gains a
  runtime-schema example plus a note on what a group actually spans — grouping is
  driven by trigger tags alone, so nothing stops the final group at `UNT`.
- *Writing* gains a character-repertoire section, and it and *Error Reference*
  state the writer's no-partial-write guarantee for `E037`.

---

## [0.14.0] — 2026-08-09

A correctness audit of the parse → validate → write pipeline. Four of the changes
below fix behaviour that silently produced or accepted **wrong data**; the rest
close documented-but-missing features and gaps in the guides.

### Breaking Changes

- **`ReaderConfig` budgets report a violation instead of truncating.**
  `max_segments`, `max_messages`, and `max_input_bytes` used to end the iterator
  by returning `None`, which is indistinguishable from a clean end of input. The
  idiomatic `collect::<Result<Vec<_>, _>>()` therefore *succeeded* on a truncated
  interchange and every downstream stage treated a fragment as the whole message.
  All three now yield [`EdifactError::LimitExceeded`] (`E036`). Input that ends
  exactly at a limit is not a violation, and segments parsed before the violation
  are still delivered. `max_input_bytes` is now a true cap: a segment whose end
  offset would pass the budget is never handed out.
  `from_bytes_with_config` also honours `max_messages`, which previously only the
  reader path enforced.
- **`Element` and `OwnedElement` gained a `repeats` field.** Code that constructs
  them with a struct literal must add `repeats: Vec::new()`; `Element::of` and
  every accessor are unchanged. See *Added* below.
- **`ValidationSeverity` orders by severity.** The derived `Ord` followed
  declaration order, ranking `Critical` **lowest** — so `max_by_key(|i| i.severity)`
  returned the least important issue, contradicting `numeric_level()`. The order is
  now `Info < Warning < Error < Critical`.
- **`DirectoryValidator` reports every violation in a segment.** It returned after
  the first `Err` per segment, so a validator whose whole purpose is an exhaustive
  report showed one issue per segment and callers fixed messages one round-trip at
  a time. A segment with two missing mandatory elements and a bad code is now three
  findings.
- **Writing a repeating data element without a declared repetition separator is an
  error** ([`EdifactError::RepetitionSeparatorNotDeclared`], `E037`) rather than
  output joined with the space sentinel, which read back as a single occurrence.

### Added

- **ISO 9735-4 §3.1 repeating data elements.** The repetition separator at `UNA`
  position 7 was parsed into `ServiceStringAdvice` and escaped by the writer, but
  the tokenizer never split on it — so `RFF+ON:1*ON:2` under a syntax-version-4
  `UNA` parsed as *one* occurrence whose second component was the literal text
  `1*ON`. Wrong data, delivered without a warning.
  `Element`/`OwnedElement`/`BorrowedElement` now expose `repeat_count()`,
  `repetition(n)`, and `repetitions()`; `components` still holds occurrence 0, so
  every positional accessor and both derives behave exactly as before on the
  interchanges that do not use the feature. `Element::and_repeat` builds one, and
  `ServiceStringAdvice::is_repetition_active()` reports whether the separator is
  live. Round-trips through `Writer::with_una` are byte-for-byte.
- **`#[derive(EdifactCompositeDeserialize)]` and `#[derive(EdifactCompositeSerialize)]`.**
  `#[edifact(element = N, composite)]` has always required these traits, and the
  guides showed the derives — but only the hand-written `Vec<String>` impl existed,
  so every composite example in the docs failed to compile. Fields map to
  components in declaration order, `#[edifact(component = N)]` overrides the index,
  and a bare `String` component is mandatory (`E021` when absent).

### Fixed

- **`Writer::write_segment` and `write_segment_parts` did not record `UNH`.**
  `finish_unt` then derived DE 0074 from the writer-lifetime total, so a preceding
  `UNB` inflated the count and the interchange failed its own validation. Every
  emit path — including the `WriterEmitter` event path — now shares one bookkeeping
  routine.
- **Duplicate control-reference detection was O(n²).** `UNH` DE 0062 and `UNG`
  DE 0048 uniqueness used `Vec::contains`; an interchange with 50 000 messages cost
  over a billion string comparisons. Now a `HashSet`.
- **An oversized segment containing multi-byte text reported `InvalidText`
  (`E003`) instead of `SegmentTooLong` (`E020`).** The scan window can cut a UTF-8
  sequence in half, and validating the text before the size guard blamed the
  payload for an encoding problem that did not exist.
- **`Writer::escape_value` no longer has a panic path.** It built a `Vec<u8>` and
  re-validated it with an `expect`; it now builds the `String` directly.
- **Removed an `unwrap` in envelope extraction** by folding the "inside a message"
  flag and the `UNH` index into one `Option`.
- **`ValidationReport::has_critical_errors` documented itself as O(1) "backed by an
  incrementally maintained counter".** It is a linear scan; the doc now says so.
- **`Components` and `OwnedComponents` are exported.** `Element::components` and
  the new `repeats` are public fields whose type had no name outside the crate.
- **The error-code documentation guard did not do what it claimed.** It compared a
  hand-written array of sample values against the reference guide and asserted that
  a new variant "fails to compile" — an array is not a match, and `EdifactError` is
  `#[non_exhaustive]`, so an integration test cannot match it exhaustively anyway.
  Two variants had already slipped past it. It now reads the codes straight out of
  `stable_code`'s exhaustive match and checks both the summary table and the
  per-code section.
- **`docs/performance.md` listed benchmark groups that do not exist**
  (`writer/large_message`, `validation/d11a_structure`, …); the table now mirrors
  the real Criterion IDs.

### Changed

- **`SegmentAccessor::repeating_components` is now `component_range`** (and
  `repeating_components_iter` → `component_range_iter`). It walks components
  *inside one data element*; with real ISO 9735-4 repetitions in the model, the
  old name pointed at the wrong concept.
- **Install instructions use `cargo add` instead of hand-written `[dependencies]`
  TOML.** The snippets carried a pinned version that had to be bumped in six
  places every release — and had already drifted a minor version behind. `cargo
  add` resolves the current version itself, so there is nothing left to go stale.
- **Documentation and fixtures no longer name specific organisations, industry
  bodies, or market verticals.** `edifact-rs` is a general-purpose ISO 9735
  library; examples now use neutral profile names and the UN/EDIFACT message
  types (`ORDERS`, `INVOIC`) that need no domain context.

### Build

- **Pinned `trybuild` to 1.0.119.** 1.0.120 raised its own MSRV to Rust 1.88,
  which broke the MSRV job. 1.0.119 is the last release that still supports 1.85.
- **A guide snippet illustrating attribute syntax was marked `rust`**, so rustdoc
  tried to compile a bare outer attribute with nothing after it. Newer toolchains
  happened to accept it; the MSRV job did not — which is exactly what that job is
  for. It is now `text`, like the other syntax illustrations.

### Site

- **The guides moved from `docs/` to a Zola site under `site/`** and are now
  published as a landing page plus a documentation section, with a CI job that
  builds and deploys to GitHub Pages. `README.md` links to the published guides
  rather than to raw Markdown, so the canonical copy is the one readers and search
  engines see.
- The guides are unchanged prose — they gained TOML front matter (title,
  description, ordering) and their cross-links became Zola's checked `@/` form.
  `zola check` validates every internal link **and anchor**, which immediately
  caught two dead anchors that plain relative links had hidden.
- Every Rust snippet is still compiled and run as a doctest from its new location,
  so the site cannot drift from the crate.
- SEO and accessibility come from the templates, not a plugin: per-page `<title>`
  and meta description, canonical URLs, Open Graph and Twitter tags, JSON-LD
  (`SoftwareSourceCode`, `WebSite`, `TechArticle`, `BreadcrumbList`, `FAQPage`),
  an auto-generated sitemap and Atom feed, semantic landmarks, a skip link, and
  visible focus rings. No JavaScript and no web fonts.
- **`README.md` shrank from 609 to ~215 lines.** It had grown into a second copy
  of the guides; it is now a front door — install, three worked examples, the
  design positions that distinguish the crate, and links to the site.

### Documentation

- **The guides are now actually compiled.** 45 of 126 code blocks were
  `rust,ignore`, so nothing checked them and they had drifted. Un-ignoring them
  surfaced roughly a dozen real bugs, all fixed: references to a
  `ValidationReport.errors`/`.warnings`/`.infos` *field* (private — the accessors
  are `errors()` and friends), `EdifactError::InvalidCodeValue { offset }` (the
  field is `span`), `WriterEmitter::into_inner` (it is `finish`), `to_bytes` on a
  `Segment` slice (that is `segments_to_bytes`), and derives that did not exist.
  Doctests went from 118 to 155 passing, with ignored blocks down from 68 to 33 —
  the remainder genuinely need `anyhow`, `tracing`, or a live file.
- **The `UNA` diagram in `core-concepts.md` was misaligned and mislabelled**, each
  arrow pointing one position right of its label and the decimal mark, release
  character, and repetition separator listed in the wrong order. Replaced with a
  correct diagram plus a table.
- **`docs/writing.md`'s custom-delimiter example wrote data elements separated by
  the *component* separator** and asserted a `UNA` prefix that omitted the
  repetition slot; `docs/parsing.md` had the same delimiter mix-up. Both now
  round-trip under assertion.
- `docs/performance.md` claimed the default `max_segment_bytes` was 512 KB; it is
  64 KiB.
- New sections: repeating data elements (`core-concepts.md`, `writing.md`), the
  `ReaderConfig` budget table and rationale (`parsing.md`), and `E036`/`E037`
  (`error-reference.md`).

[`EdifactError::LimitExceeded`]: https://docs.rs/edifact-rs/latest/edifact_rs/enum.EdifactError.html
[`EdifactError::RepetitionSeparatorNotDeclared`]: https://docs.rs/edifact-rs/latest/edifact_rs/enum.EdifactError.html

---

## [0.13.0] — 2026-07-26

Addresses feedback from downstream profile crates built on this library.

### Breaking Changes

- **`ValidationIssue::offset` is removed; `span` is the single positional field.**
  `offset` duplicated `span.start` and forced every consumer to read both.
  `with_offset` is gone — use [`with_span`]. To recover just the start, call
  `issue.start_offset()` or `issue.span.map(|s| s.start)`. Issues derived from a
  purely lexical fault (dangling release character, unexpected EOF) now carry a
  zero-width span at the offending byte.
- **`ValidationIssue::error_code` is `Option<Cow<'static, str>>`.** The field was
  `Option<&'static str>` with `#[serde(skip_deserializing)]`, so a report
  persisted to an audit store and read back always had `error_code = None` —
  filtering or routing on it after reload silently found nothing. It now
  round-trips as a plain string while a `&'static str` library constant still
  costs no allocation. `with_error_code` accepts `impl Into<Cow<'static, str>>`,
  so both `"E014"` and an owned `String` work; the `error_code()` getter returns
  `Option<&str>`.
- **Validation error variants carry `span: Span` instead of `offset: usize`.**
  Affects `InvalidSegmentForMessage`, `InvalidElementCount`,
  `InvalidComponentCount`, `InvalidCodeValue`, `QualifierMismatch`,
  `ConditionalRequirementNotMet`, and `DuplicateReference` — every variant raised
  from an already-parsed segment, where the full source range is known. Rendered
  diagnostics now underline the offending region instead of placing a zero-width
  caret; `InvalidCodeValue` points at the failing value rather than the segment
  start. The lexical variants (`UnexpectedEof`, `InvalidDelimiter`,
  `InvalidText`, `InvalidReleaseSequence`, `SegmentTooLong`,
  `UnexpectedDataToken`) keep `offset: usize`: a point fault has no meaningful
  end position.
- **`ElementRef` gained a `components` field.** Build simple elements with the
  unchanged `ElementRef::new` and composites with the new
  `ElementRef::composite`. `OwnedElementRef` gained
  `with_components` (its constructors are unchanged).
- **`ValidationReport::render_deterministic` prints `[span=start..end]`** where it
  previously printed `[offset=n]`.

### Added

- **Code-addressed element access.** Positional indices are a silent-misread
  hazard: transpose one and you read the wrong data element, and it still
  validates clean. Every segment type — `Segment`, `BorrowedSegment`,
  `OwnedSegment` — can now address data by its UN/EDIFACT data element
  identifier instead:
  - `value_by_code(layout, "3055")`, `span_by_code(layout, "3055")`, and
    `Segment::element_by_code(layout, "C082")`.
  - `value_at(path)` / `span_at(path)` for a path resolved once and reused.
  - `SegmentLayout` — the resolution trait, implemented by both
    `SegmentDefinition` (compile-time tables) and `OwnedSegmentDef`
    (runtime-loaded definitions) — plus `ElementPath`.
  - `ComponentRef` / `OwnedComponentRef` name the components inside a composite,
    so `"2380"` resolves to element *and* component position.
  - Three new errors make a bad reference loud: `UnknownDataElement` (`E033`),
    `AmbiguousDataElement` (`E034`), and `SegmentLayoutMismatch` (`E035`) when a
    layout is applied to a segment with a different tag.
- **`#[edifact(layout = PATH)]` + `#[edifact(element = "3055")]` in the derive.**
  Identifiers are resolved against the layout during *const evaluation*, so a
  stale or mistyped identifier — or two fields claiming one slot — is a **compile
  error**, and the generated code is the same index arithmetic it always was.
  `layout` must name a `const` item. Backed by the new const fns
  `SegmentDefinition::code_positions` / `element_slot` / `component_slot`.
- **`Writer::write_elements(tag, &[DataElement])`** — the general segment-emit
  form, for the everyday shape that mixes simple and composite data elements
  (`NAD+MS+id::agency`, `DTM+137:20260101:102`). Component boundaries are
  explicit, so a literal separator inside a value is escaped rather than promoted
  to a boundary, and nothing is allocated.
- **`elements!` macro and the `AsDataElement` trait** — shorthand for the above.
  Each entry is an ordinary expression borrowed through `AsDataElement`: a string
  becomes a simple data element, an array/slice/`Vec` of strings becomes a
  composite. `elements![qualifier.as_str(), [gln, "", agency]]` works the same as
  a list of literals, which is what a builder actually needs.
- `SegmentDefinition::code_is_component` — `const`, distinguishes an identifier
  that names a component inside a composite from one that names a whole data
  element. `component_slot` alone cannot: the first component of a composite also
  resolves to index 0.
- **`MessageWriter::write_elements` / `write_composites` / `write_segment_parts`.**
  The `UNH`/`UNT` guard only forwarded `write_raw` and `write_segment`, so
  callers needing the other forms dropped to the raw writer and their segments
  escaped the `UNT` DE 0074 count.
- `emit_sparse_segment` — emit one segment from `(element, component, value)`
  triples, filling gaps. Backs the code-addressed derive's serialize path.
- `DirectoryValidator` reports a missing mandatory **component** inside a declared
  composite, and caps a declared composite's arity — more components than the
  directory defines is `E013`, while fewer stays valid because conditional
  components may be omitted. An `expected_components` hook still wins for that
  element, since it is an exact count rather than an upper bound. Both checks are
  inert for definitions that declare no components, which is every pre-0.13 table.
- `ValidationIssue::start_offset()`.

### Fixed

- Three `README.md` examples never compiled: `Bgm` was referenced but never
  defined, `to_edifact_string` was called with only `ser` imported, and `?` was
  applied to `from_bufread_stream_with_config`, which returns a stream rather
  than a `Result`. The README is now compiled as a doctest alongside the guides,
  so it cannot drift again.
- `#[edifact(required)]` on a code-addressed field reported
  `MissingRequiredComponent` (`E021`) even when the identifier named a whole data
  element, where `MissingRequiredElement` (`E008`) belongs. Consumers routing on
  `error_code` took the wrong branch. The two deserialization paths (borrowed and
  owned) are now pinned to agree.
- `emit_sparse_segment` silently corrupted a segment when two triples claimed the
  same slot: emitting both shifted every later component one position right. The
  first value now wins and the rest are dropped.
- `SegmentLayout::resolve_code` scanned the definition up to four times per
  lookup (once for the count, again for the element index, again for the
  component index) by composing the `const` helpers. It is now a single pass —
  it runs per lookup on hot validation paths.
- **The derive UI suite is toolchain-portable again.** Every blessed `.stderr`
  captured an incidental `unused_imports` warning from the test-support stub,
  and rustc has since reworded the note it attaches — so 24 of 26 expectations
  mismatched on any toolchain newer than the MSRV, and a genuine regression in
  the derive's diagnostics would have been invisible in the noise. The warning
  is now silenced at the source, the expectations hold only the derive's own
  messages, and CI runs the suite on stable as well as MSRV. Three cases were
  added for the new `layout` attribute, which could not previously be blessed.

### Documentation

- New guide sections: code-addressed access
  ([validation.md](docs/validation.md)), `layout` / identifier-based `element`
  ([typed-derive.md](docs/typed-derive.md)), and `write_elements`
  ([writing.md](docs/writing.md)).
- [error-reference.md](docs/error-reference.md) documents `E033`–`E035` and the
  `offset` vs `span` split.

[`with_span`]: https://docs.rs/edifact-rs/latest/edifact_rs/struct.ValidationIssue.html#method.with_span

---

## [0.12.0] — 2026-07-20

### Breaking Changes

- **`Writer::begin_interchange` takes composite components separately.** The
  signature is now
  `begin_interchange(syntax_id, syntax_version, sender, recipient, date, time, control_ref)`.
  Previously `syntax_id` and `datetime` were passed pre-joined with `:`, which
  produced a collapsed single component under a non-default UNA.
- **`ServiceStringAdvice::is_valid` rejects alphanumeric delimiters.** Segment
  tags are written verbatim and cannot be escaped, so a delimiter such as `N`
  made `NAD` unrepresentable. Real-world UNA strings use punctuation only.
- **`EdifactError::QualifierMismatch` now maps to `Error`, not `Warning`.**
  It is only produced for UNZ/UNE/UNT control-reference mismatches, which are
  hard ISO 9735-1 violations; the previous mapping let a spliced or truncated
  interchange pass `validate_strict`.
- **`Status` and `SegmentDefinition` are `#[non_exhaustive]`.** Build segment
  definitions with `SegmentDefinition::new` instead of a struct literal.
- **`ValidationContextBuilder::with_profile_pack` no longer calls
  `set_message_type`.** The call was a no-op for `ProfileRulePack`; scoping comes
  from `ProfileRulePack::for_message_type`.

### Added

- `Writer::write_composites` — writes a segment from borrowed element/component
  slices, so component boundaries are explicit and a literal separator inside a
  value is escaped rather than promoted to a boundary.
- `EdifactError::DuplicateReference` (`E032`) — a `UNH` (DE 0062) or `UNG`
  (DE 0048) reference used more than once within an interchange.
- `Token` and `OwnedSegmentStream` are re-exported from the crate root; both
  appeared in public signatures but could not previously be named.
- `contiguous_groups_iter` re-exported from the crate root, alongside its
  existing sibling `contiguous_groups_by_qualifier`.

### Fixed

- **Quadratic scan in the tokenizer (DoS).** A value made of release sequences
  with no delimiter re-scanned the remaining input on every iteration, and the
  per-segment size guard was only applied after the loop. A 400 KB input took
  723 ms; it now fails in ~50 µs, and the cost no longer grows with input size.
- **Segments with no data elements were unparseable.** `read_tag` searched for
  the element separator across the whole remaining buffer before falling back to
  the segment terminator, so `UNZ'UNB+A'` was read as the tag `UNZ'UNB`.
- **`Writer::begin_message` hardcoded `:` in the S009 composite**, so a message
  written with a custom UNA could not be re-read by this library.
- **`Writer::finish_unt` counted the writer's lifetime segment total** into
  `UNT` DE 0074 instead of the current message, so an interchange written with a
  preceding `UNB` failed its own envelope validation.
- **The repetition separator was never escaped.** A value containing the
  separator declared by the active UNA was emitted unescaped, splitting into
  multiple repetitions for any ISO 9735-4 conformant reader.
- **`validate_envelope_lenient` stopped at the first error** for every
  structural error class, despite documenting exhaustive collection. Strict and
  lenient now share one implementation, so strict reports the first error the
  lenient path collects.
- **`LenientResult::into_strict` could panic.** `errors` is a public field, so
  filtering tolerated violations before converting is legitimate; the conversion
  is now total.
- **`ValidationIssue` could not be deserialized when `context` was empty**
  (`serde` feature): the field had `skip_serializing_if` with no matching
  `default`, so every issue the library produced failed to round-trip.
- **`has_critical_errors` and `bail_on_first_critical` silently stopped working**
  on reports that had been deserialized or merged; the cached counter is gone.
- **Envelope-segment filtering depended on validator registration order**, so
  moving `with_envelope_validation()` in a builder chain changed the report.
- **`max_issues_per_rule` was not applied to group rules**, which are the most
  likely to flood a report.
- **Derive: qualified segments were mismatched on the owned path.** The macro
  overrode `matches_segment` without setting `QUALIFIER_PATTERN`, so
  `edifact_deserialize_owned` matched on tag alone and then failed to parse.
- **Derive: `element = N` was unbounded**, letting a typo emit one statement per
  slot and exhaust memory in rustc. Indices above 256 are now rejected with a
  spanned error.
- **Derive: duplicate `(element, component)` slots and duplicate attribute keys**
  were accepted silently, dropping a field from serialization while
  deserialization still read it.
- **Derive: the `qualifier_from` guard used unqualified `Some`/`None`**, which
  broke generated code when a conflicting type was in scope.
- Slice and reader parsing paths disagreed by one byte on `max_segment_bytes`,
  so an identical segment parsed or failed depending on buffer alignment.
- `DirectoryValidator` resolved runtime-owned definitions by linear scan
  (O(segments × definitions)) and scanned all segments twice per required tag.

### Documentation

- The `docs/` guides are compiled as doctests, and a test asserts that every
  `edifact_rs::` item named in a guide is actually exported. Several guides
  referenced private module paths (`model::Element`, `tokenizer::…`,
  `envelope::validate`) and `EventEmitter` methods that never existed.
- Documented the `serde` feature, which was absent from both the README table
  and the crate-level docs.
- Error reference covers `E031`/`E032`, drops the retired `E029` section (which
  claimed functional groups were unsupported — they have been supported for
  several releases), and records retired codes.

### Internal

- CI gained `cargo fmt --check`, `clippy -D warnings`, `cargo deny`, a fuzz
  smoke job, and a bench-compile job. Removing `rust-toolchain.toml` means the
  jobs declaring `@stable` now genuinely test stable; previously the pin
  silently downgraded all of them to the MSRV toolchain.

---

## [0.10.0] — 2026-06-10

### Breaking Changes

- **`ServiceStringAdvice::from_bytes` now validates.** `from_bytes` behaves like
  the old `from_bytes_strict` — it returns `Result<Self, EdifactError>` and rejects
  inputs with duplicate or non-ASCII delimiters. The old lenient constructor is now
  `from_bytes_unchecked(input: &[u8]) -> Self`. Update call sites accordingly.

- **`OwnedSegmentDef::new` renamed to `new_unchecked`.** The panicking constructor
  is now clearly named `OwnedSegmentDef::new_unchecked(...)`. For external data use
  `OwnedSegmentDef::try_new(...)` which returns `Result`.

- **`OwnedElementRef::new` renamed to `new_unchecked`.** Same convention:
  `OwnedElementRef::new_unchecked(...)` panics on `position == 0`; use
  `OwnedElementRef::try_new(...)` for external data.

- **`ProfileRulePack::bail_on_first_error(bool)` renamed to
  `with_bail_on_first_error(bool)`.** Aligns with the builder-method naming
  convention (`with_*`) used throughout the crate.

- **`ValidationIssue::context` is now `Vec<(String, String)>`.** Previously
  `HashMap<String, String>`, the context map is now an ordered vector of key–value
  pairs. This eliminates non-deterministic serialisation order, reduces stack
  footprint for empty contexts, and avoids hashing overhead. Existing code that
  inserts duplicate keys still works (`with_context_entry` upserts). Code that
  reads context values must use `context_get(key)` instead of `map[key]`.

- **`SegmentGroup`, `group_segments`, `group_owned_segments` removed.** Use
  `SegmentGroupIndexed`, `group_segments_indexed`, `group_owned_segments_indexed`
  which are zero-copy (no per-segment clones) and carry `occurrence_index`.

- **`pub mod helpers` removed.** All helper functions (`find_segment`,
  `find_qualified_segment`, `qualifier_matches_pattern`, `composite_element`,
  `required_element`, `optional_element`, `find_segment_owned`,
  `find_qualified_segment_owned`, `find_segment_typed`, `find_segments_typed`,
  `find_segments_iter`, `get_components_iter`, `required_component`,
  `optional_component`, `contiguous_groups_by_qualifier`) are now exported
  directly from the crate root. Replace `edifact_rs::helpers::*` with
  `edifact_rs::*`.

- **`ElementRef` fields are now private.** Construct `ElementRef` values with
  `ElementRef::new(position, data_element, status, max_repeat)` which panics at
  compile time if `position == 0`. Access fields via the new `position()`,
  `data_element()`, `status()`, `max_repeat()` getters.

- **`ValidationSeverity`, `ValidationIssue`, `ValidationReport` moved to
  `edifact_rs::report`.** They are still re-exported from the crate root and
  from `edifact_rs::error` for compatibility. New code should import from
  `edifact_rs::report` or the crate root directly.

- **`Validator::fork()` returns `Option<Box<dyn Validator + Send + Sync>>`.**
  The default implementation returns `None` instead of panicking. Validators
  that cannot be forked no longer crash mixed-context setups.

### Added

- **`ValidationReport` and `ValidationIssue` implement `serde::Deserialize`**
  when the `serde` feature is enabled.  `error_code` is skipped during
  deserialization (always `None`) since it is a compile-time library constant.

- **`edifact_rs::report` module** — public module housing `ValidationSeverity`,
  `ValidationIssue`, and `ValidationReport` as first-class citizens.

- **`ElementRef::new(position, data_element, status, max_repeat)`** — `const fn`
  constructor with compile-time position-zero guard. Fails at compile time when
  called in a `const` context with `position == 0`.

- **`SegmentGroupIndexed::occurrence_index`** — zero-based occurrence counter
  stamped by `group_segments_indexed` for each repeated group trigger.

- **`ValidationContext::has_group_rules()` short-circuit** — skips the group
  walk pass when no group rules are registered, avoiding unnecessary allocations.

- **`Validator::has_group_rules()`** — default `false`; opt in to group rule
  dispatch by returning `true`.

- **`ProfileRulePack::with_max_issues_per_rule(n)`** — caps per-rule issue
  output, preventing runaway validators from flooding reports.

- **`Segment::component_str(elem, comp)`** and
  **`BorrowedSegment::component_str(elem, comp)`** — ergonomic helpers for
  direct composite component access.

- **`Span::len()`** now uses `saturating_sub` + `debug_assert!` instead of
  silent underflow on malformed input.

- **`Tokenizer::unlimited()`** has `#[must_use]` and a `# Security warning`
  section documenting the DoS risk of unlimited input.

- **`ValidationContextBuilder::with_static_issue(issue)`** — pre-inject advisory
  issues into every validation report produced by a context.

- **`ValidationReport::from_issues(errors, warnings, infos)`** — construct
  reports from pre-built issue vectors.

- **`ValidationReport::errors_vec_mut()`, `warnings_vec_mut()`,
  `infos_vec_mut()`** — `Vec` access for programmatic report building.

- **Resource-limit integration tests** — 8 new tests in `tests/resource_limits.rs`
  covering `max_segments`, `max_input_bytes`, `max_segment_bytes`, and combined
  limit behaviour.

- **`ProfileRulePack::group_scope` uses `Arc<str>`** — eliminates `'static`
  lifetime requirement for scoped group rules.

### Fixed

- **`escape_value` in `writer.rs`** no longer uses `unsafe { from_utf8_unchecked }`.
  Uses `from_utf8(...).expect(...)` with a comment explaining why the expect is
  unreachable.

- **`resolve_release_owned` capacity heuristic** — allocates 75 % of input
  length instead of 100 %, reducing wasted capacity for release-char-heavy values.

- **`edifact_rs::helpers` paths removed from derive macro** — generated code
  now references `::edifact_rs::find_segment` etc. directly.

- **`IncompatibleReleaseScopes` variant** — removed spurious
  `#[non_exhaustive]` attribute from a single variant (it only belongs on the
  enum itself).

- **`ValidationIssue::new` uses `HashMap::default()`** — avoids allocating an
  unnecessary random state for empty context maps.

### Added (0.10.0 continued)

- **`#![deny(unsafe_code)]`** enforced on both `edifact-rs` and
  `edifact-rs-derive` crate roots. Future `unsafe` introductions are compile
  errors, not silent regressions.

- **`ValidationReport::critical_count`** — O(1) cached count of
  `Critical`-severity errors. The `bail_on_first_critical` fast-path no longer
  re-scans the error list; it reads this counter instead.

- **`ValidationContext::fork_with_message_ref` emits excluded-validator advisory.**
  When validators are excluded from the forked context (because their `fork()`
  returned `None`), an `Info`-severity issue with rule-id
  `edifact-rs::fork::excluded-validator` is added to the report. Previously this
  was silent.

- **`validate_lenient_grouped_owned` early allocation guard.** The owned group-pass
  now skips the full `Vec<Segment<'_>>` borrow conversion when no group rules are
  registered — the common case for contexts without a `ProfileRulePack`.

- **`forbid_segment` and `forbid_segment_in_group` emit byte offsets.** Issues
  produced by these pack rules now carry `offset = segment.span.start`, matching
  the diagnostic precision of `DirectoryValidator`.

### Removed

- `anyhow` workspace dependency removed (was unused).

---

## [0.9.1] — 2026-06-09

### Breaking Changes

- **`from_reader_iter` removed.** The duplicate `from_reader_iter` free function
  has been removed. Use `from_reader` exclusively.

- **`f32`/`f64` `EdifactSerialize` impls removed.** Blanket impls for `f32`/`f64`
  have been deleted. Use `DecimalFloat(value)` or `DecimalFloatDisplay(value)` which
  correctly honour the interchange's decimal mark from `ServiceStringAdvice`.

- **`ProfileRulePack::merge` and `merge_unchecked` removed.** Use `extend_from`
  (prepend base rules) or `merge_with_override` (dedup by rule ID).

- **`Validator::validate_group_batch` added to trait.** External `Validator`
  implementors gain a new method with a default no-op body — no code changes needed
  unless you want to opt in to group-aware validation.

### Added

- **Group-scoped validation** (`F-011`): `ProfileRulePack` now supports
  group-scoped rules via new builder methods:
  - `with_group_rule_fn(closure)` — fires for every group in the DFS traversal
  - `with_named_group_rule_fn(id, closure)` — same, with a stable rule identifier
  - `with_scoped_group_rule_fn(group_scope, id, closure)` — fires only for groups
    whose `definition` field matches `group_scope` (e.g. `"SG5"`)
  - `require_segment_in_group(group_scope, tag, rule_id)` — built-in helper
  - `forbid_segment_in_group(group_scope, tag, rule_id)` — built-in helper
  - `require_qualifier_in_group(group_scope, tag, element, component, qualifier, id)`
  - `group_rule_count() -> usize`
  - Group-rule issues are automatically stamped with the group name in
    `ValidationIssue::segment_group`.
  - `Validator::validate_group_batch` trait method (default no-op).

- **`ValidationContext` grouped-validation methods** (`F-011`):
  - `validate_lenient_grouped(root: &SegmentGroupIndexed, segments: &[Segment<'_>])` —
    runs flat validation pass + group validation pass.
  - `validate_strict_grouped(...)` — strict variant.
  - `validate_lenient_grouped_owned(root, &[OwnedSegment])` — owned-segment variant.
  - `validate_strict_grouped_owned(...)` — strict owned variant.

- **`SegmentReader` sealed trait** (`F-007`): `envelope.rs` internal generics are
  now backed by a sealed `SegmentReader` trait shared between `Segment<'_>` and
  `OwnedSegment`. Added `validate_envelope_from_owned` and
  `validate_envelope_lenient_from_owned` that accept `&[OwnedSegment]` directly
  with no intermediate `Vec`.

- **`Arc<ProfileRulePack>` implements `Validator`** (`F-009`): Fork only increments
  the reference count; `ValidationContextBuilder::with_profile_pack_arc(Arc<ProfileRulePack>)`
  enables zero-copy pack sharing via `LazyLock`/`OnceLock`.

- **`ValidationContextBuilder::with_profile_pack_arc`** (`F-009`): Zero-allocation
  path for downstream code that caches packs in static storage.

- **`ValidationContextBuilder::bail_on_first_critical(bool)`** (`F-026`): Stop all
  validation as soon as the first `Critical`-severity issue appears.

- **`ValidationContext::fork_with_message_ref(message_ref)`** (`F-021`): Create a
  child context inheriting all rules, scoped to a specific UNH reference.

- **`ProfileRulePack::require_segment / forbid_segment / require_qualifier`** (`F-016`):
  First-class builder helpers for the most common EDIFACT validation idioms.

- **`ProfileRulePack::named_rule_count() / anonymous_rule_count()`** (`F-022`).

- **`ProfileRulePack` implements `Clone`** (`F-008`).

- **`ValidationIssue::segment_group: Option<Arc<str>>`** (`F-010`): Records which
  segment group an issue belongs to; `with_segment_group(name)` builder method.

- **`ValidationSeverity::as_str() / numeric_level()`** (`F-012`): Stable conversion
  helpers so `_ =>` arms in external match expressions can delegate cleanly.

- **`ValidationReport` / `ValidationIssue` / `ValidationSeverity` serialization**
  (`F-027`): `serde = ["dep:serde"]` feature gate; `Serialize` (and `Deserialize`
  for `ValidationSeverity`) derived behind the feature.

- **`group_owned_segments` / `group_owned_segments_indexed`** (`F-020`): Accept
  `&[OwnedSegment]` for reader-based workflows.

- **`ValidationRuleContext::message_type: Option<&str>`** (`F-017`): Pre-extracted
  once by `ValidationContext`, eliminating per-pack `UNH` scans.

- **`ValidationLayer`, `EnvelopeValidator`, `validate_each` re-exported** from crate root.

- **`rust-toolchain.toml`** pinning `channel = "1.85"` (`F-013`).

- **`ValidationIssue` and `IncompatibleReleaseScopes` variant marked
  `#[non_exhaustive]`** (`F-018`, `F-019`).

- **`ValidationContext` architecture docs** (`F-034`): Rustdoc `# Architecture`
  section explaining the four validation layers, their ordering, and which types
  implement each.

- **`group_segments_indexed` worked example in rustdoc** (`F-028`): 3-level
  MSCONS-like schema demonstrating `total_span`, `direct_segment_indices`,
  and integration with `validate_lenient_grouped`.

- **Fuzz harness extended** (`F-030`): Two new Bolero targets cover
  `ProfileRulePack::validate_batch`, `validate_group_batch`, and
  `group_segments_indexed` with adversarial byte inputs.

- **Trybuild UI test coverage** (`F-025`): Added
  `pass_qualifier_from_with_dynamic_dispatch.rs` (positive case) and
  `fail_qualifier_from_on_field.rs` (reject field-level `qualifier_from`).

- **Integration test suite `tests/group_validation.rs`** (`F-029`): 11 tests
  covering group-rule scoping, cross-group non-contamination, per-occurrence firing,
  auto-stamping, owned-segment path, and message-type scoping.

### Fixed

- **`Span::len()` wrap in release mode** (`F-001`): `checked_sub` replaces raw
  subtraction.

- **`max_input_bytes` UNA under-count** (`F-002`): `FromBytesIter` now tracks
  `bytes_consumed = seg.span.end` (absolute input offset).

- **`validate_batch` scans `UNH` once per pack** (`F-017`): `ValidationContext`
  pre-extracts the message type and injects it via `ValidationRuleContext`.

- **Absolute byte tracking for `FromBytesIter`** (`F-002`): `bytes_consumed` now
  includes the 9-byte UNA header and segment terminators.

### Validator module split (`F-033`)

`validator.rs` (~1 400 lines) split into three focused sub-modules:

- `validator/mod.rs` — `Validator` trait, `ValidationRuleContext`, `ValidationLayer`,
  `EnvelopeValidator`.
- `validator/pack.rs` — `ProfileRule`, `ProfileRulePack`, merge helpers.
- `validator/context.rs` — `ValidationContext`, `ValidationContextBuilder`.

---

## [0.8.0] — 2026-06-03

### Breaking Changes

- **`__private` module removed.** The `edifact_rs::__private` module has been deleted.
  All helpers previously accessible via `__private` are now in `edifact_rs::helpers`.
  The most commonly used functions (`find_segment`, `find_qualified_segment`,
  `required_element`, `optional_element`, `element_str`) are additionally re-exported
  at the crate root for convenience.

- **Derive macro generates `helpers` paths.** Code generated by
  `#[derive(EdifactDeserialize)]` now emits `::edifact_rs::helpers::*` call paths
  instead of `::edifact_rs::__private::*`.  Recompiling dependent crates is sufficient;
  no source changes are required unless you called `__private` items directly.

- **`Element` parallel arrays unified into a single tuple array.**  `Element::components:
  SmallVec<[Cow<str>; 4]>` and `Element::component_spans: SmallVec<[Span; 4]>` have been
  merged into a single `components: SmallVec<[(Cow<str>, Span); 4]>`.  Access patterns
  change from `elem.components[i]` / `elem.component_spans[i]` to
  `elem.components[i].0` / `elem.components[i].1`, or use the existing
  `get_component(i)` / `get_component_span(i)` accessors which are unchanged.

- **`from_reader` now returns a lazy iterator.**  `from_reader<R: Read>(reader: R)` now
  returns `FromReaderIter<R>` (an iterator over `Result<OwnedSegment, EdifactError>`)
  instead of eagerly collecting into `Result<Vec<OwnedSegment>, EdifactError>`.  Code
  that called `from_reader(r)?` or `from_reader(r).unwrap()` must be updated to
  `from_reader_collect(r)?`.

- **`ProfileRule::evaluate` signature changed.**  The method now accepts a mutable
  `issues: &mut Vec<ValidationIssue>` parameter instead of returning
  `Option<ValidationIssue>`.  Rules that previously returned `Some(issue)` should
  now call `issues.push(issue)`.  Rules that returned `None` become no-ops.  This
  enables rules to report multiple issues per invocation.

  ```rust
  // Before:
  fn evaluate(&self, segments: &[Segment<'_>], ctx: &ValidationRuleContext<'_>)
      -> Option<ValidationIssue> { … }

  // After:
  fn evaluate(&self, segments: &[Segment<'_>], ctx: &ValidationRuleContext<'_>,
              issues: &mut Vec<ValidationIssue>) { … }
  ```

- **`with_rule_fn` and `with_stateless_rule_fn` closure signatures changed.**  Closures
  must now accept an extra `&mut Vec<ValidationIssue>` parameter and push issues into it
  instead of returning `Option<ValidationIssue>`.

  ```rust
  // Before:
  .with_stateless_rule_fn(|segments| { … Some(issue) })

  // After:
  .with_stateless_rule_fn(|segments, issues| { issues.push(issue); })
  ```

- **`ValidationFailed` (E018) replaced by `ValidationErrors` (E030).**  The lossy
  `ValidationFailed { error_count, first_message }` variant has been removed.  All
  code paths now emit the richer `ValidationErrors { error_count, report }` variant,
  which preserves the full `ValidationReport`.  Update any `match` arms that pattern-match
  on `EdifactError::ValidationFailed`.

- **`IoError::0` field changed to `pub(crate)`.**  External callers must use the new
  `IoError::inner()` accessor instead of `.0` to obtain a `&std::io::Error`.

- **`message_windows_bytes` removed from crate root.**  The function is still available
  as the alias `from_bytes_windows` (added in 0.7.0).  Update imports from
  `edifact_rs::message_windows_bytes` to `edifact_rs::from_bytes_windows`.

### Added

- **`edifact_rs::helpers`** now exports the full set of segment-navigation and
  element-access helpers: `composite_element`, `contiguous_groups_by_qualifier`,
  `get_components_iter`, `optional_component`, `qualifier_matches_pattern`,
  `required_component` (previously only accessible via `__private`).

- **`find_segment`, `find_qualified_segment`, `required_element`, `optional_element`,
  `element_str`** promoted to the crate root (`use edifact_rs::find_segment;` now works).

- **`SegmentGroupIndexed` and `group_segments_indexed`.**  A zero-clone counterpart to
  `group_segments` that stores `Range<usize>` indices into the original flat segment slice
  instead of cloning each `Segment`.  Use the original slice and
  `SegmentGroupIndexed::segment_range` to access segments without any heap allocation.

- **`from_reader_collect`.**  Eagerly collects all segments from a reader into
  `Result<Vec<OwnedSegment>, EdifactError>` — the previous eager behaviour of
  `from_reader`.

- **`from_bytes_owned_with_config`.**  Like `from_bytes_owned` but accepts a
  `ReaderConfig` to enforce per-segment and per-interchange byte/segment limits.

- **`segments_to_bytes_owned`.**  Serialises a `&[OwnedSegment]` slice to bytes,
  eliminating the need to convert to borrowed `Segment<'_>` before writing.

- **`validate_envelope_owned` and `validate_envelope_lenient_owned`.**  Owned-segment
  counterparts to `validate_envelope` and `validate_envelope_lenient` that accept
  `&[OwnedSegment]` directly.

- **`validate_envelope_lenient`** re-exported at the crate root.

- **`IoError::inner()`.**  Returns `&std::io::Error` from a wrapped `IoError`.

- **`ValidationRuleContext::message_ref`** field and **`with_message_ref()`** builder
  method.  Allows rules to access the UNH message reference (DE 0062) injected by
  `ValidationContextBuilder::with_message_ref`.

- **`ProfileRulePack::merge_unchecked`.**  Merges two packs without checking release
  scope compatibility (useful when the caller guarantees both packs share the same
  scope).

### Fixed

- **`ProfileRulePack::merge` / `extend_from` / `merge_with_override` no longer panic**
  when the two packs have incompatible release scopes.  They now return
  `Err(EdifactError::IncompatibleReleaseScopes { … })` instead of calling
  `assert_eq!`.

- **`Span::offset` uses `saturating_add`.**  Previously used plain `+`, which would wrap
  silently in release builds or panic in debug builds on overflow.

- **`validate_lenient_owned` avoids redundant full-slice conversion.**  The method now
  converts owned segments lazily per validator layer, avoiding an upfront O(n) allocation
  when only a subset of layers is active.

- **`qualifier_matches_pattern` wildcard guard lowered from 8 to 4.**  EDIFACT qualifier
  patterns never use more than 1–2 wildcards; the tighter ceiling reduces the adversarial
  input budget.

- **`AllSegmentsIter` DFS stack uses `SmallVec`.**  The stack no longer heap-allocates for
  typical message group depths (≤ 8 levels).

---

## [0.7.0] — 2026-06-03

### Breaking Changes

These changes break backward compatibility with 0.6.x.  No backward-compatible
migration path is provided; users must update call sites.

#### API Surface

- **`validate_strict` / `validate_strict_with`** now return
  `Result<ValidationReport, ValidationReport>` instead of `Result<ValidationReport, EdifactError>`.
  On failure the `Err` variant holds the report of collected issues rather than a single error.

- **`ProfileRulePack::message_types()`** now returns
  `impl Iterator<Item = &str>` instead of `&[String]`.
  The internal storage changed from `Vec<String>` to `BTreeSet<String>` to provide
  O(log n) membership lookups and deterministic sorted iteration.

- **Private helpers moved to `__private` module.**
  The following functions were previously re-exported at the crate root under
  `#[doc(hidden)] pub use de::...`.  They are now under `edifact_rs::__private::`:
  `composite_element`, `contiguous_groups_by_qualifier`, `element_str`,
  `find_qualified_segment`, `find_qualified_segment_owned`, `find_segment`,
  `find_segment_owned`, `find_segment_typed`, `find_segments_iter`,
  `find_segments_typed`, `get_components_iter`, `optional_component`,
  `optional_element`, `qualifier_matches_pattern`, `required_component`,
  `required_element`.

- **`Span::len()` is no longer `const fn`.**  A `debug_assert!` guard was added
  to detect inverted (start > end) spans in debug builds.  The function signature
  changes from `pub const fn len(self) -> usize` to `pub fn len(self) -> usize`.

- **`SegmentGroup` doc corrected.**  The previously misleading suggestion to "use
  group indices" has been removed — no index-based API exists yet.

#### Derive Macro

- **`#[edifact(group)]` documentation corrected.**  The attribute now correctly
  describes its compile-time enforcement semantics (requires `Vec<T>`, mutually
  exclusive with element/component positioning).  Previously the doc incorrectly
  claimed the attribute was documentation-only.

- **Owned deserialization path now correctly handles `Option<T>` for non-String types.**
  Fields declared as `Option<u32>`, `Option<bool>`, etc. in the owned
  deserialization path previously always produced `Option<String>`.  They now use
  `parse::<T>()` via `FromStr`, matching the borrowed deserialization path.

### Added

#### Core Library

- **`DecimalFloat<T>` and `DecimalFloatDisplay<T>` newtypes** (`crates/edifact-rs/src/ser.rs`).
  Wrap a float or any `Display` value to serialize with the interchange's configured
  decimal mark (from `EventEmitter::decimal_mark()`).  Bare `f32`/`f64` serialization
  still defaults to `.` and is unchanged.

- **`EventEmitter::decimal_mark()` default method.**  Returns the decimal mark byte
  used by the emitter.  Default is `b'.'`.  Overridden in `WriterEmitter` to return
  the value from the active `ServiceStringAdvice`.

- **`EdifactError::InvalidFieldValue { tag, element_index, value }`** (stable code `E027`).
  Emitted when a required qualifier element is present but empty or has an invalid
  value.

- **`ValidationReport::merge(other)`** — drains errors, warnings, and infos from
  another report into `self`.

- **`ValidationIssue::segment_occurrence: Option<u16>`** — zero-based occurrence
  index among segments with the same tag.  Builder method `with_segment_occurrence(n)`.

- **`from_bytes_owned(input: &[u8]) -> impl Iterator<Item = Result<OwnedSegment, EdifactError>>`**
  free function at crate root.

- **`from_bytes_windows(input: &[u8])`** alias for `message_windows_bytes`.

- **`Writer::finish_unt(message_ref: &str) -> Result<W, EdifactError>`** — writes the
  closing `UNT` segment with the correct segment count and consumes the writer.

- **`validate_lenient_owned` and `validate_strict_owned`** on `ValidationContext`.

- **`ProfileRulePack::rule_ids()`** — iterator over stable identifiers of named rules.

- **`ValidationReport::merge(other)`** — merges issues from another report.

- **`WriterEmitter::with_una` and `service_string_advice()`.**

- **`ReaderConfig::max_segments(n)` and `max_input_bytes(n)`** builder methods.
  When set, `OwnedSegmentStream` stops yielding segments once the limit is reached.

- **`from_bytes_windows`** crate-root alias for `message_windows_bytes`.

- **`EdifactError::UnexpectedDataToken { offset }`** (stable code `E028`).
  Emitted by the parser when a data or component element token is encountered
  before the first segment tag.  Previously these tokens were silently skipped.

- **`EdifactError::FunctionalGroupNotSupported { offset }`** (stable code `E029`).
  Emitted by `validate_envelope` when a `UNG` or `UNE` segment is found.
  Previously these segments caused incorrect message/segment-count errors.

- **`EnvelopeValidator`** built-in validator struct.  Translates errors from
  `validate_envelope` into `ValidationIssue` objects in the unified
  `ValidationReport`.  Registered with
  `ValidationContextBuilder::with_envelope_validation()`.

- **`ValidationContextBuilder::with_envelope_validation()`** — enables the
  `EnvelopeValidator` layer (`ValidationLayer::Envelope`) in the validation
  pipeline.

- **`ValidationContextBuilder::with_message_ref(ref: impl Into<String>)`** —
  stamps every issue produced by the context with the given `UNH` reference
  (DE 0062).  Stored in `ValidationIssue::message_ref`.

- **`ValidationIssue::message_ref: Option<String>`** — reference string
  from `UNH` element 0.  Populated automatically when
  `ValidationContextBuilder::with_message_ref` is set.
  Builder method `with_message_ref(ref: impl Into<String>)`.

- **`ValidationContext::message_ref()`** accessor — returns the message
  reference configured on this context, if any.

#### Derive Macro

- **`qualifier_from` guard emits `InvalidFieldValue` instead of `MissingRequiredElement`**
  when the qualifier element is present but empty, providing a more accurate error code.

### Fixed

- **`ServiceStringAdvice::is_valid()`** now checks all 10 pairwise delimiter
  distinctness constraints, including `decimal_mark`.

- **`Writer::with_una`** delegates `is_valid()` to `ServiceStringAdvice`.

- **`write_escaped`** uses `memchr3` + `memchr` to find the nearest special byte
  with SIMD acceleration, avoiding O(n²) worst case for long values.

- **`try_fast_segment` and `OwnedSegmentStream` slow path** now use `.next()` on
  `Parser` directly instead of `collect::<Result<Vec<_>, _>>()`, avoiding an
  unnecessary allocation.

- **`DirectoryValidator::from_definitions`** builds a `HashMap` at construction time
  for O(1) tag lookups instead of O(n) linear scan on every segment.

- **`group_recursive_inner`** no longer rebuilds the `SmallVec` stop-trigger set on
  every segment iteration.  It is now computed once per schema level.

- **`mandatory_positions`** refactored to remove the ad-hoc `E<A,B>` iterator enum.

- **`// SAFETY:` comments** replaced with `// INVARIANT:` in `writer.rs` and
  `envelope.rs` where no actual unsafe code exists.

- **`IoError::PartialEq`** implementation is now documented with an explanation of
  the kind-only comparison semantics.

- **`SegmentGroup` doc** no longer references a non-existent indexed API.

- **`#[edifact(group)]` crate doc** now accurately describes compile-time constraints.

- **`Option<T>` owned deserialization** now correctly calls `parse::<T>()` for non-string
  inner types.

- **`edifact_deserialize_owned` default doc** clarified as `# Warning: allocation overhead`
  with explicit mention of the `Vec<Segment<'_>>` allocation cost.

- **Stray data/component element tokens** in the parser now return an
  `UnexpectedDataToken` error instead of silently advancing state.

- **`validate_envelope`** now detects `UNG`/`UNE` functional-group segments
  immediately and returns `FunctionalGroupNotSupported` instead of later producing
  misleading `MessageCountMismatch`/`InvalidSegmentForMessage` errors.

### Security

- No new security-relevant changes.

---

## [0.6.0] — 2024-xx-xx

Initial public release.  See commit history for full details.

---

[Unreleased]: https://github.com/hupe1980/edifact-rs/compare/v0.15.0...HEAD
[0.15.0]: https://github.com/hupe1980/edifact-rs/compare/v0.14.0...v0.15.0
[0.14.0]: https://github.com/hupe1980/edifact-rs/compare/v0.10.0...v0.14.0
[0.10.0]: https://github.com/hupe1980/edifact-rs/compare/v0.9.1...v0.10.0
[0.9.1]: https://github.com/hupe1980/edifact-rs/compare/v0.9.0...v0.9.1
[0.9.0]: https://github.com/hupe1980/edifact-rs/compare/v0.8.0...v0.9.0
[0.8.0]: https://github.com/hupe1980/edifact-rs/compare/v0.7.0...v0.8.0
[0.7.0]: https://github.com/hupe1980/edifact-rs/compare/v0.6.0...v0.7.0
[0.6.0]: https://github.com/hupe1980/edifact-rs/releases/tag/v0.6.0
