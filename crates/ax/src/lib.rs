//! Ax — systems language for AI agents.
//!
//! Research-v1 kernel: parser, type/effect/region checker, oracle interpreter,
//! structured diagnostics, holes, and the compiler protocol.

pub mod agent;
pub mod axmock;
pub mod ast;
pub mod backend_c;
pub mod backend_clif;
pub mod bench;
pub mod builtins;
pub mod caps;
pub mod check;
pub mod codegen;
pub mod conform;
pub mod daemon;
pub mod evalloop;
pub mod diag;
pub mod driver;
pub mod effects;
pub mod fmt;
pub mod frontend;
pub mod fuzz;
pub mod gbnf;
pub mod harvest;
pub mod hash;
pub mod indep;
pub mod intern;
pub mod interp;
pub mod ir;
pub mod lexer;
pub mod ownership;
pub mod perf;
pub mod libm;
pub mod lower;
pub mod packages;
pub mod parser;
pub mod reach;
pub mod silent;
pub mod software;
pub mod translate;
pub mod span;
pub mod testharness;
pub mod tokens;
pub mod tree;
pub mod types;
pub mod usecases;
pub mod workspace;

pub use driver::{check_report, render_diags, Session};
pub use intern::Interner;
pub use span::SourceMap;
