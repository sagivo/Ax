# Ax DB

Ax DB is the standalone database component for Ax. SQLite is the default
driver. The companion [`ax-db-mysql`](../ax-db-mysql/README.md) package uses
the same driver-neutral `db.Pool` surface for MySQL.

It provides parameter binding, typed row decoding, explicit transactions, and
an idempotent migration CLI without putting database policy into the language
or the REST framework.

## Ax API

```ax
type Item = {id: i64, name: String};

fn list(pool: db.Pool) -> Vec[Item] !{alloc[a], err[db.Error], io[db]} =
    db.query0(pool, test.alloc, "SELECT id, name FROM items ORDER BY id");
```

The query column names and count must exactly match the result record. Supported
field types are Ax integers, floats, `bool`, `String`, and `Option[T]` for those
scalar types. SQL `NULL` becomes `None`; a non-null value becomes `Some(value)`.

Use `db.exec` and `db.query` for parameterized SQL. Parameters are bound as
SQLite text values and are never interpolated into SQL. SQLite applies its
normal column affinity when converting those bound values:

```ax
let mut params: Vec[String] = vec.new(test.alloc);
params.push("first");
db.exec(pool, "INSERT INTO items(name) VALUES (?)", params);
```

Use `db.Value` when SQLite type affinity or `NULL` must be explicit:

```ax
let mut values: Vec[db.Value] = vec.new(test.alloc);
values.push(I64(42));
values.push(Text("first"));
values.push(Null);
db.exec_values(pool, "INSERT INTO items(id, name, status) VALUES (?, ?, ?)", values);
```

`db.Value` supports `Null`, `Text`, `I64`, `U64`, `F64`, and `Bool`. The
`*_values` operations are the typed-parameter variants; the original string
operations remain convenient for text-only queries.

The complete surface is:

- `db.open`, `db.open_timeout`, `db.set_timeout`, `db.close`
- `db.exec0`, `db.exec`
- `db.exec_values`
- `db.query0[T]`, `db.query[T]`
- `db.query_values[T]`
- `db.begin`, `db.tx_exec0`, `db.tx_exec`, `db.tx_exec_values`
- `db.tx_query0[T]`, `db.tx_query[T]`, `db.tx_query_values[T]`
- `db.commit`, `db.rollback`

`db.Pool` is a synchronized shared connection. SQLite is selected by default;
set `AX_DB_DRIVER=mysql` when building or running to select the MySQL package.
MySQL connections use `mysql://user:password@host:3306/database` DSNs and load
`libmysqlclient` dynamically. Handlers do not change between drivers. SQLite's
timeout is a statement progress deadline in milliseconds; MySQL uses the same
configuration point for driver connection policy. A timed-out or failed
statement returns `db.Error`.

## Migrations

Name migration files so lexical order is execution order:

```text
migrations/
  001_create_items.sql
  002_add_item_status.sql
```

Apply them with:

```sh
ax-db migrate app.sqlite migrations
```

Applied file names are recorded in `_ax_migrations`. Re-running the command
skips files already recorded. Each new file runs in its own immediate
transaction.

For MySQL migrations, install the MySQL client and run:

```sh
ax-db-mysql migrate mysql://user:password@host:3306/app migrations
```

## Ax API integration

An application may opt into database state:

```ax
// ax-api database app.sqlite
// ax-api GET /items -> list_items

fn list_items(database: db.Pool, request: http.Request) -> http.Response
    !{alloc[a], err[db.Error], io[db]} = {
    let rows: Vec[Item] = db.query0(
        database,
        test.alloc,
        "SELECT id, name FROM items ORDER BY id"
    );
    api.ok(json.encode(test.alloc, rows))
};
```

The framework opens the pool once, passes it explicitly as the first handler
argument, and converts an uncaught route error to a structured `500` response.
Applications can still catch `db.Error` themselves when they need a different
mapping.

Use `// ax-api database_env AX_DATABASE_PATH app.sqlite` to obtain the path
through the core `env.get_or` capability with an explicit fallback.
Use `// ax-api database_timeout 2500` alongside either database directive to
set the SQLite statement deadline in milliseconds.
