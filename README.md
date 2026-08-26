# npc-system

LLMを使わず、ルールベースのUtility AIでNPC、家族、関係、都市間移住、
出生・死亡、経済、災害、疫病、戦争を100年間進めるRust製CLIプロトタイプです。

## 拡張機能

経済システムはnpc-system本体から分離された `economy-extension` Cargo featureです。
年初、月次、NPC死亡前、年末のライフサイクルフックを通じてコアへ接続します。
互換性のため標準ビルドでは有効です。

```bash
# 経済拡張を含む通常実行（default feature）
cargo run --release -- simulate --towns 20 --population 5000 --years 100 --seed 12345

# 経済拡張を外したコアのみの実行
cargo run --release --no-default-features -- simulate \
  --towns 20 --population 5000 --years 100 --seed 12345

# featureを明示して実行
cargo run --release --no-default-features --features economy-extension -- simulate \
  --towns 20 --population 5000 --years 100 --seed 12345
```

CLIとJSONの `enabled_extensions` には、実際に有効な拡張機能が記録されます。

## 経済拡張

各NPCは所持金と商品在庫を持ち、各都市は財政、物価指数、生産性、雇用を持ちます。
月tickでは次の循環を処理します。

1. 都市の雇用力とNPCの能力から就業者・給与を決定
2. 所得税を都市財政へ留保し、給与をNPCへ支給
3. 困窮したNPCへパートナー・親が生活費を譲渡
4. NPCが都市市場から食料・衣料・医薬品・工具・嗜好品を購入
5. 商品ごとの需要と供給の差から単価を改定
6. 商品別指数の加重平均から総合物価を算出（インフレまたはデフレ）

死亡時の現金と商品は近親者へ相続され、相続人がいなければ都市財政へ戻ります。
災害・疫病・戦争による雇用低下は供給不足と物価上昇につながります。都市別に
経済力、域内総生産、住民資産、都市財政、物価変動、失業率、資産Gini係数を
確認でき、過度なインフレ・デフレ、失業、格差はwarningになります。
食料は都市の安全性、医薬品は教育、工具は雇用力、嗜好品は都市の豊かさなど、
品目ごとに供給条件が異なります。疫病による医薬品需要や、災害・戦争による
食料供給の減少も単価へ反映されます。
価格が上がると生産と都市間・外部輸入が増え、下がると生産が縮小するため、
価格は需給が釣り合う水準へ戻ります。無人化などで取引がなくなった市場は、
過去の危機価格へ張り付かず基準価格へ徐々に回帰します。
さらに品目ごとに年単位の豊作・不作、供給改善・供給障害が発生します。災害による
追加ショックと合わせて価格へ反映され、`goods[].supply_shock_basis_points` と
CLIの「供給変動」列から変動要因を確認できます。

ライブラリ利用時は `World::purchase` で商品と代金を原子的に交換し、
`World::transfer_money` でNPC間の譲渡を行えます。失敗した取引は残高や在庫を
変更しません。通貨は丸め誤差を避けるため最小単位（1/100通貨）の整数です。
都市の市場価格を使う購入には `World::purchase_at_market_price` を利用できます。

都市経済力と商品単価の年次推移をCLIで表示するには `--economy-history` を指定します。
`--economy-town` を繰り返すと対象都市を限定できます。

```bash
cargo run --release -- simulate \
  --towns 20 \
  --population 5000 \
  --years 100 \
  --seed 12345 \
  --summary-only \
  --economy-history \
  --economy-town 0 \
  --economy-town 3
```

通常の最終サマリーでも、全都市の商品別最終単価を表示します。JSONでは各年の
`town_economies[].economic_power_cents` と `town_economies[].goods[]` に、経済力、
商品別単価、騰落率、取引数量、取引総額を保存します。

## 商品売買ゲーム example

`economy-extension` を使った、1人用の年次ターン制商品売買ゲームを実行できます。
プレイヤーは開始資金を受け取り、都市の経済状況と過去の価格推移を確認しながら、
食料・衣料・医薬品・工具・嗜好品を売買します。最終年に全商品を時価評価し、
最終所持金、損益、収益率を表示します。

```bash
cargo run --release --example trade_game -- \
  --towns 5 \
  --population 500 \
  --years 10 \
  --warmup-years 10 \
  --seed 12345 \
  --town 0 \
  --starting-money 1000
```

ゲームは `ratatui` を使った全画面TUIで起動します。都市・年次、現金、総資産、
都市経済力、物価、市場価格、保有商品の時価、選択商品の価格推移、都市経済力の
推移を同じ画面で確認できます。100列×31行以上では推移グラフを含む完全表示、
一般的な80列×24行では市場と資産を中心にしたコンパクト表示へ自動で切り替わります。
開始時には既定でNPC経済だけを10年間進め、蓄積された商品価格と都市経済力の履歴を
グラフへ表示します。プレイヤーの資産と成績はこの事前期間の後から計算されます。
事前期間は `--warmup-years` で変更でき、`0` を指定すると無効にできます。

各年に利用できる主な操作は次のとおりです。売買は同じ年に複数回行えます。

```text
↑ / ↓ または J / K   商品を選択
1 ～ 5                商品を直接選択
← / → または - / +   売買数量を変更（Shift併用で10ずつ）
B / S                 選択商品を購入 / 売却
N または Enter        取引を終えて翌年へ進む
H                     全期間の経済履歴を表示
?                     操作ヘルプを表示
Q または Esc          ゲームの終了確認
```

`--starting-money` は通貨単位で指定し、内部では1/100通貨単位の整数として管理します。
取引は現在の都市市場価格で行われ、購入代金は都市財政へ入り、売却代金は都市財政から
支払われます。残高・在庫・都市財政が不足する取引は、どの資産も変更せず拒否されます。
`--no-default-features` から起動する場合は `--features trade-game` を指定してください。

## 実行

```bash
cargo run --release -- simulate \
  --towns 20 \
  --population 5000 \
  --years 100 \
  --seed 12345 \
  --world-danger normal
```

`--towns`、`--population`、`--years` は1以上、`--seed` は0以上の整数を
指定します。危険度は `peaceful`、`normal`、`harsh` から選択でき、未指定時は
`normal` です。

実行中は Year 0 と10年ごとの人口・出生・死亡・移住サマリを表示します。
指定年数が10の倍数でない場合も最終年を表示します。終了時には全期間の累積統計、
都市別人口、関係グラフの健全性指標、統計上のwarningを表示します。

長期実行で途中経過を省略する場合は `--summary-only` を指定します。

## NPC詳細表示

シミュレーション終了時のNPC情報を表示するには `--npc` へNPC IDを指定します。
複数のNPCを確認する場合はオプションを繰り返せます。

```bash
cargo run --release -- simulate \
  --towns 20 \
  --population 5000 \
  --years 100 \
  --seed 12345 \
  --npc 3185 \
  --npc 11023 \
  --summary-only
```

NPC詳細には生死・年齢・都市・所持金・商品在庫・能力・目標・信念・関係数に加え、パートナー、
親、祖父母、兄弟姉妹、子、孫を表示します。死亡者の年齢は死亡時、外部転出者の
年齢は転出時の値として明記します。指定IDが存在しない場合も統計結果は表示し、
NPC欄に利用可能なID範囲を表示します。

利用できる引数は次のコマンドでも確認できます。

```bash
cargo run -- simulate --help
```

## JSON出力

年次統計をJSONへ保存する場合は `--output` を追加します。

```bash
cargo run --release -- simulate \
  --towns 20 \
  --population 5000 \
  --years 100 \
  --seed 12345 \
  --world-danger normal \
  --output result.json
```

JSONには次の情報を保存します。全NPCの履歴や通常交流のログは保存しません。

- format version、seed、指定年数、使用した全設定
- 初期・最終の人口、延べNPC数、都市別人口と収容力
- 1年ごとの `YearStatistics`（都市別GDP、経済力、商品別単価・取引、雇用、格差を含む）
- 全期間の累積統計
- 関係グラフの健全性指標とwarning

実行時刻などの非決定的なメタデータは含めません。同じ設定とseedを指定した実行は、
同じ年次統計とJSON内容になります。

## 検証

```bash
cargo test
cargo test --release --test long_run -- --ignored
cargo run --release -- simulate --towns 20 --population 5000 --years 100 --seed 12345 --world-danger normal
```

実装は月tick（交流、Utility AI、パートナー、移住、疫病）と年tick（加齢、
出生、死亡、外部移住、災害、戦争、関係忘却）に分けています。候補探索は
同じ都市、既存関係、隣接都市の少数候補に限定し、全NPC総当たりを避けます。
