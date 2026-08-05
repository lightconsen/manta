//! Platform-abstracted subprocess execution.
//!
//! Collects the scattered `Command::new` sites in `src/tools/*` behind a
//! single trait so that mobile builds route process execution through an
//! Android `sh`/bundled-native-binary launcher (or reject it outright on
//! iOS) while the desktop keeps today's `std::process` behavior unchanged
//! (docs/mobile-migration.md §4.3).
//!
//! Tools call [`run`] / [`spawn`] instead of constructing a
//! `tokio::process::Command` directly; the facade dispatches to the
//! platform-appropriate [`ProcessRunner`].

use std::collections::HashMap;
use std::path::PathBuf;
use std::process::ExitStatus;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use std::process::Stdio;
use tokio::process::{Child, Command};

impl ProcessRequest {
    /// Build a request from a plain argv (no cwd/env/stdin/timeout).
    pub fn argv(argv: &[&str]) -> Self {
        Self {
            argv: argv.iter().map(|s| s.to_string()).collect(),
            ..Default::default()
        }
    }
}

/// How a [`ProcessRunner::spawn`]'d child has its standard streams wired.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum StdioMode {
    /// All three streams point at /dev/null — detached background processes.
    #[default]
    Null,
    /// stdout/stderr are piped (readable from the returned `Child`);
    /// stdin is /dev/null.
    Piped,
}

/// A subprocess spawn request, covering the knobs the tools actually use.
#[derive(Clone, Default)]
pub struct ProcessRequest {
    /// Program + arguments. `argv[0]` is the executable; empty => error.
    pub argv: Vec<String>,
    /// Working directory for the child.
    pub cwd: Option<PathBuf>,
    /// Clear the inherited environment before applying `env`.
    pub env_clear: bool,
    /// Extra environment variables to set on the child.
    pub env: HashMap<String, String>,
    /// Data written to the child's stdin (`None` => null stdin, matching
    /// `Command::output()` semantics).
    pub stdin: Option<Vec<u8>>,
    /// Wall-clock timeout for a [`ProcessRunner::run`]; `None` waits forever.
    pub timeout: Option<Duration>,
    /// Unix resource-limit hook applied in the child before `exec`
    /// (`setrlimit` etc.). Ignored on non-Unix platforms.
    pub pre_exec: Option<Arc<dyn Fn() -> std::io::Result<()> + Send + Sync>>,
    /// Stdio wiring for [`ProcessRunner::spawn`]. Ignored by
    /// [`ProcessRunner::run`], which always captures output.
    pub stdio: StdioMode,
}

/// Output of a completed [`ProcessRunner::run`].
#[derive(Debug, Clone)]
pub struct CommandOutput {
    /// `None` when the run was aborted (spawn failure or timeout).
    pub status: Option<ExitStatus>,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

impl CommandOutput {
    pub fn stdout_string(&self) -> String {
        String::from_utf8_lossy(&self.stdout).into_owned()
    }

    pub fn stderr_string(&self) -> String {
        String::from_utf8_lossy(&self.stderr).into_owned()
    }

    /// True when the process exited with a zero status (and actually ran).
    pub fn success(&self) -> bool {
        self.status.as_ref().is_some_and(|s| s.success())
    }

    pub fn exit_code(&self) -> Option<i32> {
        self.status.and_then(|s| s.code())
    }
}

/// Failure modes for a subprocess run.
#[derive(Debug, thiserror::Error)]
pub enum ProcessError {
    #[error("no executable specified")]
    EmptyArgv,
    #[error("failed to spawn '{program}': {source}")]
    Spawn {
        program: String,
        #[source]
        source: std::io::Error,
    },
    #[error("process timed out after {duration:?}")]
    Timeout { duration: Duration },
    #[error("process execution is not supported on this platform")]
    Unsupported,
}

/// Platform-abstracted subprocess launcher.
#[async_trait]
pub trait ProcessRunner: Send + Sync {
    /// Run `req` to completion and capture stdout/stderr. On timeout the run
    /// is aborted (the child is not force-killed, matching today's
    /// `timeout(.., cmd.output())` behavior).
    async fn run(&self, req: &ProcessRequest) -> Result<CommandOutput, ProcessError>;

    /// Spawn `req` and return a live [`Child`] for long-running process
    /// management (tracked status, detached wait). Stdio follows
    /// `ProcessRequest::stdio` ([`StdioMode::Null`] by default).
    async fn spawn(&self, req: &ProcessRequest) -> Result<Child, ProcessError>;
}

/// Desktop: spawns through `tokio::process` exactly as the tools did before
/// the abstraction (env, cwd, pipes, timeouts, error mapping unchanged).
#[derive(Debug, Clone, Copy, Default)]
pub struct StdProcessRunner;

#[async_trait]
impl ProcessRunner for StdProcessRunner {
    async fn run(&self, req: &ProcessRequest) -> Result<CommandOutput, ProcessError> {
        let mut cmd = build_command(req)?;

        match &req.stdin {
            Some(input) => {
                let mut child = cmd
                    .stdin(Stdio::piped())
                    .stdout(Stdio::piped())
                    .stderr(Stdio::piped())
                    .spawn()
                    .map_err(|source| spawn_err(req, source))?;
                let input = input.clone();
                run_with_timeout(req, async move {
                    use tokio::io::AsyncWriteExt;
                    let mut stdin = child.stdin.take().ok_or(ProcessError::Spawn {
                        program: req.argv.first().cloned().unwrap_or_default(),
                        source: std::io::Error::other("stdin pipe missing"),
                    })?;
                    let _ = stdin.write_all(&input).await;
                    drop(stdin); // EOF to the child
                    child
                        .wait_with_output()
                        .await
                        .map(|o| CommandOutput {
                            status: Some(o.status),
                            stdout: o.stdout,
                            stderr: o.stderr,
                        })
                        .map_err(|source| spawn_err(req, source))
                })
                .await
            }
            None => {
                // `output()` nulls stdin and captures stdout/stderr.
                run_with_timeout(req, async {
                    cmd.output()
                        .await
                        .map(|o| CommandOutput {
                            status: Some(o.status),
                            stdout: o.stdout,
                            stderr: o.stderr,
                        })
                        .map_err(|source| spawn_err(req, source))
                })
                .await
            }
        }
    }

    async fn spawn(&self, req: &ProcessRequest) -> Result<Child, ProcessError> {
        let mut cmd = build_command(req)?;
        match req.stdio {
            StdioMode::Null => {
                cmd.stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .stdin(Stdio::null());
            }
            StdioMode::Piped => {
                cmd.stdout(Stdio::piped())
                    .stderr(Stdio::piped())
                    .stdin(Stdio::null());
            }
        }
        cmd.spawn().map_err(|source| spawn_err(req, source))
    }
}

/// Build the underlying `tokio::process::Command` from a request.
fn build_command(req: &ProcessRequest) -> Result<Command, ProcessError> {
    let program = req.argv.first().ok_or(ProcessError::EmptyArgv)?;
    let mut cmd = Command::new(program);
    cmd.args(&req.argv[1..]);
    if let Some(cwd) = &req.cwd {
        cmd.current_dir(cwd);
    }
    if req.env_clear {
        cmd.env_clear();
    }
    for (key, value) in &req.env {
        cmd.env(key, value);
    }
    if let Some(pre) = &req.pre_exec {
        let pre = Arc::clone(pre);
        #[allow(unsafe_code)] // pre_exec runs in the child after fork; mirrors existing tools
        unsafe {
            cmd.pre_exec(move || pre());
        }
    }
    Ok(cmd)
}

fn spawn_err(req: &ProcessRequest, source: std::io::Error) -> ProcessError {
    ProcessError::Spawn {
        program: req.argv.first().cloned().unwrap_or_default(),
        source,
    }
}

/// Apply `req.timeout` around a future, preserving `timeout(.., fut)` drop
/// semantics (the child is not force-killed on timeout).
async fn run_with_timeout<T>(
    req: &ProcessRequest,
    fut: impl std::future::Future<Output = Result<T, ProcessError>>,
) -> Result<T, ProcessError> {
    match req.timeout {
        Some(duration) => match tokio::time::timeout(duration, fut).await {
            Ok(result) => result,
            Err(_) => Err(ProcessError::Timeout { duration }),
        },
        None => fut.await,
    }
}

/// The cached platform-appropriate runner.
pub fn process_runner() -> Arc<dyn ProcessRunner> {
    static RUNNER: std::sync::LazyLock<Arc<dyn ProcessRunner>> =
        std::sync::LazyLock::new(default_process_runner);
    Arc::clone(&RUNNER)
}

fn default_process_runner() -> Arc<dyn ProcessRunner> {
    #[cfg(target_os = "android")]
    {
        Arc::new(AndroidShellRunner::from_env())
    }
    #[cfg(target_os = "ios")]
    {
        Arc::new(IosProcessRunner)
    }
    #[cfg(not(mobile_os))]
    {
        Arc::new(StdProcessRunner)
    }
}

/// Run a subprocess to completion, capturing output. See [`ProcessRunner::run`].
pub async fn run(req: &ProcessRequest) -> Result<CommandOutput, ProcessError> {
    process_runner().run(req).await
}

/// Spawn a detached subprocess. See [`ProcessRunner::spawn`].
pub async fn spawn(req: &ProcessRequest) -> Result<Child, ProcessError> {
    process_runner().spawn(req).await
}

/// Android: the only executable entry points are `/system/bin/sh` (which
/// resolves the toybox applets by its built-in PATH) and bundled native
/// binaries shipped in `jniLibs` and extracted to `nativeLibraryDir`.
/// SELinux blocks `exec` from the app-private `filesDir` for targetSdk 29+,
/// so everything else is rejected (docs/mobile-migration.md §3.1).
#[cfg(target_os = "android")]
#[derive(Debug, Clone)]
pub struct AndroidShellRunner {
    native_library_dir: Option<PathBuf>,
    whitelist: Arc<std::collections::HashSet<&'static str>>,
    inner: StdProcessRunner,
}

#[cfg(target_os = "android")]
const TOYBOX_APPLETS: &[&str] = &[
    "sh",
    "/bin/sh",
    "/system/bin/sh",
    "ls",
    "cat",
    "echo",
    "printf",
    "pwd",
    "cp",
    "mv",
    "rm",
    "rmdir",
    "mkdir",
    "touch",
    "chmod",
    "chown",
    "grep",
    "sed",
    "awk",
    "wc",
    "head",
    "tail",
    "sort",
    "uniq",
    "find",
    "xxd",
    "base64",
    "date",
    "seq",
    "tr",
    "cut",
    "paste",
    "dirname",
    "basename",
    "stat",
    "df",
    "du",
    "ps",
    "sleep",
    "test",
    "which",
];

#[cfg(target_os = "android")]
impl AndroidShellRunner {
    /// Build the Android runner. `nativeLibraryDir` is read from
    /// `SYSCITY_NATIVE_LIB_DIR` (set by `MainActivity.kt` next to
    /// `SYSCITY_HOME`); bundled binaries are exec'd from there.
    pub fn from_env() -> Self {
        Self {
            native_library_dir: std::env::var("SYSCITY_NATIVE_LIB_DIR")
                .ok()
                .filter(|p| !p.is_empty())
                .map(PathBuf::from),
            whitelist: Arc::new(TOYBOX_APPLETS.iter().copied().collect()),
            inner: StdProcessRunner,
        }
    }

    /// Rewrite the request argv to an executable form permitted on Android.
    fn resolve_argv(&self, req: &ProcessRequest) -> Result<Vec<String>, ProcessError> {
        let program = req.argv.first().ok_or(ProcessError::EmptyArgv)?;

        // Bundled native binary (exec from nativeLibraryDir is the only
        // allowed exec path for same-UID binaries on targetSdk 29+).
        if let Some(dir) = &self.native_library_dir {
            let bundled = dir.join(program);
            if bundled.exists() {
                let mut argv = vec![bundled.to_string_lossy().into_owned()];
                argv.extend(req.argv[1..].iter().cloned());
                return Ok(argv);
            }
        }

        // The shell itself, or a whitelisted toybox applet routed through it
        // so the applet resolves via sh's built-in PATH.
        if self.whitelist.contains(program.as_str()) {
            return Ok(req.argv.clone());
        }

        Err(ProcessError::Unsupported)
    }
}

#[cfg(target_os = "android")]
impl AndroidShellRunner {
    /// Point a bundled native binary at its sibling libraries.
    ///
    /// The bundled `adb` client (mobile-migration §4.5) is dynamically linked
    /// against `libprotobuf.so`, `libabsl_*.so`, … shipped alongside it in
    /// nativeLibraryDir; its DT_RUNPATH points at a Termux path that does not
    /// exist here. Bionic honors `LD_LIBRARY_PATH` for non-setuid app
    /// processes, so set it to nativeLibraryDir for the bundled-exec path.
    /// `sh`/toybox need nothing (they only use bionic) and are untouched.
    fn apply_bundled_library_path(&self, eff: &mut ProcessRequest) {
        let Some(dir) = &self.native_library_dir else {
            return;
        };
        let Some(program) = eff.argv.first().map(String::as_str) else {
            return;
        };
        if dir.join(program).exists() {
            eff.env
                .insert("LD_LIBRARY_PATH".to_string(), dir.to_string_lossy().into_owned());
        }
    }
}

#[cfg(target_os = "android")]
#[async_trait]
impl ProcessRunner for AndroidShellRunner {
    async fn run(&self, req: &ProcessRequest) -> Result<CommandOutput, ProcessError> {
        let mut eff = req.clone();
        eff.argv = self.resolve_argv(req)?;
        self.apply_bundled_library_path(&mut eff);
        self.inner.run(&eff).await
    }

    async fn spawn(&self, req: &ProcessRequest) -> Result<Child, ProcessError> {
        let mut eff = req.clone();
        eff.argv = self.resolve_argv(req)?;
        self.apply_bundled_library_path(&mut eff);
        self.inner.spawn(&eff).await
    }
}

/// iOS: the sandbox forbids `fork`/`exec` for app code, so every process
/// call fails with [`ProcessError::Unsupported`] (docs/mobile-migration.md
/// §3.2).
#[cfg(target_os = "ios")]
#[derive(Debug, Clone, Copy, Default)]
pub struct IosProcessRunner;

#[cfg(target_os = "ios")]
#[async_trait]
impl ProcessRunner for IosProcessRunner {
    async fn run(&self, _req: &ProcessRequest) -> Result<CommandOutput, ProcessError> {
        Err(ProcessError::Unsupported)
    }

    async fn spawn(&self, _req: &ProcessRequest) -> Result<Child, ProcessError> {
        Err(ProcessError::Unsupported)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(parts: &[&str]) -> ProcessRequest {
        ProcessRequest {
            argv: parts.iter().map(|s| s.to_string()).collect(),
            timeout: Some(Duration::from_secs(10)),
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn test_run_captures_stdout() {
        let out = StdProcessRunner
            .run(&argv(&["/bin/echo", "hello"]))
            .await
            .unwrap();
        assert!(out.success());
        assert_eq!(out.stdout_string().trim(), "hello");
    }

    #[tokio::test]
    async fn test_run_captures_stderr_and_exit_code() {
        let out = StdProcessRunner
            .run(&argv(&["/bin/sh", "-c", "echo err 1>&2; exit 3"]))
            .await
            .unwrap();
        assert!(!out.success());
        assert_eq!(out.exit_code(), Some(3));
        assert!(out.stderr_string().contains("err"));
    }

    #[tokio::test]
    async fn test_run_cwd() {
        let tmp = tempfile::tempdir().unwrap();
        let req = ProcessRequest {
            cwd: Some(tmp.path().to_path_buf()),
            ..argv(&["/bin/pwd"])
        };
        let out = StdProcessRunner.run(&req).await.unwrap();
        assert_eq!(
            std::fs::canonicalize(out.stdout_string().trim()).unwrap(),
            std::fs::canonicalize(tmp.path()).unwrap()
        );
    }

    #[tokio::test]
    async fn test_run_env_clear_and_env() {
        let req = ProcessRequest {
            env_clear: true,
            env: HashMap::from([("FOO".to_string(), "bar".to_string())]),
            ..argv(&["/usr/bin/env"])
        };
        let out = StdProcessRunner.run(&req).await.unwrap();
        assert!(out.stdout_string().contains("FOO=bar"));
        // HOME is inherited only when not cleared; with env_clear it is gone.
        assert!(!out.stdout_string().contains("HOME="));
    }

    #[tokio::test]
    async fn test_run_stdin_piped() {
        let req = ProcessRequest {
            stdin: Some(b"ping\n".to_vec()),
            ..argv(&["/bin/cat"])
        };
        let out = StdProcessRunner.run(&req).await.unwrap();
        assert!(out.success());
        assert_eq!(out.stdout_string().trim(), "ping");
    }

    #[tokio::test]
    async fn test_run_timeout() {
        let req = ProcessRequest {
            timeout: Some(Duration::from_millis(100)),
            ..argv(&["/bin/sleep", "5"])
        };
        let err = StdProcessRunner.run(&req).await.unwrap_err();
        assert!(matches!(err, ProcessError::Timeout { .. }));
    }

    #[tokio::test]
    async fn test_run_empty_argv() {
        let req = ProcessRequest::default();
        let err = StdProcessRunner.run(&req).await.unwrap_err();
        assert!(matches!(err, ProcessError::EmptyArgv));
    }

    #[tokio::test]
    async fn test_spawn_and_wait() {
        let mut child = StdProcessRunner
            .spawn(&argv(&["/bin/echo", "hi"]))
            .await
            .unwrap();
        assert!(child.id().is_some());
        let status = child.wait().await.unwrap();
        assert!(status.success());
    }

    #[tokio::test]
    async fn test_pre_exec_does_not_break_basic_run() {
        let req = ProcessRequest {
            pre_exec: Some(Arc::new(|| Ok(()))),
            ..argv(&["/bin/echo", "ok"])
        };
        let out = StdProcessRunner.run(&req).await.unwrap();
        assert!(out.success());
    }
}
