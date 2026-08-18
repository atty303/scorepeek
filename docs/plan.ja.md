# scorepeek 実装計画

## 状態

- 初回決定日: 2026-08-15
- 最終更新日: 2026-08-18
- repository bootstrapとtarget inventory probe: 完了
- M1.1 catalog contractとlocal federation core: 完了
- M1.2 live acquisitionとsync orchestration: manual/scheduled syncまで完了
- M2 observed-profile private corpus、synthetic renderer、label/replay tooling: 完了
- 元録画をdataset rootとして固定するFFV1 packet-order import/seal/S3-compatible再利用CLI: 完了
- M4 offline canonical/recognition spike: 着手（OBS/vkcapture実録画からnormalizer、共通result layout、
  fail-closed screen判定まで）
- OCR学習/export、field認識、live capture、event daemon: 未着手
- Bazzite実機検証とprivate corpus収集: 着手（OBS/vkcapture実録画1本のcopyless isolated
  import、canonical変換、目視確認まで）

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

capture adapterは取得したpixelを`ObservedFrame`として渡す。observed frameはframe ID、
capture generation、sequence、observed monotonic intervalおよびopaqueなcapture profile IDを
持ち、adapter内でresize、OCR前処理または別profileへのfallbackを行わない。

version固定の`DomainNormalizer`が一つのcapture profile全体をopaqueなdomainとして扱い、
game共通の論理canvasを表すRGB8、top-left、C-contiguous、厳密に1920x1080の
`CanonicalFrame`へ変換する。canonical targetはcapture routeではなくversion固定のgame座標契約であり、
native pixel fixtureや、Portal、Gamescope、OBS等とのpixel equalityを要求しない。Wine、Vulkan、
Gamescope、compositor、PipeWire等のlayerをnormalizerの公開契約へ分解しない。

normalizerは決定的なgeometry、colorおよびfilter補正だけから開始する。learned residual adapterは
初期contractへ含めず、明示変換だけではshared recognition gateを満たせない証拠が得られた場合に
別decisionで検討する。文字や画像を生成するrestoration modelは使用しない。capture profileとnormalizer artifactはsemantic replayで
検証し、observed contractとgateが維持される限り同じartifactを再利用できる。unknownまたはdriftした
profileはfail closedとする。

source removal、caps変更、reconnect、capture profileまたはnormalizer変更ではgenerationを更新し、
未完了のrecognition、dwell、dedupを全消去する。

canonical frame contractはgame共通の`CanonicalLayout`を一つ所有する。layoutはcanonical上のROI、
field type、presence/absence predicate、許容alignmentを持ち、frameはそのimmutable IDまたはdigestを
参照する。capture profileはlayoutを所有しない。各normalizerはobserved geometryを同じlayoutへ写像する。
game UI geometry、canonical frame contractまたはfield contractを変更した場合だけlayout versionを上げ、
全fixture replayを要求する。

初期layoutは、exact observed contractをversion固定normalizerでcanonicalへ変換できる一つのprofileから
計測してよい。そのprofileはpixel referenceやdefault routeにはならない。後から追加するpeer profileは
同じcanonical geometry/layoutへ写すnormalizerだけを校正し、route固有layoutを作らない。

### Recognition API

pureなframe処理とstateful sessionを分ける。

```text
CaptureAdapter.capture() -> ObservedFrame
DomainNormalizer.normalize(observed, profile, artifact) -> CanonicalFrame
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

daemonは`$XDG_RUNTIME_DIR/scorepeek/v1.sock`からsame-UID clientへUTF-8 NDJSONを送る。
filesystem permission、ownership、ACLはoperatorが管理し、scorepeekのevent protocolは
Unix modeを受理条件またはconfidentiality保証にしない。

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

各Bazzite hostでは利用者がmanual-only、任意scheduler、または標準推奨のdaily jitter付き
systemd user timerを選択し、いずれも同じ`scorepeek catalog sync`を実行する。定期起動は
自動で有効化せず、systemd user timerは永続installまたはuser managerの寿命だけのtransient
起動を選べる。gameplay中のdaemonはnetworkを使わない。syncはper-host exclusive writer lockを
source取得前に獲得し、federationとactivation完了まで保持する。

定期起動層はacquisition modeを選択せず、常に同じ`scorepeek catalog sync`だけを呼ぶ。
現在のmodeは各hostがsourceを取得してcatalogをbuildする経路だけとする。将来、sourceごとの
配布許諾と新しいADRに基づいてGitHub管理catalogを採用する場合は、利用者が設定で
`self-build`またはimmutableなprovided catalogの取得を選べるようにする。GitHub上のscheduled
syncもself-buildと同じorchestrationを実行し、配布前にprovenance、content hash、size、schema、
semantic validationおよびsource policyを満たす。provided catalogの取得失敗時も
last-known-goodを維持し、gameplay daemonへnetwork fallbackを追加しない。

1. sourceごとにrevision、content hash、size、schema、record countを検証する。
2. strict parserでtyped observationへ変換する。
3. 前active ledgerを入力としてdeterministic federation candidateを作る。
4. safe addition、same-ID variant、non-conflicting chartだけを適用する。
   revisionだけが変わった同一assertionは既存evidenceへ正規化し、latest source
   revisionはsource-level provenanceとして別に保持する。
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

`CanonicalFrame`は全field recognizerが共有するRGB imageとする。titleやdigitsのROI抽出後に、
field固有のversion固定preprocessorがgrayscale、contrast、resize、paddingおよびtensor normalizationを
適用する。capture domainの補正は`DomainNormalizer`、OCR task固有の最適化はOCR preprocessorへ置き、
trainingとRust runtimeで同一preprocessing contractを使用する。

training corpusは次の二系統に分ける。

- private real crop: 実game frameからtitle ROIだけを保存し、人間がcatalog IDと表示文字列を
  確認する。model predictionによるself-labelは禁止する。
- synthetic: 再配布可能なfontと許諾済み文章またはrandom character n-gramをrenderし、font、
  size、spacing、stretch、outline、shadow、glow、gradient、background、anti-alias、subpixel、
  blur、noise、truncation、4K-to-FHD downscaleを変化させる。

external catalog stringはinference lexiconだけに使い、training textへ自動投入しない。
通常のtrain、validation、holdoutはtitle、session、playをまたがせず、同じcapture profile内でも
独立session/titleによるin-profile holdoutを持てるようにする。これと別に、学習からcapture profile
全体を外したprofile-disjoint evaluation suiteを持ち、未知domainへの汎化を測る。

最初のvertical spikeは同一cropについてPython/PaddleとRust/ORTのlogits、token order、
catalog rankingが許容誤差内で一致することを証明する。PP-OCRv6 ONNX exportが安定しない場合は
PyTorch CRNN/SVTR-style CTC modelをONNX exportする。runtime Pythonへのfallbackは作らない。

認識失敗時はtitle cropとdiagnosticをprivate queueへ保存し、人間が正しいcatalog IDを付け、
次のtraining corpusへ追加する。未知glyph、domain shift、装飾fontはunknownのまま扱い、runtime
thresholdを自動緩和しない。

初回modelは利用可能な専用capture環境からofflineで作成できるが、その環境をpixel correctness
referenceにはしない。普段のGamescope captureからunknown、low-margin、誤認識候補および代表的な
正常例を追加し、人間がlabelした新しいimmutable corpus generationからnormalizerまたはrecognizerを
tuningできる。新bundleは全supported profileの凍結holdoutを再実行し、既存profileを回帰させず、
last-known-goodへrollback可能な場合だけ昇格する。model bundleは一つのcanonical contractとlayout、
各capture profileに対応するnormalizer、OCR preprocessor/model、profile別threshold、corpus generationおよびruntime digestを
一体でbindingする。runtime self-label、online trainingおよび自動threshold緩和は禁止する。

## Capture selection

### Support model

capture route間にpixel correctnessの序列を置かない。Portal、Gamescope directおよび条件を満たす
OBS routeはpeer candidateであり、それぞれのopaque capture profile、normalizer artifact、
recognition thresholdおよびsemantic replay gateを独立に持つが、game共通のcanonical layoutを共有する。
pipeline内部のlayerはruntime contractに
含めないが、再現や診断に有用な環境情報はsecret-safeなprovenanceとして保持できる。

### Candidates

- Wayland ScreenCast Portal + PipeWire: compositor-managed sourceを取得できるcandidate。
- Gamescope direct PipeWire: output-sized streamを取得できる低copy candidate。ただしcapture repaintと
  通常表示のpixel equalityは仮定しない。
- OBS/vkcapture reuse: 実際のobserved contractを独立profileとして検証できる場合のcandidate。
- OBS WebSocket PNG: source/geometry確認用diagnosticに限定し、production backendにしない。

backendは各profileのBazzite semantic/lifecycle/performance gate後にdefaultを選ぶ。session中の
自動fallbackと異なるprofile/normalizerのframe mixingは禁止する。候補がgateに失敗してもFHD、
NV12、別color profileへ黙ってfallbackしない。

## 実装順序

1. **M0**: independent design、repository bootstrap、target inventoryを確立する。
2. **M1.1**: source schema/policy、fixture-only adapters、deterministic federation、
   quarantine report、atomic local snapshotを実装する。
3. **M1.2**: live source acquisition、manual/scheduled sync、activation orchestrationを
   実装する。M1.1だけでM1全体を完了扱いしない。
4. **M2**: normalizer未確定のobserved sourceを保持するprivate corpus schema、元録画を再利用可能な
   dataset rootとして固定するimport/seal/S3-compatible transport、synthetic renderer、label/replay
   CLIを実装する。
5. **M4 bootstrap**: 利用可能なlossless recordingへ最初のversion固定normalizerを与え、その出力だけから
   game共通canonical frame/layout、screen判定、OCR preprocessor/parityのoffline spikeを進める。
6. **M3**: PortalとGamescope directのObservedFrame adapterをvertical spikeとして実装し、Bazziteで
   両routeの実observed contractと校正corpusを確立する。M4 bootstrapのlayoutは移動しない。
7. **M4 completion**: peer profileごとのnormalizerとshared alignmentを検証し、PP-OCR
   fine-tune/exportおよびPython-to-Rust parity gateを完了する。
8. **M5**: 条件付きOBS candidateを含むsupported profileのlifecycle/performanceを比較し、defaultを選ぶ。
9. **M6**: screen、savable、play mode、difficulty/level、digits、title decoder、cross-field validationの順で
   field recognizerを追加する。
10. **M7**: deterministic session、event schema、NDJSON daemonを実装する。
11. **M8**: catalog update replay、full private holdout、Bazzite live flowをrelease gateへ統合する。

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

- title/session/play-disjointのin-profile holdoutと、独立したprofile-disjoint evaluation
- normalizerが同じobserved inputからbyte-identicalなcanonical outputを生成する
- normalizer更新後も全supported profileでsemantic replay regressionがない
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

Portal、Gamescope direct、条件を満たすOBS routeを各15分x3回と30分soakで独立に検証する。

- game共通layoutに対する各profileのcanonical geometry/crop alignment: 許容範囲内
- complete labelに対するsemantic recognition output: 一致
- false accepted event: 0
- p95 frame age: 150 ms以下
- malformed frame、unbounded queue、FD/thread/RSS leak: 0
- start/stop/reconnect: 100回

全semantic/lifecycle gateを通ったprofileだけをsupportedとする。その中から、CPU/GPU/power、
frame age、game p99 frametimeおよびOBS render/encode lagを比較してdefaultを選ぶ。pixel referenceとの
近さを選択理由にせず、supported profileが一つもなければdefaultを設定しない。

## v1対象外

- UI、score/history保存、cloud/外部service連携
- arbitrary resolution、HDR、未較正FSR/NIS/Reshade profile
- public source catalog、real fixture、trained modelの再配布
- upstream branch/resource compatibility
- public release、remote作成、push
