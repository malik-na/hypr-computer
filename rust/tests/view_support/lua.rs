//! Just enough Lua to read what a program asks Hyprland 0.57 to do: the
//! literal arguments of calls such as `hl.window_rule({...})` or
//! `hl.dsp.exec_cmd("cmd", {workspace = "10 silent"})`.

/// A Lua literal.
#[derive(Clone, Debug, PartialEq)]
pub enum Lua {
    Nil,
    Bool(bool),
    Num(f64),
    Str(String),
    /// Fields in order; `None` keys are positional.
    Table(Vec<(Option<String>, Lua)>),
}

impl Lua {
    /// The field `key` of a table.
    pub fn get(&self, key: &str) -> Option<&Lua> {
        match self {
            Lua::Table(fields) => fields
                .iter()
                .find(|(k, _)| k.as_deref() == Some(key))
                .map(|(_, v)| v),
            _ => None,
        }
    }

    /// A string, or a number written as Lua would print it.
    pub fn text(&self) -> Option<String> {
        match self {
            Lua::Str(s) => Some(s.clone()),
            Lua::Num(n) if n.fract() == 0.0 => Some(format!("{}", *n as i64)),
            Lua::Num(n) => Some(n.to_string()),
            _ => None,
        }
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Lua::Bool(b) => Some(*b),
            _ => None,
        }
    }

    pub fn fields(&self) -> &[(Option<String>, Lua)] {
        match self {
            Lua::Table(fields) => fields,
            _ => &[],
        }
    }
}

/// Every call of `function` in `code`, with its arguments, e.g. every
/// `hl.window_rule(...)`. Calls whose arguments aren't literals are skipped.
pub fn calls(code: &str, function: &str) -> Vec<Vec<Lua>> {
    let needle = format!("{function}(");
    let mut found = Vec::new();
    let mut from = 0;
    while let Some(at) = code[from..].find(&needle) {
        let start = from + at + needle.len();
        // `hl.dsp.focus(` must not match inside `xhl.dsp.focus(`.
        let before = code[..from + at].chars().next_back();
        let standalone = !before.is_some_and(|c| c.is_alphanumeric() || c == '_');
        if standalone && let Some(args) = arguments(&code[start..]) {
            found.push(args);
        }
        from = start;
    }
    found
}

/// The literal arguments of a call, given the text after its `(`.
pub fn arguments(text: &str) -> Option<Vec<Lua>> {
    let mut p = Parser {
        s: text.as_bytes(),
        i: 0,
    };
    let mut args = Vec::new();
    p.ws();
    if p.eat(b')') {
        return Some(args);
    }
    loop {
        args.push(p.value()?);
        p.ws();
        if p.eat(b')') {
            return Some(args);
        }
        if !p.eat(b',') {
            return None;
        }
    }
}

/// One Lua literal, such as a table.
pub fn value(text: &str) -> Option<Lua> {
    Parser {
        s: text.as_bytes(),
        i: 0,
    }
    .value()
}

struct Parser<'a> {
    s: &'a [u8],
    i: usize,
}

impl Parser<'_> {
    fn peek(&self) -> Option<u8> {
        self.s.get(self.i).copied()
    }

    fn eat(&mut self, c: u8) -> bool {
        if self.peek() == Some(c) {
            self.i += 1;
            true
        } else {
            false
        }
    }

    /// Skips white space and `--` comments.
    fn ws(&mut self) {
        loop {
            while self.peek().is_some_and(|c| c.is_ascii_whitespace()) {
                self.i += 1;
            }
            if self.s[self.i..].starts_with(b"--") {
                while self.peek().is_some_and(|c| c != b'\n') {
                    self.i += 1;
                }
            } else {
                return;
            }
        }
    }

    fn value(&mut self) -> Option<Lua> {
        self.ws();
        match self.peek()? {
            b'"' | b'\'' => self.string().map(Lua::Str),
            b'[' if self.s[self.i..].starts_with(b"[[") => self.long_string().map(Lua::Str),
            b'{' => self.table(),
            c if c.is_ascii_digit() || c == b'-' || c == b'.' => self.number(),
            c if c.is_ascii_alphabetic() || c == b'_' => match self.ident().as_str() {
                "true" => Some(Lua::Bool(true)),
                "false" => Some(Lua::Bool(false)),
                "nil" => Some(Lua::Nil),
                _ => None,
            },
            _ => None,
        }
    }

    fn ident(&mut self) -> String {
        let start = self.i;
        while self
            .peek()
            .is_some_and(|c| c.is_ascii_alphanumeric() || c == b'_')
        {
            self.i += 1;
        }
        String::from_utf8_lossy(&self.s[start..self.i]).into_owned()
    }

    fn number(&mut self) -> Option<Lua> {
        let start = self.i;
        if self.peek() == Some(b'-') {
            self.i += 1;
        }
        while self
            .peek()
            .is_some_and(|c| c.is_ascii_alphanumeric() || c == b'.')
        {
            self.i += 1;
        }
        let text = std::str::from_utf8(&self.s[start..self.i]).ok()?;
        if let Some(hex) = text.strip_prefix("0x") {
            return i64::from_str_radix(hex, 16)
                .ok()
                .map(|n| Lua::Num(n as f64));
        }
        text.parse().ok().map(Lua::Num)
    }

    fn long_string(&mut self) -> Option<String> {
        self.i += 2;
        let start = self.i;
        while !self.s[self.i..].starts_with(b"]]") {
            self.peek()?;
            self.i += 1;
        }
        let text = String::from_utf8_lossy(&self.s[start..self.i]).into_owned();
        self.i += 2;
        Some(text)
    }

    fn string(&mut self) -> Option<String> {
        let quote = self.peek()?;
        self.i += 1;
        let mut out = Vec::new();
        loop {
            let c = self.peek()?;
            self.i += 1;
            if c == quote {
                return Some(String::from_utf8_lossy(&out).into_owned());
            }
            if c != b'\\' {
                out.push(c);
                continue;
            }
            let e = self.peek()?;
            self.i += 1;
            match e {
                b'n' => out.push(b'\n'),
                b't' => out.push(b'\t'),
                b'r' => out.push(b'\r'),
                b'0'..=b'9' => {
                    let mut n = u32::from(e - b'0');
                    for _ in 0..2 {
                        match self.peek() {
                            Some(d @ b'0'..=b'9') => {
                                n = n * 10 + u32::from(d - b'0');
                                self.i += 1;
                            }
                            _ => break,
                        }
                    }
                    out.push(u8::try_from(n).ok()?);
                }
                b'x' => {
                    let hex = std::str::from_utf8(self.s.get(self.i..self.i + 2)?).ok()?;
                    out.push(u8::from_str_radix(hex, 16).ok()?);
                    self.i += 2;
                }
                other => out.push(other),
            }
        }
    }

    fn table(&mut self) -> Option<Lua> {
        self.i += 1; // {
        let mut fields = Vec::new();
        loop {
            self.ws();
            if self.eat(b'}') {
                return Some(Lua::Table(fields));
            }
            let key = if self.peek() == Some(b'[') && !self.s[self.i..].starts_with(b"[[") {
                self.i += 1;
                let key = self.value()?.text()?;
                self.ws();
                if !self.eat(b']') {
                    return None;
                }
                self.ws();
                if !self.eat(b'=') {
                    return None;
                }
                Some(key)
            } else if self
                .peek()
                .is_some_and(|c| c.is_ascii_alphabetic() || c == b'_')
            {
                let save = self.i;
                let name = self.ident();
                self.ws();
                if self.peek() == Some(b'=') && self.s.get(self.i + 1) != Some(&b'=') {
                    self.i += 1;
                    Some(name)
                } else {
                    self.i = save;
                    None
                }
            } else {
                None
            };
            let value = self.value()?;
            fields.push((key, value));
            self.ws();
            if !self.eat(b',') && !self.eat(b';') {
                self.ws();
                if !self.eat(b'}') {
                    return None;
                }
                return Some(Lua::Table(fields));
            }
        }
    }
}
