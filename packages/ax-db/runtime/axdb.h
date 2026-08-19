#ifndef AXDB_H
#define AXDB_H

#include "axrt.h"

void *ax_db_open(const AxStr *path);
void *ax_db_open_timeout(const AxStr *path, uint32_t timeout_ms);
bool ax_db_set_timeout(void *pool, uint32_t timeout_ms);
void ax_db_close(void *pool);
bool ax_db_exec(void *pool, const AxStr *sql, const AxVec *params, uint64_t *changes);
bool ax_db_exec_values(void *pool, const AxStr *sql, const AxVec *params, uint64_t *changes);
bool ax_db_query(void *pool, const AxAlloc *alloc, const AxStr *sql,
                 const AxVec *params, const AxTypeDesc *desc, AxVec *out);
bool ax_db_query_values(void *pool, const AxAlloc *alloc, const AxStr *sql,
                        const AxVec *params, const AxTypeDesc *desc, AxVec *out);
void *ax_db_begin(void *pool);
bool ax_db_tx_exec(void *tx, const AxStr *sql, const AxVec *params, uint64_t *changes);
bool ax_db_tx_exec_values(void *tx, const AxStr *sql, const AxVec *params, uint64_t *changes);
bool ax_db_tx_query(void *tx, const AxAlloc *alloc, const AxStr *sql,
                    const AxVec *params, const AxTypeDesc *desc, AxVec *out);
bool ax_db_tx_query_values(void *tx, const AxAlloc *alloc, const AxStr *sql,
                           const AxVec *params, const AxTypeDesc *desc, AxVec *out);
bool ax_db_commit(void *tx);
bool ax_db_rollback(void *tx);
void ax_db_shutdown(void);

#endif
