# scorepeek 実装計画

## 状態

- 初回決定日: 2026-08-15
- 最終更新日: 2026-08-26
- repository bootstrapとtarget inventory probe: 完了
- M1.1 catalog contractとlocal federation core: 完了
- M1.2 live acquisitionとsync orchestration: manual/scheduled syncまで完了
- M2 observed-profile private corpus、synthetic renderer、label/replay tooling: 完了
- M3 common PipeWire receiverとGamescope observed-frame profile: 着手（default remoteのbounded
  registry round trip、exact Gamescope `Video/Source` discovery、選択nodeとdefault remoteを保持する
  未校正lifetime lease、BGRxだけを提示する未校正receiver、およびbounded live/lifecycle gateまで。
  Bazzite上のheadless Gamescope/vkcubeで60/1要求、実callbackでのnegotiation/frame reception、
  source loss、250 ms consumer pressure下のlatest-frame overwrite、receiver-first shutdown、100回の
  source acquire/start/stopを確認済み。operatorがINFINITASを起動したGamescope sessionでも、pixelを
  保持しないgateにより2556x1428 BGRx MemFd、約60 fps、consumer pressure、100回のreceiver lifecycleを
  確認済み。ただしgate自体は表示contentを識別しない。operatorによるとこのsessionはnative
  1920x1080をGamescopeのdefault linear filterとauto scalerで環境固有の2556x1428へscaleしたものであり、
  次回から`-F linear`を明示する。実験機のexact post-scale contractを独立profileへ校正して開発を進め、
  将来の4KまたはFSR等は別profileとして扱う。既知の四辺markerを持つ1920x1080 patternを
  `-S auto -F linear`で実測した結果、2556x1428では上下を全て使い、左右にideal 8 2/3 pixelずつの
  narrow pillarboxを置くaspect-preserving fitだった。runtimeの黒帯検出は使わず、このfractional geometryを
  exact rational source rectangleとhalf-pixel/Q11 linear samplingでRGB8 1920x1080へ戻すpure Rust stageを
  実装し、既知patternとprivate live sampleでgeometryを検証済み。非標準windowのfractional geometryもadvancedな
  明示設定として利用可能にする。通常sessionの自動測定、自動profile生成および組合せごとのfirst-class gateは
  提供しないが、ADR 0047のoperator-requested setupだけはscorepeek-owned markerからgeometryを導出して
  machine-local profileを発行できる。
  calibration evidence、exact Gamescope version/backend/configuration、full BGRx video/memory/stride
  contract、opaque profile digest、fractional normalizerを一体でfail-closed検証する64 KiB以下のcanonical
  immutable binding contractまで実装済み。さらにlive INFINITAS用のexplicit Wayland
  backend/output/nested/scaler/filterを保存するsession sampleを実captureし、raw frameとmanifestを独立再hashした
  create-only development-machine binding artifactをprivate local stateへ登録済み。独立したhalf-pixel/Q11
  normalizerで既知marker geometryを再確認し、fresh Wayland sessionでbinding一致を受理した後、別のfresh
  Wayland sessionのgeneration 23で同じbindingからcanonical frameを生成した。以前のSDL marker artifactは
  そのbackend固有のcontrolled evidenceとして残すが、Wayland sessionのbindingまたは検証根拠には使用しない。
  新規leaseへlauncher/operatorが明示したsession provenanceを保持し、
  bindingの全provenance fieldとreceiverが実negotiationしたvideo/memory/stride contractが一致した場合だけ
  calibrated leaseへ昇格する境界まで実装済み。受理・拒否は値を持たないtyped factとしてbounded capture
  diagnosticへ記録する。nested refresh不一致拒否は以前のSDL固有profileで確認済みであり、Wayland profileでは
  exact一致の受理だけを確認済み。
  calibrated leaseだけがcapture generation/profile/normalizer identity付き`ObservedFrame`を生成し、同じleaseの
  binding-selected fractional normalizerだけがRGB8 1920x1080の`NormalizedCanonicalFrame`へ変換できる境界も
  実装済み。generation/profile/normalizer mixingはfail closedで、最初のnormalization success/failureだけを
  bounded typed factへ記録する。generation 1から2へのrolloverと同じcanonical RGB8 digestの再現も以前のSDL固有
  profileの証拠であり、Wayland profileのgeneration 23とは別に扱う。さらに`NormalizedCanonicalFrame`のpixel ownerを2回目のRGB copyなしで
  application-owned `BoundCanonicalFrame`へ移し、固定cadenceのbounded diagnostic workerへofferする境界まで実装した。
  callerがgeneration/profile/normalizer/pixelを作れる旧public constructorは削除し、profileとnormalizer identityは選択済み
  bindingからのみ導出する。provider/receiver/frame/diagnostic runはprovider lease起点のmonotonic clockを共有し、
  receiver/provider shutdown結果を確定してからdiagnostic manifestをfinalizeする。controlled marker animationのgeneration 16では13 normalized frames中3 framesをcompleteな
  diagnostic runへ保存し、generation 17のopt-outでは7 framesを正規化したまま保存を0件にした。さらに同じlive ownerを借用する
  screen predicate handoffを追加し、generation 18のunknown 13件と、独立生成した色矩形によるgeneration 20のresult 2件/
  music-select 11件を同じrunのtyped factへ記録した。generation 19のopt-outでもunknown 6件は変わらずartifactは0件だった。
  completeなimmutable descriptorを所有するapplication recognition sessionも実装し、binding identity変更時は旧runへ
  typed change factを記録してfinishした後にだけ次sessionを開始する。result/music-selectだけをoffline exportと同じ
  filesystem-free screen-local crop APIへrouteし、live frame ownerを借用したtyped RGB8 cropsを生成する境界も実装済み。
  resultはtitle/artist/clear type/difficulty/level/notes/current score、music selectはcentral title/artist/selected chart/active-list titleを
  必須fieldとして持ち、補助contextだけの中間shapeは残さない。未測定fieldを空のoptional cropとして表現せず、unknownも
  field inputsを生成できない。さらにcomplete descriptorから導出したrun IDと全bindingを保持し、loaderをcapture開始前に
  1回だけ呼ぶapplication-owned field-observer worker境界を実装した。capacity 2のnon-blocking queue、worker-only execution、
  queue取得後も含むaccepted-but-unconsumed resultのglobal capacity、provenance-bound result、race-free abandoned count、
  observer teardownまで保持するsingle-worker supervisor、5秒bounded finishを持つ。さらにactive catalog digest、登録済み
  PP-OCRv6-small model digest、固定CPU runtime manifest digestを照合し、catalog/dictionary/ONNX sessionをworker開始前に
  1回だけ保持するproduction resource loaderとread-only gateを実装した。gateはresourceをproduction field workerへ移し、cropを
  submitせずbounded teardownまで確認する。さらにcomplete crop setをworker thread上で登録済みruntimeへ通すproduction
  screen-field observerを実装した。resultはtitle/artist/clear type textとdifficulty/level/notes/current scoreの明示的な未実装状態、
  music-selectはcentral title/artist/active-list title textとselected chartの明示的な未実装状態を持つcompleteなscreen別型だけを
  出力し、単一field失敗もfield ID付きwhole-screen errorにする。diagnosticはscreen、固定field count、失敗field IDを保持し、
  opt-out/rejectionは認識結果を変えない。認識判断を実装する段階ではoperator-owned local artifactへboundedなexact OCR text、
  run単位のexact catalog display/comparison string table、song ID、string reference、全candidate metric、判断と理由も保持する。さらに同一immutable descriptorのrecognition sessionと
  registered field workerを一つのapplication ownerへ統合した。
  resource load完了後にdiagnostic-backed runを開始し、screen result、non-blocking field submit、diagnostic outcomeを分離して返す。
  opaque owner/pending tokenにより別runはoutputをconsumeできず、completed/disconnected handleは一度だけterminal resultを返す。
  capacity 2のexact pending-sequence ledgerからabandonmentを記録し、lifecycle timeout/worker lossは架空sequenceへ結び付けずunboundにする。
  field workerから先に終了し、synthetic current-run cropでcomplete outputとfield fact、opt-out時のartifact 0件、capacity loss時の
  recognition非干渉とpartial manifest、cross-run pending rejectionを検証済み。さらにactive catalog全songをstable ID順に保持する
  pure candidate domainを追加した。resultの
  title/artistとmusic-selectのcentral title/artist/active-list titleごとにraw/exact comparison key/domain-unique folded formを比較し、
  minimum edit distanceとinteger normalized similarityを全songについて別々に保持する。ranking、top-N truncation、field間集約、
  threshold、accepted field、song decision、selection context更新、diagnostic side effect、eventは行わない。folded observationは
  domain-unique folded candidate formとのみ比較し、search-term-only songはdropやpanicではなくtyped errorでdomain構築を停止する。
  さらにretained recording evidenceからresult-song resolver v1を固定した。titleのunique minimum edit candidateに対し
  edit distance 1以下、normalized similarity 6/7以上、runner-up edit margin 2以上、選択candidateのartist similarity 2/5以上を
  exact integerで要求し、artist scoreをtitleへ加算しない。失敗はtyped unknownにする。profile v2は全episodeのexact expected song IDをbindし、
  exact songと`CLEAR TYPE`を各2 frame以上要求する。local artifactはexact OCR/catalog strings、全candidate metrics、decision/reason、expected valuesを保持する。
  Gamescope liveでは同じserializerをcapture loop外のcapacity 2 workerで使用し、live monotonic intervalをrecording PTSと区別する。
  新しいvalue-evidence gateは1件以上のcompleted result resolution、全completed observationのenqueue、manifest完了を要求し、
  compact outputはartifact status/count/digestだけを持つ。timeout workerが実際に終了するまではprocess-wide supervisorが次runを拒否する。
  通常運用は別のforeground sessionとして同じproviderとpost-canonical経路を継続利用し、exact stdin `stop` control、
  exact-value NDJSON、create-only recognition artifact、unknownを含むnumeric screen-predicate diagnosticを持つ。
  registered resourceとcandidate domainをcapture開始前にloadし、Gamescope capture loopからfield submit、inference、全song scoring、
  capture/worker/diagnosticの順序付き終了までを一つのbounded gateへ統合済み。private INFINITAS frameによる実submit、実行cost、
  queue behavior、candidate内容は未検証で、accepted resultは未実装。
  さらにcorpus録画由来の全canonical frameをsource adapterから同じapplication sessionへ供給するrecording simulationを実装した。
  profileはrecording/recording manifest/source manifest/probe/coverage label/extraction/normalizer/layout/catalog/model/runtime、全frame span、source pacing、diagnostic
  sampling、coverage labelの全resultを一対一で覆うepisode windowとexact `CLEAR TYPE`をbindする。result presenceは固定headerと2本のpanel境界で判定し、可変な背景色や
  背景絵を使わない。対象録画459 framesでは2 `FAILED`と1 `CLEAR`の3 episode、120 field observations、全song scoring、complete
  diagnosticを同じproduction worker経路で確認済み。これはaccepted result、他clear type、別背景variant、live supportまたは性能の根拠ではない。
  OBS/obs-vkcapture並行、
  soak/performanceは未検証・未着手）
- 元録画をdataset rootとして固定するFFV1 packet-order import/seal/S3-compatible再利用CLI: 完了
- M4 offline canonical/recognition spike: 着手（OBS/vkcapture実録画からnormalizer、共通result/music-select
  layout、fail-closed screen判定、result title cropのRust前処理/Paddle/公式ONNX CTC parity、
  music-selectの選択中titleおよび可視list row crop、result artistとmusic-select artist/chart/active-rowの
  versioned context crop、active catalog trie診断まで）
- 公式ONNX recognition model比較とPP-OCRv6 small native-dynamic選定: 完了
- selection song contextとlive replay telemetry storage: 着手（最小context reducer、operator確認済み
  scenario、application-owned QOI diagnostic run writer、bounded worker、strict canonical replay、read-only
  status/list control、cross-process active ownership、crash-safe aggregate retention、digest-confirmed
  freeze/delete、verified create-only local export、canonical producerからworkerへのnon-blocking live
  handoff、およびGamescopeのprofile-bound normalized frameから同じworkerへのownership transferまで。
  recognition input、target-host性能、
  accepted field認識、event daemonは未着手）
- 別machine常用プレイ診断ループ: cargo-distによるLinux x86-64 CLI archiveとSHA-256 checksumの
  local生成まで実装。private catalog/modelは別管理とし、明示的marker calibration、profile選択だけの
  通常起動、bounded local evidence、選択runだけの明示transfer、development-machine replayを続ける。
  ADR 0048によりtransform-firstとduplicate problem-report tailを外し、ADR 0049により独自bundle、
  activationおよびside-by-side rollback protocolを廃止。4K targetへの転送・校正は未着手）
- scorepeek-owned OCR学習/export: smallをartist/chart context/selection song contextと統合した後の凍結残差が
  missing OCR signalに帰属し、別経路が安全に解消できる場合だけ再検討
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
sequence modelへ入力し、CTC logitsをcatalog title trieへ直接scoreする。trieはrawのnon-search
display variantとexact comparison keyを保持し、bounded ASCII/fullwidth folded keyはcandidate domain
全体で1 songに一意な場合だけaliasとして追加する。cross-song collisionはaliasを作らず、comparison-key
IDをcandidate artifactへbindする。詳細はADR 0019に従う。

- baseline selection: 同じ3,061 stationary crop、1,119-song catalog、comparison keyおよびsong一意性で
  比較済みの公式PP-OCRv6 small native-dynamicをv1 text observerに選定する。mediumほかのartifactは
  診断・再検討証拠として保持するが、全model phase-two比較を継続しない
- model contract: 各公式modelのinput geometry、preprocessing、dictionary、timestepおよびoutput shapeを
  そのままversion固定する。同一評価基準のためにmodel固有contractを変形しない
- decoder: dictionaryやtimestepで直接表現できないsongもfull catalog titleのまま距離検索の競合domainへ
  残し、modelに合わせたtitle削除、短縮または別identityへの置換を行わない。実行不能なmodelは理由を
  matrixへ記録する
- training/export: smallをartist、chart context、selection song contextと統合した後にもmissing OCR signalが残り、
  凍結証拠上で別modelまたはcustom経路が安全に解消できる場合だけ再検討する
- runtime: 選定したpinned official ONNXをRust/ONNX Runtimeで実行する。mapped initializerは診断比較に
  限定し、選定前にcustom exportしない
- acceptance: screenごとの独立context。resultはtitle/artist/play mode/difficulty/level/notes、music selectは
  central title/artist/play mode/selected difficulty/selected level/active right-list titleを使う。二つの
  title presentationは同一selectionの整合確認であり独立票として二重計上しない
- diagnostic open-text decode: private evaluationだけ。stable eventには出さない

`CanonicalFrame`は全field recognizerが共有するRGB imageとする。titleやdigitsのROI抽出後に、
field固有のversion固定preprocessorがgrayscale、contrast、resize、paddingおよびtensor normalizationを
適用する。capture domainの補正は`DomainNormalizer`、OCR task固有の最適化はOCR preprocessorへ置き、
公式inference artifactとRust runtimeで同一preprocessing contractを使用する。custom trainingを後で採用
する場合も、そのtraining/export/runtimeで同じcontractを使用する。

training corpusは次の二系統に分ける。

- private real crop: 実game frameからresult title、music-selectの選択中titleおよび可視list rowを
  保存する。開発中はprovenance-bound active catalogからprovisional labelを作成できるが、accepted
  holdout、thresholdおよびrelease gateは人間がcatalog IDと表示文字列を確認する。model prediction
  だけによるaccepted self-labelは禁止する。右listの静止・非選択rowはthin result titleと同じglyph
  rasterizationを支持する実測があるためprovisional trainingへ利用できるが、screen originを保持し、
  result-only holdoutを置き換えない。scroll transition、選択row、UIに隠れた左右端、非title rowは
  完全title labelへ昇格せず、連続frameから静止を確認する。
- synthetic: 再配布可能なfontと許諾済み文章またはrandom character n-gramをrenderし、font、
  size、spacing、stretch、outline、shadow、glow、gradient、background、anti-alias、subpixel、
  blur、noise、truncation、4K-to-FHD downscaleを変化させる。

認識開発は、収集コストの低いstationary・non-selected・完全表示のstandard
music-list row corpusを先に使う。complete corpusで正しく一意認識できるsong数を最大化し、candidate間の
gain/loss song集合とwrong unique decisionを併記する。既認識song集合の包含は要求せず、全体coverageが
増えるcandidateを局所的なset regressionだけで棄却しない。title-disjoint splitを汎化guardとして維持する。
music-select live認識はscroll停止後の安定状態
だけを対象とし、scrolling中の認識を要求しない。最終目的とrelease gateはresult記録漏れの防止だが、
result dataを増やすための専用playは前提にしない。通常のlive sessionからresult evidenceを自動的かつ
privateに蓄積する。収集はresult detector、OCRまたはevent発行だけをtriggerにせず、見逃したresult
episodeも後から列挙できる独立recording inventoryを保持する。このinventoryはrecognition coreのstate
machineではなく、result evidenceはtitle、session、play単位で開発用transfer sentinelと、model・threshold・
candidate選定から凍結したaccepted holdoutへ分ける。
少数のscenario録画から、stable music-selection候補をresult候補との一意化にだけ使う最小song contextを
検証し、大規模な手作業timelineを開発前提にしない。起動から終了、standardの無限反復、段位の有限反復、
retry、title復帰および終了はvalidation scenarioとして保持するが、mode、attempt、play回数またはsession
進行をrecognition coreへ実装しない。通常live sessionではrecognition成功と独立にbounded local
telemetryを残し、canonical evidence、sequence/timing、transition、全binding、decision/outcome/completenessを
後からreplayできるようにする。数時間のsessionではunknownの固定長rolling tail、partial-result、screen transition、
低頻度baselineを使い、canonical QOIを認識追試へ保持する。source-to-canonical transformにも疑義が残る
partial-resultまたはtransitionの代表frameだけは、同一sequenceのexact raw BGRxとcomplete observed contractを
canonical QOIへ対でbindする。同じunknown区間ではwarm predicateが一時的に外れてもraw sourceを再保存せず、known screenへの
transitionでのみ次区間を開始する。raw sourceを連続保存したり、canonical QOIだけからtransform correctnessを主張したりしない。
value-bearing recognition artifactはresult intervalごとの代表値と低頻度music-selectへcompactし、完全なoffline gateでは
全observationを維持する。music-list改善だけでresult改善を主張せず、凍結holdoutまたはcandidate確定後のprospectiveな通常sessionで、
screen検出、song一意決定、event emission、session処理、dedupを含むcomplete result pathを最終的に
検証する。詳細はADR 0018に従う。

private developmentでは、catalog digest、source lineage/revisionおよびpermission statusを記録した
external catalog stringをprovisional training textへ利用できる。このdataとそれを含むmodelは、必要な
許諾とlicense evidenceが揃うまで配布またはrelease bundleへ昇格しない。runtime inference lexiconとしての
利用、accepted holdoutおよびthreshold calibrationは従来どおりfail-closedに扱う。
通常のtrain、validation、holdoutはtitle、session、playをまたがせず、同じcapture profile内でも
独立session/titleによるin-profile holdoutを持てるようにする。これと別に、学習からcapture profile
全体を外したprofile-disjoint evaluation suiteを持ち、未知domainへの汎化を測る。

最初のvertical spikeは同一cropについてPython/PaddleとRust/ORTのraw CTC output、token order、
catalog rankingが許容誤差内で一致することを証明する。公式baseline graphがpost-softmax
probabilityを出力する場合はそのtensorを比較し、scorepeek-owned exportではlogitsまたは
log-probabilityのどちらをbundle contractにするか明示する。ADR 0022の再開gateを満たしてcustom経路を
改めて採用した場合だけ、公式exportの代替としてPyTorch CRNN/SVTR-style CTC modelを検討する。
runtime Pythonへのfallbackは作らない。

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

capture route間にpixel correctnessの序列を置かない。Gamescope direct、後続Portalおよびregistered
custom PipeWire sourceはpeer candidateであり、それぞれのopaque capture profile、normalizer artifact、
recognition thresholdおよびsemantic replay gateを独立に持つが、game共通のcanonical layoutを共有する。
pipeline内部のlayerはruntime contractに
含めないが、再現や診断に有用な環境情報はsecret-safeなprovenanceとして保持できる。

PipeWire routeはsource acquisitionとframe receptionを分離する。source providerはdefault remoteまたは
owned remote FD、exact nodeまたはdeterministic selector、およびsourceを維持するlifetime guardを一つの
未校正leaseとして取得する。providerはselected node/remote lossとprovider shutdownを所有する。共通
receiverはstream/caps/buffer negotiation、boundedなlatest-frame受信、sequence/timing、stream lossおよび
receiver shutdownを所有する。未校正leaseを使うdiagnostic/calibration modeは`ObservedFrame`を生成しない。
独立に登録したimmutableなprofile/observed-contract/normalizer bindingが一致する新しいleaseだけを校正済み
leaseへでき、そのreceiverだけがopaque capture profileを持つ`ObservedFrame`を生成する。capsだけから
profile IDを導出しない。providerはnormalization、recognitionまたは別providerへのfallbackを行わない。

### Candidates

- Gamescope direct PipeWire: default remote上のoutput-sized nodeを取得する最初の低copy spike。
  ただしcapture repaintと通常表示のpixel equalityは仮定しない。
- Wayland ScreenCast Portal + PipeWire: session-scoped remote FDとnode IDを取得する後続provider。
- registered custom PipeWire source: 明示したsourceを取得する後続provider。未知profileはprobe診断に限定する。
- OBS/vkcapture: scorepeek sourceにはせず、OBS配信の独立した通常並行workloadとする。
- OBS WebSocket PNG: source/geometry確認用diagnosticに限定し、production backendにしない。

backendは各profileのBazzite semantic/lifecycle/performance gate後にdefaultを選ぶ。session中の
自動fallbackと異なるprofile/normalizerのframe mixingは禁止する。候補がgateに失敗してもFHD、
NV12、別color profileへ黙ってfallbackしない。

## 別machine常用プレイ診断ループ

次の実用化checkpointは、別のoperator-owned Bazzite machineを最初の利用者環境として扱う。
一般公開releaseより先に、cargo-distで通常のLinux x86-64 binary archiveとSHA-256 checksumをlocal生成し、
repository checkout、mise、RustおよびPythonをgame-session pathから外す。archiveは`scorepeek` binaryと
cargo-dist標準のrepository metadataだけを含み、`scorepeek-corpus`、catalog、model、capture binding、frame、
player dataおよびcredentialを含めない。private catalog/modelは既存manifestを持つoperator dataとして
`$XDG_DATA_HOME/scorepeek`へ別途転送する。source permissionは引き続きpublic redistributionを制限するが、
同一operator control domain内のprivate transferをsecret扱いやgeneric errorで妨げない。

target machineでは、scorepeek-ownedな既知の1920x1080 markerをexact Gamescope Wayland configで
明示的に校正し、observed BGRx contractとmarker geometryが一致した場合だけmachine-local profileを
create-onlyで発行する。これはsetup時にoperatorが要求するguided calibrationであり、通常session中の
自動測定、自動profile切替、threshold緩和またはfallbackではない。4K、FSR/NIS、HDR、Reshade、異なる
Gamescope version/configはそれぞれ別profileとする。

通常操作は`scorepeek run --profile NAME`とし、profile省略はenabled profileが一つだけの場合に限る。
scorepeekはoperatorが起動したGamescopeへattachし、INFINITASまたは通常Gamescopeを起動、signal、終了、
restartしない。scorepeekの停止はscorepeek-owned receiver、provider、field worker、diagnostic、artifact
だけを順序付きで終了する。stable event authorityの完成を待たず、現在のprovisional recognitionを通常
経路へ流して診断改善を開始するが、accepted eventまたはsupported profileとは呼ばない。

通常runのlocal recordingは既定有効、bounded、opt-out可能とし、remote送信は既定無効の明示操作とする。
保持pixelはADR 0043のfailure-window policyから増やさない。まず既存の12-frame unknown tail、
partial-resultおよびunknown-to-known transition retention、選択済みsame-sequence raw/canonical pairを
targetで使用する。別のpre-recognition tail、problem-report ledgerまたはworker watermarkは、自然なtarget runで
既存retentionが具体的な必要証拠を失った場合だけ別decisionで追加する。後からのfreezeは既に保存されたbytesの
retention priorityだけを変更し、未保存observationを復元したとは扱わない。選択runのexportだけが
release/resource/profile identity、complete/partial status、exact OCR/catalog/song/decision、既に選択済みのcanonical
QOIとraw BGRx pairをdevelopment machineへ渡す。unrelated runは含めない。

改善時はcanonical frame以降をliveと同じproduction recognition codeでreplayする。raw-to-canonical比較は、
観測済みtransform不一致、normalizer変更、または独立実装という具体的なoracleがある場合だけ追加する。修正は
報告runと既存frozen suiteを通し、replacement archiveとresourceをtargetで再確認する。threshold、
geometryまたはrecognition修正だけを目的に追加playを要求せず、prospective confirmationは次の自然な通常playで行う。

delivery checkpointは次の順序とする。

1. cargo-dist 0.32.0で`scorepeek`だけのLinux x86-64 archiveとSHA-256 checksumをlocal生成し、private
   resourceを別途転送したclean compatible Bazzite環境でrepository checkoutなしにverify/loadできることを確認する。
2. scorepeek-owned markerをguided setupへ含め、target 4K Wayland profileをauthorして独立marker replayを通す。
3. profile名だけのroutine run、status、ADR 0043の既存retention、既存bytesのfreeze/export、
   scorepeek-only ordered teardownを通す。
4. 既存retentionが選択したevidenceをexportし、development replay、修正、replacement
   archive/resource、target verifyまでを追加playなしで一巡させる。
5. 自然な常用playからsemantic、lifecycle、frame age、queue/retention、CPU/memory/game frametimeを測定し、
   exact 4K profileをdiagnostic useからsupportedへ昇格できるか判定する。

archiveのtransfer integrityはcargo-distのSHA-256 checksumで確認する。private resourceは既存manifestと必須loaderが
読む経路で検証し、同じresourceを独自deployment preflightでもう一度full readしない。host適合性は
`scorepeek doctor`、対応Bazzite条件およびcapture profile contractで確認する。各target invocationはbounded start-attempt envelopeで
source wait/acquisition、binding admissionを区別する。exact admissionがcapture generationを作った場合だけ
ADR 0025の
binding-owned Diagnostic Runを開始してattemptからlinkし、frame reception、normalization、screen inspection、
field inference、song resolution、evidence persistence、ordered shutdownをstable operation/error typeで
区別する。admission前failureを架空のcapture Diagnostic Runにしない。public recognition result、操作CLI、
out-of-band diagnostic artifactは混在させない。recording failure、capacity、queue drop、tail evictionまたは
flush timeoutは`partial | dropped` evidenceとして残すが、play、capture resultまたはrecognition resultを
変更しない。

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
6. **M3**: source acquisitionを共通PipeWire receiverから分離し、Gamescope providerだけを最初の
   未校正receiver/calibration spikeとして実装する。明示的なimmutable profile bindingを確立した後だけ
   `ObservedFrame`へ進む。BazziteでOBS/obs-vkcapture同時稼働時の実observed
   contract、lifecycle、latest-frame behavior、性能および校正corpusを確立する。Portalとregistered
   custom providerはこのspike後へ延期し、M4 bootstrapのlayoutは移動しない。
7. **M4 completion**: peer profileごとのnormalizerとshared alignmentを検証し、選定済みPP-OCRv6 smallの
   Rust parity gateを完了する。custom fine-tune/exportは統合context後のmissing OCR signalが実測された場合だけ追加する。
8. **M5**: gateを通ったsupported PipeWire profileのlifecycle/performanceを比較し、defaultを選ぶ。
9. **M6**: screen、savable、title/artist/chart contextのfield recognizer、full-catalog screen-local song
   resolver、digits、cross-field validationの順で追加する。field recognizerとcandidate evidenceをlive
   captureへ接続する最初のgateは、1つのimmutable run bindingとbounded worker lifetimeを所有する。自動化向けの
   compact resultはtyped status、screen/worker/candidate件数、diagnostic completenessおよびartifact identityに
   限定できるが、operator-owned local recognition artifactはboundedなexact OCR文字列、run単位のexact catalog
   display/comparison string table、song ID、string reference、全candidate metric、判断と理由、expected-versus-observed値を保持する。pixelは既存のbounded image artifactをidentityで
   参照する。result-song ranking/acceptanceはADR 0038、screen-local music-select song resolutionは
   ADR 0046で追加済みとし、chart、stable-selection temporal stateおよびevent authorityは後続の独立contractとする。
10. **M7**: 少数scenario replayから最小selection song contextとrecognition-trigger非依存のbounded
    live diagnosticsを検証し、その後versioned event schemaとNDJSON daemonを統合する。ゲーム全体の
    state machine、attempt、modeまたはretry回数は実装しない。
11. **M8**: catalog update replay、full private holdout、Bazzite live flowをrelease gateへ統合する。

M3/M4からM7へ進む間は、ADR 0049で通常のRust CLI配布へ置き換えたcross-machine delivery checkpointを縦に通す。event authorityや
public releaseを待ってからtarget deploymentを始めず、現在のcapture/recognition/diagnostic coreを
cargo-dist archiveと別管理のprivate resourceで常用し、得られた通常runをM4/M6/M7の改善入力にする。

新規runtime、training、parser、capture dependencyは、version、license、代替案、distribution/host影響を
一括提示して承認を得た後にだけ追加する。

最初のPipeWire build bootstrapはLinux x86-64を対象に、safe `pipewire` 0.10 seriesを使う。
miseはchecksum固定したlibpipewire/libspa 1.6.8 SDKとnative pkgconf 3.0.1 executableを供給し、
Pythonを実行しない。通常のCargo edit/check/test loopはhost nativeのまま維持し、hostには`cc`、
shared libclangと同majorのClang resource headers、および実行時PipeWire libraryだけを要求する。
Zig、Podman、Distrobox、個人用distrobox imageおよびhost `pkg-config`はbuild prerequisiteにしない。
このbootstrapの成功はGamescope capture profileのsupport evidenceには数えない。

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
- model bytesを固定し、scorepeek側のadaptation、decoder/model選定evidenceおよびactive catalogから
  対象songを外した状態でcandidateを確定する。その後catalog recordだけを追加し、対象のheld-out real
  cropを認識するmodel-independent new-song simulation。公式modelのupstream training corpusに対象titleが
  含まれなかったことは要求しない
- accepted result/music-selectの必須field誤り0
- negativeからのevent 0、ambiguous candidateの推測0
- stable result identity acceptance 90%以上
- stable music-select identity acceptance 80%以上
- episodeごとにresult event 1件、music-select identity重複なし
- disconnect、generation変更、stall、screen exitでcandidate reset

### Bazzite capture

最初にGamescope directを15分x3回と30分soakで検証する。Portalまたはregistered custom sourceを
後からsupported candidateへ追加する場合は、同じgateを各routeで独立に実行する。

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
