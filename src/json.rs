use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq)]
pub enum JsonValue {
    Null,
    Bool(bool),
    Number(u64),
    String(String),
    Array(Vec<JsonValue>),
    Object(BTreeMap<String, JsonValue>),
}

impl JsonValue {
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Self::String(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_u64(&self) -> Option<u64> {
        match self {
            Self::Number(n) => Some(*n),
            _ => None,
        }
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Self::Bool(b) => Some(*b),
            _ => None,
        }
    }

    pub fn as_array(&self) -> Option<&[JsonValue]> {
        match self {
            Self::Array(a) => Some(a),
            _ => None,
        }
    }

    pub fn as_object(&self) -> Option<&BTreeMap<String, JsonValue>> {
        match self {
            Self::Object(o) => Some(o),
            _ => None,
        }
    }
}

pub fn parse(input: &str) -> Result<JsonValue, String> {
    let mut parser = Parser {
        chars: input.chars().peekable(),
    };
    parser.skip_ws();
    let val = parser.parse_value()?;
    parser.skip_ws();
    if parser.chars.peek().is_some() {
        return Err("JSON 末尾存在多余字符".to_string());
    }
    Ok(val)
}

pub fn escape_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for ch in s.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                out.push_str(&format!("\\u{:04x}", c as u32));
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

struct Parser<I: Iterator<Item = char>> {
    chars: std::iter::Peekable<I>,
}

impl<I: Iterator<Item = char>> Parser<I> {
    fn skip_ws(&mut self) {
        while let Some(&c) = self.chars.peek() {
            if c.is_ascii_whitespace() {
                self.chars.next();
            } else {
                break;
            }
        }
    }

    fn parse_value(&mut self) -> Result<JsonValue, String> {
        self.skip_ws();
        match self.chars.peek().copied() {
            Some('{') => self.parse_object(),
            Some('[') => self.parse_array(),
            Some('"') => self.parse_string().map(JsonValue::String),
            Some('t') => self.expect_literal("true").map(|_| JsonValue::Bool(true)),
            Some('f') => self.expect_literal("false").map(|_| JsonValue::Bool(false)),
            Some('n') => self.expect_literal("null").map(|_| JsonValue::Null),
            Some(c) if c.is_ascii_digit() => self.parse_u64().map(JsonValue::Number),
            Some(c) => Err(format!("意外的字符: '{c}'")),
            None => Err("JSON 意外结束".to_string()),
        }
    }

    fn expect_literal(&mut self, lit: &str) -> Result<(), String> {
        for expected in lit.chars() {
            match self.chars.next() {
                Some(c) if c == expected => {}
                _ => return Err(format!("期望关键字 '{lit}'")),
            }
        }
        Ok(())
    }

    fn parse_u64(&mut self) -> Result<u64, String> {
        let mut buf = String::new();
        while let Some(&c) = self.chars.peek() {
            if c.is_ascii_digit() {
                buf.push(c);
                self.chars.next();
            } else {
                break;
            }
        }
        buf.parse::<u64>()
            .map_err(|_| format!("无效的非负整数: {buf}"))
    }

    fn parse_string(&mut self) -> Result<String, String> {
        if self.chars.next() != Some('"') {
            return Err("期望 '\"'".to_string());
        }
        let mut out = String::new();
        while let Some(c) = self.chars.next() {
            match c {
                '"' => return Ok(out),
                '\\' => match self.chars.next() {
                    Some('"') => out.push('"'),
                    Some('\\') => out.push('\\'),
                    Some('/') => out.push('/'),
                    Some('n') => out.push('\n'),
                    Some('r') => out.push('\r'),
                    Some('t') => out.push('\t'),
                    Some('b') => out.push('\u{0008}'),
                    Some('f') => out.push('\u{000C}'),
                    Some('u') => {
                        let mut hex = String::with_capacity(4);
                        for _ in 0..4 {
                            hex.push(
                                self.chars
                                    .next()
                                    .ok_or_else(|| "不完整的 \\u 转义".to_string())?,
                            );
                        }
                        let code = u32::from_str_radix(&hex, 16)
                            .map_err(|_| format!("无效的 \\u 转义: {hex}"))?;
                        let ch = char::from_u32(code)
                            .ok_or_else(|| format!("无效的 Unicode 码点: {code}"))?;
                        out.push(ch);
                    }
                    Some(other) => return Err(format!("不支持的转义字符: \\{other}")),
                    None => return Err("字符串转义未结束".to_string()),
                },
                other => out.push(other),
            }
        }
        Err("字符串缺少闭合引号".to_string())
    }

    fn parse_array(&mut self) -> Result<JsonValue, String> {
        self.chars.next();
        self.skip_ws();
        let mut items = Vec::new();
        if self.chars.peek() == Some(&']') {
            self.chars.next();
            return Ok(JsonValue::Array(items));
        }
        loop {
            items.push(self.parse_value()?);
            self.skip_ws();
            match self.chars.next() {
                Some(',') => {}
                Some(']') => return Ok(JsonValue::Array(items)),
                _ => return Err("数组缺少 ',' 或 ']'".to_string()),
            }
        }
    }

    fn parse_object(&mut self) -> Result<JsonValue, String> {
        self.chars.next();
        self.skip_ws();
        let mut map = BTreeMap::new();
        if self.chars.peek() == Some(&'}') {
            self.chars.next();
            return Ok(JsonValue::Object(map));
        }
        loop {
            self.skip_ws();
            let key = self.parse_string()?;
            self.skip_ws();
            if self.chars.next() != Some(':') {
                return Err(format!("字段 \"{key}\" 后缺少 ':'"));
            }
            let val = self.parse_value()?;
            map.insert(key, val);
            self.skip_ws();
            match self.chars.next() {
                Some(',') => {}
                Some('}') => return Ok(JsonValue::Object(map)),
                _ => return Err("对象缺少 ',' 或 '}'".to_string()),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_objects_arrays_and_unicode() {
        let src = r#"{
            "window_titles": ["激战2", "Guild Wars 2"],
            "interval_ms": 3000,
            "auto_start": true
        }"#;
        let val = parse(src).unwrap();
        let obj = val.as_object().unwrap();
        assert_eq!(obj.get("interval_ms").unwrap().as_u64(), Some(3000));
        assert_eq!(obj.get("auto_start").unwrap().as_bool(), Some(true));
        let titles = obj.get("window_titles").unwrap().as_array().unwrap();
        assert_eq!(titles[0].as_str(), Some("激战2"));
    }

    #[test]
    fn rejects_malformed_json() {
        assert!(parse(r#"{"a": }"#).is_err());
        assert!(parse(r#"{"a": 1} trailing"#).is_err());
    }
}
