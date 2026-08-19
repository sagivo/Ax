# Ax token efficiency — side-by-side

Compact implementations of the same 9 tasks in TypeScript, Python, C, Rust, and Ax. Counts are exact BPE counts, not character counts or a home-grown approximation.

## o200k_base

| case | TypeScript | Python | C | Rust | Ax | Ax vs best mainstream |
|---|---:|---:|---:|---:|---:|---:|
| Add two integers | 12 | 8 | 11 | 15 | 7 | 12% |
| If / else | 13 | 12 | 12 | 17 | 10 | 17% |
| Sum a range | 26 | 10 | 28 | 15 | 8 | 20% |
| Recursion | 20 | 19 | 18 | 24 | 16 | 11% |
| Map get-or | 19 | 12 | 27 | 27 | 9 | 25% |
| Map insert + get | 42 | 28 | 46 | 49 | 19 | 32% |
| Fallible parse | 13 | 16 | 30 | 17 | 12 | 8% |
| String interpolation | 11 | 11 | 21 | 15 | 9 | 18% |
| Dot product | 29 | 17 | 44 | 47 | 17 | 0% |
| **total** | **185** | **133** | **237** | **226** | **107** | **20%** |

## cl100k_base

| case | TypeScript | Python | C | Rust | Ax | Ax vs best mainstream |
|---|---:|---:|---:|---:|---:|---:|
| Add two integers | 12 | 8 | 11 | 15 | 7 | 12% |
| If / else | 13 | 12 | 12 | 17 | 10 | 17% |
| Sum a range | 26 | 10 | 28 | 14 | 8 | 20% |
| Recursion | 20 | 19 | 18 | 23 | 16 | 11% |
| Map get-or | 18 | 12 | 27 | 25 | 9 | 25% |
| Map insert + get | 41 | 28 | 46 | 48 | 19 | 32% |
| Fallible parse | 12 | 15 | 29 | 17 | 12 | 0% |
| String interpolation | 11 | 11 | 21 | 15 | 9 | 18% |
| Dot product | 27 | 17 | 44 | 45 | 15 | 12% |
| **total** | **180** | **132** | **236** | **219** | **105** | **20%** |

Lower is better. The percentage compares Ax with the smallest of the four mainstream implementations on that row. A negative percentage means Ax lost that case.

## Source, side by side

### Add two integers

Define a two-argument integer addition function.

| TypeScript | Python | C | Rust | Ax |
|---|---|---|---|---|
| <pre>const add=(a:number,b:number):number=&gt;a+b</pre><small>12 tokens</small> | <pre>def add(a,b):return a+b</pre><small>8 tokens</small> | <pre>int add(int a,int b){return a+b;}</pre><small>11 tokens</small> | <pre>fn add(a:i32,b:i32)-&gt;i32{a+b}</pre><small>15 tokens</small> | <pre>#add(a,b)=a+b</pre><small>7 tokens</small> |

### If / else

Choose one of two integers from a boolean.

| TypeScript | Python | C | Rust | Ax |
|---|---|---|---|---|
| <pre>const pick=(b:boolean):number=&gt;b?1:0</pre><small>13 tokens</small> | <pre>def pick(b):return 1 if b else 0</pre><small>12 tokens</small> | <pre>int pick(int b){return b?1:0;}</pre><small>12 tokens</small> | <pre>fn pick(b:bool)-&gt;i32{if b{1}else{0}}</pre><small>17 tokens</small> | <pre>#pick(b B)=b??1:0</pre><small>10 tokens</small> |

### Sum a range

Accumulate the integers from zero through n-1.

| TypeScript | Python | C | Rust | Ax |
|---|---|---|---|---|
| <pre>function sum(n:number){let s=0;for(let i=0;i&lt;n;i++)s+=i;return s}</pre><small>26 tokens</small> | <pre>def sum_n(n):return sum(range(n))</pre><small>10 tokens</small> | <pre>unsigned long sum(unsigned n){unsigned long s=0;for(unsigned i=0;i&lt;n;i++)s+=i;return s;}</pre><small>28 tokens</small> | <pre>fn sum(n:usize)-&gt;usize{(0..n).sum()}</pre><small>15 tokens</small> | <pre>#sum(n:Z)=+/n</pre><small>8 tokens</small> |

### Recursion

Define factorial with a single recursive call.

| TypeScript | Python | C | Rust | Ax |
|---|---|---|---|---|
| <pre>const fac=(n:number):number=&gt;n&lt;2?1:n*fac(n-1)</pre><small>20 tokens</small> | <pre>def fac(n):return 1 if n&lt;2 else n*fac(n-1)</pre><small>19 tokens</small> | <pre>int fac(int n){return n&lt;2?1:n*fac(n-1);}</pre><small>18 tokens</small> | <pre>fn fac(n:i32)-&gt;i32{if n&lt;2{1}else{n*fac(n-1)}}</pre><small>24 tokens</small> | <pre>#fac(n)=n&lt;2??1:n*fac(n-1)</pre><small>16 tokens</small> |

### Map get-or

Read a map entry and return zero when the key is absent.

| TypeScript | Python | C | Rust | Ax |
|---|---|---|---|---|
| <pre>const get=(m:Map&lt;string,number&gt;,k:string)=&gt;m.get(k)??0</pre><small>19 tokens</small> | <pre>def get(m,k):return m.get(k,0)</pre><small>12 tokens</small> | <pre>long get(AxMap*m,AxStr*k){long v=0;ax_map_get(m,k,&amp;v);return v;}</pre><small>27 tokens</small> | <pre>fn get(m:&amp;HashMap&lt;String,i64&gt;,k:&amp;str)-&gt;i64{*m.get(k).unwrap_or(&amp;0)}</pre><small>27 tokens</small> | <pre>#get(m M,k S)=m[k]?</pre><small>9 tokens</small> |

### Map insert + get

Allocate a map, insert two keys, and return their sum.

| TypeScript | Python | C | Rust | Ax |
|---|---|---|---|---|
| <pre>function f(){const m=new Map&lt;string,number&gt;();m.set("e",2);m.set("o",3);return(m.get("e")??0)+(m.get("o")??0)}</pre><small>42 tokens</small> | <pre>def f():m={"e":2,"o":3};return m.get("e",0)+m.get("o",0)</pre><small>28 tokens</small> | <pre>long f(){AxMap*m=ax_map_new();ax_map_put(m,"e",2);ax_map_put(m,"o",3);return ax_map_get0(m,"e")+ax_map_get0(m,"o");}</pre><small>46 tokens</small> | <pre>fn f()-&gt;i64{let mut m=HashMap::new();m.insert("e",2);m.insert("o",3);m.get("e").unwrap_or(&amp;0)+m.get("o").unwrap_or(&amp;0)}</pre><small>49 tokens</small> | <pre>#f={m:={e:2,o:3};m[e]+m[o]}</pre><small>19 tokens</small> |

### Fallible parse

Parse an integer and return zero on failure.

| TypeScript | Python | C | Rust | Ax |
|---|---|---|---|---|
| <pre>const parseOr=(s:string)=&gt;Number.parseInt(s)&#124;&#124;0</pre><small>13 tokens</small> | <pre>def parse_or(s):
 try:return int(s)
 except ValueError:return 0</pre><small>16 tokens</small> | <pre>int parse_or(char*s){char*e;long v=strtol(s,&amp;e,10);return e==s?0:(int)v;}</pre><small>30 tokens</small> | <pre>fn parse_or(s:&amp;str)-&gt;i32{s.parse().unwrap_or(0)}</pre><small>17 tokens</small> | <pre>#parse_or(s S)=parse_i32(s)&#124;0</pre><small>12 tokens</small> |

### String interpolation

Build `hello {name}` with the language's standard interpolation.

| TypeScript | Python | C | Rust | Ax |
|---|---|---|---|---|
| <pre>const greet=(name:string)=&gt;`hello ${name}`</pre><small>11 tokens</small> | <pre>def greet(name):return f"hello {name}"</pre><small>11 tokens</small> | <pre>void greet(char*name,char*out,int n){snprintf(out,n,"hello %s",name);}</pre><small>21 tokens</small> | <pre>fn greet(name:&amp;str)-&gt;String{format!("hello {name}")}</pre><small>15 tokens</small> | <pre>#greet(name:S)="hello {name}"</pre><small>9 tokens</small> |

### Dot product

Multiply corresponding vector elements and sum the products.

| TypeScript | Python | C | Rust | Ax |
|---|---|---|---|---|
| <pre>const dot=(a:bigint[],b:bigint[])=&gt;a.reduce((s,x,i)=&gt;s+x*b[i],0n)</pre><small>29 tokens</small> | <pre>def dot(a,b):return sum(x*y for x,y in zip(a,b))</pre><small>17 tokens</small> | <pre>uint64_t dot(uint64_t*a,uint64_t*b,size_t n){uint64_t s=0;for(size_t i=0;i&lt;n;i++)s+=a[i]*b[i];return s;}</pre><small>44 tokens</small> | <pre>fn dot(a:&amp;[u64],b:&amp;[u64])-&gt;u64{a.iter().zip(b).fold(0,&#124;s,(x,y)&#124;s.wrapping_add(x.wrapping_mul(*y)))}</pre><small>47 tokens</small> | <pre>#dot(a V[W],b V[W])W=+/a*b</pre><small>17 tokens</small> |

## Method

- Primary encoding: `o200k_base`, used by current GPT, reasoning, and Codex model families.
- Regression encoding: `cl100k_base`, used by GPT-4-era models.
- Counter: `tiktoken-rs`, using the actual OpenAI vocabulary files.
- Scope: method bodies/signatures only. Imports, tests, comments, and executable wrappers are excluded for every language.
- Formatting: optional whitespace is removed in every language. Required Python indentation remains.
- Ax is generated mechanically from the conventional corpus form with `to_dense`, then compiled by this tool's regression tests.

This is a controlled corpus, not proof that Ax is shortest for every possible program or tokenizer. Cases live in `tools/ax-density/src/lib.rs`; counterexamples should be added there.

## Regenerate

```sh
cargo run -p ax-density -- --write
```
