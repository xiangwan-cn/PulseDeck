use std::time::Duration;

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
    crate::core::power_debug::increment(crate::core::power_debug::Counter::ExternalProcess);
    let mut child = tokio::process::Command::new(program)
        .args(args)
        .env("LANG", "en_US.UTF-8")
        .env("LC_ALL", "en_US.UTF-8")
        .env("PYTHONIOENCODING", "utf-8")
        .kill_on_drop(true)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
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

    let (stdout, stderr, status) =
        match tokio::time::timeout(Duration::from_secs(timeout_secs.max(1)), execution).await {
            Ok(result) => result.map_err(|error| format!("命令执行失败: {error}"))?,
            Err(_) => {
                let _ = child.kill().await;
                let _ = child.wait().await;
                return Err("命令执行超时".into());
            }
        };

    Ok(CommandOutput {
        stdout: clean_output(stdout),
        stderr: clean_output(stderr),
        exit_code: status.code().unwrap_or(-1),
        success: status.success(),
    })
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
}
