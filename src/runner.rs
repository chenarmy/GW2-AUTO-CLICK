use std::sync::{Arc, Condvar, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::config::{ParsedSkill, RuntimeConfig, SchedulePlan};
use crate::key::KeySpec;
use crate::win32::{self, SendError, WindowInfo};

#[derive(Debug, Default)]
struct RunnerState {
    running: bool,
    stop: bool,
}

#[derive(Clone)]
pub struct Controller {
    inner: Arc<(Mutex<RunnerState>, Condvar)>,
}

impl Controller {
    pub fn new(auto_start: bool) -> Self {
        Self {
            inner: Arc::new((
                Mutex::new(RunnerState {
                    running: auto_start,
                    stop: false,
                }),
                Condvar::new(),
            )),
        }
    }

    pub fn set_running(&self, running: bool) {
        let (lock, cv) = &*self.inner;
        let mut state = lock.lock().unwrap();
        state.running = running;
        cv.notify_all();
    }

    pub fn toggle(&self) -> bool {
        let (lock, cv) = &*self.inner;
        let mut state = lock.lock().unwrap();
        state.running = !state.running;
        let now_running = state.running;
        cv.notify_all();
        now_running
    }

    pub fn shutdown(&self) {
        let (lock, cv) = &*self.inner;
        let mut state = lock.lock().unwrap();
        state.stop = true;
        cv.notify_all();
    }

    /// Waits while paused; returns `false` if shutdown was requested.
    fn wait_until_running(&self) -> bool {
        let (lock, cv) = &*self.inner;
        let mut state = lock.lock().unwrap();
        while !state.running && !state.stop {
            state = cv.wait(state).unwrap();
        }
        !state.stop
    }

    /// Interruptible sleep; returns `false` if paused or stopped during sleep.
    fn sleep_while_running(&self, duration: Duration) -> bool {
        let (lock, cv) = &*self.inner;
        let mut state = lock.lock().unwrap();
        let deadline = Instant::now() + duration;
        while state.running && !state.stop {
            let now = Instant::now();
            if now >= deadline {
                return true;
            }
            let (guard, timeout_res) = cv.wait_timeout(state, deadline - now).unwrap();
            state = guard;
            if timeout_res.timed_out() {
                break;
            }
        }
        state.running && !state.stop
    }
}

struct SkillEntry {
    skill: ParsedSkill,
    next_ready_at: Instant,
}

enum SchedulerKind {
    Cooldown {
        entries: Vec<SkillEntry>,
        cd_buffer_ms: u64,
    },
    RoundRobin {
        keys: Vec<(String, KeySpec)>,
        index: usize,
    },
}

pub struct SkillScheduler {
    kind: SchedulerKind,
}

impl SkillScheduler {
    pub fn new(plan: &SchedulePlan, cd_buffer_ms: u64, now: Instant) -> Self {
        let kind = match plan {
            SchedulePlan::CooldownSkills(skills) => SchedulerKind::Cooldown {
                entries: skills
                    .iter()
                    .cloned()
                    .map(|skill| SkillEntry {
                        skill,
                        next_ready_at: now,
                    })
                    .collect(),
                cd_buffer_ms,
            },
            SchedulePlan::RoundRobin(keys) => SchedulerKind::RoundRobin {
                keys: keys.clone(),
                index: 0,
            },
        };
        Self { kind }
    }

    pub fn reset(&mut self, now: Instant) {
        match &mut self.kind {
            SchedulerKind::Cooldown { entries, .. } => {
                for entry in entries {
                    entry.next_ready_at = now;
                }
            }
            SchedulerKind::RoundRobin { index, .. } => {
                *index = 0;
            }
        }
    }

    /// Selects the next ready key to press at `now` and updates its cooldown/index state.
    pub fn next_action(&mut self, now: Instant) -> Option<(String, KeySpec, Option<u64>)> {
        match &mut self.kind {
            SchedulerKind::Cooldown {
                entries,
                cd_buffer_ms,
            } => {
                let selected_idx = entries.iter().position(|entry| now >= entry.next_ready_at)?;
                let entry = &mut entries[selected_idx];
                let effective_cd = if entry.skill.cooldown_ms > 0 {
                    let total_cd = entry.skill.cooldown_ms.saturating_add(*cd_buffer_ms);
                    entry.next_ready_at = now + Duration::from_millis(total_cd);
                    Some(total_cd)
                } else {
                    None
                };
                Some((entry.skill.label.clone(), entry.skill.spec, effective_cd))
            }
            SchedulerKind::RoundRobin { keys, index } => {
                let (label, spec) = &keys[*index];
                *index = (*index + 1) % keys.len();
                Some((label.clone(), *spec, None))
            }
        }
    }
}

/// Lightweight SplitMix64 PRNG so jitter needs no external crate.
struct SimpleRng(u64);

impl SimpleRng {
    fn from_clock() -> Self {
        let seed = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0x9E37_79B9_7F4A_7C15);
        Self(seed ^ 0xA076_1D64_78BD_642F)
    }

    fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
}

pub fn compute_delay_ms(base_ms: u64, jitter_ms: u64, rand_val: u64) -> u64 {
    if jitter_ms == 0 {
        return base_ms.max(1);
    }
    let span = jitter_ms.saturating_mul(2).saturating_add(1);
    let offset = (rand_val % span) as i128 - (jitter_ms as i128);
    let adjusted = (base_ms as i128 + offset).max(1);
    adjusted as u64
}

pub fn spawn_worker(
    cfg: RuntimeConfig,
    controller: Controller,
    already_elevated: bool,
) -> JoinHandle<()> {
    thread::spawn(move || {
        let mut rng = SimpleRng::from_clock();
        let mut cached_window: Option<WindowInfo> = None;
        let mut waiting_logged = false;
        let mut unfocused_logged = false;
        let mut scheduler = SkillScheduler::new(&cfg.plan, cfg.raw.cd_buffer_ms, Instant::now());

        while controller.wait_until_running() {
            // Re-validate bound window handle before each cycle.
            if let Some(ref win) = cached_window {
                if !win32::is_window_valid(win.hwnd) {
                    println!(
                        "[提示] 目标窗口已关闭或失效 (HWND: 0x{:X})，重新搜索...",
                        win.hwnd
                    );
                    cached_window = None;
                    waiting_logged = false;
                }
            }

            if cached_window.is_none() {
                match win32::find_target_window(&cfg.raw.window_titles, cfg.raw.match_mode) {
                    Some(win) => {
                        println!(
                            "[已绑定] 窗口: \"{}\" (HWND: 0x{:X})",
                            win.title, win.hwnd
                        );
                        cached_window = Some(win);
                        waiting_logged = false;
                        unfocused_logged = false;
                        scheduler.reset(Instant::now());
                    }
                    None => {
                        if !waiting_logged {
                            println!(
                                "[等待] 未找到目标窗口 {:?}，将在检测到窗口后自动开始...",
                                cfg.raw.window_titles
                            );
                            waiting_logged = true;
                        }
                        controller.sleep_while_running(Duration::from_millis(1000));
                        continue;
                    }
                }
            }

            let hwnd = cached_window.as_ref().unwrap().hwnd;
            let now = Instant::now();
            let Some((label, spec, cd_ms)) = scheduler.next_action(now) else {
                controller.sleep_while_running(Duration::from_millis(100));
                continue;
            };

            match win32::send_key(hwnd, spec, cfg.raw.key_hold_ms, cfg.raw.mode) {
                Ok(true) => {
                    unfocused_logged = false;
                    let delay_ms =
                        compute_delay_ms(cfg.raw.interval_ms, cfg.raw.jitter_ms, rng.next_u64());
                    if let Some(cd) = cd_ms {
                        println!(
                            "[技能] 释放 [{label}] ({spec}, 进入冷却 {cd}ms) -> 下次点击 {delay_ms}ms 后"
                        );
                    } else {
                        println!("[按键] 发送 [{label}] ({spec}) -> 下次点击 {delay_ms}ms 后");
                    }
                    controller.sleep_while_running(Duration::from_millis(delay_ms));
                }
                Ok(false) => {
                    if !unfocused_logged {
                        println!("[跳过] 当前为前台模式，激战2不在前台激活状态，暂停发送...");
                        unfocused_logged = true;
                    }
                    controller.sleep_while_running(Duration::from_millis(300));
                }
                Err(SendError::AccessDenied) => {
                    if cfg.raw.auto_elevate && !already_elevated {
                        println!("[提权] 激战2正以管理员身份运行，正在请求管理员权限重新启动...");
                        match win32::relaunch_as_admin() {
                            Ok(()) => {
                                win32::request_quit();
                                break;
                            }
                            Err(e) => {
                                eprintln!("[错误] {e}。已暂停，请手动「以管理员身份运行」本程序或按 {} 重试。", cfg.raw.toggle_hotkey);
                                controller.set_running(false);
                            }
                        }
                    } else {
                        eprintln!(
                            "[错误] {}。已自动暂停，请确认以管理员身份运行（或改用前台模式 --mode foreground），按 {} 可重试。",
                            SendError::AccessDenied,
                            cfg.raw.toggle_hotkey
                        );
                        controller.set_running(false);
                    }
                }
                Err(SendError::Other(err)) => {
                    eprintln!("[错误] {err}");
                    cached_window = None;
                    controller.sleep_while_running(Duration::from_millis(1500));
                }
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;

    #[test]
    fn delay_without_jitter_is_exact() {
        assert_eq!(compute_delay_ms(1000, 0, 12345), 1000);
        assert_eq!(compute_delay_ms(0, 0, 12345), 1);
    }

    #[test]
    fn delay_with_jitter_stays_within_bounds() {
        for r in 0..1000 {
            let d = compute_delay_ms(1000, 200, r);
            assert!((800..=1200).contains(&d), "unexpected delay: {d}");
        }
        assert_eq!(compute_delay_ms(50, 200, 0), 1);
    }

    #[test]
    fn cooldown_scheduler_prioritizes_ready_skills_and_fills_with_skill_1() {
        let runtime = Config::default().validate().unwrap();
        let t0 = Instant::now();
        let mut sched = SkillScheduler::new(&runtime.plan, 0, t0);

        // Initial burst: Q (22s), 4 (13s), 3 (8s), 2 (5s), then filler 1 (0s).
        let (k, _, cd) = sched.next_action(t0).unwrap();
        assert_eq!((k.as_str(), cd), ("Q", Some(22_000)));

        let (k, _, cd) = sched.next_action(t0 + Duration::from_millis(400)).unwrap();
        assert_eq!((k.as_str(), cd), ("4", Some(13_000)));

        let (k, _, cd) = sched.next_action(t0 + Duration::from_millis(800)).unwrap();
        assert_eq!((k.as_str(), cd), ("3", Some(8_000)));

        let (k, _, cd) = sched.next_action(t0 + Duration::from_millis(1200)).unwrap();
        assert_eq!((k.as_str(), cd), ("2", Some(5_000)));

        // Between 1.6s and 6.0s, only skill 1 is ready.
        let (k, _, cd) = sched.next_action(t0 + Duration::from_millis(1600)).unwrap();
        assert_eq!((k.as_str(), cd), ("1", None));

        let (k, _, cd) = sched.next_action(t0 + Duration::from_millis(6000)).unwrap();
        assert_eq!((k.as_str(), cd), ("1", None));

        // At 6.2s (1200ms + 5000ms), skill 2 comes back off cooldown!
        let (k, _, cd) = sched.next_action(t0 + Duration::from_millis(6200)).unwrap();
        assert_eq!((k.as_str(), cd), ("2", Some(5_000)));

        // At 8.8s (800ms + 8000ms), skill 3 comes back off cooldown!
        let (k, _, cd) = sched.next_action(t0 + Duration::from_millis(8800)).unwrap();
        assert_eq!((k.as_str(), cd), ("3", Some(8_000)));

        // At 13.4s (400ms + 13000ms), skill 4 comes back off cooldown!
        let (k, _, cd) = sched.next_action(t0 + Duration::from_millis(13_400)).unwrap();
        assert_eq!((k.as_str(), cd), ("4", Some(13_000)));

        // At 22.0s (0ms + 22000ms), skill Q comes back off cooldown!
        let (k, _, cd) = sched.next_action(t0 + Duration::from_millis(22_000)).unwrap();
        assert_eq!((k.as_str(), cd), ("Q", Some(22_000)));
    }
}
