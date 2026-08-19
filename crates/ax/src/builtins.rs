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
            result_type(
                b,
                Type::Prim(Prim::U8),
                Type::Named {
                    def: b.parse_error,
                    args: vec![],
                },
            ),
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
    let sym_db_error = intern.intern("db.Error");
    let sym_db_pool = intern.intern("db.Pool");
    let sym_db_tx = intern.intern("db.Tx");
    let sym_db_value = intern.intern("db.Value");
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
                (path, fs_path_ty.clone(), false),
            ],
            Type::Untrusted(Box::new(string_type(b))),
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
    let mut json_decode_sig = sig(
        intern,
        "std.json::fn:decode",
        "decode",
        vec![(alloc, alloc_type(b), false), (raw, string_type(b), false)],
        Type::Param(sym_t),
        json_eff,
    );
    json_decode_sig.generics = vec![sym_t];
    out.push(("json.decode".into(), json_decode_sig));
    let value = intern.intern("value");
    let mut json_encode_sig = sig(
        intern,
        "std.json::fn:encode",
        "encode",
        vec![
            (alloc, alloc_type(b), false),
            (value, Type::Param(sym_t), false),
        ],
        string_type(b),
        {
            let mut effects = EffectSet::new();
            effects.insert(EffectAtom::Alloc(alloc));
            effects
        },
    );
    json_encode_sig.generics = vec![sym_t];
    out.push(("json.encode".into(), json_encode_sig));

    let db_pool_type = Type::Named {
        def: sym_db_pool,
        args: vec![],
    };
    let db_tx_type = Type::Named {
        def: sym_db_tx,
        args: vec![],
    };
    let db_value_type = Type::Named {
        def: sym_db_value,
        args: vec![],
    };
    let db_error_type = Type::Named {
        def: sym_db_error,
        args: vec![],
    };
    let mut db_effects = EffectSet::new();
    db_effects.insert(EffectAtom::Io(intern.intern("db")));
    db_effects.insert(EffectAtom::Err(db_error_type.clone()));
    let mut db_query_effects = db_effects.clone();
    db_query_effects.insert(EffectAtom::Alloc(alloc));
    let sql = intern.intern("sql");
    let params = intern.intern("params");
    let pool = intern.intern("pool");
    let tx = intern.intern("tx");
    let timeout_ms = intern.intern("timeout_ms");
    let string_vec = vec_type(b, string_type(b));
    let value_vec = vec_type(b, db_value_type.clone());
    out.push((
        "db.open".into(),
        sig(
            intern,
            "std.db::fn:open",
            "open",
            vec![(path, fs_path_ty.clone(), false)],
            db_pool_type.clone(),
            db_effects.clone(),
        ),
    ));
    out.push((
        "db.open_timeout".into(),
        sig(
            intern,
            "std.db::fn:open_timeout",
            "open_timeout",
            vec![
                (path, fs_path_ty.clone(), false),
                (timeout_ms, Type::Prim(Prim::U32), false),
            ],
            db_pool_type.clone(),
            db_effects.clone(),
        ),
    ));
    out.push((
        "db.set_timeout".into(),
        sig(
            intern,
            "std.db::fn:set_timeout",
            "set_timeout",
            vec![
                (pool, db_pool_type.clone(), false),
                (timeout_ms, Type::Prim(Prim::U32), false),
            ],
            Type::unit(),
            db_effects.clone(),
        ),
    ));
    let mut db_close_effects = EffectSet::new();
    db_close_effects.insert(EffectAtom::Io(intern.intern("db")));
    out.push((
        "db.close".into(),
        sig(
            intern,
            "std.db::fn:close",
            "close",
            vec![(pool, db_pool_type.clone(), false)],
            Type::unit(),
            db_close_effects,
        ),
    ));
    for (qualified, name, with_params) in [("db.exec0", "exec0", false), ("db.exec", "exec", true)]
    {
        let mut arguments = vec![
            (pool, db_pool_type.clone(), false),
            (sql, string_type(b), false),
        ];
        if with_params {
            arguments.push((params, string_vec.clone(), false));
        }
        out.push((
            qualified.into(),
            sig(
                intern,
                &format!("std.db::fn:{name}"),
                name,
                arguments,
                Type::u64(),
                db_effects.clone(),
            ),
        ));
    }
    for (qualified, name, with_params) in
        [("db.query0", "query0", false), ("db.query", "query", true)]
    {
        let mut arguments = vec![
            (pool, db_pool_type.clone(), false),
            (alloc, alloc_type(b), false),
            (sql, string_type(b), false),
        ];
        if with_params {
            arguments.push((params, string_vec.clone(), false));
        }
        let mut query_sig = sig(
            intern,
            &format!("std.db::fn:{name}"),
            name,
            arguments,
            vec_type(b, Type::Param(sym_t)),
            db_query_effects.clone(),
        );
        query_sig.generics = vec![sym_t];
        out.push((qualified.into(), query_sig));
    }
    out.push((
        "db.exec_values".into(),
        sig(
            intern,
            "std.db::fn:exec_values",
            "exec_values",
            vec![
                (pool, db_pool_type.clone(), false),
                (sql, string_type(b), false),
                (params, value_vec.clone(), false),
            ],
            Type::u64(),
            db_effects.clone(),
        ),
    ));
    let mut db_values_query = sig(
        intern,
        "std.db::fn:query_values",
        "query_values",
        vec![
            (pool, db_pool_type.clone(), false),
            (alloc, alloc_type(b), false),
            (sql, string_type(b), false),
            (params, value_vec.clone(), false),
        ],
        vec_type(b, Type::Param(sym_t)),
        db_query_effects.clone(),
    );
    db_values_query.generics = vec![sym_t];
    out.push(("db.query_values".into(), db_values_query));
    out.push((
        "db.begin".into(),
        sig(
            intern,
            "std.db::fn:begin",
            "begin",
            vec![(pool, db_pool_type.clone(), false)],
            db_tx_type.clone(),
            db_effects.clone(),
        ),
    ));
    let tx_ref = static_ref(intern, db_tx_type.clone(), true);
    for (qualified, name, with_params) in [
        ("db.tx_exec0", "tx_exec0", false),
        ("db.tx_exec", "tx_exec", true),
    ] {
        let mut arguments = vec![(tx, tx_ref.clone(), false), (sql, string_type(b), false)];
        if with_params {
            arguments.push((params, string_vec.clone(), false));
        }
        out.push((
            qualified.into(),
            sig(
                intern,
                &format!("std.db::fn:{name}"),
                name,
                arguments,
                Type::u64(),
                db_effects.clone(),
            ),
        ));
    }
    for (qualified, name, with_params) in [
        ("db.tx_query0", "tx_query0", false),
        ("db.tx_query", "tx_query", true),
    ] {
        let mut arguments = vec![
            (tx, tx_ref.clone(), false),
            (alloc, alloc_type(b), false),
            (sql, string_type(b), false),
        ];
        if with_params {
            arguments.push((params, string_vec.clone(), false));
        }
        let mut query_sig = sig(
            intern,
            &format!("std.db::fn:{name}"),
            name,
            arguments,
            vec_type(b, Type::Param(sym_t)),
            db_query_effects.clone(),
        );
        query_sig.generics = vec![sym_t];
        out.push((qualified.into(), query_sig));
    }
    out.push((
        "db.tx_exec_values".into(),
        sig(
            intern,
            "std.db::fn:tx_exec_values",
            "tx_exec_values",
            vec![
                (tx, tx_ref.clone(), false),
                (sql, string_type(b), false),
                (params, value_vec.clone(), false),
            ],
            Type::u64(),
            db_effects.clone(),
        ),
    ));
    let mut db_tx_values_query = sig(
        intern,
        "std.db::fn:tx_query_values",
        "tx_query_values",
        vec![
            (tx, tx_ref, false),
            (alloc, alloc_type(b), false),
            (sql, string_type(b), false),
            (params, value_vec, false),
        ],
        vec_type(b, Type::Param(sym_t)),
        db_query_effects,
    );
    db_tx_values_query.generics = vec![sym_t];
    out.push(("db.tx_query_values".into(), db_tx_values_query));
    for name in ["commit", "rollback"] {
        out.push((
            format!("db.{name}"),
            sig(
                intern,
                &format!("std.db::fn:{name}"),
                name,
                vec![(tx, db_tx_type.clone(), false)],
                Type::unit(),
                db_effects.clone(),
            ),
        ));
    }

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
            alloc_eff.clone(),
        ),
    ));
    let bte = intern.intern("b");
    out.push((
        "str.from_byte".into(),
        sig(
            intern,
            "core::fn:str.from_byte",
            "from_byte",
            vec![
                (alloc, alloc_type(b), false),
                (bte, Type::Prim(Prim::U8), false),
            ],
            string_type(b),
            alloc_eff,
        ),
    ));
    let prefix = intern.intern("prefix");
    let needle = intern.intern("needle");
    let count = intern.intern("count");
    out.push((
        "str.starts_with".into(),
        sig(
            intern,
            "core::fn:str.starts_with",
            "starts_with",
            vec![(x, string_type(b), false), (prefix, string_type(b), false)],
            Type::bool(),
            EffectSet::new(),
        ),
    ));
    out.push((
        "str.contains".into(),
        sig(
            intern,
            "core::fn:str.contains",
            "contains",
            vec![(x, string_type(b), false), (needle, string_type(b), false)],
            Type::bool(),
            EffectSet::new(),
        ),
    ));
    out.push((
        "str.drop".into(),
        sig(
            intern,
            "core::fn:str.drop",
            "drop",
            vec![(x, string_type(b), false), (count, Type::usz(), false)],
            string_type(b),
            EffectSet::new(),
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
            net_abort.clone(),
        ),
    ));
    let request_ty = Type::Named {
        def: intern.intern("http.Request"),
        args: vec![],
    };
    let response_ty = Type::Named {
        def: intern.intern("http.Response"),
        args: vec![],
    };
    let handler_effect = intern.intern("handler_effect");
    let mut serve_handler_effects = EffectSet::new();
    serve_handler_effects.insert(EffectAtom::Var(handler_effect));
    let mut serve_effects = net_abort.clone();
    serve_effects.insert(EffectAtom::Var(handler_effect));
    out.push((
        "http.listen".into(),
        sig(
            intern,
            "core::fn:http.listen",
            "listen",
            vec![(port, Type::Prim(Prim::U16), false)],
            Type::unit(),
            net_abort.clone(),
        ),
    ));
    out.push((
        "http.accept".into(),
        sig(
            intern,
            "core::fn:http.accept",
            "accept",
            vec![],
            request_ty.clone(),
            net_abort.clone(),
        ),
    ));
    let status = intern.intern("status");
    out.push((
        "http.respond".into(),
        sig(
            intern,
            "core::fn:http.respond",
            "respond",
            vec![
                (status, Type::Prim(Prim::U16), false),
                (body, string_type(b), false),
            ],
            Type::unit(),
            net_abort.clone(),
        ),
    ));
    out.push((
        "http.close".into(),
        sig(
            intern,
            "core::fn:http.close",
            "close",
            vec![],
            Type::unit(),
            net_abort.clone(),
        ),
    ));
    let handler = intern.intern("handler");
    let body_limit = intern.intern("body_limit");
    let timeout_ms = intern.intern("timeout_ms");
    let cors_origin = intern.intern("cors_origin");
    let index = intern.intern("index");
    let name = intern.intern("name");
    out.push((
        "http.response".into(),
        sig(
            intern,
            "core::fn:http.response",
            "response",
            vec![
                (status, Type::Prim(Prim::U16), false),
                (body, string_type(b), false),
            ],
            response_ty.clone(),
            EffectSet::new(),
        ),
    ));
    out.push((
        "http.response_stream".into(),
        sig(
            intern,
            "core::fn:http.response_stream",
            "response_stream",
            vec![
                (status, Type::Prim(Prim::U16), false),
                (body, string_type(b), false),
            ],
            response_ty.clone(),
            EffectSet::new(),
        ),
    ));
    out.push((
        "http.serve_handler".into(),
        sig(
            intern,
            "core::fn:http.serve_handler",
            "serve_handler",
            vec![
                (port, Type::Prim(Prim::U16), false),
                (
                    handler,
                    Type::Fn {
                        params: vec![request_ty.clone()],
                        ret: Box::new(response_ty.clone()),
                        effects: serve_handler_effects.clone(),
                    },
                    false,
                ),
            ],
            Type::unit(),
            serve_effects.clone(),
        ),
    ));
    out.push((
        "http.serve_handler_config".into(),
        sig(
            intern,
            "core::fn:http.serve_handler_config",
            "serve_handler_config",
            vec![
                (port, Type::Prim(Prim::U16), false),
                (
                    handler,
                    Type::Fn {
                        params: vec![request_ty.clone()],
                        ret: Box::new(response_ty.clone()),
                        effects: serve_handler_effects.clone(),
                    },
                    false,
                ),
                (body_limit, Type::Prim(Prim::U32), false),
                (timeout_ms, Type::Prim(Prim::U32), false),
                (cors_origin, string_type(b), false),
            ],
            Type::unit(),
            serve_effects.clone(),
        ),
    ));
    let state = intern.intern("state");
    let mut state_handler_sig = sig(
        intern,
        "core::fn:http.serve_handler_state",
        "serve_handler_state",
        vec![
            (port, Type::Prim(Prim::U16), false),
            (state, Type::Param(sym_t), false),
            (
                handler,
                Type::Fn {
                    params: vec![Type::Param(sym_t), request_ty.clone()],
                    ret: Box::new(response_ty.clone()),
                    effects: serve_handler_effects.clone(),
                },
                false,
            ),
        ],
        Type::unit(),
        serve_effects.clone(),
    );
    state_handler_sig.generics = vec![sym_t];
    out.push(("http.serve_handler_state".into(), state_handler_sig));
    let mut state_handler_config_sig = sig(
        intern,
        "core::fn:http.serve_handler_state_config",
        "serve_handler_state_config",
        vec![
            (port, Type::Prim(Prim::U16), false),
            (state, Type::Param(sym_t), false),
            (
                handler,
                Type::Fn {
                    params: vec![Type::Param(sym_t), request_ty.clone()],
                    ret: Box::new(response_ty.clone()),
                    effects: serve_handler_effects,
                },
                false,
            ),
            (body_limit, Type::Prim(Prim::U32), false),
            (timeout_ms, Type::Prim(Prim::U32), false),
            (cors_origin, string_type(b), false),
        ],
        Type::unit(),
        serve_effects,
    );
    state_handler_config_sig.generics = vec![sym_t];
    out.push((
        "http.serve_handler_state_config".into(),
        state_handler_config_sig,
    ));
    out.push((
        "http.path_match".into(),
        sig(
            intern,
            "core::fn:http.path_match",
            "path_match",
            vec![(path, string_type(b), false), (raw, string_type(b), false)],
            Type::bool(),
            EffectSet::new(),
        ),
    ));
    out.push((
        "http.path_param".into(),
        sig(
            intern,
            "core::fn:http.path_param",
            "path_param",
            vec![
                (path, string_type(b), false),
                (raw, string_type(b), false),
                (index, Type::Prim(Prim::U16), false),
            ],
            string_type(b),
            EffectSet::new(),
        ),
    ));
    out.push((
        "http.query_param".into(),
        sig(
            intern,
            "core::fn:http.query_param",
            "query_param",
            vec![(raw, string_type(b), false), (name, string_type(b), false)],
            string_type(b),
            EffectSet::new(),
        ),
    ));
    out.push((
        "http.header".into(),
        sig(
            intern,
            "core::fn:http.header",
            "header",
            vec![(raw, string_type(b), false), (name, string_type(b), false)],
            string_type(b),
            EffectSet::new(),
        ),
    ));
    out.push((
        "http.cookie".into(),
        sig(
            intern,
            "core::fn:http.cookie",
            "cookie",
            vec![(raw, string_type(b), false), (name, string_type(b), false)],
            string_type(b),
            EffectSet::new(),
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
    let mut env_row = EffectSet::new();
    env_row.insert(EffectAtom::Io(intern.intern("env")));
    let fallback = intern.intern("fallback");
    out.push((
        "env.get_or".into(),
        sig(
            intern,
            "core::fn:env.get_or",
            "get_or",
            vec![
                (name, string_type(b), false),
                (fallback, string_type(b), false),
            ],
            Type::Untrusted(Box::new(string_type(b))),
            env_row,
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
            name: intern.intern("db.Error"),
            generics: vec![],
            kind: TypeDefKind::Variants(vec![(intern.intern("Failed"), vec![])]),
            injections: vec![],
            span: Span::DUMMY,
            def_id: "std.db::type:Error".into(),
        },
        TypeDef {
            name: intern.intern("db.Pool"),
            generics: vec![],
            kind: TypeDefKind::Record(vec![]),
            injections: vec![],
            span: Span::DUMMY,
            def_id: "std.db::type:Pool".into(),
        },
        TypeDef {
            name: intern.intern("db.Tx"),
            generics: vec![],
            kind: TypeDefKind::Record(vec![]),
            injections: vec![],
            span: Span::DUMMY,
            def_id: "std.db::type:Tx".into(),
        },
        TypeDef {
            name: intern.intern("db.Value"),
            generics: vec![],
            kind: TypeDefKind::Variants(vec![
                (intern.intern("Null"), vec![]),
                (
                    intern.intern("Text"),
                    vec![(
                        intern.intern("value"),
                        Type::Named {
                            def: intern.intern("String"),
                            args: vec![],
                        },
                    )],
                ),
                (
                    intern.intern("I64"),
                    vec![(intern.intern("value"), Type::Prim(Prim::I64))],
                ),
                (
                    intern.intern("U64"),
                    vec![(intern.intern("value"), Type::Prim(Prim::U64))],
                ),
                (
                    intern.intern("F64"),
                    vec![(intern.intern("value"), Type::Prim(Prim::F64))],
                ),
                (
                    intern.intern("Bool"),
                    vec![(intern.intern("value"), Type::Prim(Prim::Bool))],
                ),
            ]),
            injections: vec![],
            span: Span::DUMMY,
            def_id: "std.db::type:Value".into(),
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
            name: intern.intern("http.Request"),
            generics: vec![],
            kind: TypeDefKind::Record(vec![
                (
                    intern.intern("method"),
                    Type::Named {
                        def: intern.intern("String"),
                        args: vec![],
                    },
                ),
                (
                    intern.intern("path"),
                    Type::Named {
                        def: intern.intern("String"),
                        args: vec![],
                    },
                ),
                (
                    intern.intern("body"),
                    Type::Named {
                        def: intern.intern("String"),
                        args: vec![],
                    },
                ),
                (
                    intern.intern("query"),
                    Type::Named {
                        def: intern.intern("String"),
                        args: vec![],
                    },
                ),
                (
                    intern.intern("headers"),
                    Type::Named {
                        def: intern.intern("String"),
                        args: vec![],
                    },
                ),
            ]),
            injections: vec![],
            span: Span::DUMMY,
            def_id: "std.net::type:Request".into(),
        },
        TypeDef {
            name: intern.intern("http.Response"),
            generics: vec![],
            kind: TypeDefKind::Record(vec![
                (intern.intern("status"), Type::Prim(Prim::U16)),
                (
                    intern.intern("body"),
                    Type::Named {
                        def: intern.intern("String"),
                        args: vec![],
                    },
                ),
                (intern.intern("static_body"), Type::Prim(Prim::Bool)),
                (intern.intern("stream"), Type::Prim(Prim::Bool)),
            ]),
            injections: vec![],
            span: Span::DUMMY,
            def_id: "std.net::type:Response".into(),
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
