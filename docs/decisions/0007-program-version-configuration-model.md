# ADR 0007: Program-version configuration model

- **Status:** Accepted
- **Date:** 2026-08-28

## Context

Milestone 2 introduces the most consequential schema in the product.
Finalized records pin the exact configuration versions used to create
them for as long as they are retained (PRINCIPLES.md 5), so every shape
this schema can express is a permanent reproduction obligation. ADR 0001
named configuration-version tooling as a cost to design early and
rejected the generic form builder; issue #12 recorded the open design
questions and the owner's direction. This ADR encodes that direction.

The guiding frame: agencies configure the vocabulary; the product owns
the grammar. Configurability lives in content — terminology, scales,
competency lists, labels, citations, phase graphs — never in structure.
A recognizably consistent record structure is part of what makes a
record defensible to an assessor, arbitrator, or counsel years later.

## Decision

### Program and ProgramVersion

A `program` is the stable continuing identity. A `program_version` is
the publishable unit: mutable while draft, frozen by publishing.
Enrollments pin a `program_version`, never a `program`.

Each version carries an internal monotonic version number per program
for identity and ordering, plus a free-text agency-visible label such
as "2026 CTO Program rev B". The label is presentation; the number is
authority.

Once `published_at` is set, UPDATE and DELETE on the version and its
owned configuration are rejected by database triggers — the same
enforcement class as `audit_event`, not application discipline.

### Version contents are owned typed rows

Phases, allowed phase transitions, competencies, tasks, evaluation
forms, and rating scales are typed tables keyed to the owning
`program_version`. Domain-model database invariant 5 — all referenced
configuration belongs to the pinned version — becomes enforceable
foreign keys instead of convention.

Phases are optional. A program version with no phases is valid: annual
and in-service training is a real program shape with topics and hours
per cycle rather than a progression. Nothing in the schema may assume
phased structure.

Phase transitions are an explicit directed graph of allowed moves,
which expresses advancement, remediation loops, skips, and restarts.
There are no conditional or rule-based transitions; adding them later
requires demonstrated need from a real center and its own ADR.

### Rating scales

The product defines a closed set of scale kinds: anchored numeric,
pass/fail, and narrative-only. Everything inside a kind is
agency-defined content — range, anchor definitions, labels — and scales
are assignable per competency, mixed freely within one version. There
are no freeform scale semantics the product cannot reason about.

### Evaluation forms

The product owns the form skeleton for each record type; agencies
configure the competency lists, labels, scales, and citations that
populate it. There are no agency-configurable form sections. The
anti-form-builder decision of ADR 0001 holds.

### External standards citations

Competencies, tasks, and program versions may carry agency-entered
citations to external standards — body, edition, clause, and an
optional note — covering accreditation and certification mappings such
as CALEA communications standards, APCO ANS training standards, or
state continuing-education requirements. Citations are versioned
configuration: they freeze at publish, so a standards body revising an
edition never rewrites what an existing record claimed. The product
never embeds standard text and never grows standards-body-specific
code paths (PRINCIPLES.md 1). Coverage reporting belongs to Milestone 5
exports; the citation fields exist from the first configuration
migration so that report is a query, not a retrofit.

### Export and import

Whole program versions export to a documented, versioned JSON format
and import losslessly, round-trip tested (PRINCIPLES.md 9). Its
normalization follows the canonical record-bytes design.

### Authoring

Program authoring is single-editor with honest last-write behavior in
this milestone. Real concurrent editing is Milestone 3's concern, where
session drafts already plan contributor and ownership-transfer history.

## Consequences

### Positive

- invariant 5 is database-enforced, and published configuration is
  immutable at the same strength as the audit log;
- the set of shapes exports must reproduce forever is bounded and
  known;
- annual in-service programs arrive later as configuration, not a
  schema break;
- accreditation coverage reporting becomes a query over existing
  fields; and
- export/import proves the configuration boundary before records
  depend on it.

### Costs

- typed tables mean more migrations and mapping code than a JSON blob
  per version;
- a new scale kind is a product change with export and rendering
  obligations, not an agency setting;
- centers whose transition rules are genuinely conditional must express
  them as graph edges plus policy until an ADR says otherwise; and
- publishing must make freeze semantics unmistakable in the interface,
  since there is no unpublish.

## Rejected alternatives

- **JSON blob per program version:** hides invariant 5 from the
  database and makes import validation, comparison, and migration
  unreliable.
- **Agency-configurable form sections:** a form builder by increments;
  erodes cross-record legibility and creates an unbounded forever-
  render surface. Rejected in ADR 0001 and again here.
- **Conditional or rule-based phase transitions:** configuration
  quietly becomes code; the explicit graph covers every workflow named
  by the domain model.
- **Embedding external standard text:** the standards are copyrighted,
  access-controlled reference material; the agency's mapping is
  configuration, the standard itself is not product content.
