//! Hand-written lexer. Zero-copy tokens over interned source.

use crate::intern::{Interner, Symbol};
use crate::span::{FileId, Span};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TokenKind {
    // Keywords
    Module,
    Export,
    Use,
    As,
    Fn,
    Type,
    Dict,
    Test,
    Contract,
    Let,
    Mut,
    If,
    Else,
    Match,
    For,
    In,
    Loop,
    Return,
    Raise,
    Catch,
    Attempt,
    Region,
    Par,
    While,
    Break,
    Continue,
    With,
    From,
    Pre,
    Post,
    Inv,
    Own,
    Default,
    True,
    False,
    // Punct
    LParen,
    RParen,
    LBrace,
    RBrace,
    LBracket,
    RBracket,
    Comma,
    Semi,
    Colon,
    ColonColon,
    Hash,
    Dot,
    Eq,
    Arrow,
    FatArrow,
    Bang,
    Question,
    At,
    Pipe,
    Amp,
    // Operators
    Plus,
    Minus,
    Star,
    Slash,
    Percent,
    EqEq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    AndAnd,
    OrOr,
    Caret,
    Tilde,
    Shl,
    Shr,
    BangLBrace, // !{
    // Literals / names
    Ident,
    Integer,
    Float,
    String,
    /// `f"…"` interpolation opener. The lexer yields this then a sequence of
    /// `String` / interpolated tokens reconstructed by the parser.
    FString,
    Eof,
}

#[derive(Clone, Copy, Debug)]
pub struct Token {
    pub kind: TokenKind,
    pub span: Span,
    pub symbol: Symbol,
}

pub struct Lexer<'a> {
    src: &'a str,
    bytes: &'a [u8],
    file: FileId,
    pos: usize,
    intern: &'a mut Interner,
}

impl<'a> Lexer<'a> {
    pub fn new(src: &'a str, file: FileId, intern: &'a mut Interner) -> Self {
        Self {
            src,
            bytes: src.as_bytes(),
            file,
            pos: 0,
            intern,
        }
    }

    pub fn tokenize(&mut self) -> Result<Vec<Token>, LexError> {
        let mut out = Vec::with_capacity(self.src.len() / 4 + 8);
        loop {
            let t = self.next_token()?;
            let eof = t.kind == TokenKind::Eof;
            out.push(t);
            if eof {
                break;
            }
        }
        Ok(out)
    }

    fn next_token(&mut self) -> Result<Token, LexError> {
        self.skip_ws_and_comments();
        let start = self.pos;
        if self.pos >= self.bytes.len() {
            return Ok(self.mk(TokenKind::Eof, start, start, ""));
        }
        let b = self.bytes[self.pos];
        match b {
            b'(' => {
                self.pos += 1;
                Ok(self.mk(TokenKind::LParen, start, self.pos, ""))
            }
            b')' => {
                self.pos += 1;
                Ok(self.mk(TokenKind::RParen, start, self.pos, ""))
            }
            b'{' => {
                self.pos += 1;
                Ok(self.mk(TokenKind::LBrace, start, self.pos, ""))
            }
            b'}' => {
                self.pos += 1;
                Ok(self.mk(TokenKind::RBrace, start, self.pos, ""))
            }
            b'[' => {
                self.pos += 1;
                Ok(self.mk(TokenKind::LBracket, start, self.pos, ""))
            }
            b']' => {
                self.pos += 1;
                Ok(self.mk(TokenKind::RBracket, start, self.pos, ""))
            }
            b',' => {
                self.pos += 1;
                Ok(self.mk(TokenKind::Comma, start, self.pos, ""))
            }
            b';' => {
                self.pos += 1;
                Ok(self.mk(TokenKind::Semi, start, self.pos, ""))
            }
            b':' => {
                self.pos += 1;
                if self.peek() == Some(b':') {
                    self.pos += 1;
                    Ok(self.mk(TokenKind::ColonColon, start, self.pos, ""))
                } else {
                    Ok(self.mk(TokenKind::Colon, start, self.pos, ""))
                }
            }
            b'#' => {
                self.pos += 1;
                Ok(self.mk(TokenKind::Hash, start, self.pos, ""))
            }
            b'.' => {
                self.pos += 1;
                Ok(self.mk(TokenKind::Dot, start, self.pos, ""))
            }
            b'?' => {
                self.pos += 1;
                Ok(self.mk(TokenKind::Question, start, self.pos, ""))
            }
            b'@' => {
                self.pos += 1;
                Ok(self.mk(TokenKind::At, start, self.pos, ""))
            }
            b'|' => {
                self.pos += 1;
                if self.peek() == Some(b'|') {
                    self.pos += 1;
                    Ok(self.mk(TokenKind::OrOr, start, self.pos, ""))
                } else {
                    Ok(self.mk(TokenKind::Pipe, start, self.pos, ""))
                }
            }
            b'&' => {
                self.pos += 1;
                if self.peek() == Some(b'&') {
                    self.pos += 1;
                    Ok(self.mk(TokenKind::AndAnd, start, self.pos, ""))
                } else {
                    Ok(self.mk(TokenKind::Amp, start, self.pos, ""))
                }
            }
            b'^' => {
                self.pos += 1;
                Ok(self.mk(TokenKind::Caret, start, self.pos, ""))
            }
            b'~' => {
                self.pos += 1;
                Ok(self.mk(TokenKind::Tilde, start, self.pos, ""))
            }
            b'+' => {
                self.pos += 1;
                Ok(self.mk(TokenKind::Plus, start, self.pos, ""))
            }
            b'*' => {
                self.pos += 1;
                Ok(self.mk(TokenKind::Star, start, self.pos, ""))
            }
            b'%' => {
                self.pos += 1;
                Ok(self.mk(TokenKind::Percent, start, self.pos, ""))
            }
            b'-' => {
                self.pos += 1;
                if self.peek() == Some(b'>') {
                    self.pos += 1;
                    Ok(self.mk(TokenKind::Arrow, start, self.pos, ""))
                } else {
                    Ok(self.mk(TokenKind::Minus, start, self.pos, ""))
                }
            }
            b'=' => {
                self.pos += 1;
                match self.peek() {
                    Some(b'=') => {
                        self.pos += 1;
                        Ok(self.mk(TokenKind::EqEq, start, self.pos, ""))
                    }
                    Some(b'>') => {
                        self.pos += 1;
                        Ok(self.mk(TokenKind::FatArrow, start, self.pos, ""))
                    }
                    _ => Ok(self.mk(TokenKind::Eq, start, self.pos, "")),
                }
            }
            b'!' => {
                self.pos += 1;
                match self.peek() {
                    Some(b'=') => {
                        self.pos += 1;
                        Ok(self.mk(TokenKind::Ne, start, self.pos, ""))
                    }
                    Some(b'{') => {
                        self.pos += 1;
                        Ok(self.mk(TokenKind::BangLBrace, start, self.pos, ""))
                    }
                    _ => Ok(self.mk(TokenKind::Bang, start, self.pos, "")),
                }
            }
            b'<' => {
                self.pos += 1;
                match self.peek() {
                    Some(b'=') => {
                        self.pos += 1;
                        Ok(self.mk(TokenKind::Le, start, self.pos, ""))
                    }
                    Some(b'<') => {
                        self.pos += 1;
                        Ok(self.mk(TokenKind::Shl, start, self.pos, ""))
                    }
                    _ => Ok(self.mk(TokenKind::Lt, start, self.pos, "")),
                }
            }
            b'>' => {
                self.pos += 1;
                match self.peek() {
                    Some(b'=') => {
                        self.pos += 1;
                        Ok(self.mk(TokenKind::Ge, start, self.pos, ""))
                    }
                    // Note: generic type arguments use `[T]`, not `<T>`, so `>>`
                    // is always a shift and never a pair of closing brackets.
                    Some(b'>') => {
                        self.pos += 1;
                        Ok(self.mk(TokenKind::Shr, start, self.pos, ""))
                    }
                    _ => Ok(self.mk(TokenKind::Gt, start, self.pos, "")),
                }
            }
            b'/' => {
                // comments already skipped; this is division
                self.pos += 1;
                Ok(self.mk(TokenKind::Slash, start, self.pos, ""))
            }
            b'"' => self.lex_string(start, b'"'),
            b'`' => self.lex_string(start, b'`'),
            b'f' if self.peek_at(1) == Some(b'"') => {
                // `f"…"` — consume the `f` and lex the string; the parser
                // sees FString and splits interpolations.
                self.pos += 1;
                let mut t = self.lex_string(start, b'"')?;
                t.kind = TokenKind::FString;
                Ok(t)
            }
            b'0'..=b'9' => self.lex_number(start),
            b'A'..=b'Z' | b'a'..=b'z' | b'_' => self.lex_ident(start),
            _ => Err(LexError {
                span: Span::new(self.file, start as u32, (start + 1) as u32),
                msg: format!("unexpected character {:?}", b as char),
            }),
        }
    }

    fn lex_ident(&mut self, start: usize) -> Result<Token, LexError> {
        while let Some(b) = self.peek() {
            if b.is_ascii_alphanumeric() || b == b'_' {
                self.pos += 1;
            } else {
                break;
            }
        }
        let text = &self.src[start..self.pos];
        let kind = keyword(text);
        Ok(self.mk(kind, start, self.pos, text))
    }

    fn lex_number(&mut self, start: usize) -> Result<Token, LexError> {
        // Radix prefixes, as in Go and Rust: `0x1f`, `0o17`, `0b1010`. A radix
        // literal is always an integer.
        // `self.pos` is still on the first digit here.
        if self.bytes[start] == b'0' {
            if let Some(c) = self.peek_at(1) {
                if matches!(c, b'x' | b'X' | b'o' | b'O' | b'b' | b'B') {
                    self.pos += 2;
                    let digits = |b: u8| b.is_ascii_alphanumeric() || b == b'_';
                    while matches!(self.peek(), Some(b) if digits(b)) {
                        self.pos += 1;
                    }
                    let text = &self.src[start..self.pos];
                    let sym = self.intern.intern(text);
                    return Ok(Token {
                        kind: TokenKind::Integer,
                        span: Span {
                            file: self.file,
                            start: start as u32,
                            end: self.pos as u32,
                        },
                        symbol: sym,
                    });
                }
            }
        }
        // `_` is a digit separator: `1_000_000`.
        while matches!(self.peek(), Some(b'0'..=b'9' | b'_')) {
            self.pos += 1;
        }
        let mut is_float = false;
        if self.peek() == Some(b'.') {
            if matches!(self.peek_at(1), Some(b'0'..=b'9')) {
                is_float = true;
                self.pos += 1;
                while matches!(self.peek(), Some(b'0'..=b'9' | b'_')) {
                    self.pos += 1;
                }
            }
        }
        // Exponent: `1e9`, `1.5e-3`. Only consumed when digits actually follow,
        // so `1e` stays an integer followed by an identifier.
        if matches!(self.peek(), Some(b'e' | b'E')) {
            let save = self.pos;
            self.pos += 1;
            if matches!(self.peek(), Some(b'+' | b'-')) {
                self.pos += 1;
            }
            if matches!(self.peek(), Some(b'0'..=b'9')) {
                is_float = true;
                while matches!(self.peek(), Some(b'0'..=b'9' | b'_')) {
                    self.pos += 1;
                }
            } else {
                self.pos = save;
            }
        }
        // optional suffix
        if matches!(self.peek(), Some(b'i' | b'u' | b'f')) {
            let s = self.pos;
            self.pos += 1;
            while matches!(self.peek(), Some(b'0'..=b'9' | b's' | b'z')) {
                self.pos += 1;
            }
            let suf = &self.src[s..self.pos];
            if !is_valid_suffix(suf) {
                self.pos = s; // leave suffix as ident
            }
        }
        let text = &self.src[start..self.pos];
        let kind = if is_float {
            TokenKind::Float
        } else {
            TokenKind::Integer
        };
        Ok(self.mk(kind, start, self.pos, text))
    }

    fn lex_string(&mut self, start: usize, quote: u8) -> Result<Token, LexError> {
        self.pos += 1; // opening quote
        let mut buf = String::new();
        while self.pos < self.bytes.len() {
            let b = self.bytes[self.pos];
            if b == quote {
                self.pos += 1;
                return Ok(self.mk(TokenKind::String, start, self.pos, &buf));
            }
            if b == b'\\' && quote == b'"' {
                self.pos += 1;
                if self.pos >= self.bytes.len() {
                    break;
                }
                match self.bytes[self.pos] {
                    b'n' => buf.push('\n'),
                    b't' => buf.push('\t'),
                    b'r' => buf.push('\r'),
                    b'\\' => buf.push('\\'),
                    b'"' => buf.push('"'),
                    b'`' => buf.push('`'),
                    b'0' => buf.push('\0'),
                    b'x' => {
                        self.pos += 1;
                        let h1 = self.peek().ok_or_else(|| self.unterm(start))?;
                        self.pos += 1;
                        let h2 = self.peek().ok_or_else(|| self.unterm(start))?;
                        let hex = [h1, h2];
                        let s = std::str::from_utf8(&hex).unwrap_or("00");
                        let v = u8::from_str_radix(s, 16).unwrap_or(0);
                        buf.push(v as char);
                        self.pos += 1;
                        continue;
                    }
                    c => buf.push(c as char),
                }
                self.pos += 1;
                continue;
            }
            // raw string / regular: take utf8 char
            let ch = self.src[self.pos..].chars().next().unwrap();
            buf.push(ch);
            self.pos += ch.len_utf8();
        }
        Err(self.unterm(start))
    }

    fn unterm(&self, start: usize) -> LexError {
        LexError {
            span: Span::new(self.file, start as u32, self.pos as u32),
            msg: "unterminated string".into(),
        }
    }

    fn skip_ws_and_comments(&mut self) {
        loop {
            while matches!(self.peek(), Some(b' ' | b'\t' | b'\n' | b'\r')) {
                self.pos += 1;
            }
            if self.peek() == Some(b'/') && self.peek_at(1) == Some(b'/') {
                self.pos += 2;
                while let Some(b) = self.peek() {
                    self.pos += 1;
                    if b == b'\n' {
                        break;
                    }
                }
                continue;
            }
            if self.peek() == Some(b'/') && self.peek_at(1) == Some(b'*') {
                self.pos += 2;
                while self.pos + 1 < self.bytes.len() {
                    if self.bytes[self.pos] == b'*' && self.bytes[self.pos + 1] == b'/' {
                        self.pos += 2;
                        break;
                    }
                    self.pos += 1;
                }
                continue;
            }
            break;
        }
    }

    #[inline]
    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.pos).copied()
    }

    #[inline]
    fn peek_at(&self, n: usize) -> Option<u8> {
        self.bytes.get(self.pos + n).copied()
    }

    fn mk(&mut self, kind: TokenKind, start: usize, end: usize, text: &str) -> Token {
        let symbol = if text.is_empty() {
            Symbol(0)
        } else {
            self.intern.intern(text)
        };
        Token {
            kind,
            span: Span::new(self.file, start as u32, end as u32),
            symbol,
        }
    }
}

fn keyword(s: &str) -> TokenKind {
    match s {
        "module" => TokenKind::Module,
        "export" => TokenKind::Export,
        "use" => TokenKind::Use,
        "as" => TokenKind::As,
        "fn" => TokenKind::Fn,
        "type" => TokenKind::Type,
        "dict" => TokenKind::Dict,
        "test" => TokenKind::Test,
        "contract" => TokenKind::Contract,
        "let" => TokenKind::Let,
        "mut" => TokenKind::Mut,
        "if" => TokenKind::If,
        "else" => TokenKind::Else,
        "match" => TokenKind::Match,
        "for" => TokenKind::For,
        "in" => TokenKind::In,
        "loop" => TokenKind::Loop,
        "while" => TokenKind::While,
        "break" => TokenKind::Break,
        "continue" => TokenKind::Continue,
        "return" => TokenKind::Return,
        "raise" => TokenKind::Raise,
        "catch" => TokenKind::Catch,
        "attempt" => TokenKind::Attempt,
        "region" => TokenKind::Region,
        "par" => TokenKind::Par,
        "with" => TokenKind::With,
        "from" => TokenKind::From,
        "pre" => TokenKind::Pre,
        "post" => TokenKind::Post,
        "inv" => TokenKind::Inv,
        "own" => TokenKind::Own,
        "default" => TokenKind::Default,
        "true" => TokenKind::True,
        "false" => TokenKind::False,
        _ => TokenKind::Ident,
    }
}

fn is_valid_suffix(s: &str) -> bool {
    matches!(
        s,
        "i8" | "i16" | "i32" | "i64" | "isz" | "u8" | "u16" | "u32" | "u64" | "usz" | "f32" | "f64"
    )
}

#[derive(Clone, Debug)]
pub struct LexError {
    pub span: Span,
    pub msg: String,
}
