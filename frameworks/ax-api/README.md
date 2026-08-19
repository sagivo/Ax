# Ax API

Standalone, source-generated REST API framework for Ax. The compiler and core
runtime do not know about routes or `api.*` response helpers; `ax-api` expands
those conveniences to an ordinary typed `http.serve_handler` program.

```sh
ax-api new my-api
ax-api run my-api/app.ax
```

See [`docs/api-framework.md`](../../docs/api-framework.md) for the complete
quick start and API reference, including typed JSON bodies, nested/wildcard
routes, query/header/cookie extraction, auth/session/CORS directives, body and
timeout limits, generated `/docs`, hot reload, TLS termination, and chunked
responses.
