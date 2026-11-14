use std::fmt;

/// Parsed keyboard key representation backed by a Windows Virtual-Key code.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeySpec {
    pub vk: u16,
    pub extended: bool,
}

impl KeySpec {
    pub const fn new(vk: u16, extended: bool) -> Self {
        Self { vk, extended }
    }
}

impl fmt::Display for KeySpec {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "VK(0x{:02X})", self.vk)
    }
}

/// Parses a key name (e.g. `"1"`, `"F9"`, `"Space"`) or numeric virtual-key code (`"49"`, `"0x31"`).
pub fn parse_key(raw: &str) -> Result<KeySpec, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err("按键配置不能为空".to_string());
    }

    // Numeric VK codes (decimal like "49" for '1', or hex like "0x31"),
    // checked after single-digit keys so "1" parses as key '1' (0x31), not VK 0x01 (LBUTTON).
    if trimmed.len() == 1 {
        let ch = trimmed.chars().next().unwrap();
        if ch.is_ascii_digit() {
            return Ok(KeySpec::new(ch as u16, false));
        }
        if ch.is_ascii_alphabetic() {
            return Ok(KeySpec::new(ch.to_ascii_uppercase() as u16, false));
        }
        if let Some(vk) = match_punctuation(ch) {
            return Ok(KeySpec::new(vk, false));
        }
    }

    if let Some(hex) = trimmed
        .strip_prefix("0x")
        .or_else(|| trimmed.strip_prefix("0X"))
    {
        let code = u16::from_str_radix(hex, 16)
            .map_err(|_| format!("无效的十六进制键码: {trimmed}"))?;
        return validate_vk(code, trimmed);
    }

    if trimmed.chars().all(|c| c.is_ascii_digit()) {
        let code = trimmed
            .parse::<u16>()
            .map_err(|_| format!("无效的键码: {trimmed}"))?;
        return validate_vk(code, trimmed);
    }

    let upper = trimmed.to_ascii_uppercase();
    if let Some(rest) = upper.strip_prefix('F') {
        if let Ok(num) = rest.parse::<u16>() {
            if (1..=24).contains(&num) {
                return Ok(KeySpec::new(0x70 + (num - 1), false));
            }
        }
    }

    match upper.as_str() {
        "BACKSPACE" | "BACK" => Ok(KeySpec::new(0x08, false)),
        "TAB" => Ok(KeySpec::new(0x09, false)),
        "ENTER" | "RETURN" => Ok(KeySpec::new(0x0D, false)),
        "SHIFT" => Ok(KeySpec::new(0x10, false)),
        "CTRL" | "CONTROL" => Ok(KeySpec::new(0x11, false)),
        "ALT" | "MENU" => Ok(KeySpec::new(0x12, false)),
        "ESC" | "ESCAPE" => Ok(KeySpec::new(0x1B, false)),
        "SPACE" | "SPACEBAR" => Ok(KeySpec::new(0x20, false)),
        "PAGEUP" | "PGUP" => Ok(KeySpec::new(0x21, true)),
        "PAGEDOWN" | "PGDN" => Ok(KeySpec::new(0x22, true)),
        "END" => Ok(KeySpec::new(0x23, true)),
        "HOME" => Ok(KeySpec::new(0x24, true)),
        "LEFT" => Ok(KeySpec::new(0x25, true)),
        "UP" => Ok(KeySpec::new(0x26, true)),
        "RIGHT" => Ok(KeySpec::new(0x27, true)),
        "DOWN" => Ok(KeySpec::new(0x28, true)),
        "INSERT" | "INS" => Ok(KeySpec::new(0x2D, true)),
        "DELETE" | "DEL" => Ok(KeySpec::new(0x2E, true)),
        _ => Err(format!("不支持的按键名称: {trimmed}")),
    }
}

fn validate_vk(code: u16, raw: &str) -> Result<KeySpec, String> {
    if (1..=254).contains(&code) {
        let extended = matches!(code, 0x21..=0x28 | 0x2D | 0x2E);
        Ok(KeySpec::new(code, extended))
    } else {
        Err(format!("虚拟键码超出有效范围 (1..=254): {raw}"))
    }
}

fn match_punctuation(ch: char) -> Option<u16> {
    match ch {
        ';' => Some(0xBA),
        '=' => Some(0xBB),
        ',' => Some(0xBC),
        '-' => Some(0xBD),
        '.' => Some(0xBE),
        '/' => Some(0xBF),
        '`' => Some(0xC0),
        '[' => Some(0xDB),
        '\\' => Some(0xDC),
        ']' => Some(0xDD),
        '\'' => Some(0xDE),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_digits_and_letters() {
        assert_eq!(parse_key("1").unwrap(), KeySpec::new(0x31, false));
        assert_eq!(parse_key("2").unwrap(), KeySpec::new(0x32, false));
        assert_eq!(parse_key("a").unwrap(), KeySpec::new(0x41, false));
        assert_eq!(parse_key("Z").unwrap(), KeySpec::new(0x5A, false));
    }

    #[test]
    fn parses_numeric_vk_codes_for_macro_compatibility() {
        assert_eq!(parse_key("49").unwrap(), KeySpec::new(0x31, false));
        assert_eq!(parse_key("0x32").unwrap(), KeySpec::new(0x32, false));
    }

    #[test]
    fn parses_function_and_named_keys() {
        assert_eq!(parse_key("f9").unwrap(), KeySpec::new(0x78, false));
        assert_eq!(parse_key("F10").unwrap(), KeySpec::new(0x79, false));
        assert_eq!(parse_key("Space").unwrap(), KeySpec::new(0x20, false));
        assert_eq!(parse_key("Up").unwrap(), KeySpec::new(0x26, true));
    }

    #[test]
    fn rejects_invalid_keys() {
        assert!(parse_key("").is_err());
        assert!(parse_key("0").is_ok());
        assert!(parse_key("0x00").is_err());
        assert!(parse_key("999").is_err());
        assert!(parse_key("UnknownKey").is_err());
    }
}
