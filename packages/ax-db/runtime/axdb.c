#include "axdb.h"

#include <limits.h>
#include <pthread.h>
#include <sqlite3.h>
#include <stdlib.h>
#include <string.h>

typedef struct AxDbTx AxDbTx;

typedef struct AxDbPool {
    sqlite3 *database;
    pthread_mutex_t mutex;
    char *path;
    AxDbTx *active_tx;
    struct AxDbPool *next;
} AxDbPool;

struct AxDbTx {
    sqlite3 *database;
    AxDbPool *pool;
    AxDbTx *next;
};

static pthread_mutex_t ax_db_registry_mutex = PTHREAD_MUTEX_INITIALIZER;
static AxDbPool *ax_db_registry;
static pthread_mutex_t ax_db_tx_registry_mutex = PTHREAD_MUTEX_INITIALIZER;
static AxDbTx *ax_db_tx_registry;

static AxDbTx *ax_db_find_tx(void *handle) {
    for (AxDbTx *tx = ax_db_tx_registry; tx; tx = tx->next) {
        if (tx == handle) return tx;
    }
    return NULL;
}

static char *ax_db_cstring(const AxStr *value) {
    char *text = (char *)malloc(value->len + 1);
    if (!text) return NULL;
    memcpy(text, value->ptr, value->len);
    text[value->len] = 0;
    return text;
}

static sqlite3_stmt *ax_db_prepare(sqlite3 *database, const AxStr *sql,
                                   const AxVec *params) {
    sqlite3_stmt *statement = NULL;
    if (sqlite3_prepare_v2(database, sql->ptr, (int)sql->len, &statement, NULL) != SQLITE_OK)
        return NULL;
    uint64_t count = params ? params->len : 0;
    if ((uint64_t)sqlite3_bind_parameter_count(statement) != count) {
        sqlite3_finalize(statement);
        return NULL;
    }
    const AxStr *values = params ? (const AxStr *)params->data : NULL;
    for (uint64_t index = 0; index < count; index++) {
        if (sqlite3_bind_text64(statement, (int)index + 1, values[index].ptr,
                                values[index].len, SQLITE_TRANSIENT, SQLITE_UTF8) != SQLITE_OK) {
            sqlite3_finalize(statement);
            return NULL;
        }
    }
    return statement;
}

static const AxFieldDesc *ax_db_field(const AxTypeDesc *desc, const char *name) {
    for (uint32_t index = 0; index < desc->n_fields; index++) {
        if (strcmp(desc->fields[index].name, name) == 0) return &desc->fields[index];
    }
    return NULL;
}

static bool ax_db_store_field(const AxAlloc *alloc, sqlite3_stmt *statement,
                              int column, const AxFieldDesc *field,
                              unsigned char *record) {
    if (sqlite3_column_type(statement, column) == SQLITE_NULL) return false;
    unsigned char *slot = record + field->offset;
    if (field->kind == AX_FLD_STR) {
        if (sqlite3_column_type(statement, column) != SQLITE_TEXT) return false;
        const unsigned char *value = sqlite3_column_text(statement, column);
        int length = sqlite3_column_bytes(statement, column);
        char *copy = (char *)ax_alloc_raw(alloc, (uint64_t)length + 1, 1);
        if (!copy) return false;
        memcpy(copy, value, (size_t)length);
        copy[length] = 0;
        AxStr string = {copy, (size_t)length};
        memcpy(slot, &string, sizeof(string));
        return true;
    }
    if (field->kind == AX_FLD_F32 || field->kind == AX_FLD_F64) {
        double value = sqlite3_column_double(statement, column);
        if (field->kind == AX_FLD_F32) {
            float narrowed = (float)value;
            memcpy(slot, &narrowed, sizeof(narrowed));
        } else {
            memcpy(slot, &value, sizeof(value));
        }
        return true;
    }
    sqlite3_int64 value = sqlite3_column_int64(statement, column);
    switch (field->kind) {
        case AX_FLD_I8: if (value < INT8_MIN || value > INT8_MAX) return false; else { int8_t v = (int8_t)value; memcpy(slot, &v, 1); } break;
        case AX_FLD_I16: if (value < INT16_MIN || value > INT16_MAX) return false; else { int16_t v = (int16_t)value; memcpy(slot, &v, 2); } break;
        case AX_FLD_I32: if (value < INT32_MIN || value > INT32_MAX) return false; else { int32_t v = (int32_t)value; memcpy(slot, &v, 4); } break;
        case AX_FLD_I64: { int64_t v = (int64_t)value; memcpy(slot, &v, 8); } break;
        case AX_FLD_U8: if (value < 0 || value > UINT8_MAX) return false; else { uint8_t v = (uint8_t)value; memcpy(slot, &v, 1); } break;
        case AX_FLD_U16: if (value < 0 || value > UINT16_MAX) return false; else { uint16_t v = (uint16_t)value; memcpy(slot, &v, 2); } break;
        case AX_FLD_U32: if (value < 0 || (uint64_t)value > UINT32_MAX) return false; else { uint32_t v = (uint32_t)value; memcpy(slot, &v, 4); } break;
        case AX_FLD_U64: if (value < 0) return false; else { uint64_t v = (uint64_t)value; memcpy(slot, &v, 8); } break;
        case AX_FLD_BOOL: if (value != 0 && value != 1) return false; else { bool v = value != 0; memcpy(slot, &v, 1); } break;
        default: return false;
    }
    return true;
}

static bool ax_db_run_exec(sqlite3 *database, const AxStr *sql,
                           const AxVec *params, uint64_t *changes) {
    sqlite3_stmt *statement = ax_db_prepare(database, sql, params);
    if (!statement) return false;
    int result;
    do {
        result = sqlite3_step(statement);
    } while (result == SQLITE_ROW);
    bool ok = result == SQLITE_DONE;
    if (ok && changes) *changes = (uint64_t)sqlite3_changes(database);
    sqlite3_finalize(statement);
    return ok;
}

static bool ax_db_run_query(sqlite3 *database, const AxAlloc *alloc,
                            const AxStr *sql, const AxVec *params,
                            const AxTypeDesc *desc, AxVec *out) {
    sqlite3_stmt *statement = ax_db_prepare(database, sql, params);
    if (!statement) return false;
    int columns = sqlite3_column_count(statement);
    if (columns != (int)desc->n_fields) {
        sqlite3_finalize(statement);
        return false;
    }
    const AxFieldDesc **fields = (const AxFieldDesc **)calloc((size_t)columns, sizeof(*fields));
    if (!fields) {
        sqlite3_finalize(statement);
        return false;
    }
    for (int column = 0; column < columns; column++) {
        fields[column] = ax_db_field(desc, sqlite3_column_name(statement, column));
        if (!fields[column]) {
            free(fields);
            sqlite3_finalize(statement);
            return false;
        }
    }
    ax_rt_vec_new(alloc, desc->size, out);
    unsigned char *record = (unsigned char *)calloc(1, desc->size);
    if (!record) {
        free(fields);
        sqlite3_finalize(statement);
        return false;
    }
    bool ok = true;
    int result;
    while ((result = sqlite3_step(statement)) == SQLITE_ROW) {
        memset(record, 0, desc->size);
        for (int column = 0; column < columns; column++) {
            if (!ax_db_store_field(alloc, statement, column, fields[column], record)) {
                ok = false;
                break;
            }
        }
        if (!ok) break;
        ax_rt_vec_push(out, record, desc->size);
    }
    if (result != SQLITE_DONE) ok = false;
    if (!ok) out->len = 0;
    free(record);
    free(fields);
    sqlite3_finalize(statement);
    return ok;
}

void *ax_db_open(const AxStr *path) {
    char *name = ax_db_cstring(path);
    if (!name) return NULL;
    AxDbPool *pool = (AxDbPool *)calloc(1, sizeof(*pool));
    if (!pool) {
        free(name);
        return NULL;
    }
    int flags = SQLITE_OPEN_READWRITE | SQLITE_OPEN_CREATE | SQLITE_OPEN_FULLMUTEX;
    if (sqlite3_open_v2(name, &pool->database, flags, NULL) != SQLITE_OK) {
        if (pool->database) sqlite3_close(pool->database);
        free(name);
        free(pool);
        return NULL;
    }
    sqlite3_busy_timeout(pool->database, 5000);
    sqlite3_exec(pool->database, "PRAGMA foreign_keys = ON", NULL, NULL, NULL);
    pthread_mutex_init(&pool->mutex, NULL);
    pool->path = name;
    pthread_mutex_lock(&ax_db_registry_mutex);
    pool->next = ax_db_registry;
    ax_db_registry = pool;
    pthread_mutex_unlock(&ax_db_registry_mutex);
    return pool;
}

void ax_db_close(void *handle) {
    AxDbPool *pool = (AxDbPool *)handle;
    if (!pool) return;
    pthread_mutex_lock(&pool->mutex);
    if (pool->active_tx) {
        pthread_mutex_unlock(&pool->mutex);
        return;
    }
    pthread_mutex_lock(&ax_db_registry_mutex);
    AxDbPool **link = &ax_db_registry;
    while (*link && *link != pool) link = &(*link)->next;
    if (*link) *link = pool->next;
    pthread_mutex_unlock(&ax_db_registry_mutex);
    sqlite3_close(pool->database);
    pthread_mutex_unlock(&pool->mutex);
    pthread_mutex_destroy(&pool->mutex);
    free(pool->path);
    free(pool);
}

bool ax_db_exec(void *handle, const AxStr *sql, const AxVec *params, uint64_t *changes) {
    AxDbPool *pool = (AxDbPool *)handle;
    if (!pool) return false;
    pthread_mutex_lock(&pool->mutex);
    bool ok = !pool->active_tx &&
              ax_db_run_exec(pool->database, sql, params, changes);
    pthread_mutex_unlock(&pool->mutex);
    return ok;
}

bool ax_db_query(void *handle, const AxAlloc *alloc, const AxStr *sql,
                 const AxVec *params, const AxTypeDesc *desc, AxVec *out) {
    AxDbPool *pool = (AxDbPool *)handle;
    if (!pool) return false;
    pthread_mutex_lock(&pool->mutex);
    bool ok = !pool->active_tx &&
              ax_db_run_query(pool->database, alloc, sql, params, desc, out);
    pthread_mutex_unlock(&pool->mutex);
    return ok;
}

void *ax_db_begin(void *handle) {
    AxDbPool *pool = (AxDbPool *)handle;
    if (!pool) return NULL;
    AxDbTx *tx = (AxDbTx *)calloc(1, sizeof(*tx));
    if (!tx) return NULL;
    pthread_mutex_lock(&ax_db_tx_registry_mutex);
    pthread_mutex_lock(&pool->mutex);
    if (pool->active_tx ||
        sqlite3_exec(pool->database, "BEGIN IMMEDIATE", NULL, NULL, NULL) != SQLITE_OK) {
        pthread_mutex_unlock(&pool->mutex);
        pthread_mutex_unlock(&ax_db_tx_registry_mutex);
        free(tx);
        return NULL;
    }
    tx->database = pool->database;
    tx->pool = pool;
    pool->active_tx = tx;
    tx->next = ax_db_tx_registry;
    ax_db_tx_registry = tx;
    pthread_mutex_unlock(&pool->mutex);
    pthread_mutex_unlock(&ax_db_tx_registry_mutex);
    return tx;
}

bool ax_db_tx_exec(void *handle, const AxStr *sql, const AxVec *params, uint64_t *changes) {
    pthread_mutex_lock(&ax_db_tx_registry_mutex);
    AxDbTx *tx = ax_db_find_tx(handle);
    if (!tx) {
        pthread_mutex_unlock(&ax_db_tx_registry_mutex);
        return false;
    }
    pthread_mutex_lock(&tx->pool->mutex);
    bool ok = tx->pool->active_tx == tx &&
              ax_db_run_exec(tx->database, sql, params, changes);
    pthread_mutex_unlock(&tx->pool->mutex);
    pthread_mutex_unlock(&ax_db_tx_registry_mutex);
    return ok;
}

bool ax_db_tx_query(void *handle, const AxAlloc *alloc, const AxStr *sql,
    const AxVec *params, const AxTypeDesc *desc, AxVec *out) {
    pthread_mutex_lock(&ax_db_tx_registry_mutex);
    AxDbTx *tx = ax_db_find_tx(handle);
    if (!tx) {
        pthread_mutex_unlock(&ax_db_tx_registry_mutex);
        return false;
    }
    pthread_mutex_lock(&tx->pool->mutex);
    bool ok = tx->pool->active_tx == tx &&
              ax_db_run_query(tx->database, alloc, sql, params, desc, out);
    pthread_mutex_unlock(&tx->pool->mutex);
    pthread_mutex_unlock(&ax_db_tx_registry_mutex);
    return ok;
}

static bool ax_db_finish(void *handle, const char *sql) {
    pthread_mutex_lock(&ax_db_tx_registry_mutex);
    AxDbTx **link = &ax_db_tx_registry;
    while (*link && *link != handle) link = &(*link)->next;
    AxDbTx *tx = *link;
    if (!tx) {
        pthread_mutex_unlock(&ax_db_tx_registry_mutex);
        return false;
    }
    *link = tx->next;
    AxDbPool *pool = tx->pool;
    pthread_mutex_lock(&pool->mutex);
    bool ok = pool->active_tx == tx &&
              sqlite3_exec(tx->database, sql, NULL, NULL, NULL) == SQLITE_OK;
    if (!ok && pool->active_tx == tx)
        sqlite3_exec(tx->database, "ROLLBACK", NULL, NULL, NULL);
    if (pool->active_tx == tx) pool->active_tx = NULL;
    pthread_mutex_unlock(&pool->mutex);
    free(tx);
    pthread_mutex_unlock(&ax_db_tx_registry_mutex);
    return ok;
}

bool ax_db_commit(void *tx) { return ax_db_finish(tx, "COMMIT"); }
bool ax_db_rollback(void *tx) { return ax_db_finish(tx, "ROLLBACK"); }

void ax_db_shutdown(void) {
    pthread_mutex_lock(&ax_db_tx_registry_mutex);
    AxDbTx *transactions = ax_db_tx_registry;
    ax_db_tx_registry = NULL;
    pthread_mutex_unlock(&ax_db_tx_registry_mutex);
    while (transactions) {
        AxDbTx *tx = transactions;
        transactions = tx->next;
        pthread_mutex_lock(&tx->pool->mutex);
        if (tx->pool->active_tx == tx) {
            sqlite3_exec(tx->database, "ROLLBACK", NULL, NULL, NULL);
            tx->pool->active_tx = NULL;
        }
        pthread_mutex_unlock(&tx->pool->mutex);
        free(tx);
    }
    for (;;) {
        pthread_mutex_lock(&ax_db_registry_mutex);
        AxDbPool *pool = ax_db_registry;
        if (pool) ax_db_registry = pool->next;
        pthread_mutex_unlock(&ax_db_registry_mutex);
        if (!pool) break;
        pthread_mutex_lock(&pool->mutex);
        sqlite3_close(pool->database);
        pthread_mutex_unlock(&pool->mutex);
        pthread_mutex_destroy(&pool->mutex);
        free(pool->path);
        free(pool);
    }
}
