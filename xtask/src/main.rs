use std::env;
use std::process::{Command, ExitCode};

const MSRV_TOOLCHAIN: &str = "1.85.0";
const WINDOWS_TARGET: &str = "x86_64-pc-windows-msvc";
const USAGE: &str = "usage: cargo xtask <ci|ci-host|ci-msrv|install-hooks>";

fn main() -> ExitCode {
    let mut arguments = env::args().skip(1);
    let command = arguments.next();
    if arguments.next().is_some() {
        return fail(USAGE);
    }

    let result = match command.as_deref() {
        Some("ci") => run_ci(),
        Some("ci-host") => run_host_ci(),
        Some("ci-msrv") => run_msrv_check(),
        Some("install-hooks") => install_hooks(),
        _ => Err(USAGE.to_owned()),
    };

    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => fail(&error),
    }
}

fn run_ci() -> Result<(), String> {
    run_format_check()?;
    run_msrv_check()?;
    run_host_clippy()?;
    run_windows_clippy()?;
    run_tests()?;
    run_release_build()
}

fn run_host_ci() -> Result<(), String> {
    run_format_check()?;
    run_host_clippy()?;
    run_tests()?;
    run_release_build()
}

fn run_format_check() -> Result<(), String> {
    run_cargo(None, &["fmt", "--all", "--", "--check"])
}

fn run_msrv_check() -> Result<(), String> {
    run_cargo(
        Some(MSRV_TOOLCHAIN),
        &[
            "check",
            "--locked",
            "--workspace",
            "--all-targets",
            "--all-features",
        ],
    )
}

fn run_host_clippy() -> Result<(), String> {
    run_cargo(
        None,
        &[
            "clippy",
            "--locked",
            "--workspace",
            "--all-targets",
            "--all-features",
            "--",
            "-D",
            "warnings",
        ],
    )
}

fn run_windows_clippy() -> Result<(), String> {
    run_cargo(
        None,
        &[
            "clippy",
            "--locked",
            "--workspace",
            "--target",
            WINDOWS_TARGET,
            "--all-targets",
            "--all-features",
            "--",
            "-D",
            "warnings",
        ],
    )
}

fn run_tests() -> Result<(), String> {
    run_cargo(
        None,
        &[
            "test",
            "--locked",
            "--workspace",
            "--all-targets",
            "--all-features",
        ],
    )
}

fn run_release_build() -> Result<(), String> {
    run_cargo(None, &["build", "--locked", "--workspace", "--release"])
}

fn install_hooks() -> Result<(), String> {
    run_command("git", &["config", "core.hooksPath", ".githooks"])?;
    println!("Git hooks configured from .githooks");
    Ok(())
}

fn run_cargo(toolchain: Option<&str>, arguments: &[&str]) -> Result<(), String> {
    let mut command = Command::new("cargo");
    if let Some(toolchain) = toolchain {
        command.arg(format!("+{toolchain}"));
    }
    command.args(arguments);
    run(&mut command)
}

fn run_command(program: &str, arguments: &[&str]) -> Result<(), String> {
    let mut command = Command::new(program);
    command.args(arguments);
    run(&mut command)
}

fn run(command: &mut Command) -> Result<(), String> {
    eprintln!("==> {command:?}");
    let status = command
        .status()
        .map_err(|error| format!("could not run {command:?}: {error}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("{command:?} exited with {status}"))
    }
}

fn fail(message: &str) -> ExitCode {
    eprintln!("xtask: {message}");
    ExitCode::FAILURE
}
