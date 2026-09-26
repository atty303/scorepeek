# 実装と視覚検証

**責務:** 汎用skin制作の実装接続先、標準preview、native/browser検証、
レビュー提示matrixと合否手順を定める。skin固有の結果・素材・最終寸法、
旧作との一回限りの比較結果は各skin仕様または当該taskの証拠が所有する。

## 現行の接続先

repository rootから次を読む。行番号ではなく型・関数を確認し、将来の移動を追う。

| 責務 | 原典 |
| --- | --- |
| ZIP manifest・theme・package resource | `skins/<name>/skin.toml`、`theme.css`、`resources/` |
| skin Wasm実装 | `skins/<name>/src/lib.rs` |
| authoring基準・scene・生成処理 | `.agents/skills/create-overlay-skin/` |
| Rust ABI型・buffer helper | `crates/scorepeek-skin-sdk/src/lib.rs` |
| package検証・native Wasmtime/DOM | `crates/scorepeek-overlay-runtime/src/skin/package.rs`、`crates/scorepeek-overlay-wayland/src/skin/runtime.rs`、`crates/scorepeek-overlay-wayland/src/render/dioxus_dom/skin_session.rs` |
| OBS Web Worker/browser DOM | `crates/scorepeek-overlay-web-host/src/bundle/skin_browser.js`、`crates/scorepeek-overlay-web-host/src/http/skin.rs` |
| package build/install | `scripts/build-skins.sh`、`scripts/with-isolated-skins.sh` |
| versioned authoring contract | `docs/skin-plugin-api-v2.md` |
| 設定schemaとmanifest property | `crates/scorepeek-overlay-runtime/src/config/layout.rs`、`crates/scorepeek-overlay/src/skin/manifest.rs` |
| 採用conceptと素材の由来 | `skins/<name>/SPEC.md` |

skin ID、表示名、release、propertyはmanifestが所有し、editorへ静的enumを追加しない。各skinは
独立したWasm crateとし、素材と描画コードはskin自身が所有する。
独自treeもv2 ABIに従う。`skins/<name>/skin.toml` を追加すると `scripts/build-skins.sh` が検出する。
画像・font・licenseは
ZIP内へ入れ、通常runtimeのembedded assetへ登録しない。widget resizeで枠厚や文字が一緒に
伸びないこと、canvas cropと意図したoverflowを両hostで確認する。
`preview.png`と`preview.webm`はskill所有の`preview-scene.json`と
`.agents/skills/create-overlay-skin/scripts/generate-skin-previews.bash`からproduction browser経路で生成する。
採用concept、比較sheet、DOMの手動書換えまたは別実装による再現画像をpackageへ入れない。生成後は
PNGをnativeとbrowserのeditorで選択して表示する。WebMは生成したファイルをbrowserで再生してmotionとloopを確認する。現行editorのskin選択UIはPNGを表示し、`preview_video`を再生しない。
同じ生成runでresource request、Wasm init/render、期待DOM、media metadataと成果物hashを記録し、欠落resourceや不完全な描画を
成功成果物として残さない。

素材には生成元/制作条件・再生成方法・必要なlicenseを残す。独自生成素材とOFL fontを用い、
ゲーム画像、upstream資産、実playerデータはrepositoryの包含許可なしにcommitしない。
新しいdependencyはrepositoryの承認規則に従う。通常起動で画像/fontをdownloadしない。

## 検証を実行する

厳密なコマンドとscenario schemaは `docs/overlay-visual-debugging.md` が原典。
以下のpathは例。実行前にagent所有の新しいpath/未使用portへ置き換える。

```sh
mise run overlay:visual:native -- crates/scorepeek-overlay/tests/fixtures/visual-debug.json /tmp/skin-native-new
```

必ず**全PNG、対応するselector-layout JSON、manifestのcomplete/status**を確認する。
初期の`scorepeek_init`だけで全widgetが描かれることも、最初のPNGで確認する。
contact sheetで全体を走査しても、文字/baseline/frameの疑わしい箇所は原寸で開く。
layoutの正のrectangleだけではnative paintを証明できない。
`visual-composition.json` も使用し、各skin IDと明示motion時刻のvariantを新しい一時scenarioとして作る。
sourceのscenario型が許す項目だけを使う。fixtureに未対応のstate注入をあるものとして実行しない。

```sh
mise run --raw overlay:visual:obs -- /tmp/skin-browser-new/overlay.toml 127.0.0.1:17384
```

- configは未存在であること。stdinを保持する対話terminalで起動し、HTTP応答後に開く。Enterで終了するserverなのでstdin EOFを起動失敗と誤認しない。
- `mise run browser:cli -- open <url>` で `/overlay` を開き、`resize` で原則1920×1080（指定があればそのlogical size）にする。利用者が別のブラウザを明示した場合はその指定を使う。
- `snapshot` と `run-code` でtop-levelとcanvas iframeのDOMを両方読む。`click <ref> right` または `mousedown right` で編集に入り、対象skin・screen/canvasを選ぶ。`screenshot --filename=<path>` でcomposed screenshotを取得して実際に見る。
- widget選択、移動、四隅resize、manifestで宣言したproperty、EMPTY title有無・aspect、scroll、save/reopen、別変更のdiscardを試す。保存先は一時configだけ。
- 狙ったskinが全canvasへ反映されたか確認する。一つのcanvasの変更だけで全画面のskin検証済みとしない。
- 時刻の異なる表示を取得し、動きと安定した実値を確認する。CSS animationは明示motion時刻で、Wasmのscheduled full-tree更新はrender呼出しを伴うcaptureで確認する。v2 ABIの `backend` と `monotonic_ms` を使う最適化とmotionはskinが所有する。nativeとbrowserで同じ内容/サイズを比較し、pixel equalityは求めない。
- 終了後 `mise run browser:cli -- close` でsessionを閉じ、serverを止め、一時config/scenario/outputをcleanupする。比較証拠を残すなら保持先を明示する。

`overlay:visual:obs` はbundle依存を持つ。backendだけを古いbundleと組み合わせない。
通常のcargo build/testもbundleを埋め込み、bundleの欠落またはbuild identity不一致はbuild時に拒否する。
実際に配布用binaryを作る依頼なら `mise run release:build -- VERSION OUTPUT_DIRECTORY` のbuild手順と成果物を確認する。

## レビュー提示用のbrowser動画

skin制作中に利用者へ実装のレビューを求めるたびに、production skinと合成stateをbrowserで動画にする。
候補選択用のdesign画像とpackageの`preview.png`は別の成果物であり、レビュー一覧の作成・修正には使わない。
レビュー制作中は`preview.png`を現状のまま維持する。agentは同じ内容、サイズ、状態とmotion時刻をnativeでも検証する。

1. 提示物は**1920×1440（4:3）の一画面を連続描画したbrowser動画**とする。この中に`status`、`selection`、`score`、`history-list`、`history-graph`、`empty`とcanvas背景を配置する。`selection`と`score`は状態別の複数実例を同じ画面に配置し、残りのwidgetは読み取れる大きさで1例ずつ置く。`empty`を大きな空白領域にせず、他のwidgetと釣り合う小さな開口部にする。skin ID、case ID、logical size、motion時刻は動画に隣接するcase表にも明記する。
2. 次の8 variantを作る。各行の`selection`と`score`を1組として動画の全フレームに含め、SP/DP、5難易度、DJ LEVELのA/AA/AAAとMAX-差分、8 clear typeを組み合わせて網羅する。notesとSCOREには3桁の例を含める。共通statusにはinactiveとerrorのランプを表示する。clearとDJ LEVELは別軸なので、ランクとclearの組合せから結果を推定しない。

   Skillの `.agents/skills/create-overlay-skin/scripts/generate-skin-review-scenes.ts` は、このmatrixの合成stateと最終画面上のwidget配置を新しい出力directoryへ生成する。`deno run --allow-write .agents/skills/create-overlay-skin/scripts/generate-skin-review-scenes.ts <skin-id> <new-output-dir> <motion-periods-ms> <paint-padding-px>` を実行する。motion引数は採用skinの全ループ周期を整数ミリ秒のカンマ区切りで指定し、動きがないskinでは空文字列にする。最後のpaddingは枠外に実際に描く光や装飾の最大範囲に合わせた任意の非負整数で、省略時は16 pxから検証を始める値とする。16 pxは表現上の上限ではない。生成された`cases.json` と `01.json`–`08.json` をnative harnessへ渡す。native出力の親directoryは先に作り、8つのcase IDを子directory名にする。

   | ID | Play | Difficulty | DJ LEVEL | Clear type |
   | --- | --- | --- | --- | --- |
   | 01 | SP | BEGINNER | A | NO PLAY |
   | 02 | DP | NORMAL | AAA | FAILED |
   | 03 | SP | HYPER | AA | ASSIST |
   | 04 | DP | ANOTHER | AAA | EASY |
   | 05 | SP | LEGGENDARIA | AA | CLEAR |
   | 06 | DP | BEGINNER | AAA (MAX-) | HARD |
   | 07 | SP | NORMAL | AAA | EX HARD |
   | 08 | DP | HYPER | AA | FULL COMBO |

3. 各variantの曲名・artist・譜面level・notesに加え、score widget内のSCORE、MISS COUNT、判定数、FAST/SLOW、COMBO BREAKと、履歴・graphの数値を変える。固定の代表値を複写しない。SCORE、notes、DJ LEVEL、達成率、しきい値差分は各合成state内で整合させる。BESTとRESULT DETAILで異なる値を示すvariant、0・不明・長い文字列など共通matrixの境界条件も追加する。補助variantを増やしても上記8行を省かない。
4. `bash .agents/skills/create-overlay-skin/scripts/verify-skin-review-browser.bash <native-scene-dir> <skin-slug> <new-browser-output-dir>` で全8状態をproduction browserに描画し、同一motion時刻のwidget画像を合成して1920×1440の動画を生成する。共通widgetと背景はcase `01`、各selection/scoreはそのcaseのwidgetだけを透過で切り出して等倍で重ねる。通常は他の要素を非表示にして対象だけをcaptureし、対象を隠した際に余白が透明になることを検査する。要素または擬似要素の`mix-blend-mode`や`backdrop-filter`には、元の背景を保った描画と対象を隠した描画の差分から対象の領域を取り出す。この合成経路では全widgetを隠したcanvas背景がcase `01`と同じmotion時刻に一致することを各frameで検査し、異なる場合は動画を作らず失敗する。背景が透明なら対象のalphaを保持し、不透明なら完成画素を重ねる。半透明の背景からは対象を一意に復元できないため失敗する。文字・素材・値は描き直さない。`empty`は300×80の実寸に抑える。検証toolはsceneのwidget座標と実DOMの矩形から描画領域を特定する。skin固有のclass名や`data-*`属性を要求しない。
5. 各variantをnativeで独立してcaptureし、開始、主な変化、中間、終端、申告した各周期のloopの継ぎ目を含む代表時刻のlayout JSON、manifest、実画像を確認する。browser動画の同時刻のframeと見比べ、意味上の動きと見え方が両hostで成立することを目視判定する。video scriptはbrowserの仮想時計を使い、native harnessは同じ0秒の単調時刻基準からWasm入力を進める。CSS時刻は両方とも動画先頭を0とする。pixel一致は要求しない。nativeで欠落・破綻したmotionをbrowserだけで合格にしない。
6. 提示前に `deno run --allow-read .agents/skills/create-overlay-skin/scripts/check-skin-review-scenes.ts <scene-dir> <native-output-root> <browser-review-video>` を実行する。widget種別、SP/DP、5難易度、A/AA/AAAとMAX-、8 clear type、3桁のnotes/SCORE、inactive/error、各数値列の変化、score/rank計算の整合、各native manifestの完了と全widget境界に対応するDOM矩形、動画の4:3寸法とdurationを検査する。提示時は**この動画**とcase表、検証したsize・motion時刻、残る制限をセットにする。原寸frameも目視評価する。

## 共通matrixと合否

同じmatrixを全ての新skinへ適用する。全組合せの直積は不要だが各行と意味のある重なりを両経路で確認する。

| 条件 | 合格oracle |
| --- | --- |
| 全体/詳細と採用concept | 採用した世界観と素材の方向が、同じ内容・幅でも成立し、輪郭、素材の縁/反射、文字階層、密度が仕上がっている |
| 長い和英混在title・artist・option | 共通の収め方を維持し、文字化け/無断略称/ラベルと値の重なりなし |
| 数値0/1/999/1000など桁変化 | 桁/label baselineが安定し、atlasの透明余白で列が浮かない |
| SP/DP・全難易度・level1/12 | 難易度色がスキン背景から分離し、種別と数値を独立に読める |
| 全clear・A/AA/AAAとB以下 | AAA/FCの特別感が独立し、unknownに達成処理を付けない |
| AAA+FAILED、AA+FC、A+EX HARD | 評価軸を混同せず、同時motionがcontentを圧倒しない |
| 判定、FAST/SLOW、miss0、不明 | 原則どおりの強弱、対称性、中立表示。0とunknownを区別 |
| 履歴・graph、欠損値 | 列/軸/単位/順序を維持し、欠損を架空の線で結ばない。`history_count` の5/10/20/50行と `graph_months` の1/3/6/12か月を、設定に合う高さと期間で別途確認する |
| 小widget・横長/縦長・host geometry | 固定cornerと枠厚、内側寸法、resize handle、cropを保つ |
| EMPTY、title有無、aspect | cut cornerが矩形化せず、背景on/offでも開口の内側が下地まで透過する。opacity 0/0.5のalphaを両hostの実画像で確認し、枠とtitleだけを残す |
| manifest-declared enum/range/color/string | 宣言したcontrolだけが表示され、変更が両hostへ同じ型と値で届く |
| canvas背景（skinが提供する場合） | narrow gapで素材が見え、背景なしでも完成形。canvas外のparticle等を含め意図したoverflowだけが現れる |
| 複数motion時刻・非表示/再表示 | 状態表現だけが動き、実値とplot位置は動かない。新規達成演出なし |

既存harnessで表現できない条件は、まず型とfixtureの制限を報告する。必要最小の合成fixture/harness変更を
開発workflowで行うか、利用者と合意した未検証範囲として残す。DOMを直接書き換えた画像をproduction確認にしない。
制作完了ではmatrixの未検証項目を隠さない。評価だけの依頼なら対象範囲に比例した部分matrixを明示してよい。

静的check、関連test、bundle/build、画像/操作の順で検証し、repositoryの必須checkも完了する。
実機Wayland composition/input、OBS内部のrender/Interactionは別の明示的live gate。
ブラウザの合格を実機OBS合格と報告しない。
