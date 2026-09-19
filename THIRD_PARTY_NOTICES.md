# scorepeek catalog third-party notices

The catalog is a generated data artifact. It is distributed separately from the
scorepeek program, and its contents are not licensed under scorepeek's software
license merely because they are packaged for use by scorepeek.

## Tachi IIDX seeds

- Project: Tachi
- Source: https://github.com/zkldi/Tachi/tree/main/db/seeds
- Role: song and chart identity, display metadata, and INFINITAS availability
- Notice: Tachi describes its seed data as source-of-truth data released under
  the Unlicense. The catalog preserves the exact source revision and content
  hashes in `catalog.sqlite3`.

## Textage

- Site: https://textage.cc/score/index.html
- Readme and usage notice: https://textage.cc/score/readme.html
- Role: independent title, artist, chart, BPM, and INFINITAS corroboration
- Notice: Textage requests common-sense use and recommends linking to the site;
  it does not present these tables under a standard open-data license. This
  catalog contains only the accepted normalized assertions required by
  scorepeek and does not redistribute the source JavaScript tables.
- Usage clarification: On 2026-09-19, in response to our inquiry about
  processing song metadata and providing an application-specific database to
  users, the Textage administrator confirmed that chart-page data may
  currently be used without prior contact, provided that it does not place
  excessive load on the site. Scorepeek stated that it would fetch the data
  once per day.
- References: [inquiry](https://textage.cc/bbs/index.php?res=763&disp=1),
  [administrator response](https://textage.cc/bbs/index.php?res=765&disp=1), and
  [follow-up](https://textage.cc/bbs/index.php?res=766&disp=1).

## dqn/iidxapi

- Project: https://github.com/dqn/iidxapi
- License: ISC, as declared by the upstream
  [README](https://github.com/dqn/iidxapi#license) and
  [package metadata](https://github.com/dqn/iidxapi/blob/main/package.json)
- Published data endpoint: https://dqn.github.io/iidxapi/infinitas/music.json
- Role: positive INFINITAS roster and pack evidence derived from the official
  INFINITAS page
- Notice: The catalog records the acquired content hash and accepted evidence;
  it does not redistribute the source JSON snapshot. The upstream ISC license
  declaration and project credit remain part of this notice.

IIDX and INFINITAS are trademarks of their respective owner. No affiliation or
endorsement is implied.
