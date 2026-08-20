# npc-system

LLMを使わず、ルールベースのUtility AIでNPC、家族、関係、都市間移住、
出生・死亡、災害、疫病、戦争を100年間進めるRust製CLIプロトタイプです。

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
- 1年ごとの `YearStatistics`
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
