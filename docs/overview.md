# sqltool

A desktop database client for Mac and Windows (Rust) whose top priority is not to weigh down the PC.
The overall design is in the design document (`SQLツール 設計書.docx`, not included in this repository).

## Status

Of phases 0–4 in the design document, the parts that **can be verified against a real database** are implemented.
The UI is **GPUI Kit** (decided in phase 0: build, rendering and Japanese text rendering were confirmed on Mac).

| Crate | Responsibility | Status |
|---|---|---|
| `sqlscan` | SQL lexing, statement splitting, leading keyword, inputs for safety checks | Done |
| `result-store` | Holding results, spilling to temp files, sorting without converting numbers to f64 | Done |
| `driver-api` | Definitions of Driver / Session / Dialect / MetadataProvider | Done |
| `driver-postgres` | PostgreSQL driver (tokio-postgres, TLS via rustls) | Done |
| `driver-sqlite` | SQLite driver (dedicated thread) | Done |
| `driver-mysql` | MySQL / MariaDB driver (mysql_async, text protocol, TLS via rustls) | Done |
| `driver-sqlserver` | SQL Server driver (tiberius, TLS via rustls) | Done |
| `driver-oracle` | Oracle driver (official pure-Rust thin driver `oracledb`, **beta**; no Instant Client needed) | Done |
| `storage` | In-app SQLite, OS credential store | Done |
| `data-io` | CSV / Excel export, CSV import (Shift_JIS, delimiter auto-detection) | Done |
| `er-design` | ER design: model, text notation, `CREATE` SQL for 5 databases, auto layout, SVG, specification documents (HTML / Excel). Pure logic, no I/O | Done |
| `ssh-tunnel` | SSH local port forwarding (russh, 127.0.0.1 only, host key verification) | Done |
| `core` (package `sqltool-core`) | Session management, execution, safety checks, editing, import/export, completion, execution plans, ER, commands and events | Done |
| `app` (binary `sqltool`) | UI (GPUI Kit) | Done |
| `test-pg` / `test-mysql` / `test-sqlserver` / `test-oracle` / `test-ssh` | PostgreSQL / MySQL / SQL Server / Oracle / throwaway sshd for integration tests (test only) | — |
| `cli` | Development CLI to drive `core` by hand (not the product UI) | — |

### Features by phase

| Phase | Feature | Status |
|---|---|---|
| 1 | Connection management (production label, passwords only in the OS credential store), SQL execution (selection, cancel, timeout), result grid (1M rows), schema tree | Done |
| 2 | Table editing (preview → one transaction, optimistic locking), CSV / Excel import/export, SQL completion, TLS | Done |
| 2 | SSH tunnel (private key / password, host key confirmation on first use, refuse to connect if it changes) | Done |
| 3 | SQLite | Done |
| 3 | MySQL / MariaDB, SQL Server | Done |
| 4 | Execution plan visualization (PostgreSQL, MySQL, MariaDB), ER diagram | Done |
| 4 | Oracle | Done (see the limits below) |
| 5 | ER design (see below) | Done |

## Running

```sh
cargo run -p app                                   # start the app
scripts/dev-demo.sh --open "demo (dev)"            # start with a throwaway PostgreSQL and sample data
```

Startup options (act at launch, like `psql -c`):

```text
--import-connections FILE   import connection settings (exported JSON)
--open NAME                 open the connection with that name
--run "SQL"                 after opening the connection, run the whole SQL
--table schema.table        open that table (editable)
--er SCHEMA                 show the ER diagram
--design FILE               open the ER designer with that file (.erd)
```

## ER design

The **ER設計** button opens a designer that needs no connection. The screen is the diagram alone, as large as the window allows;
the design is one model that can also be seen as a text notation (one line per column, opened with **テキスト** on the left) and
the two stay in sync. Everything is undoable; the design is kept when the screen is closed and an unsaved draft is restored next time.

Editing happens on the diagram itself:

- click a table or column name to edit it in place (Enter / click elsewhere to confirm, Tab for the next column, Esc to cancel; references follow renames); click a type to pick it from a list (type anything else with 直接入力…);
- **＋** in a table header adds a column and starts editing its name;
- double-click a table header (or **詳細**) for a dialog with every item: names, logical names, types, defaults, notes,
  PK / NOT NULL / UNIQUE, order, delete, table size;
- double-click a relation line for its properties: constraint name, ON DELETE / ON UPDATE, parent required (child columns NOT NULL), one-to-one (child columns unique), delete;
- drag a header to move a table (it follows the pointer, no grid), drag its bottom-right corner (the cursor becomes a resize arrow) to give it room: the box gets wider and taller with the extra room left as blank space (the column rows grow only slightly; the text and the header row keep their size) (100 % to 250 %, stored as `@x,y,scale`);
- zoom the whole diagram with − / + / 全体 or ⌘ (Ctrl) + scroll (20 % to 300 %).

CREATE SQL, the check results and the document export are not on the main screen: **SQL・書き出し…** opens them in a dialog
(the SQL is generated only while it is open).

```text
customers "顧客"
  id    int pk auto "顧客ID"
  name  varchar(100) not null "氏名"
orders "注文"
  id          bigint pk auto
  customer_id int not null -> customers.id on delete cascade
  index (customer_id)
```

| | |
|---|---|
| Create tables and relations | type the notation, or **＋ 表を追加** / **関係線を引く** (click a child table, then its parent: the foreign key column is added for you; or click two columns). |
| `CREATE` SQL | for PostgreSQL / MySQL·MariaDB / SQLite / SQL Server / Oracle from one logical design (types, identity, defaults, comments, dependency order, circular references via `ALTER TABLE`). Save as `.sql`, copy, or put it in the SQL editor of the open connection. Things a database cannot express are listed as warnings, not dropped silently. |
| Specification documents | **Excel** (cover, table list, relation list, one sheet per table, hyperlinks) and **HTML** (self-contained, with the ER diagram as SVG, printable); the ER diagram alone as SVG. Logical names (論理名), notes, cardinality are included. |
| From an existing database | **設計** next to a schema in the tree turns its tables, keys and indexes into a design. |

The `CREATE` SQL is verified against real PostgreSQL, SQLite, MySQL, MariaDB, SQL Server and Oracle
(`crates/core/tests/e2e/design.rs`: run it twice, read the tables back, check keys, relations and `ON DELETE CASCADE`).

## Architecture

The crate layout from the design document (direction of dependencies) is enforced by a test (`crates/core/tests/architecture.rs`).

```text
app ──► core ──► driver-api ◄── driver-*
           ├────► sqlscan
           ├────► result-store
           ├────► storage
           ├────► data-io
           ├────► er-design
           └────► ssh-tunnel ──► driver-api (Secret type only)
```

* Lower crates (`sqlscan`, `result-store`, `driver-api`, `storage`, `data-io`, `er-design`) depend on no other workspace crate.
* `core` knows no concrete driver (it never branches on a DB name; it uses `Capabilities` and `Dialect`).
  The caller (`app`) registers drivers in `DriverRegistry`.
* The UI uses only `core` commands and events, and never touches `sqlscan` / `result-store` / `storage` directly.
* Only the development `sqltool-cli` depends on `test-pg`, for a throwaway DB. Product layers do not depend on `test-*` crates.
* **A source file is at most 500 lines** (enforced by the same test). When a file exceeds it, split it by responsibility. The limit is not raised.

### Inside `core`

| Module | Responsibility |
|---|---|
| `types` | Contract with the UI (`Command` / `Event` and related types) |
| `core` | Entry point (`Core`), state, command dispatch only |
| `connections` | Saving, deleting and testing connection settings, passwords |
| `tabs` / `tab` | Opening and closing tabs (sessions), reconnecting, read-only mode |
| `execution` | Entry to execution (choosing statements, safety checks, asking the user), cancel, fetch control |
| `batch` | The loop that runs statements in order, cleanup (transaction, history, schema) |
| `outcome` | Judging the outcome of one statement. **Pure functions** (unit-tested without a DB) |
| `plan` | Choosing statements to run, safety checks, transaction state. **Pure functions** |
| `sink` / `result` / `results` | Receiving results (limit, progress), `ResultHandle`, sorting |
| `schema` / `browse` | Schema cache and the separate connection for metadata |
| `history` / `storage_actor` | Recording history, the dedicated thread that runs SQLite |

Decision logic (`plan`, `outcome`) is kept apart from asynchronous progress (`execution`, `batch`),
so the former can be tested exhaustively without a DB.

### Development CLI

```sh
cargo run -p sqltool-cli -- --demo      # interactive, with a throwaway PostgreSQL and sample data
```

This is not the product UI; it exists to drive `core` by hand and check it.

## Usage (core)

The UI sends commands to `Core` and receives events.

```rust
let (core, mut events) = Core::start(config, tokio::runtime::Handle::current())?;
core.send(Command::OpenTab { connection_id });
// ... once Event::TabOpened { tab, .. } arrives
core.send(Command::Execute { tab, sql, target: ExecuteTarget::Cursor { offset }, options: Default::default() });
// From the ResultHandle in Event::ResultOpened { result, .. }, read only the visible rows
let cell = result.with_row(position, |row| row.cell(0).map(|b| b.to_vec()));
```

* `ResultHandle::with_row` does no file I/O. A spilled row returns `Access::Loading`, and `set_on_ready`
  notifies when it has been read back (the UI thread never blocks).
* The caller registers drivers in `DriverRegistry` (`driver-*` are designed to be switched by Cargo features).

## Development

```sh
cargo build --workspace
cargo test --workspace
cargo clippy --workspace --all-targets
```

### Database for integration tests

Tests that need a real database skip when none is available; set `SQLTOOL_REQUIRE_<DB>=1` (`PG`, `MYSQL`, `MSSQL`, `ORACLE`) to **fail instead of skipping**.

PostgreSQL picks the server in this order (`crates/test-pg`).

1. If `SQLTOOL_TEST_PG_HOST` (and `_PORT`, `_USER`, `_PASSWORD`) is set, that server is used (for CI).
2. Otherwise, `initdb` is looked up in PATH, Homebrew, etc., and a throwaway cluster is created in a temp directory
   (Unix only; it is stopped and deleted automatically when the test ends). `SQLTOOL_PG_BIN_DIR` sets the location.
3. If neither is available, the test is skipped.

MySQL / MariaDB (`crates/test-mysql`) works the same way: `SQLTOOL_TEST_MYSQL_HOST` (and `_PORT`, `_USER`, `_PASSWORD`) points at a
server such as a MariaDB container; otherwise a throwaway `mysqld` is started in a temp directory (`SQLTOOL_MYSQLD` sets the path).
SQL Server and Oracle cannot be started locally, so `crates/test-sqlserver` (`SQLTOOL_TEST_MSSQL_*`) and `crates/test-oracle`
(`SQLTOOL_TEST_ORACLE_*`, including `_SERVICE`) need a server, for example the `mcr.microsoft.com/mssql/server` and
`gvenzl/oracle-free` containers. Each test gets its own database (or Oracle user), created and dropped by the test crate.
On Apple silicon, the SQL Server image needs a VM with Rosetta (Docker Desktop with Rosetta, or Colima with `--vz-rosetta`).

Tests that touch the real Keychain / Credential Manager are not run normally (`#[ignore]`).
To run them by hand: `cargo test -p storage -- --ignored keyring`.

### Performance measurements

```sh
cargo run --release -p result-store --example bench                 # result store only
cargo run --release -p sqltool-core --example bench_pg -- 512 spill # from PostgreSQL through core (1M rows, 20 columns)
```

Measured on an Apple M4 Pro (macOS). All values cover `core` and below only, and do not include UI rendering.

| Item | Result |
|---|---|
| Receiving 1M rows × 20 columns (server → result store) | about 1.6 s |
| Result store memory (1M rows × 20 columns) | 233 MiB |
| Same, with a 128 MiB limit and spilling | 127 MiB resident + 105 MiB spilled, same time |
| Reading the visible rows (`with_row`) | about 0.5 ms for 1000 rows |
| Sorting a numeric column (1M rows) | about 0.1 s |

## Deliberate changes and additions to the design document

Decisions not written in the design document, or changes forced by what implementation revealed.

1. **`Dialect` returns `LexicalFeatures`, not `sqlscan::Rules`.** In the design document's dependency diagram,
   `driver-api` has no dependencies. `core` converts the feature flags into `sqlscan::Rules` (it never branches on a DB name).
2. **Added `Driver::connect_metadata`, and added `indexes` and `close` to `MetadataProvider`.**
   To express "metadata is fetched on a separate connection", and because the schema tree needs indexes.
3. **Column types of a result are obtained by running Parse only, before execution.** `simple_query` returns only column names,
   so the types needed for numeric sorting, right alignment and binary display are unknown. If Parse fails,
   the real execution returns the same, correct error (with a position).
4. **Transaction state is tracked from the kinds of statements executed.** tokio-postgres does not expose the
   ReadyForQuery state. `COMMIT AND CHAIN` cannot be tracked (it becomes correct at the next COMMIT / ROLLBACK).
5. **Temp files keep no name.** On Unix they are unlinked right after creation; on Windows they use `DELETE_ON_CLOSE`.
   This meets the design document's "delete at exit and at next startup" in a form that leaves nothing behind even after a crash.
6. **Cancel is resent until the statement finishes.** A stop request sent right after execution could arrive before the
   query reached the server and be ignored (a race reproduced and fixed with a real-server test).
7. **Statements that call `set_config(...)` are rejected in read-only mode.** Even a SELECT could
   turn the read-only setting off.
8. **Recording of execution history is implemented.** The list view is phase 2 in the design document, but this
   guarantees from the start the rule "do not store statements that may contain passwords".

## Not implemented / needs checking

### Limits of each database

* **MySQL / MariaDB**: DDL is committed implicitly (`transactional_ddl` is false). Syntax-error positions are estimated from the quoted
  text in the message. Execution plans: MySQL uses `EXPLAIN FORMAT=TREE` / `EXPLAIN ANALYZE` (8.0.18+), MariaDB uses
  `FORMAT=JSON` / `ANALYZE FORMAT=JSON`; the session picks the statement from the server's version string. `Verify` TLS trusts the bundled
  web roots plus `ca_file`, not the OS store (a server's default self-signed certificate is refused; use `ca_file` or "skip verify").
* **SQL Server**: no session-wide read-only mode (production-label protection relies on the statement check in `core`). **Cancel and fetch-limit
  stop reconnect**: tiberius 0.13's cancel leaves the connection out of step, so the connection is closed and reopened (an uncommitted
  transaction is rolled back by the server and the error says so). Execution plans are not supported. Windows integrated authentication is not supported.
* **Oracle**: `oracledb` is a beta crate. It has no cancel API, so cancel kills the session from a second connection
  (`ALTER SYSTEM KILL SESSION`, needs that privilege; the tab must be reopened afterwards). No session-wide read-only mode. Each statement is committed
  unless a transaction is started explicitly (`SAVEPOINT` / `SET TRANSACTION`). `SkipVerify`, `ca_file` and TLS over an SSH tunnel are refused (never
  silently downgraded). Types such as JSON and VECTOR show `(未対応の型: …)`. NUMBER has no trailing zeros. Statement splitting does not understand PL/SQL blocks or
  `q'[...]'` quoting. Execution plans are not supported.
* Open issues in the design document (distribution scope, Windows integrated authentication for SQL Server,
  how to build installers and auto-update) are not covered by this implementation.

### Unverified / needs a human check

* **Japanese IME input**: The IME path in the GPUI source (marked text, etc.) was checked, but input on a real machine was not.
  Start with `scripts/dev-demo.sh` and check that conversion candidates appear in the editor, the connection form and cell editing.
* **Windows**: `storage` / `core` / `app` have C dependencies and could not be cross-checked from a Mac.
  `.github/workflows/ci.yml` is provided but has not been run yet.
* **Real Keychain / Credential Manager**: the real-machine test of `KeyringSecretStore` is manual (see above).
* **SSH**: key authentication, host key confirmation and refusal, and secret storage were checked with a throwaway sshd.
  **Password authentication** is unverified against a real server (sshd needs PAM, which was not available). Host name
  verification when using TLS (verifying the certificate) over SSH is also unverified (`host` stays as the verification name and the
  connection goes to 127.0.0.1 via `hostaddr`). The SSH fields of the connection form have rendering tests but were not inspected visually.
* **Servers other than the ones tested**: PostgreSQL 14 (local) and 16 (CI), MySQL 9 (local), MariaDB 11.8 (Docker), SQL Server 2022 (Docker, Rosetta),
  Oracle Free 23 (Docker) were used. Older versions, Autonomous DB / Azure SQL, and real TLS to Oracle were not tried.
* **CI**: the service containers for MySQL, MariaDB, SQL Server and Oracle in `.github/workflows/ci.yml` have not run on GitHub Actions.
* **Testcontainers**: the design document lists Testcontainers for integration tests, but Docker could not be started in the
  development environment and it could not be verified, so it is not included. Instead, the environment variables above (a CI service container)
  and a local throwaway cluster are used.

## Screen layout

The top bar has only: Connect, the connection tabs, ER design, Query input, and Run (which turns into Stop while a
statement is running). The Query input opens above the results, with tabs (＋ adds one) so several SQL texts can be kept side by side
(Cmd/Ctrl + Enter runs the statement at the cursor, or the selection). History, snippets, the execution-plan view, "run all" and the read-only toggle
were removed from the screen; `core` still keeps the execution history, and production connections stay read-only.

Right-clicking a row in the schema tree opens a menu grouped by feature:

* schema: **Diagram** (import into the ER design, ER diagram)
* table: **View** (first 100 rows / all rows), **Make SQL** (SELECT / INSERT / UPDATE / DELETE templates, each in a new Query tab);
  "first 100 rows" / "all rows" also show their SELECT in a new Query tab, **Data** (CSV import)

In the ER design, the drawing tools (add table, relation line, auto layout) are a large rail on the left, separated
by a line from the text toggle. A selected table shows a small x at the top right of its header, and every column
row has an x at the right of its type.
