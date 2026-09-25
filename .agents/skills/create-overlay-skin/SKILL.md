---
name: create-overlay-skin
description: scorepeekのoverlayスキンを、IIDXの意味に沿った複数デザイン案、自己完結ZIPのWasm DOM・CSS・素材実装、native／ブラウザの視覚評価まで制作する。新しいスキンの追加、既存スキンの大幅な再設計、スキンのデザイン原則に基づく評価を依頼されたときに使う。スキンの切替操作、接続障害の診断、無関係なゲームUIには使わない。
---

# Create overlay skin

**責務:** 基本design systemを使う汎用制作工程を所有する。候補選択、
全widget実装、標準preview静止画・動画とnative/browser検証までを案内する。
skin APIの技術契約、特定skinの世界観、任意シリーズの表現仕様、旧作比較はここで定義しない。

## Scope and sources

- 依頼を新規制作・再設計・評価のいずれかとして扱い、依頼された段階だけを実行する。評価依頼から実装へ勝手に進まない。
- repositoryのAGENTS.mdと開発workflowに従う。本skillはruntimeのデータ契約や保存済み配置の変更を許可しない。
- 最初に [basic-design-system.md](references/basic-design-system.md) を読む。ゲーム知識が必要な場合は [game-knowledge.md](references/game-knowledge.md) を参照する。実装・評価の前に [implementation-and-verification.md](references/implementation-and-verification.md) を読む。
- references内のrepository相対pathはrepository rootから解決する。常設の設計入力は基本design systemだけとし、利用者がシリーズ仕様を指定した場合だけ追加入力する。skin固有仕様は [template](references/skin-specification-template.md) に記録する。既存スキンの設計資料と共有実装を制作入力にしない。
- 本skillの原則は新規スキンと依頼された再設計に適用する。評価で既存スキンとの差を見つけても、全既存スキンを無断で変更しない。
- 不明な事実はsourceまたは根拠資料で調べる。利用者の好みと、ゲームの事実や実装仕様を分ける。未確定のデザイン判断だけを質問する。

## 1. Compare directions

1. 対象widget、用途、参考画像、希望する世界観、既存の承認を確認する。保存済みレイアウトは利用者が所有する。
   API文書とSDKを使い、skinごとに独立したWasm実装、manifest、CSS、素材を制作する。共通のDOMやCSSを複製元として使わない。
2. 指定がなければ異なる3案を作る。各案は現行package previewに近い情報量で全widgetと主要状態の世界観を伝えるモックとし、色だけでなく素材・輪郭・文字・光の性格を変える。各方向を全widgetへ展開できるか見通しを確認する。モックの数字や配置に実装相当の厳密さは求めない。
3. 画像生成toolとそのskillをconceptと素材部品の制作候補にする。生成画像の文字は視覚見本であり、実装ではlive textや意味を保持するatlasへ置き換える。SVGやImageMagickだけで質感を満たせない場合は画像生成で枠、面、発光、文字を試作する。
4. 候補を実際に表示し、世界観の候補名、素材、配色、文字の性格、想定する動き、全widgetへの展開方針を短く添えて選択する。利用者が代理選択を明示的に任せた場合は担当agentが選び、理由を記録する。
5. 採用モックの世界観と素材の方向を保持する。配置、寸法、具体色、書体、光、動きは完成度と可読性のために調整し、モック再現だけで完了しない。

## 2. Prove material and motion

1. 全widgetへ進む前に、素材と文字と動きの一部を作り、Select・Score・Historyの代表部分を実寸でnative描画する。少なくともレビューmatrixの表示寸法と、別に使うpackage previewの寸法で確認し、大きい試作だけで小さい表示の可読性を推定しない。情報を載せたときの輪郭、素材の厚み、ラベルと数値の質感、可読性、階層を確認し、不足があれば部品を作り直す。AAAとFULL COMBOの特別感は独立して設計する。
   数字、固定ラベル、DJ LEVEL、CLEAR TYPEなど低カーディナリティの文字はテクスチャアトラスを基本候補にする。曲名・artistと集合が定まらないPLAY OPTIONSは通常のテキストにする。明確なフラット表現など別手法が適する場合は実寸描画で品質を判定する。atlasを使う場合も意味テキストをDOMに保つ。
2. 常時の控えめな動きと状態固有の継続表現だけを扱う。AAAやFULL COMBOには、動的DOM/CSSに加え、事前生成したスプライトアトラスをCSS Animationで切り替える方法も検討する。新規達成判定、一度きりの祝福、カウントアップ、演出のqueueや調停機構を作らない。
3. 同じDOM/CSS・素材・意味設定をnativeとブラウザで評価する。Rust/JSの駆動差は許す。nativeで描けないCSSをブラウザ画像だけで合格にしない。
4. 代表widgetの静止画と開始・中間・終端・loop継ぎ目の実寸表示で素材と動きを内部確認する。世界観の一部として設計した継続motionが時刻差分で目視できず、状態の特別感も伝わらなければ再制作する。静止を意図した世界観に動きを強制しない。利用者へskinのレビューを求めるときは、先に全widgetへ展開して「3. Expand and verify」の提示条件を満たす。明確な素材・輪郭・文字の妥協が必要なら見本との比較を先に示す。
5. 再現手段の通常の選択や修正は自律的に行う。見本の見た目を変える妥協だけを確認対象にし、簡略化した成果を黙って採用しない。

## Package preview

- package previewは採用concept、比較sheetまたは手描きmockupを流用せず、完成したskinのWasm、CSS、fontおよびpackage resourceをブラウザ経路で実際に描画して生成する。
- skill所有の [`preview-scene.json`](preview-scene.json) と
  [`generate-skin-previews.bash`](scripts/generate-skin-previews.bash) を使用する。
  canvasは640×640とし、skinがmanifestで宣言した既定の表現を使って次のwidgetを固定配置する。

  | widget | x | y | width | height |
  | --- | ---: | ---: | ---: | ---: |
  | selection | 48 | 22 | 544 | 124 |
  | score | 48 | 178 | 544 | 200 |
  | history-graph | 48 | 410 | 544 | 208 |

- selectionのtitleへmanifestのskin名、artistへmanifestのauthorを入れる。AAAとFULL COMBOを含むscore、detailおよびgraphはsceneの固定合成データを使い、skinごとに値を変えない。status、history-listおよびemptyはpackage previewへ含めない。
- `preview.png`は640×640 PNG、`preview.webm`は640×640、8秒、25 fps、VP9、音声なしとする。動画は実際の継続animationを記録し、loop境界の不連続は許容する。値とgraph geometryを動かさない。
- PNGとWebMは同じsceneとproduction browser DOMから生成する。web catalogとbrowser editorは両方、native editorはPNGを使用する。これは制作・catalog掲載規格であり、任意の外部skinに対するinstall拒否条件へ拡張しない。
- 生成時は全resource request、Wasm init/render、期待DOM、画像寸法、動画codec・duration・frame rate・audio absenceおよび成果物hashを機械的に検証する。欠落resource、skin failure、文字やwidgetの欠落を残したpreviewをpackageしない。
- sceneのskin一覧は制作対象を明示してから生成する。同梱以外の作者も同じsceneと生成・検査処理を使用し、出力先を指定できる。
- 実装後に`SCOREPEEK_SKIN_PREVIEW_SLUG=<slug> mise run overlay:skins:preview:generate`でproduction描画からpackage previewを生成する。

## 3. Expand and verify

- 代表widgetの内部検証後、status・selection・score・history-list・history-graph・emptyとcanvas背景へ展開する。欧文の主要数値だけでなく、定型ラベルも一貫した素材にする。
- 意味別の色・文字・素材・動きの規則をまとめ、widgetごとの場当たり的なCSSや別のnative/webレイアウトを増やさない。
- 検証referenceの共通matrixを実行し、見本比較と条件変更耐性を分けて評価する。既知の崩れを残したまま利用者へ品質判断を委ねない。
- skin制作中のレビュー提示は、[実装と視覚検証](references/implementation-and-verification.md#レビュー提示用のbrowser動画)の全widget・状態matrixを1920×1440の一画面に収めたbrowser動画を使う。nativeでは同条件の代表時刻を描画し、開始、変化、中間、終端、loop境界の整合をagentが確認する。レビュー制作中はpackageの`preview.png`を変更しない。
- 選択済み見本、全widgetのレビュー結果、共通原則を満たせば追加の最終承認なしで完了してよい。成果物・比較画像・検証条件・限界を示し、適用される開発workflowのreviewとローカルcommitを行う。

## Stop and report precisely

- 案が未選択、または提示した全widgetのレビューで修正が求められたら、その依存段階だけを止める。利用者の既存承認を繰り返し求めない。
- 必須の画像生成やブラウザtoolが利用不能なら、その境界と未完了成果を示す。文章だけを生成見本、DOM検査だけを視覚評価の代用にしない。明示されたブラウザを別製品へ無断で置換しない。
- 認識、成績計算、データモデル、保存、依存追加、利用者の配置変更が必要ならスキンの範囲から切り離して確認する。ゲーム知識を表示機能追加の許可として使わない。
- 検証した既存スキンの画像は新規案の承認証拠ではない。部分的な試用・部分matrixを全工程合格と呼ばない。
- agent所有のserver、tab、temp configと生成途中の素材をcleanupする。保持する比較・診断証拠だけを明示した成果物先に残す。
