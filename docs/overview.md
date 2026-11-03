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
| `storage` | In-app SQLite, OS credential store | Done |
| `data-io` | CSV / Excel export, CSV import (Shift_JIS, delimiter auto-detection) | Done |
| `ssh-tunnel` | SSH local port forwarding (russh, 127.0.0.1 only, host key verification) | Done |
| `core` (package `sqltool-core`) | Session management, execution, safety checks, editing, import/export, completion, execution plans, ER, commands and events | Done |
| `app` (binary `sqltool`) | UI (GPUI Kit) | Done |
| `test-pg` / `test-ssh` | PostgreSQL / throwaway sshd for integration tests (test only) | — |
| `cli` | Development CLI to drive `core` by hand (not the product UI) | — |

### Features by phase

| Phase | Feature | Status |
|---|---|---|
| 1 | Connection management (production label, passwords only in the OS credential store), SQL execution (selection, cancel, timeout), result grid (1M rows), schema tree | Done |
| 2 | Table editing (preview → one transaction, optimistic locking), CSV / Excel import/export, history and snippets, SQL completion, TLS | Done |
| 2 | SSH tunnel (private key / password, host key confirmation on first use, refuse to connect if it changes) | Done |
| 3 | SQLite | Done |
| 3 | MySQL / MariaDB, SQL Server | **Not implemented** (see below) |
| 4 | Execution plan visualization, ER diagram | Done |
| 4 | Oracle | **Not implemented** (see below) |

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
--explain "SQL"             show the execution plan (with actual timings)
--er SCHEMA                 show the ER diagram
```

## Architecture

The crate layout from the design document (direction of dependencies) is enforced by a test (`crates/core/tests/architecture.rs`).

```text
app ──► core ──► driver-api ◄── driver-*
           ├────► sqlscan
           ├────► result-store
           ├────► storage
           ├────► data-io
           └────► ssh-tunnel ──► driver-api (Secret type only)
```

* Lower crates (`sqlscan`, `result-store`, `driver-api`, `storage`, `data-io`) depend on no other workspace crate.
* `core` knows no concrete driver (it never branches on a DB name; it uses `Capabilities` and `Dialect`).
  The caller (`app`) registers drivers in `DriverRegistry`.
* The UI uses only `core` commands and events, and never touches `sqlscan` / `result-store` / `storage` directly.
* Only the development `sqltool-cli` depends on `test-pg`, for a throwaway DB. Product layers do not depend on `test-pg` / `test-ssh`.
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

Tests that need a real PostgreSQL pick the server in this order (`crates/test-pg`).

1. If `SQLTOOL_TEST_PG_HOST` (and `_PORT`, `_USER`, `_PASSWORD`) is set, that server is used (for CI).
2. Otherwise, `initdb` is looked up in PATH, Homebrew, etc., and a throwaway cluster is created in a temp directory
   (Unix only; it is stopped and deleted automatically when the test ends). `SQLTOOL_PG_BIN_DIR` sets the location.
3. If neither is available, the test is skipped. Set `SQLTOOL_REQUIRE_PG=1` to **fail instead of skipping**.

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

### Not implemented

* **MySQL / MariaDB, SQL Server, Oracle** (phases 3 and 4). There is no server in the development environment to verify them,
  and connection, execution, cancel and read-only guarantees that have not been checked against a real database cannot be shipped,
  so no code was written. Given a server, implement `Driver` / `Dialect` from `driver-api` and run
  the `core` integration tests (`crates/core/tests/e2e`) in the same way.
* Open issues in the design document (distribution scope, whether Oracle is needed, Windows integrated authentication for SQL Server,
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
* **Testcontainers**: the design document lists Testcontainers for integration tests, but Docker could not be started in the
  development environment and it could not be verified, so it is not included. Instead, the environment variables above (a CI service container)
  and a local throwaway cluster are used.
