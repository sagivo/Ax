# Ax API framework

Ax API is a small, source-generated HTTP framework for building JSON REST
services in Ax. You describe routes in comments next to ordinary Ax handlers;
`ax-api` expands those declarations into a typed `http.serve_handler` program,
then asks the normal Ax compiler to produce a native server.

The framework is intentionally separate from the language core:

```text
app.ax + ax-api directives
          │
          ▼
ax-api route/response expansion
          │ ordinary Ax source
          ▼
Ax checker and native code generator
          │
          ▼
http.serve_handler → HTTP/1.1 server
```

There is no runtime route registry, reflection, framework global, or `api.*`
builtin. If you inspect the expanded source, you will see the same typed HTTP
request and response operations available to any Ax program.

## Install and create an application

From a checkout of this repository, run the framework directly:

```sh
cargo run -p ax-api -- new my-api
cargo run -p ax-api -- run my-api/app.ax
```

For an installed binary:

```sh
cargo install --path frameworks/ax-api
ax-api new my-api
ax-api run my-api/app.ax
```

`new` creates a directory and writes `app.ax`. It refuses to overwrite an
existing `app.ax`. The generated application listens on port `8080` and is a
complete runnable example.

The generated build files are kept in `<app directory>/.ax-api/`. This folder
contains generated Ax source and compiler output; it is safe to remove when you
want a clean rebuild.

## A complete application

An Ax API application is one source file with a module declaration, a port
directive, route directives, and handler functions:

```ax
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

Save it as `app.ax`, then start it:

```sh
ax-api run app.ax
```

In this example:

| Request | Handler | Result |
|---|---|---|
| `GET /health` | `health` | static JSON, `200` |
| `GET /v1/items` | `list_items` | static JSON array, `200` |
| `GET /v1/items/42` | `show_item` | path parameter `id = "42"`, `200` |
| `POST /v1/items` | `create_item` | request body, `201` |
| `DELETE /v1/items/42` | `delete_item` | empty body, `204` |

Try the service with curl:

```sh
curl http://127.0.0.1:8080/health
curl http://127.0.0.1:8080/v1/items
curl http://127.0.0.1:8080/v1/items/42
curl -X POST -H 'Content-Type: application/json' \
  --data '{"name":"new"}' http://127.0.0.1:8080/v1/items
curl -i -X DELETE http://127.0.0.1:8080/v1/items/42
```

The repository contains the same style of service in
[`examples/rest_api.ax`](../examples/rest_api.ax).

## CLI commands

```text
ax-api new [directory]
ax-api run [--tier dev|release] app.ax
ax-api run [--tls-cert cert --tls-key key --tls-port 8443] app.ax
ax-api build [-o binary] [--tier dev|release] app.ax
ax-api watch [--tier dev|release] [--interval ms] app.ax
ax-api expand app.ax
```

- `new` creates a starter application.
- `run` expands, checks, builds, and starts the native server. Release is the
  default tier.
- `build` performs the same expansion and checking but leaves the server
  binary on disk. `-o` copies it to the path you specify.
- `expand` prints the generated core Ax source without compiling it. Use this
  to debug route matching or inspect exactly what the framework adds.
- `watch` rebuilds and restarts the application when `app.ax` changes. The
  default poll interval is 250 ms; use `--interval` to change it.
- `--tls-cert` and `--tls-key` enable the built-in Rustls TLS terminator for
  `run`; `--tls-port` defaults to `8443` and forwards decrypted HTTP to the
  application port.
- `--tier dev` is quicker for iteration; `--tier release` is optimized for a
  server binary and is the default.

The source file must not define `fn main`; `ax-api` generates the only main
function and wires it to the configured port.

## Database state

Ax DB is optional and remains a standalone component. Configure one shared
SQLite pool with a directive:

```ax
// ax-api database app.sqlite
// ax-api database_timeout 2500
// ax-api GET /items -> list_items

type Item = {id: i64, name: String};
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

When configured, `database` is the first argument of every application handler.
The generated `main` opens it once and uses the core stateful-handler ABI. The
generated route boundary catches an unhandled database error and returns `500`
with code `database_error`; handlers may catch errors themselves for more
specific responses. See [`packages/ax-db/README.md`](../packages/ax-db/README.md)
for parameter binding, typed rows, transactions, migrations, and current SQLite
type limits.

For deployment configuration, read the path through the core environment
capability and retain a local fallback:

```ax
// ax-api database_env AX_DATABASE_PATH app.sqlite
```

This expands to `env.get_or("AX_DATABASE_PATH", "app.sqlite")`, and the
generated main function declares `io[env]` in addition to its database and
network effects.

`// ax-api database_timeout N` configures the SQLite statement and lock-wait
deadline in milliseconds. It must be used with `database` or `database_env`;
`0` disables the statement progress deadline.

## Defining routes

A route directive has this exact shape:

```ax
// ax-api METHOD /path -> handler_name
```

Supported methods are `GET`, `POST`, `PUT`, `PATCH`, and `DELETE`. Method names
are normalized to uppercase, but writing uppercase is clearer. The directive
may appear anywhere in the file; the parser scans lines for the `// ax-api `
prefix.

Static paths are matched literally:

```ax
// ax-api GET /health -> health
```

Path parameters may be nested. A `{name}` segment captures one non-empty path
segment, while `*name` captures the remaining suffix and must be last:

```ax
// ax-api GET /users/{user_id}/posts/{post_id} -> show_post
// ax-api GET /assets/*path -> asset
fn show_post(request: http.Request, user_id: String, post_id: String) -> http.Response =
    api.ok(user_id);
fn asset(request: http.Request, path: String) -> http.Response = api.ok(path);
```

Parameters are decoded strings. A slash inside a `{name}` value does not match
the route even when it is percent-encoded; a wildcard does include slashes.
Query strings are excluded from path matching, so
`/users/42/posts/7?verbose=1` still reaches the route.

Routes are tested in declaration order. Put more specific static routes before
a parameter route with the same prefix, for example `/users/me` before
`/users/{id}`. The generator rejects missing or malformed handlers, paths that
contain a query or fragment, multiple parameters, non-final parameters,
duplicate method/path shapes, and attempts to claim `/openapi.json`. Route
options follow the handler name and are passed as additional `String` values:

```ax
// ax-api GET /search -> search query=q header=X-Request-Id session=sid
fn search(request: http.Request, q: String, trace: String, sid: String) -> http.Response =
    api.ok(q);
```

Use `body=Type` for typed JSON decoding. Invalid JSON returns `422` before the
handler runs:

```ax
// ax-api POST /items -> create body=CreateItem
type CreateItem = {name: String, count: u64};
fn create(request: http.Request, item: CreateItem) -> http.Response =
    api.created({ok: true});
```

The handler signature is checked by the normal Ax compiler:

```ax
// static route
fn handler(request: http.Request) -> http.Response = ...;

// route with `{name}`
fn handler(request: http.Request, name: String) -> http.Response = ...;
```

The first argument is always a request, followed by path parameters in path
order, then `query=`, `header=`, and `session=` values in declaration order.

Application-wide middleware and transport settings are directives:

```ax
// ax-api body_limit 2048
// ax-api timeout_ms 2500
// ax-api cors https://app.example
// ax-api auth Authorization=Bearer token
// ax-api session sid
```

The equivalent explicit middleware spelling is also accepted, for example
`// ax-api middleware auth Authorization=Bearer token` or
`// ax-api middleware cors https://app.example`.

`auth` compares the named header exactly. `session` requires the named cookie
and route-level `session=name` passes its value to the handler. These guards run
before route dispatch, so unauthorized requests do not enter application code.

## Reading a request

Every handler receives a typed `http.Request` record:

| Field | Contents |
|---|---|
| `request.method` | Uppercase HTTP method, such as `GET` or `POST` |
| `request.path` | URL path without `?query` or a fragment |
| `request.query` | Raw query text without the leading `?` |
| `request.headers` | Raw HTTP header lines |
| `request.body` | Raw request body as a `String` |

Named route options use extraction helpers. `query=name` reads a query key,
`header=Name` reads a case-insensitive header, and `session=name` reads a named
cookie. Path, query, and cookie values are percent-decoded; query `+` is
interpreted as a space. Header values remain verbatim.

Typed `body=Type` routes use Ax's descriptor-driven `json.decode` and catch
malformed input as `422`. The native decoder rejects unknown or duplicate
fields, missing fields, wrong primitive kinds, and numeric overflow.

## Returning a response

`api.*` helpers are compile-time conveniences. They are rewritten to ordinary
`http.response(status, body)` expressions:

```ax
api.ok({status: "ready"})              // 200
api.created({id: 42})                   // 201
api.json(202u16, {status: "queued"})   // caller-selected status
api.stream("large-but-bounded-body")   // 200, HTTP chunked framing
api.no_content()                       // 204 with an empty body
api.bad_request()                      // standard JSON 400
api.not_found()                        // standard JSON 404
```

Object keys may be bare (`{status: "ready"}`) or quoted. Nested objects,
arrays, strings, numbers, booleans, and `null` are supported in static literals.
Static literals are serialized at build time and reused for every request.

Dynamic strings are also accepted:

```ax
fn echo(request: http.Request) -> http.Response = api.ok(request.body);
```

Dynamic bodies are sent unchanged. Because the server advertises
`Content-Type: application/json`, the handler is responsible for ensuring a
dynamic body is valid JSON (and for quoting a string value when appropriate).
The `204` helper sends no body. `api.stream` uses HTTP/1.1 chunked framing and
closes the connection after the body; it is useful for clients that consume a
chunked response, but the current runtime still bounds one generated response
to its configured body limit.

The built-in `bad_request` and `not_found` bodies are:

```json
{"error":{"code":"bad_request","message":"Bad request"}}
{"error":{"code":"not_found","message":"Resource not found"}}
```

For custom error payloads, use `api.json(400u16, {...})` or another status code.

## Generated behavior

Every service automatically exposes:

- `GET /openapi.json`, a generated OpenAPI 3.1 document containing paths,
  methods, operation IDs, and final path parameters.
- `GET /docs`, a generated interactive HTML route browser backed by the
  OpenAPI document.
- `404` with a structured JSON error when no path matches.
- `405` with a structured JSON error when a known path is called with an
  unsupported method.
- `204` to `OPTIONS` for a known path.
- `Access-Control-Allow-Origin` when `// ax-api cors origin` is configured.
- `401` authentication and session-cookie guards when `// ax-api auth` or
  `// ax-api session` is configured.

The generated OpenAPI document includes path, query, header, and cookie
parameters, JSON request-body references for `body=Type` routes, common error
responses, and an `apiAuth` security scheme when authentication is configured.
Record schemas are represented as open objects because Ax's descriptor metadata
is not available to the source generator.

The server speaks HTTP/1.1. `// ax-api body_limit N` bounds request bodies and
`// ax-api timeout_ms N` applies socket read/write timeouts. `ax-api run` can
terminate TLS with Rustls and forward clear HTTP to the generated service.
`405` responses include an `Allow` header listing the verbs understood by the
generated dispatcher.

## Testing an application

Use `expand` and the normal Ax checker before starting a server:

```sh
ax-api expand app.ax > generated.ax
ax-api build --tier dev -o app app.ax
./app
```

Then exercise each route with a small curl script or an HTTP client. Include at
least one request for each method, a path parameter containing punctuation, a
query string, an empty body, an unknown path (`404`), and an unsupported method
(`405`). The package includes an end-to-end test for dispatch, request bodies,
`404`, and OpenAPI; run it with:

```sh
cargo test -p ax-api
```

For development reload, keep the service running with:

```sh
ax-api watch --interval 250 app.ax
```

To test TLS locally, provide a PEM certificate and private key:

```sh
ax-api run --tls-cert dev-cert.pem --tls-key dev-key.pem --tls-port 8443 app.ax
curl -k https://127.0.0.1:8443/docs
```

Useful negative checks include:

```sh
curl -i http://127.0.0.1:8080/does-not-exist
curl -i -X POST http://127.0.0.1:8080/health
curl -i -X OPTIONS http://127.0.0.1:8080/health
curl http://127.0.0.1:8080/openapi.json
curl -i http://127.0.0.1:8080/docs
curl -i http://127.0.0.1:8080/stream
```

Do not commit `.ax-api/` build output unless your project deliberately tracks
generated artifacts.

## Troubleshooting

- **`no routes found`** — check that each declaration starts with the exact
  `// ax-api ` prefix and includes `METHOD /path -> handler`.
- **`duplicate route`** — two routes with the same method and path shape are
  ambiguous, even when their parameter names differ.
- **`ax-api owns \`main\``** — remove the application `fn main`; the framework
  generates it and calls `http.serve_handler`.
- **A handler does not type-check** — arguments must be ordered as request,
  path parameters, query/header/session options, then the typed body.
- **A dynamic response is rejected or not valid JSON** — `api.ok` and friends
  accept a `String` at runtime. Structured values must be static literals, and a
  dynamic body must already be valid JSON.
- **The server will not start** — another process may already own the port.
  Change the `// ax-api port N` directive and rebuild.
- **Generated output looks stale** — `run` and `build` regenerate automatically;
  remove `.ax-api/` if you are inspecting artifacts from an interrupted build.
- **TLS fails before listening** — verify the certificate and key are PEM files,
  contain a matching key pair, and that `--tls-port` is free.

## Performance model

The generated dispatcher is a native if-chain. Static routes use direct string
equality; simple final parameters use prefix checks and string slicing; nested
and wildcard routes use a compact segment matcher. There is no runtime route
registry, reflection, or middleware traversal on the dispatch path. Static
response literals are marked for the runtime's serialize-once response path,
while dynamic and chunked bodies use a connection-local buffer.

The framework's benchmark compares a generated route with a direct typed Ax
handler on the same HTTP runtime. Reproduce the local measurements with:

```sh
sh bench/http/run.sh
```

Results depend on the host, compiler, and load pattern. The current reactor has
one 64 KiB connection buffer; `body_limit` can lower the accepted request size
and may be set up to 65,280 bytes.

## Implemented feature set and boundaries

The framework now provides typed record JSON decoding, named query/header/cookie
extraction, auth and session guards, CORS headers, nested and wildcard routes,
configurable body/time-out limits, generated OpenAPI and interactive docs, a
dependency-aware hot-reload watcher, Rustls TLS termination, and chunked
response framing.

Typed JSON bodies are schema-checked at the native boundary: malformed JSON,
unknown or duplicate fields, missing fields, wrong primitive kinds, and numeric
overflow all produce `422`. Path, query, and cookie values are percent-decoded
(query `+` becomes a space). The native reactor bounds one connection at 64 KiB;
`body_limit` may be set up to 65,280 bytes. Chunked request bodies are decoded
before the handler receives `request.body`; `api.stream` provides chunked HTTP
framing for bounded responses. True incremental producer callbacks still
require a future async stream type in the Ax core.
