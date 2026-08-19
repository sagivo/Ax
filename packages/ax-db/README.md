# Ax DB

Ax DB is the standalone database component for Ax. The first driver is SQLite.
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
field types are Ax integers, floats, `bool`, and `String`. `NULL` must be removed
with SQL such as `COALESCE` until `Option[T]` fields are represented in runtime
type descriptors.

Use `db.exec` and `db.query` for parameterized SQL. Parameters are bound as
SQLite text values and are never interpolated into SQL:

```ax
let mut params: Vec[String] = vec.new(test.alloc);
params.push("first");
db.exec(pool, "INSERT INTO items(name) VALUES (?)", params);
```

The complete surface is:

- `db.open`, `db.close`
- `db.exec0`, `db.exec`
- `db.query0[T]`, `db.query[T]`
- `db.begin`, `db.tx_exec0`, `db.tx_exec`
- `db.tx_query0[T]`, `db.tx_query[T]`
- `db.commit`, `db.rollback`

`db.Pool` is a synchronized shared SQLite connection in this release. The type
name is the stable driver-neutral API; a later PostgreSQL driver can back it
with multiple physical connections without changing handlers.

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
