# SQLツール

PC を重くしないことを最優先にした、Mac / Windows 両対応の DB 操作デスクトップアプリ(Rust)。
設計の全体は `SQLツール 設計書.docx` を参照。

## 現状

設計書のフェーズ 0〜4 のうち、**実際のデータベースで動作を確かめられる範囲**を実装している。
UI は **GPUI Kit** に決定した(フェーズ 0 の検証。Mac でビルド・表示・日本語の描画を確認)。

| クレート | 責務 | 状態 |
|---|---|---|
| `sqlscan` | SQL の字句解析、文分割、先頭キーワード、安全確認の判定材料 | 実装済み |
| `result-store` | 結果の保持、一時ファイルへの退避、数値を f64 にしないソート | 実装済み |
| `driver-api` | Driver / Session / Dialect / MetadataProvider の定義 | 実装済み |
| `driver-postgres` | PostgreSQL ドライバ(tokio-postgres、rustls で TLS) | 実装済み |
| `driver-sqlite` | SQLite ドライバ(専用スレッド) | 実装済み |
| `storage` | アプリ内 SQLite、OS の資格情報ストア | 実装済み |
| `data-io` | CSV / Excel の書き出し、CSV の読み取り(Shift_JIS、区切り自動判定) | 実装済み |
| `ssh-tunnel` | SSH のローカルポート転送(russh、127.0.0.1 のみ、ホスト鍵を確認) | 実装済み |
| `core`(パッケージ名 `sqltool-core`) | セッション管理、実行、安全確認、編集、入出力、補完、実行計画、ER、コマンドとイベント | 実装済み |
| `app`(バイナリ名 `sqltool`) | 画面(GPUI Kit) | 実装済み |
| `test-pg` / `test-ssh` | 結合テスト用の PostgreSQL / 使い捨て sshd(テスト専用) | — |
| `cli` | `core` を手で動かす開発用 CLI(製品の UI ではない) | — |

### 機能とフェーズの対応

| フェーズ | 機能 | 状態 |
|---|---|---|
| 1 | 接続管理(本番ラベル、パスワードは OS の資格情報ストアだけ)、SQL 実行(選択範囲、キャンセル、タイムアウト)、結果グリッド(100 万行)、スキーマツリー | 済 |
| 2 | テーブル編集(プレビュー → 1 トランザクション、楽観的ロック)、CSV / Excel 入出力、履歴・スニペット、SQL 補完、TLS | 済 |
| 2 | SSH トンネル(秘密鍵 / パスワード、初回のホスト鍵の確認、変化したら接続しない) | 済 |
| 3 | SQLite | 済 |
| 3 | MySQL / MariaDB、SQL Server | **未実装**(下記) |
| 4 | 実行計画の可視化、ER 図 | 済 |
| 4 | Oracle | **未実装**(下記) |

## 起動

```sh
cargo run -p app                                   # 画面を起動する
scripts/dev-demo.sh --open "demo (dev)"            # 使い捨ての PostgreSQL とサンプルデータで起動する
```

起動オプション(`psql -c` のように、起動と同時に操作できる):

```text
--import-connections FILE   接続設定(エクスポートした JSON)を取り込む
--open NAME                 その名前の接続を開く
--run "SQL"                 接続を開いた後、SQL を全文実行する
--table スキーマ.表名        そのテーブルを開く(編集できる)
--explain "SQL"             実行計画(実測つき)を表示する
--er スキーマ名              ER 図を表示する
```

## アーキテクチャ

設計書のクレート構成(依存の向き)を、テストで守っている(`crates/core/tests/architecture.rs`)。

```text
app ──► core ──► driver-api ◄── driver-*
           ├────► sqlscan
           ├────► result-store
           ├────► storage
           ├────► data-io
           └────► ssh-tunnel ──► driver-api(Secret 型だけ)
```

* 下位のクレート(`sqlscan`、`result-store`、`driver-api`、`storage`、`data-io`)は、ほかのワークスペース内クレートに依存しない。
* `core` は具体的なドライバを知らない(DB 名では分岐せず、`Capabilities` と `Dialect` を使う)。
  ドライバは起動する側(`app`)が `DriverRegistry` に登録する。
* UI は `core` のコマンドとイベントだけを使い、`sqlscan` / `result-store` / `storage` を直接触らない。
* 開発用の `sqltool-cli` だけは、使い捨ての DB のために `test-pg` に依存する。製品の層は、`test-pg` / `test-ssh` に依存しない。
* **ソースファイルは 1 つ 500 行まで**(同じテストが強制する)。超えたら責務ごとにモジュールを分ける。上限は上げない。

### `core` の内部

| モジュール | 責務 |
|---|---|
| `types` | UI との契約(`Command` / `Event` と関連する型) |
| `core` | 入口(`Core`)、状態、コマンドの振り分けだけ |
| `connections` | 接続設定の保存・削除・テスト、パスワード |
| `tabs` / `tab` | タブ(セッション)の開閉、再接続、読み取り専用 |
| `execution` | 実行の入口(文の決定、安全確認、利用者への確認)、キャンセル、取得の制御 |
| `batch` | 文を順に実行するループ、後始末(トランザクション、履歴、スキーマ) |
| `outcome` | 1 つの文の実行結果の判定。**純粋関数**(DB なしで単体テスト) |
| `plan` | 実行する文の決定、安全確認、トランザクション状態。**純粋関数** |
| `sink` / `result` / `results` | 結果の受け取り(上限・進捗)、`ResultHandle`、並べ替え |
| `schema` / `browse` | スキーマのキャッシュと、メタデータ用の別接続 |
| `history` / `storage_actor` | 履歴の記録、SQLite を動かす専用スレッド |

判断ロジック(`plan`、`outcome`)と、非同期の進行(`execution`、`batch`)を分けてあり、
前者は DB なしで網羅的にテストできる。

### 開発用 CLI

```sh
cargo run -p sqltool-cli -- --demo      # 使い捨ての PostgreSQL とサンプルデータで対話
```

製品の UI ではなく、`core` を手で動かして確かめるためのもの。

## 使い方(core)

UI は `Core` にコマンドを送り、イベントを受け取る。

```rust
let (core, mut events) = Core::start(config, tokio::runtime::Handle::current())?;
core.send(Command::OpenTab { connection_id });
// ... Event::TabOpened { tab, .. } を受け取ったら
core.send(Command::Execute { tab, sql, target: ExecuteTarget::Cursor { offset }, options: Default::default() });
// Event::ResultOpened { result, .. } の ResultHandle から、表示範囲の行だけを読む
let cell = result.with_row(position, |row| row.cell(0).map(|b| b.to_vec()));
```

* `ResultHandle::with_row` はファイル I/O をしない。退避済みの行は `Access::Loading` を返し、読み戻しが
  終わると `set_on_ready` で通知する(UI スレッドは止まらない)。
* ドライバは、起動する側が `DriverRegistry` に登録する(`driver-*` は Cargo feature で切り替える設計)。

## 開発

```sh
cargo build --workspace
cargo test --workspace
cargo clippy --workspace --all-targets
```

### 結合テストの DB

実際の PostgreSQL が必要なテストは、次の順で接続先を決める(`crates/test-pg`)。

1. `SQLTOOL_TEST_PG_HOST`(と `_PORT`、`_USER`、`_PASSWORD`)があれば、そのサーバーを使う(CI 用)。
2. なければ、PATH や Homebrew などから `initdb` を探し、一時ディレクトリに使い捨てのクラスタを作る
   (Unix のみ。テストの終了時に自動で停止・削除される)。`SQLTOOL_PG_BIN_DIR` で場所を指定できる。
3. どちらも使えなければ、そのテストは飛ばす。**飛ばさず失敗させる**には `SQLTOOL_REQUIRE_PG=1`。

実 Keychain / 資格情報マネージャーを操作するテストは、通常は実行しない(`#[ignore]`)。
手動で行うには `cargo test -p storage -- --ignored keyring`。

### 性能の計測

```sh
cargo run --release -p result-store --example bench                 # 結果ストアだけ
cargo run --release -p sqltool-core --example bench_pg -- 512 spill # PostgreSQL から core 経由(100 万行 20 列)
```

Apple M4 Pro(macOS)での実測。いずれも `core` 以下だけの値で、UI の描画は含まない。

| 項目 | 結果 |
|---|---|
| 100 万行 20 列の受信(サーバー → 結果ストア) | 約 1.6 秒 |
| 結果ストアのメモリ(100 万行 20 列) | 233 MiB |
| 同、上限 128 MiB で退避あり | 常駐 127 MiB + 退避 105 MiB、時間は変わらず |
| 表示範囲の行の読み出し(`with_row`) | 1000 行で約 0.5 ms |
| 数値列のソート(100 万行) | 約 0.1 秒 |

## 設計書から意図的に変えた点・補った点

設計書に書かれていない判断や、実装してみて分かった事情による変更。

1. **`Dialect` は `sqlscan::Rules` ではなく `LexicalFeatures` を返す。** 設計書の依存図では `driver-api` の
   依存先が「なし」のため。`core` が機能のフラグを `sqlscan::Rules` に変換する(DB 名では分岐しない)。
2. **`Driver::connect_metadata` を追加し、`MetadataProvider` に `indexes` と `close` を足した。**
   「メタデータ取得は別接続」を表すため、また、スキーマツリーに索引が必要なため。
3. **結果の列の型は、実行前に Parse だけを行って得る。** `simple_query` は列名しか返さず、数値列の
   ソート・右寄せ・バイナリ表示に必要な型が分からないため。Parse に失敗しても、本実行が同じ原因の
   正しいエラー(位置つき)を返す。
4. **トランザクションの状態は、実行した文の種類から追跡する。** tokio-postgres が ReadyForQuery の
   状態を公開しないため。`COMMIT AND CHAIN` は追えない(次の COMMIT / ROLLBACK で正しくなる)。
5. **一時ファイルは名前を残さない。** Unix は作成直後に unlink、Windows は `DELETE_ON_CLOSE`。
   設計書の「終了時と次回起動時に削除」を、異常終了でも残らない形で満たす。
6. **キャンセルは、文が終わるまで再送する。** 実行の直後に送った停止要求が、クエリがサーバーに届く前に
   着くと無視される競合があったため(実機のテストで再現して修正済み)。
7. **`set_config(...)` を呼ぶ文は、読み取り専用モードで拒否する。** SELECT でも、読み取り専用の設定を
   解除できてしまうため。
8. **実行履歴の記録は実装済み。** 一覧は設計書ではフェーズ 2 だが、「パスワードを含みうる文を保存しない」
   規則を最初から保証するため。

## 未実装・要確認

### 未実装

* **MySQL / MariaDB、SQL Server、Oracle**(フェーズ 3・4)。開発環境に検証できるサーバーがなく、
  「実際のデータベースで確かめていない接続・実行・キャンセル・読み取り専用の保証」は出荷できないため、
  コードを書いていない。サーバーを用意できれば、`driver-api` の `Driver` / `Dialect` を実装し、
  `core` の結合テスト(`crates/core/tests/e2e`)を同じ形で流せる。
* 設計書の未解決事項(配布範囲、Oracle の要否、SQL Server の Windows 統合認証、インストーラー作成と
  自動更新の方式)は、この実装では扱っていない。

### 確かめていない・人の確認が必要

* **日本語 IME の入力**: GPUI のソースで IME の経路(marked text など)は確認したが、実機での入力は未確認。
  `scripts/dev-demo.sh` で起動し、エディタ・接続フォーム・セル編集で変換候補が出るか確かめること。
* **Windows**: `storage` / `core` / `app` は C 依存のため、Mac からクロスチェックできていない。
  `.github/workflows/ci.yml` を用意したが、まだ実行していない。
* **実 Keychain / 資格情報マネージャー**: `KeyringSecretStore` の実機テストは手動(上記)。
* **SSH**: 鍵認証、ホスト鍵の確認・拒否、秘密の保管は、使い捨ての sshd で確かめた。**パスワード認証**は
  sshd(PAM が要る)を用意できず、実サーバーでは未確認。SSH 経由で TLS(証明書を検証)を使う場合の、
  ホスト名の検証も未確認(`host` は検証名に残し、`hostaddr` で 127.0.0.1 に接続する作り)。
  接続フォームの SSH の項目は、描画までのテストはあるが、画面で目視していない。
* **Testcontainers**: 設計書は結合テストに Testcontainers を挙げているが、開発環境で Docker を起動できず
  動作を確かめられなかったため入れていない。代わりに、上記の環境変数(CI のサービスコンテナ)と
  ローカルの使い捨てクラスタを使う。
