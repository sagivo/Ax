/* Ax native runtime — zero-copy IO, pooled HTTP, bump allocation. */
#ifndef AXRT_H
#define AXRT_H

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

typedef struct {
    const char *ptr;
    size_t len;
} AxStr;

/* ---- IO ---------------------------------------------------------------- */

/* Read entire file. Buffer is reused across calls (thread-local).
   Returned pointer is valid until the next io call on this thread. */
int ax_io_read_file(const char *path, AxStr *out);

/* Checksum every byte of a file without an intermediate String. */
int ax_io_bytesum_file(const char *path, uint64_t *out);

/* Write bytes. */
int ax_io_write_file(const char *path, const void *data, size_t len);

/* ---- HTTP -------------------------------------------------------------- */

/* GET url. Uses a keep-alive pool keyed by host:port.
   Body pointer valid until the next http call on this thread. */
int ax_http_get(const char *url, AxStr *out);

/* GET and checksum the body in one shot. */
int ax_http_get_bytesum(const char *url, uint64_t *out);

/* Serve `body` on 127.0.0.1:`port` until the process is stopped. Uses the same
   non-blocking keep-alive reactor as typed handlers. */
int ax_http_serve_static(uint16_t port, const void *body, size_t len);

void ax_http_stop_server(void);

/* ---- util -------------------------------------------------------------- */

uint64_t ax_bytesum(const void *p, size_t n);
void ax_rt_shutdown(void);

/* ======================================================================== *
 * Language ABI — everything below is what generated code calls.
 *
 * The `ax_rt_*` functions are the capability-typed standard library. The
 * `static inline` helpers exist because C leaves undefined what Ax defines:
 * signed division overflow, shift counts past the width, float->int range,
 * and NaN bit patterns. Generated code never emits a raw C `/`, `%`, `<<`,
 * or `>>` on integers.
 * ======================================================================== */

#include <stdbool.h>

/* `unit` needs a representation because it can be an SSA value. */
typedef unsigned char AxUnit;

/* ---- process ----------------------------------------------------------- */

void ax_rt_init(int argc, char **argv);
/* Prints `msg` to stderr and exits non-zero. Aborts are observable behaviour
   and must match the oracle interpreter's abort messages exactly. */
_Noreturn void ax_abort(const char *msg);
_Noreturn void ax_unreachable(void);
void ax_test_passed(const char *name);
void ax_test_failed(const char *name);

/* ---- bump arenas (regions) --------------------------------------------- */

/* A region is a bump arena with lexical extent. `store(&r T, l)` is checked so
   that nothing outlives the arena, which is what makes release-at-scope safe. */
typedef struct {
    unsigned char *base;
    size_t cap;
    size_t used;
    /* First block is inline, so a small region never touches malloc. */
    unsigned char inline_buf[4096];
    void *spill;
    /* The most recent allocation. A growing buffer that is still the top of the
       bump pointer can be extended in place instead of copied — something a
       general-purpose allocator cannot do, because it has no idea whether
       anything was handed out after this block. */
    void *last_ptr;
    size_t last_size;
} AxArena;

void ax_arena_init(AxArena *a);
void *ax_arena_alloc(AxArena *a, uint64_t size, uint32_t align);
void ax_arena_release(AxArena *a);

/* ---- exact integer semantics ------------------------------------------- */

/* Truncating division. `INT_MIN / -1` wraps (C says undefined); division by
   zero cannot reach here because lowering emits the check and raises. */
#define AX_DIV_SIGNED(N, T, U)                                                 \
    static inline T ax_div_##N(T a, T b) {                                     \
        if (b == 0) return 0;                                                  \
        if (b == (T)-1) return (T)(-(U)a);                                     \
        return (T)(a / b);                                                     \
    }                                                                          \
    static inline T ax_rem_##N(T a, T b) {                                     \
        if (b == 0) return 0;                                                  \
        if (b == (T)-1) return 0;                                              \
        return (T)(a % b);                                                     \
    }
#define AX_DIV_UNSIGNED(N, T)                                                  \
    static inline T ax_div_##N(T a, T b) { return b ? (T)(a / b) : (T)0; }     \
    static inline T ax_rem_##N(T a, T b) { return b ? (T)(a % b) : (T)0; }

/* Divisor already proven non-zero by the IR's explicit test, so only the
   `INT_MIN / -1` overflow case remains to be defined. Unsigned operands need no
   helper at all — the backend emits a bare `/` or `%`. */
#define AX_DIV_SIGNED_NZ(N, T, U)                                              \
    static inline T ax_div_nz_##N(T a, T b) {                                  \
        if (b == (T)-1) return (T)(-(U)a);                                     \
        return (T)(a / b);                                                     \
    }                                                                          \
    static inline T ax_rem_nz_##N(T a, T b) {                                  \
        if (b == (T)-1) return 0;                                              \
        return (T)(a % b);                                                     \
    }
AX_DIV_SIGNED_NZ(i8, int8_t, uint8_t)
AX_DIV_SIGNED_NZ(i16, int16_t, uint16_t)
AX_DIV_SIGNED_NZ(i32, int32_t, uint32_t)
AX_DIV_SIGNED_NZ(i64, int64_t, uint64_t)

AX_DIV_SIGNED(i8, int8_t, uint8_t)
AX_DIV_SIGNED(i16, int16_t, uint16_t)
AX_DIV_SIGNED(i32, int32_t, uint32_t)
AX_DIV_SIGNED(i64, int64_t, uint64_t)
AX_DIV_UNSIGNED(u8, uint8_t)
AX_DIV_UNSIGNED(u16, uint16_t)
AX_DIV_UNSIGNED(u32, uint32_t)
AX_DIV_UNSIGNED(u64, uint64_t)

/* Shift counts are masked to the operand width (spec/primitives.md), matching
   the hardware and avoiding C's undefined over-shift. */
#define AX_SHIFT(N, T, U, BITS)                                                \
    static inline T ax_shl_##N(T a, T b) {                                     \
        return (T)((U)a << ((U)b & (BITS - 1)));                               \
    }                                                                          \
    static inline T ax_shr_##N(T a, T b) { return (T)(a >> ((U)b & (BITS - 1))); }

AX_SHIFT(i8, int8_t, uint8_t, 8)
AX_SHIFT(i16, int16_t, uint16_t, 16)
AX_SHIFT(i32, int32_t, uint32_t, 32)
AX_SHIFT(i64, int64_t, uint64_t, 64)
AX_SHIFT(u8, uint8_t, uint8_t, 8)
AX_SHIFT(u16, uint16_t, uint16_t, 16)
AX_SHIFT(u32, uint32_t, uint32_t, 32)
AX_SHIFT(u64, uint64_t, uint64_t, 64)

/* ---- exact float semantics --------------------------------------------- */

float ax_canon_f32(float x);
double ax_canon_f64(double x);
float ax_nan_f32(void);
double ax_nan_f64(void);
float ax_inf_f32(void);
double ax_inf_f64(void);
float ax_fmodf(float a, float b);
double ax_fmod(double a, double b);

/* Float -> int saturates at the destination bounds instead of trapping. */
#define AX_F2I_DECL(N, T)                                                      \
    T ax_f2i_##N(double x);                                                    \
    T ax_f2u_##N(double x);
AX_F2I_DECL(i8, int8_t)
AX_F2I_DECL(i16, int16_t)
AX_F2I_DECL(i32, int32_t)
AX_F2I_DECL(i64, int64_t)
AX_F2I_DECL(u8, uint8_t)
AX_F2I_DECL(u16, uint16_t)
AX_F2I_DECL(u32, uint32_t)
AX_F2I_DECL(u64, uint64_t)

double ax_rt_sqrt(double x);
float ax_rt_sqrtf(float x);
double ax_rt_fabs(double x);
float ax_rt_fabsf(float x);
double ax_rt_hypot(double a, double b);
float ax_rt_hypotf(float a, float b);

/* ---- checked arithmetic ------------------------------------------------ */

/* `true` on success with the result in `*out`; `false` on overflow, which the
   caller turns into `None`. */
#define AX_CHECKED_DECL(N, T)                                                  \
    bool ax_rt_checked_add_##N(T a, T b, T *out);                               \
    bool ax_rt_checked_sub_##N(T a, T b, T *out);                               \
    bool ax_rt_checked_mul_##N(T a, T b, T *out);
AX_CHECKED_DECL(i8, int8_t)
AX_CHECKED_DECL(i16, int16_t)
AX_CHECKED_DECL(i32, int32_t)
AX_CHECKED_DECL(i64, int64_t)
AX_CHECKED_DECL(u8, uint8_t)
AX_CHECKED_DECL(u16, uint16_t)
AX_CHECKED_DECL(u32, uint32_t)
AX_CHECKED_DECL(u64, uint64_t)

/* ---- strings ----------------------------------------------------------- */

bool ax_rt_str_eq(const AxStr *a, const AxStr *b);
bool ax_rt_str_eq_raw(const AxStr *a, const char *data, uint64_t len);
bool ax_rt_mem_eq(const void *a, const void *b, uint64_t n);
bool ax_rt_parse_i32(const AxStr *s, int32_t *out);
bool ax_rt_str_starts_with(const AxStr *s, const AxStr *prefix);
bool ax_rt_str_contains(const AxStr *s, const AxStr *needle);
void ax_rt_str_drop(const AxStr *s, uint64_t count, AxStr *out);

/* ---- output ------------------------------------------------------------ */

/* Shortest decimal form that round-trips, matching the oracle's Rust-side
   formatting exactly (`0.1`, `0.30000000000000004`, `NaN`, `inf`, `-inf`).
   Writes into `buf` and returns it. */
const char *ax_fmt_f64(double v, char *buf, size_t n);
const char *ax_fmt_f32(float v, char *buf, size_t n);

void ax_rt_print(const AxStr *s);
void ax_rt_print_i64(int64_t v);
void ax_rt_print_u64(uint64_t v);
void ax_rt_print_f64(double v);
void ax_rt_print_bool(bool v);

/* ---- allocators -------------------------------------------------------- */

/* An allocator handle. `alloc[a]` in a signature names one of these, so every
   allocation in a program is attributable to a handle the caller passed in.
   A region's arena is an allocator, which is what makes `region r { .. }`
   change how code allocates rather than merely annotating it. */
typedef enum { AX_ALLOC_HEAP = 0, AX_ALLOC_ARENA = 1 } AxAllocKind;

typedef struct {
    AxAllocKind kind;
    AxArena *arena; /* NULL for the heap allocator */
} AxAlloc;

void *ax_alloc_raw(const AxAlloc *a, uint64_t size, uint32_t align);
/* Grow a block. Arena allocators cannot free, so growth copies forward; the old
   block is abandoned until the region is released. */
void *ax_alloc_grow(const AxAlloc *a, void *old, uint64_t old_size,
                    uint64_t new_size, uint32_t align);

/* ---- vectors ----------------------------------------------------------- */

/* Layout note: the first two fields match `AxSlice`, so `&Vec[T]` is passed
   where `&[T]` is expected by a prefix cast. Lowering depends on this. */
typedef struct {
    void *data;
    uint64_t len;
    uint64_t cap;
    AxAlloc alloc;
} AxVec;

typedef struct {
    void *data;
    uint64_t len;
} AxSlice;

void ax_rt_vec_new(const AxAlloc *a, uint64_t elem_size, AxVec *out);
/* Opaque hash map (string keys, i64 values) for v0.3 Map[K,V]. */
typedef struct AxMap AxMap;
AxMap *ax_rt_map_new(void);

/* Ownership ladder (spec §5.2). Unique heap has no RC word. Residual RC
   stores a size_t count immediately before the payload pointer. */
void *ax_rt_unique_alloc(uint64_t size, uint32_t align);
void ax_rt_unique_free(void *p);
void *ax_rt_rc_alloc(uint64_t size, uint32_t align, int atomic);
void ax_rt_rc_retain(void *p);
void ax_rt_rc_release(void *p);
void ax_rt_map_insert(AxMap *m, const AxStr *key, int64_t val);
void ax_rt_map_add(AxMap *m, const AxStr *key, int64_t delta);
int ax_rt_map_get(AxMap *m, const AxStr *key, int64_t *out);
uint64_t ax_rt_map_len(AxMap *m);
/* Free entries only; the Map header is UniqueFree'd by lowering at last use. */
void ax_rt_map_free_entries(AxMap *m);
void ax_rt_vec_push(AxVec *v, const void *elem, uint64_t elem_size);
/* Capacity growth only. `push` is lowered inline by the backend — a capacity
   test and a typed store — and calls this only when the buffer is full, so the
   common case is not a function call and not a `memcpy`. */
void ax_rt_vec_grow(AxVec *v, uint64_t elem_size);
/* Grow capacity to at least `n` elements without changing `len`. */
void ax_rt_vec_reserve(AxVec *v, uint64_t elem_size, uint64_t n);
/* Bounds-checked element address. Aborts out of range: `at` is checked always. */
void *ax_rt_vec_at(const AxVec *v, uint64_t i, uint64_t elem_size);
void *ax_rt_slice_at(const AxSlice *s, uint64_t i, uint64_t elem_size);

/* Stable merge sort with a comparison callback returning <0, 0, >0.
   Stable so the resulting permutation is unique, which lets the oracle and the
   native tiers agree element for element. */
void ax_rt_sort(void *data, uint64_t len, uint64_t elem_size,
                int (*cmp)(const void *, const void *));
/* Same stable mergesort, i32 elements, `<=` compare. Used when the
   comparator is `i32.cmp` so the trampoline is not in the way. */
void ax_rt_sort_i32(int32_t *data, uint64_t len);

/* The C callback signature has no room for an environment, so the Ax comparator
   travels through a thread-local slot that the generated trampoline reads.
   Thread-local, so two threads may sort concurrently with different
   comparators. */
void ax_rt_sort_set_cmp(void *fn);
void *ax_rt_sort_get_cmp(void);

/* ---- type descriptors -------------------------------------------------- */

/* A backend-supplied description of a record's layout, used by data-driven
   runtime code (JSON decoding) instead of generating a parser per type.
   Offsets come from the emitting backend, so the runtime never assumes a layout:
   the C backend uses `offsetof`, and any other backend supplies its own. */
typedef enum {
    AX_FLD_I8 = 0,
    AX_FLD_I16,
    AX_FLD_I32,
    AX_FLD_I64,
    AX_FLD_U8,
    AX_FLD_U16,
    AX_FLD_U32,
    AX_FLD_U64,
    AX_FLD_F32,
    AX_FLD_F64,
    AX_FLD_BOOL,
    AX_FLD_STR,
    AX_FLD_OPT_I8,
    AX_FLD_OPT_I16,
    AX_FLD_OPT_I32,
    AX_FLD_OPT_I64,
    AX_FLD_OPT_U8,
    AX_FLD_OPT_U16,
    AX_FLD_OPT_U32,
    AX_FLD_OPT_U64,
    AX_FLD_OPT_F32,
    AX_FLD_OPT_F64,
    AX_FLD_OPT_BOOL,
    AX_FLD_OPT_STR
} AxFieldKind;

typedef struct AxTypeDesc AxTypeDesc;

typedef struct {
    const char *name;
    uint32_t offset;
    AxFieldKind kind;
    const AxTypeDesc *nested;
} AxFieldDesc;

struct AxTypeDesc {
    const char *name;
    uint32_t size;
    uint32_t n_fields;
    const AxFieldDesc *fields;
};

/* Request/response server primitives. The descriptor is emitted by the
   compiler for the typed `http.Request` aggregate, so this ABI remains valid
   if another backend chooses different record padding. */
int ax_http_listen(uint16_t port);
int ax_http_accept(const AxTypeDesc *desc, void *out);
int ax_http_respond(uint16_t status, const void *body, size_t len);
void ax_http_close(void);
int ax_http_serve_handler(uint16_t port, void *handler,
                          const AxTypeDesc *request_desc,
                          const AxTypeDesc *response_desc);
int ax_http_serve_handler_config(uint16_t port, void *handler,
                                 const AxTypeDesc *request_desc,
                                 const AxTypeDesc *response_desc,
                                 uint32_t body_limit, uint32_t timeout_ms,
                                 const AxStr *cors_origin);
bool ax_http_path_match(const AxStr *path, const AxStr *pattern);
void ax_http_path_param(const AxStr *path, const AxStr *pattern,
                        uint16_t index, AxStr *out);
void ax_http_query_param(const AxStr *query, const AxStr *name, AxStr *out);
void ax_http_header(const AxStr *headers, const AxStr *name, AxStr *out);
void ax_http_cookie(const AxStr *headers, const AxStr *name, AxStr *out);

/* ---- json -------------------------------------------------------------- */

/* Decode `[{...}, ...]` into a Vec of records described by `desc`.
   Returns false on malformed input or schema mismatch; the caller raises
   json.Error. Every descriptor field must appear exactly once. */
bool ax_rt_json_decode_recs(const AxAlloc *a, const AxStr *raw,
                            const AxTypeDesc *desc, AxVec *out);
bool ax_rt_json_decode_record(const AxAlloc *a, const AxStr *raw,
                              const AxTypeDesc *desc, void *out);
void ax_rt_json_encode_record(const AxAlloc *a, const AxTypeDesc *desc,
                              const void *record, AxStr *out);
void ax_rt_json_encode_recs(const AxAlloc *a, const AxTypeDesc *desc,
                            const AxVec *records, AxStr *out);

/* ---- filesystem (capability-mediated) ---------------------------------- */

/* Read a file named relative to the capability's directory. Absolute paths and
   any path escaping the root fail closed. Returns false if not found. */
bool ax_rt_fs_read(void *cap, const AxAlloc *a, const AxStr *path, AxStr *out);

/* Build a capability backed by an in-memory file set, for tests. */
void *ax_rt_read_cap_files(const AxStr *names, const AxStr *contents, uint64_t n);

/* ---- strings ----------------------------------------------------------- */

void ax_rt_str_concat(const AxAlloc *a, const AxStr *x, const AxStr *y, AxStr *out);
void ax_rt_str_concat_cached(const AxAlloc *a, const AxStr *x, const AxStr *y,
                             uint64_t *cache_capacity, uint64_t iterations,
                             AxStr *out);
/* Byte at `i`, bounds-checked. */
uint8_t ax_rt_str_byte(const AxStr *s, uint64_t i);
/* One-byte string. Used by the self-hosted tree frontend. */
void ax_rt_str_from_byte(const AxAlloc *a, uint8_t b, AxStr *out);

/* ---- capabilities ------------------------------------------------------ */

/* Handles, not ambient authority: a `ReadCap` names a directory and cannot be
   widened. `..` and absolute paths fail closed. */
void *ax_rt_alloc_default(void);
void *ax_rt_read_cap_cwd(void);

/* ---- capability-typed IO (new ABI) ------------------------------------- */

uint64_t ax_rt_io_bytesum_file(const AxStr *path);
uint64_t ax_rt_io_read_file(const AxStr *path);
uint64_t ax_rt_io_write_file(const AxStr *path, const AxStr *data);
uint64_t ax_rt_http_get_bytesum(const AxStr *url);
uint64_t ax_rt_http_get(const AxStr *url);
void ax_rt_http_serve(uint16_t port, const AxStr *body);
void ax_rt_http_listen(uint16_t port);
void ax_rt_http_accept(const AxTypeDesc *desc, void *out);
void ax_rt_http_respond(uint16_t status, const AxStr *body);
void ax_rt_http_close(void);
void ax_rt_http_serve_handler(uint16_t port, void *handler,
                              const AxTypeDesc *request_desc,
                              const AxTypeDesc *response_desc);
void ax_rt_http_serve_handler_config(uint16_t port, void *handler,
                                     const AxTypeDesc *request_desc,
                                     const AxTypeDesc *response_desc,
                                     uint32_t body_limit, uint32_t timeout_ms,
                                     const AxStr *cors_origin);
void ax_rt_http_serve_handler_state(uint16_t port, void *state, void *handler,
                                    const AxTypeDesc *request_desc,
                                    const AxTypeDesc *response_desc);
void ax_rt_http_serve_handler_state_config(uint16_t port, void *state, void *handler,
                                           const AxTypeDesc *request_desc,
                                           const AxTypeDesc *response_desc,
                                           uint32_t body_limit, uint32_t timeout_ms,
                                           const AxStr *cors_origin);
bool ax_rt_http_path_match(const AxStr *path, const AxStr *pattern);
void ax_rt_http_path_param(const AxStr *path, const AxStr *pattern,
                           uint16_t index, AxStr *out);
void ax_rt_http_query_param(const AxStr *query, const AxStr *name, AxStr *out);
void ax_rt_http_header(const AxStr *headers, const AxStr *name, AxStr *out);
void ax_rt_http_cookie(const AxStr *headers, const AxStr *name, AxStr *out);
void ax_rt_argv(int32_t i, AxStr *out);
void ax_rt_env_get_or(const AxStr *name, const AxStr *fallback, AxStr *out);

/* ---- division by a loop-invariant divisor ------------------------------ */

/* A hardware 64-bit `udiv` costs ~10-20 cycles and does not pipeline. When the
   divisor does not change inside a loop, the quotient can instead be a
   multiply-high and a shift (~5 cycles) against a reciprocal computed once
   outside the loop. Neither rustc nor gc does this hoisting, because neither
   compiler knows the divisor is invariant *and* non-zero at the point it would
   have to commit to the transform; Ax's checker already proves both.
   Granlund-Montgomery, with the same error-correction step libdivide uses.
   `more`'s bit 6 selects the correction path; a zero `m` marks a power of two. */
static inline uint64_t ax_recip_m(uint64_t d) {
    if (d == 0 || (d & (d - 1)) == 0) return 0;
    uint32_t fl = 63u - (uint32_t)__builtin_clzll(d);
    __uint128_t num = ((__uint128_t)1) << (64 + fl);
    uint64_t pm = (uint64_t)(num / d);
    uint64_t rem = (uint64_t)(num % d);
    if (d - rem < ((uint64_t)1 << fl)) return pm + 1;
    pm += pm;
    uint64_t tr = rem + rem;
    if (tr >= d || tr < rem) pm += 1;
    return pm + 1;
}

static inline uint64_t ax_recip_more(uint64_t d) {
    if (d == 0) return 0;
    if ((d & (d - 1)) == 0) return (uint64_t)__builtin_ctzll(d);
    uint32_t fl = 63u - (uint32_t)__builtin_clzll(d);
    __uint128_t num = ((__uint128_t)1) << (64 + fl);
    uint64_t rem = (uint64_t)(num % d);
    if (d - rem < ((uint64_t)1 << fl)) return (uint64_t)fl;
    return (uint64_t)fl | 0x40u;
}

static inline uint64_t ax_div_recip(uint64_t n, uint64_t m, uint64_t more) {
    if (m == 0) return n >> (more & 63u); /* power of two */
    uint64_t q = (uint64_t)(((__uint128_t)m * n) >> 64);
    if (more & 0x40u) {
        uint64_t t = ((n - q) >> 1) + q;
        return t >> (more & 63u);
    }
    return q >> (more & 63u);
}

static inline uint64_t ax_rem_recip(uint64_t n, uint64_t d, uint64_t m,
                                   uint64_t more) {
    return n - ax_div_recip(n, m, more) * d;
}

/* Exported so a backend that cannot inline a header (the Cranelift tier calls
   through dlsym) reaches exactly the same code. */
uint64_t ax_rt_recip_m(uint64_t d);
uint64_t ax_rt_recip_more(uint64_t d);
uint64_t ax_rt_div_recip(uint64_t n, uint64_t m, uint64_t more);
uint64_t ax_rt_rem_recip(uint64_t n, uint64_t d, uint64_t m, uint64_t more);

/* ---- support for a backend that is not C ------------------------------- */

/* The C backend writes `AxArena a;` and lets the C compiler size it. A backend
   emitting machine code directly has to make the frame slot itself, and must
   not hardcode a layout this header owns — so it asks. */
uint64_t ax_arena_slot_size(void);
uint32_t ax_arena_slot_align(void);
uint64_t ax_alloc_slot_size(void);
uint32_t ax_alloc_slot_align(void);
/* Initialise an `AxAlloc` slot as the arena allocator for `arena`. */
void ax_alloc_bind_arena(void *alloc_slot, void *arena);

/* Layout descriptors, built at run time. The C backend emits these as static
   initialisers with `offsetof`; a JIT has no static initialiser stage, so it
   builds them by calling in. Descriptors live for the process. */
void *ax_desc_new(const char *name, uint32_t size, uint32_t n_fields);
void ax_desc_field(void *desc, const char *name, uint32_t offset, int32_t kind);

#ifdef __cplusplus
}
#endif
#endif
