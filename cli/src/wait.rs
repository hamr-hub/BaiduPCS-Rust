// 轮询到终态的帮助函数。
//
// 核心库没有 wait_for_completion —— 只有 get_task / get_all_tasks。
// 这里包装一个轮询循环，超时返回 Err，结束时返回最终 task 快照。

use std::time::{Duration, Instant};

use baidu_netdisk_rust::{
    DownloadManager, DownloadTask, TaskStatus,
    TransferManager, TransferStatus, TransferTask,
    UploadManager, UploadTask, UploadTaskStatus,
};

use crate::error::{CliError, CliResult};
use crate::output::{Printer, bar, human_bytes, human_duration, human_speed};

/// 等待单个下载任务到终态。
///
/// - `deadline = 0` 表示无限等待
/// - 任务从 manager 内存消失 → TaskNotFound
/// - TaskStatus::Completed → Ok
/// - TaskStatus::Failed → Err TaskFailed
pub async fn for_download(
    mgr: &DownloadManager,
    task_id: &str,
    printer: &Printer,
    deadline_s: u64,
) -> CliResult<DownloadTask> {
    let start = Instant::now();
    let mut last_progress: Option<String> = None;
    loop {
        let Some(task) = mgr.get_task(task_id).await else {
            return Err(CliError::TaskNotFound(task_id.to_string()));
        };

        if !printer.quiet {
            let msg = format_download_progress(&task);
            if last_progress.as_deref() != Some(msg.as_str()) {
                printer.progress(msg.clone());
                last_progress = Some(msg);
            }
        }

        match task.status {
            TaskStatus::Completed => {
                printer.progress_end();
                return Ok(task);
            }
            TaskStatus::Failed => {
                printer.progress_end();
                return Err(CliError::TaskFailed(
                    task.error.unwrap_or_else(|| "未知错误".to_string()),
                ));
            }
            _ => {}
        }

        if deadline_s > 0 && start.elapsed().as_secs() > deadline_s {
            printer.progress_end();
            return Err(CliError::TaskTimeout(task_id.to_string()));
        }

        tokio::time::sleep(Duration::from_millis(500)).await;
    }
}

/// 等待单个上传任务到终态。
pub async fn for_upload(
    mgr: &UploadManager,
    task_id: &str,
    printer: &Printer,
    deadline_s: u64,
) -> CliResult<UploadTask> {
    let start = Instant::now();
    let mut last_progress: Option<String> = None;
    loop {
        let Some(task) = mgr.get_task(task_id).await else {
            return Err(CliError::TaskNotFound(task_id.to_string()));
        };

        if !printer.quiet {
            let msg = format_upload_progress(&task);
            if last_progress.as_deref() != Some(msg.as_str()) {
                printer.progress(msg.clone());
                last_progress = Some(msg);
            }
        }

        match task.status {
            UploadTaskStatus::Completed | UploadTaskStatus::RapidUploadSuccess => {
                printer.progress_end();
                return Ok(task);
            }
            UploadTaskStatus::Failed => {
                printer.progress_end();
                let msg = task
                    .failure_reason
                    .clone()
                    .or(task.error.clone())
                    .unwrap_or_else(|| "未知错误".to_string());
                return Err(CliError::TaskFailed(msg));
            }
            _ => {}
        }

        if deadline_s > 0 && start.elapsed().as_secs() > deadline_s {
            printer.progress_end();
            return Err(CliError::TaskTimeout(task_id.to_string()));
        }

        tokio::time::sleep(Duration::from_millis(500)).await;
    }
}

/// 等待单个转存任务到终态。
pub async fn for_transfer(
    mgr: &TransferManager,
    task_id: &str,
    printer: &Printer,
    deadline_s: u64,
) -> CliResult<TransferTask> {
    let start = Instant::now();
    loop {
        let Some(task) = mgr.get_task(task_id).await else {
            return Err(CliError::TaskNotFound(task_id.to_string()));
        };

        if !printer.quiet {
            let pct = if task.total_count > 0 {
                task.transferred_count as f64 * 100.0 / task.total_count as f64
            } else {
                0.0
            };
            let msg = format!(
                "[{}] {} {}/{} files",
                task.id,
                task.status.description(),
                task.transferred_count,
                task.total_count
            );
            printer.progress(format!("{msg} {}", bar(pct)));
        }

        if task.status.is_terminal() {
            printer.progress_end();
            match task.status {
                TransferStatus::TransferFailed | TransferStatus::DownloadFailed => {
                    return Err(CliError::TaskFailed(
                        task.error.unwrap_or_else(|| "转存失败".to_string()),
                    ));
                }
                _ => return Ok(task),
            }
        }

        if deadline_s > 0 && start.elapsed().as_secs() > deadline_s {
            printer.progress_end();
            return Err(CliError::TaskTimeout(task_id.to_string()));
        }

        tokio::time::sleep(Duration::from_millis(1000)).await;
    }
}

fn format_download_progress(t: &DownloadTask) -> String {
    let pct = t.progress();
    let total = human_bytes(t.total_size);
    let done = human_bytes(t.downloaded_size);
    let speed = if t.speed > 0 { format!(" {}", human_speed(t.speed)) } else { String::new() };
    let eta = t.eta().map(|s| format!(" ETA {}", human_duration(s))).unwrap_or_default();
    format!(
        "[{}] {} {}% {}/{} {}{}{}",
        short_id(&t.id),
        t.status.status_label(),
        pct as u32,
        done,
        total,
        bar(pct),
        speed,
        eta,
    )
}

fn format_upload_progress(t: &UploadTask) -> String {
    let pct = t.progress();
    let total = human_bytes(t.total_size);
    let done = human_bytes(t.uploaded_size);
    let speed = if t.speed > 0 { format!(" {}", human_speed(t.speed)) } else { String::new() };
    let eta = t.eta().map(|s| format!(" ETA {}", human_duration(s))).unwrap_or_default();
    format!(
        "[{}] {} {}% {}/{} {}{}{}",
        short_id(&t.id),
        t.status.status_label(),
        pct as u32,
        done,
        total,
        bar(pct),
        speed,
        eta,
    )
}

fn short_id(id: &str) -> String {
    id.chars().take(8).collect()
}

/// 给人类模式用：把状态枚举翻译成简短中文。
trait StatusLabel {
    fn status_label(&self) -> &'static str;
}

impl StatusLabel for TaskStatus {
    fn status_label(&self) -> &'static str {
        match self {
            TaskStatus::Pending => "等待中",
            TaskStatus::Downloading => "下载中",
            TaskStatus::Decrypting => "解密中",
            TaskStatus::Paused => "已暂停",
            TaskStatus::Completed => "已完成",
            TaskStatus::Failed => "失败",
        }
    }
}

impl StatusLabel for UploadTaskStatus {
    fn status_label(&self) -> &'static str {
        match self {
            UploadTaskStatus::Pending => "等待中",
            UploadTaskStatus::CheckingRapid => "秒传校验",
            UploadTaskStatus::Encrypting => "加密中",
            UploadTaskStatus::Uploading => "上传中",
            UploadTaskStatus::Paused => "已暂停",
            UploadTaskStatus::Completed => "已完成",
            UploadTaskStatus::RapidUploadSuccess => "秒传成功",
            UploadTaskStatus::Failed => "失败",
        }
    }
}

// 让 TransferStatus.description() 直接复用（已在 transfer/task.rs 中定义）。
// 我们仅在 is_terminal 之后分支，不需要单独 label。
