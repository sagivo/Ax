#include "axdb_mysql.h"

#include <dlfcn.h>
#include <errno.h>
#include <limits.h>
#include <pthread.h>
#include <stdbool.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

typedef struct AxMysql MYSQL;
typedef struct AxMysqlResult MYSQL_RES;
typedef char **MYSQL_ROW;
typedef struct { char *name; } AxMysqlField;

typedef struct {
    void *library;
    MYSQL *(*init)(MYSQL *);
    int (*options)(MYSQL *, int, const void *);
    MYSQL *(*connect)(MYSQL *, const char *, const char *, const char *, const char *, unsigned int, const char *, unsigned long);
    void (*close)(MYSQL *);
    int (*query)(MYSQL *, const char *, unsigned long);
    MYSQL_RES *(*store)(MYSQL *);
    unsigned int (*field_count)(MYSQL *);
    unsigned int (*num_fields)(MYSQL_RES *);
    AxMysqlField *(*fields)(MYSQL_RES *);
    MYSQL_ROW (*row)(MYSQL_RES *);
    unsigned long *(*lengths)(MYSQL_RES *);
    void (*free_result)(MYSQL_RES *);
    unsigned long long (*affected_rows)(MYSQL *);
    const char *(*error)(MYSQL *);
    int (*autocommit)(MYSQL *, bool);
    int (*commit)(MYSQL *);
    int (*rollback)(MYSQL *);
} AxMysqlApi;

typedef struct AxDbValue {
    int32_t tag;
    union {
        AxStr text;
        int64_t i64;
        uint64_t u64;
        double f64;
        bool boolean;
    } value;
} AxDbValue;

typedef struct AxDbTx AxDbTx;
typedef struct AxDbPool {
    MYSQL *database;
    pthread_mutex_t mutex;
    char *dsn;
    AxDbTx *active_tx;
    uint32_t timeout_ms;
    struct AxDbPool *next;
} AxDbPool;

struct AxDbTx {
    AxDbPool *pool;
    AxDbTx *next;
};

static AxMysqlApi ax_mysql;
static pthread_once_t ax_mysql_once = PTHREAD_ONCE_INIT;
static bool ax_mysql_loaded;
static pthread_mutex_t ax_db_registry_mutex = PTHREAD_MUTEX_INITIALIZER;
static AxDbPool *ax_db_registry;
static pthread_mutex_t ax_db_tx_registry_mutex = PTHREAD_MUTEX_INITIALIZER;
static AxDbTx *ax_db_tx_registry;

static void *ax_mysql_symbol(const char *name) {
    return dlsym(ax_mysql.library, name);
}

static void ax_mysql_load_once(void) {
    const char *configured = getenv("AX_MYSQL_LIB");
    const char *names[] = {
        "libmysqlclient.so",
        "libmysqlclient.so.21",
        "libmysqlclient.dylib",
        "libmysqlclient.24.dylib",
        NULL,
    };
    if (configured) ax_mysql.library = dlopen(configured, RTLD_NOW | RTLD_LOCAL);
    for (size_t i = 0; !ax_mysql.library && names[i]; i++) {
        ax_mysql.library = dlopen(names[i], RTLD_NOW | RTLD_LOCAL);
        if (ax_mysql.library) break;
    }
    if (!ax_mysql.library) return;
#define AX_MYSQL_LOAD(field, symbol) do { \
        *(void **)(&ax_mysql.field) = ax_mysql_symbol(symbol); \
        if (!ax_mysql.field) return; \
    } while (0)
    AX_MYSQL_LOAD(init, "mysql_init");
    AX_MYSQL_LOAD(options, "mysql_options");
    AX_MYSQL_LOAD(connect, "mysql_real_connect");
    AX_MYSQL_LOAD(close, "mysql_close");
    AX_MYSQL_LOAD(query, "mysql_real_query");
    AX_MYSQL_LOAD(store, "mysql_store_result");
    AX_MYSQL_LOAD(field_count, "mysql_field_count");
    AX_MYSQL_LOAD(num_fields, "mysql_num_fields");
    AX_MYSQL_LOAD(fields, "mysql_fetch_fields");
    AX_MYSQL_LOAD(row, "mysql_fetch_row");
    AX_MYSQL_LOAD(lengths, "mysql_fetch_lengths");
    AX_MYSQL_LOAD(free_result, "mysql_free_result");
    AX_MYSQL_LOAD(affected_rows, "mysql_affected_rows");
    AX_MYSQL_LOAD(error, "mysql_error");
    AX_MYSQL_LOAD(autocommit, "mysql_autocommit");
    AX_MYSQL_LOAD(commit, "mysql_commit");
    AX_MYSQL_LOAD(rollback, "mysql_rollback");
#undef AX_MYSQL_LOAD
    ax_mysql_loaded = true;
}

static bool ax_mysql_ready(void) {
    pthread_once(&ax_mysql_once, ax_mysql_load_once);
    return ax_mysql_loaded;
}

static char *ax_db_copy(const char *value, size_t length) {
    char *copy = (char *)malloc(length + 1);
    if (!copy) return NULL;
    memcpy(copy, value, length);
    copy[length] = 0;
    return copy;
}

typedef struct {
    char *user;
    char *password;
    char *host;
    char *database;
    unsigned port;
} AxMysqlDsn;

static void ax_mysql_dsn_free(AxMysqlDsn *dsn) {
    free(dsn->user);
    free(dsn->password);
    free(dsn->host);
    free(dsn->database);
    memset(dsn, 0, sizeof(*dsn));
}

static bool ax_mysql_parse_dsn(const AxStr *value, AxMysqlDsn *out) {
    memset(out, 0, sizeof(*out));
    const char *start = value->ptr;
    size_t length = value->len;
    const char prefix[] = "mysql://";
    if (length < sizeof(prefix) - 1 || memcmp(start, prefix, sizeof(prefix) - 1) != 0)
        return false;
    start += sizeof(prefix) - 1;
    length -= sizeof(prefix) - 1;
    const char *slash = memchr(start, '/', length);
    if (!slash || slash == start) return false;
    const char *authority_end = slash;
    const char *at = NULL;
    for (const char *cursor = start; cursor < authority_end; cursor++)
        if (*cursor == '@') at = cursor;
    const char *host_start = start;
    if (at) {
        const char *colon = memchr(start, ':', (size_t)(at - start));
        if (colon) {
            out->user = ax_db_copy(start, (size_t)(colon - start));
            out->password = ax_db_copy(colon + 1, (size_t)(at - colon - 1));
        } else {
            out->user = ax_db_copy(start, (size_t)(at - start));
            out->password = ax_db_copy("", 0);
        }
        host_start = at + 1;
    } else {
        out->user = ax_db_copy("root", 4);
        out->password = ax_db_copy("", 0);
    }
    const char *host_end = authority_end;
    const char *port_colon = memchr(host_start, ':', (size_t)(host_end - host_start));
    if (port_colon) {
        out->host = ax_db_copy(host_start, (size_t)(port_colon - host_start));
        char *port = ax_db_copy(port_colon + 1, (size_t)(host_end - port_colon - 1));
        if (!port) { ax_mysql_dsn_free(out); return false; }
        char *end = NULL;
        unsigned long parsed = strtoul(port, &end, 10);
        bool valid = end && *end == 0;
        free(port);
        if (!valid || parsed > UINT16_MAX) { ax_mysql_dsn_free(out); return false; }
        out->port = (unsigned)parsed;
    } else {
        out->host = ax_db_copy(host_start, (size_t)(host_end - host_start));
        out->port = 3306;
    }
    out->database = ax_db_copy(slash + 1, length - (size_t)(slash - start) - 1);
    if (!out->user || !out->password || !out->host || !out->database || !out->host[0] || !out->database[0]) {
        ax_mysql_dsn_free(out);
        return false;
    }
    return true;
}

static bool ax_db_append(char **buffer, size_t *length, size_t *capacity,
                         const char *value, size_t count) {
    if (*length > SIZE_MAX - count - 1) return false;
    size_t needed = *length + count + 1;
    if (needed > *capacity) {
        size_t next = *capacity ? *capacity : 128;
        while (next < needed) {
            if (next > SIZE_MAX / 2) return false;
            next *= 2;
        }
        char *grown = (char *)realloc(*buffer, next);
        if (!grown) return false;
        *buffer = grown;
        *capacity = next;
    }
    memcpy(*buffer + *length, value, count);
    *length += count;
    (*buffer)[*length] = 0;
    return true;
}

static bool ax_db_append_text(char **buffer, size_t *length, size_t *capacity,
                              const AxStr *value) {
    if (!ax_db_append(buffer, length, capacity, "'", 1)) return false;
    for (size_t i = 0; i < value->len; i++) {
        unsigned char c = (unsigned char)value->ptr[i];
        if (c == '\\' || c == '\'' || c == '"' || c == '\n' || c == '\r' || c == 26 || c == 0) {
            char slash = '\\';
            if (!ax_db_append(buffer, length, capacity, &slash, 1)) return false;
            char escaped = c == '\n' ? 'n' : c == '\r' ? 'r' : c == 26 ? 'Z' : c;
            if (!ax_db_append(buffer, length, capacity, &escaped, 1)) return false;
        } else if (!ax_db_append(buffer, length, capacity, (const char *)&c, 1)) {
            return false;
        }
    }
    return ax_db_append(buffer, length, capacity, "'", 1);
}

static bool ax_db_append_value(char **buffer, size_t *length, size_t *capacity,
                               const AxDbValue *value) {
    char number[96];
    switch (value->tag) {
        case 0: return ax_db_append(buffer, length, capacity, "NULL", 4);
        case 1: return ax_db_append_text(buffer, length, capacity, &value->value.text);
        case 2: snprintf(number, sizeof(number), "%lld", (long long)value->value.i64); break;
        case 3: snprintf(number, sizeof(number), "%llu", (unsigned long long)value->value.u64); break;
        case 4: snprintf(number, sizeof(number), "%.17g", value->value.f64); break;
        case 5: return ax_db_append(buffer, length, capacity, value->value.boolean ? "1" : "0", 1);
        default: return false;
    }
    return ax_db_append(buffer, length, capacity, number, strlen(number));
}

static char *ax_db_interpolate(const AxStr *sql, const AxVec *params, bool values) {
    uint64_t count = params ? params->len : 0;
    const AxStr *text = params ? (const AxStr *)params->data : NULL;
    const AxDbValue *typed = params ? (const AxDbValue *)params->data : NULL;
    char *out = NULL;
    size_t length = 0, capacity = 0, index = 0;
    bool single = false, double_quote = false, backtick = false;
    for (size_t i = 0; i < sql->len; i++) {
        char c = sql->ptr[i];
        if ((single || double_quote) && c == '\\' && i + 1 < sql->len) {
            if (!ax_db_append(&out, &length, &capacity, sql->ptr + i, 2)) { free(out); return NULL; }
            i++;
            continue;
        }
        if (single && c == '\'' && i + 1 < sql->len && sql->ptr[i + 1] == '\'') {
            if (!ax_db_append(&out, &length, &capacity, sql->ptr + i, 2)) { free(out); return NULL; }
            i++;
            continue;
        }
        if (double_quote && c == '"' && i + 1 < sql->len && sql->ptr[i + 1] == '"') {
            if (!ax_db_append(&out, &length, &capacity, sql->ptr + i, 2)) { free(out); return NULL; }
            i++;
            continue;
        }
        if (c == '\'' && !double_quote && !backtick) single = !single;
        if (c == '"' && !single && !backtick) double_quote = !double_quote;
        if (c == '`' && !single && !double_quote) backtick = !backtick;
        if (c == '?' && !single && !double_quote && !backtick) {
            if (index >= count) { free(out); return NULL; }
            bool ok = values
                ? ax_db_append_value(&out, &length, &capacity, &typed[index])
                : ax_db_append_text(&out, &length, &capacity, &text[index]);
            if (!ok) { free(out); return NULL; }
            index++;
        } else if (!ax_db_append(&out, &length, &capacity, &c, 1)) {
            free(out); return NULL;
        }
    }
    if (index != count || !out) {
        if (!out && index == count && count == 0) out = ax_db_copy(sql->ptr, sql->len);
        else free(out);
        return out;
    }
    return out;
}

static const AxFieldDesc *ax_db_field(const AxTypeDesc *desc, const char *name) {
    for (uint32_t i = 0; i < desc->n_fields; i++)
        if (strcmp(desc->fields[i].name, name) == 0) return &desc->fields[i];
    return NULL;
}

static bool ax_db_number(const char *text, size_t length, long long *signed_value,
                         unsigned long long *unsigned_value, bool *negative) {
    char *copy = ax_db_copy(text, length);
    if (!copy) return false;
    char *end = NULL;
    errno = 0;
    long long signed_result = strtoll(copy, &end, 10);
    if (errno == 0 && end && *end == 0) {
        *signed_value = signed_result;
        *negative = signed_result < 0;
        free(copy);
        return true;
    }
    errno = 0;
    unsigned long long unsigned_result = strtoull(copy, &end, 10);
    bool ok = errno == 0 && end && *end == 0;
    if (ok) *unsigned_value = unsigned_result;
    free(copy);
    return ok;
}

static bool ax_db_store_field(const AxAlloc *alloc, MYSQL_ROW row, unsigned long *lengths,
                              unsigned column, const AxFieldDesc *field, unsigned char *record) {
    unsigned char *slot = record + field->offset;
    AxFieldKind kind = field->kind;
    bool optional = kind >= AX_FLD_OPT_I8;
    if (optional) {
        kind = (AxFieldKind)(kind - AX_FLD_OPT_I8);
        int32_t tag = row[column] ? 1 : 0;
        memcpy(slot, &tag, sizeof(tag));
        if (!row[column]) return true;
        slot += 8;
    } else if (!row[column]) return false;
    const char *text = row[column];
    size_t length = lengths[column];
    if (kind == AX_FLD_STR) {
        char *copy = (char *)ax_alloc_raw(alloc, length + 1, 1);
        if (!copy) return false;
        memcpy(copy, text, length); copy[length] = 0;
        AxStr value = {copy, length}; memcpy(slot, &value, sizeof(value)); return true;
    }
    if (kind == AX_FLD_F32 || kind == AX_FLD_F64) {
        char *copy = ax_db_copy(text, length); if (!copy) return false;
        char *end = NULL; errno = 0; double value = strtod(copy, &end);
        bool ok = errno == 0 && end && *end == 0; free(copy); if (!ok) return false;
        if (kind == AX_FLD_F32) { float narrowed = (float)value; memcpy(slot, &narrowed, 4); }
        else memcpy(slot, &value, 8);
        return true;
    }
    long long signed_value = 0; unsigned long long unsigned_value = 0; bool negative = false;
    if (!ax_db_number(text, length, &signed_value, &unsigned_value, &negative)) return false;
    switch (kind) {
        case AX_FLD_I8: if (signed_value < INT8_MIN || signed_value > INT8_MAX) return false; { int8_t v = (int8_t)signed_value; memcpy(slot, &v, 1); } break;
        case AX_FLD_I16: if (signed_value < INT16_MIN || signed_value > INT16_MAX) return false; { int16_t v = (int16_t)signed_value; memcpy(slot, &v, 2); } break;
        case AX_FLD_I32: if (signed_value < INT32_MIN || signed_value > INT32_MAX) return false; { int32_t v = (int32_t)signed_value; memcpy(slot, &v, 4); } break;
        case AX_FLD_I64: { int64_t v = (int64_t)signed_value; memcpy(slot, &v, 8); } break;
        case AX_FLD_U8: if (negative || unsigned_value > UINT8_MAX) return false; { uint8_t v = (uint8_t)unsigned_value; memcpy(slot, &v, 1); } break;
        case AX_FLD_U16: if (negative || unsigned_value > UINT16_MAX) return false; { uint16_t v = (uint16_t)unsigned_value; memcpy(slot, &v, 2); } break;
        case AX_FLD_U32: if (negative || unsigned_value > UINT32_MAX) return false; { uint32_t v = (uint32_t)unsigned_value; memcpy(slot, &v, 4); } break;
        case AX_FLD_U64: if (negative) return false; { uint64_t v = (uint64_t)unsigned_value; memcpy(slot, &v, 8); } break;
        case AX_FLD_BOOL: if (signed_value != 0 && signed_value != 1) return false; { bool v = signed_value != 0; memcpy(slot, &v, 1); } break;
        default: return false;
    }
    return true;
}

static bool ax_db_exec_raw(AxDbPool *pool, const AxStr *sql, const AxVec *params,
                           uint64_t *changes, bool values) {
    char *query = ax_db_interpolate(sql, params, values);
    if (!query || ax_mysql.query(pool->database, query, (unsigned long)strlen(query)) != 0) {
        free(query); return false;
    }
    MYSQL_RES *result = ax_mysql.store(pool->database);
    if (result) ax_mysql.free_result(result);
    if (changes) *changes = (uint64_t)ax_mysql.affected_rows(pool->database);
    free(query); return true;
}

static bool ax_db_query_raw(AxDbPool *pool, const AxAlloc *alloc, const AxStr *sql,
                            const AxVec *params, const AxTypeDesc *desc, AxVec *out,
                            bool values) {
    char *query = ax_db_interpolate(sql, params, values);
    if (!query || ax_mysql.query(pool->database, query, (unsigned long)strlen(query)) != 0) {
        free(query); return false;
    }
    free(query);
    MYSQL_RES *result = ax_mysql.store(pool->database);
    if (!result || ax_mysql.num_fields(result) != desc->n_fields) {
        if (result) ax_mysql.free_result(result); return false;
    }
    unsigned columns = ax_mysql.num_fields(result);
    AxMysqlField *metadata = ax_mysql.fields(result);
    const AxFieldDesc **fields = (const AxFieldDesc **)calloc(columns, sizeof(*fields));
    if (!fields || !metadata) { free(fields); ax_mysql.free_result(result); return false; }
    for (unsigned i = 0; i < columns; i++) {
        fields[i] = ax_db_field(desc, metadata[i].name);
        if (!fields[i]) { free(fields); ax_mysql.free_result(result); return false; }
    }
    ax_rt_vec_new(alloc, desc->size, out);
    unsigned char *record = (unsigned char *)calloc(1, desc->size);
    if (!record) { free(fields); ax_mysql.free_result(result); return false; }
    bool ok = true;
    MYSQL_ROW row;
    while ((row = ax_mysql.row(result))) {
        unsigned long *lengths = ax_mysql.lengths(result);
        memset(record, 0, desc->size);
        for (unsigned i = 0; i < columns; i++) {
            if (!lengths || !ax_db_store_field(alloc, row, lengths, i, fields[i], record)) { ok = false; break; }
        }
        if (!ok) break;
        ax_rt_vec_push(out, record, desc->size);
    }
    if (!ok) out->len = 0;
    free(record); free(fields); ax_mysql.free_result(result); return ok;
}

static AxDbTx *ax_db_find_tx(void *handle) {
    for (AxDbTx *tx = ax_db_tx_registry; tx; tx = tx->next) if (tx == handle) return tx;
    return NULL;
}

void *ax_db_open_timeout(const AxStr *path, uint32_t timeout_ms) {
    if (!ax_mysql_ready()) return NULL;
    AxMysqlDsn dsn;
    if (!ax_mysql_parse_dsn(path, &dsn)) return NULL;
    MYSQL *database = ax_mysql.init(NULL);
    if (!database) { ax_mysql_dsn_free(&dsn); return NULL; }
    unsigned seconds = timeout_ms ? (timeout_ms + 999) / 1000 : 0;
    if (seconds) {
        ax_mysql.options(database, 0, &seconds);
        ax_mysql.options(database, 11, &seconds);
        ax_mysql.options(database, 12, &seconds);
    }
    if (!ax_mysql.connect(database, dsn.host, dsn.user, dsn.password, dsn.database, dsn.port, NULL, 0)) {
        ax_mysql.close(database); ax_mysql_dsn_free(&dsn); return NULL;
    }
    AxDbPool *pool = (AxDbPool *)calloc(1, sizeof(*pool));
    if (!pool) { ax_mysql.close(database); ax_mysql_dsn_free(&dsn); return NULL; }
    pool->database = database; pool->timeout_ms = timeout_ms; pool->dsn = ax_db_copy(path->ptr, path->len);
    ax_mysql_dsn_free(&dsn);
    if (!pool->dsn) { ax_mysql.close(database); free(pool); return NULL; }
    pthread_mutex_init(&pool->mutex, NULL);
    pthread_mutex_lock(&ax_db_registry_mutex); pool->next = ax_db_registry; ax_db_registry = pool; pthread_mutex_unlock(&ax_db_registry_mutex);
    return pool;
}

void *ax_db_open(const AxStr *path) { return ax_db_open_timeout(path, 5000); }

bool ax_db_set_timeout(void *handle, uint32_t timeout_ms) {
    AxDbPool *pool = (AxDbPool *)handle; if (!pool) return false;
    pthread_mutex_lock(&pool->mutex); pool->timeout_ms = timeout_ms; pthread_mutex_unlock(&pool->mutex); return true;
}

void ax_db_close(void *handle) {
    AxDbPool *pool = (AxDbPool *)handle; if (!pool) return;
    pthread_mutex_lock(&pool->mutex); if (pool->active_tx) { pthread_mutex_unlock(&pool->mutex); return; }
    pthread_mutex_lock(&ax_db_registry_mutex); AxDbPool **link = &ax_db_registry; while (*link && *link != pool) link = &(*link)->next; if (*link) *link = pool->next; pthread_mutex_unlock(&ax_db_registry_mutex);
    ax_mysql.close(pool->database); pthread_mutex_unlock(&pool->mutex); pthread_mutex_destroy(&pool->mutex); free(pool->dsn); free(pool);
}

static bool ax_db_exec_impl(void *handle, const AxStr *sql, const AxVec *params, uint64_t *changes, bool values) {
    AxDbPool *pool = (AxDbPool *)handle; if (!pool) return false; pthread_mutex_lock(&pool->mutex); bool ok = !pool->active_tx && ax_db_exec_raw(pool, sql, params, changes, values); pthread_mutex_unlock(&pool->mutex); return ok;
}
bool ax_db_exec(void *h, const AxStr *s, const AxVec *p, uint64_t *c) { return ax_db_exec_impl(h, s, p, c, false); }
bool ax_db_exec_values(void *h, const AxStr *s, const AxVec *p, uint64_t *c) { return ax_db_exec_impl(h, s, p, c, true); }

static bool ax_db_query_impl(void *handle, const AxAlloc *alloc, const AxStr *sql, const AxVec *params, const AxTypeDesc *desc, AxVec *out, bool values) {
    AxDbPool *pool = (AxDbPool *)handle; if (!pool) return false; pthread_mutex_lock(&pool->mutex); bool ok = !pool->active_tx && ax_db_query_raw(pool, alloc, sql, params, desc, out, values); pthread_mutex_unlock(&pool->mutex); return ok;
}
bool ax_db_query(void *h, const AxAlloc *a, const AxStr *s, const AxVec *p, const AxTypeDesc *d, AxVec *o) { return ax_db_query_impl(h, a, s, p, d, o, false); }
bool ax_db_query_values(void *h, const AxAlloc *a, const AxStr *s, const AxVec *p, const AxTypeDesc *d, AxVec *o) { return ax_db_query_impl(h, a, s, p, d, o, true); }

void *ax_db_begin(void *handle) {
    AxDbPool *pool = (AxDbPool *)handle; if (!pool) return NULL;
    AxDbTx *tx = (AxDbTx *)calloc(1, sizeof(*tx)); if (!tx) return NULL;
    pthread_mutex_lock(&ax_db_tx_registry_mutex); pthread_mutex_lock(&pool->mutex);
    bool ok = !pool->active_tx && ax_mysql.autocommit(pool->database, false) == 0;
    if (ok) { tx->pool = pool; pool->active_tx = tx; tx->next = ax_db_tx_registry; ax_db_tx_registry = tx; }
    pthread_mutex_unlock(&pool->mutex); pthread_mutex_unlock(&ax_db_tx_registry_mutex);
    if (!ok) free(tx); return ok ? tx : NULL;
}

static bool ax_db_tx_exec_impl(void *handle, const AxStr *sql, const AxVec *params, uint64_t *changes, bool values) {
    pthread_mutex_lock(&ax_db_tx_registry_mutex); AxDbTx *tx = ax_db_find_tx(handle); if (!tx) { pthread_mutex_unlock(&ax_db_tx_registry_mutex); return false; }
    pthread_mutex_lock(&tx->pool->mutex); bool ok = tx->pool->active_tx == tx && ax_db_exec_raw(tx->pool, sql, params, changes, values); pthread_mutex_unlock(&tx->pool->mutex); pthread_mutex_unlock(&ax_db_tx_registry_mutex); return ok;
}
bool ax_db_tx_exec(void *h, const AxStr *s, const AxVec *p, uint64_t *c) { return ax_db_tx_exec_impl(h, s, p, c, false); }
bool ax_db_tx_exec_values(void *h, const AxStr *s, const AxVec *p, uint64_t *c) { return ax_db_tx_exec_impl(h, s, p, c, true); }

static bool ax_db_tx_query_impl(void *handle, const AxAlloc *alloc, const AxStr *sql, const AxVec *params, const AxTypeDesc *desc, AxVec *out, bool values) {
    pthread_mutex_lock(&ax_db_tx_registry_mutex); AxDbTx *tx = ax_db_find_tx(handle); if (!tx) { pthread_mutex_unlock(&ax_db_tx_registry_mutex); return false; }
    pthread_mutex_lock(&tx->pool->mutex); bool ok = tx->pool->active_tx == tx && ax_db_query_raw(tx->pool, alloc, sql, params, desc, out, values); pthread_mutex_unlock(&tx->pool->mutex); pthread_mutex_unlock(&ax_db_tx_registry_mutex); return ok;
}
bool ax_db_tx_query(void *h, const AxAlloc *a, const AxStr *s, const AxVec *p, const AxTypeDesc *d, AxVec *o) { return ax_db_tx_query_impl(h, a, s, p, d, o, false); }
bool ax_db_tx_query_values(void *h, const AxAlloc *a, const AxStr *s, const AxVec *p, const AxTypeDesc *d, AxVec *o) { return ax_db_tx_query_impl(h, a, s, p, d, o, true); }

static bool ax_db_finish(void *handle, bool commit) {
    pthread_mutex_lock(&ax_db_tx_registry_mutex); AxDbTx **link = &ax_db_tx_registry; while (*link && *link != handle) link = &(*link)->next; AxDbTx *tx = *link; if (!tx) { pthread_mutex_unlock(&ax_db_tx_registry_mutex); return false; }
    *link = tx->next; pthread_mutex_lock(&tx->pool->mutex); bool ok = tx->pool->active_tx == tx && (commit ? ax_mysql.commit(tx->pool->database) == 0 : ax_mysql.rollback(tx->pool->database) == 0);
    if (tx->pool->active_tx == tx) { ax_mysql.autocommit(tx->pool->database, true); tx->pool->active_tx = NULL; }
    pthread_mutex_unlock(&tx->pool->mutex); free(tx); pthread_mutex_unlock(&ax_db_tx_registry_mutex); return ok;
}
bool ax_db_commit(void *tx) { return ax_db_finish(tx, true); }
bool ax_db_rollback(void *tx) { return ax_db_finish(tx, false); }

void ax_db_shutdown(void) {
    pthread_mutex_lock(&ax_db_tx_registry_mutex); AxDbTx *transactions = ax_db_tx_registry; ax_db_tx_registry = NULL; pthread_mutex_unlock(&ax_db_tx_registry_mutex);
    while (transactions) { AxDbTx *tx = transactions; transactions = tx->next; pthread_mutex_lock(&tx->pool->mutex); if (tx->pool->active_tx == tx) { ax_mysql.rollback(tx->pool->database); ax_mysql.autocommit(tx->pool->database, true); tx->pool->active_tx = NULL; } pthread_mutex_unlock(&tx->pool->mutex); free(tx); }
    for (;;) {
        pthread_mutex_lock(&ax_db_registry_mutex); AxDbPool *pool = ax_db_registry; if (pool) ax_db_registry = pool->next; pthread_mutex_unlock(&ax_db_registry_mutex); if (!pool) break;
        pthread_mutex_lock(&pool->mutex); ax_mysql.close(pool->database); pthread_mutex_unlock(&pool->mutex); pthread_mutex_destroy(&pool->mutex); free(pool->dsn); free(pool);
    }
}
