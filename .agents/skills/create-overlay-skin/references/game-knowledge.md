# IIDXの意味とscorepeekの表示契約

確認日: 2026-09-08。公式の事実、sourceで確認した実装、利用者のデザイン判断を混同しない。
作品固有の画面装飾やoption条件を全シリーズ共通の規則へ拡張しない。
以下は要約であり、ゲーム画像・文面・座標・資産を取り込む資料ではない。

## 公式資料で確認した概念

| 概念 | 意味と混同しないこと | 根拠 |
| --- | --- | --- |
| 難易度種別 | BEGINNER緑、NORMAL青、HYPER黄、ANOTHER赤、LEGGENDARIA紫。種別とレベル数値は別情報 | [EPOLIS公式・選曲操作](https://p.eagate.573.jp/game/2dx/31/howto/play/tenkey.html) |
| EX SCORE | PGREATを2点、GREATを1点として数える。クリア条件の達成とは別の評価軸 | [SINOBUZ公式・記録](https://p.eagate.573.jp/game/2dx/24/p/howto/epass/epass1.html) |
| CLEAR TYPE | 未プレイ・失敗・各種クリア・FCを区別する。ゲージ/補助optionと関係するが、skinがoption文字列からクリア状態を再判定しない | [CANNON BALLERS公式・記録](https://p.eagate.573.jp/game/2dx/25/p/howto/epass/epass1.html) |
| MISS COUNT | ミスの回数を表す。自己ベストは小さい値。COMBO BREAKとは別の項目 | [Rootage公式・リザルト説明](https://p.eagate.573.jp/game/2dx/26/howto/play/game_start.html) |
| COMBO BREAK | コンボが途切れた回数。MISS COUNTの別名にしない | 同上 |
| FAST/SLOW | 押すtimingが早かった/遅かった回数。良い/悪いの対ではない | 同上、および [BEMANI公式コラム](https://p.eagate.573.jp/game/bemani/p/channel/column/kotsu/kotsu_04.html) |
| 判定内訳 | 各判定の獲得数。PGREAT/GREATはEX SCOREの加点に関わる。他の判定をskinでまとめたり数え直したりしない | Rootage公式・リザルト説明、SINOBUZ公式・記録 |
| プレイオプション | 配置、ゲージ、補助等の異なる設定を含む。例: RANDOMとAUTO SCRATCHは同じ作用ではない。作品別の詳細は参照先で確認する | [CastHour公式・オプション](https://p.eagate.573.jp/game/2dx/29/howto/play/option.html) |

SP/DPはSINGLE/DOUBLEのプレイ形式。具体的な入力配置の図や作品別の例外を新たに描く場合は
該当作品の公式操作図を確認する。既存の `Chart.play_type` と `difficulty` を別に表示し、DPを難易度種別と混同しない。
選曲は譜面の識別・記録の確認、プレイはゲーム映像を読む場面、リザルトは成績を振り返る場面として
デザインするが、どのcanvasを出すかは利用者のscreen filterとruntimeに従う。

## scorepeekが現在渡す値（ゲーム仕様の証拠とは別）

source anchorはrepository rootから解決する。実装が変わったらanchorを読み直す。

| 表示 | sourceと意味 |
| --- | --- |
| `OverlayState` | `crates/scorepeek-overlay/src/lib.rs`: chart、best、detail、history、system/result_signal、screenをskin ABI inputへ渡す |
| BEST | adapterの `runtime.rs` にある `refresh_history()`、`crates/scorepeek-core/src/scores/query.rs` の `chart_dashboard()`: 統合されたchart best。各値が同じ一回のplayから得られたとは限らない |
| RESULT DETAIL | `query.rs`: 保存playの最高EX、同点なら既知かつ少ないmiss、その後新しい記録を代表にする。`runtime.rs` が内訳を抽出。最新画面のリザルトと断定しない |
| DJ LEVEL | `runtime.rs` の `dj_level()`: N=notes、S=scoreとして `min(8, floor(9*max(S,0)/(2*N)))`。8/7/6/5/4/3/2/0–1をAAA/AA/A/B/C/D/E/Fへ対応。必要値なし/notes=0は中立表示 |
| クリア文字 | `runtime.rs` の `clear()`: 保存rank 0–7をNO PLAY/FAILED/ASSIST/EASY/CLEAR/HARD/EX HARD/FULL COMBOへ対応。wire literalとは別 |
| 履歴 | `query.rs`: 受信日時等による降順。skinがソートし直したり最高記録を再判定しない |
| グラフ | `runtime.rs`: EX/(2*notes)、MISS/notes。notes不明/0はplotしない。UIは0–100%へclipし、不明missの区間を接続しない。各playの値であり累積best曲線ではない |
| 軸 | `skins/shared/src/lib.rs`: DJ LEVEL境界と右側MISS RATE。missは大きいほど高い位置。成功率やaccuracyへ読み替えない。月・日時は親から渡される |
| SYSTEM | `crates/scorepeek-overlay/src/projection.rs` の `apply_status()`: watcher/session、catalog/model等の状態。ゲーム成績ではない |
| RESULT | `projection.rs`: inactive/provisional/confirmed/retractedをlampへ写す。DB保存完了や接続可否そのものではない。recorded indicatorとも別 |
| MISSとCB | `runtime.rs`: miss_countとcombo_breakは独立field。BAD+POORなどからskinが推定しない |
| プレイ条件 | `runtime.rs` が渡す表示文字列を保持。知らないoptionも消さない |

DJ LEVELの比率目安はAAA 8/9、AA 7/9、A 6/9、B 5/9、C 4/9、D 3/9、E 2/9、Fは2/9未満。
これは上記の現行実装との対応表。小数表示の88.889%などからrankを計算し直さない。
例: N=1000なら現行式でAAAの下限は1778、1777はAA。skinが計算を所有するわけではない。
AAAはDJ LEVELの最高区分であり満点ではない。FCも最高クリア区分でありAAAや満点を意味しない。

## 利用者の肌感と、誤った一般化

2026-09-08の合意: 想定する利用者層ではA/AA/AAAが頻出し、AAAが最高、Aが普段の実質下限、
B以下は譜面が適正範囲より難しい感覚。この肌感は文字の強弱を設計するためのもの。
全プレイヤー共通の技能判定、難易度推薦、否定的メッセージに使わない。
AAAとFCの特別な達成感は独立に設計し、共通のgold tokenへ自動集約しない。

組合せ検証: AAA+FAILED、AA+FC、A+EX HARD、BESTとdetailで値が異なる状態、
未取得MISSと実値0、未取得clearとNO PLAY/FAILED。互いを推定せず、それぞれ正しく表示する。
これらは表示contractを試す合成例であり、実在するplay記録とは主張しない。

## 不明事項の扱い

作品別ゲージ減少量、正確な判定時間幅、特殊ノーツ/optionの集計例外は本skillの固定知識にしない。
必要なら該当版の公式資料を確認し、資料・版・確認日・未確認範囲を追記する。
記事の存在だけで画像内の説明を読めたことにしない。game知識の追加で未実装の項目表示を勝手に増やさない。
