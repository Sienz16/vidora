use std::{io, path::Path, process::Stdio, time::Duration};

use tokio::{
    io::{AsyncRead, AsyncReadExt},
    process::Command,
    time::timeout,
};

use crate::Error;

const MAX_TOOL_OUTPUT: u64 = 256 * 1024;
const MAX_CHILD_FILE: libc::rlim_t = 20_000_000;

async fn capture(reader: impl AsyncRead + Unpin) -> Result<Vec<u8>, Error> {
    let mut bytes = Vec::new();
    reader
        .take(MAX_TOOL_OUTPUT + 1)
        .read_to_end(&mut bytes)
        .await?;
    if bytes.len() as u64 > MAX_TOOL_OUTPUT {
        return Err(Error::ToolOutput);
    }
    Ok(bytes)
}

pub(crate) async fn run(
    executable: &Path,
    directory: &Path,
    args: &[String],
    limit: Duration,
    failure: Error,
) -> Result<Vec<u8>, Error> {
    let mut command = Command::new(executable);
    command
        .args(args)
        .env_clear()
        .env("LC_ALL", "C")
        .current_dir(directory)
        .kill_on_drop(true)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let cpu_seconds = limit.as_secs().max(1) as libc::rlim_t;
    // Only async-signal-safe operations may run between fork and exec.
    unsafe {
        command.pre_exec(move || {
            for (resource, value) in [
                (libc::RLIMIT_FSIZE, MAX_CHILD_FILE),
                (libc::RLIMIT_CPU, cpu_seconds),
                #[cfg(target_os = "linux")]
                (libc::RLIMIT_AS, 2 * 1024 * 1024 * 1024),
            ] {
                let limit = libc::rlimit {
                    rlim_cur: value,
                    rlim_max: value,
                };
                if libc::setrlimit(resource, &limit) != 0 {
                    return Err(io::Error::last_os_error());
                }
            }
            Ok(())
        });
    }

    let mut child = command.spawn().map_err(|_| Error::ToolUnavailable)?;
    let stdout = child.stdout.take().expect("piped stdout");
    let stderr = child.stderr.take().expect("piped stderr");
    match timeout(limit, async {
        tokio::try_join!(capture(stdout), capture(stderr), async {
            Ok(child.wait().await?)
        })
    })
    .await
    {
        Ok(Ok((out, err, status))) if status.success() && err.is_empty() => Ok(out),
        Ok(Ok(_)) => Err(failure),
        Ok(Err(error)) => {
            let _ = child.kill().await;
            Err(error)
        }
        Err(_) => {
            let _ = child.kill().await;
            Err(Error::Timeout)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn kills_timed_out_child() {
        let dir = tempfile::tempdir().unwrap();
        let result = run(
            Path::new("/bin/sleep"),
            dir.path(),
            &["30".into()],
            Duration::from_millis(50),
            Error::Decode,
        )
        .await;
        assert!(matches!(result, Err(Error::Timeout)));
    }

    #[tokio::test]
    async fn bounds_tool_output() {
        assert!(matches!(
            capture(tokio::io::repeat(0)).await,
            Err(Error::ToolOutput)
        ));
    }
}
