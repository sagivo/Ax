# Syntax & token cost — basic use cases

Same task in Rust, C, Go, and Ax. Token counts use the **in-repo proxy** (`ax::tokens`): letters split at `_` / case / digit boundaries; each punctuation mark is one token except two-char operators (`->`, `:=`, `==` …); a newline is one token; spaces are free. Absolute numbers are not any particular model’s vocabulary; the comparison is identical across languages.

**Ax** is the language (short syntax). Conventional is the corpus dialect, shown only so the same idea can be compared with Rust / C / Go. `ax fmt` prints Ax.

## Summary

| case | rust | c | go | ax-conv | ax | ax/rust |
|---|---:|---:|---:|---:|---:|---:|
| Add two integers | 24 | 19 | 22 | 22 | 15 | 0.62× |
| If / else | 24 | 22 | 24 | 22 | 17 | 0.71× |
| Sum a range | 59 | 53 | 43 | 45 | 28 | 0.47× |
| Recursion | 34 | 31 | 34 | 32 | 26 | 0.76× |
| Option unwrap-or | 52 | 57 | 41 | 44 | 25 | 0.48× |
| Map insert + get | 112 | 156 | 56 | 126 | 75 | 0.67× |
| Fallible parse | 35 | 52 | 42 | 48 | 26 | 0.74× |
| String interpolation | 25 | 37 | 25 | 19 | 16 | 0.64× |
| **total** | 365 | 427 | 287 | 358 | 228 | 0.62× |

Read the totals as *how much text an agent pays to write the same idea*. **Ax** is the short syntax (the language). Conventional is the corpus dialect, kept so the same idea can be compared with Rust / C / Go.

## Add two integers

A named function that returns `a + b`.

| rust | c | go | ax-conv | ax |
|---:|---:|---:|---:|---:|
| 24 | 19 | 22 | 22 | 15 |

**Rust**

```rust
fn add(a: i32, b: i32) -> i32 {
    a + b
}
```

**C**

```c
int add(int a, int b) {
    return a + b;
}
```

**Go**

```go
func add(a int32, b int32) int32 {
    return a + b
}
```

**Ax (corpus / conventional)**

```ax
fn add(a: i32, b: i32) -> i32 = a + b;
```

**Ax**

```ax
#add(a I, b I) I = a + b;
```
## If / else

Pick one of two integers from a boolean.

| rust | c | go | ax-conv | ax |
|---:|---:|---:|---:|---:|
| 24 | 22 | 24 | 22 | 17 |

**Rust**

```rust
fn pick(b: bool) -> i32 {
    if b { 1 } else { 0 }
}
```

**C**

```c
int pick(int b) {
    if (b) return 1;
    return 0;
}
```

**Go**

```go
func pick(b bool) int32 {
    if b {
        return 1
    }
    return 0
}
```

**Ax (corpus / conventional)**

```ax
fn pick(b: bool) -> i32 = if b { 1 } else { 0 };
```

**Ax**

```ax
#pick(b B) I = $b{1}{0};
```
## Sum a range

Accumulate `0 + 1 + … + (n-1)`.

| rust | c | go | ax-conv | ax |
|---:|---:|---:|---:|---:|
| 59 | 53 | 43 | 45 | 28 |

**Rust**

```rust
fn sum(n: u64) -> u64 {
    let mut s = 0u64;
    let mut i = 0u64;
    while i < n {
        s = s.wrapping_add(i);
        i += 1;
    }
    s
}
```

**C**

```c
uint64_t sum(uint64_t n) {
    uint64_t s = 0;
    for (uint64_t i = 0; i < n; i++) s += i;
    return s;
}
```

**Go**

```go
func sum(n uint64) uint64 {
    var s uint64
    var i uint64
    for i < n {
        s += i
        i++
    }
    return s
}
```

**Ax (corpus / conventional)**

```ax
fn sum(n: usz) -> usz = {
    let mut s: usz = 0;
    for i in range(0, n) { s = s + i; };
    s
};
```

**Ax**

```ax
#sum(n Z) Z = { s Z:= 0; i~n { s = s + i }; s };
```
## Recursion

Factorial by a single recursive call.

| rust | c | go | ax-conv | ax |
|---:|---:|---:|---:|---:|
| 34 | 31 | 34 | 32 | 26 |

**Rust**

```rust
fn fac(n: i32) -> i32 {
    if n < 2 { 1 } else { n * fac(n - 1) }
}
```

**C**

```c
int fac(int n) {
    if (n < 2) return 1;
    return n * fac(n - 1);
}
```

**Go**

```go
func fac(n int32) int32 {
    if n < 2 {
        return 1
    }
    return n * fac(n-1)
}
```

**Ax (corpus / conventional)**

```ax
fn fac(n: i32) -> i32 = if n < 2 { 1 } else { n * fac(n - 1) };
```

**Ax**

```ax
#fac(n I) I = $n < 2{1}{n * fac(n - 1)};
```
## Option unwrap-or

Read a map entry, default to `0` when missing.

| rust | c | go | ax-conv | ax |
|---:|---:|---:|---:|---:|
| 52 | 57 | 41 | 44 | 25 |

**Rust**

```rust
fn get(m: &HashMap<String, i64>, k: &str) -> i64 {
    match m.get(k) {
        Some(v) => *v,
        None => 0,
    }
}
```

**C**

```c
int64_t get(AxMap *m, const AxStr *k) {
    int64_t v = 0;
    if (!ax_rt_map_get(m, k, &v)) return 0;
    return v;
}
```

**Go**

```go
func get(m map[string]int64, k string) int64 {
    if v, ok := m[k]; ok {
        return v
    }
    return 0
}
```

**Ax (corpus / conventional)**

```ax
fn get(m: Map[String, i64], k: String) -> i64 =
    match m.get(k) { Some(v) => v; None => 0; };
```

**Ax**

```ax
#get(m M[S, L], k S) L = m.get(k)?0;
```
## Map insert + get

Allocate a map, insert two keys, return their sum.

| rust | c | go | ax-conv | ax |
|---:|---:|---:|---:|---:|
| 112 | 156 | 56 | 126 | 75 |

**Rust**

```rust
fn main() {
    let mut m = HashMap::new();
    m.insert("e".into(), 2i64);
    m.insert("o".into(), 3i64);
    let e = *m.get("e").unwrap_or(&0);
    let o = *m.get("o").unwrap_or(&0);
    println!("{}", e + o);
}
```

**C**

```c
int main(void) {
    AxMap *m = ax_rt_map_new();
    AxStr e = {"e", 1}, o = {"o", 1};
    ax_rt_map_insert(m, &e, 2);
    ax_rt_map_insert(m, &o, 3);
    int64_t ev = 0, ov = 0;
    ax_rt_map_get(m, &e, &ev);
    ax_rt_map_get(m, &o, &ov);
    printf("%lld\n", (long long)(ev + ov));
    return 0;
}
```

**Go**

```go
func main() {
    m := map[string]int64{}
    m["e"] = 2
    m["o"] = 3
    fmt.Println(m["e"] + m["o"])
}
```

**Ax (corpus / conventional)**

```ax
fn main() -> i64 !{alloc[a]} = {
    let mut m: Map[String, i64] = map.new(test.alloc);
    m.insert("e", 2i64);
    m.insert("o", 3i64);
    let e = match m.get("e") { Some(v) => v; None => 0; };
    let o = match m.get("o") { Some(v) => v; None => 0; };
    e + o
};
```

**Ax**

```ax
#main() L !alloc[a] = { m M[S, L]:= %; m.insert("e", 2L); m.insert("o", 3L); e:= m.get("e")?0; o:= m.get("o")?0; e + o };
```
## Fallible parse

Parse an integer; on failure return a default.

| rust | c | go | ax-conv | ax |
|---:|---:|---:|---:|---:|
| 35 | 52 | 42 | 48 | 26 |

**Rust**

```rust
fn parse_or(s: &str) -> i32 {
    s.parse::<i32>().unwrap_or(0)
}
```

**C**

```c
int parse_or(const char *s) {
    char *end = NULL;
    long v = strtol(s, &end, 10);
    if (end == s) return 0;
    return (int)v;
}
```

**Go**

```go
func parseOr(s string) int32 {
    v, err := strconv.Atoi(s)
    if err != nil {
        return 0
    }
    return int32(v)
}
```

**Ax (corpus / conventional)**

```ax
fn parse_or(s: String) -> i32 !{err[ParseError]} =
    match parse_i32(s) { Ok(v) => v; Err(_) => 0; };
```

**Ax**

```ax
#parse_or(s S) I !err[ParseError] = parse_i32(s)|0;
```
## String interpolation

Build `hello {name}` without a format macro.

| rust | c | go | ax-conv | ax |
|---:|---:|---:|---:|---:|
| 25 | 37 | 25 | 19 | 16 |

**Rust**

```rust
fn greet(name: &str) -> String {
    format!("hello {name}")
}
```

**C**

```c
void greet(const char *name, char *out, size_t n) {
    snprintf(out, n, "hello %s", name);
}
```

**Go**

```go
func greet(name string) string {
    return fmt.Sprintf("hello %s", name)
}
```

**Ax (corpus / conventional)**

```ax
fn greet(name: String) -> String = f"hello {name}";
```

**Ax**

```ax
#greet(name S) S = f"hello {name}";
```
## How to regenerate

```
ax bench usecases
```

Writes this file from `ax::usecases` so the numbers cannot drift from the tokenizer. Ax snippets are complete enough to type-check as a module body (wrap in `module t; export { main };` plus a `main` if needed).
