use std::ffi::OsString;
use std::io::{Read as _, Write as _};
use std::path::Path;
use std::process::{Child, ChildStderr, ChildStdin, Command, ExitStatus, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

use command_group::CommandGroup as _;

use crate::{Result, error};

const GUARD_ENV: &str = "LUAU_LSP_ROBLOX_GUARD";

pub struct Guard {
    child: Child,
    signal: Option<ChildStdin>,
    stderr: Option<thread::JoinHandle<()>>,
}

impl Guard {
    pub(crate) fn spawn(command: &[String], root: &Path) -> Result<Self> {
        if command.is_empty() {
            return Err(error("guarded process command is empty"));
        }
        let mut child = Command::new(std::env::current_exe()?)
            .args(command)
            .current_dir(root)
            .env(GUARD_ENV, "1")
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()?;
        let signal = child
            .stdin
            .take()
            .ok_or_else(|| error("failed to open process guard signal"))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| error("failed to open process guard stderr"))?;
        Ok(Self {
            child,
            signal: Some(signal),
            stderr: Some(forward_stderr(stderr)),
        })
    }

    pub(crate) fn try_wait(&mut self) -> std::io::Result<Option<ExitStatus>> {
        self.child.try_wait()
    }

    pub(crate) fn wait(&mut self) -> std::io::Result<ExitStatus> {
        self.signal.take();
        let status = self.child.wait();
        if let Some(stderr) = self.stderr.take() {
            let _result = stderr.join();
        }
        status
    }
}

impl Drop for Guard {
    fn drop(&mut self) {
        self.signal.take();
    }
}

pub fn run_guard() -> Option<Result<u8>> {
    std::env::var_os(GUARD_ENV).map(|_value| supervise(std::env::args_os().skip(1).collect()))
}

pub fn forward_stderr(stderr: ChildStderr) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        let mut input = stderr;
        let mut block = [0_u8; 8192];
        while let Ok(bytes) = input.read(&mut block) {
            if bytes == 0 {
                return;
            }
            let mut output = std::io::stderr().lock();
            if output.write_all(&block[..bytes]).is_err() || output.flush().is_err() {
                return;
            }
        }
    })
}

fn supervise(arguments: Vec<OsString>) -> Result<u8> {
    let mut arguments = arguments.into_iter();
    let program = arguments
        .next()
        .ok_or_else(|| error("process guard target is missing"))?;
    let mut command = Command::new(program);
    command
        .args(arguments)
        .env_remove(GUARD_ENV)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::inherit());
    let mut child = command.group_spawn()?;
    let closed = Arc::new(AtomicBool::new(false));
    let reader_closed = Arc::clone(&closed);
    thread::spawn(move || {
        let mut input = std::io::stdin().lock();
        let mut block = [0_u8; 1];
        while input.read(&mut block).is_ok_and(|bytes| bytes != 0) {}
        reader_closed.store(true, Ordering::Release);
    });

    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(exit_code(status.code()));
        }
        if closed.load(Ordering::Acquire) {
            let _result = child.kill();
            return Ok(exit_code(child.wait()?.code()));
        }
        thread::sleep(Duration::from_millis(25));
    }
}

fn exit_code(code: Option<i32>) -> u8 {
    code.and_then(|code| u8::try_from(code).ok()).unwrap_or(1)
}
