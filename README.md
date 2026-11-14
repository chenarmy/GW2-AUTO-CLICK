# GW2 Auto Click (`gw2-auto-click`)

一个用 **纯 Rust**（零第三方 Crate 依赖）编写的《激战2》（Guild Wars 2）后台/前台自动技能循环小工具，重构替代早期的按键精灵脚本。

## 特性

- **零外部依赖**：仅依赖 Rust 标准库与 Windows 原生 `user32.dll` / `kernel32.dll` / `shell32.dll` API，编译速度快，生成的 `.exe` 单文件无任何运行库依赖。
- **冷却优先级技能循环**：支持为每个技能配置独立 CD 与优先级。默认按 `Q (22s) → 4 (13s) → 3 (8s) → 2 (5s) → 1 (无CD)` 连续点击，冷却一好立即释放，其余时间连续填充 `1` 技能，保持所有带 CD 技能全程处于冷却中。
- **自动绑定激战2窗口**：默认查找标题为 `激战2` 或 `Guild Wars 2` 的窗口（对应原按键精灵 `Plugin.Window.Find("0", "激战2")`）；游戏关闭时自动暂停并重新等待窗口。
- **自动管理员提权**：若《激战2》以管理员身份运行导致后台消息被拒绝（`ERROR_ACCESS_DENIED`），支持自动弹出 UAC 请求提权重启（也可手动 `--admin` 或右键管理员运行）。
- **后台 / 前台双模式**：
  - `background`（默认）：通过 `PostMessageW` 向游戏窗口句柄发送 `WM_KEYDOWN` / `WM_KEYUP`（对应 `Plugin.Bkgnd.KeyPress`），切出游戏浏览网页或聊天时不会抢焦点。
  - `foreground`：通过 `SendInput` 发送硬件扫描码，仅在游戏处于前台激活窗口时触发。
- **全局热键控制**：默认 `F9` 启动/暂停，`F10` 安全退出。

## 快速开始

### 1. 编译

需要已安装 Rust 工具链（`x86_64-pc-windows-msvc` 或 `gnu`）：

```powershell
cargo build --release
```

编译产物位于 `target\release\gw2-auto-click.exe`。

### 2. 运行

直接运行编译好的 `.exe`：

```powershell
.\target\release\gw2-auto-click.exe
```

首次运行若同目录没有 `config.json`，程序会自动生成一份默认配置并开始监听 `激战2` 窗口。如果检测到《激战2》以管理员身份运行，程序会自动弹出 UAC 提示以管理员权限重启自身。

## 配置文件说明 (`config.json`)

可参考仓库中的 [`config.example.json`](./config.example.json)：

```json
{
  "window_titles": ["激战2", "Guild Wars 2"],
  "match_mode": "exact",
  "skills": [
    { "key": "Q", "cooldown_ms": 22000 },
    { "key": "4", "cooldown_ms": 13000 },
    { "key": "3", "cooldown_ms": 8000 },
    { "key": "2", "cooldown_ms": 5000 },
    { "key": "1", "cooldown_ms": 0 }
  ],
  "keys": [],
  "interval_ms": 400,
  "cd_buffer_ms": 200,
  "jitter_ms": 0,
  "key_hold_ms": 50,
  "mode": "background",
  "toggle_hotkey": "F9",
  "exit_hotkey": "F10",
  "auto_start": true,
  "auto_elevate": true
}
```

| 字段 | 说明 | 默认值 |
| --- | --- | --- |
| `window_titles` | 目标窗口标题列表，按顺序匹配 | `["激战2", "Guild Wars 2"]` |
| `match_mode` | 窗口匹配方式：`exact`（精确）或 `contains`（模糊包含） | `"exact"` |
| `skills` | 技能列表（按数组顺序决定优先级）。`cooldown_ms > 0` 的技能冷却好即优先释放，`cooldown_ms = 0` 作为无 CD 填充技 | `Q(22s), 4(13s), 3(8s), 2(5s), 1(0)` |
| `keys` | 简单轮询按键列表（仅当 `skills` 为空时生效） | `[]` |
| `interval_ms` | 连续点击的间隔（毫秒，建议 `300~500`） | `400` |
| `cd_buffer_ms` | 冷却额外保护时间（毫秒），防止因技能施法动画/网络延迟导致过早触发 | `200` |
| `jitter_ms` | 随机时间抖动范围 `±jitter_ms`（毫秒，`0` 表示关闭） | `0` |
| `key_hold_ms` | 按下到松开之间的按住时长（毫秒） | `50` |
| `mode` | `background`（后台 `PostMessage`）或 `foreground`（前台 `SendInput`） | `"background"` |
| `toggle_hotkey` | 启动 / 暂停全局热键 | `"F9"` |
| `exit_hotkey` | 退出程序全局热键 | `"F10"` |
| `auto_start` | 启动程序后是否立即进入运行状态 | `true` |
| `auto_elevate` | 遇到 `ERROR_ACCESS_DENIED` 时是否自动弹窗请求管理员提权 | `true` |

## 命令行参数覆盖

不改 `config.json` 也可以通过命令行参数临时覆盖配置：

```powershell
# 启动时直接以管理员权限运行，并将连续点击间隔设为 500ms
.\target\release\gw2-auto-click.exe --admin --interval 500ms

# 临时切换为简单轮询模式（每 3 秒交替按 1 和 2）
.\target\release\gw2-auto-click.exe --keys 1,2 --interval 3s

# 使用前台模式，启动后先暂停，按 F9 再开始
.\target\release\gw2-auto-click.exe --mode foreground --paused

# 查看所有参数
.\target\release\gw2-auto-click.exe --help
```

## 免责声明

本项目仅供学习 Windows API、Rust 系统编程及个人辅助研究使用，请遵守游戏服务条款与相关规定，切勿用于破坏游戏平衡的用途。

## License

[MIT](./LICENSE)
