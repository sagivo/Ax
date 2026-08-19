//! GBNF export and parser-equivalence (spec v0.3 §1.4, §8.4).
//!
//! The fragment is a grammar for what is syntactically legal at a cursor.
//! Under constrained decoding, a syntax error and a hallucinated symbol are
//! unrepresentable. CI verifies: every string the grammar accepts parses, and
//! every string the parser accepts is generated (sampled both directions).

/// Full-file GBNF for the conventional Ax surface (v0.2 + v0.3 additions).
pub fn file_gbnf() -> &'static str {
    concat!(
        "root ::= file\n",
        "file ::= ws module-opt export-opt use-decl* decl*\n",
        "module-opt ::= (\"module\" ws path ws \";\" ws)?\n",
        "export-opt ::= (\"export\" ws \"{\" ws ident-list ws \"}\" ws \";\" ws)?\n",
        "use-decl ::= \"use\" ws path ws ( \"as\" ws ident ws )? \";\" ws\n",
        "decl ::= attr* vis-opt \"fn\" ws ident ws generics-opt \"(\" ws params-opt \")\" ws \"->\" ws type-e ws body\n",
        "attr ::= (\"@\" ident (ws atom)? ws) | (\"#[\" ident (\"(\" atom \")\")? \"]\" ws)\n",
        "vis-opt ::= (\"pub\" ws)?\n",
        "generics-opt ::= (\"[\" gparam (\",\" ws gparam)* \"]\" ws)?\n",
        "gparam ::= ident (\":\" ws type-e)?\n",
        "params-opt ::= (param (\",\" ws param)*)?\n",
        "param ::= ident \":\" ws type-e\n",
        "body ::= \"=\" ws expr \";\" | \"{\" ws block-inner \"}\"\n",
        "block-inner ::= (stmt)* expr?\n",
        "stmt ::= \"let\" ws \"mut\"? ws ident (\":\" ws type-e)? \"=\" ws expr \";\" ws\n",
        "type-e ::= \"own\" ws type-e | \"&\" \"mut\"? ws type-e | named-type | prim\n",
        "named-type ::= ident type-args-opt\n",
        "type-args-opt ::= (\"[\" type-e (\",\" ws type-e)* \"]\")?\n",
        "prim ::= \"i8\"|\"i16\"|\"i32\"|\"i64\"|\"isz\"|\"u8\"|\"u16\"|\"u32\"|\"u64\"|\"usz\"|\"f32\"|\"f64\"|\"bool\"|\"byte\"|\"unit\"|\"String\"\n",
        "expr ::= or-e\n",
        "or-e ::= and-e (\"||\" and-e)*\n",
        "and-e ::= cmp-e (\"&&\" cmp-e)*\n",
        "cmp-e ::= add-e ((\"==\"|\"!=\"|\"<\"|\"<=\"|\">\"|\">=\") add-e)?\n",
        "add-e ::= mul-e ((\"+\"|\"-\") mul-e)*\n",
        "mul-e ::= unary ((\"*\"|\"/\"|\"%\") unary)*\n",
        "unary ::= (\"!\"|\"-\"|\"&\"|\"*\") unary | postfix\n",
        "postfix ::= primary (\"(\" args? \")\" | \"[\" expr \"]\" | \".\" ident | \"?\")*\n",
        "primary ::= ident | integer | float | string | fstring | \"true\" | \"false\" | \"(\" expr \")\" | \"{\" block-inner \"}\"\n",
        "args ::= expr (\",\" ws expr)*\n",
        "ident-list ::= ident (\",\" ws ident)*\n",
        "path ::= ident (\".\" ident | \"::\" ident)*\n",
        "ident ::= [A-Za-z_] [A-Za-z0-9_]*\n",
        "integer ::= [0-9] [0-9_]*\n",
        "float ::= [0-9]+ \".\" [0-9]+\n",
        "string ::= \"\\\"\" [^\\\"]* \"\\\"\"\n",
        "fstring ::= \"f\\\"\" [^\\\"]* \"\\\"\"\n",
        "atom ::= ident | integer | string\n",
        "ws ::= [ \\t\\n\\r]*\n",
    )
}

/// Cursor-local fragment: identifiers that are type-correct here, plus the
/// syntactic wrapper. Used by `ax complete`.
pub fn fragment_at(idents: &[String]) -> String {
    let alts = if idents.is_empty() {
        "[A-Za-z_] [A-Za-z0-9_]*".into()
    } else {
        idents
            .iter()
            .map(|s| format!("\"{}\"", s))
            .collect::<Vec<_>>()
            .join(" | ")
    };
    format!("root ::= ident\nident ::= {alts}\n")
}
