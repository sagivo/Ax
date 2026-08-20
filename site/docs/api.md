# Ax API

Standalone source-generated REST framework. The compiler and core runtime do
not know about routes or `api.*`. `ax-api` expands directives into an ordinary
typed `http.serve_handler` program.

Do not define `fn main`. The generator writes it.

## Commands

```
ax-api new [directory]
ax-api run [--tier dev|release] app.ax
ax-api run [--tls-cert cert --tls-key key --tls-port 8443] app.ax
ax-api build [-o binary] [--tier dev|release] app.ax
ax-api watch [--tier dev|release] [--interval ms] app.ax
ax-api expand app.ax
```

Release is the default tier for `run` / `build`.

## Directives

Prefix: `// ax-api `.

```
// ax-api port 8080
// ax-api METHOD /path -> handler [query=k header=Name session=sid body=Type]
// ax-api database app.sqlite
// ax-api database_env AX_DATABASE_PATH app.sqlite
// ax-api database_timeout 2500
// ax-api body_limit 2048
// ax-api timeout_ms 2500
// ax-api cors https://app.example
// ax-api auth Authorization=Bearer token
// ax-api session sid
```

Methods: `GET` `POST` `PUT` `PATCH` `DELETE`.

`{name}` captures one non-empty segment. `*name` captures the suffix and must
be last. Routes are tested in declaration order. Do not claim `/openapi.json`.

Handler shape:

```
fn handler(request: http.Request, /* path params in order */, /* query/header/session */) -> http.Response
```

If `database` is set, `database: db.Pool` is the first argument.

## Responses

Rewritten to `http.response(status, body)` at expand time.

| helper | status |
|---|---|
| `api.ok(body)` | 200 |
| `api.created(body)` | 201 |
| `api.json(status, body)` | caller |
| `api.stream(body)` | 200 chunked |
| `api.no_content()` | 204 |
| `api.bad_request()` | 400 |
| `api.not_found()` | 404 |

Static literals are serialized once and reused. Dynamic bodies are
request-local. `body=Type` uses `json.decode`; invalid JSON is `422` before
the handler runs.

## Generated routes

- `GET /openapi.json` — OpenAPI 3.1
- `GET /docs` — interactive route browser

## Minimal service

```
module app;

// ax-api port 8080
// ax-api GET /health -> health
// ax-api GET /v1/items -> list_items
// ax-api GET /v1/items/{id} -> show_item
// ax-api POST /v1/items -> create_item
// ax-api DELETE /v1/items/{id} -> delete_item

fn health(request: http.Request) -> http.Response =
    api.ok({ok: true});

fn list_items(request: http.Request) -> http.Response =
    api.ok([{id: 1, name: "first"}]);

fn show_item(request: http.Request, id: String) -> http.Response =
    api.ok(id);

fn create_item(request: http.Request) -> http.Response =
    api.created(request.body);

fn delete_item(request: http.Request, id: String) -> http.Response =
    api.no_content();
```

```
ax-api run app.ax
```

## Performance

The framework expands to the same `http.serve_handler` hot path as a hand-
written server. On the reference machine, generated routing and direct Ax
routing measured 145,382 vs 145,044 req/s (0.23%, noise). Headline HTTP
bench: Ax 143,273 req/s at 256 connections, p99 2.34 ms.

## Database

Optional standalone packages: `ax-db` (SQLite) and `ax-db-mysql`.
`AX_DB_DRIVER=mysql` plus a `mysql://` DSN selects MySQL.
