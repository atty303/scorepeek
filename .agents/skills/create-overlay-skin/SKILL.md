---
name: create-overlay-skin
description: scorepeekのoverlayスキンを、IIDXの意味に沿った複数デザイン案、自己完結ZIPのWasm DOM・CSS・素材実装、native／ブラウザの視覚評価まで制作する。新しいスキンの追加、既存スキンの大幅な再設計、スキンのデザイン原則に基づく評価を依頼されたときに使う。スキンの切替操作、接続障害の診断、無関係なゲームUIには使わない。
---

# Create overlay skin

## Scope and sources

- 依頼を新規制作・再設計・評価のいずれかとして扱い、依頼された段階だけを実行する。評価依頼から実装へ勝手に進まない。
- repositoryのAGENTS.mdと開発workflowに従う。本skillはruntimeのデータ契約や保存済み配置の変更を許可しない。
- 最初に [design-principles.md](references/design-principles.md) と [game-knowledge.md](references/game-knowledge.md) を読む。実装・評価の前に [implementation-and-verification.md](references/implementation-and-verification.md) を読む。
- references内のrepository相対pathはrepository rootから解決する。既存スキンの実装は手段の例であり、新しい原則への適合を証明する見本ではない。
- 本skillの原則は新規スキンと依頼された再設計に適用する。評価で既存スキンとの差を見つけても、全既存スキンを無断で変更しない。
- 不明な事実はsourceまたは根拠資料で調べる。利用者の好みと、ゲームの事実や実装仕様を分ける。未確定のデザイン判断だけを質問する。

## 1. Compare directions

1. 対象widget、用途、参考画像、希望する世界観、既存の承認を確認する。保存済みレイアウトは利用者が所有する。
2. 指定がなければ異なる3案を作る。各案は同じ合成データ・サイズ・配置条件で、全体構成と代表widgetの詳細をセットにする。色だけ変えた案にせず、素材・輪郭・文字・光の性格に違いを作る。
3. 画像が必要なら利用可能な画像生成toolとそのskillを使う。生成画像に焼き込まれた文字は、実装時にlive text/atlasへ置き換えるための視覚見本として扱う。
   生成後に全体と詳細を見比べ、widgetの欠落・勝手な項目追加・値や縦横比の不一致を確認する。不一致の画像を選択候補として合格にしない。修正生成で解消できなければ、生成素材を同一の決定的なレイアウトと文字へ組み合わせた比較見本を作るか、未達の比較条件を示してその段階で停止する。
4. 全体と詳細を実際に表示し、選択を求める。世界観の候補名、素材、配色、文字の性格、想定する動きを短く添える。未選択のまま任意の案を採用して実装しない。
5. 選択済みの参考がある場合は再選択を求めない。採用画像と承認された条件を制作成果物に保持し、以後の再現基準にする。

## 2. Prove material and motion

1. 代表widgetでフレーム、EX SCORE・DJ LEVEL、難易度、クリア状態、和英混在曲名を実装する。AAAとFULL COMBOの特別感は独立して設計する。
2. 常時の控えめな動きと状態固有の継続表現だけを扱う。新規達成判定、一度きりの祝福、カウントアップ、演出のqueueや調停機構を作らない。
3. 同じDOM/CSS・素材・意味設定をnativeとブラウザで評価する。Rust/JSの駆動差は許す。nativeで描けないCSSをブラウザ画像だけで合格にしない。
4. 両経路の静止画と、異なる時刻の表示または短い動きの記録を提示し、動く試作の確認を求める。明確な素材・輪郭・文字の妥協が必要なら見本との比較を先に示す。
5. 再現手段の通常の選択や修正は自律的に行う。見本の見た目を変える妥協だけを確認対象にし、簡略化した成果を黙って採用しない。

## Package preview

- package previewはデザインmaster、比較sheetまたは手描きmockupを流用せず、完成したskinのWasm、CSS、fontおよびpackage resourceをブラウザ経路で実際に描画して生成する。
- repository共通のversioned preview sceneを使用する。canvasは640×640、背景はanimated、frame widthはMとし、次のwidgetを固定配置する。

  | widget | x | y | width | height |
  | --- | ---: | ---: | ---: | ---: |
  | selection | 48 | 22 | 544 | 124 |
  | score | 48 | 178 | 544 | 200 |
  | history-graph | 48 | 410 | 544 | 208 |

- selectionのtitleへmanifestのskin名、artistへmanifestのauthorを入れる。AAAとFULL COMBOを含むscore、detailおよびgraphはsceneの固定合成データを使い、skinごとに値を変えない。status、history-listおよびemptyはpackage previewへ含めない。
- `preview.png`は640×640 PNG、`preview.webm`は640×640、8秒、25 fps、VP9、音声なしとする。動画は実際の継続animationを記録し、loop境界の不連続は許容する。値とgraph geometryを動かさない。
- PNGとWebMは同じsceneとproduction browser DOMから生成する。web catalogとbrowser editorは両方、native editorはPNGを使用する。これは制作・catalog掲載規格であり、任意の外部skinに対するinstall拒否条件へ拡張しない。
- 生成時は全resource request、Wasm init/render、期待DOM、画像寸法、動画codec・duration・frame rate・audio absenceおよび成果物hashを機械的に検証する。欠落resource、skin failure、文字やwidgetの欠落を残したpreviewをpackageしない。

## 3. Expand and verify

- 試作の承認後、status・selection・score・history-list・history-graph・emptyとcanvas背景へ展開する。欧文の主要数値だけでなく、定型ラベルも一貫した素材にする。
- 意味別の色・文字・素材・動きの規則をまとめ、widgetごとの場当たり的なCSSや別のnative/webレイアウトを増やさない。
- 検証referenceの共通matrixを実行し、見本比較と条件変更耐性を分けて評価する。既知の崩れを残したまま利用者へ品質判断を委ねない。
- 選択済み見本と試作、共通原則を満たせば追加の最終承認なしで完了してよい。成果物・比較画像・検証条件・限界を示し、適用される開発workflowのreviewとローカルcommitを行う。

## Stop and report precisely

- 案が未選択、動く試作が未承認なら、その依存段階だけを止める。利用者の既存承認を繰り返し求めない。
- 必須の画像生成やブラウザtoolが利用不能なら、その境界と未完了成果を示す。文章だけを生成見本、DOM検査だけを視覚評価の代用にしない。明示されたブラウザを別製品へ無断で置換しない。
- 認識、成績計算、データモデル、保存、依存追加、利用者の配置変更が必要ならスキンの範囲から切り離して確認する。ゲーム知識を表示機能追加の許可として使わない。
- 検証した既存スキンの画像は新規案の承認証拠ではない。部分的な試用・部分matrixを全工程合格と呼ばない。
- agent所有のserver、tab、temp configと生成途中の素材をcleanupする。保持する比較・診断証拠だけを明示した成果物先に残す。
