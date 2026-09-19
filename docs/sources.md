# External IIDX source policy

This document is the source of truth for automated catalog inputs, lineage,
field authority, and reuse boundaries. Adapters must pin an immutable revision
or content digest rather than assuming that a live page still has the shape
described here.

This is a conservative engineering policy, not a legal conclusion. The
non-distributed `scorepeek-catalog-publisher` crate fetches source data in the
daily GitHub Actions job. Raw snapshots remain ephemeral/private build inputs;
the Pages artifact contains accepted normalized catalog assertions and carries
[`THIRD_PARTY_NOTICES.md`](../THIRD_PARTY_NOTICES.md) separately from the
scorepeek software license.

The project's source-permission requirements for providing this generated
catalog to scorepeek users are satisfied for the three automated inputs below.
Tachi and dqn/iidxapi are used under their published Unlicense and ISC terms
without separate contact. Textage's usage clarification covers the inquiry
about processing metadata and providing an application-specific database;
the administrator's response and source notices are linked in
[`THIRD_PARTY_NOTICES.md`](../THIRD_PARTY_NOTICES.md#textage).
Distribution retains the source credits and applicable license notices.
The catalog is not relicensed under the application's planned MIT license, and
this policy does not grant a blanket right to redistribute the combined catalog.

## Automated inputs

| Source | Lineage | Automated role | Fields used | Access and reuse boundary |
| --- | --- | --- | --- | --- |
| [Tachi IIDX seeds](https://github.com/zkldi/Tachi/tree/main/db/seeds) | game MDB | General-IIDX identity and chart anchor | source-scoped song/chart IDs, exact titles, artist, version, play type, difficulty, level, notes, product availability | The README describes seeds as Unlicense/source-of-truth data, while the current path is `db/seeds`; preserve exact revision/provenance and the Tachi notice. The [MDB cookbook](https://github.com/zkldi/Tachi/blob/main/docs/src/contributing/cookbook/iidx-mdb.md) is recorded as lineage evidence. |
| [Textage](https://textage.cc/score/index.html) | Textage capture/manual data | Independent corroboration and display variants | title, artist, BPM, version, SP/DP level and notes, INFINITAS flag | Use the [site readme](https://textage.cc/score/readme.html) and [administrator clarification](https://textage.cc/bbs/index.php?res=765&disp=1) as the basis for providing accepted normalized metadata in the catalog. Avoid excessive load; scorepeek's stated acquisition frequency is once per day. Credit and link Textage. The [title](https://textage.cc/score/titletbl.js), [availability](https://textage.cc/score/actbl.js), and [chart](https://textage.cc/score/datatbl.js) files remain build inputs, not distributed source tables. |
| [dqn/iidxapi](https://github.com/dqn/iidxapi) | official INFINITAS HTML | Positive INFINITAS roster/pack signal | exact title, artist, pack name | Use the upstream [ISC declaration](https://github.com/dqn/iidxapi#license) with project credit and applicable license notices. Preserve the content hash and distribute accepted normalized roster/pack evidence in the catalog; the raw [JSON snapshot](https://dqn.github.io/iidxapi/infinitas/music.json) remains a build input. The adapter output has no stable song identity or chart data. |

The dqn/iidxapi contract inspected at repository commit
`6f76e8e0286f8a91a688a549e023ce5261b4b7c2` is a top-level JSON array whose
rows contain exactly `title`, `artist`, and nullable `packName` fields. The live
adapter accepts that bounded shape only, keeps a null pack distinct from every
named pack, and rejects duplicate rows and unknown fields. Synthetic tests use
the same wire shape; current endpoint bytes remain private and uncommitted.

Tachi's opaque IDs are stable source bindings, not semantic universal IDs.
Textage numeric and slug IDs remain Textage-local. dqn rows never create an
identity by themselves. v1 derives the public UUIDv5 song ID from the exact
Tachi binding; records without that anchor remain provisional until Tachi
catches up.

The live Tachi contract inspected at repository commit
`4ef9ca588424e1a98dc73421a49dd8efe3b37ddd` consists of
`db/seeds/songs-iidx.json`, `db/seeds/charts-iidx-sp.json`, and
`db/seeds/charts-iidx-dp.json`. Synchronization first resolves `main` through the
GitHub Git-ref API and then requests all three raw files at that exact commit.
The strict parser accepts Tachi's prefixed 20-character song/chart IDs, typed
song metadata, and the documented chart row shape. It imports only primary
NORMAL, HYPER, ANOTHER, and LEGGENDARIA SP/DP charts; known Tachi custom-mode
charts are schema-validated but excluded from the scorepeek catalog. The main
title is `in_game_display`, `altTitles` are `alternate_display`, and
`eamusementCsvTitle` is `eamusement_csv`; `searchTerms` remain excluded from
identity and OCR lexicons. A primary imported chart whose `versions` contains
`inf` is positive Tachi INFINITAS evidence. The three exact files are cached as
one framed content-digested bundle; repository scripts and downloaded code are
never executed. Each publication candidate contains only the evidence from the
three snapshots used for that zero build.

The live Textage contract inspected at framed bundle SHA-256
`3c1291f96946279512632ec69e5bf0f8d49ff0b7e301e43457bfe36bd5ad4f81`
consists of `titletbl.js`, `actbl.js`, and `datatbl.js`. The three exact byte
streams are decoded as the web `Shift_JIS` encoding, whose mapping is
Windows-31J/CP932; any replacement would reject the bundle. The parser reads
only the declared prefix constants and named object assignment. It accepts
bounded comments, quoted strings and escapes, decimal integers, declared
integer constants, object rows, fixed arrays, and the static
`string.fontcolor(string)` display wrapper. It never invokes a JavaScript
runtime and does not parse or execute code after the table assignment.

`actbl` is the admitted song-row set. Every admitted slug must have matching
title and chart-data rows; extra non-admitted metadata rows do not become
observations. The exact Textage binding combines the admitted slug and its
positive numeric title-table ID; both remain source-local. Source-specific
title extraction removes only leading and trailing ASCII HTML whitespace and
the static `fontcolor` wrapper while preserving case, width, punctuation,
internal whitespace, and HTML fragments in the selected title value. The
adapter imports only standard SP BEGINNER/NORMAL/
HYPER/ANOTHER/LEGGENDARIA and DP NORMAL/HYPER/ANOTHER/LEGGENDARIA slots for
which both a positive level and note count exist. Partial slots remain unknown.
The `datatbl` value `"0"` is preserved as Textage's unknown-BPM sentinel;
otherwise BPM must be a positive nondecreasing value or range.
The availability bitfield contributes a corroborating INFINITAS flag but cannot
establish catalog availability by itself. The three inputs are cached together
under their framed content digest with at most 64 private bundles or 64 MiB.

A dqn row has no stable key. Its raw NFC `(title, artist)` tuple can contribute
positive availability only when it resolves to exactly one active
Tachi-anchored record. This is secondary evidence, not an identity merge. Zero
or multiple matches are quarantined. No previous dqn binding set is used as a
generation base; disappearance and correction are reflected by the next valid
zero-built candidate without inferring a rename.

## Generation and distribution

The daily and manual publisher workflow resolves all three current snapshots,
strictly parses them, federates from `Catalog::default()`, writes SQLite, and
runs the production loader plus complete catalog invariants in a fresh
directory. A failure in acquisition, parsing, source policy, or whole-catalog
validation prevents deployment. Individual
`provisional_without_tachi_anchor`, `ambiguous_identity`, and
`conflicting_chart` records may be excluded; every other quarantine reason
blocks publication. Counts and source lineage are written to the Actions run
summary.

The current schema is published at `/catalog/v1/catalog.zip` on the same GitHub
Pages site that owns the future landing page. The ZIP contains exactly
`catalog.sqlite3`, `manifest.json`, and `THIRD_PARTY_NOTICES.md`. The manifest
binds the SQLite SHA-256, runtime semantic digest, schema artifact revision,
generator commit, and official workflow run URL. Detailed source revisions,
hashes, provenance, and evidence remain in SQLite. The workflow URL is a
best-effort human reference, not permanent proof.

Every candidate is generated and completely validated even when it is a
publication no-op. The existing ZIP bytes are retained when runtime semantics,
`artifact_revision`, and notices are all unchanged. Source-only lineage,
quarantine detail, timestamps, storage layout, and other non-runtime metadata
do not affect the semantic digest. A schema's artifact revision is incremented
when logically invisible SQLite storage changes must be distributed.

## Reference-only sources

| Source | Permitted scorepeek use | Reason it is not an automated input |
| --- | --- | --- |
| [RemyWiki](https://remywiki.com/Category%3ABeatmania_IIDX_Songs) | Manual investigation and discrepancy confirmation | MediaWiki API is available, but no standard content reuse license is declared. Its [robots policy](https://remywiki.com/robots.txt) says `ai-train=no` and `use=reference`; neural OCR fine-tuning is training even though it is not an LLM. Automated ingestion or training requires explicit administrator permission. |
| [BEMANIWiki 2nd](https://bemaniwiki.com/) | Update alert and manual confirmation | Current community data but no clear content license or stable machine schema. |
| [KONAMI INFINITAS music list](https://p.eagate.573.jp/game/infinitas/2/music/index.html) | Human confirmation of official spelling and releases | Human-facing HTML with no stable item ID or API; the [site policy](https://www.konami.com/siteinfo/ja/) restricts unauthorized reproduction. |
| [BEMANICN](https://wiki.bemani.cc/) and other community wikis | Manual investigation | License, access stability, schema, and independent lineage are not established. |

[`iidx_all_songs_master`](https://github.com/tts1374/iidx_all_songs_master)
is deliberately excluded even though its generated SQLite interface is
convenient: its build inputs include the prohibited upstream visual/music
resources, so adopting it would reintroduce the dependency this project removed.
Textage mirrors and applications derived from Textage also remain one lineage
and cannot corroborate Textage independently.

## Adapter safety contract

- Resolve Git inputs to a commit SHA. For mutable HTTP inputs, retain the exact
  bytes privately and identify them by SHA-256 plus available source timestamps.
- Send an honest scorepeek user agent, obey rate limits and retry headers, and
  apply conservative serial polling. Daily synchronization is sufficient.
- Never execute downloaded JavaScript. Textage adapters decode Windows-31J
  without replacement and accept only the documented assignment/literal
  grammar with explicit size, nesting, field, and record-count limits.
- Parse into source observations before federation. Preserve decoded display
  values after only the documented source-specific static display extraction,
  plus source IDs, source revision, parser version, and field provenance.
- Treat mirrors, forks, and downstream databases as the same `lineage_id` as
  their input. Agreement within one lineage is one observation, not a quorum.
- Each valid build represents the current snapshots only. Source corrections
  and removals therefore flow into the next accepted catalog; the previously
  published ZIP remains active when the candidate build fails.
- Schema drift, duplicate source IDs, truncated data, invalid domains, or a
  non-immutable revision invalidates the complete candidate and leaves the
  published Pages artifact unchanged.

## Federation and OCR boundary

Catalog strings are inference-time lexical constraints. They do not become
training examples, synthetic text prompts, model weights, or repository
fixtures. `searchTerms` and site navigation aliases are excluded from the OCR
lexicon. Exact display variants enter the lexicon only after their source
binding is resolved without fuzzy identity matching.

Each accepted catalog assertion records its contributing source revision and
lineage. A UI may show provenance and quarantine diagnostics, but stable
recognition events expose only the internal song ID, accepted exact display
title, catalog digest, and INFINITAS status.
