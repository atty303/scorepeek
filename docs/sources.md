# External IIDX source policy

This document is the source of truth for automated catalog inputs, lineage,
field authority, and reuse boundaries. It records the research state on
2026-08-15; adapters must pin an immutable revision or content digest rather
than assuming that a live page still has the shape described here.

This is a conservative engineering policy, not a legal conclusion. scorepeek
fetches third-party data on each user's machine and does not republish raw or
normalized source snapshots.

## Automated inputs

| Source | Lineage | Automated role | Fields used | Access and reuse boundary |
| --- | --- | --- | --- | --- |
| [Tachi IIDX seeds](https://github.com/zkldi/Tachi/tree/main/db/seeds) | game MDB | General-IIDX identity and chart anchor | source-scoped song/chart IDs, exact titles, artist, version, play type, difficulty, level, notes, product availability | The README describes seeds as Unlicense/source-of-truth data, while the current path is `db/seeds`; keep local snapshots and provenance, and confirm scope before any redistribution. The [MDB cookbook](https://github.com/zkldi/Tachi/blob/main/docs/src/contributing/cookbook/iidx-mdb.md) is recorded as lineage evidence. |
| [Textage](https://textage.cc/score/index.html) | Textage capture/manual data | Independent corroboration and display variants | title, artist, genre, BPM, version, SP/DP level and notes, INFINITAS flag | The [site readme](https://textage.cc/score/readme.html) permits common-sense use and recommends a link but is not a standard data license. Fetch [title](https://textage.cc/score/titletbl.js), [availability](https://textage.cc/score/actbl.js), and [chart](https://textage.cc/score/datatbl.js) bytes locally; do not republish them. |
| [dqn/iidxapi](https://github.com/dqn/iidxapi) | official INFINITAS HTML | Positive INFINITAS roster/pack signal | exact title, artist, pack name | The adapter output has no stable song identity or chart data. Treat it as corroboration of the official page, preserve its content hash, and do not redistribute the derived roster. [Current JSON endpoint](https://dqn.github.io/iidxapi/infinitas/music.json) |

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
never executed. A later Git commit remains recorded as the latest source
snapshot, but an unchanged title, chart, or binding assertion reuses its
existing evidence instead of adding a full revision-wide duplicate. Changed
assertions retain their new evidence independently.

A dqn row has no stable key. Its raw NFC `(title, artist)` tuple can contribute
positive availability only when it resolves to exactly one active
Tachi-anchored record. This is secondary evidence, not an identity merge. Zero
or multiple matches are quarantined. On every later snapshot, every previously
accepted tuple must still be present and resolve to the same Tachi ID; if any
does not, all new dqn bindings from that snapshot stay quarantined and the
previous accepted set remains unchanged. scorepeek does not infer whether a
disappearance and addition represent a rename, removal, or unrelated new song.

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
- Never execute downloaded JavaScript. Textage adapters decode the declared
  encoding and accept only the documented assignment/literal grammar with
  explicit size, nesting, field, and record-count limits.
- Parse into source observations before federation. Preserve raw UTF-8 display
  values, source IDs, source revision, parser version, and field provenance.
- Treat mirrors, forks, and downstream databases as the same `lineage_id` as
  their input. Agreement within one lineage is one observation, not a quorum.
- A missing record means `unknown` unless the source policy explicitly marks
  the snapshot exhaustive and a removal protocol has been defined. v1 has no
  automatic deletion based only on absence.
- Schema drift, duplicate source IDs, truncated data, count regression, invalid
  domains, or a non-immutable revision invalidates that source snapshot without
  replacing its last-known-good observation set.

## Federation and OCR boundary

Catalog strings are inference-time lexical constraints. They do not become
training examples, synthetic text prompts, model weights, or repository
fixtures. `searchTerms` and site navigation aliases are excluded from the OCR
lexicon. Exact display variants enter the lexicon only after their source
binding is resolved without fuzzy identity matching.

Each accepted catalog assertion records its contributing source revision and
lineage, and the catalog separately records the latest accepted source
revision. Identical assertions are normalized across revisions so a source
commit that leaves them unchanged cannot cause unbounded snapshot growth. A UI
may show provenance and quarantine diagnostics, but stable recognition events
expose only the internal song ID, accepted exact display title, catalog digest,
and INFINITAS status.
