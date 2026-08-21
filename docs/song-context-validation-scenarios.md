# Song-context validation scenarios

This inventory preserves the operator-supplied INFINITAS game flow as
validation material. It is not a runtime state machine and does not make every
scene a scorepeek domain state. Recording replay must confirm the observable
anchors, neutral intervals, and reset boundaries before they become supported
recognition behavior.

## Ordinary session flow

```text
launch
  -> title
  -> mode selection
  -> standard
       -> music selection
       -> non-state transition
       -> gameplay
       -> non-state transition
       -> result
       -> non-state transition
       -> music selection ... without a fixed play-count limit
```

Standard mode also permits two retry shapes:

- gameplay may restart without an intervening result, producing one unusually
  long gameplay scene;
- result may return directly to gameplay for the same song without another
  music-selection scene.

Neither retry shape requires live play-count or attempt inference. Confirmed
non-state scenes, unrecognized frames, gameplay, retries, and result replay
preserve the last stable selection context. A new stable selection replaces it.

## Dan course flow

```text
mode selection
  -> dan course selection
  -> finite repetition of:
       gameplay
       -> non-state transition
       -> ordinary result
  -> optional final dan result after completion
```

The repetition count is not fixed and may end early after failure. Ordinary
results have the same meaning as standard results. The song-context observer
does not infer mode, course progress, play count, failure, or completion. With
no stable music-selection context, result resolution remains screen-local.

## Reset and termination cases

The game can expose a sequence that returns to title or exits normally, and it
can also terminate abruptly. A confidently observed title, session end,
recording coverage gap, or recognition-binding change clears selection
context. A frame that merely has no recognized semantic anchor does not.

Live recognition normally starts with game launch. If observation begins in
the middle, scorepeek emits only observed facts; persistence or another
consumer decides how to compose incomplete history. Standard mode may be an
explicitly marked default assumption outside this context observer, but the
observer does not own mode and never rewrites earlier observations.

## Replay use

At least one retained private recording contains an ordinary play session from
launch onward. Its observed composition is reported here before the projection
into a private label and committed synthetic contract. Real frames, complete
labels, song strings, and recording paths remain outside the repository.

That inspection is now complete for one 458.3-second FFV1 recording. Its media
probe indexes 27,499 contiguous decode positions from source PTS 0 through
458,300 ms. PTS is strictly increasing with a maximum adjacent delta of 17 ms,
so the retained packet index exposes no recording coverage gap. Visual scene
composition was sampled every five seconds. The 145–190-second interval was
additionally sampled every 250 ms to distinguish a direct replay from a short
intervening selection. Scene boundaries below therefore remain sample-bracketed
rather than frame-exact.

| Source PTS | Recording observation | Song-context projection |
| --- | --- | --- |
| 0–50 s | black, launch/loading, warning, publisher splash, then title | clear at the observed title |
| 60–90 s | play-style, player-entry, loading, then mode selection | preserve while empty |
| 105–115 s | stable selection for opaque song A | set A |
| 120–145 s | transition, gameplay, failure transition, result A | preserve; resolve result against A |
| 145–148.5 s | result A | resolve against A without consuming context |
| 148.75–151.75 s | result-to-selection transition | preserve A |
| 152–154.25 s | short stable selection for A | replace with A |
| 154.5–159.5 s | play intro and transition | preserve A |
| 159.75–190 s | gameplay A, failure transition, then result A | preserve; resolve the second result against A |
| 195–235 s | transition back to selection A, with settings overlays | replace with A; overlays preserve |
| 240–280 s | selection changes to opaque song B, then menu/settings overlays and stable B | replace with B; overlays preserve |
| 285–425 s | transition, gameplay B, then result B | preserve; resolve result against B |
| 430–440 s | transition, music-selection scene without a confirmed stable song, then game-ended screen | preserve through the unconfirmed scene; clear on session end |
| 445–458.3 s | post-session black | remain empty |

This recording confirms a non-vacuous session-end clear, but it does not
confirm result-to-gameplay replay without an intervening selection. The dense
interval contains a short music-selection scene that five-second sampling alone
would miss. It also does not contain a gameplay restart without result, dan
play, a return to title after selection, or abrupt termination. Those
operator-supplied cases remain required validation material but are not
recording-derived facts.

The provisional private replay label is bound to the immutable recording,
probe, and capture profile and is marked
`agent_observed_pending_operator_review`. Its SHA-256 is
`cc2dd0a95d2874e9c775311af156aa14ff9da995b71f0fcab3dbdc401970a7e1`.
The committed value-free projection is
`crates/scorepeek/src/song-context-conformance-v1.json`; it contains opaque song
tokens and no recording identity, timestamp, path, player data, or game string.

The replay scenarios must demonstrate only these context properties:

1. a stable selection installs a bounded candidate set;
2. neutral or unrecognized scenes preserve it;
3. result candidates intersect with it to improve song uniqueness;
4. result processing and both retry shapes do not consume it;
5. a new stable selection replaces it;
6. an explicit reset clears it;
7. no mode, attempt, retry-count, or full-session inference is introduced.
