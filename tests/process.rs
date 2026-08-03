//! Process-lifetime regression tests for guarded wrapper children.

use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

const GUARD_ENV: &str = "LUAU_LSP_ROBLOX_GUARD";

#[test]
fn guard_stops_its_child_when_the_parent_pipe_closes() -> Result<(), Box<dyn std::error::Error>> {
    let test = std::env::current_exe()?;
    let mut guard = Command::new(env!("CARGO_BIN_EXE_luau-lsp"))
        .args([
            test.as_os_str(),
            "--ignored".as_ref(),
            "--exact".as_ref(),
            "guard_child".as_ref(),
        ])
        .env(GUARD_ENV, "1")
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    drop(guard.stdin.take());

    let deadline = Instant::now() + Duration::from_secs(5);
    while guard.try_wait()?.is_none() {
        if Instant::now() >= deadline {
            let _result = guard.kill();
            let _result = guard.wait();
            return Err(std::io::Error::other("process guard did not stop its child").into());
        }
        thread::sleep(Duration::from_millis(25));
    }
    Ok(())
}

#[test]
#[ignore = "fixture launched by the process guard integration test"]
fn guard_child() {
    thread::sleep(Duration::from_secs(30));
}
