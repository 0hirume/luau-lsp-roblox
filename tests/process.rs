//! Process-lifetime regression tests for guarded wrapper children.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant, SystemTime};

const GUARD_ENV: &str = "LUAU_LSP_ROBLOX_GUARD";
const HEARTBEAT_ENV: &str = "LUAU_LSP_ROBLOX_TEST_HEARTBEAT";
const WAIT_LIMIT: Duration = Duration::from_secs(5);

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

    wait_for_exit(&mut guard)
}

#[test]
fn guard_stops_descendants_when_the_parent_pipe_closes() -> Result<(), Box<dyn std::error::Error>> {
    let heartbeat = temporary_path("guard-heartbeat")?;
    let test = std::env::current_exe()?;
    let mut guard = Command::new(env!("CARGO_BIN_EXE_luau-lsp"))
        .args([
            test.as_os_str(),
            "--ignored".as_ref(),
            "--exact".as_ref(),
            "guard_parent".as_ref(),
        ])
        .env(GUARD_ENV, "1")
        .env(HEARTBEAT_ENV, &heartbeat)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;

    wait_for_file(&heartbeat, &mut guard)?;
    drop(guard.stdin.take());
    wait_for_exit(&mut guard)?;

    thread::sleep(Duration::from_millis(100));
    let stopped = fs::read(&heartbeat)?;
    thread::sleep(Duration::from_millis(150));
    let unchanged = fs::read(&heartbeat)?;
    fs::remove_file(heartbeat)?;

    if stopped != unchanged {
        return Err(std::io::Error::other("guarded descendant remained alive").into());
    }
    Ok(())
}

fn wait_for_exit(child: &mut std::process::Child) -> Result<(), Box<dyn std::error::Error>> {
    let deadline = Instant::now() + WAIT_LIMIT;
    while child.try_wait()?.is_none() {
        if Instant::now() >= deadline {
            let _result = child.kill();
            let _result = child.wait();
            return Err(std::io::Error::other("process guard did not stop its child").into());
        }
        thread::sleep(Duration::from_millis(25));
    }
    Ok(())
}

fn wait_for_file(
    path: &Path,
    guard: &mut std::process::Child,
) -> Result<(), Box<dyn std::error::Error>> {
    let deadline = Instant::now() + WAIT_LIMIT;
    while !path.is_file() {
        if guard.try_wait()?.is_some() {
            return Err(std::io::Error::other("guarded descendant did not start").into());
        }
        if Instant::now() >= deadline {
            let _result = guard.kill();
            let _result = guard.wait();
            return Err(std::io::Error::other("guarded descendant did not become ready").into());
        }
        thread::sleep(Duration::from_millis(25));
    }
    Ok(())
}

fn temporary_path(label: &str) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let nanos = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)?
        .as_nanos();
    Ok(std::env::temp_dir().join(format!(
        "luau-lsp-roblox-{label}-{}-{nanos}",
        std::process::id()
    )))
}

#[test]
#[ignore = "fixture launched by the process guard descendant test"]
fn guard_parent() -> Result<(), Box<dyn std::error::Error>> {
    let test = std::env::current_exe()?;
    let mut child = Command::new(test)
        .args(["--ignored", "--exact", "guard_leaf"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    let _status = child.wait()?;
    Ok(())
}

#[test]
#[ignore = "fixture launched by the process guard integration test"]
fn guard_child() {
    thread::sleep(Duration::from_secs(30));
}

#[test]
#[ignore = "fixture launched by the process guard descendant test"]
fn guard_leaf() -> Result<(), Box<dyn std::error::Error>> {
    let heartbeat = std::env::var_os(HEARTBEAT_ENV)
        .map(PathBuf::from)
        .ok_or_else(|| std::io::Error::other("heartbeat path is missing"))?;
    for beat in 0_u16..1_200 {
        fs::write(&heartbeat, beat.to_string())?;
        thread::sleep(Duration::from_millis(25));
    }
    Ok(())
}
