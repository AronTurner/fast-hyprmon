pub enum Json {
    Null,
    Bool(bool),
    Num(f64),
    Str(String),
    Arr(Vec<Json>),
    Obj(Vec<(String, Json)>),
}

impl Json {
    pub fn get(&self, key: &str) -> Option<&Json> {
        match self {
            Json::Obj(kv) => kv.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    pub fn num(&self, key: &str) -> f64 {
        match self.get(key) {
            Some(Json::Num(n)) => *n,
            _ => 0.0,
        }
    }

    pub fn str(&self, key: &str) -> String {
        match self.get(key) {
            Some(Json::Str(s)) => s.clone(),
            _ => String::new(),
        }
    }

    pub fn bool(&self, key: &str) -> bool {
        matches!(self.get(key), Some(Json::Bool(true)))
    }

    pub fn arr(&self, key: &str) -> &[Json] {
        match self.get(key) {
            Some(Json::Arr(a)) => a,
            _ => &[],
        }
    }
}

pub fn parse(s: &str) -> Result<Json, String> {
    let mut p = Parser { b: s.as_bytes(), i: 0 };
    let v = p.value()?;
    p.ws();
    if p.i != p.b.len() {
        return Err(p.err());
    }
    Ok(v)
}

struct Parser<'a> {
    b: &'a [u8],
    i: usize,
}

impl Parser<'_> {
    fn err(&self) -> String {
        format!("invalid JSON at byte {}", self.i)
    }

    fn ws(&mut self) {
        while self.b.get(self.i).is_some_and(u8::is_ascii_whitespace) {
            self.i += 1;
        }
    }

    fn eat(&mut self, c: u8) -> bool {
        self.ws();
        let hit = self.b.get(self.i) == Some(&c);
        if hit {
            self.i += 1;
        }
        hit
    }

    fn lit(&mut self, word: &str, v: Json) -> Result<Json, String> {
        if self.b[self.i..].starts_with(word.as_bytes()) {
            self.i += word.len();
            Ok(v)
        } else {
            Err(self.err())
        }
    }

    fn value(&mut self) -> Result<Json, String> {
        self.ws();
        match self.b.get(self.i) {
            Some(b'{') => {
                self.i += 1;
                let mut kv = Vec::new();
                if self.eat(b'}') {
                    return Ok(Json::Obj(kv));
                }
                loop {
                    self.ws();
                    let k = self.string()?;
                    if !self.eat(b':') {
                        return Err(self.err());
                    }
                    kv.push((k, self.value()?));
                    if self.eat(b'}') {
                        return Ok(Json::Obj(kv));
                    }
                    if !self.eat(b',') {
                        return Err(self.err());
                    }
                }
            }
            Some(b'[') => {
                self.i += 1;
                let mut a = Vec::new();
                if self.eat(b']') {
                    return Ok(Json::Arr(a));
                }
                loop {
                    a.push(self.value()?);
                    if self.eat(b']') {
                        return Ok(Json::Arr(a));
                    }
                    if !self.eat(b',') {
                        return Err(self.err());
                    }
                }
            }
            Some(b'"') => self.string().map(Json::Str),
            Some(b't') => self.lit("true", Json::Bool(true)),
            Some(b'f') => self.lit("false", Json::Bool(false)),
            Some(b'n') => self.lit("null", Json::Null),
            Some(_) => {
                let start = self.i;
                while self
                    .b
                    .get(self.i)
                    .is_some_and(|c| c.is_ascii_digit() || b"+-.eE".contains(c))
                {
                    self.i += 1;
                }
                std::str::from_utf8(&self.b[start..self.i])
                    .ok()
                    .and_then(|s| s.parse().ok())
                    .map(Json::Num)
                    .ok_or_else(|| self.err())
            }
            None => Err(self.err()),
        }
    }

    fn hex4(&mut self) -> Result<u16, String> {
        let h = self.b.get(self.i..self.i + 4).ok_or_else(|| self.err())?;
        let v = std::str::from_utf8(h)
            .ok()
            .and_then(|h| u16::from_str_radix(h, 16).ok())
            .ok_or_else(|| self.err())?;
        self.i += 4;
        Ok(v)
    }

    fn string(&mut self) -> Result<String, String> {
        if self.b.get(self.i) != Some(&b'"') {
            return Err(self.err());
        }
        self.i += 1;
        let mut out = Vec::new();
        loop {
            let c = *self.b.get(self.i).ok_or_else(|| self.err())?;
            self.i += 1;
            match c {
                b'"' => return String::from_utf8(out).map_err(|_| self.err()),
                b'\\' => {
                    let e = *self.b.get(self.i).ok_or_else(|| self.err())?;
                    self.i += 1;
                    let ch = match e {
                        b'n' => '\n',
                        b't' => '\t',
                        b'r' => '\r',
                        b'b' => '\u{8}',
                        b'f' => '\u{c}',
                        b'u' => {
                            let mut units = vec![self.hex4()?];
                            if (0xD800..0xDC00).contains(&units[0])
                                && self.b[self.i..].starts_with(b"\\u")
                            {
                                self.i += 2;
                                units.push(self.hex4()?);
                            }
                            char::decode_utf16(units)
                                .next()
                                .and_then(Result::ok)
                                .unwrap_or('\u{FFFD}')
                        }
                        other => other as char,
                    };
                    out.extend_from_slice(ch.encode_utf8(&mut [0; 4]).as_bytes());
                }
                _ => out.push(c),
            }
        }
    }
}
