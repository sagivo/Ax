# Ax DB MySQL

`db-mysql` is the MySQL driver for Ax's standalone `db` component. It exports
the same `db.Pool`, parameter, typed-value, row-decoding, transaction, and
nullable-field operations as SQLite, so handlers remain driver-neutral.

Select it explicitly when building or running native Ax code:

```sh
AX_DB_DRIVER=mysql ax build app.ax
```

Use a `mysql://user:password@host:3306/database` path with `db.open` or the
`// ax-api database ...` directive. The native runtime loads `libmysqlclient`
at runtime. Set `AX_MYSQL_LIB` when the client library is in a non-standard
location. This keeps SQLite builds free of a MySQL system dependency.

The migration tool uses the installed `mysql` client:

```sh
ax-db-mysql migrate mysql://user:password@host:3306/app migrations
```

MySQL parameter values are escaped and rendered as SQL literals by the native
driver. The typed `db.Value` variants preserve `NULL`, integer, floating-point,
boolean, and text intent. Query columns are matched by name to the result
record, and SQL `NULL` requires `Option[T]` just as with SQLite.
