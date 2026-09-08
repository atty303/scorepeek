# 実装と視覚検証

## 現行の接続先

repository rootから次を読む。行番号ではなく型・関数を確認し、将来の移動を追う。

| 責務 | 原典 |
| --- | --- |
| Skin列挙・CSS組込み・widget描画 | `crates/scorepeek-overlay-ui/src/lib.rs`、`src/appearance.rs` |
| frameのcorner/edge/surface | `crates/scorepeek-overlay-ui/src/frame.rs`、`styles/rich.css` |
| canvas背景・emptyの形状・fill | `crates/scorepeek-overlay-ui/src/composition.rs`、`styles/composition.css` |
| 色・skin-specific CSS | `crates/scorepeek-overlay-ui/styles/` |
| artwork/fontの共有登録 | `crates/scorepeek-overlay-ui/src/assets.rs`、`assets/skins/README.md` |
| 文字の意味・atlas寸法/baseline | `crates/scorepeek-overlay-ui/src/typography.rs` |
| atlas生成 | `crates/scorepeek-overlay/examples/generate_type_atlas.rs`、`mise run overlay:type:generate` |
| motionの共有仕様と両driver | `crates/scorepeek-overlay-ui/assets/motion.json`、`src/motion.rs`、`motion.js` |
| native/web接続 | `crates/scorepeek-overlay/src/native.rs`、`src/web.rs`、`crates/scorepeek-overlay-web/src/` |
| 設定と既存schema | `crates/scorepeek-overlay/src/config.rs` |
| masterと素材の由来 | `docs/design/overlay-canvas/README.md` |

表の後半にある省略path（`src/`、`styles/`、`assets/`、`motion.js`）は同じセルの先頭pathと同じcrate配下。新skin登録ではenum、名前、CSS、画像URL、
atlas/labelの選択、frame/composition、editor選択肢、serialization、生成toolの全対応をsourceで追う。
既存3種だけを列挙するmatch/arrayを検索し、登録漏れを確認する。新しい別rendererは追加しない。
画像の固定cornerと伸縮edgeを保ち、widget resizeで枠厚や文字が一緒に伸びないようにする。
S/M/Lは内側のcontent寸法を維持して外側へ広がる。canvas crop、chamfered apertureを保つ。

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
`visual-composition.json` も使用し、各skinと明示motion時刻のvariantを新しい一時scenarioとして作る。
sourceのscenario型が許す項目だけを使う。fixtureに未対応のstate注入をあるものとして実行しない。

```sh
mise run overlay:visual:obs -- /tmp/skin-browser-new/overlay.toml 127.0.0.1:17384
```

- configは未存在であること。stdinを保持する対話terminalで起動し、HTTP応答後に開く。Enterで終了するserverなのでstdin EOFを起動失敗と誤認しない。
- 利用可能なBrowser skillに従い、利用者が指定したブラウザを使う。`/overlay` を原則1920×1080（指定があればそのlogical size）で開く。
- 右clickで編集に入り、対象skin・screen/canvasを選ぶ。top-levelとcanvas iframeのDOMを両方読む。composed screenshotを取得して実際に見る。
- widget選択、移動、四隅resize、S/M/L、EMPTY title有無・aspect・fill・背景、scroll、save/reopen、別変更のdiscardを試す。保存先は一時configだけ。
- 狙ったskinが全canvasへ反映されたか確認する。一つのcanvasの変更だけで全画面のskin検証済みとしない。
- 時刻の異なる表示を取得し、動きと安定した実値を確認する。nativeとbrowserで同じ内容/サイズを比較し、pixel equalityは求めない。
- 終了後serverを止め、所有tabを閉じ、viewportを戻し、一時config/scenario/outputをcleanupする。比較証拠を残すなら保持先を明示する。

`overlay:visual:obs` はbundle依存を持つ。backendだけを古いbundleと組み合わせない。
通常のcargo build/testが `target/debug/scorepeek` をembedded-webなしに置き換える場合があるため、
検証後にそのpathをOBS対応の起動成果物だと案内しない。実際に配布用binaryを作る依頼なら
既存の `mise run dist:build` 等のbuild手順と成果物を確認する。

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
| 小widget・横長/縦長・S/M/L | 固定cornerと枠厚、内側寸法、resize handle、cropを保つ |
| EMPTY、title有無、aspect、fill | cut cornerが矩形化せず、内側は透過/fill設定に従う |
| 背景none/static/animated | narrow gapで素材が見え、noneでも完成形。背景や光がcanvasから漏れない |
| 複数motion時刻・非表示/再表示 | 状態表現だけが動き、実値とplot位置は動かない。新規達成演出なし |

既存harnessで表現できない条件は、まず型とfixtureの制限を報告する。必要最小の合成fixture/harness変更を
開発workflowで行うか、利用者と合意した未検証範囲として残す。DOMを直接書き換えた画像をproduction確認にしない。
制作完了ではmatrixの未検証項目を隠さない。評価だけの依頼なら対象範囲に比例した部分matrixを明示してよい。

静的check、関連test、bundle/build、画像/操作の順で検証し、repositoryの必須checkも完了する。
実機Wayland composition/input、OBS内部のrender/Interactionは別の明示的live gate。
ブラウザの合格を実機OBS合格と報告しない。
