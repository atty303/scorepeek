# 実装と視覚検証

## 現行の接続先

repository rootから次を読む。行番号ではなく型・関数を確認し、将来の移動を追う。

| 責務 | 原典 |
| --- | --- |
| ZIP manifest・theme・package resource | `skins/<name>/skin.toml`、`theme.css`、`resources/` |
| skin Wasm実装 | `skins/<name>/src/lib.rs` |
| 任意の共通実装・authoring tool | `skins/shared/` |
| Rust ABI型・buffer helper | `crates/scorepeek-skin-sdk/src/lib.rs` |
| package検証・native Wasmtime/DOM | `crates/scorepeek-overlay/src/skin.rs`、`src/native.rs` |
| OBS Web Worker/browser DOM | `crates/scorepeek-overlay/src/skin_browser.js`、`src/web.rs` |
| package build/install | `scripts/build-skins.sh`、`scripts/with-isolated-skins.sh` |
| versioned authoring contract | `docs/skin-plugin-api-v2.md` |
| 設定schemaとmanifest property | `crates/scorepeek-overlay/src/config.rs` |
| masterと素材の由来 | `skins/DESIGN.md`、`skins/ASSETS.md` |

skin ID、表示名、release、propertyはmanifestが所有し、editorへ静的enumを追加しない。各skinは
独立したWasm crateとし、必要ならskin実体を知らない `skins/shared` を実装のショートカットとして使う。
独自treeもv2 ABIに従う。`skins/<name>/skin.toml` を追加すると `scripts/build-skins.sh` が検出する。
画像・font・licenseは
ZIP内へ入れ、通常runtimeのembedded assetへ登録しない。widget resizeで枠厚や文字が一緒に
伸びないこと、canvas cropと意図したoverflowを両hostで確認する。
`preview.png`と`preview.webm`はskill本体の共通preview sceneからproduction browser経路で生成する。
デザインmaster、比較sheet、DOMの手動書換えまたは別実装による再現画像をpackageへ入れない。生成後は
PNGをnativeとbrowserのeditorで選択して表示し、WebMをbrowser editorで再生・loop確認する。native editorはPNGだけを表示する。
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
contact sheetで全体を走査しても、文字/baseline/frameの疑わしい箇所は原寸で開く。
layoutの正のrectangleだけではnative paintを証明できない。
`visual-composition.json` も使用し、各skin IDと明示motion時刻のvariantを新しい一時scenarioとして作る。
sourceのscenario型が許す項目だけを使う。fixtureに未対応のstate注入をあるものとして実行しない。

```sh
mise run overlay:visual:obs -- /tmp/skin-browser-new/overlay.toml 127.0.0.1:17384
```

- configは未存在であること。stdinを保持する対話terminalで起動し、HTTP応答後に開く。Enterで終了するserverなのでstdin EOFを起動失敗と誤認しない。
- 利用可能なBrowser skillに従い、利用者が指定したブラウザを使う。`/overlay` を原則1920×1080（指定があればそのlogical size）で開く。
- 右clickで編集に入り、対象skin・screen/canvasを選ぶ。top-levelとcanvas iframeのDOMを両方読む。composed screenshotを取得して実際に見る。
- widget選択、移動、四隅resize、manifestで宣言したproperty、EMPTY title有無・aspect、scroll、save/reopen、別変更のdiscardを試す。保存先は一時configだけ。
- 狙ったskinが全canvasへ反映されたか確認する。一つのcanvasの変更だけで全画面のskin検証済みとしない。
- 時刻の異なる表示を取得し、動きと安定した実値を確認する。CSS animationは明示motion時刻で、Wasmのscheduled full-tree更新はrender呼出しを伴うcaptureで確認する。v2 ABIの `backend` と `monotonic_ms` を使う最適化とmotionはskinが所有する。nativeとbrowserで同じ内容/サイズを比較し、pixel equalityは求めない。
- 終了後serverを止め、所有tabを閉じ、viewportを戻し、一時config/scenario/outputをcleanupする。比較証拠を残すなら保持先を明示する。

`overlay:visual:obs` はbundle依存を持つ。backendだけを古いbundleと組み合わせない。
通常のcargo build/testもbundleを埋め込み、bundleの欠落またはbuild identity不一致はbuild時に拒否する。
実際に配布用binaryを作る依頼なら既存の `mise run dist:build` 等のbuild手順と成果物を確認する。

## 共通matrixと合否

同じmatrixを全ての新skinへ適用する。全組合せの直積は不要だが各行と意味のある重なりを両経路で確認する。

| 条件 | 合格oracle |
| --- | --- |
| 全体/詳細と採用master | 同じ内容・幅で輪郭、素材の縁/反射、文字階層、密度が再現されている |
| 長い和英混在title・artist・option | 共通の収め方を維持し、文字化け/無断略称/ラベルと値の重なりなし |
| 数値0/1/999/1000など桁変化 | 桁/label baselineが安定し、atlasの透明余白で列が浮かない |
| SP/DP・全難易度・level1/12 | 難易度色がスキン背景から分離し、種別と数値を独立に読める |
| 全clear・A/AA/AAAとB以下 | AAA/FCの特別感が独立し、unknownに達成処理を付けない |
| AAA+FAILED、AA+FC、A+EX HARD | 評価軸を混同せず、同時motionがcontentを圧倒しない |
| 判定、FAST/SLOW、miss0、不明 | 原則どおりの強弱、対称性、中立表示。0とunknownを区別 |
| 履歴・graph、欠損値 | 列/軸/単位/順序を維持し、欠損を架空の線で結ばない |
| 小widget・横長/縦長・host geometry | 固定cornerと枠厚、内側寸法、resize handle、cropを保つ |
| EMPTY、title有無、aspect | cut cornerが矩形化せず、内側の意図した透過またはfillを保つ |
| manifest-declared enum/range/color/string | 宣言したcontrolだけが表示され、変更が両hostへ同じ型と値で届く |
| canvas背景（skinが提供する場合） | narrow gapで素材が見え、背景なしでも完成形。canvas外のparticle等を含め意図したoverflowだけが現れる |
| 複数motion時刻・非表示/再表示 | 状態表現だけが動き、実値とplot位置は動かない。新規達成演出なし |

既存harnessで表現できない条件は、まず型とfixtureの制限を報告する。必要最小の合成fixture/harness変更を
開発workflowで行うか、利用者と合意した未検証範囲として残す。DOMを直接書き換えた画像をproduction確認にしない。
制作完了ではmatrixの未検証項目を隠さない。評価だけの依頼なら対象範囲に比例した部分matrixを明示してよい。

静的check、関連test、bundle/build、画像/操作の順で検証し、repositoryの必須checkも完了する。
実機Wayland composition/input、OBS内部のrender/Interactionは別の明示的live gate。
ブラウザの合格を実機OBS合格と報告しない。
