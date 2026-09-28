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
- references内のrepository相対pathはrepository rootから解決する。常設の設計入力は基本design systemだけとし、利用者がシリーズ仕様を指定した場合だけ追加入力する。skinの現行仕様は [specification template](references/skin-specification-template.md) に従って `skins/<slug>/SPEC.md` に、実行ごとの検証記録は [verification template](references/skin-verification-template.md) に従ってGit除外された `skins/<slug>/verification/verification.md` に記す。既存スキンの設計資料と共有実装を制作入力にしない。
- 本skillの原則は新規スキンと依頼された再設計に適用する。評価で既存スキンとの差を見つけても、全既存スキンを無断で変更しない。
- 不明な事実はsourceまたは根拠資料で調べる。利用者の好みと、ゲームの事実や実装仕様を分ける。未確定のデザイン判断だけを質問する。

## 制作順序のチェックポイント

新規制作と大幅な再設計では、次の順番を守る。後段の機械検査に通っても、前段の実寸・視覚判定を飛ばした完成扱いはできない。

1. 全widgetを含む異なる3案を描き、採用する世界観を選ぶ。
2. **全widgetの仕上げ、最終preview、最終ZIPより先に**、採用案のSelect・Score・Historyを情報入りで試作する。レビュー配置とpackage previewそれぞれの実寸で、枠の離れた辺、情報面、通常の数字とラベルを確認する。物理素材を核にする案では画像生成部品と決定的な部品、通常文字のlive表示と素材入り字形をnative/browserで比較する。素材や文字が平板、ぼけ、二重像、読みにくさへ退いたら技法を変えて再試作し、比較画像と採否理由を検証記録へ残す。このチェックポイントを通るまで後段へ進まない。
3. 通過した表現を全widgetと必要な状態へ展開し、推奨寸法でnative/browserを検証する。長い曲名・artist・PLAY OPTIONSの標準試験値も推奨寸法で描き、末尾が画像から読めるまで直す。DOMの`textContent`だけでは可視の全文表示を証明しない。標準のpreview、レビュー動画、ZIPを最終sourceから生成し、独立した画像だけのreviewを受ける。任意の連続的なリサイズ範囲の証明より、推奨寸法における素材・文字・状態の完成度を優先する。

試作前に全widgetを仮実装した場合も、試作の合格を記録するまで仕上げと最終成果物の制作を止める。以下の各節はこの順番の具体的な判定方法を定める。

## 1. Compare directions

1. 対象widget、用途、参考画像、希望する世界観、既存の承認を確認する。保存済みレイアウトは利用者が所有する。
   API文書とSDKを使い、skinごとに独立したWasm実装、manifest、CSS、素材を制作する。共通のDOMやCSSを複製元として使わない。
   新規skinには、採用する世界観を識別できる新しいdirectory slugとmanifest IDを与える。既存skinと併存させる依頼では既存IDを再利用せず、選択画面でも区別できる表示名にする。隔離worktreeで既存skinのファイルを外していても、衝突検査だけはGit HEADのslugとmanifest IDまで含める。この検査は旧skinの意匠を制作入力にするものではない。
2. 指定がなければ異なる3案を作る。各案は現行package previewに近い情報量で全widgetと主要状態の世界観を伝えるモックとし、色だけでなく素材・輪郭・文字・光の性格を変える。各方向を全widgetへ展開できるか見通しを確認する。モックの数字や配置に実装相当の厳密さは求めない。
3. 画像生成toolとそのskillをconceptと素材部品の制作候補にする。生成・手描きのどちらのモックでも文字・見出し・数値は世界観を伝える仮表示であり、実装の情報契約ではない。実装前に基本design systemの全widget項目・表示名と照合し、モックにあっても不要な見出しを実装へ持ち込まない。実装ではlive textや意味を保持するatlasへ置き換える。金属、ガラス、陶器など物理素材が世界観の核なら、決定的なSVG/CSS/ImageMagickだけで完成を宣言する前に、画像生成toolで枠の接合部または情報面の部品を最低1案試作し、実寸で並べて選ぶ。明確なフラット表現にはこの試作を要求しない。
4. 候補の全widgetモックを実際に表示し、世界観の候補名、素材、配色、文字の性格、想定する動き、全widgetへの展開方針を短く添えて選択する。利用者が代理選択を明示的に任せた場合は担当agentが選び、理由と採用モックを実装前に利用者へ示す。
5. 採用モックから、枠・**情報が載る面**・固定文字・動的文字・特別な状態に必要な世界観の手掛かりをskin固有仕様に記す。素材を核にする案では、情報面の走査・反射・粒子・奥行きなどから核となる手掛かりを名指しし、最終renderで実際に描かれた箇所を対応付ける。物理的な筐体なら、華やかな角だけでなく離れた辺と情報面へのつながりも制作対象にする。可読性のためにその手掛かりを取り除くなら、情報面に同等の素材表現を作り直す。これは制作後の合否に使う観測項目であり、モックの位置や色を固定する設計書ではない。どの手掛かりも背景画像だけに任せない。
6. 採用モックの世界観と素材の方向を保持する。配置、寸法、具体色、書体、光、動きは完成度と可読性のために調整し、モック再現だけで完了しない。

## 2. Prove material and motion

1. 全widgetへ進む前に、素材と文字と動きの一部を作り、Select・Score・Historyの代表部分を実寸でnative描画する。少なくともレビューmatrixの表示寸法と、別に使うpackage previewの寸法で確認し、大きい試作だけで小さい表示の可読性を推定しない。情報を載せたときの輪郭、素材の厚み、ラベルと数値の質感、可読性、階層を確認し、不足があれば部品を作り直す。AAAとFULL COMBOの特別感は独立して設計する。
   まず基本design systemの行列・比率・行高を、選んだ文字と素材で情報入りの動く試作に実装する。特にScoreは左右54/46、左の66/68/24 px、上段54/46・中段40/60、右の判定行と二段optionsを440×194で描き、同じ内容を最終的な推奨寸法でも見る。値が入らなければ、必要な表示面積と文字寸法を調整してから次へ進む。baselineを飛ばして別の比率へ直行しない。Selectionは短い通常titleと長い試験titleを同じ寸法で描き、長文への対処が短いtitleとartistを一律に小さくしていないことを確認する。Scoreは通常状態とAAA/FULL COMBOを描き、SCOREとDJ LEVELが最初の二つの値として目に入り、CLEARが判定詳細より先に独立した結果として見えることを確認する。文字が転記できるだけではこの階層確認を通さない。
   数字、固定ラベル、DJ LEVEL、CLEAR TYPEなど低カーディナリティの文字はテクスチャアトラスを基本候補にする。曲名・artistと集合が定まらないPLAY OPTIONSは通常のテキストにする。明確なフラット表現など別手法が適する場合は実寸描画で品質を判定する。atlasを使う場合も意味テキストをDOMに保つ。
   物理的な素材を核にする世界観では、通常のSCORE数字・固定ラベル・判定ラベル・Historyの数字と見出しについて、live文字と、素材を字形へ持ち込むatlasまたはglyph maskを実際に制作し、同じscene内の実寸Score・History panelで比較する。書体だけで完成させる場合も、この素材入り候補と比較する。素材の手掛かりは文字の面・輪郭・刻印・特徴的な字形などに見える必要があり、背後の質感や隣の発光だけで代用しない。既製の等幅fontを太くしただけ、AAA/FCだけを飾っただけなら試作は不合格とする。候補の一方がぼやけ、もう一方が平板なら勝者を決めず、mask解像度・font・レイヤー・制作技法を変えて再試作する。選ばなかった手法と実寸での理由を検証記録へ記す。明確なフラット表現ならその理由と実寸証拠を記して素材入り候補を省ける。
   **全widget実装へ進む停止条件:** 通常のSCORE数字・固定／判定ラベルとHistory数字／見出しを載せた、実寸のScore・History panelを同じsceneに置く。物理的な素材を核にする場合は、live文字、素材入り候補と技法に合う対照のnative画像を並べ、字形領域の画素差と目視できる世界観の差を確認する。書体を比較する対照は太さ・字幅・明るさを揃えた中立的なfont、面・縁の効果を比べる対照は装飾なしの同書体にする。画素差は技法が描画された証拠に限り、質感の合格証拠にはしない。この素材を核にする場合は、通常文字を枠から切り離して原寸で見ても、世界観に固有の字形・面・縁の手掛かりを2つ以上指摘できること。素材入り候補のnativeで差がない、または差が太さ・ぼけ・二重像だけなら未合格として技法を変える。明確なフラット表現では素材入り候補を省き、意図した字形・間隔・階層を実寸で示す。browserでも適用する候補を描く。`text-shadow`などのCSS宣言やbrowser画像からnativeの文字表現を推定しない。適用する比較またはフラット表現の実寸確認と可読性の合格を検証記録へ残すまで、他widgetへの仕上げの展開、最終previewと最終packageの制作へ進まない。native/browser試作用の一時ZIPにはAPI必須のwidget設定と仮previewを入れてよいが、完成証拠に含めず後で作り直す。制作を中断する場合も、未合格の試作を完成品として提示しない。比較方法と技法の切替は[実装と視覚検証](references/implementation-and-verification.md#文字素材の実寸試作)に従う。
2. 常時の控えめな動きと状態固有の継続表現だけを扱う。AAAやFULL COMBOには、動的DOM/CSSに加え、事前生成したスプライトアトラスをCSS Animationで切り替える方法も検討する。新規達成判定、一度きりの祝福、カウントアップ、演出のqueueや調停機構を作らない。
3. 同じDOM/CSS・素材・意味設定をnativeとブラウザで評価する。Rust/JSの駆動差は許す。nativeで描けないCSSをブラウザ画像だけで合格にしない。
4. 代表widgetの静止画と開始・中間・終端・loop継ぎ目の実寸表示で素材と動きを内部確認する。申告した各継続motionについて、変化が最大・最小となる時刻の同じ効果領域をnativeとbrowserそれぞれで比較し、実画素の差と原寸で知覚できる差を確認する。CSS Animation宣言やbrowser動画だけでnativeの動きを合格にしない。データ領域は静止していることも確かめる。時刻差分で目視できず、状態の特別感も伝わらなければ再制作する。静止を意図した世界観に動きを強制しない。利用者へskinのレビューを求めるときは、先に全widgetへ展開して「3. Expand and verify」の提示条件を満たす。明確な素材・輪郭・文字の妥協が必要なら見本との比較を先に示す。
5. [basic design system の finish gates](references/basic-design-system.md#self-assessed-finish-gates) を、情報を載せた実寸の Select・Score・History の試作済み部分に適用する。採用conceptとrenderを並べ、背景をoffにしたrenderでもwidget自体が世界観を伝えるか確認する。モックとproductionの**情報面・通常の数字または固定ラベル・離れた2か所の枠辺と面への遷移**を対応させた比較画像を作る。各部について、モックの層・光・表面の特徴が最終描画のどこに現れるか、単一の塗りや細線へ簡略化していないかを実寸で判定する。モックの文字列・座標一致は求めない。物理素材を採る場合は一つの豪華な角材で残りの細線の枠を代用せず、package previewの全panelでも同じ構造が見えるまで進まない。モック自体の描き込みが少なくても完成品の素材品質をモック水準に下げない。試作に現れていないwidget・状態のgateは合格扱いせず最終展開まで保留する。判定できるgateについて、実寸の画像・時刻と具体的な画素・文字・状態を根拠に合否を記す。「問題なし」や素材名だけでは合格記録にならない。失敗時は原因を素材・文字・配置・動きに切り分け、必要なら画像生成部品、atlas、font、CSS構造など制作技法自体を変えて再描画する。親agentや利用者による質感指摘を待たず、制作担当自身が合否を決める。
6. 全widgetとpreviewを最終sourceから描いた後、制作過程・CSS・自己評価を見せずに、採用モック、背景offの実寸Select/Score/History、package previewと通常文字・離れた枠辺2か所の原寸cropだけを**独立した視覚reviewer**へ渡す。別agentを使える環境では制作担当と別のagentが判定し、使えない環境では制作過程を参照せず画像だけを時間を置いて再評価する。reviewerは説明から素材を推定せず、枠全体・情報面・通常文字だけで宣言した金属／ガラス等の素材を認識できるか、推奨サイズとpackage previewで読めるかを判定する。物理素材の一角だけが精巧で残りの枠が細線の場合、またはatlasを使っても通常文字が無個性なfontに見える場合は不合格にする。明確なフラット表現はその意図した形・文字・階層で判定する。各領域の可視の欠落を挙げた不合格を、制作担当の自己評価やDOM/checker通過で覆さない。素材または文字を作り直してnative/browserとpreviewを再生成し、同じ条件で再reviewする。結果と対応画像を検証記録へ残す。旧skinとの比較はこの恒常工程に含めない。
   最終の小文字・History行と同内容、同寸法、近い太さ・明るさの無装飾対照も渡す。reviewerに原寸で短い数値、長い日付、判定名をそれぞれ転記させ、最も判別しにくい箇所を指摘させる。装飾後の字画が点状に途切れる、二重ににじむ、または対照より連続して読むのが難しいなら、転記が正しくても文字gateは不合格とする。背景offだけでなく既定背景の全画面も渡し、モックが示した画面全体の素材手掛かり、または同等の世界観を担う代替表現が、標準previewの下地と合成した表示にも残るか判定させる。透過していること自体は失敗ではない。
   Statusの実寸画像も渡し、採用した公式logoの明暗・透過variantが表面に馴染むか判定させる。不透明画像の長方形が意図しない別の下地として浮いている場合は不合格とし、透過variantまたは意図を持つplate構造へ直す。
7. 再現手段の通常の選択や修正は自律的に行う。見本の見た目を変える妥協だけを確認対象にし、簡略化した成果を黙って採用しない。

## Package preview

- package previewは採用concept、比較sheetまたは手描きmockupを流用せず、完成したskinのWasm、CSS、fontおよびpackage resourceをブラウザ経路で実際に描画して生成する。
- skill所有の [`preview-scene.json`](preview-scene.json) と
  [`generate-skin-previews.bash`](scripts/generate-skin-previews.bash) を使用する。
  完成packageの生成ではskin固有のsceneを複製・上書き入力にせず、`SCOREPEEK_SKIN_PREVIEW_SCENE`を指定しない。独自sceneは出力先を明示した診断だけに使い、その画像を最終previewへ入れない。標準動画で採用した継続motionを見せる場合は、標準sceneで選ばれるmanifestの既定表現を設計する。
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
- `preview.png`そのものを原寸で確認する。canvasの半透明表現はpreview経路の下地色と合成された見た目で判定し、暗いreview sceneだけで素材感を証明しない。意図した世界観や情報の階層が下地で崩れるなら、skinの背景素材か不透明度を調整してpreviewを再生成する。
- sceneのskin一覧は制作対象を明示してから生成する。同梱以外の作者も同じsceneと生成・検査処理を使用し、出力先を指定できる。
- 実装後に`SCOREPEEK_SKIN_PREVIEW_SLUG=<slug> mise run overlay:skins:preview:generate`でproduction描画からpackage previewを生成する。

## 3. Expand and verify

- 代表widgetの内部検証後、status・selection・score・history-list・history-graph・emptyとcanvas背景へ展開する。欧文の主要数値だけでなく、定型ラベルも一貫した素材にする。
- baselineの寸法を変更する場合は、**同じskin、同じ内容・寸法・状態で変更前後をnative/browserに描く**。固有仕様の表に変更前後の数値と変える理由を残し、検証記録に両方の実寸画像への相対linkと影響を受けた隣接項目の読取結果を残す。文字素材を大きくしたいだけではCLEAR、差分、目盛り、判定、optionsの面積を奪う理由にならない。baselineの画像がない変更や、ひとつでも項目が読みにくくなる変更は完成版として採用せず、レイアウトか文字を作り直す。
- packageと最終描画の前に `bash .agents/skills/create-overlay-skin/scripts/check-skin-authoring-contract.bash <skin-dir>` で新skinのID・slugの未使用、6 widgetの既定寸法と背景状態を検査する。既存skinをその場所で再設計する依頼だけは`--existing`を付ける。canvasを静止させるconceptは固有仕様に理由を記して`--still`を追加し、無効なanimated controlを置かない。検査結果の`background_off_value`と`background_motion`をscene生成へ渡す。共通sceneの`background-off`/`background-static`と、canvasが動くconceptなら`background-animated`で全widgetを載せてnative/browser描画する。背景なしではSelect・Score・Historyの情報面と枠の世界観を原寸で読み、animated背景では周期中の時刻差を確認する。manifestの値だけで描画対応を合格にしない。
- 欠損・不正なSCOREまたはNOTESと、正当なSCORE 0を同じ条件で描画する。DJ LEVEL差分だけでなくscore rateのbar/dialを含む**すべての派生表示**を比較し、欠損時に0%を確定表示しない。欠損値を数字へparseして0に丸める実装を不合格にする。
- 最終描画の静的なラベルと見出しを基本design systemへ照合する。Scoreの表示名は`SCORE`で、モックに描かれた`EX SCORE`、`BEST`、`RESULT DETAIL`、`SCORE RATE`をpanel title・eyebrow・group headingとして復活させない。意味データのBEST/RESULT DETAILを表示上で混同しないこととは別の検査である。
- 意味別の色・文字・素材・動きの規則をまとめ、widgetごとの場当たり的なCSSや別のnative/webレイアウトを増やさない。
- 検証referenceの共通matrixを実行し、見本比較と条件変更耐性を分けて評価する。既知の崩れを残したまま利用者へ品質判断を委ねない。
- `history_count` 5/10/20/50、`graph_months` 1/3/6/12、EMPTYの横長/縦長・title有無・opacity 0/0.5、欠損と0、および長い曲名・artist・PLAY OPTIONSをmanifest既定寸法とpackage preview寸法で描く補助sceneを、標準レビュー動画とは別に両hostで実寸描画する。全情報を表示できる高さを各countに割り当てる。補助条件を未検証のまま完成扱いにしない。
- widgetごとに全情報がきれいに表示される**推奨幅・高さ**を完成仕様へ記す。Historyの行数など内容によって必要な高さが変わる場合は、その状態の推奨寸法も記す。推奨寸法と必要な状態をnative/browserで実寸確認する。任意のリサイズ組合せや幅・高さの連続範囲を完成条件にしない。追加で確認した寸法は個別の結果として検証記録へ残す。
- 全widgetへ展開した後もfinish gatesを実寸で再判定する。各widgetに実際の情報を載せたnative画像とbrowserの同時刻frameを使い、推奨寸法と異なる状態を確認する。package previewに載るSelection・Score・History Graphはそのsizeも確認する。全画面を縮小した一覧だけで小さい文字や素材を判定しない。小文字は画像だけからラベルと代表値を転記してからsceneと照合し、拡大やsource参照がないと読めない項目を不合格にする。長い曲名・artist・PLAY OPTIONSの試験値は末尾まで画像から読めるか確かめる。短いtitleの通常sceneと長いtitleの境界sceneを同じ寸法で並べ、長文の収まりを理由に通常titleとartistの優先順位が下がっていないか確認する。DOMに全文が残っていても、表示上の省略記号やclipで末尾を隠したら、その内容とsizeを全情報表示の検証済み条件に含めない。検証記録には各widgetの実寸証拠への参照、gateごとの可視の根拠、失敗時の修正と再描画結果を残す。文字と質感が簡略化したwidgetを、他のwidgetや背景が美しいという理由で通さない。
- 推奨Scoreとpackage previewの画像は拡大せず、SCORE、DJ LEVELと差分、MISS COUNT、CLEAR、バーの全目盛り、PGREATからCOMBO BREAKまで、FAST/SLOW、PLAY OPTIONSの値を画像だけから**すべて**転記する。ひとつでも隣の行・目盛り・枠へ接触し、値とラベルの境界を迷う、またはほかの補助情報より読みにくい寸法まで縮めないと収まらないなら、その寸法は可読性合格としない。Scoreの右列を下へ空けたまま詳細を左下の狭い一列へ押し込まない。転記結果を検証記録へ残し、scene値と照合する。
- PNG等のbitmapを文字atlasに使う場合、Score/Historyの全状態とpackage previewで、**表示される個々の字形・sprite cell**の元pixel寸法と、crop・`background-size`・transformを反映した実効表示寸法を照合する。atlas画像全体の寸法では判定しない。輪郭を滑らかに描く字形を元のcellより大きく表示してはいけない。小さいcellを拡大したAAAやCLEAR TYPEが読めても、にじみ・二重輪郭が見えるならlettering gateは不合格とし、表示寸法に足りる解像度で作り直す。意図したpixel artの補間方法は固有仕様に、実寸の仕上がりは検証記録に記録して判定する。
- ScoreとHistoryで同じ意味表現を検証する際は、レビューcase 02のAAAとcase 08のFULL COMBOを含むHistoryのnative・browser画像を実寸で読む。合成browser動画でHistoryが1例しか表示されないことを、この確認の代わりにしない。
- 独立した画像のみのreviewerには、まず通常Score、AAA、FULL COMBO、通常Selectionの**最終原寸画像だけ**を見せる。モック、scene値、実装、期待する読順はまだ渡さず、最初に目に入る二つのScore値と次の結果、Selectionで最初に目に入る項目を答えてもらう。SCOREとDJ LEVELが最初の二つ、CLEARが判定詳細より先、曲名がSelectionの先頭にならなければ階層gateは未合格。AAA/FULL COMBOが判読できても小さな周辺badgeに退いたら未合格。読順を記録した後でモックと全項目の転記、素材・動き・状態を評価する。比較対象の旧skinは渡さない。
- **意味段階の停止条件:** 共通scene生成toolの`rank-a`、`rank-aa`、`rank-aaa`は、同じchart・CLEAR・判定・FAST/SLOW・Historyで、ランクと整合するSCOREと先頭History行のSCORE/ランクだけを変える。3 sceneをnative/browserの両方で実寸描画して横に比較する。AからAAへ文字列以外のpositiveな手掛かりが加わり、AAからAAAへvery positiveな素材表現がさらに加わることを、ScoreとHistory双方の具体的な色・形・面・光を検証記録へ残す。score rate barや差分値の変化をランク文字の表現と取り違えない。同じScore画像内のPGREAT/GREAT/GOODを原寸で比較し、PGREATにだけより強い肯定表現が見えることを検証記録へ残す。3行へ同じclass・色・面を与えて文字列だけ変える実装は不合格。clearのNO PLAY/FAILED/ASSIST/EASY/CLEAR/HARD/EX HARD/FULL COMBOとBAD/POOR/COMBO BREAKの意味段階、FAST/SLOWの等しい重みの方向差も照合する。AAAとFCだけ、または文字列の読み分けだけで意味gateを合格にしない。
- History Graphはsceneの具体値から上下方向を再計算し、最終native/browser画像の右軸と赤線を照合する。共通matrixのcase 01では最初のMISS RATEが75%、最後が10%なので、100%は上、0%は下、最初の赤点は最後の赤点より上にある。軸・線・凡例の位置が一致しても、値の方向が逆なら不合格とする。
- skin制作中の利用者向けレビューには、採用済みの全widgetモックと、[実装と視覚検証](references/implementation-and-verification.md#レビュー提示用のbrowser動画)の全widget・状態matrixを1920×1440の一画面に収めたbrowser動画を一緒に示す。モックで承認された世界観・素材・文字・光が、完成描画のどこに実現されたかも短く対応付ける。モックの文字列や座標の一致を要求しない。nativeでは同条件の代表時刻を描画し、開始、変化、中間、終端、loop境界の整合をagentが確認する。レビュー制作中はpackageの`preview.png`を変更しない。
- 最後のWasm・CSS・font・素材変更後に、package preview、native行列、browser行列とZIPを同じsourceから再生成する。成果物の時刻・hashとpackage内のresourceを照合し、変更前の画像を最終検証の証拠へ混ぜない。
- 最終合格の根拠となる画像、動画、scene、検査結果は、Git除外された `skins/<slug>/verification/evidence/` に保存し、`verification/verification.md` から相対linkで示す。`/tmp`の試作画像は試行の説明にのみ使い、作業treeを消すと壊れる絶対pathを最終証拠にしない。通常ビルドで再生成するZIPそのものは恒久保存せず、生成commandとhashを検証記録へ記す。
- 選択済み見本、全widgetのレビュー結果、共通原則を満たせば追加の最終承認なしで完了してよい。成果物・比較画像・検証条件・限界を示し、適用される開発workflowのreviewとローカルcommitを行う。

## Stop and report precisely

- 案が未選択、または提示した全widgetのレビューで修正が求められたら、その依存段階だけを止める。利用者の既存承認を繰り返し求めない。
- 必須の画像生成やブラウザtoolが利用不能なら、その境界と未完了成果を示す。文章だけを生成見本、DOM検査だけを視覚評価の代用にしない。明示されたブラウザを別製品へ無断で置換しない。
- 認識、成績計算、データモデル、保存、依存追加、利用者の配置変更が必要ならスキンの範囲から切り離して確認する。ゲーム知識を表示機能追加の許可として使わない。
- 検証した既存スキンの画像は新規案の承認証拠ではない。部分的な試用・部分matrixを全工程合格と呼ばない。
- agent所有のserver、tab、temp configと生成途中の素材をcleanupする。保持する比較・診断証拠だけをskinの`verification/evidence/`に残す。
