# npc-system

LLMを使わず、ルールベースのUtility AIでNPC、家族、関係、都市間移住、
出生・死亡、災害、疫病、戦争を指定期間または無期限に進めるRust製CLIプロトタイプです。

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
`normal` です。`--years` を省略した場合は無期限モードになります。

実行中は Year 0 と10年ごとの人口・出生・死亡・移住サマリを表示します。
指定年数が10の倍数でない場合も最終年を表示します。終了時には全期間の累積統計、
都市別人口、関係グラフの健全性指標、統計上のwarningを表示します。

長期実行で途中経過を省略する場合は `--summary-only` を指定します。

## 無期限実行と監視

`--years` を付けずに起動すると、プロセスが停止されるまで年次処理を継続します。
無期限モードでは年次履歴を最新1件だけに制限し、累積統計は別途保持します。

```bash
cargo run --release -- simulate \
  --towns 20 \
  --population 5000 \
  --seed 12345 \
  --world-danger normal \
  --summary-only
```

無期限モードは既定で `npc-system-status.json` を原子的に更新します。別のターミナルで
次を実行すると、現在年、人口、稼働時間、warning、最終更新からの経過時間を監視
できます。更新が既定の30秒を超えて止まると `stale（更新停止）` と表示します。

```bash
cargo run --release -- status --watch
```

statusファイルや更新頻度を変える場合は `--status-file FILE` と
`--status-interval-years N` を指定します。監視側では `--file FILE`、
`--interval-seconds N`、`--stale-after-seconds N` を利用できます。期間指定実行でも
`--status-file` を明示すれば同じ監視方法を利用でき、完了時は `completed` になります。

無期限モードでは終了時にだけ生成できる `--output`、`--npc`、各タイムライン指定は
使用できません。強制終了後にstatusファイルが残っていても、最終更新時刻によって
停止を検知できます。

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

NPC詳細には生死・年齢・都市・能力・目標・信念・関係数に加え、パートナー、
親、祖父母、兄弟姉妹、子、孫を表示します。死亡者の年齢は死亡時、外部転出者の
年齢は転出時の値として明記します。指定IDが存在しない場合も統計結果は表示し、
NPC欄に利用可能なID範囲を表示します。

## タイムライン表示

世界、都市、NPCの変化を時系列で確認できます。各オプションは同時に利用でき、
都市とNPCは複数回指定できます。

```bash
cargo run --release -- simulate \
  --towns 20 \
  --population 5000 \
  --years 100 \
  --seed 12345 \
  --summary-only \
  --timeline-world \
  --timeline-town 5 \
  --timeline-npc 3185
```

- `--timeline-world`: 10年ごとの年次指標と、災害・疫病・戦争・飢饉が起きた年
- `--timeline-town ID`: 人口、出生、死亡、転入、転出、パートナー成立、都市災害
- `--timeline-npc ID`: 出生、外部流入、移住、パートナー、子、目標、信念、死亡

年初の処理は「N年」、月次処理は「N年MM月」として表示します。タイムライン用の
年次イベントはオプション指定時だけ収集し、毎年リセットするため、長期実行でも
全履歴をWorldへ保持し続けません。

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
