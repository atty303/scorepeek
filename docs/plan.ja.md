# scorepeek 実装計画

## 状態

- 決定日: 2026-08-15
- repository bootstrapとtarget inventory probe: 完了
- M1.1 catalog contractとlocal federation core: 完了
- M1.2 live acquisitionとsync orchestration: dqn manual syncまで完了、継続中
- Tachi/Textage live adapter、scheduled sync、capture、認識、OCR学習、event daemon: 未着手
- Bazzite実機検証とprivate corpus収集: 未着手

現在commitに含まれるcheckpointは[`STATUS.md`](../STATUS.md)を参照する。この文書は
stable milestoneの実装順序とrelease gateの原典である。長期的な判断理由は
[`decisions/`](decisions/README.md)に、外部dataの利用境界は
[`sources.md`](sources.md)に置く。

## 目的と境界

`scorepeek`は、Linux上のIIDXゲーム画面をcaptureし、music-selectとresultを
構造化eventとして提供する独立applicationである。

- Windows upstreamとGit履歴を共有せず、code、座標、`.res`、music database、
  image、生成物を取得・importしない。
- upstreamの画面構造は初期調査の着眼点にだけ利用できる。commitする座標は
  scorepeek自身のcaptureから再計測し、根拠fixtureへ結び付ける。
- v1はcapture、catalog federation、認識core、offline OCR training、Unix socket
  APIまでとする。UI、score保存、外部service連携は対象外とする。
- gameplay runtimeはRustとし、Pythonはoffline OCR trainingとONNX exportにだけ使う。
- 実行・最終検証先は別のBazzite機、現在の開発機はbuild、source fixture test、
  model training/evaluation、capture replayを担当する。
- repository、corpus、外部source cache、model storeを分離する。real capture、完全label、
  raw external source、生成catalog、modelはcommitしない。

## Architectureと公開契約

### Canonical frameとlayout

全capture adapterは、RGB8、top-left、C-contiguous、厳密に1920x1080の
`CanonicalFrame`を生成する。frame ID、capture generation、sequence、observed
monotonic interval、capture/normalizer profile IDを含める。

初期profileは、Gamescope scale後の3840x2160 SDR frameを取得し、version固定の
2:1 normalizerでcanonical FHDへ変換する。native FHD game swapchainはpre-scaleなので
契約外とする。source removal、caps変更、reconnect、profile変更ではgenerationを更新し、
未完了のrecognition、dwell、dedupを全消去する。

`LayoutProfile`はcanonical上のROI、field type、presence/absence predicate、許容alignmentを
持つ。座標はprivate capture上でscorepeekが計測する。値を変更した場合はprofile versionを
上げ、全fixture replayを要求する。

### Recognition API

pureなframe処理とstateful sessionを分ける。

```text
RecognitionEngine.inspect(frame, catalog, model) -> RecognitionSnapshot
RecognitionSession.process(snapshot) -> DomainEvent[]
```

fieldは`known(value) | unknown(reason) | not_applicable`で表す。matcher failure、曖昧値、
未較正classを既定値やabsenceへ変換しない。

v1の必須fieldは次とする。

- result: screen state、savable、playside、play mode/type、song、difficulty、level、notes、
  current score
- music select: screen state、play mode、song、selected difficulty、selected level

clear、DJ level、miss、best/current/new、option、graph等は各fieldの独立fixture gateを
満たしたものからoptional `known`として追加する。必須fieldまたはcross-field validationが
一つでも失敗したcandidateはdetected eventにしない。

### Event API

daemonは`$XDG_RUNTIME_DIR/scorepeek/v1.sock`をparent 0700、socket 0600で公開し、
same-UID clientへUTF-8 NDJSONを送る。

- request: `hello`、`get_status`、`subscribe`
- accepted event: `result_detected`、`music_select_detected`
- lifecycle/status: capture generation、catalog/model readiness、quarantine summary
- envelope: schema version、event ID、monotonic time、capture generation、capture/layout/
  catalog/model/runtime digest
- payload: immutable `scorepeek_song_id`、exact display title、INFINITAS status、typed fields

画像、crop、raw OCR text、候補list、source record、保存履歴はsocketへ流さない。client queueは
boundedとし、遅いclientだけを切断する。future UIはこのAPIだけへ依存する。

## Catalog federation

### Source contract

各adapterはdownloaded codeを実行せず、immutable revisionまたはcontent digestへ固定した
`SourceSnapshot`を生成する。

```text
SourcePolicy
  source_id / lineage_id / parser_version
  revision strategy / declared scope / completeness
  field authority / freshness / rights and provenance

SongObservation
  source song key / external IDs / exact title variants
  artist / version / availability assertions / charts

ChartObservation
  play type / difficulty / level / notes
  source-local game IDs / product versions / primary status
```

initial sourceは次とする。

- Tachi seeds: 一般IIDX identity/chart anchor
- Textage direct data: 独立lineageのtitle/artist/BPM/chart/INFINITAS照合
- dqn/iidxapi: 公式INFINITAS page由来の収録/pack signal

RemyWiki、BEMANIWiki、KONAMI公式、その他community DBはv1の自動ingestionへ入れない。
権利とlineageの詳細は[`sources.md`](sources.md)を参照する。

### Identityとfield resolution

v1の`scorepeek_song_id`は、固定namespaceとexact Tachi song IDからUUIDv5で決定する。
Tachi anchorをまだ持たないTextage/dqn observationはprovisionalのまま保持し、public catalogへ
activationしない。既存ledgerへ後からsource bindingやdisplay variantが増えてもIDを変更・再利用
しない。

自動identity edgeは次だけを許す。

1. 既存のexact `(source_id, external_id)` binding
2. policyで同じnamespaceと明示したexact game ID
3. NFC title、NFC artist、versionが完全一致し、さらに複数の
   `(play type, difficulty, notes)`が一致する独立lineage間のrecord

case folding、記号除去、NFKC、edit distance、substring、発音、search aliasをidentityに
使わない。mirrorと派生sourceは同じlineageの一票である。一つのobservationが二つの既存IDを
結ぶ場合はmergeせず、新しいedgeをquarantineする。

dqn rowはstable keyを持たずidentity edgeを作らない。raw NFC `(title, artist)` tupleが
既存active Tachi-anchor recordのちょうど一つへ完全一致した場合だけ、INFINITAS availability
evidenceとしてbindingを保存する。0件または複数件はquarantineする。前回accepted tupleを
同じTachi IDへ再解決できない、または一つでもsnapshotから消えた場合、そのsnapshotの全新規
dqn bindingを昇格せず、previous accepted binding setを維持する。したがってrenameを推測しない。

titleは`in_game_display`、`official_display`、`eamusement_csv`、
`alternate_display`、`search_term`を分離する。`search_term`はidentityとOCR lexiconへ
入れない。同じstable source IDの表記変更は新しいexact display variantとして追加し、
旧variantを保持する。

catalogは一般IIDX candidate全体を持ち、INFINITAS状態を
`confirmed_present | unknown | conflicted`で別管理する。未確認candidateは通常より厳しい
title marginと画面contextを通った場合だけ認識でき、収録済みとは断定しない。sourceからの
一時的な欠落はremoval evidenceにしない。Tachi primary chartの`versions`に`inf`があるか、
上記のexact dqn bindingがある場合をpositive evidenceとする。TextageのINFINITAS flagは
corroborationに限定し、単独で状態を確定しない。

### Syncとactivation

各Bazzite hostがdaily jitter付きsyncと明示的な`scorepeek catalog sync`を実行する。
gameplay中のdaemonはnetworkを使わない。syncはper-host exclusive writer lockをsource取得前に
獲得し、federationとactivation完了まで保持する。

1. sourceごとにrevision、content hash、size、schema、record countを検証する。
2. strict parserでtyped observationへ変換する。
3. 前active ledgerを入力としてdeterministic federation candidateを作る。
4. safe addition、same-ID variant、non-conflicting chartだけを適用する。
5. unsupported token、ambiguous identity、existing-ID bridge、critical conflict、source health
   regression、recognition replay regressionはrecord単位でquarantineする。
6. model hashへbindingした保存済みCTC logitsをexpanded lexiconで再scoreし、既存accepted
   song IDとmarginが維持されることを確認する。
7. rename直前に基底active digestを再読し、開始時から変化していればcandidateを破棄して新しい
   ledgerから再構築する。
8. 同一filesystemのstagingでSQLite snapshotとmanifestを完成させ、各fileとstaging directoryを
   fsyncしてcontent-addressed pathへrenameする。直後にcontent storeのdestination parent
   directoryをfsyncし、新snapshotをdurableにしてからactive manifestへ進む。
9. manifest directory内のtemporary `active.json`をwrite/fsyncしてatomic replaceし、manifest
   parent directoryをfsyncしてからlockを解放する。

どの段階で失敗してもlast-known-goodを変更しない。quarantineは次回syncで全source evidenceを
再評価し、別lineageが追い付けば人手なしで昇格できる。

## OCR modelとcorpus

曲ごとのclosed-set classifierやimage prototype databaseは作らない。固定title ROIを
sequence modelへ入力し、CTC logitsをexact catalog title trieへ直接scoreする。

- baseline: PP-OCRv6 small recognition modelをfine-tune
- training/export: pinned Python、Paddle/PaddleOCR、offline environment
- runtime: pinned ONNX modelをRust/ONNX Runtimeで実行
- model dictionary: 現在catalogへ縮小せず、pretrainedの広い文字集合を維持
- acceptance: field/profile別absolute bound、runner-up margin、temporal agreementと、screenごとの
  独立image context。resultはplay mode/difficulty/level/notes、music selectはplay mode/selected
  difficulty/selected levelを使う。versionは独立image fieldとして認識できた場合だけ追加証拠にする
- diagnostic open-text decode: private evaluationだけ。stable eventには出さない

training corpusは次の二系統に分ける。

- private real crop: 実game frameからtitle ROIだけを保存し、人間がcatalog IDと表示文字列を
  確認する。model predictionによるself-labelは禁止する。
- synthetic: 再配布可能なfontと許諾済み文章またはrandom character n-gramをrenderし、font、
  size、spacing、stretch、outline、shadow、glow、gradient、background、anti-alias、subpixel、
  blur、noise、truncation、4K-to-FHD downscaleを変化させる。

external catalog stringはinference lexiconだけに使い、training textへ自動投入しない。
splitはtitle、session、capture profile単位とし、holdoutはtitle-disjointかつ可能な範囲で
font/profile-disjointにする。

最初のvertical spikeは同一cropについてPython/PaddleとRust/ORTのlogits、token order、
catalog rankingが許容誤差内で一致することを証明する。PP-OCRv6 ONNX exportが安定しない場合は
PyTorch CRNN/SVTR-style CTC modelをONNX exportする。runtime Pythonへのfallbackは作らない。

認識失敗時はtitle cropとdiagnosticをprivate queueへ保存し、人間が正しいcatalog IDを付け、
次のtraining corpusへ追加する。未知glyph、domain shift、装飾fontはunknownのまま扱い、runtime
thresholdを自動緩和しない。

## Capture selection

### Correctness baseline

Wayland ScreenCast Portal + PipeWireをpost-scale correctness referenceとする。portal pickerや
compositor差を含むため、profileはBazzite image、desktop、portal backend、format、stride、
colorimetry、Gamescope command/versionへbindingする。

### Candidates

- Gamescope direct PipeWire: 4K sourceを直接受けられる低copy候補。ただしcapture repaintは
  normal outputと同一とは限らない。
- OBS/vkcapture reuse: 配信と同じ4K post-scale sourceを共有できる場合だけ候補。game processの
  native FHD sourceはpre-scaleなのでrejectする。標準Wayland Gamescopeにcapture可能なswapchainが
  ない場合、forced SDL backendを含む別profile全体を比較する。
- OBS WebSocket PNG: source/geometry確認用diagnosticに限定し、production backendにしない。

backendはBazzite gate後に一つをdefaultにする。session中の自動fallbackと異なるprofileのframe
mixingは禁止する。候補がgateに失敗してもFHD、NV12、別color profileへ黙ってfallbackしない。

## 実装順序

1. **M0**: independent design、repository bootstrap、target inventoryを確立する。
2. **M1.1**: source schema/policy、fixture-only adapters、deterministic federation、
   quarantine report、atomic local snapshotを実装する。
3. **M1.2**: live source acquisition、manual/scheduled sync、activation orchestrationを
   実装する。M1.1だけでM1全体を完了扱いしない。
4. **M2**: private corpus schema、layout measurement、synthetic renderer、label/replay CLIを実装する。
5. **M3**: PP-OCR fine-tune/exportとPython-to-Rust parity spikeを通す。
6. **M4**: Portal reference adapterを実装し、Bazziteでpost-scale canonical contractを確定する。
7. **M5**: Gamescope directと条件付きOBS candidateをvertical spikeし、correctness/performanceを比較する。
8. **M6**: screen、savable、play mode、difficulty/level、digits、title decoder、cross-field validationの順で
   field recognizerを追加する。
9. **M7**: deterministic session、event schema、NDJSON daemonを実装する。
10. **M8**: catalog update replay、full private holdout、Bazzite live flowをrelease gateへ統合する。

新規runtime、training、parser、capture dependencyは、version、license、代替案、bundle/host影響を
一括提示して承認を得た後にだけ追加する。

## Verificationとrelease gate

### Catalog

- source順、property順、並列順に依存せずbyte-identical output
- 同じinputとledgerからのrebuildがidempotent
- mirrorが複数票にならない
- fuzzy-similar songをmergeしない
- same-ID renameで内部ID不変、旧exact variant保持
- existing 2 IDを結ぶevidenceをmergeせずquarantine
- dqn rowをunique exact title+artistだけで既存Tachi IDへavailability bindingし、0/multipleを隔離
- accepted dqn tuple消失時は全新規bindingを保留し、renameとnew additionを推測で取り違えない
- source schema drift、truncated data、duplicate ID、件数急減を隔離
- activation途中失敗で旧snapshot維持
- scheduled/manual syncの並行実行をsingle writerへ直列化し、base digest変更時にlost updateを起こさない
- staging、snapshot rename、destination-parent fsync、active-manifest rename、manifest-parent fsyncの
  各crash pointで旧または完全な新snapshotだけが見える
- catalog追加後も保存済みlogitsのaccepted song ID不変

### OCRとrecognition

- title/session/profile-disjoint holdout
- Python/PaddleとRust/ORTのlogits/ranking parity
- trainingとcatalogからtitleを外してmodelを固定し、catalog recordだけ追加した後にheld-out real
  cropを認識するnew-song simulation
- accepted result/music-selectの必須field誤り0
- negativeからのevent 0、ambiguous candidateの推測0
- stable result identity acceptance 90%以上
- stable music-select identity acceptance 80%以上
- episodeごとにresult event 1件、music-select identity重複なし
- disconnect、generation変更、stall、screen exitでcandidate reset

### Bazzite capture

Portal、Gamescope direct、条件を満たすOBS routeを各15分x3回と30分soakで比較する。

- canonical geometry/crop alignment: Portal比1 pixel以内
- paired semantic recognition output: 一致
- false accepted event: 0
- p95 frame age: 150 ms以下
- malformed frame、unbounded queue、FD/thread/RSS leak: 0
- start/stop/reconnect: 100回

Portal以外を採用するのは、CPU/GPU/powerの少なくとも一つが20%以上改善し、他指標が5%以上
悪化せず、game p99 frametimeが+1 ms以内、OBS render/encode lagが悪化しない場合だけとする。
該当候補がなければPortalをdefaultにする。

## v1対象外

- UI、score/history保存、cloud/外部service連携
- arbitrary resolution、HDR、未較正FSR/NIS/Reshade profile
- public source catalog、real fixture、trained modelの再配布
- upstream branch/resource compatibility
- public release、remote作成、push
