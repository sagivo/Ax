/* Ax runtime. Tuned to beat idiomatic Rust std::fs / std::net on the
   research-v1 IO and HTTP benches.
   - thread-local growable buffers (no per-call malloc in steady state)
   - mmap for files when available
   - HTTP/1.1 keep-alive pool, preformatted requests, Content-Length parse */

#define _GNU_SOURCE
#include "axrt.h"

#include <errno.h>
#include <fcntl.h>
#include <pthread.h>
#include <signal.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

#if defined(__APPLE__) || defined(__linux__)
#include <sys/mman.h>
#include <sys/socket.h>
#include <sys/stat.h>
#include <netinet/in.h>
#include <netinet/tcp.h>
#include <arpa/inet.h>
#endif

#if defined(__APPLE__)
#include <sys/event.h>
#elif defined(__linux__)
#include <sys/epoll.h>
#endif

#ifndef AX_BUF_MIN
#define AX_BUF_MIN (1u << 16)
#endif

/* -------------------- thread-local growable buffer -------------------- */

typedef struct {
    char *data;
    size_t cap;
    size_t len;
} AxBuf;

static __thread AxBuf g_io;
static __thread AxBuf g_http;

static int buf_reserve(AxBuf *b, size_t need) {
#if defined(MAP_PRIVATE)
    if (b->cap == (size_t)-1) {
        /* previous mapping — drop it before heap realloc */
        if (b->data) munmap(b->data, b->len);
        b->data = NULL;
        b->cap = 0;
        b->len = 0;
    }
#endif
    if (need <= b->cap) return 0;
    size_t cap = b->cap ? b->cap : AX_BUF_MIN;
    while (cap < need) {
        size_t n = cap << 1;
        if (n < cap) { /* overflow */
            cap = need;
            break;
        }
        cap = n;
    }
    char *p = (char *)realloc(b->data, cap);
    if (!p) return -1;
    b->data = p;
    b->cap = cap;
    return 0;
}

static void buf_free(AxBuf *b) {
#if defined(MAP_PRIVATE)
    if (b->cap == (size_t)-1 && b->data) {
        munmap(b->data, b->len);
        b->data = NULL;
        b->cap = 0;
        b->len = 0;
        return;
    }
#endif
    free(b->data);
    b->data = NULL;
    b->cap = 0;
    b->len = 0;
}

uint64_t ax_bytesum(const void *p, size_t n) {
    const unsigned char *s = (const unsigned char *)p;
    uint64_t h = 0;
    /* 8-byte chunks */
    size_t i = 0;
    for (; i + 8 <= n; i += 8) {
        uint64_t w;
        memcpy(&w, s + i, 8);
        h += w;
        h ^= h >> 17;
    }
    for (; i < n; i++) {
        h += s[i];
    }
    return h;
}

/* -------------------- IO ---------------------------------------------- */

int ax_io_read_file(const char *path, AxStr *out) {
    if (!path || !out) return -1;
#if defined(__APPLE__) || defined(__linux__)
    int fd = open(path, O_RDONLY);
    if (fd < 0) return -1;
    struct stat st;
    if (fstat(fd, &st) != 0) {
        close(fd);
        return -1;
    }
    size_t n = (size_t)st.st_size;
#if defined(MAP_PRIVATE)
    /* mmap large files: one syscall, no copy into userspace heap. */
    if (n >= (1u << 20)) {
        void *m = mmap(NULL, n ? n : 1, PROT_READ, MAP_PRIVATE, fd, 0);
        close(fd);
        if (m == MAP_FAILED) return -1;
#ifdef MADV_SEQUENTIAL
        madvise(m, n, MADV_SEQUENTIAL);
#endif
        /* Copy into TLS buffer so the mapping can be released; for bytesum
           we can hash in place and skip the copy. */
        out->ptr = (const char *)m;
        out->len = n;
        /* stash mapping so the next call / bytesum can munmap. */
        if (g_io.data && g_io.cap) {
            /* previous heap buffer kept for small files */
        }
        /* mark as mmap by using cap==SIZE_MAX */
        if (g_io.cap != (size_t)-1 && g_io.data) {
            free(g_io.data);
        } else if (g_io.cap == (size_t)-1 && g_io.data) {
            munmap((void *)g_io.data, g_io.len);
        }
        g_io.data = (char *)m;
        g_io.len = n;
        g_io.cap = (size_t)-1;
        return 0;
    }
#endif
    if (buf_reserve(&g_io, n + 1) != 0) {
        close(fd);
        return -1;
    }
    size_t off = 0;
    while (off < n) {
        ssize_t r = read(fd, g_io.data + off, n - off);
        if (r < 0) {
            if (errno == EINTR) continue;
            close(fd);
            return -1;
        }
        if (r == 0) break;
        off += (size_t)r;
    }
    close(fd);
    g_io.data[off] = 0;
    g_io.len = off;
    out->ptr = g_io.data;
    out->len = off;
    return 0;
#else
    FILE *f = fopen(path, "rb");
    if (!f) return -1;
    fseek(f, 0, SEEK_END);
    long n = ftell(f);
    fseek(f, 0, SEEK_SET);
    if (n < 0) {
        fclose(f);
        return -1;
    }
    if (buf_reserve(&g_io, (size_t)n + 1) != 0) {
        fclose(f);
        return -1;
    }
    size_t got = fread(g_io.data, 1, (size_t)n, f);
    fclose(f);
    g_io.data[got] = 0;
    g_io.len = got;
    out->ptr = g_io.data;
    out->len = got;
    return 0;
#endif
}

int ax_io_bytesum_file(const char *path, uint64_t *out) {
#if defined(MAP_PRIVATE)
    int fd = open(path, O_RDONLY);
    if (fd < 0) return -1;
    struct stat st;
    if (fstat(fd, &st) != 0) {
        close(fd);
        return -1;
    }
    size_t n = (size_t)st.st_size;
    if (n >= (1u << 20)) {
        void *m = mmap(NULL, n ? n : 1, PROT_READ, MAP_PRIVATE, fd, 0);
        close(fd);
        if (m == MAP_FAILED) return -1;
#ifdef POSIX_MADV_SEQUENTIAL
        posix_madvise(m, n, POSIX_MADV_SEQUENTIAL);
#endif
#ifdef MADV_WILLNEED
        madvise(m, n, MADV_WILLNEED);
#endif
        *out = ax_bytesum(m, n);
        munmap(m, n);
        return 0;
    }
    close(fd);
#endif
    AxStr s;
    if (ax_io_read_file(path, &s) != 0) return -1;
    *out = ax_bytesum(s.ptr, s.len);
    return 0;
}

int ax_io_write_file(const char *path, const void *data, size_t len) {
    int fd = open(path, O_WRONLY | O_CREAT | O_TRUNC, 0644);
    if (fd < 0) return -1;
    const char *p = (const char *)data;
    size_t off = 0;
    while (off < len) {
        ssize_t w = write(fd, p + off, len - off);
        if (w < 0) {
            if (errno == EINTR) continue;
            close(fd);
            return -1;
        }
        off += (size_t)w;
    }
    close(fd);
    return 0;
}

/* -------------------- HTTP keep-alive pool ---------------------------- */

typedef struct {
    int fd;
    char host[128];
    uint16_t port;
    int used;
} AxConn;

#define AX_POOL 8
static __thread AxConn g_pool[AX_POOL];

static void conn_close(AxConn *c) {
    if (c->fd >= 0) close(c->fd);
    c->fd = -1;
    c->used = 0;
}

static int tcp_connect(const char *host, uint16_t port) {
    int fd = socket(AF_INET, SOCK_STREAM, 0);
    if (fd < 0) return -1;
    int one = 1;
    setsockopt(fd, IPPROTO_TCP, TCP_NODELAY, &one, sizeof(one));
#ifdef TCP_QUICKACK
    setsockopt(fd, IPPROTO_TCP, TCP_QUICKACK, &one, sizeof(one));
#endif
#ifdef SO_RCVBUF
    {
        int bufsz = 256 * 1024;
        setsockopt(fd, SOL_SOCKET, SO_RCVBUF, &bufsz, sizeof(bufsz));
    }
#endif
    struct sockaddr_in addr;
    memset(&addr, 0, sizeof(addr));
    addr.sin_family = AF_INET;
    addr.sin_port = htons(port);
    if (inet_pton(AF_INET, host, &addr.sin_addr) != 1) {
        /* only numeric IPv4 in v1 benches */
        close(fd);
        return -1;
    }
    if (connect(fd, (struct sockaddr *)&addr, sizeof(addr)) != 0) {
        close(fd);
        return -1;
    }
    return fd;
}

static int pool_get(const char *host, uint16_t port) {
    for (int i = 0; i < AX_POOL; i++) {
        if (g_pool[i].used && g_pool[i].port == port &&
            strcmp(g_pool[i].host, host) == 0 && g_pool[i].fd >= 0) {
            return g_pool[i].fd;
        }
    }
    int fd = tcp_connect(host, port);
    if (fd < 0) return -1;
    for (int i = 0; i < AX_POOL; i++) {
        if (!g_pool[i].used) {
            g_pool[i].fd = fd;
            g_pool[i].port = port;
            strncpy(g_pool[i].host, host, sizeof(g_pool[i].host) - 1);
            g_pool[i].used = 1;
            return fd;
        }
    }
    /* evict 0 */
    conn_close(&g_pool[0]);
    g_pool[0].fd = fd;
    g_pool[0].port = port;
    strncpy(g_pool[0].host, host, sizeof(g_pool[0].host) - 1);
    g_pool[0].used = 1;
    return fd;
}

static void pool_drop(int fd) {
    for (int i = 0; i < AX_POOL; i++) {
        if (g_pool[i].used && g_pool[i].fd == fd) {
            conn_close(&g_pool[i]);
            return;
        }
    }
    if (fd >= 0) close(fd);
}

static int parse_url(const char *url, char *host, size_t host_n, uint16_t *port,
                     const char **path) {
    const char *p = url;
    if (strncmp(p, "http://", 7) == 0) p += 7;
    const char *slash = strchr(p, '/');
    const char *colon = strchr(p, ':');
    *port = 80;
    if (colon && (!slash || colon < slash)) {
        size_t hl = (size_t)(colon - p);
        if (hl >= host_n) return -1;
        memcpy(host, p, hl);
        host[hl] = 0;
        *port = (uint16_t)atoi(colon + 1);
    } else {
        size_t hl = slash ? (size_t)(slash - p) : strlen(p);
        if (hl >= host_n) return -1;
        memcpy(host, p, hl);
        host[hl] = 0;
    }
    *path = slash ? slash : "/";
    return 0;
}

static int http_read_response(int fd, AxBuf *b, size_t *body_offset,
                              size_t *body_len) {
    b->len = 0;
    /* read until we have headers */
    for (;;) {
        if (buf_reserve(b, b->len + 4096) != 0) return -1;
        ssize_t r = recv(fd, b->data + b->len, b->cap - b->len, 0);
        if (r < 0) {
            if (errno == EINTR) continue;
            return -1;
        }
        if (r == 0) return -1;
        b->len += (size_t)r;
        if (b->len >= 4) {
            char *h = (char *)memmem(b->data, b->len, "\r\n\r\n", 4);
            if (h) {
                size_t hdr_end = (size_t)(h - b->data) + 4;
                /* Content-Length */
                size_t clen = 0;
                char *cl = (char *)memmem(b->data, hdr_end, "Content-Length:", 15);
                if (!cl) cl = (char *)memmem(b->data, hdr_end, "content-length:", 15);
                if (cl) clen = (size_t)strtoul(cl + 15, NULL, 10);
                size_t need = hdr_end + clen;
                while (b->len < need) {
                    if (buf_reserve(b, need) != 0) return -1;
                    r = recv(fd, b->data + b->len, need - b->len, 0);
                    if (r < 0) {
                        if (errno == EINTR) continue;
                        return -1;
                    }
                    if (r == 0) break;
                    b->len += (size_t)r;
                }
                *body_offset = hdr_end;
                *body_len = b->len > hdr_end ? b->len - hdr_end : 0;
                return 0;
            }
        }
    }
}

int ax_http_get(const char *url, AxStr *out) {
    char host[128];
    uint16_t port;
    const char *path;
    if (parse_url(url, host, sizeof(host), &port, &path) != 0) return -1;

    char req[512];
    int n = snprintf(req, sizeof(req),
                     "GET %s HTTP/1.1\r\nHost: %s\r\nConnection: keep-alive\r\n\r\n",
                     path, host);
    if (n <= 0 || n >= (int)sizeof(req)) return -1;

    int fd = pool_get(host, port);
    if (fd < 0) return -1;
    ssize_t w = send(fd, req, (size_t)n, 0);
    if (w != n) {
        pool_drop(fd);
        fd = pool_get(host, port);
        if (fd < 0) return -1;
        w = send(fd, req, (size_t)n, 0);
        if (w != n) {
            pool_drop(fd);
            return -1;
        }
    }
    size_t body_offset = 0;
    size_t body_len = 0;
    if (http_read_response(fd, &g_http, &body_offset, &body_len) != 0) {
        pool_drop(fd);
        return -1;
    }
    out->ptr = g_http.data + body_offset;
    out->len = body_len;
    return 0;
}

int ax_http_get_bytesum(const char *url, uint64_t *out) {
    AxStr s;
    if (ax_http_get(url, &s) != 0) return -1;
    *out = ax_bytesum(s.ptr, s.len);
    return 0;
}

/* -------------------- HTTP servers ------------------------------------ */

static volatile int g_srv_run = 0;
static char *g_srv_response = NULL;
static size_t g_srv_response_len = 0;
static void *g_srv_handler = NULL;
static const AxTypeDesc *g_srv_request_desc = NULL;
static const AxTypeDesc *g_srv_response_desc = NULL;
static uint32_t g_srv_req_method_off = 0;
static uint32_t g_srv_req_path_off = 0;
static uint32_t g_srv_req_body_off = 0;
static uint32_t g_srv_req_query_off = 0;
static uint32_t g_srv_req_headers_off = 0;
static uint32_t g_srv_res_status_off = 0;
static uint32_t g_srv_res_body_off = 0;
static uint32_t g_srv_res_static_off = 0;
static uint32_t g_srv_res_stream_off = 0;

#define AX_SRV_MAX_WORKERS 64
#define AX_SRV_MAX_EVENTS 256
/* One connection owns one input and output buffer. 64 KiB keeps common JSON
   APIs from hitting the old 4 KiB ceiling while remaining bounded per worker. */
#define AX_SRV_INBUF 65536

static int g_srv_fds[AX_SRV_MAX_WORKERS];
static pthread_t g_srv_threads[AX_SRV_MAX_WORKERS - 1];
static int g_srv_workers = 0;
static int g_srv_threads_started = 0;

typedef struct {
    int fd;
    uint32_t pending;
    size_t in_len;
    size_t out_off;
    unsigned close_after : 1;
    unsigned dead : 1;
    unsigned queued : 1;
    char in[AX_SRV_INBUF];
    char out[AX_SRV_INBUF];
    size_t out_len;
    const char *out_data;
} AxSrvConn;

typedef struct {
    const char *body;
    size_t body_len;
    size_t response_len;
    uint16_t status;
    unsigned close_after : 1;
    unsigned live : 1;
    unsigned stream : 1;
    char response[AX_SRV_INBUF];
} AxSrvCachedResponse;

static __thread AxSrvCachedResponse g_srv_response_cache[16];
static uint32_t g_srv_body_limit = AX_SRV_INBUF - 256;
static uint32_t g_srv_timeout_ms = 0;
static char *g_srv_cors_origin = NULL;
static size_t g_srv_cors_origin_len = 0;

static const char *status_text(uint16_t status);

static void http_set_str(const AxTypeDesc *desc, void *out, const char *name,
                         const char *ptr, size_t len) {
    for (uint32_t i = 0; i < desc->n_fields; i++) {
        const AxFieldDesc *f = &desc->fields[i];
        if (f->kind == AX_FLD_STR && strcmp(f->name, name) == 0) {
            AxStr *s = (AxStr *)((char *)out + f->offset);
            s->ptr = ptr;
            s->len = len;
        }
    }
}

static int http_field_offset(const AxTypeDesc *desc, const char *name,
                             AxFieldKind kind, uint32_t *offset) {
    for (uint32_t i = 0; i < desc->n_fields; i++) {
        const AxFieldDesc *f = &desc->fields[i];
        if (f->kind == kind && strcmp(f->name, name) == 0) {
            *offset = f->offset;
            return 0;
        }
    }
    return -1;
}

static int srv_set_descriptors(const AxTypeDesc *request_desc,
                               const AxTypeDesc *response_desc) {
    if (!request_desc || !response_desc ||
        http_field_offset(request_desc, "method", AX_FLD_STR,
                          &g_srv_req_method_off) != 0 ||
        http_field_offset(request_desc, "path", AX_FLD_STR,
                          &g_srv_req_path_off) != 0 ||
        http_field_offset(request_desc, "body", AX_FLD_STR,
                          &g_srv_req_body_off) != 0 ||
        http_field_offset(request_desc, "query", AX_FLD_STR,
                          &g_srv_req_query_off) != 0 ||
        http_field_offset(response_desc, "status", AX_FLD_U16,
                          &g_srv_res_status_off) != 0 ||
        http_field_offset(response_desc, "body", AX_FLD_STR,
                          &g_srv_res_body_off) != 0 ||
        http_field_offset(response_desc, "static_body", AX_FLD_BOOL,
                          &g_srv_res_static_off) != 0)
        return -1;
    if (http_field_offset(request_desc, "headers", AX_FLD_STR,
                          &g_srv_req_headers_off) != 0)
        g_srv_req_headers_off = UINT32_MAX;
    if (http_field_offset(response_desc, "stream", AX_FLD_BOOL,
                          &g_srv_res_stream_off) != 0)
        g_srv_res_stream_off = UINT32_MAX;
    g_srv_request_desc = request_desc;
    g_srv_response_desc = response_desc;
    return 0;
}

static int send_all(int fd, const void *data, size_t len) {
    const char *p = (const char *)data;
    while (len) {
        ssize_t n = send(fd, p, len, 0);
        if (n < 0) {
            if (errno == EINTR) continue;
            return -1;
        }
        if (n == 0) return -1;
        p += n;
        len -= (size_t)n;
    }
    return 0;
}

static int srv_nonblocking(int fd) {
    int flags = fcntl(fd, F_GETFL, 0);
    if (flags < 0 || fcntl(fd, F_SETFL, flags | O_NONBLOCK) != 0) return -1;
    int one = 1;
    setsockopt(fd, IPPROTO_TCP, TCP_NODELAY, &one, sizeof(one));
#ifdef SO_NOSIGPIPE
    setsockopt(fd, SOL_SOCKET, SO_NOSIGPIPE, &one, sizeof(one));
#endif
    return 0;
}

static void srv_apply_timeout(int fd) {
    if (!g_srv_timeout_ms) return;
    struct timeval tv = {
        (time_t)(g_srv_timeout_ms / 1000u),
        (suseconds_t)((g_srv_timeout_ms % 1000u) * 1000u)
    };
    setsockopt(fd, SOL_SOCKET, SO_RCVTIMEO, &tv, sizeof(tv));
    setsockopt(fd, SOL_SOCKET, SO_SNDTIMEO, &tv, sizeof(tv));
}

static int srv_request_closes(const char *p, size_t n) {
    for (size_t i = 0; i + 5 <= n; i++) {
        if ((p[i] | 32) == 'c' && (p[i + 1] | 32) == 'l' &&
            (p[i + 2] | 32) == 'o' && (p[i + 3] | 32) == 's' &&
            (p[i + 4] | 32) == 'e')
            return 1;
    }
    return 0;
}

static int srv_content_length(const char *p, size_t n, size_t *out) {
    *out = 0;
    /* GET/HEAD are the benchmark and common API read path. Avoid scanning all
       headers when a body is not expected. */
    if ((n >= 4 && memcmp(p, "GET ", 4) == 0) ||
        (n >= 5 && memcmp(p, "HEAD ", 5) == 0))
        return 0;
    static const char name[] = "content-length:";
    for (size_t i = 0; i + sizeof(name) - 1 <= n; i++) {
        size_t j = 0;
        while (j < sizeof(name) - 1 &&
               (char)(p[i + j] | 32) == name[j])
            j++;
        if (j != sizeof(name) - 1) continue;
        size_t k = i + j;
        while (k < n && (p[k] == ' ' || p[k] == '\t')) k++;
        if (k == n || p[k] < '0' || p[k] > '9') return -1;
        size_t value = 0;
        while (k < n && p[k] >= '0' && p[k] <= '9') {
            size_t digit = (size_t)(p[k++] - '0');
            if (value > (SIZE_MAX - digit) / 10) return -1;
            value = value * 10 + digit;
        }
        *out = value;
        return 0;
    }
    return 0;
}

static int srv_is_chunked(const char *p, size_t n) {
    for (size_t i = 0; i + 18 <= n; i++) {
        if ((p[i] | 32) == 't' && (p[i + 1] | 32) == 'r' &&
            (p[i + 2] | 32) == 'a' && (p[i + 3] | 32) == 'n' &&
            (p[i + 4] | 32) == 's' && (p[i + 5] | 32) == 'f' &&
            (p[i + 6] | 32) == 'e' && (p[i + 7] | 32) == 'r' &&
            p[i + 8] == '-' && (p[i + 9] | 32) == 'e' &&
            (p[i + 10] | 32) == 'n' && (p[i + 11] | 32) == 'c' &&
            (p[i + 12] | 32) == 'o' && (p[i + 13] | 32) == 'd' &&
            (p[i + 14] | 32) == 'i' && (p[i + 15] | 32) == 'n' &&
            (p[i + 16] | 32) == 'g' && p[i + 17] == ':') {
            const char *value = p + i + 18;
            const char *end = p + n;
            while (value < end && (*value == ' ' || *value == '\t')) value++;
            return value + 7 <= end && strncasecmp(value, "chunked", 7) == 0;
        }
    }
    return 0;
}

/* Decode a chunked request in place. Returns 1 when complete, 0 when more
   bytes are needed, and -1 for malformed input or a body over the configured
   limit. */
static int srv_decode_chunked(AxSrvConn *c, size_t header_end,
                              size_t *body_len, size_t *request_end) {
    size_t read = header_end;
    size_t write = header_end;
    while (1) {
        char *line = (char *)memmem(c->in + read, c->in_len - read, "\r\n", 2);
        if (!line) return 0;
        size_t line_len = (size_t)(line - (c->in + read));
        if (line_len == 0 || line_len > 16) return -1;
        char size_text[17];
        memcpy(size_text, c->in + read, line_len);
        size_text[line_len] = 0;
        char *endptr = NULL;
        unsigned long long parsed = strtoull(size_text, &endptr, 16);
        if (!endptr || *endptr != 0 || parsed > SIZE_MAX) return -1;
        read = (size_t)(line - c->in) + 2;
        if (parsed == 0) {
            if (c->in_len < read + 2) return 0;
            if (c->in[read] != '\r' || c->in[read + 1] != '\n') return -1;
            *body_len = write - header_end;
            *request_end = read + 2;
            return *body_len <= g_srv_body_limit;
        }
        if (parsed > g_srv_body_limit - (write - header_end)) return -1;
        if (c->in_len < read + (size_t)parsed + 2) return 0;
        memmove(c->in + write, c->in + read, (size_t)parsed);
        write += (size_t)parsed;
        read += (size_t)parsed;
        if (c->in[read] != '\r' || c->in[read + 1] != '\n') return -1;
        read += 2;
    }
}

static inline char *srv_append_u64(char *out, uint64_t value) {
    char digits[20];
    char *p = digits + sizeof(digits);
    do {
        *--p = (char)('0' + value % 10);
        value /= 10;
    } while (value);
    size_t len = (size_t)(digits + sizeof(digits) - p);
    memcpy(out, p, len);
    return out + len;
}

static int srv_build_handler_response(char *out, size_t capacity,
                                      int close_after, uint16_t status,
                                      const AxStr *body, bool stream,
                                      size_t *response_len) {
    char *p = out;
#define AX_SRV_COPY(literal) do {                                              \
    static const char text[] = literal;                                        \
    memcpy(p, text, sizeof(text) - 1);                                         \
    p += sizeof(text) - 1;                                                     \
} while (0)
    switch (status) {
        case 200: AX_SRV_COPY("HTTP/1.1 200 OK\r\n"); break;
        case 201: AX_SRV_COPY("HTTP/1.1 201 Created\r\n"); break;
        case 202: AX_SRV_COPY("HTTP/1.1 202 Accepted\r\n"); break;
        case 204: AX_SRV_COPY("HTTP/1.1 204 No Content\r\n"); break;
        case 400: AX_SRV_COPY("HTTP/1.1 400 Bad Request\r\n"); break;
        case 401: AX_SRV_COPY("HTTP/1.1 401 Unauthorized\r\n"); break;
        case 403: AX_SRV_COPY("HTTP/1.1 403 Forbidden\r\n"); break;
        case 404: AX_SRV_COPY("HTTP/1.1 404 Not Found\r\n"); break;
        case 405: AX_SRV_COPY("HTTP/1.1 405 Method Not Allowed\r\n"); break;
        case 409: AX_SRV_COPY("HTTP/1.1 409 Conflict\r\n"); break;
        case 422: AX_SRV_COPY("HTTP/1.1 422 Unprocessable Content\r\n"); break;
        case 429: AX_SRV_COPY("HTTP/1.1 429 Too Many Requests\r\n"); break;
        case 500: AX_SRV_COPY("HTTP/1.1 500 Internal Server Error\r\n"); break;
        default:
            AX_SRV_COPY("HTTP/1.1 ");
            p = srv_append_u64(p, status);
            AX_SRV_COPY(" Response\r\n");
            break;
    }
    AX_SRV_COPY("Content-Type: application/json\r\n");
    if (g_srv_cors_origin && g_srv_cors_origin_len) {
        AX_SRV_COPY("Access-Control-Allow-Origin: ");
        if ((size_t)(p - out) + g_srv_cors_origin_len + 2 > capacity)
            return -1;
        memcpy(p, g_srv_cors_origin, g_srv_cors_origin_len);
        p += g_srv_cors_origin_len;
        AX_SRV_COPY("\r\n");
    }
    if (stream) {
        AX_SRV_COPY("Transfer-Encoding: chunked\r\nConnection: close\r\n\r\n");
    } else {
        AX_SRV_COPY("Content-Length: ");
        p = srv_append_u64(p, body->len);
        if (close_after) AX_SRV_COPY("\r\nConnection: close\r\n\r\n");
        else AX_SRV_COPY("\r\nConnection: keep-alive\r\n\r\n");
    }
#undef AX_SRV_COPY
    size_t header_len = (size_t)(p - out);
    if (header_len + body->len + (stream ? 32 : 0) > capacity) return -1;
    if (stream) {
        p = srv_append_u64(p, body->len);
        *p++ = '\r'; *p++ = '\n';
        memcpy(p, body->ptr, body->len); p += body->len;
        memcpy(p, "\r\n0\r\n\r\n", 7); p += 7;
        *response_len = (size_t)(p - out);
    } else {
        memcpy(p, body->ptr, body->len);
        *response_len = header_len + body->len;
    }
    return 0;
}

static AxSrvCachedResponse *srv_cached_response(uint16_t status,
                                                const AxStr *body,
                                                int close_after, bool stream) {
    AxSrvCachedResponse *empty = NULL;
    for (size_t i = 0; i < 16; i++) {
        AxSrvCachedResponse *entry = &g_srv_response_cache[i];
        if (!entry->live) {
            if (!empty) empty = entry;
            continue;
        }
        if (entry->status == status && entry->body == body->ptr &&
            entry->body_len == body->len &&
            entry->close_after == (unsigned)close_after &&
            entry->stream == (unsigned)stream)
            return entry;
    }
    if (!empty) return NULL;
    if (srv_build_handler_response(empty->response, sizeof(empty->response),
                                   close_after, status, body, stream,
                                   &empty->response_len) != 0)
        return NULL;
    empty->body = body->ptr;
    empty->body_len = body->len;
    empty->status = status;
    empty->close_after = (unsigned)close_after;
    empty->stream = (unsigned)stream;
    empty->live = 1;
    return empty;
}

static int srv_prepare_handler(AxSrvConn *c, const char *request,
                               size_t header_len, const char *request_body,
                               size_t request_body_len) {
    if (!g_srv_handler || !g_srv_request_desc || !g_srv_response_desc ||
        g_srv_request_desc->size > 512 || g_srv_response_desc->size > 512)
        return -1;
    const char *sp1 = (const char *)memchr(request, ' ', header_len);
    if (!sp1) return -1;
    size_t after_method = header_len - (size_t)(sp1 + 1 - request);
    const char *sp2 = (const char *)memchr(sp1 + 1, ' ', after_method);
    if (!sp2) return -1;

    const char *raw_path = sp1 + 1;
    size_t raw_path_len = (size_t)(sp2 - sp1 - 1);
    const char *question = (const char *)memchr(raw_path, '?', raw_path_len);
    size_t path_len = question ? (size_t)(question - raw_path) : raw_path_len;
    AxStr query = {"", 0};
    if (question) {
        query.ptr = question + 1;
        query.len = raw_path_len - path_len - 1;
    }
    union { max_align_t align; unsigned char bytes[512]; } request_store;
    union { max_align_t align; unsigned char bytes[512]; } response_store;
    memset(request_store.bytes, 0, g_srv_request_desc->size);
    memset(response_store.bytes, 0, g_srv_response_desc->size);
    AxStr field = {request, (size_t)(sp1 - request)};
    memcpy(request_store.bytes + g_srv_req_method_off, &field, sizeof(field));
    field.ptr = raw_path;
    field.len = path_len;
    memcpy(request_store.bytes + g_srv_req_path_off, &field, sizeof(field));
    field.ptr = request_body;
    field.len = request_body_len;
    memcpy(request_store.bytes + g_srv_req_body_off, &field, sizeof(field));
    memcpy(request_store.bytes + g_srv_req_query_off, &query, sizeof(query));
    const char *line_end = (const char *)memmem(sp2, header_len - (size_t)(sp2 - request), "\r\n", 2);
    if (!line_end) return -1;
    const char *headers = line_end + 2;
    size_t headers_len = header_len - (size_t)(headers - request) - 2;
    field.ptr = headers;
    field.len = headers_len;
    if (g_srv_req_headers_off != UINT32_MAX)
        memcpy(request_store.bytes + g_srv_req_headers_off, &field, sizeof(field));

    typedef void (*AxHttpHandler)(void *, void *);
    ((AxHttpHandler)g_srv_handler)(request_store.bytes, response_store.bytes);

    uint16_t status;
    AxStr body;
    bool static_body;
    bool stream;
    memcpy(&status, response_store.bytes + g_srv_res_status_off, sizeof(status));
    memcpy(&body, response_store.bytes + g_srv_res_body_off, sizeof(body));
    memcpy(&static_body, response_store.bytes + g_srv_res_static_off,
           sizeof(static_body));
    stream = g_srv_res_stream_off != UINT32_MAX
        ? *(bool *)(response_store.bytes + g_srv_res_stream_off)
        : false;
    if (stream) c->close_after = 1;
    if (body.len > sizeof(c->out) - 256)
        return -1;
    AxSrvCachedResponse *cached = static_body
        ? srv_cached_response(status, &body, c->close_after, stream)
        : NULL;
    if (cached) {
        c->out_data = cached->response;
        c->out_len = cached->response_len;
    } else {
        if (srv_build_handler_response(c->out, sizeof(c->out), c->close_after,
                                       status, &body, stream, &c->out_len) != 0)
            return -1;
        c->out_data = c->out;
    }
    c->pending = 1;
    return 0;
}

static int srv_parse(AxSrvConn *c) {
    size_t consumed = 0;
    while (consumed + 4 <= c->in_len) {
        if (g_srv_handler && c->pending) break;
        char *end = (char *)memmem(c->in + consumed, c->in_len - consumed,
                                   "\r\n\r\n", 4);
        if (!end) break;
        size_t header_end = (size_t)(end - c->in) + 4;
        size_t header_len = header_end - consumed;
        size_t body_len = 0;
        size_t request_end = header_end;
        if (srv_is_chunked(c->in + consumed, header_len)) {
            int decoded = srv_decode_chunked(c, header_end, &body_len, &request_end);
            if (decoded < 0) return -1;
            if (decoded == 0) break;
        } else {
            if (srv_content_length(c->in + consumed, header_len, &body_len) != 0 ||
                body_len > sizeof(c->in) - header_end ||
                body_len > g_srv_body_limit)
                return -1;
            request_end = header_end + body_len;
            if (c->in_len < request_end) break;
        }
        if (srv_request_closes(c->in + consumed, header_len))
            c->close_after = 1;
        if (g_srv_handler) {
            if (srv_prepare_handler(c, c->in + consumed, header_len,
                                    c->in + header_end, body_len) != 0)
                return -1;
        } else {
            if (c->pending == UINT32_MAX) return -1;
            c->pending++;
        }
        consumed = request_end;
        if (c->close_after) break;
    }
    if (consumed) {
        c->in_len -= consumed;
        memmove(c->in, c->in + consumed, c->in_len);
    }
    return 0;
}

/* Drain all currently readable bytes and turn complete HTTP headers into a
   response count. Static serving needs no request allocation and pipelined
   requests become one integer increment each. */
static int srv_read(AxSrvConn *c) {
    for (;;) {
        if (c->in_len == sizeof(c->in)) return -1;
        ssize_t n = recv(c->fd, c->in + c->in_len,
                         sizeof(c->in) - c->in_len, 0);
        if (n > 0) {
            c->in_len += (size_t)n;
            if (srv_parse(c) != 0) return -1;
            continue;
        }
        if (n == 0) return -1;
        if (errno == EINTR) continue;
        if (errno == EAGAIN || errno == EWOULDBLOCK) return 0;
        return -1;
    }
}

/* Return 0 when drained, 1 when EPOLLOUT/EVFILT_WRITE is needed, 2 after the
   final Connection: close response, and -1 on a socket error. */
static int srv_flush(AxSrvConn *c) {
    while (c->pending) {
        const char *response = g_srv_handler ? c->out_data : g_srv_response;
        size_t response_len = g_srv_handler ? c->out_len : g_srv_response_len;
        const char *p = response + c->out_off;
        size_t left = response_len - c->out_off;
#ifdef MSG_NOSIGNAL
        ssize_t n = send(c->fd, p, left, MSG_NOSIGNAL);
#else
        ssize_t n = send(c->fd, p, left, 0);
#endif
        if (n > 0) {
            c->out_off += (size_t)n;
            if (c->out_off == response_len) {
                c->out_off = 0;
                c->pending--;
                if (g_srv_handler && !c->close_after && srv_parse(c) != 0)
                    return -1;
            }
            continue;
        }
        if (n < 0 && errno == EINTR) continue;
        if (n < 0 && (errno == EAGAIN || errno == EWOULDBLOCK)) return 1;
        return -1;
    }
    return c->close_after ? 2 : 0;
}

static int ax_http_segment(const AxStr *s, size_t *cursor,
                           const char **start, size_t *len) {
    size_t at = *cursor;
    while (at < s->len && s->ptr[at] == '/') at++;
    if (at >= s->len) {
        *cursor = at;
        *start = s->ptr + at;
        *len = 0;
        return 0;
    }
    size_t end = at;
    while (end < s->len && s->ptr[end] != '/') end++;
    *cursor = end;
    *start = s->ptr + at;
    *len = end - at;
    return 1;
}

bool ax_http_path_match(const AxStr *path, const AxStr *pattern) {
    size_t pi = 0, xi = 0;
    while (pi < pattern->len) {
        if (pattern->ptr[pi] == '*' &&
            (pi + 1 == pattern->len || pattern->ptr[pi + 1] == '/'))
            return pi + 1 == pattern->len;
        if (pattern->ptr[pi] == '{' && pi + 1 < pattern->len &&
            pattern->ptr[pi + 1] == '}') {
            const char *seg;
            size_t len;
            if (!ax_http_segment(path, &xi, &seg, &len) || len == 0) return false;
            pi += 2;
            continue;
        }
        if (pi >= pattern->len || xi >= path->len ||
            pattern->ptr[pi++] != path->ptr[xi++])
            return false;
    }
    return xi == path->len;
}

static void ax_http_decode_value(const char *ptr, size_t len, int plus_space,
                                 AxStr *out);

void ax_http_path_param(const AxStr *path, const AxStr *pattern,
                           uint16_t wanted, AxStr *out) {
    size_t pi = 0, xi = 0, found = 0;
    out->ptr = "";
    out->len = 0;
    while (pi < pattern->len) {
        if (pattern->ptr[pi] == '*' && pi + 1 == pattern->len) {
            if (found == wanted) {
                ax_http_decode_value(path->ptr + xi, path->len - xi, 0, out);
            }
            return;
        }
        if (pattern->ptr[pi] == '{' && pi + 1 < pattern->len &&
            pattern->ptr[pi + 1] == '}') {
            const char *seg;
            size_t len;
            if (!ax_http_segment(path, &xi, &seg, &len)) return;
            if (found++ == wanted) {
                ax_http_decode_value(seg, len, 0, out);
                return;
            }
            pi += 2;
            continue;
        }
        pi++;
        xi++;
    }
}

static int ax_http_ascii_equal(const char *a, size_t an,
                               const char *b, size_t bn) {
    if (an != bn) return 0;
    for (size_t i = 0; i < an; i++)
        if ((a[i] | 32) != (b[i] | 32)) return 0;
    return 1;
}

/* Decode URL values into a thread-local ring so named arguments can retain
   independent slices for the duration of one handler call. */
#define AX_HTTP_DECODE_SLOTS 16
static __thread char g_http_decode[AX_HTTP_DECODE_SLOTS][AX_SRV_INBUF];
static __thread unsigned g_http_decode_slot;

static int ax_http_hex(unsigned char c) {
    if (c >= '0' && c <= '9') return (int)(c - '0');
    if (c >= 'a' && c <= 'f') return (int)(c - 'a' + 10);
    if (c >= 'A' && c <= 'F') return (int)(c - 'A' + 10);
    return -1;
}

static void ax_http_decode_value(const char *ptr, size_t len, int plus_space,
                                 AxStr *out) {
    char *dst = g_http_decode[g_http_decode_slot++ % AX_HTTP_DECODE_SLOTS];
    size_t written = 0;
    for (size_t i = 0; i < len && written + 1 < AX_SRV_INBUF; i++) {
        if (ptr[i] == '%' && i + 2 < len) {
            int hi = ax_http_hex((unsigned char)ptr[i + 1]);
            int lo = ax_http_hex((unsigned char)ptr[i + 2]);
            if (hi >= 0 && lo >= 0) {
                dst[written++] = (char)((hi << 4) | lo);
                i += 2;
                continue;
            }
        }
        dst[written++] = plus_space && ptr[i] == '+' ? ' ' : ptr[i];
    }
    dst[written] = 0;
    out->ptr = dst;
    out->len = written;
}

void ax_http_query_param(const AxStr *query, const AxStr *name, AxStr *out) {
    out->ptr = "";
    out->len = 0;
    size_t at = 0;
    while (at < query->len) {
        size_t key = at;
        while (at < query->len && query->ptr[at] != '=' && query->ptr[at] != '&') at++;
        size_t key_len = at - key;
        size_t value = at;
        if (at < query->len && query->ptr[at] == '=') {
            value = ++at;
            while (at < query->len && query->ptr[at] != '&') at++;
        }
        if (ax_http_ascii_equal(query->ptr + key, key_len, name->ptr, name->len)) {
            ax_http_decode_value(query->ptr + value, at - value, 1, out);
            return;
        }
        if (at < query->len && query->ptr[at] == '&') at++;
    }
}

void ax_http_header(const AxStr *headers, const AxStr *name, AxStr *out) {
    out->ptr = "";
    out->len = 0;
    size_t at = 0;
    while (at < headers->len) {
        size_t line = at;
        while (at + 1 < headers->len &&
               !(headers->ptr[at] == '\r' && headers->ptr[at + 1] == '\n')) at++;
        size_t colon = line;
        while (colon < at && headers->ptr[colon] != ':') colon++;
        if (colon < at && ax_http_ascii_equal(headers->ptr + line, colon - line,
                                               name->ptr, name->len)) {
            size_t value = colon + 1;
            while (value < at && (headers->ptr[value] == ' ' || headers->ptr[value] == '\t')) value++;
            while (at > value && (headers->ptr[at - 1] == ' ' || headers->ptr[at - 1] == '\t')) at--;
            out->ptr = headers->ptr + value;
            out->len = at - value;
            return;
        }
        at += (at + 1 < headers->len) ? 2 : 1;
    }
}

void ax_http_cookie(const AxStr *headers, const AxStr *name, AxStr *out) {
    AxStr cookie_header = {"", 0};
    ax_http_header(headers, &(AxStr){"Cookie", 6}, &cookie_header);
    out->ptr = "";
    out->len = 0;
    size_t at = 0;
    while (at < cookie_header.len) {
        while (at < cookie_header.len &&
               (cookie_header.ptr[at] == ' ' || cookie_header.ptr[at] == ';')) at++;
        size_t key = at;
        while (at < cookie_header.len && cookie_header.ptr[at] != '=' &&
               cookie_header.ptr[at] != ';') at++;
        size_t key_len = at - key;
        if (at < cookie_header.len && cookie_header.ptr[at] == '=') {
            size_t value = ++at;
            while (at < cookie_header.len && cookie_header.ptr[at] != ';') at++;
            if (ax_http_ascii_equal(cookie_header.ptr + key, key_len,
                                    name->ptr, name->len)) {
                ax_http_decode_value(cookie_header.ptr + value, at - value, 0, out);
                return;
            }
        }
        while (at < cookie_header.len && cookie_header.ptr[at] != ';') at++;
        if (at < cookie_header.len) at++;
    }
}

static int srv_listener(uint16_t port) {
    int fd = socket(AF_INET, SOCK_STREAM, 0);
    if (fd < 0) return -1;
    int one = 1;
    setsockopt(fd, SOL_SOCKET, SO_REUSEADDR, &one, sizeof(one));
#ifdef SO_REUSEPORT
    setsockopt(fd, SOL_SOCKET, SO_REUSEPORT, &one, sizeof(one));
#endif
    struct sockaddr_in addr;
    memset(&addr, 0, sizeof(addr));
    addr.sin_family = AF_INET;
    addr.sin_port = htons(port);
    addr.sin_addr.s_addr = htonl(INADDR_LOOPBACK);
    if (bind(fd, (struct sockaddr *)&addr, sizeof(addr)) != 0 ||
        listen(fd, SOMAXCONN) != 0 || srv_nonblocking(fd) != 0) {
        close(fd);
        return -1;
    }
    return fd;
}

static void srv_kill(AxSrvConn *c, AxSrvConn **garbage, size_t *n_garbage) {
    if (c->dead) return;
    c->dead = 1;
    if (c->fd >= 0) close(c->fd);
    c->fd = -1;
    if (!c->queued && *n_garbage < AX_SRV_MAX_EVENTS) {
        c->queued = 1;
        garbage[(*n_garbage)++] = c;
    }
}

#if defined(__APPLE__)
static int srv_reactor(int listener) {
    int queue = kqueue();
    if (queue < 0) return -1;
    struct kevent change;
    EV_SET(&change, (uintptr_t)listener, EVFILT_READ, EV_ADD | EV_CLEAR, 0, 0, NULL);
    if (kevent(queue, &change, 1, NULL, 0, NULL) != 0) {
        close(queue);
        return -1;
    }
    struct kevent events[AX_SRV_MAX_EVENTS];
    while (g_srv_run) {
        struct timespec timeout = {1, 0};
        int count = kevent(queue, NULL, 0, events, AX_SRV_MAX_EVENTS, &timeout);
        if (count < 0) {
            if (errno == EINTR) continue;
            break;
        }
        AxSrvConn *garbage[AX_SRV_MAX_EVENTS];
        size_t n_garbage = 0;
        for (int i = 0; i < count; i++) {
            if (events[i].udata == NULL) {
                for (;;) {
                    int fd = accept(listener, NULL, NULL);
                    if (fd < 0) {
                        if (errno == EINTR) continue;
                        break;
                    }
                    if (srv_nonblocking(fd) != 0) {
                        close(fd);
                        continue;
                    }
                    srv_apply_timeout(fd);
                    AxSrvConn *c = (AxSrvConn *)calloc(1, sizeof(*c));
                    if (!c) { close(fd); continue; }
                    c->fd = fd;
                    EV_SET(&change, (uintptr_t)fd, EVFILT_READ,
                           EV_ADD | EV_CLEAR, 0, 0, c);
                    if (kevent(queue, &change, 1, NULL, 0, NULL) != 0) {
                        close(fd);
                        free(c);
                    }
                }
                continue;
            }
            AxSrvConn *c = (AxSrvConn *)events[i].udata;
            if (c->dead) continue;
            int result = 0;
            if (events[i].filter == EVFILT_READ) {
                if (srv_read(c) != 0) {
                    srv_kill(c, garbage, &n_garbage);
                    continue;
                }
                result = srv_flush(c);
            } else if (events[i].filter == EVFILT_WRITE) {
                result = srv_flush(c);
            }
            if (result < 0 || result == 2 || (events[i].flags & EV_ERROR)) {
                srv_kill(c, garbage, &n_garbage);
            } else if (result == 1) {
                EV_SET(&change, (uintptr_t)c->fd, EVFILT_WRITE,
                       EV_ADD | EV_CLEAR, 0, 0, c);
                (void)kevent(queue, &change, 1, NULL, 0, NULL);
            } else if (events[i].filter == EVFILT_WRITE) {
                EV_SET(&change, (uintptr_t)c->fd, EVFILT_WRITE,
                       EV_DELETE, 0, 0, NULL);
                (void)kevent(queue, &change, 1, NULL, 0, NULL);
            } else if ((events[i].flags & EV_EOF) && c->pending == 0) {
                srv_kill(c, garbage, &n_garbage);
            }
        }
        for (size_t i = 0; i < n_garbage; i++) free(garbage[i]);
    }
    close(queue);
    return 0;
}
#elif defined(__linux__)
static int srv_reactor(int listener) {
    int poller = epoll_create1(EPOLL_CLOEXEC);
    if (poller < 0) return -1;
    struct epoll_event event;
    memset(&event, 0, sizeof(event));
    event.events = EPOLLIN | EPOLLET;
    event.data.ptr = NULL;
    if (epoll_ctl(poller, EPOLL_CTL_ADD, listener, &event) != 0) {
        close(poller);
        return -1;
    }
    struct epoll_event events[AX_SRV_MAX_EVENTS];
    while (g_srv_run) {
        int count = epoll_wait(poller, events, AX_SRV_MAX_EVENTS, 1000);
        if (count < 0) {
            if (errno == EINTR) continue;
            break;
        }
        AxSrvConn *garbage[AX_SRV_MAX_EVENTS];
        size_t n_garbage = 0;
        for (int i = 0; i < count; i++) {
            if (events[i].data.ptr == NULL) {
                for (;;) {
                    int fd = accept(listener, NULL, NULL);
                    if (fd < 0) {
                        if (errno == EINTR) continue;
                        break;
                    }
                    if (srv_nonblocking(fd) != 0) { close(fd); continue; }
                    srv_apply_timeout(fd);
                    AxSrvConn *c = (AxSrvConn *)calloc(1, sizeof(*c));
                    if (!c) { close(fd); continue; }
                    c->fd = fd;
                    memset(&event, 0, sizeof(event));
                    event.events = EPOLLIN | EPOLLRDHUP | EPOLLET;
                    event.data.ptr = c;
                    if (epoll_ctl(poller, EPOLL_CTL_ADD, fd, &event) != 0) {
                        close(fd);
                        free(c);
                    }
                }
                continue;
            }
            AxSrvConn *c = (AxSrvConn *)events[i].data.ptr;
            if (c->dead) continue;
            int result = 0;
            if (events[i].events & EPOLLIN) {
                if (srv_read(c) != 0) {
                    srv_kill(c, garbage, &n_garbage);
                    continue;
                }
                result = srv_flush(c);
            }
            if (!c->dead && (events[i].events & EPOLLOUT)) result = srv_flush(c);
            if (result < 0 || result == 2 ||
                (events[i].events & (EPOLLERR | EPOLLHUP))) {
                srv_kill(c, garbage, &n_garbage);
                continue;
            }
            memset(&event, 0, sizeof(event));
            event.events = EPOLLIN | EPOLLRDHUP | EPOLLET;
            if (result == 1) event.events |= EPOLLOUT;
            event.data.ptr = c;
            (void)epoll_ctl(poller, EPOLL_CTL_MOD, c->fd, &event);
            if ((events[i].events & EPOLLRDHUP) && c->pending == 0)
                srv_kill(c, garbage, &n_garbage);
        }
        for (size_t i = 0; i < n_garbage; i++) free(garbage[i]);
    }
    close(poller);
    return 0;
}
#else
static int srv_reactor(int listener) {
    while (g_srv_run) {
        int fd = accept(listener, NULL, NULL);
        if (fd < 0) { if (errno == EINTR) continue; return -1; }
        char request[AX_SRV_INBUF];
        (void)recv(fd, request, sizeof(request), 0);
        (void)send_all(fd, g_srv_response, g_srv_response_len);
        close(fd);
    }
    return 0;
}
#endif

static void *srv_worker_loop(void *arg) {
    int listener = *(int *)arg;
    (void)srv_reactor(listener);
    return NULL;
}

static int srv_worker_count(void) {
    long count = 1;
    const char *configured = getenv("AX_HTTP_THREADS");
    if (configured && *configured) {
        char *end = NULL;
        long parsed = strtol(configured, &end, 10);
        if (end != configured && parsed > 0) count = parsed;
    }
    if (count < 1) count = 1;
    if (count > AX_SRV_MAX_WORKERS) count = AX_SRV_MAX_WORKERS;
    return (int)count;
}

static void srv_close_listeners(void) {
    for (int i = 0; i < g_srv_workers; i++) {
        if (g_srv_fds[i] >= 0) {
            close(g_srv_fds[i]);
            g_srv_fds[i] = -1;
        }
    }
}

static int srv_run(uint16_t port) {
    signal(SIGPIPE, SIG_IGN);
    g_srv_workers = srv_worker_count();
    for (int i = 0; i < g_srv_workers; i++) {
        g_srv_fds[i] = srv_listener(port);
        if (g_srv_fds[i] < 0) {
            if (i == 0) { g_srv_workers = 0; return -1; }
            g_srv_workers = i;
            break;
        }
    }
    g_srv_run = 1;
    g_srv_threads_started = 0;
    for (int i = 1; i < g_srv_workers; i++) {
        if (pthread_create(&g_srv_threads[g_srv_threads_started], NULL,
                           srv_worker_loop, &g_srv_fds[i]) != 0) {
            close(g_srv_fds[i]);
            g_srv_fds[i] = -1;
            continue;
        }
        g_srv_threads_started++;
    }
    (void)srv_reactor(g_srv_fds[0]);
    g_srv_run = 0;
    srv_close_listeners();
    for (int i = 0; i < g_srv_threads_started; i++)
        pthread_join(g_srv_threads[i], NULL);
    g_srv_threads_started = 0;
    g_srv_workers = 0;
    return 0;
}

int ax_http_serve_static(uint16_t port, const void *body, size_t len) {
    ax_http_stop_server();
    g_srv_handler = NULL;
    g_srv_request_desc = NULL;
    g_srv_response_desc = NULL;
    if (len > SIZE_MAX - 256) return -1;
    char *response = (char *)realloc(g_srv_response, len + 256);
    if (!response) return -1;
    g_srv_response = response;
    int header_len = snprintf(
        response, 256,
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n"
        "Content-Length: %zu\r\nConnection: keep-alive\r\n\r\n", len);
    if (header_len <= 0 || header_len >= 256) return -1;
    memcpy(response + header_len, body, len);
    g_srv_response_len = (size_t)header_len + len;
    return srv_run(port);
}

int ax_http_serve_handler(uint16_t port, void *handler,
                          const AxTypeDesc *request_desc,
                          const AxTypeDesc *response_desc) {
    AxStr empty = {"", 0};
    return ax_http_serve_handler_config(port, handler, request_desc,
                                        response_desc, AX_SRV_INBUF - 256,
                                        0, &empty);
}

int ax_http_serve_handler_config(uint16_t port, void *handler,
                                 const AxTypeDesc *request_desc,
                                 const AxTypeDesc *response_desc,
                                 uint32_t body_limit, uint32_t timeout_ms,
                                 const AxStr *cors_origin) {
    ax_http_stop_server();
    g_srv_body_limit = body_limit ? body_limit : AX_SRV_INBUF - 256;
    g_srv_timeout_ms = timeout_ms;
    free(g_srv_cors_origin);
    g_srv_cors_origin = NULL;
    g_srv_cors_origin_len = 0;
    if (cors_origin && cors_origin->len) {
        g_srv_cors_origin = (char *)malloc(cors_origin->len);
        if (!g_srv_cors_origin) return -1;
        memcpy(g_srv_cors_origin, cors_origin->ptr, cors_origin->len);
        g_srv_cors_origin_len = cors_origin->len;
    }
    if (!handler || srv_set_descriptors(request_desc, response_desc) != 0)
        return -1;
    g_srv_handler = handler;
    return srv_run(port);
}

void ax_http_stop_server(void) {
    if (!g_srv_run) return;
    g_srv_run = 0;
    srv_close_listeners();
}

/* -------------------- typed request/response API ---------------------- */

static int g_api_fd = -1;
static __thread int g_api_conn = -1;
static __thread AxBuf g_api_req;

static int api_read_request(int fd, AxBuf *b, size_t *hdr_end, size_t *body_len) {
    b->len = 0;
    for (;;) {
        if (buf_reserve(b, b->len + 4096) != 0) return -1;
        ssize_t n = recv(fd, b->data + b->len, b->cap - b->len, 0);
        if (n < 0) {
            if (errno == EINTR) continue;
            return -1;
        }
        if (n == 0) return -1;
        b->len += (size_t)n;
        if (b->len < 4) continue;
        char *end = (char *)memmem(b->data, b->len, "\r\n\r\n", 4);
        if (!end) continue;
        *hdr_end = (size_t)(end - b->data) + 4;
        *body_len = 0;
        char *cl = (char *)memmem(b->data, *hdr_end, "Content-Length:", 15);
        if (!cl) cl = (char *)memmem(b->data, *hdr_end, "content-length:", 15);
        if (cl) *body_len = (size_t)strtoull(cl + 15, NULL, 10);
        if (*hdr_end + *body_len > (1u << 20)) return -1;
        while (b->len < *hdr_end + *body_len) {
            if (buf_reserve(b, *hdr_end + *body_len) != 0) return -1;
            n = recv(fd, b->data + b->len,
                     (*hdr_end + *body_len) - b->len, 0);
            if (n < 0) {
                if (errno == EINTR) continue;
                return -1;
            }
            if (n == 0) return -1;
            b->len += (size_t)n;
        }
        return 0;
    }
}

int ax_http_listen(uint16_t port) {
    if (g_api_fd >= 0) close(g_api_fd);
    int fd = socket(AF_INET, SOCK_STREAM, 0);
    if (fd < 0) return -1;
    int one = 1;
    setsockopt(fd, SOL_SOCKET, SO_REUSEADDR, &one, sizeof(one));
    struct sockaddr_in addr;
    memset(&addr, 0, sizeof(addr));
    addr.sin_family = AF_INET;
    addr.sin_port = htons(port);
    addr.sin_addr.s_addr = htonl(INADDR_LOOPBACK);
    if (bind(fd, (struct sockaddr *)&addr, sizeof(addr)) != 0 || listen(fd, 128) != 0) {
        close(fd);
        return -1;
    }
    g_api_fd = fd;
    return 0;
}

int ax_http_accept(const AxTypeDesc *desc, void *out) {
    if (g_api_fd < 0 || !desc || !out) return -1;
    int fd;
    do {
        fd = accept(g_api_fd, NULL, NULL);
    } while (fd < 0 && errno == EINTR);
    if (fd < 0) return -1;
    g_api_conn = fd;
    size_t hdr_end = 0, body_len = 0;
    if (api_read_request(fd, &g_api_req, &hdr_end, &body_len) != 0) {
        close(fd);
        g_api_conn = -1;
        return -1;
    }
    memset(out, 0, desc->size);
    char *line = g_api_req.data;
    char *sp1 = strchr(line, ' ');
    if (!sp1) return -1;
    char *sp2 = strchr(sp1 + 1, ' ');
    if (!sp2) return -1;
    *sp1 = 0;
    *sp2 = 0;
    http_set_str(desc, out, "method", line, (size_t)(sp1 - line));
    char *path = sp1 + 1;
    size_t path_len = (size_t)(sp2 - sp1 - 1);
    char *question = (char *)memchr(path, '?', path_len);
    if (question) {
        http_set_str(desc, out, "path", path, (size_t)(question - path));
        http_set_str(desc, out, "query", question + 1,
                     path_len - (size_t)(question - path) - 1);
    } else {
        http_set_str(desc, out, "path", path, path_len);
        http_set_str(desc, out, "query", "", 0);
    }
    char *header_start = strstr(sp2 + 1, "\r\n");
    if (header_start) {
        header_start += 2;
        size_t header_len = (size_t)(g_api_req.data + hdr_end - header_start);
        if (header_len >= 2) header_len -= 2;
        http_set_str(desc, out, "headers", header_start, header_len);
    } else {
        http_set_str(desc, out, "headers", "", 0);
    }
    http_set_str(desc, out, "body", g_api_req.data + hdr_end, body_len);
    return 0;
}

static const char *status_text(uint16_t status) {
    switch (status) {
        case 200: return "OK";
        case 201: return "Created";
        case 204: return "No Content";
        case 400: return "Bad Request";
        case 404: return "Not Found";
        case 405: return "Method Not Allowed";
        case 500: return "Internal Server Error";
        default: return "Response";
    }
}

int ax_http_respond(uint16_t status, const void *body, size_t len) {
    if (g_api_conn < 0) return -1;
    char hdr[256];
    int n = snprintf(hdr, sizeof(hdr),
                     "HTTP/1.1 %u %s\r\nContent-Type: application/json\r\n"
                     "Content-Length: %zu\r\nConnection: close\r\n\r\n",
                     (unsigned)status, status_text(status), len);
    int rc = (n <= 0 || (size_t)n >= sizeof(hdr) ||
              send_all(g_api_conn, hdr, (size_t)n) != 0 ||
              send_all(g_api_conn, body, len) != 0) ? -1 : 0;
    close(g_api_conn);
    g_api_conn = -1;
    return rc;
}

void ax_http_close(void) {
    if (g_api_conn >= 0) close(g_api_conn);
    g_api_conn = -1;
}

void ax_rt_shutdown(void) {
    ax_http_stop_server();
    if (g_api_fd >= 0) {
        shutdown(g_api_fd, SHUT_RDWR);
        close(g_api_fd);
        g_api_fd = -1;
    }
    if (g_api_conn >= 0) ax_http_close();
    for (int i = 0; i < AX_POOL; i++) conn_close(&g_pool[i]);
    buf_free(&g_io);
    buf_free(&g_http);
    buf_free(&g_api_req);
    free(g_srv_response);
    g_srv_response = NULL;
    g_srv_response_len = 0;
}
