use crate::tokens::{Token, TokenType, TokenValue};
use crate::types::{FloatType, IntType, Number, Source};
use crate::utils::success;
use rslua_derive::Traceable;
use rslua_traits::Error;
use std::str;

// context for lexer
struct Context<'a> {
    buffer: &'a str,
    current: usize,
    line: usize,
    col: usize,
    old_pos: usize,
    old_line: usize,
    old_col: usize,
    offset: usize,
    comment_offset: usize,
}

impl<'a> Context<'a> {
    pub fn new(buffer: &'a str) -> Self {
        Context::<'a> {
            buffer,
            current: 0,
            line: 1,
            col: 1,
            old_pos: 0,
            old_line: 0,
            old_col: 0,
            offset: 0,
            comment_offset: 0,
        }
    }

    pub fn save(&mut self) {
        self.old_pos = self.current;
        self.old_col = self.col;
        self.old_line = self.line;
    }

    pub fn get_saved_source(&self) -> Source {
        Source {
            line: self.old_line,
            col: self.old_col,
            length: self.current - self.old_pos,
        }
    }

    pub fn next(&mut self) {
        self.skip(1);
    }

    pub fn skip(&mut self, n: usize) {
        self.current += n;
        self.col += n;
    }

    // eat n chars, and write these chars to output
    pub fn write_into(&mut self, n: usize, output: &mut Vec<u8>) {
        if let Some(slice) = self.buffer.as_bytes().get(self.current..(self.current + n)) {
            output.extend_from_slice(slice);
            self.skip(n);
        }
    }

    pub fn inc_line(&mut self) {
        self.col = 1;
        self.line += 1;
    }

    pub fn get(&self) -> Option<u8> {
        self.get_ahead(0)
    }

    pub fn get_next(&self) -> Option<u8> {
        self.get_ahead(1)
    }

    pub fn get_ahead(&self, index: usize) -> Option<u8> {
        self.buffer.as_bytes().get(self.current + index).copied()
    }
}

#[derive(Default)]
pub struct LexerConfig {
    // if use origin string, lexer won't escape special chars and keep the quotes or string boundaries.
    pub use_origin_string: bool,
    // reserve comments or not
    pub reserve_comments: bool,
}
#[derive(Traceable, Default)]
pub struct Lexer {
    config: LexerConfig,
    tokens: Vec<Token>,
}

#[derive(Debug)]
pub struct LexError(String);

impl Error for LexError {
    fn what(&self) -> &str {
        &self.0
    }
}

type LexResult = Result<Option<(TokenType, TokenValue)>, LexError>;

impl<'a> Lexer {
    pub fn set_config(&mut self, config: LexerConfig) {
        self.config = config;
    }

    pub fn run(&mut self, input: &'a str) -> Result<Vec<Token>, LexError> {
        self.reset();
        let mut ctx = Context::new(input);
        loop {
            ctx.save();
            if let Some(c) = ctx.get() {
                if let Some((token_type, token_value)) = match c {
                    _ if Lexer::is_line_break(c) => self.read_line_break(&mut ctx)?,
                    _ if Lexer::is_space(c) => self.read_space(&mut ctx)?,
                    _ if Lexer::is_digit(c) => self.read_number(&mut ctx)?,
                    b'-' if self.check_next(&ctx, '-') => self.read_comment(&mut ctx)?,
                    b'=' => self.read_eq_assign(&mut ctx)?,
                    b'<' => self.read_le_shl_lt(&mut ctx)?,
                    b'>' => self.read_ge_shr_gt(&mut ctx)?,
                    b'/' if self.check_next(&ctx, '/') => self.read_idiv(&mut ctx)?,
                    b'~' => self.read_ne_xor(&mut ctx)?,
                    b':' => self.read_colon(&mut ctx)?,
                    b'.' => self.read_attr_concat_dots_numbers(&mut ctx)?,
                    b'"' | b'\'' | b'`' => self.read_short_string(&mut ctx)?,
                    b'[' if self.check_next2(&ctx, '[', '=') => self.read_long_string(&mut ctx)?,
                    _ => self.read_other_tokens(&mut ctx)?,
                } {
                    self.add_token(&mut ctx, token_type, token_value);
                }
            } else {
                // append eos and return tokens
                self.add_token(&mut ctx, TokenType::Eos, TokenValue::None);
                return Ok(std::mem::take(&mut self.tokens));
            }
        }
    }

    pub fn tokens(&self) -> &Vec<Token> {
        &self.tokens
    }

    fn read_line_break(&self, ctx: &mut Context) -> LexResult {
        let old = ctx.get();
        ctx.next();

        // skip \r\n or \n\r
        if old != ctx.get() && self.check_current_if(ctx, Lexer::is_line_break) {
            ctx.next();
        }

        ctx.inc_line();
        Ok(None)
    }

    fn read_space(&self, ctx: &mut Context) -> LexResult {
        ctx.next();
        Ok(None)
    }

    fn read_comment(&mut self, ctx: &mut Context) -> LexResult {
        ctx.skip(2);
        let sep_count = self.try_read_long_string_boundary(ctx, b'[');
        if sep_count >= 0 {
            let comment = self.read_long_string_impl(ctx, sep_count as usize, "comment")?;
            if self.config.reserve_comments {
                success((TokenType::MComment, TokenValue::Str(comment)))
            } else {
                Ok(None)
            }
        } else {
            self.read_short_comment(ctx)
        }
    }

    fn read_short_comment(&mut self, ctx: &mut Context) -> LexResult {
        let mut bytes: Vec<u8> = Vec::new();
        while let Some(c) = ctx.get() {
            if Lexer::is_line_break(c) {
                break;
            }
            ctx.write_into(1, &mut bytes);
        }
        if let Ok(comment) = str::from_utf8(&bytes) {
            if self.config.reserve_comments {
                success((TokenType::SComment, TokenValue::Str(comment.to_string())))
            } else {
                Ok(None)
            }
        } else {
            self.lex_error(ctx, "invalid single line comment")
        }
    }

    // if next char equals c, return t1, else return t2
    fn read_token2(
        &mut self,
        ctx: &mut Context,
        c: char,
        t1: TokenType,
        t2: TokenType,
    ) -> LexResult {
        ctx.next();
        if self.check_current(ctx, c) {
            ctx.next();
            return success((t1, TokenValue::None));
        }
        success((t2, TokenValue::None))
    }

    // if next char equals c1, return t1, else if equals t2, return t2, else return t3
    fn read_token3(
        &mut self,
        ctx: &mut Context,
        c1: char,
        c2: char,
        t1: TokenType,
        t2: TokenType,
        t3: TokenType,
    ) -> LexResult {
        ctx.next();
        if self.check_current(ctx, c1) {
            ctx.next();
            success((t1, TokenValue::None))
        } else if self.check_current(ctx, c2) {
            ctx.next();
            success((t2, TokenValue::None))
        } else {
            success((t3, TokenValue::None))
        }
    }

    fn read_eq_assign(&mut self, ctx: &mut Context) -> LexResult {
        self.read_token2(ctx, '=', TokenType::Eq, TokenType::Assign)
    }

    fn read_le_shl_lt(&mut self, ctx: &mut Context) -> LexResult {
        self.read_token3(ctx, '=', '<', TokenType::Le, TokenType::Shl, TokenType::Lt)
    }

    fn read_ge_shr_gt(&mut self, ctx: &mut Context) -> LexResult {
        self.read_token3(ctx, '=', '>', TokenType::Ge, TokenType::Shr, TokenType::Gt)
    }

    fn read_idiv(&mut self, ctx: &mut Context) -> LexResult {
        ctx.skip(2);
        success((TokenType::IDiv, TokenValue::None))
    }

    fn read_ne_xor(&mut self, ctx: &mut Context) -> LexResult {
        self.read_token2(ctx, '=', TokenType::Ne, TokenType::BXor)
    }

    fn read_colon(&mut self, ctx: &mut Context) -> LexResult {
        self.read_token2(ctx, ':', TokenType::DbColon, TokenType::Colon)
    }

    fn read_attr_concat_dots_numbers(&mut self, ctx: &mut Context) -> LexResult {
        if self.check_next(ctx, '.') {
            ctx.next();
            if self.check_next(ctx, '.') {
                ctx.skip(2);
                return success((TokenType::Dots, TokenValue::None));
            } else {
                ctx.next();
                return success((TokenType::Concat, TokenValue::None));
            }
        }
        if let Some(c) = ctx.get_next() {
            if Lexer::is_digit(c) {
                return self.read_number(ctx);
            }
        }
        ctx.next();
        success((TokenType::Attr, TokenValue::None))
    }

    fn read_number(&mut self, ctx: &mut Context) -> LexResult {
        let mut expo = ('E', 'e');
        let mut num_str: Vec<u8> = Vec::new();
        let mut hex = false;
        if self.check_current(ctx, '0') && self.check_next2(ctx, 'x', 'X') {
            expo = ('P', 'p');
            ctx.write_into(2, &mut num_str);
            hex = true;
        }
        let is_digit = |c| {
            (hex && Lexer::is_hex_digit(c)) || (!hex && Lexer::is_digit(c)) || (c as char) == '.'
        };
        loop {
            if self.check_current_if(ctx, is_digit) {
                ctx.write_into(1, &mut num_str)
            } else if self.check_current2(ctx, expo.0, expo.1) {
                ctx.write_into(1, &mut num_str);
                if self.check_current2(ctx, '-', '+') {
                    ctx.write_into(1, &mut num_str)
                }
            } else {
                break;
            }
        }
        if let Ok(string) = str::from_utf8(&num_str) {
            let num = Lexer::str_to_num(string);
            match num {
                Number::Int(n) => success((TokenType::Int, TokenValue::Int(n))),
                Number::Float(n) => success((TokenType::Flt, TokenValue::Float(n))),
                _ => self.lex_error(ctx, "malformed number"),
            }
        } else {
            unreachable!();
        }
    }

    fn try_read_hexa(&mut self, ctx: &mut Context) -> Option<u8> {
        ctx.get().filter(|c| Lexer::is_hex_digit(*c)).map(|c| {
            ctx.next();
            Lexer::to_hex_digit(c)
        })
    }

    fn try_read_hex_esc(&mut self, ctx: &mut Context) -> Result<u8, LexError> {
        if let Some(p1) = self.try_read_hexa(ctx) {
            if let Some(p2) = self.try_read_hexa(ctx) {
                return Ok((p1 << 4) + p2);
            }
        }
        self.lex_error(ctx, "hexadecimal digit expected")
    }

    fn try_read_utf8_esc(
        &mut self,
        ctx: &mut Context,
        bytes: &mut Vec<u8>,
    ) -> Result<(), LexError> {
        if let Some(c) = ctx.get() {
            if c != b'{' {
                return self.lex_error(ctx, "missing '{'");
            }
            ctx.next();
            if let Some(c) = self.try_read_hexa(ctx) {
                let mut r = c as u32;
                while let Some(c) = self.try_read_hexa(ctx) {
                    r = (r << 4) + (c as u32);
                    if r > 0x10FFFF {
                        return self.lex_error(ctx, "UTF-8 value too large");
                    }
                }
                if self.check_current(ctx, '}') {
                    if let Some(c) = std::char::from_u32(r) {
                        let mut string = String::new();
                        string.push(c);
                        bytes.append(&mut string.into_bytes());
                        ctx.next();
                    } else {
                        return self.lex_error(ctx, "invalid utf8 codepoint");
                    }
                } else {
                    return self.lex_error(ctx, "missing '}'");
                }
            } else {
                return self.lex_error(ctx, "hexadecimal digit expected");
            }
        }
        Ok(())
    }

    fn try_read_dec_esc(
        &mut self,
        ctx: &mut Context,
        bytes: &mut Vec<u8>,
        first_place: u8,
    ) -> Result<(), LexError> {
        let mut r: u32 = Lexer::to_digit(first_place) as u32;
        let mut i = 0;
        while let Some(c) = ctx.get() {
            i += 1;
            if i > 2 || !Lexer::is_digit(c) {
                // three digits at most
                break;
            }
            r = r * 10 + Lexer::to_digit(c) as u32;
            ctx.next();
        }
        if r > 0xFF {
            self.lex_error(ctx, "decimal escape too large")
        } else {
            bytes.push(r as u8);
            Ok(())
        }
    }

    fn try_read_esc(&mut self, ctx: &mut Context, bytes: &mut Vec<u8>) -> Result<(), LexError> {
        ctx.next();
        if let Some(next) = ctx.get() {
            ctx.next();
            match next {
                b'n' => bytes.push(b'\n'),
                b'r' => bytes.push(b'\r'),
                b't' => bytes.push(b'\t'),
                b'a' => bytes.push(b'\x07'),
                b'b' => bytes.push(b'\x08'),
                b'v' => bytes.push(b'\x0B'),
                b'f' => bytes.push(b'\x0C'),
                b'x' => {
                    let v = self.try_read_hex_esc(ctx)?;
                    bytes.push(v);
                }
                b'u' => self.try_read_utf8_esc(ctx, bytes)?,
                b'\r' | b'\n' => {
                    bytes.push(b'\n');
                    ctx.inc_line();
                }
                b'\\' | b'"' | b'\'' => bytes.push(next),
                b'z' => {
                    // skip the following span of white-space characters, including line breaks
                    while let Some(c) = ctx.get() {
                        match c {
                            _ if Lexer::is_space(c) => ctx.next(),
                            _ if Lexer::is_line_break(c) => {
                                ctx.next();
                                ctx.inc_line();
                            }
                            _ => break,
                        }
                    }
                }
                _ if Lexer::is_digit(next) => self.try_read_dec_esc(ctx, bytes, next)?,
                _ => {
                    return self.lex_error(ctx, "invalid escape sequence");
                }
            }
        }
        Ok(())
    }

    fn read_short_string(&mut self, ctx: &mut Context) -> LexResult {
        let mut bytes: Vec<u8> = Vec::new();
        let start = ctx.get();
        if self.config.use_origin_string {
            bytes.push(start.unwrap());
        }
        ctx.next();
        let unfinished_error: &'static str = "unfinished string";
        while ctx.get() != start {
            match ctx.get() {
                Some(b'\\') if self.config.use_origin_string => ctx.write_into(2, &mut bytes),
                Some(b'\\') => self.try_read_esc(ctx, &mut bytes)?,
                Some(c) => {
                    if Lexer::is_line_break(c) {
                        return self.lex_error(ctx, unfinished_error);
                    } else {
                        bytes.push(c);
                        ctx.next();
                    }
                }
                None => return self.lex_error(ctx, unfinished_error),
            }
        }
        if self.config.use_origin_string {
            bytes.push(ctx.get().unwrap());
        }
        if let Ok(string) = String::from_utf8(bytes) {
            ctx.next();
            success((TokenType::String, TokenValue::Str(string)))
        } else {
            self.lex_error(ctx, "invalid utf8 string")
        }
    }

    // return count of '===' if a long string, otherwise return -1
    fn try_read_long_string_boundary(&self, ctx: &mut Context, sep: u8) -> i8 {
        if self.check_current(ctx, sep as char) {
            let mut sep_count = 0;
            loop {
                if let Some(c) = ctx.get_ahead(sep_count + 1) {
                    match c {
                        b'=' => sep_count += 1,
                        _ if c == sep => {
                            ctx.skip(sep_count + 2);
                            return sep_count as i8;
                        }
                        _ => break,
                    };
                }
            }
        }
        -1
    }

    // read long string
    fn read_long_string_impl(
        &mut self,
        ctx: &mut Context,
        sep_count: usize,
        sem: &str,
    ) -> Result<String, LexError> {
        let line = ctx.line;
        let mut start = 0;

        if self.config.use_origin_string {
            start = ctx.current - 2 - sep_count;
        }

        // skip first line break
        if self.check_current_if(ctx, Lexer::is_line_break) {
            self.read_line_break(ctx)?;
        }

        if !self.config.use_origin_string {
            start = ctx.current;
        }

        while let Some(c) = ctx.get() {
            match c {
                b']' => {
                    if self.try_read_long_string_boundary(ctx, b']') == sep_count as i8 {
                        let length = if self.config.use_origin_string {
                            ctx.current - start
                        } else {
                            ctx.current - 2 - sep_count - start
                        };
                        if let Some(slice) = ctx.buffer.get(start..(start + length)) {
                            return Ok(slice.to_string());
                        }
                    } else {
                        ctx.next();
                    }
                }
                _ if Lexer::is_line_break(c) => {
                    self.read_line_break(ctx)?;
                }
                _ => ctx.next(),
            }
        }
        self.lex_error(
            ctx,
            &format!("unfinished long {} (starting at line {})", sem, line),
        )
    }

    fn read_long_string(&mut self, ctx: &mut Context) -> LexResult {
        let sep_count = self.try_read_long_string_boundary(ctx, b'[');
        if sep_count >= 0 {
            let string = self.read_long_string_impl(ctx, sep_count as usize, "string")?;
            return success((TokenType::String, TokenValue::Str(string)));
        }
        unreachable!()
    }

    fn read_other_tokens(&mut self, ctx: &mut Context) -> LexResult {
        if let Some(c) = ctx.get() {
            let token_type = match c {
                b'+' => Some(TokenType::Add),
                b'-' => Some(TokenType::Minus),
                b'*' => Some(TokenType::Mul),
                b'/' => Some(TokenType::Div),
                b'%' => Some(TokenType::Mod),
                b'^' => Some(TokenType::Pow),
                b'#' => Some(TokenType::Len),
                b'&' => Some(TokenType::BAnd),
                b'|' => Some(TokenType::BOr),
                b'(' => Some(TokenType::Lp),
                b')' => Some(TokenType::Rp),
                b'[' => Some(TokenType::Ls),
                b']' => Some(TokenType::Rs),
                b'{' => Some(TokenType::Lb),
                b'}' => Some(TokenType::Rb),
                b';' => Some(TokenType::Semi),
                b',' => Some(TokenType::Comma),
                _ => None,
            };

            if let Some(t) = token_type {
                ctx.next();
                return success((t, TokenValue::None));
            } else if self.check_current_if(ctx, Lexer::is_valid_name_start) {
                let mut word: Vec<u8> = Vec::new();
                ctx.write_into(1, &mut word);
                while self.check_current_if(ctx, Lexer::is_valid_name) {
                    ctx.write_into(1, &mut word);
                }
                if let Ok(s) = str::from_utf8(&word) {
                    if let Some(t) = TokenType::from_keyword(s) {
                        return success((t, TokenValue::None));
                    } else {
                        return success((TokenType::Name, TokenValue::Str(s.to_string())));
                    }
                }
            } else {
                return self.lex_error(ctx, &format!("unknown token near {}", c as char));
            }
        }
        unreachable!()
    }

    fn reset(&mut self) {
        self.tokens.clear();
    }

    fn is_line_break(c: u8) -> bool {
        matches!(c, b'\r' | b'\n')
    }

    fn is_space(c: u8) -> bool {
        matches!(c, b' ' | b'\t' | b'\x0B' | b'\x0C')
    }

    fn is_digit(c: u8) -> bool {
        c.is_ascii_digit()
    }

    fn is_hex_digit(c: u8) -> bool {
        match c {
            b'a' | b'b' | b'c' | b'd' | b'e' | b'f' | b'A' | b'B' | b'C' | b'D' | b'E' | b'F' => {
                true
            }
            _ if Lexer::is_digit(c) => true,
            _ => false,
        }
    }

    fn is_alpha(c: u8) -> bool {
        (c as char).is_ascii_alphabetic()
    }

    fn is_valid_name_start(c: u8) -> bool {
        Lexer::is_alpha(c) || Lexer::is_digit(c) || c == b'_'
    }

    fn is_valid_name(c: u8) -> bool {
        Lexer::is_valid_name_start(c) || Lexer::is_alpha(c)
    }

    fn to_digit(c: u8) -> u8 {
        c - b'0'
    }

    fn to_hex_digit(c: u8) -> u8 {
        if c.is_ascii_digit() {
            c - b'0'
        } else {
            ((c as char).to_ascii_lowercase() as u8) - b'a' + 10
        }
    }

    fn skip_spaces(bytes: &[u8], i: usize) -> usize {
        let mut index = i;
        while index < bytes.len() && bytes[index] == b' ' {
            index += 1;
        }
        index
    }

    fn starts_with_0x(bytes: &[u8], i: usize) -> bool {
        bytes.len() > i + 2
            && bytes[i] == b'0'
            && (bytes[i + 1] == b'x' || bytes[i + 1] == b'X')
    }

    // get number sign, return (sign, index)
    fn get_sign(bytes: &[u8], i: usize) -> (IntType, usize) {
        let (mut sign, mut index) = (1, i);
        if index < bytes.len() {
            if bytes[index] == b'-' {
                index += 1;
                sign = -1;
            } else if bytes[i] == b'+' {
                index += 1;
            }
        }
        (sign, index)
    }

    pub fn str_to_int(s: &str) -> Option<IntType> {
        let bytes = s.as_bytes();
        let len = bytes.len();
        let mut r: IntType = 0;
        let mut i = 0;
        let mut empty = true;
        i = Lexer::skip_spaces(bytes, i);
        let (sign, mut i) = Lexer::get_sign(bytes, i);
        if Lexer::starts_with_0x(bytes, i) {
            i += 2;
            while i < len && Lexer::is_hex_digit(bytes[i]) {
                r = (r << 4) + (Lexer::to_hex_digit(bytes[i]) as IntType);
                i += 1;
                empty = false;
            }
        } else {
            while i < len && Lexer::is_digit(bytes[i]) {
                r = r * 10 + (Lexer::to_digit(bytes[i]) as IntType);
                i += 1;
                empty = false;
            }
        }
        i = Lexer::skip_spaces(bytes, i);
        if empty || i != len {
            None
        } else {
            Some((r as IntType) * sign)
        }
    }

    pub fn str_to_float(s: &str) -> Option<FloatType> {
        let bytes = s.as_bytes();
        let mut i = 0;
        i = Lexer::skip_spaces(bytes, i);
        if Lexer::starts_with_0x(bytes, i) {
            Lexer::str_to_hex_float(&bytes[2..])
        } else {
            match s.parse::<FloatType>() {
                Ok(f) => Some(f),
                Err(_e) => None,
            }
        }
    }

    pub fn str_to_hex_float(bytes: &[u8]) -> Option<FloatType> {
        let (sign, mut i) = Lexer::get_sign(bytes, 0);
        let mut has_dot = false;
        let mut e: IntType = 0;
        let mut r = 0.0;
        let mut empty = true;
        while i < bytes.len() {
            match bytes[i] {
                b'.' => {
                    if has_dot {
                        // can't have more than one dot.
                        return None;
                    } else {
                        has_dot = true;
                    }
                }
                _ if Lexer::is_hex_digit(bytes[i]) => {
                    r = r * 16.0 + Lexer::to_hex_digit(bytes[i]) as FloatType;
                    if has_dot {
                        e -= 1;
                    }
                    empty = false;
                }
                _ => break,
            }
            i += 1;
        }
        e *= 4;
        if i < bytes.len() && (bytes[i] == b'p' || bytes[i] == b'P') {
            i += 1;
            let (esign, mut index) = Lexer::get_sign(bytes, i);
            let mut exp_value = 0;
            let mut exp_empty = true;
            while index < bytes.len() {
                if Lexer::is_digit(bytes[index]) {
                    exp_empty = false;
                    exp_value = exp_value * 10 + Lexer::to_digit(bytes[index]) as IntType;
                } else {
                    break;
                }
                index += 1;
            }
            if exp_empty {
                return None;
            }
            e += exp_value * esign;
            i = index;
        }
        r *= (2 as FloatType).powf(e as FloatType);
        Lexer::skip_spaces(bytes, i);
        if empty || i != bytes.len() {
            None
        } else {
            Some(r * (sign as FloatType))
        }
    }

    fn str_to_num(s: &str) -> Number {
        if let Some(i) = Lexer::str_to_int(s) {
            Number::Int(i)
        } else if let Some(f) = Lexer::str_to_float(s) {
            Number::Float(f)
        } else {
            Number::None
        }
    }

    fn check(&self, src: Option<u8>, target: char) -> bool {
        match src {
            Some(c) => c as char == target,
            None => false,
        }
    }

    fn check_if(&self, src: Option<u8>, f: impl Fn(u8) -> bool) -> bool {
        match src {
            Some(c) => f(c),
            None => false,
        }
    }

    fn check_current(&self, ctx: &Context, c: char) -> bool {
        self.check(ctx.get(), c)
    }

    fn check_current2(&self, ctx: &Context, c1: char, c2: char) -> bool {
        self.check(ctx.get(), c1) || self.check(ctx.get(), c2)
    }

    fn check_current_if(&self, ctx: &Context, f: impl Fn(u8) -> bool) -> bool {
        self.check_if(ctx.get(), f)
    }

    fn check_next(&self, ctx: &Context, c: char) -> bool {
        self.check(ctx.get_next(), c)
    }

    fn check_next2(&self, ctx: &Context, c1: char, c2: char) -> bool {
        self.check(ctx.get_next(), c1) || self.check(ctx.get_next(), c2)
    }

    fn add_token(&mut self, ctx: &mut Context, t: TokenType, value: TokenValue) {
        let source = ctx.get_saved_source();
        let mut comments: Vec<Token> = Vec::new();

        if !t.is_comment() {
            for comment in &mut self.tokens[ctx.comment_offset..ctx.offset].iter() {
                comments.push(comment.clone());
            }
        }

        self.tokens.push(Token {
            t,
            value,
            source,
            comments,
        });
        ctx.offset += 1;
        if !t.is_comment() {
            ctx.comment_offset = ctx.offset;
        }
    }

    fn lex_error<T>(&self, ctx: &Context, msg: &str) -> Result<T, LexError> {
        let error_msg = format!("[lex error] {} at line [{}:{}].", msg, ctx.line, ctx.col);
        Lexer::trace_error(LexError(error_msg))
    }
}
