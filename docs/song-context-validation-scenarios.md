# Song-context validation scenarios

This reference describes the current INFINITAS flow semantics used by the
song-context observer. It is not a runtime state machine and does not make every
scene a scorepeek domain state. The observer uses only explicit semantic
anchors, neutral intervals, and reset boundaries.

## Ordinary session flow

```text
launch
  -> title
  -> play side / play style / player entry
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
Being in a music-selection scene does not imply that a song title is visible:
option settings and other overlays may obscure it. An overlay or unreadable
selection preserves the last stable context; only a newly recognized stable
selection replaces it.

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

## Conformance

The value-free projection in
`crates/scorepeek/src/song-context-conformance-v1.json` contains opaque song
tokens and no recording identity, timestamp, path, player data, or game string.
Its tests demonstrate only these context properties:

1. a stable selection installs a bounded candidate set;
2. neutral or unrecognized scenes preserve it;
3. result candidates intersect with it to improve song uniqueness;
4. result processing and both retry shapes do not consume it;
5. a new stable selection replaces it;
6. an explicit reset clears it;
7. no mode, attempt, retry-count, or full-session inference is introduced.
