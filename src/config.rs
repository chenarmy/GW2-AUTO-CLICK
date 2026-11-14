use std::fs;
use std::path::{Path, PathBuf};

use crate::json::{self, escape_string, JsonValue};
use crate::key::{parse_key, KeySpec};

pub const DEFAULT_CONFIG_FILE: &str = "config.json";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SendMode {
    /// Sends WM_KEYDOWN / WM_KEYUP directly to the target window handle (no focus steal).
    Background,
    /// Sends hardware scan-code input only when the target window is active in foreground.
    Foreground,
}

impl Default for SendMode {
    fn default() -> Self {
        Self::Background
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MatchMode {
    Exact,
    Contains,
}

impl Default for MatchMode {
    fn default() -> Self {
        Self::Exact
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillConfig {
    pub key: String,
    pub cooldown_ms: u64,
}

impl SkillConfig {
    pub fn new(key: &str, cooldown_ms: u64) -> Self {
        Self {
            key: key.to_string(),
            cooldown_ms,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    /// Candidate window titles to search for in priority order.
    pub window_titles: Vec<String>,
    /// Window title matching strategy: "exact" or "contains".
    pub match_mode: MatchMode,
    /// Priority-ordered skill list with per-skill cooldowns.
    /// Skills with cooldown_ms > 0 are cast whenever off cooldown; cooldown_ms == 0 acts as filler.
    pub skills: Vec<SkillConfig>,
    /// Fallback simple round-robin keys used only when `skills` is empty.
    pub keys: Vec<String>,
    /// Delay between consecutive key presses in milliseconds (continuous clicking cadence).
    pub interval_ms: u64,
    /// Extra safety margin added to non-zero skill cooldowns (ms) to avoid casting slightly too early.
    pub cd_buffer_ms: u64,
    /// Optional random +/- jitter in milliseconds applied to interval_ms.
    pub jitter_ms: u64,
    /// Hold duration between KeyDown and KeyUp in milliseconds.
    pub key_hold_ms: u64,
    /// "background" (PostMessage) or "foreground" (SendInput).
    pub mode: SendMode,
    /// Global hotkey to start/pause pressing.
    pub toggle_hotkey: String,
    /// Global hotkey to exit the program.
    pub exit_hotkey: String,
    /// Whether to start pressing immediately when launched.
    pub auto_start: bool,
    /// Automatically prompt UAC elevation if PostMessageW hits ERROR_ACCESS_DENIED.
    pub auto_elevate: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            window_titles: vec!["激战2".to_string(), "Guild Wars 2".to_string()],
            match_mode: MatchMode::Exact,
            skills: vec![
                SkillConfig::new("Q", 22_000),
                SkillConfig::new("4", 13_000),
                SkillConfig::new("3", 8_000),
                SkillConfig::new("2", 5_000),
                SkillConfig::new("1", 0),
            ],
            keys: Vec::new(),
            interval_ms: 400,
            cd_buffer_ms: 200,
            jitter_ms: 0,
            key_hold_ms: 50,
            mode: SendMode::Background,
            toggle_hotkey: "F9".to_string(),
            exit_hotkey: "F10".to_string(),
            auto_start: true,
            auto_elevate: true,
        }
    }
}

#[derive(Debug, Clone)]
pub struct ParsedSkill {
    pub label: String,
    pub spec: KeySpec,
    pub cooldown_ms: u64,
}

#[derive(Debug, Clone)]
pub enum SchedulePlan {
    /// Priority-ordered skills tracked by individual cooldowns.
    CooldownSkills(Vec<ParsedSkill>),
    /// Simple cyclic key list when `skills` is empty.
    RoundRobin(Vec<(String, KeySpec)>),
}

/// Validated runtime representation ready for execution.
#[derive(Debug, Clone)]
pub struct RuntimeConfig {
    pub raw: Config,
    pub plan: SchedulePlan,
    pub toggle_vk: KeySpec,
    pub exit_vk: KeySpec,
}

impl Config {
    pub fn to_pretty_json(&self) -> String {
        let format_str_array = |items: &[String]| -> String {
            items
                .iter()
                .map(|s| escape_string(s))
                .collect::<Vec<_>>()
                .join(", ")
        };
        let skills_json = if self.skills.is_empty() {
            "[]".to_string()
        } else {
            let entries: Vec<String> = self
                .skills
                .iter()
                .map(|s| {
                    format!(
                        "    {{ \"key\": {}, \"cooldown_ms\": {} }}",
                        escape_string(&s.key),
                        s.cooldown_ms
                    )
                })
                .collect();
            format!("[\n{}\n  ]", entries.join(",\n"))
        };
        let match_mode = match self.match_mode {
            MatchMode::Exact => "exact",
            MatchMode::Contains => "contains",
        };
        let mode = match self.mode {
            SendMode::Background => "background",
            SendMode::Foreground => "foreground",
        };
        format!(
            "{{\n  \"window_titles\": [{}],\n  \"match_mode\": {},\n  \"skills\": {},\n  \"keys\": [{}],\n  \"interval_ms\": {},\n  \"cd_buffer_ms\": {},\n  \"jitter_ms\": {},\n  \"key_hold_ms\": {},\n  \"mode\": {},\n  \"toggle_hotkey\": {},\n  \"exit_hotkey\": {},\n  \"auto_start\": {},\n  \"auto_elevate\": {}\n}}\n",
            format_str_array(&self.window_titles),
            escape_string(match_mode),
            skills_json,
            format_str_array(&self.keys),
            self.interval_ms,
            self.cd_buffer_ms,
            self.jitter_ms,
            self.key_hold_ms,
            escape_string(mode),
            escape_string(&self.toggle_hotkey),
            escape_string(&self.exit_hotkey),
            self.auto_start,
            self.auto_elevate
        )
    }

    pub fn from_json_str(content: &str) -> Result<Self, String> {
        let root = json::parse(content)?;
        let obj = root
            .as_object()
            .ok_or_else(|| "配置根节点必须是 JSON 对象".to_string())?;

        let mut cfg = Config::default();

        if let Some(v) = obj.get("window_titles") {
            cfg.window_titles = parse_string_array("window_titles", v)?;
        }
        if let Some(v) = obj.get("match_mode") {
            let s = v
                .as_str()
                .ok_or_else(|| "match_mode 必须是字符串".to_string())?
                .to_ascii_lowercase();
            cfg.match_mode = match s.as_str() {
                "exact" => MatchMode::Exact,
                "contains" => MatchMode::Contains,
                _ => return Err(format!("无效的 match_mode: {s}（可选 exact / contains）")),
            };
        }
        if let Some(v) = obj.get("skills") {
            cfg.skills = parse_skills_array(v)?;
        }
        if let Some(v) = obj.get("keys") {
            cfg.keys = parse_string_array("keys", v)?;
            // If user provided legacy `keys` without `skills`, switch to round-robin mode.
            if !obj.contains_key("skills") && !cfg.keys.is_empty() {
                cfg.skills.clear();
            }
        }
        if let Some(v) = obj.get("interval_ms") {
            cfg.interval_ms = v
                .as_u64()
                .ok_or_else(|| "interval_ms 必须是非负整数".to_string())?;
        }
        if let Some(v) = obj.get("cd_buffer_ms") {
            cfg.cd_buffer_ms = v
                .as_u64()
                .ok_or_else(|| "cd_buffer_ms 必须是非负整数".to_string())?;
        }
        if let Some(v) = obj.get("jitter_ms") {
            cfg.jitter_ms = v
                .as_u64()
                .ok_or_else(|| "jitter_ms 必须是非负整数".to_string())?;
        }
        if let Some(v) = obj.get("key_hold_ms") {
            cfg.key_hold_ms = v
                .as_u64()
                .ok_or_else(|| "key_hold_ms 必须是非负整数".to_string())?;
        }
        if let Some(v) = obj.get("mode") {
            let s = v
                .as_str()
                .ok_or_else(|| "mode 必须是字符串".to_string())?
                .to_ascii_lowercase();
            cfg.mode = match s.as_str() {
                "background" => SendMode::Background,
                "foreground" => SendMode::Foreground,
                _ => return Err(format!("无效的 mode: {s}（可选 background / foreground）")),
            };
        }
        if let Some(v) = obj.get("toggle_hotkey") {
            cfg.toggle_hotkey = v
                .as_str()
                .ok_or_else(|| "toggle_hotkey 必须是字符串".to_string())?
                .to_string();
        }
        if let Some(v) = obj.get("exit_hotkey") {
            cfg.exit_hotkey = v
                .as_str()
                .ok_or_else(|| "exit_hotkey 必须是字符串".to_string())?
                .to_string();
        }
        if let Some(v) = obj.get("auto_start") {
            cfg.auto_start = v
                .as_bool()
                .ok_or_else(|| "auto_start 必须是布尔值".to_string())?;
        }
        if let Some(v) = obj.get("auto_elevate") {
            cfg.auto_elevate = v
                .as_bool()
                .ok_or_else(|| "auto_elevate 必须是布尔值".to_string())?;
        }

        Ok(cfg)
    }

    pub fn validate(&self) -> Result<RuntimeConfig, String> {
        if self.window_titles.iter().all(|t| t.trim().is_empty()) {
            return Err("window_titles 不能为空".to_string());
        }
        if self.interval_ms == 0 {
            return Err("interval_ms 必须大于 0".to_string());
        }

        let plan = if !self.skills.is_empty() {
            let mut parsed = Vec::with_capacity(self.skills.len());
            for skill in &self.skills {
                let spec = parse_key(&skill.key)?;
                parsed.push(ParsedSkill {
                    label: skill.key.trim().to_string(),
                    spec,
                    cooldown_ms: skill.cooldown_ms,
                });
            }
            SchedulePlan::CooldownSkills(parsed)
        } else if !self.keys.is_empty() {
            let mut parsed_keys = Vec::with_capacity(self.keys.len());
            for k in &self.keys {
                let spec = parse_key(k)?;
                parsed_keys.push((k.trim().to_string(), spec));
            }
            SchedulePlan::RoundRobin(parsed_keys)
        } else {
            return Err("skills 和 keys 不能同时为空，至少配置一个技能或按键".to_string());
        };

        let toggle_vk = parse_key(&self.toggle_hotkey)
            .map_err(|e| format!("toggle_hotkey 无效 ({}): {e}", self.toggle_hotkey))?;
        let exit_vk = parse_key(&self.exit_hotkey)
            .map_err(|e| format!("exit_hotkey 无效 ({}): {e}", self.exit_hotkey))?;

        if toggle_vk.vk == exit_vk.vk {
            return Err("toggle_hotkey 与 exit_hotkey 不能相同".to_string());
        }

        Ok(RuntimeConfig {
            raw: self.clone(),
            plan,
            toggle_vk,
            exit_vk,
        })
    }
}

fn parse_skills_array(val: &JsonValue) -> Result<Vec<SkillConfig>, String> {
    let arr = val
        .as_array()
        .ok_or_else(|| "skills 必须是数组".to_string())?;
    let mut out = Vec::with_capacity(arr.len());
    for (idx, item) in arr.iter().enumerate() {
        let obj = item
            .as_object()
            .ok_or_else(|| format!("skills[{idx}] 必须是对象"))?;
        let key = obj
            .get("key")
            .and_then(|v| v.as_str())
            .ok_or_else(|| format!("skills[{idx}].key 必须是字符串"))?
            .to_string();
        let cooldown_ms = match obj.get("cooldown_ms") {
            Some(v) => v
                .as_u64()
                .ok_or_else(|| format!("skills[{idx}].cooldown_ms 必须是非负整数"))?,
            None => 0,
        };
        out.push(SkillConfig { key, cooldown_ms });
    }
    Ok(out)
}

fn parse_string_array(field: &str, val: &JsonValue) -> Result<Vec<String>, String> {
    let arr = val
        .as_array()
        .ok_or_else(|| format!("{field} 必须是字符串数组"))?;
    let mut out = Vec::with_capacity(arr.len());
    for item in arr {
        let s = item
            .as_str()
            .ok_or_else(|| format!("{field} 的每一项都必须是字符串"))?;
        out.push(s.to_string());
    }
    Ok(out)
}

/// Loads config from `path`. When `create_if_missing` is true and `path` does not exist,
/// writes a default `config.json` and returns it.
pub fn load_or_create(path: &Path, create_if_missing: bool) -> Result<Config, String> {
    if !path.exists() {
        if !create_if_missing {
            return Err(format!("找不到配置文件: {}", path.display()));
        }
        let default_cfg = Config::default();
        fs::write(path, default_cfg.to_pretty_json())
            .map_err(|e| format!("写入默认配置 {} 失败: {e}", path.display()))?;
        return Ok(default_cfg);
    }

    let content =
        fs::read_to_string(path).map_err(|e| format!("读取配置 {} 失败: {e}", path.display()))?;
    Config::from_json_str(&content)
        .map_err(|e| format!("解析配置 {} 失败: {e}", path.display()))
}

/// Resolves the default config path: `./config.json` if it exists, otherwise checks beside the exe.
pub fn resolve_default_config_path() -> PathBuf {
    let cwd_path = PathBuf::from(DEFAULT_CONFIG_FILE);
    if cwd_path.exists() {
        return cwd_path;
    }
    if let Ok(exe_path) = std::env::current_exe() {
        if let Some(dir) = exe_path.parent() {
            let exe_cfg = dir.join(DEFAULT_CONFIG_FILE);
            if exe_cfg.exists() {
                return exe_cfg;
            }
        }
    }
    cwd_path
}

/// Parses an interval string like `"3000"`, `"3000ms"`, `"3s"`, or `"1.5s"` into milliseconds.
pub fn parse_interval_str(raw: &str) -> Result<u64, String> {
    let s = raw.trim().to_ascii_lowercase();
    if let Some(ms) = s.strip_suffix("ms") {
        let val = ms
            .trim()
            .parse::<u64>()
            .map_err(|_| format!("无效的毫秒间隔: {raw}"))?;
        return Ok(val);
    }
    if let Some(secs) = s.strip_suffix('s') {
        let val = secs
            .trim()
            .parse::<f64>()
            .map_err(|_| format!("无效的秒数间隔: {raw}"))?;
        if !val.is_finite() || val <= 0.0 {
            return Err(format!("间隔必须大于 0: {raw}"));
        }
        return Ok((val * 1000.0).round() as u64);
    }
    s.parse::<u64>()
        .map_err(|_| format!("无效的间隔格式: {raw}（支持如 1000、1000ms、2.5s）"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_roundtrips_and_validates() {
        let cfg = Config::default();
        let json = cfg.to_pretty_json();
        let parsed = Config::from_json_str(&json).unwrap();
        assert_eq!(parsed, cfg);

        let runtime = parsed.validate().expect("default config should be valid");
        match runtime.plan {
            SchedulePlan::CooldownSkills(skills) => {
                assert_eq!(skills.len(), 5);
                assert_eq!(skills[0].label, "Q");
                assert_eq!(skills[0].cooldown_ms, 22_000);
                assert_eq!(skills[4].label, "1");
                assert_eq!(skills[4].cooldown_ms, 0);
            }
            _ => panic!("expected CooldownSkills plan"),
        }
    }

    #[test]
    fn partial_json_with_legacy_keys_switches_to_round_robin() {
        let cfg = Config::from_json_str(r#"{"keys": ["1", "2"], "interval_ms": 3000}"#).unwrap();
        assert_eq!(cfg.interval_ms, 3000);
        assert!(cfg.skills.is_empty());
        assert_eq!(cfg.keys, vec!["1", "2"]);
    }

    #[test]
    fn rejects_invalid_configs() {
        let mut cfg = Config::default();
        cfg.interval_ms = 0;
        assert!(cfg.validate().is_err());

        let mut cfg2 = Config::default();
        cfg2.skills.clear();
        cfg2.keys.clear();
        assert!(cfg2.validate().is_err());

        let mut cfg3 = Config::default();
        cfg3.exit_hotkey = cfg3.toggle_hotkey.clone();
        assert!(cfg3.validate().is_err());
    }

    #[test]
    fn parses_interval_strings() {
        assert_eq!(parse_interval_str("1000").unwrap(), 1000);
        assert_eq!(parse_interval_str("2500ms").unwrap(), 2500);
        assert_eq!(parse_interval_str("3s").unwrap(), 3000);
        assert_eq!(parse_interval_str("1.5s").unwrap(), 1500);
        assert!(parse_interval_str("0s").is_err());
    }

    #[test]
    fn example_config_file_is_valid() {
        let example = include_str!("../config.example.json");
        let cfg = Config::from_json_str(example).expect("config.example.json should parse");
        assert_eq!(cfg, Config::default());
        cfg.validate().expect("config.example.json should validate");
    }
}
