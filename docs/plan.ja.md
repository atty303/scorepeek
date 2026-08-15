# scorepeek 実装計画

## 状態

- 決定日: 2026-08-15
- repository bootstrap: 完了
- runtime実装、upstream adoption、fixture収集、Bazzite実機検証: 未着手

## 目的と境界

`scorepeek` は、リズムゲーム画面をcaptureし、曲・譜面・score・resultを
構造化eventとして提供する独立Linuxアプリである。

- upstream Windows applicationへ変更を加えず、Git履歴も共有しない。
- upstreamはrelease tag単位で取得する外部resource/semantic inputとする。
- このrepositoryは将来の独自UIまで収容できるmonorepoとするが、v1は
  capture、認識core、Unix socket APIまでとする。
- runtimeはRustとし、ゲーム中にPythonを必要としない。
- 実行・最終検証先は別のBazzite機、現在の開発機はbuild、unit test、
  fixture replayを担当する。
- upstreamに公開LICENSEがないため、当面はpersonal/private利用とし、
  remote作成、push、public release、resource再配布を行わない。

## Architectureと公開契約

### Repository構成

Rust workspaceは次の責務へ分ける。

- `core`: canonical frame、resource pack、matcher、OCR、session state、event schema
- `capture`: OBS WebSocket PNGとGamescope PipeWireのadapter
- `scorepeek` CLI/daemon: `daemon`、`doctor`、`collect`、`replay`、
  `upstream adopt`、`models sync`
- 将来のUIは別appとして追加し、event APIだけに依存させる

### Canonical frame

全capture backendは、RGB8、top-left、C-contiguous、厳密に1920x1080の
`CanonicalFrame`を生成する。frame ID、backend generation、sequence、
monotonic時刻、取得時刻区間、capture/normalizer profile IDを含める。

backend再接続、source変更、profile変更ではgenerationを更新し、未完了認識、
debounce、dedup状態を全消去する。

### Recognition API

認識はpureなframe inspectionとstateful sessionを分ける。

```text
RecognitionEngine.inspect(frame) -> RecognitionSnapshot
RecognitionSession.process(snapshot) -> DomainEvent[]
```

fieldの内部状態は`known(value) | unknown(reason) | not_applicable`とする。
推測値、generic confidence、未較正値は公開しない。

v1は現行upstream Recognitionの全fieldを対象とする。

- Result: playside、loveletter、dead、play type、play mode、difficulty、level、
  notes、play speed、song、options、graph type/target、clear、DJ level、
  score/miss best/current/new、tab、rival rank/position、notes radar
- Music select: score有無、play mode/type、version、song、selected difficulty、
  clear、DJ level、score、miss、各difficulty level

fieldごとのapplicability predicate、正当なabsence、`known`に必要な証拠は
[`field-semantics.md`](field-semantics.md)をschemaとreplay testの原典とする。
matcher不一致をabsenceと推測してはならない。適用対象のunknownが一つでもあれば
detected eventを出さない。

### Event API

`RecognitionSession`は入力snapshotからdeterministicな`DomainEvent`を出力する。
daemonがそれをtransport envelopeで包み、
`$XDG_RUNTIME_DIR/scorepeek/events-v1.sock` のUnix NDJSONを公開境界とする。

- 親directory 0700、socket 0600、same-UID clientのみ
- eventは`screen_changed`、`result_detected`、`music_select_detected`、
  episode単位の`recognition_rejected`
- transport envelopeにschema version、UUIDv7 event ID、delivery wall timeを持つ
- domain eventにframe/generation/observed monotonic time、upstream tag、
  commit/resource hash、capture/layout/recognition/OCR model profile、domain payload、
  issuesを含める
- 画像、履歴、永続storageは送らない
- client queueはboundedとし、遅いclientだけを切断する
- replayはdomain eventだけを比較し、UUIDとdelivery wall timeをgoldenに含めない。
  daemon integration testではclockとID generatorを注入する

## Capture

### OBS WebSocket PNG backend

Flatpak OBS内蔵のobs-websocket v5へ`127.0.0.1:4455`だけで接続する。
passwordは必須とし、所有者一致かつ0600の外部credentialから読み、通常設定、
CLI引数、ログ、eventへ出さない。

設定したsource UUIDについて次を検証する。

- `inputKind == vkcapture-source`
- expected game executableと完全一致
- cursor、transparency無効
- enabled source filterなし
- active source
- 初回native PNGが厳密に1920x1080

OBS backendはゲームのFHD swapchainをOBSでrenderした画像であり、Gamescope 4K後の
画像ではない。独立したcapture/recognition profileとfixtureを持つ。

通常取得はlossless PNG 1920x1080、最大4 Hz、常に1 requestだけin-flightとする。
遅れたtickは捨て、並列取得やcatch-upを行わない。response、Base64/PNG decode、
recognition間はcapacity 1のlatest-only queueとする。

- response上限16 MiB
- PNG header、寸法、RGBA8、不透明alphaを検証してRGBへ変換
- request開始とresponse受信を取得時刻区間として保存
- 1秒timeout、3回連続失敗で5秒circuit open
- source/filter/scene collection変更では取得を止め、再探索後に新generationを開始
- 1秒以上frameがない場合は最後の候補を失効

### Gamescope direct PipeWire backend

OBSなしでも動作する必須backendとする。`GstDeviceMonitor`でGamescopeの標準
PipeWire nodeを検出し、`node.name=gamescope`、`media.class=Video/Source`の
一意なsourceだけを使う。

v1の実行profileは次へ固定する。

- game internal: 1920x1080
- Gamescope output: 3840x2160
- SDR、default/linear scaling
- native caps: SystemMemory BGRx 3840x2160のみ

NV12、DMABuf-only、別寸法、HDR/10bit、複数nodeはfail closedとする。
stride-awareに受信し、固定bilinear 2:1 normalizerでRGB 1920x1080へ変換する。
受信後最大15 fpsへ間引き、appsinkはlatest-onlyとする。

node removal、ERROR、EOS、caps変更ではpipeline全体を破棄して再列挙し、旧serialや
node IDを再利用しない。

4K BGRxは更新60 fps時にnominal約2 GB/sとなり得る。consumer側のframerate制限は
producer負荷を減らさない。Bazzite性能gateに失敗した場合、FHDへ黙って落とさず、
DMABuf/GPU normalizerまたはPortalを別計画として再設計する。

backendは設定で明示選択し、実行中の自動切替やframe混在を禁止する。

## Upstream adoptionとresource

`upstream.lock.json`は生成対象とし、upstream URL、release tag、commit SHA、
取得対象、入力と生成packの各SHA-256、resource schema、layout profileを記録する。
submodule、subtree、Git fork関係は作らない。

adoptionは、信頼判断とarbitrary-code-capableなunpickleを同一操作にしない。

1. `upstream inspect <tag>`はimmutable tag/commitとresource byteをXDG cacheへ取得し、
   filename、source URL、commit、SHA-256だけのcandidate manifestを出す。
   Pythonとunpickleは起動しない
2. 人間がprovenanceとexact digestを明示的に承認し、
   `upstream/approved/<tag>.json`を別のlogical commitで確定する。inspectは自動承認しない
3. `upstream adopt --approved ...`は再取得した全byteを承認済みmanifestと照合し、
   一つでも不一致ならunpickle前に停止する
4. adoption全体の排他lockを取り、networkなし、非root、read-only input、
   秘密情報なしの隔離containerでPython importerを実行する
5. dtype、shape、key schemaを検証し、external storeの同一filesystem上の
   staging directoryへdeterministicなMessagePack+zstd packを生成する
6. semantic diffとprivate fixture replayをstaging artifactに対して実行する
7. pack、model、dictionaryをcontent-addressed pathへ確定し、それぞれの
   output digest、input resource digest、schema adapter、layout、capture/recognition
   profile、model/dictionary/runtime compatibilityを`active-manifest.json`に結び付ける
8. 全fileとdirectoryをflushした後、active manifestをatomic renameで公開する。
   失敗時はstagingを消去し、旧active manifestは変更しない
9. 全gate成功後だけ`upstream.lock.json`のproposed updateを生成する

runtimeはpickle、upstream Python module、`resources.py`をimportせず、networkにも
接続しない。起動時にactive manifestと全artifactのheader/schema/digest/bindingを
検証し、不一致なら`not_ready`にする。event provenanceは検証済みactive manifestから
作り、candidateやrepository lockの値を代用しない。imported pack、original resource、
modelはrepositoryへcommitしない。

### OCR model取得

`models sync`もcandidate自身のhashを信頼起点にしない。repositoryにcommitする
`models/approved/<model>.json`で、immutable source URL/revision、model、dictionary、
configの各SHA-256、license/attributionとlicense textのdigest、preprocess schema、
互換OAR/ORT versionを取得前に固定する。approval manifestはsyncとは別のlogical
commitで人間が承認する。

`models sync --approved ...`はstagingへ取得した全byteを既存approvalと照合し、
不一致なら公開しない。任意のlocal pathも承認済みdigestと一致しない限り受け付けない。
一致後はresource packと同じsingle-writer lock、content-addressed store、atomic active
manifest経路を使う。OARその他のruntime auto-downloadはbuild featureとruntime configの
両方で無効化し、daemonは検証済みlocal pathだけを開く。missing/mismatch時は
network fallbackせず`not_ready`にする。

`.res`に含まれないFHD top-level crop座標は、scorepeek所有のversioned
`LayoutProfile`へ明示する。resource hash、capture profile、normalizerへbindingし、
replayが壊れた場合だけ新versionを追加する。

## Recognition

- upstream resource由来templateのexact一致を第一候補とする
- 必要なfieldだけabsolute distanceとrunner-up marginを較正する
- alignmentはcanonical上の±1 pixelまで
- global fuzzy flag、threshold自動緩和、default値は禁止
- 数値は全桁が一意に読めた場合だけ返す
- graph type不一致時のGAUGEなどlegacy defaultは互換しない

曲名はupstream musictableのclosed setとする。resource templateを主証拠、
PP-OCRv6 small recognition-onlyを独立した補助証拠とする。

- OCR文字列単独ではacceptしない
- exact Unicode、whitespace normalization、NFKCの段階でcatalogへ照合
- punctuation、symbol、caseを広く削除しない
- resourceとOCRが同じ一意候補を示し、play mode、difficulty、level、versionが
  catalogと整合した場合だけaccept
- exact upstream resource matchだけは単独accept可能

title/version/play mode/difficulty/level、notes、score、miss、best/current/newの
範囲と相互整合を検証する。

resultはresult state、savable、全適用fieldが500 ms安定したとき一度だけemitする。
music selectは完全identityと全適用fieldが500 ms安定したときだけemitし、
`(play type, song, difficulty)`でdedupする。

安定性は、generationが同じでsequenceの異なる最低2つのcapture observationが
同じcandidateを示し、最初と最後のobserved monotonic timeが500 ms以上離れた
ときだけ成立する。最新frameはbackend profileの`max_frame_age`以内でなければならず、
sourceが接続中でも同一frameの経過時間だけをdwell証拠にしない。stall、disconnect、
generation変更はcandidateをemit前に破棄する。

## 実装順序

1. Bazzite target inventory probeを作り、OS image、GPU、Gamescope、GStreamer、
   PipeWire、Flatpak OBS、obs-vkcapture、obs-websocket、flags/capsを記録する。
2. targetと同じFedora majorのdigest-pinned build image、Rust、Python importer環境を
   lockする。
3. upstream importerとsafe resource packを完成させる。
4. 両capture adapter、`doctor`、外部private corpusへ保存する`collect`を実装する。
5. Bazziteでcapture contractと負荷を検証する。
6. fixtureをsession/play/title単位でcalibration/holdoutへ分割して凍結する。
7. screen/gate、closed-set field、数字、曲名OCR、cross-field validationの順で実装。
8. deterministic domain sessionとdaemon-owned transport envelopeを持つNDJSON daemonを
   追加し、full replayとlive flowを通す。
9. upstream adoptionの全経路を自動化し、release gateへ組み込む。

全面実装前に、OBS capture、Gamescope capture、resource importの各外部境界を
vertical spikeで検証する。失敗したspikeを互換layerとして残さない。

## Fixtureと認識release gate

private corpusはrepository外のcontent-addressed directoryへ保存する。real captureの
完全labelはscore、best/current、miss、rival情報などの個人dataを含み得るため、
画像と同じprivate storeに置く。repositoryにcommitできるのはschema、opaqueな
fixture ID/hash、非個人的なscreen/class label、独立に作成したsynthetic contract
fixture、明示的にredact済みのexpected value、runnerだけとする。

初期corpusは次を含める。

- 両backendのpaired scene
- 全screen state、transition、black、cut-in、overlay、desktop、wrong-window
- SP/DP、両playside、savable/non-savable、dead、loveletter
- 全closed enum
- field/fontごとの数字0-9、blank、leading zero、NEW
- 日本語、Latin、mixed、symbol-heavy、短名、長名、confusable pairを含む200曲以上
- 20 session以上、3,000 independent negative decision window

adjacent frameをrandom splitせず、session、play、title単位で分割する。OCR holdoutは
title-disjointとする。

calibrationで「wrong accept 0の候補中、受理率最大」のfield別thresholdを固定し、
holdoutは一度だけ評価する。holdoutで誤受理したsampleはcalibrationへ移し、新しい
holdoutを収集する。

認識release条件:

- emitted result/music-selectの適用field正答率100%
- negativeからのevent 0、曖昧候補の誤推測0
- stable result acceptance 90%以上
- stable music-select acceptance 80%以上
- paired backendでsemantic output一致
- result episodeごとにevent 1件、music-select identity重複なし
- exit/reconnect/profile generation変更で正しくreset
- candidate後のframe stallでeventを出さず、freshな異なるobservationのみでstable化
- resource/model/layout/profile mismatchをnot-readyまたはreject

## Bazzite実機gate

### OBS backend

実配信設定で「配信のみ」と「配信+4 Hz scorepeek」をrandom順に各15分x3回、さらに
30分soakする。

- game p99 frametime増加 <= 0.25 ms
- OBS average render time増加 <= 0.20 ms
- OBS render/output skipped率増加 <= 0.01 percentage point
- screenshot RTT p95 <= 200 ms、p99 <= 500 ms、1秒超0件
- 実効取得率 >= 3.8 fps
- OBS CPU増加 <= 3 percentage points
- RSS増加 <= 64 MiB、かつ単調増加なし
- stable event latency p95 <= 1秒
- false accept 0

一つでも失敗した場合、OBS backendをproduction capabilityとして残さない。

### Gamescope backend

- 5分capture smoke
- 10分animation
- 30分soak
- 100回start/stop/reconnect
- malformed frame 0
- 500 ms超stall 0
- FD/thread/RSS leak 0
- game p99 frametime悪化 <= 1 ms
- 1% low低下 <= 5%

このgateに失敗した場合、OBSなし動作要件は未達として停止し、capture architectureを
再検討する。

両backendそれぞれからfull-parityのmusic-select eventとresult eventを最低1件liveで
取得するまでstableとしない。

## v1対象外

- UI、score保存、外部service連携
- OBS plugin、Virtual Camera、Portal
- OpenCV、custom学習model
- FSR/NIS、Reshade、HDR、任意解像度
- 旧Linux branch互換
- public release、remote作成、push
