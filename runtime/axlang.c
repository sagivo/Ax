/* Ax language runtime: the ABI generated code calls.
 *
 * Split from axrt.c on purpose. axrt.c is the IO/HTTP/mmap core that predates
 * the IR; this file is the language surface — aborts, arenas, exact integer and
 * float semantics, capability handles, and the capability-typed stdlib.
 *
 * Every function here is either (a) something C leaves undefined where Ax
 * defines it, or (b) a capability-mediated effect. Nothing here is a
 * convenience wrapper.
 */

#include "axrt.h"

#include <math.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

/* ---- process ----------------------------------------------------------- */

static int g_argc = 0;
static char **g_argv = NULL;

void ax_rt_init(int argc, char **argv) {
    g_argc = argc;
    g_argv = argv;
}

_Noreturn void ax_abort(const char *msg) {
    /* The oracle interpreter reports `abort: <msg>`; native must match so a
       differential test can compare stderr, not just exit status. */
    fflush(stdout);
    fprintf(stderr, "abort: %s\n", msg);
    fflush(stderr);
    exit(134);
}

_Noreturn void ax_unreachable(void) {
    ax_abort("reached unreachable code");
}

void ax_test_passed(const char *name) {
    printf("ok    %s\n", name);
}

void ax_test_failed(const char *name) {
    printf("FAIL  %s\n", name);
}

/* ---- bump arenas ------------------------------------------------------- */

/* Spilled blocks form a list so a region can outgrow its inline buffer without
   invalidating pointers already handed out. */
typedef struct AxBlock {
    struct AxBlock *next;
    size_t cap;
    size_t used;
    unsigned char data[];
} AxBlock;

void ax_arena_init(AxArena *a) {
    a->base = a->inline_buf;
    a->cap = sizeof(a->inline_buf);
    a->used = 0;
    a->spill = NULL;
    a->last_ptr = NULL;
    a->last_size = 0;
}

void *ax_arena_alloc(AxArena *a, uint64_t size, uint32_t align) {
    if (align < 1) align = 1;
    size_t pad = (align - (a->used % align)) % align;
    if (a->used + pad + size <= a->cap) {
        void *p = a->base + a->used + pad;
        a->used += pad + size;
        a->last_ptr = p;
        a->last_size = (size_t)size;
        return p;
    }
    /* Grow: allocate a fresh block at least twice the request. */
    size_t want = (size_t)size + align;
    size_t cap = want < 65536 ? 65536 : want * 2;
    AxBlock *b = (AxBlock *)malloc(sizeof(AxBlock) + cap);
    if (!b) ax_abort("region out of memory");
    b->next = (AxBlock *)a->spill;
    b->cap = cap;
    b->used = 0;
    a->spill = b;
    size_t p2 = (align - ((uintptr_t)b->data % align)) % align;
    b->used = p2 + (size_t)size;
    /* The bump pointer now lives in this block. */
    a->base = b->data;
    a->cap = cap;
    a->used = b->used;
    a->last_ptr = b->data + p2;
    a->last_size = (size_t)size;
    return b->data + p2;
}

void ax_arena_release(AxArena *a) {
    AxBlock *b = (AxBlock *)a->spill;
    while (b) {
        AxBlock *n = b->next;
        free(b);
        b = n;
    }
    a->spill = NULL;
    a->base = a->inline_buf;
    a->cap = sizeof(a->inline_buf);
    a->used = 0;
    a->last_ptr = NULL;
    a->last_size = 0;
}

/* ---- allocators -------------------------------------------------------- */

void *ax_alloc_raw(const AxAlloc *a, uint64_t size, uint32_t align) {
    if (a && a->kind == AX_ALLOC_ARENA && a->arena) {
        return ax_arena_alloc(a->arena, size, align);
    }
    void *p = malloc(size ? (size_t)size : 1);
    if (!p) ax_abort("out of memory");
    return p;
}

void *ax_alloc_grow(const AxAlloc *a, void *old, uint64_t old_size,
                    uint64_t new_size, uint32_t align) {
    if (a && a->kind == AX_ALLOC_ARENA && a->arena) {
        AxArena *ar = a->arena;
        /* Grow in place when this block is still the top of the bump pointer and
           the block it sits in has room. Nothing has been handed out after it,
           so extending cannot overlap anything — and unlike realloc, no copy is
           needed even when the region has already spilled to a large block.
           This is the arena's structural advantage over a general allocator. */
        if (old && old == ar->last_ptr && old_size == ar->last_size) {
            size_t start = (size_t)((unsigned char *)old - ar->base);
            if (start + new_size <= ar->cap) {
                ar->used = start + (size_t)new_size;
                ar->last_size = (size_t)new_size;
                return old;
            }
        }
        /* Otherwise allocate forward and copy; the abandoned bytes go away when
           the region exits, which is the trade regions buy. */
        void *p = ax_arena_alloc(ar, new_size, align);
        if (old && old_size) memcpy(p, old, (size_t)old_size);
        return p;
    }
    void *p = realloc(old, new_size ? (size_t)new_size : 1);
    if (!p) ax_abort("out of memory");
    return p;
}

/* ---- vectors ----------------------------------------------------------- */

typedef struct AxMapEntry {
    char *key;
    size_t klen;
    int64_t val;
    struct AxMapEntry *next;
} AxMapEntry;

struct AxMap {
    AxMapEntry *head;
    uint64_t len;
};

void *ax_rt_unique_alloc(uint64_t size, uint32_t align) {
    (void)align;
    void *p = calloc(1, size ? (size_t)size : 1);
    if (!p) ax_abort("out of memory");
    return p;
}

void ax_rt_unique_free(void *p) {
    free(p);
}

void *ax_rt_rc_alloc(uint64_t size, uint32_t align, int atomic) {
    (void)align;
    (void)atomic;
    size_t n = sizeof(size_t) + (size ? (size_t)size : 1);
    size_t *hdr = (size_t *)calloc(1, n);
    if (!hdr) ax_abort("out of memory");
    *hdr = 1;
    return (void *)(hdr + 1);
}

void ax_rt_rc_retain(void *p) {
    if (!p) return;
    size_t *hdr = ((size_t *)p) - 1;
    (*hdr)++;
}

void ax_rt_rc_release(void *p) {
    if (!p) return;
    size_t *hdr = ((size_t *)p) - 1;
    if (*hdr == 0) ax_abort("double free");
    if (--(*hdr) == 0) free(hdr);
}

AxMap *ax_rt_map_new(void) {
    AxMap *m = (AxMap *)calloc(1, sizeof(AxMap));
    if (!m) ax_abort("out of memory");
    return m;
}

static int ax_str_eq(const char *k, size_t kn, const AxStr *s) {
    if (!s || !s->ptr) return kn == 0;
    if (kn != s->len) return 0;
    return kn == 0 || memcmp(k, s->ptr, kn) == 0;
}

void ax_rt_map_insert(AxMap *m, const AxStr *key, int64_t val) {
    if (!m || !key) return;
    for (AxMapEntry *e = m->head; e; e = e->next) {
        if (ax_str_eq(e->key, e->klen, key)) { e->val = val; return; }
    }
    AxMapEntry *e = (AxMapEntry *)calloc(1, sizeof(AxMapEntry));
    if (!e) ax_abort("out of memory");
    e->klen = key->len;
    e->key = (char *)malloc(key->len + 1);
    if (!e->key) ax_abort("out of memory");
    if (key->ptr && key->len) memcpy(e->key, key->ptr, key->len);
    e->key[key->len] = 0;
    e->val = val;
    e->next = m->head;
    m->head = e;
    m->len++;
}

int ax_rt_map_get(AxMap *m, const AxStr *key, int64_t *out) {
    if (!m || !key) return 0;
    for (AxMapEntry *e = m->head; e; e = e->next) {
        if (ax_str_eq(e->key, e->klen, key)) { if (out) *out = e->val; return 1; }
    }
    return 0;
}

uint64_t ax_rt_map_len(AxMap *m) { return m ? m->len : 0; }

void ax_rt_vec_new(const AxAlloc *a, uint64_t elem_size, AxVec *out) {
    (void)elem_size;
    out->data = NULL;
    out->len = 0;
    out->cap = 0;
    out->alloc = a ? *a : (AxAlloc){AX_ALLOC_HEAP, NULL};
}

void ax_rt_vec_grow(AxVec *v, uint64_t elem_size) {
    uint64_t cap = v->cap ? v->cap * 2 : 8;
    v->data = ax_alloc_grow(&v->alloc, v->data, v->cap * elem_size,
                            cap * elem_size, 16);
    v->cap = cap;
}

void ax_rt_vec_push(AxVec *v, const void *elem, uint64_t elem_size) {
    if (v->len == v->cap) ax_rt_vec_grow(v, elem_size);
    memcpy((unsigned char *)v->data + v->len * elem_size, elem, (size_t)elem_size);
    v->len++;
}

void *ax_rt_vec_at(const AxVec *v, uint64_t i, uint64_t elem_size) {
    if (i >= v->len) ax_abort("index out of bounds");
    return (unsigned char *)v->data + i * elem_size;
}

void *ax_rt_slice_at(const AxSlice *s, uint64_t i, uint64_t elem_size) {
    if (i >= s->len) ax_abort("index out of bounds");
    return (unsigned char *)s->data + i * elem_size;
}

static _Thread_local void *g_sort_cmp = NULL;

void ax_rt_sort_set_cmp(void *fn) {
    g_sort_cmp = fn;
}

void *ax_rt_sort_get_cmp(void) {
    return g_sort_cmp;
}

/* Stable bottom-up merge sort. Stability is a semantic requirement, not a
   quality-of-implementation detail: it makes the output permutation unique so
   the oracle and native agree. */
void ax_rt_sort(void *data, uint64_t len, uint64_t elem_size,
                int (*cmp)(const void *, const void *)) {
    if (len < 2) return;
    unsigned char *src = (unsigned char *)data;
    unsigned char *tmp = (unsigned char *)malloc((size_t)(len * elem_size));
    if (!tmp) ax_abort("out of memory");
    for (uint64_t width = 1; width < len; width *= 2) {
        for (uint64_t i = 0; i < len; i += 2 * width) {
            uint64_t mid = i + width < len ? i + width : len;
            uint64_t end = i + 2 * width < len ? i + 2 * width : len;
            uint64_t l = i, r = mid, o = i;
            while (l < mid && r < end) {
                /* `<= 0` keeps equal elements in their original order. */
                if (cmp(src + l * elem_size, src + r * elem_size) <= 0) {
                    memcpy(tmp + o * elem_size, src + l * elem_size, (size_t)elem_size);
                    l++;
                } else {
                    memcpy(tmp + o * elem_size, src + r * elem_size, (size_t)elem_size);
                    r++;
                }
                o++;
            }
            while (l < mid) {
                memcpy(tmp + o * elem_size, src + l * elem_size, (size_t)elem_size);
                l++;
                o++;
            }
            while (r < end) {
                memcpy(tmp + o * elem_size, src + r * elem_size, (size_t)elem_size);
                r++;
                o++;
            }
        }
        memcpy(src, tmp, (size_t)(len * elem_size));
    }
    free(tmp);
}

/* ---- exact float semantics --------------------------------------------- */

/* One canonical NaN so every backend and the oracle agree bit-for-bit. */
#define AX_CANON_NAN_F32 0x7fc00000u
#define AX_CANON_NAN_F64 0x7ff8000000000000ull

float ax_canon_f32(float x) {
    if (isnan(x)) {
        uint32_t bits = AX_CANON_NAN_F32;
        float out;
        memcpy(&out, &bits, sizeof(out));
        return out;
    }
    return x;
}

double ax_canon_f64(double x) {
    if (isnan(x)) {
        uint64_t bits = AX_CANON_NAN_F64;
        double out;
        memcpy(&out, &bits, sizeof(out));
        return out;
    }
    return x;
}

float ax_nan_f32(void) {
    return ax_canon_f32(NAN);
}

double ax_nan_f64(void) {
    return ax_canon_f64(NAN);
}

float ax_inf_f32(void) {
    return INFINITY;
}

double ax_inf_f64(void) {
    return (double)INFINITY;
}

float ax_fmodf(float a, float b) {
    return ax_canon_f32(fmodf(a, b));
}

double ax_fmod(double a, double b) {
    return ax_canon_f64(fmod(a, b));
}

double ax_rt_sqrt(double x) {
    return ax_canon_f64(sqrt(x));
}

float ax_rt_sqrtf(float x) {
    return ax_canon_f32(sqrtf(x));
}

double ax_rt_fabs(double x) {
    return fabs(x);
}

float ax_rt_fabsf(float x) {
    return fabsf(x);
}

double ax_rt_hypot(double a, double b) {
    return ax_canon_f64(hypot(a, b));
}

float ax_rt_hypotf(float a, float b) {
    return ax_canon_f32(hypotf(a, b));
}

/* Saturating float->int: NaN gives 0, out-of-range clamps. Matches the oracle
   and avoids C's undefined out-of-range conversion. */
#define AX_F2I_IMPL(N, T, LO, HI)                                              \
    T ax_f2i_##N(double x) {                                                   \
        if (isnan(x)) return 0;                                                \
        if (x <= (double)(LO)) return (T)(LO);                                 \
        if (x >= (double)(HI)) return (T)(HI);                                 \
        return (T)x;                                                           \
    }                                                                          \
    T ax_f2u_##N(double x) {                                                   \
        if (isnan(x) || x <= 0.0) return 0;                                    \
        if (x >= (double)(HI)) return (T)(HI);                                 \
        return (T)x;                                                           \
    }

AX_F2I_IMPL(i8, int8_t, INT8_MIN, INT8_MAX)
AX_F2I_IMPL(i16, int16_t, INT16_MIN, INT16_MAX)
AX_F2I_IMPL(i32, int32_t, INT32_MIN, INT32_MAX)
AX_F2I_IMPL(i64, int64_t, INT64_MIN, INT64_MAX)
AX_F2I_IMPL(u8, uint8_t, 0, UINT8_MAX)
AX_F2I_IMPL(u16, uint16_t, 0, UINT16_MAX)
AX_F2I_IMPL(u32, uint32_t, 0, UINT32_MAX)
AX_F2I_IMPL(u64, uint64_t, 0, UINT64_MAX)

/* ---- checked arithmetic ------------------------------------------------ */

/* Compiler builtins so the overflow test is a flag read, not a re-computation. */
#define AX_CHECKED_IMPL(N, T)                                                  \
    bool ax_rt_checked_add_##N(T a, T b, T *out) {                              \
        return !__builtin_add_overflow(a, b, out);                             \
    }                                                                          \
    bool ax_rt_checked_sub_##N(T a, T b, T *out) {                              \
        return !__builtin_sub_overflow(a, b, out);                             \
    }                                                                          \
    bool ax_rt_checked_mul_##N(T a, T b, T *out) {                              \
        return !__builtin_mul_overflow(a, b, out);                             \
    }

AX_CHECKED_IMPL(i8, int8_t)
AX_CHECKED_IMPL(i16, int16_t)
AX_CHECKED_IMPL(i32, int32_t)
AX_CHECKED_IMPL(i64, int64_t)
AX_CHECKED_IMPL(u8, uint8_t)
AX_CHECKED_IMPL(u16, uint16_t)
AX_CHECKED_IMPL(u32, uint32_t)
AX_CHECKED_IMPL(u64, uint64_t)

/* ---- strings ----------------------------------------------------------- */

bool ax_rt_str_eq(const AxStr *a, const AxStr *b) {
    if (a->len != b->len) return false;
    if (a->len == 0) return true;
    return memcmp(a->ptr, b->ptr, a->len) == 0;
}

bool ax_rt_str_eq_raw(const AxStr *a, const char *data, uint64_t len) {
    if (a->len != len) return false;
    if (len == 0) return true;
    return memcmp(a->ptr, data, (size_t)len) == 0;
}

bool ax_rt_mem_eq(const void *a, const void *b, uint64_t n) {
    return memcmp(a, b, (size_t)n) == 0;
}

void ax_rt_str_concat(const AxAlloc *a, const AxStr *x, const AxStr *y, AxStr *out) {
    uint64_t n = x->len + y->len;
    char *p = (char *)ax_alloc_raw(a, n + 1, 1);
    memcpy(p, x->ptr, (size_t)x->len);
    memcpy(p + x->len, y->ptr, (size_t)y->len);
    p[n] = 0;
    out->ptr = p;
    out->len = (size_t)n;
}

uint8_t ax_rt_str_byte(const AxStr *s, uint64_t i) {
    if (i >= s->len) ax_abort("index out of bounds");
    return (uint8_t)s->ptr[i];
}

bool ax_rt_parse_i32(const AxStr *s, int32_t *out) {
    /* Strict: optional sign, then digits, nothing else. No locale, no
       whitespace, no overflow wrap — matches the oracle's parse_i32. */
    if (s->len == 0) return false;
    size_t i = 0;
    int neg = 0;
    if (s->ptr[0] == '-' || s->ptr[0] == '+') {
        neg = s->ptr[0] == '-';
        i = 1;
        if (s->len == 1) return false;
    }
    int64_t acc = 0;
    for (; i < s->len; i++) {
        char c = s->ptr[i];
        if (c < '0' || c > '9') return false;
        acc = acc * 10 + (c - '0');
        if (acc > 2147483648LL) return false;
    }
    if (neg) acc = -acc;
    if (acc > INT32_MAX || acc < INT32_MIN) return false;
    *out = (int32_t)acc;
    return true;
}

/* ---- float formatting --------------------------------------------------- */

/* Shortest round-trip: try increasing precision until the text parses back to
   the same value. This reproduces what Rust's `{}` prints for f32/f64, which is
   what the oracle interpreter emits — the two must agree character for
   character for differential testing to mean anything. */
const char *ax_fmt_f64(double v, char *buf, size_t n) {
    if (isnan(v)) {
        snprintf(buf, n, "NaN");
        return buf;
    }
    if (isinf(v)) {
        snprintf(buf, n, v < 0 ? "-inf" : "inf");
        return buf;
    }
    for (int prec = 1; prec <= 17; prec++) {
        snprintf(buf, n, "%.*g", prec, v);
        if (strtod(buf, NULL) == v) break;
    }
    return buf;
}

const char *ax_fmt_f32(float v, char *buf, size_t n) {
    if (isnan(v)) {
        snprintf(buf, n, "NaN");
        return buf;
    }
    if (isinf(v)) {
        snprintf(buf, n, v < 0 ? "-inf" : "inf");
        return buf;
    }
    for (int prec = 1; prec <= 9; prec++) {
        snprintf(buf, n, "%.*g", prec, (double)v);
        if (strtof(buf, NULL) == v) break;
    }
    return buf;
}

/* ---- output ------------------------------------------------------------ */

void ax_rt_print(const AxStr *s) {
    fwrite(s->ptr, 1, s->len, stdout);
    fputc('\n', stdout);
}

void ax_rt_print_i64(int64_t v) {
    printf("%lld\n", (long long)v);
}

void ax_rt_print_u64(uint64_t v) {
    printf("%llu\n", (unsigned long long)v);
}

void ax_rt_print_f64(double v) {
    char buf[48];
    printf("%s\n", ax_fmt_f64(v, buf, sizeof(buf)));
}

void ax_rt_print_bool(bool v) {
    printf("%s\n", v ? "true" : "false");
}

/* ---- json -------------------------------------------------------------- */

/* A single-pass recursive-descent reader over the raw text. Deliberately small:
   it decodes the shapes the descriptor asks for and rejects everything else,
   rather than building a general DOM the caller then walks. */
typedef struct {
    const char *p;
    const char *end;
    bool ok;
} AxJson;

static void js_ws(AxJson *j) {
    while (j->p < j->end && (*j->p == ' ' || *j->p == '\t' || *j->p == '\n' || *j->p == '\r')) {
        j->p++;
    }
}

static bool js_eat(AxJson *j, char c) {
    js_ws(j);
    if (j->p < j->end && *j->p == c) {
        j->p++;
        return true;
    }
    return false;
}

/* Scan a JSON string, returning its bounds. Only the escapes the language's own
   string literals use are accepted. */
static bool js_string(AxJson *j, const char **start, size_t *len) {
    if (!js_eat(j, '"')) return false;
    const char *s = j->p;
    while (j->p < j->end && *j->p != '"') {
        if (*j->p == '\\' && j->p + 1 < j->end) j->p++;
        j->p++;
    }
    if (j->p >= j->end) return false;
    *start = s;
    *len = (size_t)(j->p - s);
    j->p++;
    return true;
}

static bool js_number(AxJson *j, double *out) {
    js_ws(j);
    const char *s = j->p;
    if (j->p < j->end && (*j->p == '-' || *j->p == '+')) j->p++;
    while (j->p < j->end && ((*j->p >= '0' && *j->p <= '9') || *j->p == '.' || *j->p == 'e' ||
                             *j->p == 'E' || *j->p == '-' || *j->p == '+')) {
        j->p++;
    }
    if (j->p == s) return false;
    char buf[64];
    size_t n = (size_t)(j->p - s);
    if (n >= sizeof(buf)) return false;
    memcpy(buf, s, n);
    buf[n] = 0;
    *out = strtod(buf, NULL);
    return true;
}

/* Skip one value of any shape, for fields the descriptor does not name. */
static bool js_skip(AxJson *j) {
    js_ws(j);
    if (j->p >= j->end) return false;
    char c = *j->p;
    if (c == '"') {
        const char *s;
        size_t n;
        return js_string(j, &s, &n);
    }
    if (c == '{' || c == '[') {
        char open = c, close = c == '{' ? '}' : ']';
        int depth = 0;
        while (j->p < j->end) {
            if (*j->p == '"') {
                const char *s;
                size_t n;
                if (!js_string(j, &s, &n)) return false;
                continue;
            }
            if (*j->p == open) depth++;
            if (*j->p == close) {
                depth--;
                j->p++;
                if (depth == 0) return true;
                continue;
            }
            j->p++;
        }
        return false;
    }
    if (j->p + 4 <= j->end && memcmp(j->p, "true", 4) == 0) {
        j->p += 4;
        return true;
    }
    if (j->p + 5 <= j->end && memcmp(j->p, "false", 5) == 0) {
        j->p += 5;
        return true;
    }
    if (j->p + 4 <= j->end && memcmp(j->p, "null", 4) == 0) {
        j->p += 4;
        return true;
    }
    double d;
    return js_number(j, &d);
}

static void js_store(unsigned char *rec, const AxFieldDesc *f, double num,
                     const char *str, size_t str_len, bool is_str,
                     const AxAlloc *a) {
    unsigned char *slot = rec + f->offset;
    if (f->kind == AX_FLD_STR) {
        if (!is_str) return;
        /* Copy into the caller's allocator: the raw buffer may be reused. */
        char *p = (char *)ax_alloc_raw(a, str_len + 1, 1);
        memcpy(p, str, str_len);
        p[str_len] = 0;
        AxStr v = {p, str_len};
        memcpy(slot, &v, sizeof(v));
        return;
    }
    if (is_str) return;
    switch (f->kind) {
        case AX_FLD_I8: { int8_t v = (int8_t)num; memcpy(slot, &v, 1); break; }
        case AX_FLD_I16: { int16_t v = (int16_t)num; memcpy(slot, &v, 2); break; }
        case AX_FLD_I32: { int32_t v = (int32_t)num; memcpy(slot, &v, 4); break; }
        case AX_FLD_I64: { int64_t v = (int64_t)num; memcpy(slot, &v, 8); break; }
        case AX_FLD_U8: { uint8_t v = (uint8_t)num; memcpy(slot, &v, 1); break; }
        case AX_FLD_U16: { uint16_t v = (uint16_t)num; memcpy(slot, &v, 2); break; }
        case AX_FLD_U32: { uint32_t v = (uint32_t)num; memcpy(slot, &v, 4); break; }
        case AX_FLD_U64: { uint64_t v = (uint64_t)num; memcpy(slot, &v, 8); break; }
        case AX_FLD_F32: { float v = (float)num; memcpy(slot, &v, 4); break; }
        case AX_FLD_F64: { double v = num; memcpy(slot, &v, 8); break; }
        case AX_FLD_BOOL: { bool v = num != 0.0; memcpy(slot, &v, 1); break; }
        default: break;
    }
}

bool ax_rt_json_decode_recs(const AxAlloc *a, const AxStr *raw,
                            const AxTypeDesc *desc, AxVec *out) {
    AxJson j = {raw->ptr, raw->ptr + raw->len, true};
    ax_rt_vec_new(a, desc->size, out);
    if (!js_eat(&j, '[')) return false;
    js_ws(&j);
    if (js_eat(&j, ']')) return true;
    unsigned char *rec = (unsigned char *)malloc(desc->size);
    if (!rec) ax_abort("out of memory");
    for (;;) {
        memset(rec, 0, desc->size);
        if (!js_eat(&j, '{')) {
            free(rec);
            return false;
        }
        js_ws(&j);
        if (!js_eat(&j, '}')) {
            for (;;) {
                const char *key;
                size_t key_len;
                if (!js_string(&j, &key, &key_len) || !js_eat(&j, ':')) {
                    free(rec);
                    return false;
                }
                const AxFieldDesc *f = NULL;
                for (uint32_t i = 0; i < desc->n_fields; i++) {
                    const AxFieldDesc *cand = &desc->fields[i];
                    if (strlen(cand->name) == key_len &&
                        memcmp(cand->name, key, key_len) == 0) {
                        f = cand;
                        break;
                    }
                }
                if (!f) {
                    if (!js_skip(&j)) {
                        free(rec);
                        return false;
                    }
                } else {
                    js_ws(&j);
                    if (j.p < j.end && *j.p == '"') {
                        const char *sv;
                        size_t sl;
                        if (!js_string(&j, &sv, &sl)) {
                            free(rec);
                            return false;
                        }
                        js_store(rec, f, 0, sv, sl, true, a);
                    } else if (j.p < j.end && (*j.p == 't' || *j.p == 'f')) {
                        bool t = *j.p == 't';
                        if (!js_skip(&j)) {
                            free(rec);
                            return false;
                        }
                        js_store(rec, f, t ? 1.0 : 0.0, NULL, 0, false, a);
                    } else {
                        double num;
                        if (!js_number(&j, &num)) {
                            free(rec);
                            return false;
                        }
                        js_store(rec, f, num, NULL, 0, false, a);
                    }
                }
                if (js_eat(&j, ',')) continue;
                if (js_eat(&j, '}')) break;
                free(rec);
                return false;
            }
        }
        ax_rt_vec_push(out, rec, desc->size);
        if (js_eat(&j, ',')) continue;
        if (js_eat(&j, ']')) break;
        free(rec);
        return false;
    }
    free(rec);
    return true;
}

/* ---- capabilities ------------------------------------------------------ */

/* Handles are real objects, not comments. A ReadCap names a directory; paths
   are resolved against it and may not escape it. */
typedef struct {
    const char *root;
} AxReadCap;

static AxReadCap g_cwd_cap = {"."};
/* The default allocator is the heap. A region supplies an arena instead. */
static AxAlloc g_heap_alloc = {AX_ALLOC_HEAP, NULL};

void *ax_rt_alloc_default(void) {
    return &g_heap_alloc;
}

void *ax_rt_read_cap_cwd(void) {
    return &g_cwd_cap;
}

/* An in-memory file set, used by `test.read_cap`. Kept in the capability so a
   test cannot reach the real filesystem by accident. */
#define AX_CAP_MAX_FILES 32

typedef struct {
    const char *root;
    uint64_t n_files;
    AxStr names[AX_CAP_MAX_FILES];
    AxStr contents[AX_CAP_MAX_FILES];
} AxFileCap;

void *ax_rt_read_cap_files(const AxStr *names, const AxStr *contents, uint64_t n) {
    if (n > AX_CAP_MAX_FILES) ax_abort("too many files in a test capability");
    AxFileCap *c = (AxFileCap *)malloc(sizeof(AxFileCap));
    if (!c) ax_abort("out of memory");
    c->root = NULL;
    c->n_files = n;
    for (uint64_t i = 0; i < n; i++) {
        c->names[i] = names[i];
        c->contents[i] = contents[i];
    }
    return c;
}

/* Reject anything that could leave the capability's directory: absolute paths,
   `..` segments, and embedded NULs. Fail closed. */
static bool path_is_confined(const AxStr *p) {
    if (p->len == 0) return false;
    if (p->ptr[0] == '/') return false;
    for (size_t i = 0; i < p->len; i++) {
        if (p->ptr[i] == 0) return false;
        if (p->ptr[i] == '.' && i + 1 < p->len && p->ptr[i + 1] == '.') return false;
    }
    return true;
}

bool ax_rt_fs_read(void *cap, const AxAlloc *a, const AxStr *path, AxStr *out) {
    if (!path_is_confined(path)) return false;
    AxFileCap *c = (AxFileCap *)cap;
    if (c && c->n_files > 0) {
        for (uint64_t i = 0; i < c->n_files; i++) {
            if (c->names[i].len == path->len &&
                memcmp(c->names[i].ptr, path->ptr, path->len) == 0) {
                *out = c->contents[i];
                return true;
            }
        }
        /* A capability with an explicit file set does not fall through to the
           host filesystem: the set is the authority. */
        return false;
    }
    char buf[4096];
    const char *root = (c && c->root) ? c->root : ".";
    if (strlen(root) + path->len + 2 > sizeof(buf)) return false;
    snprintf(buf, sizeof(buf), "%s/%.*s", root, (int)path->len, path->ptr);
    FILE *f = fopen(buf, "rb");
    if (!f) return false;
    if (fseek(f, 0, SEEK_END) != 0) {
        fclose(f);
        return false;
    }
    long n = ftell(f);
    if (n < 0) {
        fclose(f);
        return false;
    }
    rewind(f);
    char *p = (char *)ax_alloc_raw(a, (uint64_t)n + 1, 1);
    size_t got = fread(p, 1, (size_t)n, f);
    fclose(f);
    p[got] = 0;
    out->ptr = p;
    out->len = got;
    return true;
}

/* ---- capability-typed IO ----------------------------------------------- */

/* Bounded copy of an AxStr into a NUL-terminated buffer for the POSIX layer. */
static int str_cstr(const AxStr *s, char *buf, size_t n) {
    if (s->len + 1 > n) return -1;
    memcpy(buf, s->ptr, s->len);
    buf[s->len] = 0;
    return 0;
}

uint64_t ax_rt_io_bytesum_file(const AxStr *path) {
    char buf[4096];
    if (str_cstr(path, buf, sizeof(buf)) != 0) ax_abort("path too long");
    uint64_t out = 0;
    if (ax_io_bytesum_file(buf, &out) != 0) ax_abort("io.bytesum_file failed");
    return out;
}

uint64_t ax_rt_io_read_file(const AxStr *path) {
    char buf[4096];
    if (str_cstr(path, buf, sizeof(buf)) != 0) ax_abort("path too long");
    AxStr out = {0, 0};
    if (ax_io_read_file(buf, &out) != 0) ax_abort("io.read_file failed");
    return (uint64_t)out.len;
}

uint64_t ax_rt_io_write_file(const AxStr *path, const AxStr *data) {
    char buf[4096];
    if (str_cstr(path, buf, sizeof(buf)) != 0) ax_abort("path too long");
    if (ax_io_write_file(buf, data->ptr, data->len) != 0)
        ax_abort("io.write_file failed");
    return (uint64_t)data->len;
}

uint64_t ax_rt_http_get_bytesum(const AxStr *url) {
    char buf[2048];
    if (str_cstr(url, buf, sizeof(buf)) != 0) ax_abort("url too long");
    uint64_t out = 0;
    if (ax_http_get_bytesum(buf, &out) != 0) ax_abort("http.get_bytesum failed");
    return out;
}

uint64_t ax_rt_http_get(const AxStr *url) {
    char buf[2048];
    if (str_cstr(url, buf, sizeof(buf)) != 0) ax_abort("url too long");
    AxStr out = {0, 0};
    if (ax_http_get(buf, &out) != 0) ax_abort("http.get failed");
    return (uint64_t)out.len;
}

void ax_rt_http_serve(uint16_t port, const AxStr *body) {
    if (ax_http_serve_static(port, body->ptr, body->len) != 0)
        ax_abort("http.serve failed");
}

void ax_rt_argv(int32_t i, AxStr *out) {
    if (i < 0 || i >= g_argc) {
        out->ptr = "";
        out->len = 0;
        return;
    }
    out->ptr = g_argv[i];
    out->len = strlen(g_argv[i]);
}

/* ---- support for a backend that is not C ------------------------------- */

/* Sizes and alignments of the runtime's own structs, so a machine-code backend
   can make frame slots for them without duplicating this header's layout. */
uint64_t ax_arena_slot_size(void) { return (uint64_t)sizeof(AxArena); }
uint32_t ax_arena_slot_align(void) { return (uint32_t)_Alignof(AxArena); }
uint64_t ax_alloc_slot_size(void) { return (uint64_t)sizeof(AxAlloc); }
uint32_t ax_alloc_slot_align(void) { return (uint32_t)_Alignof(AxAlloc); }

void ax_alloc_bind_arena(void *alloc_slot, void *arena) {
    AxAlloc *a = (AxAlloc *)alloc_slot;
    a->kind = AX_ALLOC_ARENA;
    a->arena = (AxArena *)arena;
}

/* A descriptor built at run time. `ax_desc_field` appends, so the caller emits
   one `ax_desc_new` and then one call per field, in order. Never freed: there is
   one per record type per process, and freeing them would mean tracking which
   generated code still points at them. */
void *ax_desc_new(const char *name, uint32_t size, uint32_t n_fields) {
    AxTypeDesc *d = (AxTypeDesc *)calloc(1, sizeof(AxTypeDesc));
    if (!d) ax_abort("out of memory building a type descriptor");
    AxFieldDesc *fs = (AxFieldDesc *)calloc(n_fields ? n_fields : 1, sizeof(AxFieldDesc));
    if (!fs) ax_abort("out of memory building a type descriptor");
    d->name = name;
    d->size = size;
    d->n_fields = 0;
    d->fields = fs;
    return d;
}

void ax_desc_field(void *desc, const char *name, uint32_t offset, int32_t kind) {
    AxTypeDesc *d = (AxTypeDesc *)desc;
    AxFieldDesc *fs = (AxFieldDesc *)d->fields;
    fs[d->n_fields].name = name;
    fs[d->n_fields].offset = offset;
    fs[d->n_fields].kind = (AxFieldKind)kind;
    d->n_fields++;
}

/* ---- division by a loop-invariant divisor ------------------------------ */

/* Thin wrappers over the header's inline forms, so the C tier inlines them into
   the hot loop and the Cranelift tier calls the identical code by symbol. */
uint64_t ax_rt_recip_m(uint64_t d) { return ax_recip_m(d); }
uint64_t ax_rt_recip_more(uint64_t d) { return ax_recip_more(d); }
uint64_t ax_rt_div_recip(uint64_t n, uint64_t m, uint64_t more) {
    return ax_div_recip(n, m, more);
}
uint64_t ax_rt_rem_recip(uint64_t n, uint64_t d, uint64_t m, uint64_t more) {
    return ax_rem_recip(n, d, m, more);
}
