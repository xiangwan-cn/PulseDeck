use std::time::Duration;

use tokio::io::{AsyncRead, AsyncReadExt};
use tokio_util::sync::CancellationToken;

pub struct CommandOutput {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: i32,
    pub success: bool,
}

/// Run a command with an event-driven cancellation token.
pub async fn run_command_with_cancellation(
    program: &str,
    args: &[String],
    timeout_secs: u64,
    max_output: usize,
    cancellation: CancellationToken,
) -> Result<CommandOutput, String> {
    if cancellation.is_cancelled() {
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
    let wait = tokio::select! {
        result = execution => CommandWait::Completed(result),
        _ = cancellation.cancelled() => CommandWait::Cancelled,
        _ = tokio::time::sleep(Duration::from_secs(timeout_secs.max(1))) => CommandWait::TimedOut,
    };
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
            for next in chars.by_ref() {
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
    use tokio_util::sync::CancellationToken;

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
        let cancellation = CancellationToken::new();
        let signal = cancellation.clone();
        let task = tokio::spawn(async move {
            run_command_with_cancellation(
                "sh",
                &["-c".to_string(), "sleep 30".to_string()],
                30,
                1024,
                signal,
            )
            .await
        });
        tokio::time::sleep(Duration::from_millis(50)).await;
        cancellation.cancel();
        let result = tokio::time::timeout(Duration::from_secs(2), task)
            .await
            .expect("command cancellation timed out")
            .expect("command task panicked");
        assert!(result.is_err());
    }
}
