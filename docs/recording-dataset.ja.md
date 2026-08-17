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

成功時の`recording_sha256`が録画byte identityである。同じ録画とcontextの再importは
idempotentなので、成功したか不明な場合は同じコマンドを再実行できる。初回importは外側のsource
SHA-256を確定し、private stagingへcopyしながら同じSHA-256を再確認したうえで、そのstaging snapshot
だけを全decoded frameのPTS probeへ渡す。stream contractもcapture profileと一致した場合だけsource
bindingを公開する。したがって公開されるPTSは必ずhash-verified staging sourceのものであり、外側の
pathname変更によってcopyのSHA-256または観測contractが食い違えばfail closedになる。完全なrecording
bundleが同じsource SHA-256とcontextですでに存在する場合、
再importは入力と保存済みobjectの全byte hashおよびtyped bindingを検証し、保存済みprobeを再利用する。
同じ録画を再びFFprobeへ通したり、storeへ再copyしたりしない。importはlocalだけで完結し、network
uploadは行わない。元ファイルはstore内へbyte-identicalにコピーされるため、import成功後は外側の
作業コピーを別途残す必要はない。ただしS3へのpushとremote verificationが終わるまでは削除しない
運用を推奨する。

1本の録画にすべての曲や状態を詰め込む必要はない。録画を追加する条件は、未収録のscreen、
transition、style、difficulty、失敗・中断経路が必要になったとき、またはcapture条件を変えた
ときである。normalizerやOCR実装を更新しただけなら再録画しない。

最初のcalibration setでは、PortalとGamescope directの各profileで、できるだけ同じgame設定と
比較可能なplay sequenceを1本ずつ収録する。これは片方をpixel baselineにするためではなく、
複数domainに共通するcanonical geometryを後から定義するための対応する観測を確保するためである。
各録画には少なくとも起動・終了、安定したmusic-select、選曲やdifficulty/style変更、play、
result、各画面間のtransition、recognition対象外のloadingまたはnegative stateを含める。
clear/failやSP/DPなど、一度のplayで両立しない状態は別録画として追記してよい。

capture routeのsupport gateで必要な反復やsoak recordingは、同じimport経路で追加generationへ
含める。ただし最初から全gate分を撮り切る必要はない。まず各peer profileのobserved contractを
1本ずつ確立し、既存録画の解析で不足が判明した時点で対象を絞って追加する。

## generationを固定して保存する

必要な録画をimportしたら、その時点でstoreにある全録画をsealする。

```text
mise run corpus:dataset:seal -- --store /absolute/private/store calibration-001
```

JSON出力の`generation_sha256`を保存する。続いてlocal byteを全hash検証する。

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

local storeとremote generationは、元録画だけでなくsource manifest、capture profile、media probe、
recording manifestをそれぞれcanonical schemaとして再parseし、録画identity、source size、profile、
probeの相互参照まで検証する。local storeはsource、各document class、dataset generationごとに
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
