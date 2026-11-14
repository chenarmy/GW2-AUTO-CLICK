use std::ffi::{c_void, OsStr};
use std::fmt;
use std::mem::{size_of, zeroed};
use std::os::windows::ffi::OsStrExt;
use std::ptr::{null, null_mut};
use std::sync::atomic::{AtomicU32, Ordering};
use std::thread;
use std::time::Duration;

use crate::config::{MatchMode, SendMode};
use crate::key::KeySpec;

type HWND = *mut c_void;
type BOOL = i32;
type UINT = u32;
type DWORD = u32;
type WORD = u16;
type WPARAM = usize;
type LPARAM = isize;
type WNDENUMPROC = unsafe extern "system" fn(HWND, LPARAM) -> BOOL;
#[allow(non_camel_case_types)]
type PHANDLER_ROUTINE = unsafe extern "system" fn(DWORD) -> BOOL;

const TRUE: BOOL = 1;
const SW_SHOWNORMAL: i32 = 1;
const ERROR_ACCESS_DENIED: DWORD = 5;
const WM_QUIT: UINT = 0x0012;
const WM_KEYDOWN: UINT = 0x0100;
const WM_KEYUP: UINT = 0x0101;
const WM_HOTKEY: UINT = 0x0312;
const MAPVK_VK_TO_VSC: UINT = 0;
const INPUT_KEYBOARD: DWORD = 1;
const KEYEVENTF_EXTENDEDKEY: DWORD = 0x0001;
const KEYEVENTF_KEYUP: DWORD = 0x0002;
const KEYEVENTF_SCANCODE: DWORD = 0x0008;

#[repr(C)]
#[derive(Clone, Copy)]
struct POINT {
    x: i32,
    y: i32,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct MSG {
    hwnd: HWND,
    message: UINT,
    w_param: WPARAM,
    l_param: LPARAM,
    time: DWORD,
    pt: POINT,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct MOUSEINPUT {
    dx: i32,
    dy: i32,
    mouse_data: DWORD,
    dw_flags: DWORD,
    time: DWORD,
    dw_extra_info: usize,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct KEYBDINPUT {
    w_vk: WORD,
    w_scan: WORD,
    dw_flags: DWORD,
    time: DWORD,
    dw_extra_info: usize,
}

#[repr(C)]
#[derive(Clone, Copy)]
union INPUT_UNION {
    mi: MOUSEINPUT,
    ki: KEYBDINPUT,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct INPUT {
    r#type: DWORD,
    u: INPUT_UNION,
}

#[link(name = "user32")]
extern "system" {
    fn FindWindowW(lp_class_name: *const u16, lp_window_name: *const u16) -> HWND;
    fn EnumWindows(lp_enum_func: WNDENUMPROC, l_param: LPARAM) -> BOOL;
    fn IsWindowVisible(hwnd: HWND) -> BOOL;
    fn IsWindow(hwnd: HWND) -> BOOL;
    fn GetWindowTextLengthW(hwnd: HWND) -> i32;
    fn GetWindowTextW(hwnd: HWND, lp_string: *mut u16, n_max_count: i32) -> i32;
    fn GetForegroundWindow() -> HWND;
    fn PostMessageW(hwnd: HWND, msg: UINT, w_param: WPARAM, l_param: LPARAM) -> BOOL;
    fn MapVirtualKeyW(u_code: UINT, u_map_type: UINT) -> UINT;
    fn SendInput(c_inputs: UINT, p_inputs: *const INPUT, cb_size: i32) -> UINT;
    fn RegisterHotKey(hwnd: HWND, id: i32, fs_modifiers: UINT, vk: UINT) -> BOOL;
    fn UnregisterHotKey(hwnd: HWND, id: i32) -> BOOL;
    fn GetMessageW(lp_msg: *mut MSG, hwnd: HWND, w_msg_filter_min: UINT, w_msg_filter_max: UINT) -> BOOL;
    fn PostThreadMessageW(id_thread: DWORD, msg: UINT, w_param: WPARAM, l_param: LPARAM) -> BOOL;
}

#[link(name = "kernel32")]
extern "system" {
    fn GetLastError() -> DWORD;
    fn GetCurrentThreadId() -> DWORD;
    fn SetConsoleCtrlHandler(handler_routine: Option<PHANDLER_ROUTINE>, add: BOOL) -> BOOL;
}

#[link(name = "shell32")]
extern "system" {
    fn ShellExecuteW(
        hwnd: HWND,
        lp_operation: *const u16,
        lp_file: *const u16,
        lp_parameters: *const u16,
        lp_directory: *const u16,
        n_show_cmd: i32,
    ) -> isize;
}

static MAIN_THREAD_ID: AtomicU32 = AtomicU32::new(0);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowInfo {
    pub hwnd: isize,
    pub title: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SendError {
    AccessDenied,
    Other(String),
}

impl fmt::Display for SendError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::AccessDenied => write!(
                f,
                "PostMessageW 被拒绝访问 (ERROR_ACCESS_DENIED)：激战2正以管理员身份运行，请右键「以管理员身份运行」本程序"
            ),
            Self::Other(msg) => write!(f, "{msg}"),
        }
    }
}

/// Converts a Rust string to a null-terminated UTF-16 buffer for Win32 `*W` APIs.
fn to_wide(s: &str) -> Vec<u16> {
    OsStr::new(s).encode_wide().chain(Some(0)).collect()
}

/// Builds the Win32 `lParam` for `WM_KEYDOWN` / `WM_KEYUP`.
pub fn build_key_lparam(scan_code: u32, extended: bool, key_up: bool) -> isize {
    let mut lparam: u32 = 1 | ((scan_code & 0xFF) << 16);
    if extended {
        lparam |= 1 << 24;
    }
    if key_up {
        lparam |= (1 << 30) | (1 << 31);
    }
    lparam as i32 as isize
}

/// Finds a target window matching any title in `titles` according to `match_mode`.
pub fn find_target_window(titles: &[String], match_mode: MatchMode) -> Option<WindowInfo> {
    match match_mode {
        MatchMode::Exact => {
            for title in titles {
                let trimmed = title.trim();
                if trimmed.is_empty() {
                    continue;
                }
                let wide = to_wide(trimmed);
                let hwnd = unsafe { FindWindowW(null(), wide.as_ptr()) };
                if !hwnd.is_null() {
                    return Some(WindowInfo {
                        hwnd: hwnd as isize,
                        title: trimmed.to_string(),
                    });
                }
            }
            None
        }
        MatchMode::Contains => find_window_contains(titles),
    }
}

struct EnumSearchState {
    patterns: Vec<String>,
    found: Option<WindowInfo>,
}

fn find_window_contains(titles: &[String]) -> Option<WindowInfo> {
    let patterns: Vec<String> = titles
        .iter()
        .map(|t| t.trim().to_lowercase())
        .filter(|t| !t.is_empty())
        .collect();
    if patterns.is_empty() {
        return None;
    }

    let mut state = EnumSearchState {
        patterns,
        found: None,
    };

    unsafe extern "system" fn enum_proc(hwnd: HWND, lparam: LPARAM) -> BOOL {
        if IsWindowVisible(hwnd) == 0 {
            return TRUE;
        }
        let len = GetWindowTextLengthW(hwnd);
        if len <= 0 {
            return TRUE;
        }
        let mut buf = vec![0u16; (len as usize) + 1];
        let copied = GetWindowTextW(hwnd, buf.as_mut_ptr(), buf.len() as i32);
        if copied <= 0 {
            return TRUE;
        }
        let title = String::from_utf16_lossy(&buf[..copied as usize]);
        let title_lower = title.to_lowercase();
        let state = &mut *(lparam as *mut EnumSearchState);
        if state.patterns.iter().any(|p| title_lower.contains(p)) {
            state.found = Some(WindowInfo {
                hwnd: hwnd as isize,
                title,
            });
            return 0;
        }
        TRUE
    }

    unsafe {
        EnumWindows(enum_proc, (&mut state as *mut EnumSearchState) as LPARAM);
    }
    state.found
}

pub fn is_window_valid(hwnd: isize) -> bool {
    hwnd != 0 && unsafe { IsWindow(hwnd as HWND) != 0 }
}

pub fn is_window_foreground(hwnd: isize) -> bool {
    hwnd != 0 && unsafe { GetForegroundWindow() == (hwnd as HWND) }
}

/// Sends a key press according to `mode`.
/// Returns `Ok(true)` if sent, `Ok(false)` if skipped (e.g., foreground mode when unfocused).
pub fn send_key(hwnd: isize, key: KeySpec, hold_ms: u64, mode: SendMode) -> Result<bool, SendError> {
    match mode {
        SendMode::Background => {
            send_background_key(hwnd, key, hold_ms)?;
            Ok(true)
        }
        SendMode::Foreground => {
            if !is_window_foreground(hwnd) {
                return Ok(false);
            }
            send_foreground_key(key, hold_ms).map_err(SendError::Other)?;
            Ok(true)
        }
    }
}

fn send_background_key(hwnd: isize, key: KeySpec, hold_ms: u64) -> Result<(), SendError> {
    let target = hwnd as HWND;
    let scan_code = unsafe { MapVirtualKeyW(key.vk as UINT, MAPVK_VK_TO_VSC) };
    let lparam_down = build_key_lparam(scan_code, key.extended, false);
    let lparam_up = build_key_lparam(scan_code, key.extended, true);

    post_message_checked(target, WM_KEYDOWN, key.vk as WPARAM, lparam_down)?;
    if hold_ms > 0 {
        thread::sleep(Duration::from_millis(hold_ms));
    }
    post_message_checked(target, WM_KEYUP, key.vk as WPARAM, lparam_up)?;
    Ok(())
}

fn post_message_checked(
    hwnd: HWND,
    msg: UINT,
    wparam: WPARAM,
    lparam: LPARAM,
) -> Result<(), SendError> {
    let ok = unsafe { PostMessageW(hwnd, msg, wparam, lparam) };
    if ok != 0 {
        return Ok(());
    }
    let err = unsafe { GetLastError() };
    if err == ERROR_ACCESS_DENIED {
        Err(SendError::AccessDenied)
    } else {
        Err(SendError::Other(format!(
            "PostMessageW 失败，Win32 错误码: {err}"
        )))
    }
}

fn send_foreground_key(key: KeySpec, hold_ms: u64) -> Result<(), String> {
    let scan_code = unsafe { MapVirtualKeyW(key.vk as UINT, MAPVK_VK_TO_VSC) } as u16;
    let mut flags = KEYEVENTF_SCANCODE;
    if key.extended {
        flags |= KEYEVENTF_EXTENDEDKEY;
    }

    send_single_input(key.vk, scan_code, flags)?;
    if hold_ms > 0 {
        thread::sleep(Duration::from_millis(hold_ms));
    }
    send_single_input(key.vk, scan_code, flags | KEYEVENTF_KEYUP)?;
    Ok(())
}

fn send_single_input(vk: u16, scan_code: u16, flags: u32) -> Result<(), String> {
    let input = INPUT {
        r#type: INPUT_KEYBOARD,
        u: INPUT_UNION {
            ki: KEYBDINPUT {
                w_vk: if scan_code == 0 { vk } else { 0 },
                w_scan: scan_code,
                dw_flags: if scan_code == 0 {
                    flags & !KEYEVENTF_SCANCODE
                } else {
                    flags
                },
                time: 0,
                dw_extra_info: 0,
            },
        },
    };
    let sent = unsafe { SendInput(1, &input, size_of::<INPUT>() as i32) };
    if sent != 1 {
        let err = unsafe { GetLastError() };
        return Err(format!("SendInput 失败，Win32 错误码: {err}"));
    }
    Ok(())
}

/// Relaunches the current executable with UAC elevation (`runas`) in the current directory.
pub fn relaunch_as_admin() -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| format!("获取当前程序路径失败: {e}"))?;
    let cwd = std::env::current_dir().ok();

    let mut params: Vec<String> = std::env::args()
        .skip(1)
        .filter(|a| a != "--elevated")
        .map(|a| quote_win_arg(&a))
        .collect();
    params.push("--elevated".to_string());
    let param_str = params.join(" ");

    let op_w = to_wide("runas");
    let exe_w = to_wide(&exe.to_string_lossy());
    let params_w = to_wide(&param_str);
    let cwd_w = cwd.map(|p| to_wide(&p.to_string_lossy()));
    let cwd_ptr = cwd_w.as_ref().map_or(null(), |v| v.as_ptr());

    let res = unsafe {
        ShellExecuteW(
            null_mut(),
            op_w.as_ptr(),
            exe_w.as_ptr(),
            params_w.as_ptr(),
            cwd_ptr,
            SW_SHOWNORMAL,
        )
    };
    if res <= 32 {
        return Err(format!("管理员提权被取消或失败 (ShellExecuteW 返回 {res})"));
    }
    Ok(())
}

fn quote_win_arg(arg: &str) -> String {
    if !arg.is_empty() && !arg.chars().any(|c| c == ' ' || c == '\t' || c == '"') {
        return arg.to_string();
    }
    let mut out = String::from("\"");
    let mut backslashes = 0usize;
    for ch in arg.chars() {
        if ch == '\\' {
            backslashes += 1;
        } else if ch == '"' {
            out.push_str(&"\\".repeat(backslashes * 2 + 1));
            out.push('"');
            backslashes = 0;
        } else {
            out.push_str(&"\\".repeat(backslashes));
            backslashes = 0;
            out.push(ch);
        }
    }
    out.push_str(&"\\".repeat(backslashes * 2));
    out.push('"');
    out
}

/// Posts `WM_QUIT` to the main thread's message loop so the program exits cleanly.
pub fn request_quit() {
    let tid = MAIN_THREAD_ID.load(Ordering::SeqCst);
    if tid != 0 {
        unsafe {
            PostThreadMessageW(tid, WM_QUIT, 0, 0);
        }
    }
}

/// Installs Ctrl+C / console close handler so the message loop exits gracefully.
pub fn install_ctrl_handler() {
    let tid = unsafe { GetCurrentThreadId() };
    MAIN_THREAD_ID.store(tid, Ordering::SeqCst);

    unsafe extern "system" fn handler(_ctrl_type: DWORD) -> BOOL {
        request_quit();
        TRUE
    }

    unsafe {
        SetConsoleCtrlHandler(Some(handler), TRUE);
    }
}

pub struct HotkeyGuard {
    registered_ids: Vec<i32>,
}

impl HotkeyGuard {
    pub fn register(pairs: &[(i32, KeySpec)]) -> Result<Self, String> {
        let mut registered_ids = Vec::with_capacity(pairs.len());
        for &(id, key) in pairs {
            let ok = unsafe { RegisterHotKey(null_mut(), id, 0, key.vk as UINT) };
            if ok == 0 {
                let err = unsafe { GetLastError() };
                for reg_id in registered_ids {
                    unsafe { UnregisterHotKey(null_mut(), reg_id) };
                }
                return Err(format!(
                    "注册全局热键 {} 失败 (Win32 错误码: {err})，可能已被其他程序占用",
                    key
                ));
            }
            registered_ids.push(id);
        }
        Ok(Self { registered_ids })
    }
}

impl Drop for HotkeyGuard {
    fn drop(&mut self) {
        for &id in &self.registered_ids {
            unsafe {
                UnregisterHotKey(null_mut(), id);
            }
        }
    }
}

/// Runs the Win32 message loop on the main thread, invoking `on_hotkey` when a registered hotkey fires.
pub fn run_hotkey_loop(mut on_hotkey: impl FnMut(i32) -> bool) {
    let mut msg: MSG = unsafe { zeroed() };
    loop {
        let res = unsafe { GetMessageW(&mut msg, null_mut(), 0, 0) };
        if res <= 0 {
            break;
        }
        if msg.message == WM_HOTKEY {
            let id = msg.w_param as i32;
            if !on_hotkey(id) {
                break;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_lparam_bits_are_valid() {
        let down = build_key_lparam(0x02, false, false);
        assert_eq!(down, 0x0002_0001);

        let up = build_key_lparam(0x02, false, true);
        assert_eq!(up as u32, 0xC002_0001);

        let ext_down = build_key_lparam(0x48, true, false);
        assert_eq!(ext_down as u32, 0x0148_0001);
    }

    #[cfg(target_pointer_width = "64")]
    #[test]
    fn input_struct_size_matches_win64_abi() {
        assert_eq!(size_of::<INPUT>(), 40);
    }

    #[test]
    fn quotes_windows_args_safely() {
        assert_eq!(quote_win_arg("--interval"), "--interval");
        assert_eq!(quote_win_arg(r"C:\My Config\a.json"), r#""C:\My Config\a.json""#);
    }
}
