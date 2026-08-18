//! Research-v1 standard library surface: core, alloc, str, fmt, collections, json, fs, test.
//! Registered as prelude symbols so agents get a closed, typed world.

use crate::effects::{EffectAtom, EffectSet};
use crate::intern::{Interner, Symbol};
use crate::span::Span;
use crate::types::{FnSig, Prim, RegionId, Type, TypeDef, TypeDefKind};

pub struct Builtins {
    pub option: Symbol,
    pub result: Symbol,
    pub string: Symbol,
    pub vec: Symbol,
    pub map: Symbol,
    pub sorted_map: Symbol,
    pub alloc: Symbol,
    pub ordering: Symbol,
    pub div_error: Symbol,
    pub parse_error: Symbol,
    pub alloc_error: Symbol,
    pub fs_error: Symbol,
    pub json_error: Symbol,
    pub fs_read_cap: Symbol,
    pub none: Symbol,
    pub some: Symbol,
    pub ok: Symbol,
    pub err: Symbol,
    pub lt: Symbol,
    pub eq: Symbol,
    pub gt: Symbol,
    pub zero: Symbol,
}

impl Builtins {
    pub fn intern(intern: &mut Interner) -> Self {
        Self {
            option: intern.intern("Option"),
            result: intern.intern("Result"),
            string: intern.intern("String"),
            vec: intern.intern("Vec"),
            map: intern.intern("Map"),
            sorted_map: intern.intern("SortedMap"),
            alloc: intern.intern("Alloc"),
            ordering: intern.intern("Ordering"),
            div_error: intern.intern("DivError"),
            parse_error: intern.intern("ParseError"),
            alloc_error: intern.intern("AllocError"),
            fs_error: intern.intern("Error"), // fs.Error / json.Error resolved via path
            json_error: intern.intern("Error"),
            fs_read_cap: intern.intern("ReadCap"),
            none: intern.intern("None"),
            some: intern.intern("Some"),
            ok: intern.intern("Ok"),
            err: intern.intern("Err"),
            lt: intern.intern("Lt"),
            eq: intern.intern("Eq"),
            gt: intern.intern("Gt"),
            zero: intern.intern("Zero"),
        }
    }
}

pub fn option_type(b: &Builtins, inner: Type) -> Type {
    Type::Named {
        def: b.option,
        args: vec![inner],
    }
}

pub fn result_type(b: &Builtins, ok: Type, err: Type) -> Type {
    Type::Named {
        def: b.result,
        args: vec![ok, err],
    }
}

pub fn vec_type(b: &Builtins, inner: Type) -> Type {
    Type::Named {
        def: b.vec,
        args: vec![inner],
    }
}

pub fn string_type(b: &Builtins) -> Type {
    Type::Named {
        def: b.string,
        args: vec![],
    }
}

pub fn alloc_type(b: &Builtins) -> Type {
    Type::Named {
        def: b.alloc,
        args: vec![],
    }
}

pub fn ordering_type(b: &Builtins) -> Type {
    Type::Named {
        def: b.ordering,
        args: vec![],
    }
}

pub fn static_ref(intern: &mut Interner, inner: Type, mutable: bool) -> Type {
    let r = intern.intern("static");
    Type::Ref {
        region: RegionId::static_region(r),
        mutable,
        inner: Box::new(inner),
    }
}

pub fn str_ref(intern: &mut Interner, region: RegionId) -> Type {
    let s = intern.intern("str");
    Type::Ref {
        region,
        mutable: false,
        inner: Box::new(Type::Named {
            def: s,
            args: vec![],
        }),
    }
}

pub fn core_type_defs(intern: &mut Interner, b: &Builtins) -> Vec<TypeDef> {
    let t = intern.intern("T");
    let e = intern.intern("E");
    let k = intern.intern("K");
    let v = intern.intern("V");
    let value = intern.intern("value");
    let error = intern.intern("error");
    vec![
        TypeDef {
            name: b.option,
            generics: vec![t],
            kind: TypeDefKind::Variants(vec![
                (b.none, vec![]),
                (b.some, vec![(value, Type::Param(t))]),
            ]),
            injections: vec![],
            span: Span::DUMMY,
            def_id: "core::type:Option".into(),
        },
        TypeDef {
            name: b.result,
            generics: vec![t, e],
            kind: TypeDefKind::Variants(vec![
                (b.ok, vec![(value, Type::Param(t))]),
                (b.err, vec![(error, Type::Param(e))]),
            ]),
            injections: vec![],
            span: Span::DUMMY,
            def_id: "core::type:Result".into(),
        },
        TypeDef {
            name: b.string,
            generics: vec![],
            kind: TypeDefKind::Alias(Type::Named {
                def: b.string,
                args: vec![],
            }),
            injections: vec![],
            span: Span::DUMMY,
            def_id: "core::type:String".into(),
        },
        TypeDef {
            name: b.vec,
            generics: vec![t],
            kind: TypeDefKind::Alias(Type::Named {
                def: b.vec,
                args: vec![Type::Param(t)],
            }),
            injections: vec![],
            span: Span::DUMMY,
            def_id: "core::type:Vec".into(),
        },
        TypeDef {
            name: b.map,
            generics: vec![k, v],
            kind: TypeDefKind::Alias(Type::Named {
                def: b.map,
                args: vec![Type::Param(k), Type::Param(v)],
            }),
            injections: vec![],
            span: Span::DUMMY,
            def_id: "core::type:Map".into(),
        },
        TypeDef {
            name: b.sorted_map,
            generics: vec![k, v],
            kind: TypeDefKind::Alias(Type::Named {
                def: b.sorted_map,
                args: vec![Type::Param(k), Type::Param(v)],
            }),
            injections: vec![],
            span: Span::DUMMY,
            def_id: "core::type:SortedMap".into(),
        },
        TypeDef {
            name: b.alloc,
            generics: vec![],
            kind: TypeDefKind::Record(vec![]),
            injections: vec![],
            span: Span::DUMMY,
            def_id: "core::type:Alloc".into(),
        },
        TypeDef {
            name: b.ordering,
            generics: vec![],
            kind: TypeDefKind::Variants(vec![(b.lt, vec![]), (b.eq, vec![]), (b.gt, vec![])]),
            injections: vec![],
            span: Span::DUMMY,
            def_id: "core::type:Ordering".into(),
        },
        TypeDef {
            name: b.div_error,
            generics: vec![],
            kind: TypeDefKind::Variants(vec![(b.zero, vec![])]),
            injections: vec![],
            span: Span::DUMMY,
            def_id: "core::type:DivError".into(),
        },
        TypeDef {
            name: intern.intern("ParseError"),
            generics: vec![],
            kind: TypeDefKind::Variants(vec![(intern.intern("Invalid"), vec![])]),
            injections: vec![],
            span: Span::DUMMY,
            def_id: "core::type:ParseError".into(),
        },
        TypeDef {
            name: intern.intern("AllocError"),
            generics: vec![],
            kind: TypeDefKind::Variants(vec![(intern.intern("Oom"), vec![])]),
            injections: vec![],
            span: Span::DUMMY,
            def_id: "core::type:AllocError".into(),
        },
    ]
}

fn sig(
    intern: &mut Interner,
    def_id: &str,
    name: &str,
    params: Vec<(Symbol, Type, bool)>,
    ret: Type,
    effects: EffectSet,
) -> FnSig {
    let n = intern.intern(name);
    FnSig {
        name: n,
        generics: vec![],
        params,
        ret,
        effects,
        is_contract_fn: false,
        span: Span::DUMMY,
        def_id: def_id.into(),
    }
}

pub fn core_fns(intern: &mut Interner, b: &Builtins) -> Vec<(String, FnSig)> {
    let mut out = Vec::new();
    let a = intern.intern("a");
    let bb = intern.intern("b");
    let xs = intern.intern("xs");
    let i = intern.intern("i");
    let s = intern.intern("s");
    let x = intern.intern("x");
    let y = intern.intern("y");
    let msg = intern.intern("msg");
    let cond = intern.intern("cond");
    let start = intern.intern("start");
    let end = intern.intern("end");
    let n = intern.intern("n");
    let cap = intern.intern("fs_cap");
    let alloc = intern.intern("a");
    let path = intern.intern("path");
    let raw = intern.intern("raw");

    let mut div_eff = EffectSet::new();
    div_eff.insert(EffectAtom::Err(Type::Named {
        def: b.div_error,
        args: vec![],
    }));

    let mut abort_eff = EffectSet::new();
    abort_eff.insert(EffectAtom::Abort);

    let mut diverge_eff = EffectSet::new();
    diverge_eff.insert(EffectAtom::Diverge);

    // int.div / rem — recoverable, and generic over integer width. Hardcoding
    // i32 would leave `usz` and `u64` arithmetic with no division at all.
    let sym_int_t = intern.intern("T");
    for (qid, name) in [
        ("int.div", "div"),
        ("int.rem", "rem"),
        ("int.div_trunc", "div_trunc"),
    ] {
        out.push((
            qid.into(),
            FnSig {
                name: intern.intern(name),
                generics: vec![sym_int_t],
                params: vec![
                    (a, Type::Param(sym_int_t), false),
                    (bb, Type::Param(sym_int_t), false),
                ],
                ret: Type::Param(sym_int_t),
                effects: div_eff.clone(),
                is_contract_fn: false,
                span: Span::DUMMY,
                def_id: format!("core::fn:{qid}"),
            },
        ));
    }
    // int.div_exact — abort on zero (pre b != 0)
    out.push((
        "int.div_exact".into(),
        sig(
            intern,
            "core::fn:int.div_exact",
            "div_exact",
            vec![(a, Type::i32(), false), (bb, Type::i32(), false)],
            Type::i32(),
            abort_eff.clone(),
        ),
    ));

    // checked_*
    for name in ["checked_add", "checked_sub", "checked_mul"] {
        out.push((
            format!("int.{name}"),
            sig(
                intern,
                &format!("core::fn:int.{name}"),
                name,
                vec![(a, Type::i32(), false), (bb, Type::i32(), false)],
                option_type(b, Type::i32()),
                EffectSet::empty(),
            ),
        ));
    }

    // math
    // v0.3: explicit conversions replace `as`. Widening is total; narrowing
    // is `try_to_*` → Result.
    for (from, to, qid) in [
        (Type::i32(), Type::i64(), "to_i64"),
        (Type::i32(), Type::f64(), "to_f64"),
        (Type::i64(), Type::f64(), "to_f64"),
        (Type::u32(), Type::u64(), "to_u64"),
        (Type::usz(), Type::u64(), "to_u64"),
    ] {
        out.push((
            qid.into(),
            sig(
                intern,
                &format!("core::fn:{qid}"),
                qid,
                vec![(x, from.clone(), false)],
                to.clone(),
                EffectSet::empty(),
            ),
        ));
    }
    out.push((
        "try_to_u8".into(),
        sig(
            intern,
            "core::fn:try_to_u8",
            "try_to_u8",
            vec![(x, Type::i64(), false)],
            result_type(b, Type::Prim(Prim::U8), Type::Named { def: b.parse_error, args: vec![] }),
            EffectSet::empty(),
        ),
    ));
    let sym_dt = intern.intern("T");
    out.push((
        "declassify".into(),
        sig(
            intern,
            "core::fn:declassify",
            "declassify",
            vec![(x, Type::Param(sym_dt), false)],
            Type::Param(sym_dt),
            EffectSet::empty(),
        ),
    ));
    out.push((
        "math.hypot".into(),
        sig(
            intern,
            "core::fn:math.hypot",
            "hypot",
            vec![(x, Type::f32(), false), (y, Type::f32(), false)],
            Type::f32(),
            EffectSet::empty(),
        ),
    ));
    out.push((
        "math.sqrt".into(),
        sig(
            intern,
            "core::fn:math.sqrt",
            "sqrt",
            vec![(x, Type::f64(), false)],
            Type::f64(),
            EffectSet::empty(),
        ),
    ));
    out.push((
        "math.abs".into(),
        sig(
            intern,
            "core::fn:math.abs",
            "abs",
            vec![(x, Type::f32(), false)],
            Type::f32(),
            EffectSet::empty(),
        ),
    ));
    out.push((
        "f32.abs".into(),
        sig(
            intern,
            "core::fn:f32.abs",
            "abs",
            vec![(x, Type::f32(), false)],
            Type::f32(),
            EffectSet::empty(),
        ),
    ));
    // Comparators take `&T`, so one function serves both as a `sort` argument
    // and as a dictionary field. Auto-ref lets a call site still pass a value.
    let f32_ref = static_ref(intern, Type::f32(), false);
    let i32_ref = static_ref(intern, Type::i32(), false);
    out.push((
        "f32.cmp".into(),
        sig(
            intern,
            "core::fn:f32.cmp",
            "cmp",
            vec![(x, f32_ref.clone(), false), (y, f32_ref, false)],
            ordering_type(b),
            EffectSet::empty(),
        ),
    ));
    out.push((
        "i32.cmp".into(),
        sig(
            intern,
            "core::fn:i32.cmp",
            "cmp",
            vec![(x, i32_ref.clone(), false), (y, i32_ref, false)],
            ordering_type(b),
            EffectSet::empty(),
        ),
    ));

    let sym_range = intern.intern("Range");
    let sym_t = intern.intern("T");
    let sym_u = intern.intern("U");
    let sym_seq = intern.intern("Seq");
    let sym_freeze = intern.intern("Freeze");
    let sym_r = intern.intern("r");
    let sym_static = intern.intern("static");
    let sym_parse_error = intern.intern("ParseError");
    let sym_fs_error = intern.intern("fs.Error");
    let sym_json_error = intern.intern("json.Error");
    let sym_fs_read_cap = intern.intern("fs.ReadCap");
    let sym_rec = intern.intern("Rec");
    let sym_files = intern.intern("files");
    let sym_slice = intern.intern("slice");

    // range — finite iterator
    out.push((
        "range".into(),
        sig(
            intern,
            "core::fn:range",
            "range",
            vec![(start, Type::usz(), false), (end, Type::usz(), false)],
            Type::Named {
                def: sym_range,
                args: vec![Type::usz()],
            },
            EffectSet::empty(),
        ),
    ));

    // assert / fail
    out.push((
        "assert".into(),
        sig(
            intern,
            "core::fn:assert",
            "assert",
            vec![(cond, Type::bool(), false)],
            Type::unit(),
            abort_eff.clone(),
        ),
    ));
    out.push((
        "fail".into(),
        sig(
            intern,
            "core::fn:fail",
            "fail",
            vec![(msg, string_type(b), false)],
            Type::unit(),
            abort_eff.clone(),
        ),
    ));

    // freeze
    let src = intern.intern("src");
    let out_a = intern.intern("out");
    let dict = intern.intern("dict");
    let mut freeze_eff = EffectSet::new();
    freeze_eff.insert(EffectAtom::Alloc(out_a));
    out.push((
        "freeze".into(),
        sig(
            intern,
            "core::fn:freeze",
            "freeze",
            vec![
                (
                    src,
                    Type::Ref {
                        region: RegionId::static_region(sym_r),
                        mutable: false,
                        inner: Box::new(Type::Param(sym_t)),
                    },
                    false,
                ),
                (out_a, alloc_type(b), false),
                (dict, Type::Param(sym_freeze), false),
            ],
            Type::Own(Box::new(Type::Param(sym_u))),
            freeze_eff,
        ),
    ));

    // parse
    let mut parse_eff = EffectSet::new();
    parse_eff.insert(EffectAtom::Err(Type::Named {
        def: sym_parse_error,
        args: vec![],
    }));
    let parse_str_ty = str_ref(intern, RegionId::static_region(sym_static));
    out.push((
        "parse_i32".into(),
        sig(
            intern,
            "core::fn:parse_i32",
            "parse_i32",
            vec![(s, parse_str_ty, false)],
            Type::i32(),
            parse_eff,
        ),
    ));

    // print — io
    let stdout = intern.intern("stdout");
    let mut io_eff = EffectSet::new();
    io_eff.insert(EffectAtom::Io(stdout));
    out.push((
        "print".into(),
        sig(
            intern,
            "core::fn:print",
            "print",
            vec![(s, string_type(b), false)],
            Type::unit(),
            io_eff,
        ),
    ));

    // all / any / count / sorted_by — contract primitives, also usable in ordinary code
    // over finite sequences. Predicates are lambdas.
    let pred = intern.intern("pred");
    out.push((
        "all".into(),
        sig(
            intern,
            "core::fn:all",
            "all",
            vec![
                (xs, Type::Param(sym_seq), false),
                (
                    pred,
                    Type::Fn {
                        params: vec![Type::Param(sym_t)],
                        ret: Box::new(Type::bool()),
                        effects: EffectSet::empty(),
                    },
                    false,
                ),
            ],
            Type::bool(),
            EffectSet::empty(),
        ),
    ));
    out.push((
        "any".into(),
        sig(
            intern,
            "core::fn:any",
            "any",
            vec![
                (xs, Type::Param(sym_seq), false),
                (
                    pred,
                    Type::Fn {
                        params: vec![Type::Param(sym_t)],
                        ret: Box::new(Type::bool()),
                        effects: EffectSet::empty(),
                    },
                    false,
                ),
            ],
            Type::bool(),
            EffectSet::empty(),
        ),
    ));
    out.push((
        "count".into(),
        sig(
            intern,
            "core::fn:count",
            "count",
            vec![
                (xs, Type::Param(sym_seq), false),
                (
                    pred,
                    Type::Fn {
                        params: vec![Type::Param(sym_t)],
                        ret: Box::new(Type::bool()),
                        effects: EffectSet::empty(),
                    },
                    false,
                ),
            ],
            Type::usz(),
            EffectSet::empty(),
        ),
    ));
    out.push((
        "sorted_by".into(),
        sig(
            intern,
            "core::fn:sorted_by",
            "sorted_by",
            vec![
                (xs, Type::Param(sym_seq), false),
                (
                    pred,
                    Type::Fn {
                        params: vec![Type::Param(sym_t), Type::Param(sym_t)],
                        ret: Box::new(Type::bool()),
                        effects: EffectSet::empty(),
                    },
                    false,
                ),
            ],
            Type::bool(),
            EffectSet::empty(),
        ),
    ));
    out.push((
        "len".into(),
        sig(
            intern,
            "core::fn:len",
            "len",
            vec![(xs, Type::Param(sym_seq), false)],
            Type::usz(),
            EffectSet::empty(),
        ),
    ));

    // sort[T](xs: &mut [T], cmp: fn(&T, &T) -> Ordering)
    //
    // An explicit comparator, like Go's sort.Slice and Rust's sort_by. The sort
    // is stable, so the output permutation is unique and the oracle and the
    // native tiers agree element for element.
    let t = sym_t;
    let cmp_name = intern.intern("cmp");
    let rgn = sym_r;
    let elem_ref = Type::Ref {
        region: RegionId {
            name: rgn,
            depth: 0,
        },
        mutable: false,
        inner: Box::new(Type::Param(t)),
    };
    out.push((
        "sort".into(),
        FnSig {
            name: intern.intern("sort"),
            generics: vec![t],
            params: vec![
                (
                    xs,
                    Type::Ref {
                        region: RegionId {
                            name: rgn,
                            depth: 0,
                        },
                        mutable: true,
                        inner: Box::new(Type::Named {
                            def: sym_slice,
                            args: vec![Type::Param(t)],
                        }),
                    },
                    false,
                ),
                (
                    cmp_name,
                    Type::Fn {
                        params: vec![elem_ref.clone(), elem_ref],
                        ret: Box::new(ordering_type(b)),
                        effects: EffectSet::empty(),
                    },
                    false,
                ),
            ],
            ret: Type::unit(),
            effects: diverge_eff,
            is_contract_fn: false,
            span: Span::DUMMY,
            def_id: "core::fn:sort".into(),
        },
    ));

    // fs / json / test — capability-mediated
    let mut fs_eff = EffectSet::new();
    fs_eff.insert(EffectAtom::Io(cap));
    fs_eff.insert(EffectAtom::Alloc(alloc));
    fs_eff.insert(EffectAtom::Err(Type::Named {
        def: sym_fs_error,
        args: vec![],
    }));
    let fs_path_ty = str_ref(intern, RegionId::static_region(sym_static));
    out.push((
        "fs.read".into(),
        sig(
            intern,
            "std.fs::fn:read",
            "read",
            vec![
                (
                    cap,
                    Type::Named {
                        def: sym_fs_read_cap,
                        args: vec![],
                    },
                    false,
                ),
                (alloc, alloc_type(b), false),
                (path, fs_path_ty, false),
            ],
            string_type(b),
            fs_eff,
        ),
    ));

    let mut json_eff = EffectSet::new();
    json_eff.insert(EffectAtom::Alloc(alloc));
    json_eff.insert(EffectAtom::Err(Type::Named {
        def: sym_json_error,
        args: vec![],
    }));
    out.push((
        "json.decode_recs".into(),
        sig(
            intern,
            "std.json::fn:decode_recs",
            "decode_recs",
            vec![(alloc, alloc_type(b), false), (raw, string_type(b), false)],
            vec_type(
                b,
                Type::Named {
                    def: sym_rec,
                    args: vec![],
                },
            ),
            json_eff.clone(),
        ),
    ));
    out.push((
        "json.decode".into(),
        sig(
            intern,
            "std.json::fn:decode",
            "decode",
            vec![(alloc, alloc_type(b), false), (raw, string_type(b), false)],
            Type::Param(sym_t),
            json_eff,
        ),
    ));

    out.push((
        "test.read_cap".into(),
        sig(
            intern,
            "std.test::fn:read_cap",
            "read_cap",
            vec![(sym_files, Type::Record(vec![]), false)],
            Type::Named {
                def: sym_fs_read_cap,
                args: vec![],
            },
            EffectSet::empty(),
        ),
    ));
    out.push((
        "test.alloc".into(),
        FnSig {
            name: intern.intern("alloc"),
            generics: vec![],
            params: vec![],
            ret: alloc_type(b),
            effects: EffectSet::empty(),
            is_contract_fn: false,
            span: Span::DUMMY,
            def_id: "std.test::fn:alloc".into(),
        },
    ));

    // ---- containers ----
    // `vec.new` is the only way to get a Vec, and it takes the allocator that
    // will own its storage. There is no ambient heap: `alloc[a]` in the row
    // names the handle every allocation came from.
    let mut alloc_eff = EffectSet::new();
    alloc_eff.insert(EffectAtom::Alloc(alloc));
    out.push((
        "vec.new".into(),
        FnSig {
            name: intern.intern("new"),
            generics: vec![sym_t],
            params: vec![(alloc, alloc_type(b), false)],
            ret: vec_type(b, Type::Param(sym_t)),
            effects: alloc_eff.clone(),
            is_contract_fn: false,
            span: Span::DUMMY,
            def_id: "core::fn:vec.new".into(),
        },
    ));
    let sym_k = intern.intern("K");
    let sym_v = intern.intern("V");
    out.push((
        "map.new".into(),
        FnSig {
            name: intern.intern("new"),
            generics: vec![sym_k, sym_v],
            params: vec![(alloc, alloc_type(b), false)],
            ret: Type::Named {
                def: b.map,
                args: vec![Type::Param(sym_k), Type::Param(sym_v)],
            },
            effects: alloc_eff.clone(),
            is_contract_fn: false,
            span: Span::DUMMY,
            def_id: "core::fn:map.new".into(),
        },
    ));
    out.push((
        "str.concat".into(),
        sig(
            intern,
            "core::fn:str.concat",
            "concat",
            vec![
                (alloc, alloc_type(b), false),
                (x, string_type(b), false),
                (y, string_type(b), false),
            ],
            string_type(b),
            alloc_eff,
        ),
    ));

    // ---- corelib IO / HTTP (native-backed) ----
    let path_s = intern.intern("path");
    let url = intern.intern("url");
    let data = intern.intern("data");
    let port = intern.intern("port");
    let body = intern.intern("body");
    let mut io_abort = EffectSet::new();
    io_abort.insert(EffectAtom::Io(intern.intern("fs")));
    io_abort.insert(EffectAtom::Abort);
    let mut net_abort = EffectSet::new();
    net_abort.insert(EffectAtom::Io(intern.intern("net")));
    net_abort.insert(EffectAtom::Abort);

    let io_path = str_ref(intern, RegionId::static_region(sym_static));
    out.push((
        "io.bytesum_file".into(),
        sig(
            intern,
            "core::fn:io.bytesum_file",
            "bytesum_file",
            vec![(path_s, io_path.clone(), false)],
            Type::Prim(Prim::U64),
            io_abort.clone(),
        ),
    ));
    out.push((
        "io.read_file".into(),
        sig(
            intern,
            "core::fn:io.read_file",
            "read_file",
            vec![(path_s, io_path.clone(), false)],
            Type::usz(),
            io_abort.clone(),
        ),
    ));
    out.push((
        "io.write_file".into(),
        sig(
            intern,
            "core::fn:io.write_file",
            "write_file",
            vec![
                (path_s, io_path.clone(), false),
                (data, string_type(b), false),
            ],
            Type::usz(),
            io_abort,
        ),
    ));
    out.push((
        "http.get_bytesum".into(),
        sig(
            intern,
            "core::fn:http.get_bytesum",
            "get_bytesum",
            vec![(url, io_path.clone(), false)],
            Type::Prim(Prim::U64),
            net_abort.clone(),
        ),
    ));
    out.push((
        "http.get".into(),
        sig(
            intern,
            "core::fn:http.get",
            "get",
            vec![(url, io_path.clone(), false)],
            Type::usz(),
            net_abort.clone(),
        ),
    ));
    out.push((
        "http.serve".into(),
        sig(
            intern,
            "core::fn:http.serve",
            "serve",
            vec![
                (port, Type::Prim(Prim::U16), false),
                (body, string_type(b), false),
            ],
            Type::unit(),
            net_abort,
        ),
    ));
    let mut argv_row = EffectSet::new();
    argv_row.insert(EffectAtom::Io(intern.intern("argv")));
    let argv_i = intern.intern("i");
    let argv_ret = Type::Named {
        def: intern.intern("str"),
        args: vec![],
    };
    out.push((
        "argv".into(),
        sig(
            intern,
            "core::fn:argv",
            "argv",
            vec![(argv_i, Type::i32(), false)],
            argv_ret,
            argv_row,
        ),
    ));

    let _ = (n, i);
    out
}

pub fn extra_type_defs(intern: &mut Interner) -> Vec<TypeDef> {
    vec![
        TypeDef {
            name: intern.intern("fs.Error"),
            generics: vec![],
            kind: TypeDefKind::Variants(vec![(
                intern.intern("NotFound"),
                vec![(intern.intern("path"), Type::Prim(Prim::Unit))],
            )]),
            injections: vec![],
            span: Span::DUMMY,
            def_id: "std.fs::type:Error".into(),
        },
        TypeDef {
            name: intern.intern("json.Error"),
            generics: vec![],
            kind: TypeDefKind::Variants(vec![(intern.intern("Invalid"), vec![])]),
            injections: vec![],
            span: Span::DUMMY,
            def_id: "std.json::type:Error".into(),
        },
        TypeDef {
            name: intern.intern("fs.ReadCap"),
            generics: vec![],
            kind: TypeDefKind::Record(vec![]),
            injections: vec![],
            span: Span::DUMMY,
            def_id: "std.fs::type:ReadCap".into(),
        },
        TypeDef {
            name: intern.intern("str"),
            generics: vec![],
            kind: TypeDefKind::Alias(Type::Named {
                def: intern.intern("str"),
                args: vec![],
            }),
            injections: vec![],
            span: Span::DUMMY,
            def_id: "core::type:str".into(),
        },
        TypeDef {
            name: intern.intern("Range"),
            generics: vec![intern.intern("T")],
            kind: TypeDefKind::Record(vec![
                (intern.intern("start"), Type::usz()),
                (intern.intern("end"), Type::usz()),
            ]),
            injections: vec![],
            span: Span::DUMMY,
            def_id: "core::type:Range".into(),
        },
        TypeDef {
            name: intern.intern("Ord"),
            generics: vec![intern.intern("T")],
            kind: TypeDefKind::Record(vec![(
                intern.intern("cmp"),
                Type::Fn {
                    params: vec![
                        Type::Ref {
                            region: RegionId::static_region(intern.intern("static")),
                            mutable: false,
                            inner: Box::new(Type::Param(intern.intern("T"))),
                        },
                        Type::Ref {
                            region: RegionId::static_region(intern.intern("static")),
                            mutable: false,
                            inner: Box::new(Type::Param(intern.intern("T"))),
                        },
                    ],
                    ret: Box::new(Type::Named {
                        def: intern.intern("Ordering"),
                        args: vec![],
                    }),
                    effects: EffectSet::empty(),
                },
            )]),
            injections: vec![],
            span: Span::DUMMY,
            def_id: "core::type:Ord".into(),
        },
        TypeDef {
            name: intern.intern("slice"),
            generics: vec![intern.intern("T")],
            kind: TypeDefKind::Alias(Type::Named {
                def: intern.intern("slice"),
                args: vec![Type::Param(intern.intern("T"))],
            }),
            injections: vec![],
            span: Span::DUMMY,
            def_id: "core::type:slice".into(),
        },
    ]
}
