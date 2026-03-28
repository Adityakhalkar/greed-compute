use std::path::PathBuf;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::mpsc;
use tokio::time::{timeout, Duration};

#[derive(Debug, Clone, serde::Serialize)]
pub struct ExecutionResult {
    pub stdout: String,
    pub result: Option<String>,
    pub error: Option<String>,
    pub duration_ms: u64,
    /// Base64-encoded PNG images captured from matplotlib plt.show() calls
    pub plots: Vec<String>,
    /// HTML string when the last expression was a DataFrame or Series
    pub html: Option<String>,
}

#[derive(serde::Serialize)]
struct WorkerCommand {
    #[serde(rename = "type")]
    cmd_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    stream: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    path: Option<String>,
}

pub struct PythonRuntime {
    child: Child,
    stdin: tokio::process::ChildStdin,
    reader: BufReader<tokio::process::ChildStdout>,
}

fn nsjail_available() -> bool {
    std::process::Command::new("which")
        .arg("nsjail")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn spawn_direct(
    workspace: &PathBuf,
    worker_path: &str,
    python_path: &str,
) -> Result<tokio::process::Child, String> {
    Command::new(python_path)
        .arg(worker_path)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .env("GREED_WORKSPACE", workspace.to_string_lossy().as_ref())
        .env("GREED_MAX_MEMORY_MB", "512")
        .env("GREED_MAX_CPU_SECONDS", "30")
        .env("GREED_PRELOAD", "1")
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| format!("Failed to spawn Python worker: {}", e))
}

fn spawn_jailed(
    workspace: &PathBuf,
    worker_path: &str,
    python_path: &str,
) -> Result<tokio::process::Child, String> {
    let workspace_str = workspace.to_string_lossy().to_string();

    // nsjail 3.4: network isolation is ON by default (CLONE_NEWNET).
    // No chroot needed — Python-level import hooks handle filesystem access.
    // We get: isolated network namespace, hidden /proc, kernel resource limits.
    let mut cmd = Command::new("nsjail");
    cmd
        .args(["--mode", "o"])
        .args(["--log", "/dev/null"])
        // Hide /proc — workers can't enumerate PIDs or read system info
        .arg("--disable_proc")
        // Resource limits
        .args(["--max_cpus", "1"])
        .args(["--rlimit_as", "1024"])       // 1 GB virtual memory
        .args(["--rlimit_nofile", "128"])
        .args(["--rlimit_nproc", "64"])
        .args(["--rlimit_fsize", "inf"])     // allow writing .pyc cache files
        .args(["--rlimit_cpu", "inf"])       // session TTL manages lifetime
        .args(["--time_limit", "0"])
        // Pass required env vars explicitly (nsjail clears parent env)
        .args(["--env", "GREED_PRELOAD=1"])
        .args(["--env", "GREED_MAX_MEMORY_MB=512"])
        .args(["--env", "GREED_MAX_CPU_SECONDS=30"])
        .args(["--env", &format!("GREED_WORKSPACE={}", workspace_str)])
        .args(["--env", "PYTHONDONTWRITEBYTECODE=1"])
        .args(["--env", &format!("HOME={}", workspace_str)])
        .arg("--")
        .arg(python_path)
        .arg(worker_path)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true);

    tracing::info!("Spawning worker under nsjail");
    cmd.spawn().map_err(|e| format!("Failed to spawn jailed Python worker: {}", e))
}

impl PythonRuntime {
    pub async fn spawn(
        workspace: &PathBuf,
        worker_path: &str,
        python_path: &str,
    ) -> Result<Self, String> {
        let mut child = if nsjail_available() {
            spawn_jailed(workspace, worker_path, python_path)?
        } else {
            spawn_direct(workspace, worker_path, python_path)?
        };

        let stdin = child.stdin.take().ok_or("Failed to capture worker stdin")?;
        let stdout = child
            .stdout
            .take()
            .ok_or("Failed to capture worker stdout")?;
        let mut reader = BufReader::new(stdout);

        // Wait for {"type": "ready"} with 30s timeout
        let mut line = String::new();
        let read_result = timeout(Duration::from_secs(30), reader.read_line(&mut line)).await;

        match read_result {
            Ok(Ok(0)) => return Err("Worker process exited before sending ready signal".into()),
            Ok(Ok(_)) => {
                let msg: serde_json::Value = serde_json::from_str(line.trim())
                    .map_err(|e| format!("Invalid ready message from worker: {}", e))?;
                if msg.get("type").and_then(|v| v.as_str()) != Some("ready") {
                    return Err(format!(
                        "Expected ready signal from worker, got: {}",
                        line.trim()
                    ));
                }
            }
            Ok(Err(e)) => return Err(format!("IO error reading worker ready signal: {}", e)),
            Err(_) => return Err("Timeout waiting for worker ready signal (30s)".into()),
        }

        Ok(Self {
            child,
            stdin,
            reader,
        })
    }

    pub async fn execute(&mut self, code: &str) -> ExecutionResult {
        let cmd = WorkerCommand {
            cmd_type: "execute".to_string(),
            code: Some(code.to_string()),
            stream: Some(false),
            path: None,
        };

        let mut cmd_json =
            serde_json::to_string(&cmd).unwrap_or_else(|_| r#"{"type":"execute","code":""}"#.to_string());
        cmd_json.push('\n');

        // Send command to worker
        if let Err(e) = self.stdin.write_all(cmd_json.as_bytes()).await {
            return ExecutionResult {
                stdout: String::new(),
                result: None,
                error: Some(format!("Failed to write to worker stdin: {}", e)),
                duration_ms: 0,
                plots: vec![],
                html: None,
            };
        }
        if let Err(e) = self.stdin.flush().await {
            return ExecutionResult {
                stdout: String::new(),
                result: None,
                error: Some(format!("Failed to flush worker stdin: {}", e)),
                duration_ms: 0,
                plots: vec![],
                html: None,
            };
        }

        // Read response with 35s timeout
        let mut line = String::new();
        let read_result = timeout(Duration::from_secs(35), self.reader.read_line(&mut line)).await;

        match read_result {
            Ok(Ok(0)) => ExecutionResult {
                stdout: String::new(),
                result: None,
                error: Some("Worker process died during execution".to_string()),
                duration_ms: 0,
                plots: vec![],
                html: None,
            },
            Ok(Ok(_)) => {
                match serde_json::from_str::<serde_json::Value>(line.trim()) {
                    Ok(msg) => ExecutionResult {
                        stdout: msg
                            .get("stdout")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string(),
                        result: msg
                            .get("result")
                            .and_then(|v| v.as_str())
                            .map(|s| s.to_string()),
                        error: msg
                            .get("error")
                            .and_then(|v| {
                                if v.is_null() {
                                    None
                                } else {
                                    v.as_str().map(|s| s.to_string())
                                }
                            }),
                        duration_ms: msg
                            .get("duration_ms")
                            .and_then(|v| v.as_u64())
                            .unwrap_or(0),
                        plots: msg
                            .get("plots")
                            .and_then(|v| v.as_array())
                            .map(|arr| {
                                arr.iter()
                                    .filter_map(|v| v.as_str().map(|s| s.to_string()))
                                    .collect()
                            })
                            .unwrap_or_default(),
                        html: msg
                            .get("html")
                            .and_then(|v| {
                                if v.is_null() { None } else { v.as_str().map(|s| s.to_string()) }
                            }),
                    },
                    Err(e) => ExecutionResult {
                        stdout: String::new(),
                        result: None,
                        error: Some(format!("Invalid JSON from worker: {}", e)),
                        duration_ms: 0,
                        plots: vec![],
                        html: None,
                    },
                }
            }
            Ok(Err(e)) => ExecutionResult {
                stdout: String::new(),
                result: None,
                error: Some(format!("IO error reading from worker: {}", e)),
                duration_ms: 0,
                plots: vec![],
                html: None,
            },
            Err(_) => ExecutionResult {
                stdout: String::new(),
                result: None,
                error: Some("Execution timed out (35s)".to_string()),
                duration_ms: 35_000,
                plots: vec![],
                html: None,
            },
        }
    }

    /// Streaming execution — sends {"type":"stream"} lines to `tx` as they arrive,
    /// then returns the final ExecutionResult when {"type":"result"} is received.
    pub async fn execute_streaming(
        &mut self,
        code: &str,
        tx: mpsc::Sender<String>,
    ) -> ExecutionResult {
        let cmd = WorkerCommand {
            cmd_type: "execute".to_string(),
            code: Some(code.to_string()),
            stream: Some(true),
            path: None,
        };

        let mut cmd_json = serde_json::to_string(&cmd)
            .unwrap_or_else(|_| r#"{"type":"execute","code":"","stream":true}"#.to_string());
        cmd_json.push('\n');

        if self.stdin.write_all(cmd_json.as_bytes()).await.is_err()
            || self.stdin.flush().await.is_err()
        {
            return ExecutionResult {
                stdout: String::new(),
                result: None,
                error: Some("Failed to send execute command".into()),
                duration_ms: 0,
                plots: vec![],
                html: None,
            };
        }

        // Read lines until we get {"type":"result"}. Intermediate {"type":"stream"}
        // lines are forwarded to the SSE channel.
        let deadline = Duration::from_secs(35);
        loop {
            let mut line = String::new();
            match timeout(deadline, self.reader.read_line(&mut line)).await {
                Ok(Ok(0)) => {
                    return ExecutionResult {
                        stdout: String::new(),
                        result: None,
                        error: Some("Worker process died during execution".into()),
                        duration_ms: 0,
                        plots: vec![],
                        html: None,
                    };
                }
                Ok(Ok(_)) => {
                    let trimmed = line.trim();
                    if let Ok(msg) = serde_json::from_str::<serde_json::Value>(trimmed) {
                        match msg.get("type").and_then(|v| v.as_str()) {
                            Some("stream") => {
                                // Forward stream chunk to SSE channel; ignore send errors
                                // (client may have disconnected).
                                let _ = tx.send(trimmed.to_string()).await;
                            }
                            Some("result") => {
                                return ExecutionResult {
                                    stdout: msg.get("stdout").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                                    result: msg.get("result").and_then(|v| v.as_str()).map(|s| s.to_string()),
                                    error: msg.get("error").and_then(|v| if v.is_null() { None } else { v.as_str().map(|s| s.to_string()) }),
                                    duration_ms: msg.get("duration_ms").and_then(|v| v.as_u64()).unwrap_or(0),
                                    plots: msg.get("plots").and_then(|v| v.as_array())
                                        .map(|arr| arr.iter().filter_map(|v| v.as_str().map(|s| s.to_string())).collect())
                                        .unwrap_or_default(),
                                    html: msg.get("html").and_then(|v| if v.is_null() { None } else { v.as_str().map(|s| s.to_string()) }),
                                };
                            }
                            _ => {}
                        }
                    }
                }
                _ => {
                    return ExecutionResult {
                        stdout: String::new(),
                        result: None,
                        error: Some("Execution timed out (35s)".into()),
                        duration_ms: 35_000,
                        plots: vec![],
                        html: None,
                    };
                }
            }
        }
    }

    pub async fn install_packages(&mut self, packages: &[String]) -> (String, Option<String>, u64) {
        #[derive(serde::Serialize)]
        struct InstallCmd<'a> {
            #[serde(rename = "type")]
            cmd_type: &'static str,
            packages: &'a [String],
        }

        let mut cmd_json = serde_json::to_string(&InstallCmd {
            cmd_type: "install",
            packages,
        })
        .unwrap_or_else(|_| r#"{"type":"install","packages":[]}"#.to_string());
        cmd_json.push('\n');

        if self.stdin.write_all(cmd_json.as_bytes()).await.is_err()
            || self.stdin.flush().await.is_err()
        {
            return (String::new(), Some("Failed to send install command".into()), 0);
        }

        let mut line = String::new();
        // pip installs can be slow — allow up to 130s
        match timeout(Duration::from_secs(130), self.reader.read_line(&mut line)).await {
            Ok(Ok(_)) => {
                if let Ok(msg) = serde_json::from_str::<serde_json::Value>(line.trim()) {
                    let stdout = msg.get("stdout").and_then(|v| v.as_str()).unwrap_or("").to_string();
                    let error = msg.get("error").and_then(|v| if v.is_null() { None } else { v.as_str().map(|s| s.to_string()) });
                    let duration_ms = msg.get("duration_ms").and_then(|v| v.as_u64()).unwrap_or(0);
                    (stdout, error, duration_ms)
                } else {
                    (String::new(), Some("Invalid response from worker".into()), 0)
                }
            }
            _ => (String::new(), Some("Install timed out or worker died".into()), 0),
        }
    }

    pub async fn clear_state(&mut self) {
        let cmd = WorkerCommand {
            cmd_type: "clear".to_string(),
            code: None,
            stream: None,
            path: None,
        };
        let mut cmd_json = serde_json::to_string(&cmd).unwrap_or_else(|_| r#"{"type":"clear"}"#.to_string());
        cmd_json.push('\n');

        if self.stdin.write_all(cmd_json.as_bytes()).await.is_ok() {
            let _ = self.stdin.flush().await;
            let mut line = String::new();
            let _ = timeout(Duration::from_secs(5), self.reader.read_line(&mut line)).await;
        }
    }

    /// Serialize session globals to disk. Returns (size_bytes, error).
    pub async fn create_checkpoint(&mut self, path: &str) -> (u64, Option<String>) {
        let cmd = WorkerCommand {
            cmd_type: "checkpoint".to_string(),
            code: None,
            stream: None,
            path: Some(path.to_string()),
        };

        let mut cmd_json = serde_json::to_string(&cmd)
            .unwrap_or_else(|_| r#"{"type":"checkpoint","path":""}"#.to_string());
        cmd_json.push('\n');

        if self.stdin.write_all(cmd_json.as_bytes()).await.is_err()
            || self.stdin.flush().await.is_err()
        {
            return (0, Some("Failed to send checkpoint command".into()));
        }

        let mut line = String::new();
        match timeout(Duration::from_secs(30), self.reader.read_line(&mut line)).await {
            Ok(Ok(_)) => {
                if let Ok(msg) = serde_json::from_str::<serde_json::Value>(line.trim()) {
                    let size = msg.get("size_bytes").and_then(|v| v.as_u64()).unwrap_or(0);
                    let error = msg.get("error").and_then(|v| {
                        if v.is_null() { None } else { v.as_str().map(|s| s.to_string()) }
                    });
                    (size, error)
                } else {
                    (0, Some("Invalid response from worker".into()))
                }
            }
            _ => (0, Some("Checkpoint timed out or worker died".into())),
        }
    }

    /// Deserialize session globals from disk and merge into worker state. Returns (vars, error).
    pub async fn restore_checkpoint(&mut self, path: &str) -> (Vec<String>, Option<String>) {
        let cmd = WorkerCommand {
            cmd_type: "restore".to_string(),
            code: None,
            stream: None,
            path: Some(path.to_string()),
        };

        let mut cmd_json = serde_json::to_string(&cmd)
            .unwrap_or_else(|_| r#"{"type":"restore","path":""}"#.to_string());
        cmd_json.push('\n');

        if self.stdin.write_all(cmd_json.as_bytes()).await.is_err()
            || self.stdin.flush().await.is_err()
        {
            return (vec![], Some("Failed to send restore command".into()));
        }

        let mut line = String::new();
        match timeout(Duration::from_secs(30), self.reader.read_line(&mut line)).await {
            Ok(Ok(_)) => {
                if let Ok(msg) = serde_json::from_str::<serde_json::Value>(line.trim()) {
                    let vars = msg.get("vars")
                        .and_then(|v| v.as_array())
                        .map(|arr| arr.iter().filter_map(|v| v.as_str().map(|s| s.to_string())).collect())
                        .unwrap_or_default();
                    let error = msg.get("error").and_then(|v| {
                        if v.is_null() { None } else { v.as_str().map(|s| s.to_string()) }
                    });
                    (vars, error)
                } else {
                    (vec![], Some("Invalid response from worker".into()))
                }
            }
            _ => (vec![], Some("Restore timed out or worker died".into())),
        }
    }

    pub fn is_alive(&mut self) -> bool {
        match self.child.try_wait() {
            Ok(None) => true,    // still running
            Ok(Some(_)) => false, // exited
            Err(_) => false,      // error checking
        }
    }
}
