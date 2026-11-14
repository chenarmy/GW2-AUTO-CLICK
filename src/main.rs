mod config;
mod json;
mod key;
mod runner;
mod win32;

use std::path::PathBuf;
use std::process;

use config::{
    load_or_create, parse_interval_str, resolve_default_config_path, Config, MatchMode,
    SchedulePlan, SendMode,
};
use runner::{spawn_worker, Controller};
use win32::{install_ctrl_handler, relaunch_as_admin, run_hotkey_loop, HotkeyGuard};

const HOTKEY_TOGGLE_ID: i32 = 1;
const HOTKEY_EXIT_ID: i32 = 2;

struct CliOverrides {
    config_path: Option<PathBuf>,
    interval_ms: Option<u64>,
    jitter_ms: Option<u64>,
    keys: Option<Vec<String>>,
    window: Option<String>,
    mode: Option<SendMode>,
    auto_start: Option<bool>,
    elevated: bool,
    force_admin: bool,
}

fn print_help() {
    println!(
        "GW2 Auto Click (激战2自动技能循环工具)
用法:
  gw2-auto-click [选项]

选项:
  -c, --config <路径>      指定配置文件路径 (默认: config.json，不存在时自动生成)
  -i, --interval <间隔>    覆盖连续点击间隔，支持毫秒或秒 (如: 400、500ms、1s)
  -j, --jitter <抖动>      覆盖随机抖动时间，支持毫秒或秒 (如: 100ms、0.2s)
  -k, --keys <按键列表>    覆盖为简单轮询模式，逗号分隔 (如: 1,2 或 49,50)
  -w, --window <窗口标题>  覆盖目标窗口标题 (如: 激战2)
  -m, --mode <模式>        按键模式: background (后台) 或 foreground (前台)
      --admin              启动时直接请求管理员权限重新运行
      --paused             启动时先保持暂停，按全局热键后再开始
  -h, --help               显示此帮助信息"
    );
}

fn parse_args() -> Result<Option<CliOverrides>, String> {
    let mut args = std::env::args().skip(1);
    let mut out = CliOverrides {
        config_path: None,
        interval_ms: None,
        jitter_ms: None,
        keys: None,
        window: None,
        mode: None,
        auto_start: None,
        elevated: false,
        force_admin: false,
    };

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-h" | "--help" => {
                print_help();
                return Ok(None);
            }
            "-c" | "--config" => {
                let val = args.next().ok_or("缺少 --config 参数值")?;
                out.config_path = Some(PathBuf::from(val));
            }
            "-i" | "--interval" => {
                let val = args.next().ok_or("缺少 --interval 参数值")?;
                out.interval_ms = Some(parse_interval_str(&val)?);
            }
            "-j" | "--jitter" => {
                let val = args.next().ok_or("缺少 --jitter 参数值")?;
                out.jitter_ms = Some(parse_interval_str(&val)?);
            }
            "-k" | "--keys" => {
                let val = args.next().ok_or("缺少 --keys 参数值")?;
                let keys: Vec<String> = val
                    .split(',')
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect();
                if keys.is_empty() {
                    return Err("--keys 不能为空".to_string());
                }
                out.keys = Some(keys);
            }
            "-w" | "--window" => {
                let val = args.next().ok_or("缺少 --window 参数值")?;
                out.window = Some(val);
            }
            "-m" | "--mode" => {
                let val = args.next().ok_or("缺少 --mode 参数值")?.to_ascii_lowercase();
                out.mode = Some(match val.as_str() {
                    "background" | "bg" => SendMode::Background,
                    "foreground" | "fg" => SendMode::Foreground,
                    _ => return Err(format!("无效模式: {val}（可选 background / foreground）")),
                });
            }
            "--paused" => {
                out.auto_start = Some(false);
            }
            "--admin" => {
                out.force_admin = true;
            }
            "--elevated" => {
                out.elevated = true;
            }
            other => return Err(format!("未知参数: {other}（使用 --help 查看说明）")),
        }
    }

    Ok(Some(out))
}

fn apply_overrides(cfg: &mut Config, ov: &CliOverrides) {
    if let Some(ms) = ov.interval_ms {
        cfg.interval_ms = ms;
    }
    if let Some(jitter) = ov.jitter_ms {
        cfg.jitter_ms = jitter;
    }
    if let Some(ref keys) = ov.keys {
        cfg.skills.clear();
        cfg.keys = keys.clone();
    }
    if let Some(ref win) = ov.window {
        cfg.window_titles = vec![win.clone()];
    }
    if let Some(mode) = ov.mode {
        cfg.mode = mode;
    }
    if let Some(auto_start) = ov.auto_start {
        cfg.auto_start = auto_start;
    }
}

fn run() -> Result<(), String> {
    let Some(overrides) = parse_args()? else {
        return Ok(());
    };

    if overrides.force_admin && !overrides.elevated {
        println!("[提权] 正在请求管理员权限重新启动...");
        relaunch_as_admin()?;
        return Ok(());
    }

    let (cfg_path, create_if_missing) = match overrides.config_path {
        Some(ref p) => (p.clone(), false),
        None => (resolve_default_config_path(), true),
    };

    let existed = cfg_path.exists();
    let mut cfg = load_or_create(&cfg_path, create_if_missing)?;
    if !existed {
        println!("[配置] 未找到配置文件，已生成默认配置: {}", cfg_path.display());
    } else {
        println!("[配置] 已加载: {}", cfg_path.display());
    }

    apply_overrides(&mut cfg, &overrides);
    let runtime = cfg.validate()?;

    let mode_label = match runtime.raw.mode {
        SendMode::Background => "后台按键 (PostMessage)",
        SendMode::Foreground => "前台按键 (SendInput)",
    };
    let match_label = match runtime.raw.match_mode {
        MatchMode::Exact => "精确标题匹配",
        MatchMode::Contains => "模糊包含匹配",
    };

    println!("========================================");
    println!("  GW2 Auto Click - 激战2自动技能循环");
    println!("========================================");
    println!("  目标窗口 : {:?} ({})", runtime.raw.window_titles, match_label);
    match &runtime.plan {
        SchedulePlan::CooldownSkills(skills) => {
            let desc: Vec<String> = skills
                .iter()
                .map(|s| {
                    if s.cooldown_ms == 0 {
                        format!("{}(无CD)", s.label)
                    } else if s.cooldown_ms % 1000 == 0 {
                        format!("{}({}s)", s.label, s.cooldown_ms / 1000)
                    } else {
                        format!("{}({}ms)", s.label, s.cooldown_ms)
                    }
                })
                .collect();
            println!(
                "  技能调度 : {} (冷却保护 +{}ms)",
                desc.join(" -> "),
                runtime.raw.cd_buffer_ms
            );
        }
        SchedulePlan::RoundRobin(keys) => {
            let labels: Vec<&str> = keys.iter().map(|(k, _)| k.as_str()).collect();
            println!("  轮询按键 : {:?}", labels);
        }
    }
    println!(
        "  点击间隔 : {}ms (随机抖动 ±{}ms, 按住 {}ms)",
        runtime.raw.interval_ms, runtime.raw.jitter_ms, runtime.raw.key_hold_ms
    );
    println!("  发送模式 : {}", mode_label);
    println!(
        "  全局热键 : [{}] 启动/暂停 | [{}] 退出",
        runtime.raw.toggle_hotkey, runtime.raw.exit_hotkey
    );
    println!(
        "  初始状态 : {}",
        if runtime.raw.auto_start { "运行中" } else { "已暂停 (按热键开始)" }
    );
    println!("========================================");

    install_ctrl_handler();
    let _hotkeys = HotkeyGuard::register(&[
        (HOTKEY_TOGGLE_ID, runtime.toggle_vk),
        (HOTKEY_EXIT_ID, runtime.exit_vk),
    ])?;

    let controller = Controller::new(runtime.raw.auto_start);
    let worker = spawn_worker(runtime.clone(), controller.clone(), overrides.elevated);

    run_hotkey_loop(|id| match id {
        HOTKEY_TOGGLE_ID => {
            let now_running = controller.toggle();
            println!(
                "[热键] 状态切换 -> {}",
                if now_running { "运行中" } else { "已暂停" }
            );
            true
        }
        HOTKEY_EXIT_ID => {
            println!("[热键] 收到退出指令，正在停止...");
            false
        }
        _ => true,
    });

    controller.shutdown();
    let _ = worker.join();
    println!("[退出] 程序已安全退出。");
    Ok(())
}

fn main() {
    if let Err(err) = run() {
        eprintln!("[启动失败] {err}");
        process::exit(1);
    }
}
