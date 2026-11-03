#!/bin/sh
# 開発用: 使い捨ての PostgreSQL(サンプルデータつき)を用意し、そこへの接続設定つきでアプリを起動する。
#   scripts/dev-demo.sh [アプリへの引数...]       例: --open "demo (dev)" --run "select * from customers"
# データは /tmp/sqltool-demo に置く(SQLTOOL_DEMO_DIR で変更)。止めるには scripts/dev-demo.sh --stop-db。
set -eu
ROOT=${SQLTOOL_DEMO_DIR:-/tmp/sqltool-demo}
PORT=${SQLTOOL_DEMO_PORT:-54329}
REPO=$(cd "$(dirname "$0")/.." && pwd)

BIN=${SQLTOOL_PG_BIN_DIR:-}
if [ -z "$BIN" ]; then
  for d in $(command -v initdb >/dev/null 2>&1 && dirname "$(command -v initdb)") /opt/homebrew/opt/postgresql@*/bin /usr/local/opt/postgresql@*/bin /usr/lib/postgresql/*/bin; do
    [ -x "$d/initdb" ] && BIN=$d
  done
fi
[ -n "$BIN" ] || { echo "PostgreSQL(initdb)が見つかりません。SQLTOOL_PG_BIN_DIR で指定してください" >&2; exit 1; }

if [ "${1:-}" = "--stop-db" ]; then "$BIN/pg_ctl" -D "$ROOT/pg" -m immediate stop || true; exit 0; fi

mkdir -p "$ROOT"
[ -d "$ROOT/pg" ] || "$BIN/initdb" -D "$ROOT/pg" -U postgres --auth=trust -E UTF8 --locale=C >/dev/null
if ! "$BIN/pg_ctl" -D "$ROOT/pg" status >/dev/null 2>&1; then
  "$BIN/pg_ctl" -D "$ROOT/pg" -w -l "$ROOT/pg.log" -o "-p $PORT -c listen_addresses=127.0.0.1 -c unix_socket_directories=$ROOT" start >/dev/null
fi
PSQL="$BIN/psql -h 127.0.0.1 -p $PORT -U postgres -q -v ON_ERROR_STOP=1"
$PSQL -d postgres -tc "select 1 from pg_database where datname='demo'" | grep -q 1 || $PSQL -d postgres -c "create database demo"
$PSQL -d demo -tc "select to_regclass('public.customers')" | grep -q customers || $PSQL -d demo <<'SQL'
create table customers (id bigint primary key, name text not null, email text, balance numeric(20,6), joined timestamptz default now());
insert into customers values
 (1, '佐伯 光平', 'saeki@example.com', 12345678901234.123456),
 (2, 'Alice', null, 0.5),
 (3, '', 'empty-name@example.com', -99.000001),
 (4, '山田 花子', 'hanako@example.com', 100);
create table orders (id serial primary key, customer_id bigint not null references customers(id), placed timestamptz default now(), total numeric(12,2), note text);
insert into orders (customer_id, total, note) select (g % 4) + 1, (g * 13.37)::numeric(12,2), case when g % 5 = 0 then null else 'order ' || g end from generate_series(1, 5000) g;
create index orders_customer_placed on orders (customer_id, placed desc);
create view v_big_orders as select * from orders where total > 30000;
create table events (id bigserial primary key, at timestamptz, kind text, payload bytea);
insert into events (at, kind, payload) select now() - (g || ' seconds')::interval, (array['click','view','buy'])[1 + g % 3], decode(md5(g::text), 'hex') from generate_series(1, 200000) g;
SQL

cat > "$ROOT/connections.json" <<JSON
{"version":1,"connections":[
 {"name":"demo (dev)","driver":"postgresql","host":"127.0.0.1","port":$PORT,"database":"demo","username":"postgres","options":{"tls":"disable"},"env_label":"dev"},
 {"name":"demo (本番)","driver":"postgresql","host":"127.0.0.1","port":$PORT,"database":"demo","username":"postgres","options":{"tls":"disable"},"env_label":"production"}
]}
JSON
APP=${SQLTOOL_APP:-$REPO/target/debug/sqltool}
# 接続設定の取り込みは初回だけ(毎回取り込むと、同名の接続が (2)、(3) と増える)
IMPORT=""
[ -e "$ROOT/data/app.db" ] || IMPORT="--import-connections $ROOT/connections.json"
SQLTOOL_DATA_DIR="$ROOT/data" SQLTOOL_NO_KEYCHAIN=1 exec "$APP" $IMPORT "$@"
