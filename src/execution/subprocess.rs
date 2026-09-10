use std::{
    future::Future,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::Duration,
};

use tokio::io::{AsyncRead, AsyncReadExt};

pub struct CommandOutput {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: i32,
    pub success: bool,
}

pub async fn run_command(
    program: &str,
    args: &[String],
    timeout_secs: u64,
    max_output: usize,
) -> Result<CommandOutput, String> {
    run_command_inner(program, args, timeout_secs, max_output, None).await
}

pub async fn run_command_with_shutdown(
    program: &str,
    args: &[String],
    timeout_secs: u64,
    max_output: usize,
    shutdown: Arc<AtomicBool>,
) -> Result<CommandOutput, String> {
    run_command_inner(program, args, timeout_secs, max_output, Some(shutdown)).await
}

async fn run_command_inner(
    program: &str,
    args: &[String],
    timeout_secs: u64,
    max_output: usize,
    shutdown: Option<Arc<AtomicBool>>,
) -> Result<CommandOutput, String> {
    if shutdown
        .as_ref()
        .is_some_and(|signal| signal.load(Ordering::Acquire))
    {
        return Err("命令因应用关闭而取消".into());
    }
    crate::core::power_debug::increment(crate::core::power_debug::Counter::ExternalProcess);
    let mut command = tokio::process::Command::new(program);
    command
        .args(args)
        .env("LANG", "en_US.UTF-8")
        .env("LC_ALL", "en_US.UTF-8")
        .env("PYTHONIOENCODING", "utf-8")
        .kill_on_drop(true)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    #[cfg(unix)]
    command.process_group(0);
    let mut child = command
        .spawn()
        .map_err(|error| format!("启动失败: {error}"))?;

    let stdout = child.stdout.take().ok_or("无法读取标准输出")?;
    let stderr = child.stderr.take().ok_or("无法读取标准错误")?;
    let execution = async {
        let (stdout, stderr, status) = tokio::try_join!(
            read_limited(stdout, max_output),
            read_limited(stderr, max_output),
            child.wait(),
        )?;
        Ok::<_, std::io::Error>((stdout, stderr, status))
    };

    let wait = await_command(
        execution,
        Duration::from_secs(timeout_secs.max(1)),
        shutdown,
    )
    .await;
    let (stdout, stderr, status) = match wait {
        CommandWait::Completed(Ok(result)) => result,
        CommandWait::Completed(Err(error)) => {
            terminate_child(&mut child).await;
            return Err(format!("命令执行失败: {error}"));
        }
        CommandWait::TimedOut => {
            terminate_child(&mut child).await;
            return Err("命令执行超时".into());
        }
        CommandWait::Cancelled => {
            terminate_child(&mut child).await;
            return Err("命令因应用关闭而取消".into());
        }
    };

    Ok(CommandOutput {
        stdout: clean_output(stdout),
        stderr: clean_output(stderr),
        exit_code: status.code().unwrap_or(-1),
        success: status.success(),
    })
}

enum CommandWait<T> {
    Completed(T),
    TimedOut,
    Cancelled,
}

async fn await_command<F, T>(
    future: F,
    timeout: Duration,
    shutdown: Option<Arc<AtomicBool>>,
) -> CommandWait<T>
where
    F: Future<Output = T>,
{
    if let Some(shutdown) = shutdown {
        tokio::pin!(future);
        tokio::select! {
            result = &mut future => CommandWait::Completed(result),
            _ = wait_for_shutdown(shutdown) => CommandWait::Cancelled,
            _ = tokio::time::sleep(timeout) => CommandWait::TimedOut,
        }
    } else {
        match tokio::time::timeout(timeout, future).await {
            Ok(result) => CommandWait::Completed(result),
            Err(_) => CommandWait::TimedOut,
        }
    }
}

async fn wait_for_shutdown(shutdown: Arc<AtomicBool>) {
    while !shutdown.load(Ordering::Acquire) {
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

async fn terminate_child(child: &mut tokio::process::Child) {
    #[cfg(unix)]
    {
        if let Some(pid) = child.id() {
            // Commands such as `sh -c`, npx, or Python commonly create
            // descendants. Killing only the direct child leaves those
            // descendants holding pipes and CPU after a timeout.
            let process_group = -(pid as libc::pid_t);
            unsafe {
                let _ = libc::kill(process_group, libc::SIGTERM);
            }
            let _ = tokio::time::timeout(Duration::from_millis(250), child.wait()).await;
            unsafe {
                let _ = libc::kill(process_group, libc::SIGKILL);
            }
            let _ = child.wait().await;
        }
    }
    #[cfg(not(unix))]
    {
        let _ = child.kill().await;
        let _ = child.wait().await;
    }
}

async fn read_limited<R: AsyncRead + Unpin>(
    mut reader: R,
    limit: usize,
) -> std::io::Result<Vec<u8>> {
    let limit = limit.max(1);
    let mut kept = Vec::with_capacity(limit.min(8192));
    let mut buffer = [0_u8; 4096];
    loop {
        let read = reader.read(&mut buffer).await?;
        if read == 0 {
            break;
        }
        let remaining = limit.saturating_sub(kept.len());
        kept.extend_from_slice(&buffer[..read.min(remaining)]);
        // Continue draining after the limit so the child cannot block on a full
        // pipe, but never retain more than the configured amount in memory.
    }
    Ok(kept)
}

fn clean_output(bytes: Vec<u8>) -> String {
    let input = String::from_utf8_lossy(&bytes);
    let mut output = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();
    while let Some(character) = chars.next() {
        if character == '\u{1b}' && chars.peek() == Some(&'[') {
            chars.next();
            while let Some(next) = chars.next() {
                if ('\u{40}'..='\u{7e}').contains(&next) {
                    break;
                }
            }
        } else {
            output.push(character);
        }
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preserves_unicode_while_removing_ansi() {
        assert_eq!(
            clean_output("中文\u{1b}[31m红色\u{1b}[0m".as_bytes().to_vec()),
            "中文红色"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn shutdown_cancels_command_process_group() {
        let shutdown = Arc::new(AtomicBool::new(false));
        let signal = shutdown.clone();
        let task = tokio::spawn(async move {
            run_command_with_shutdown(
                "sh",
                &["-c".to_string(), "sleep 30".to_string()],
                30,
                1024,
                signal,
            )
            .await
        });
        tokio::time::sleep(Duration::from_millis(50)).await;
        shutdown.store(true, Ordering::Release);
        let result = tokio::time::timeout(Duration::from_secs(2), task)
            .await
            .expect("command cancellation timed out")
            .expect("command task panicked");
        assert!(result.is_err());
    }
}
