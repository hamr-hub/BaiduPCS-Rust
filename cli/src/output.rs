// 输出模型：人类可读 vs JSON。
//
// 设计：
// - 人类模式：进度走 stderr（eprintln! 配合 carriage-return 覆盖行）；
//             最终结果走 stdout，调用方自己 Display。
// - JSON 模式：进度走 stderr NDJSON（事件流，方便日志收集）；
//             最终结果走 stdout，单行 JSON。
// - -q 模式：完全抑制 stderr；stdout 只输出最终结果。

use std::io::Write;

use serde::Serialize;
use serde_json::json;

/// 输出模式
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputMode {
    Human,
    Json,
}

/// 进度 / 结果打印机。线程安全（内部无状态）。
#[derive(Debug, Clone, Copy)]
pub struct Printer {
    pub mode: OutputMode,
    pub quiet: bool,
}

impl Printer {
    pub const fn new(mode: OutputMode, quiet: bool) -> Self {
        Self { mode, quiet }
    }

    /// 输出一行进度（stderr，覆盖同一行）
    pub fn progress(&self, msg: impl AsRef<str>) {
        if self.quiet {
            return;
        }
        match self.mode {
            OutputMode::Json => {
                let line = json!({"event": "progress", "msg": msg.as_ref()});
                eprintln!("{line}");
            }
            OutputMode::Human => {
                let mut stderr = std::io::stderr().lock();
                let _ = write!(stderr, "\r\x1b[2K⏳ {}", msg.as_ref());
                let _ = stderr.flush();
            }
        }
    }

    /// 进度结束（覆盖行 → 换行，避免下次输出粘连）
    pub fn progress_end(&self) {
        if self.quiet || self.mode == OutputMode::Json {
            return;
        }
        eprintln!();
    }

    /// 输出最终结果（stdout）。T 必须可 Serialize。
    pub fn result_json<T: Serialize>(&self, value: &T) {
        match self.mode {
            OutputMode::Json | OutputMode::Human => {
                match serde_json::to_string_pretty(value) {
                    Ok(s) => println!("{s}"),
                    Err(e) => eprintln!("warning: serialize result failed: {e}"),
                }
            }
        }
    }

    /// 输出人类可读的最终结果（一行字符串）
    pub fn result_human(&self, line: impl AsRef<str>) {
        match self.mode {
            OutputMode::Human => println!("{}", line.as_ref()),
            OutputMode::Json => {
                let line = json!({"event": "result", "msg": line.as_ref()});
                println!("{line}");
            }
        }
    }

    /// 成功事件（仅 JSON 模式有意义；人类模式与 result 一致）
    pub fn event<T: Serialize>(&self, event: &str, payload: T) {
        if self.quiet {
            return;
        }
        match self.mode {
            OutputMode::Json => {
                let line = json!({"event": event, "data": payload});
                eprintln!("{line}");
            }
            OutputMode::Human => {
                // 人类模式事件折叠到 progress（一般用于"任务已入队"这种瞬时信号）
                // 此处保持沉默，避免重复噪音
                let _ = event;
            }
        }
    }
}

/// 字节数人类可读
pub fn human_bytes(n: u64) -> String {
    const UNITS: &[&str] = &["B", "KB", "MB", "GB", "TB", "PB"];
    let mut v = n as f64;
    let mut u = 0;
    while v >= 1024.0 && u < UNITS.len() - 1 {
        v /= 1024.0;
        u += 1;
    }
    if u == 0 {
        format!("{} {}", n, UNITS[0])
    } else {
        format!("{:.2} {}", v, UNITS[u])
    }
}

/// 速度人类可读（字节/秒）
pub fn human_speed(bps: u64) -> String {
    format!("{}/s", human_bytes(bps))
}

/// 秒数 → "1h 23m 12s" / "12s"
pub fn human_duration(secs: u64) -> String {
    if secs < 60 {
        return format!("{secs}s");
    }
    let m = secs / 60;
    let s = secs % 60;
    if m < 60 {
        return format!("{m}m {s}s");
    }
    let h = m / 60;
    let m = m % 60;
    format!("{h}h {m}m {s}s")
}

/// 简易进度条（10 格）
pub fn bar(percent: f64) -> String {
    let pct = percent.clamp(0.0, 100.0);
    let filled = ((pct / 10.0).round() as usize).min(10);
    let mut s = String::with_capacity(10);
    for i in 0..10 {
        s.push(if i < filled { '▰' } else { '▱' });
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_human_bytes() {
        assert_eq!(human_bytes(0), "0 B");
        assert_eq!(human_bytes(1023), "1023 B");
        assert_eq!(human_bytes(1024), "1.00 KB");
        assert_eq!(human_bytes(1024 * 1024), "1.00 MB");
        assert_eq!(human_bytes(1024 * 1024 * 1024), "1.00 GB");
    }

    #[test]
    fn test_bar() {
        assert_eq!(bar(0.0), "▱▱▱▱▱▱▱▱▱▱");
        assert_eq!(bar(50.0), "▰▰▰▰▰▱▱▱▱▱");
        assert_eq!(bar(100.0), "▰▰▰▰▰▰▰▰▰▰");
        assert_eq!(bar(150.0), "▰▰▰▰▰▰▰▰▰▰"); // clamp
    }

    #[test]
    fn test_human_duration() {
        assert_eq!(human_duration(0), "0s");
        assert_eq!(human_duration(59), "59s");
        assert_eq!(human_duration(60), "1m 0s");
        assert_eq!(human_duration(3661), "1h 1m 1s");
    }

    #[test]
    fn test_printer_quiet_suppresses_progress() {
        let p = Printer::new(OutputMode::Human, true);
        // 不会有 panic；只验证调用不报错。
        p.progress("test");
        p.progress_end();
    }

    #[test]
    fn test_printer_json_serializes() {
        let p = Printer::new(OutputMode::Json, false);
        p.result_json(&serde_json::json!({"ok": true}));
        p.event("task_enqueued", serde_json::json!({"id": "abc"}));
    }
}
