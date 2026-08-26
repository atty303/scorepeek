# 録画datasetの運用

このworkflowでは、ゲーム起動前から終了後までの元録画を長期保存の単位にする。
frame抽出、canonical frame、layout、domain normalizer、label、OCR model、replay indexは
すべて派生物であり、scorepeekの実装が変わったら保存済み録画から作り直す。

## 一度だけ用意するもの

private corpus storeは追記型で使う。新しい録画を同じstoreへimportし、節目ごとにsealすると、
古い録画を含む新しいimmutable generationができる。generationの名前は人間向けであり、
再利用時に指定するidentityはコマンド出力の`generation_sha256`である。

capture条件ごとにcapture-context JSONを作る。これはWindows VMや特定の画像をbaselineとして
指定するものではない。同じroute、adapter実装、adapter version、録画設定を再利用している間は
同じ文書を使い、いずれかを意図的に変えたときだけ新しい文書を作る。

```json
{
  "schema": "scorepeek-capture-context-v1",
  "route": "portal_pipewire",
  "environment_id": "bazzite-handheld-2026-08",
  "capture_adapter_id": "portal-pipewire",
  "capture_adapter_version": "v1",
  "settings_revision": "matroska-lossless-1080p-v1"
}
```

`route`は`portal_pipewire`、`gamescope_direct_pipewire`、`obs_vkcapture`のいずれか。
その他の値は秘密情報や個人情報を含めない短いversioned identifierにする。importerはこの文書と、
録画から実測したcontainer、codec、pixel format、dimensions、time base、color metadataを結合して
capture-profile digestを決める。したがって、あるprofileを基準に別profileを定義する必要はない。

S3-compatible remoteはbucket、prefix、region、endpoint、addressing styleと、test専用の
loopback HTTP許可だけをJSONに置く。
AWS S3では`endpoint`を`null`、`path_style`を`false`にする。custom endpointではpath、userinfo、
query、fragmentを含まないHTTPS originを指定し、providerがpath-style addressingを必要とする
場合だけ`true`にする。通常は
`allow_http_loopback`を`false`にする。

```json
{
  "schema": "scorepeek-corpus-s3-remote-v1",
  "url": "s3://private-bucket/scorepeek-corpus",
  "region": "ap-northeast-1",
  "endpoint": null,
  "path_style": false,
  "allow_http_loopback": false
}
```

repositoryのfocused E2Eだけは、miseで固定した`rclone serve s3`と
`http://127.0.0.1:<port>`を使い、`allow_http_loopback`を`true`にする。この例外は厳密な
loopback IPv4/IPv6 endpointにだけ適用され、`localhost`やremote HTTP endpointは拒否される。
本番remoteのTLS要件を緩める設定ではない。

credentialはremote JSONやCLI引数へ書かず、実行環境から渡す。短期credentialまたはworkload
identityを優先する。固定credentialを使う必要がある場合も、少なくとも
`AWS_ACCESS_KEY_ID`、`AWS_SECRET_ACCESS_KEY`、必要なら`AWS_SESSION_TOKEN`としてprocess環境へ
渡し、ファイルをrepositoryへcommitしない。bucketはprivateにし、未完了multipart uploadを
期限後に削除するlifecycle ruleを設定する。

## 1回の収録

1. 固定したcapture条件で録画を開始する。
2. ゲームを起動する。
3. 必要な画面と遷移を含む一連のplayを行う。
4. ゲームを正常終了する。
5. 録画を停止し、self-contained Matroskaファイルを確定する。
6. 元録画を編集、trim、transcodeせずimporterへ渡す。

```text
mise run corpus:recording:import -- --store /absolute/private/store --capture-context /absolute/private/capture-context.json /absolute/recordings/complete-run.mkv
```

すでにdurableなlocal fileとして保持しており、storeへ大容量copyを作りたくない場合は
`--external`を指定する。

```text
mise run corpus:recording:import -- --store /absolute/private/store --capture-context /absolute/private/capture-context.json --external /absolute/recordings/complete-run.mkv
```

このmodeはsource SHA-256とbytesをgeneration identityとして維持し、絶対pathだけを
local locatorへ保存する。pathはgeneration、manifest、remote objectへ入らない。初回import、extractなど
source内容を読むoperationは、必要な処理と同じopen handleで一度full hashする。sealとreplay index生成は
選択済みmanifest、locator、size、必要なtyped referenceだけを確認し、sourceを再hashしない。明示的なverifyと
remote側へ渡ったbytesは各境界でcomplete verificationを行う。
fileが移動した場合は、同じbytesを新pathから再importするとlocatorだけを更新し、dataset identityは
変わらない。外部fileはregular fileでなければならない。permission、ownership、ACL、read access、
durabilityはoperatorが管理し、scorepeekはmodeを受理条件または出力保証にしない。

成功時の`recording_sha256`が録画byte identityである。同じ録画とcontextの再importは
idempotentなので、成功したか不明な場合は同じコマンドを再実行できる。copy modeの初回importは外側の
source SHA-256を一度確定し、その同じ選択fileをprivate stagingへcopyして、staging snapshotだけを
FFV1 video packet-orderのPTS probeへ渡す。copy中のbytesは同時変更によって誤った内容を正しいdigest名で
content-addressed storeへ公開しないため、最初に確定したdigestと照合する。これはstore publication境界であり、
probe後に同じsnapshotを再hashする処理ではない。`--external`の初回importは一つのopen file
handleを一度full hashしてdeclared source identityを確認してから、その同じhandleでcontractとpacket-orderを
観測する。concurrent writerを含まない通常operationでは、観測後に同じlocal handleをもう一度full hashしない。
明示的なlocal verifyとremote transferは、それぞれの境界でcomplete bytesを検証する。
成功後にだけcanonical pathとsource SHA-256をprivate locatorへbindingするため、invalid mediaは
孤立locatorを残さない。どちらもstream contractがcapture profileと一致した場合だけsource bindingを
公開する。完全なrecording bundleが同じsource SHA-256とcontextですでに存在する場合、
再importは選択入力を一度hashしてidentityを定め、参照objectの存在とsize、capture profileの
digest/schema/contextを確認して保存済みprobeを再利用する。全objectのbyte監査は明示的な
`dataset verify`に限定する。
同じ録画を再びFFprobeへ通したり、storeへ再copyしたりしない。importはlocalだけで完結し、network
uploadは行わない。copy modeではstore内の`source.media`、`--external`ではhash-bound locatorがsourceを
解決する。後者では外側の録画fileそのものがdataset rootなので削除しない。

高速probeはself-contained Matroska内の単一FFV1 video streamだけを受理し、他codecへ自動fallback
しない。frame抽出時には選択した各decode indexの実decoded PTSをFFmpegから取得し、packet-order
probeと一致したframeだけを公開する。完全な全byte監査が必要なときは`dataset verify`を明示的に使う。

1本の録画にすべての曲や状態を詰め込む必要はない。録画を追加する条件は、未収録のscreen、
transition、style、difficulty、失敗・中断経路が必要になったとき、またはcapture条件を変えた
ときである。normalizerやOCR実装を更新しただけなら再録画しない。

最初のcalibration setは、利用可能なlossless profileをversioned normalizerで固定済み
`CanonicalFrame`へ変換し、その出力から共通layoutとrecognition spikeを開始できる。このprofileは
pixel correctness referenceやdefaultにはならない。Portal、Gamescopeまたは別OBS profileを後から
追加するときは、それぞれを同じcanonical contract/layoutへ写すnormalizerを校正する。recognizerが
raw `ObservedFrame`を直接受け取る経路は作らない。各録画には少なくとも起動・終了、安定した
music-select、選曲やdifficulty/style変更、play、result、transition、loadingまたはnegative stateを
含める。clear/failやSP/DPなど、一度のplayで両立しない状態は必要になった時点で追記してよい。

capture routeのsupport gateで必要な反復やsoak recordingは、同じimport経路で追加generationへ
含める。ただし最初から全gate分を撮り切る必要はない。まず各peer profileのobserved contractを
1本ずつ確立し、既存録画の解析で不足が判明した時点で対象を絞って追加する。

### 所有曲listを1回で収録する場合

全曲title用のmusic-select録画では、連続高速scrollだけにしない。一定のsort/filterと先頭位置を
記録したうえで、1回に8–10 row進め、入力を離して0.5秒以上停止する操作を末尾まで繰り返す。
前後の画面にrowを重複させることで、ある画面で選択状態、左右clip、overlay重なりになった曲も、
別画面では非選択の完全rowとして観測できる。先頭と末尾では2秒程度停止し、wrapせず終了する。

scroll中のframeも削除しないが、negative transitionとして扱う。positive provisional labelは、隣接
frameから垂直移動が止まったことを確認できる非選択rowにだけ付ける。中央の大型selected title、
選択中の右row、separator、UIに隠れた長いtitle、画面端の部分rowには完全titleを割り当てない。
現在は実scroll sequenceから安定度分布をまだ測っていないため、録画後にthresholdを固定するまで
自動採用しない。この収録はresult-only holdoutを代替せず、result画面は得られる範囲で別途保持する。

row annotationのdraftは`scorepeek-private-music-list-row-observation-draft-v2`として保存し、次の
入口でshapeを検査する。

```text
mise run corpus:music-list:observation-draft:inspect -- /absolute/private/music-list-observation-draft.json
```

`stationary`と`scrolling`のdraftは、可用性を`available`または`locked_dimmed`、文字色domainを
`standard`、`infinitas_blue`または`leggendaria_purple`として移動状態とは独立に記録する。選択した
未解禁rowの直下に挿入される解禁条件barは`non_title: unlock_condition`であり、前後の曲名を
割り当てない。同じcanonical extraction内の隣接decode index、両cropの
file/pixel SHA-256、申告RGB L1差分合計、および比較したRGB値数を必須とする。ただしこの入口は
artifactを読まず、L1を再計算せず、常に`evidence_verified: false`を返す。draftを校正入力として
使ってはならない。明示的な次のverify入口は、両manifest、canonical frame、crop bytesを読み、cropが
canonical frameの固定ROIと一致することとRGB L1を再計算する。これは通常の下流処理に暗黙で重ねる
検証ではなく、operatorが完全な再検証を明示的に要求するための入口である。

```text
mise run corpus:music-list:observation-draft:verify -- /absolute/private/music-list-observation-draft.json
```

commandの`evidence_verified: true`はこの明示的な完全再検証が成功したことを示す。通常の下流処理は
このcommandを自動実行しない。1 frame/slotへの相反annotationは拒否し、`selected`、
`clipped`、`non_title`、`unknown`は完全titleを持てない。

全20 rowを一つの隣接frame pairとして測る場合は、各frameに20件ちょうどのsemantic annotationと、
pair全体に`stationary`、`scrolling`または理由付き`unknown`を付けた
`scorepeek-private-music-list-motion-request-v1`を用意する。測定値からmotion正解を自動推定せず、次の
入口で選択digestと必要なshape/referenceを確認し、各rowと合計のRGB L1を生成する。測定に必要な
pixel readは行うが、同じbytesを別の完全性審査として重ねて読まない。

```text
mise run corpus:music-list:motion:measure -- --output /absolute/private/music-list-motion-artifact.json /absolute/private/music-list-motion-request.json
mise run corpus:music-list:motion:verify -- /absolute/private/music-list-motion-artifact.json
mise run corpus:music-list:motion:review-plan -- --output /absolute/private/music-list-motion-review-plan.json /absolute/private/music-list-motion-artifact.json
mise run corpus:music-list:motion:review-apply -- --output /absolute/private/reviewed-motion-request.json /absolute/private/music-list-motion-artifact.json /absolute/private/music-list-motion-review-plan.json /absolute/private/music-list-motion-review-decisions.json
```

出力は`scorepeek-private-music-list-motion-artifact-v1`であり、既存fileを置換しない。`unknown` pairは
分布観測には残せるがthresholdの正解集合へ使わない。両frameのlocked/dimmed、INFINITAS-blue、
LEGGENDARIA-purple、selected、clipped、separator、unlock-conditionも個別に保持し、標準titleへ暗黙変換しない。
`motion:measure`は各pairで比較に必要な20 row cropだけを一度読み、declared digestとP6 shapeを確認しながら
L1を計算する。`review-plan`は選択したartifactとscorepeek crop manifestをdigest/schema/referenceで受け取り、
crop pixelsやcanonical frameを再読込せず、全row occurrenceを残したままdeclared pixel SHA-256が完全一致するcropだけを
一つの目視単位へまとめる。色、明るさ、OCR結果またはmotion測定値からannotationを生成しない。成功summaryの
`source_artifact_bound: true`は選択artifactへ構造的にbindしたことを示し、完全再検証を意味しない。
`review-apply`は選択したplan SHA-256へbindしたcanonicalな
`scorepeek-private-music-list-motion-review-decisions-v1`だけを受け入れる。各decisionは
`crop_pixel_sha256`とunknown以外の`annotation`を一つ持つ。未確定groupはdecisionから省略し、出力requestでは
元のannotation（通常は理由付きunknown）のまま残す。decisionの重複、planにないhash、改変されたartifact/plan、
既存outputは拒否する。apply時はartifact/planのdigest、schema、pair/frame/slot/current annotationと
occurrence全体の対応を確認するが、crop pixelsの再読込やreview planの再構築は行わない。成功summaryの
`source_artifact_bound: true`も同じ意味である。出力requestを再度`motion measure`へ渡すことで、部分レビュー済みartifactを作成できる。
`motion:verify`はoperatorが要求したときだけcanonical frame、crop、全測定を再計算する明示的な監査入口であり、
`review-plan`または`review-apply`から自動実行しない。

## generationを固定して保存する

必要な録画をimportしたら、その時点でstoreにある全録画をsealする。

```text
mise run corpus:dataset:seal -- --store /absolute/private/store calibration-001
```

sealは選択済みrecording manifestと参照objectの存在、size、必要なtyped bindingを固定し、sourceや各documentを
全hashし直さない。JSON出力の`generation_sha256`を保存する。完全なlocal byte監査が必要なら続けて明示的に実行する。

```text
mise run corpus:dataset:verify -- --store /absolute/private/store GENERATION_SHA256
```

remoteへ明示的にpushする。content-addressed objectは同じSHA-256なら再uploadせず、既存objectも
全byteをdownloadしてhashが一致した場合だけ再利用する。録画objectを先に送り、最後にgeneration
manifestを送る。

```text
mise run corpus:dataset:push -- --store /absolute/private/store --remote /absolute/private/remote.json GENERATION_SHA256
```

push後または定期監査では、remoteだけを全byte検証する。

```text
mise run corpus:dataset:remote-verify -- --store /absolute/private/store --remote /absolute/private/remote.json GENERATION_SHA256
```

remoteにはmutableな`latest`やdelete CLIを作らない。開発記録、model export record、replay suiteは
使用した`generation_sha256`を明記する。

pushするobjectは選択generationに記録されたpathとsizeからuniqueなscorepeek-owned remote staging
keyへuploadし、remote stagingの全byte hashがdeclared digestと一致することを検証してから、content-addressed
final keyへ条件付きでserver-side publishする。local objectの完全な事前検証やupload後の再full-readは行わない。
final keyが競合した場合は
既存remote objectの全byteを再検証する。成功・失敗のどちらでもowned stagingを削除し、cleanupに
失敗した操作は成功として扱わない。

明示的なlocal/remote verifyとpullしたremote generationは、元録画だけでなくsource manifest、capture
profile、media probe、recording manifestをそれぞれcanonical schemaとしてparseし、録画identity、source
size、profile、probeの相互参照まで検証する。通常のsealとpushはこの完全再検証を重ねない。local storeは
source、各document class、dataset generationごとに
object数とaggregate bytesを制限し、pullは追加量全体をwriter lock下で事前検査する。
typed documentのrole別上限はremote GETより前に検査する。download途中のbytesはunlink済みprivate
temporary fileへ流し、publication途中でcrashして残ったscorepeek-owned stagingは次回のwriter lock
取得時に回収してからcapacityを再計算する。

## 将来の開発で再利用する

別hostや空のlocal storeで、必要なgenerationをdigest指定で復元する。

```text
mise run corpus:dataset:pull -- --store /absolute/private/restored-store --remote /absolute/private/remote.json GENERATION_SHA256
```

pullはgenerationと全objectのsize、SHA-256、manifest bindingを検証してからprivate storeへ
publishする。その後、対象generationを入力として新しいframe selection、canonical conversion、
layout measurement、normalizer calibration、label、training/export、replay artifactを生成する。
これらの派生物は元録画generationを置き換えず、新しいartifact digestから参照する。

再録画が必要なのは、保存済み録画に必要な観測が存在しない場合か、新しいcapture条件そのものを
評価する場合だけである。ゲーム共通layoutの変更やnormalizer/OCRの更新は、まず既存generationを
replayして評価する。

現在のexact FFV1/yuv420p/limited-range BT.709 profileは、capture-profile digest、pinned FFmpeg
binary digest、1920x1080、time base 1/1000、range/space/transfer/primariesを一つのregistry entryとして
固定したversioned normalizerでcanonical RGB8へ変換できる。codecや解像度だけが似た別profileは
このentryを再利用できない。

```text
mise run corpus:canonical:extract -- --store /absolute/private/store --output /absolute/private/canonical-frames /absolute/private/probe.json /absolute/private/extract-request.json
mise run recognition:inspect -- --extraction /absolute/private/canonical-frames --extraction-sha256 FRAME_EXTRACTION_SHA256 --frame-id FRAME_ID
```

出力の`normalizer.json`、`manifest.json`、RGB8 PPMは同じartifact digestへbindingされる。未校正の
pixel format、geometryまたはcolor contractは自動fallbackせず拒否する。recognition CLIはこれらの
binding、canonical extraction時に返された期待SHA、選択frameのfile/pixel hashを検証してからだけ
`CanonicalFrame`を構築し、bare PPMやobserved frame抽出を直接受理しない。
