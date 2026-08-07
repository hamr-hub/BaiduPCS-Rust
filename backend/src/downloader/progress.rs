use std::collections::VecDeque;
use std::time::{Duration, Instant};

/// 速度分母下限（秒）。
///
/// 刚开始下载的头一小段时间里「已运行时长」还很小，若直接拿它当分母，
/// 一个分片落盘就会算出天文数字。取下限后起步阶段宁可低估也不虚高。
const MIN_SPEED_DENOM_SECS: f64 = 1.0;

/// 速度计算器（使用滑动窗口）
#[derive(Debug)]
pub struct SpeedCalculator {
    /// 数据点（时间，字节数）
    samples: VecDeque<(Instant, u64)>,
    /// 窗口大小（秒）
    window_size: Duration,
    /// 累计下载字节数
    total_bytes: u64,
    /// 第一个样本的时刻，用于起步阶段的分母（见 [`SpeedCalculator::speed`]）
    started_at: Option<Instant>,
}

impl SpeedCalculator {
    /// 创建新的速度计算器
    pub fn new(window_seconds: u64) -> Self {
        Self {
            samples: VecDeque::new(),
            window_size: Duration::from_secs(window_seconds),
            total_bytes: 0,
            started_at: None,
        }
    }

    /// 使用默认窗口大小（5秒）
    pub fn with_default_window() -> Self {
        Self::new(5)
    }

    /// 添加数据点
    pub fn add_sample(&mut self, bytes: u64) {
        let now = Instant::now();
        self.started_at.get_or_insert(now);
        self.total_bytes += bytes;
        self.samples.push_back((now, bytes));
        self.cleanup_old_samples(now);
    }

    /// 清理超出窗口的旧数据
    fn cleanup_old_samples(&mut self, now: Instant) {
        while let Some((timestamp, _)) = self.samples.front() {
            if now.duration_since(*timestamp) > self.window_size {
                self.samples.pop_front();
            } else {
                break;
            }
        }
    }

    /// 计算当前速度（字节/秒）
    ///
    /// 分母取**固定的窗口长度**（起步阶段取"已运行时长"），而不是"最早样本到现在"。
    ///
    /// 原实现是 `窗口内字节数 / (now - 最早样本时间)`，样本稀疏时分母会塌缩：
    /// 下载是按分片落盘的，`progress_callback` 并非匀速调用 —— 实测出现过
    /// 「连续 5 秒 downloaded 不动，然后一次跳 256KB」，那一刻窗口里只剩一两个
    /// 间隔几十毫秒的样本，算出 `256KB / 0.05s ≈ 5.2 MB/s`，而真实速率只有
    /// 约 65 KB/s。加上 `samples.len() < 2` 直接返回 0 的分支，界面上就表现为
    /// 速度在 0 / 285KB/s / 5.2MB/s 之间乱跳。
    ///
    /// 改为固定分母后，「窗口内传了多少字节」除以「窗口有多长」，正是该窗口的
    /// 平均速率，不会被单个分片的落盘瞬间放大。
    pub fn speed(&self) -> u64 {
        let Some(started) = self.started_at else {
            return 0;
        };
        let now = Instant::now();

        // 只统计**仍在窗口内**的样本。
        //
        // 不能直接 sum 全部：`cleanup_old_samples` 只在 `add_sample` 里调用，
        // 下载停下来之后没有新样本进来，过期样本永远不会被淘汰 —— 速度就会
        // 一直停在最后那个值不动（实测见过卡在 5.33 MB/s 好几秒）。
        let total_bytes: u64 = self
            .samples
            .iter()
            .filter(|(ts, _)| now.duration_since(*ts) <= self.window_size)
            .map(|(_, bytes)| bytes)
            .sum();
        if total_bytes == 0 {
            // 窗口内一个字节都没有 = 确实停了
            return 0;
        }

        // 起步阶段（运行时长还不足一个窗口）用已运行时长，否则用窗口长度。
        // 再兜一个下限，避免最开始几十毫秒内把一个分片算成天文数字。
        let elapsed = now.duration_since(started).as_secs_f64();
        let denom = elapsed
            .min(self.window_size.as_secs_f64())
            .max(MIN_SPEED_DENOM_SECS);

        (total_bytes as f64 / denom) as u64
    }

    /// 获取累计下载字节数
    pub fn total_bytes(&self) -> u64 {
        self.total_bytes
    }

    /// 格式化速度（返回人类可读的字符串）
    pub fn format_speed(&self) -> String {
        let speed = self.speed();
        format_bytes_per_second(speed)
    }

    /// 重置计算器
    pub fn reset(&mut self) {
        self.samples.clear();
        self.total_bytes = 0;
        // 必须一并清掉：否则复用这个计算器时「已运行时长」还是上一轮的，
        // 起步阶段会直接用满窗口做分母，把刚开始的一点点字节算成很低的速度。
        self.started_at = None;
    }
}

/// 格式化字节/秒
pub fn format_bytes_per_second(bytes_per_sec: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = 1024 * KB;
    const GB: u64 = 1024 * MB;

    if bytes_per_sec >= GB {
        format!("{:.2} GB/s", bytes_per_sec as f64 / GB as f64)
    } else if bytes_per_sec >= MB {
        format!("{:.2} MB/s", bytes_per_sec as f64 / MB as f64)
    } else if bytes_per_sec >= KB {
        format!("{:.2} KB/s", bytes_per_sec as f64 / KB as f64)
    } else {
        format!("{} B/s", bytes_per_sec)
    }
}

/// 格式化剩余时间
pub fn format_eta(seconds: u64) -> String {
    if seconds == 0 {
        return "即将完成".to_string();
    }

    let hours = seconds / 3600;
    let minutes = (seconds % 3600) / 60;
    let secs = seconds % 60;

    if hours > 0 {
        format!("{}小时{}分钟", hours, minutes)
    } else if minutes > 0 {
        format!("{}分钟{}秒", minutes, secs)
    } else {
        format!("{}秒", secs)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;

    #[test]
    fn test_speed_calculator_creation() {
        let calc = SpeedCalculator::new(5);
        assert_eq!(calc.total_bytes(), 0);
        assert_eq!(calc.speed(), 0);
    }

    #[test]
    fn test_add_sample() {
        let mut calc = SpeedCalculator::new(5);

        calc.add_sample(1024);
        assert_eq!(calc.total_bytes(), 1024);

        calc.add_sample(2048);
        assert_eq!(calc.total_bytes(), 3072);
    }

    /// 短时间内落盘一大块**不能**被算成天文速度。
    ///
    /// 这是实测踩到的坑：下载按分片落盘，`progress_callback` 并非匀速调用 ——
    /// 出现过「连续 5 秒 downloaded 不动，然后一次跳 256KB」，旧实现用
    /// 「最早样本到现在」当分母，算出 `256KB / 0.05s ≈ 5.2 MB/s`，
    /// 而真实速率只有约 65 KB/s。
    ///
    /// 注：本测试的前身断言的恰恰是旧行为（2MB/0.1s 要 >10MB/s），
    /// 那是把瞬时写盘爆发当成了下载速率。
    #[test]
    fn test_burst_write_is_not_reported_as_huge_speed() {
        let mut calc = SpeedCalculator::new(5);

        calc.add_sample(1024 * 1024); // 1MB
        thread::sleep(Duration::from_millis(100));
        calc.add_sample(1024 * 1024); // 1MB

        let speed = calc.speed();
        // 分母有 1 秒下限：2MB / 1s = 2MB/s，绝不该是 20MB/s
        assert!(
            speed <= 2 * 1024 * 1024,
            "0.1 秒内落盘 2MB 不应被算成 {} B/s（分母塌缩）",
            speed
        );
        assert!(speed > 0, "有字节进来就该有速度");
    }

    /// 窗口内的平均速率：分母是窗口长度，不随样本疏密漂移。
    #[test]
    fn test_speed_uses_window_as_denominator() {
        // 窗口 1 秒，方便测试
        let mut calc = SpeedCalculator::new(1);
        calc.add_sample(100_000);
        // 跑满一个窗口后，分母应为窗口长度而非「到现在的时长」
        thread::sleep(Duration::from_millis(1100));
        calc.add_sample(100_000);

        let speed = calc.speed();
        // 第一个样本已被窗口淘汰，窗口内只剩 100_000 字节；
        // 分母取 max(窗口 1s, 下限 1s) = 1s → 约 100 KB/s
        assert!(
            (50_000..=150_000).contains(&speed),
            "窗口平均速率应在 100KB/s 量级，实际 {}",
            speed
        );
    }

    /// 窗口内没有任何字节 = 确实停了，速度为 0（而不是沿用旧值）。
    #[test]
    fn test_speed_zero_when_window_empty() {
        let mut calc = SpeedCalculator::new(1);
        calc.add_sample(500_000);
        thread::sleep(Duration::from_millis(1200));
        // 不再有新样本，窗口内已被清空
        assert_eq!(calc.speed(), 0);
    }

    #[test]
    fn test_reset() {
        let mut calc = SpeedCalculator::new(5);

        calc.add_sample(1024);
        calc.add_sample(2048);
        assert_eq!(calc.total_bytes(), 3072);

        calc.reset();
        assert_eq!(calc.total_bytes(), 0);
        assert_eq!(calc.speed(), 0);
    }

    #[test]
    fn test_format_speed() {
        assert_eq!(format_bytes_per_second(500), "500 B/s");
        assert_eq!(format_bytes_per_second(1024), "1.00 KB/s");
        assert_eq!(format_bytes_per_second(1024 * 1024), "1.00 MB/s");
        assert_eq!(format_bytes_per_second(1024 * 1024 * 1024), "1.00 GB/s");
    }

    #[test]
    fn test_format_eta() {
        assert_eq!(format_eta(0), "即将完成");
        assert_eq!(format_eta(30), "30秒");
        assert_eq!(format_eta(90), "1分钟30秒");
        assert_eq!(format_eta(3661), "1小时1分钟");
    }
}
