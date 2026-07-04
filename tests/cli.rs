use std::path::PathBuf;
use std::process::{Command, Output};
use std::sync::atomic::{AtomicUsize, Ordering};

const BIN: &str = env!("CARGO_BIN_EXE_tx-processor");

fn command() -> Command {
    let mut command = Command::new(BIN);
    command.env("RUST_LOG", "warn");
    command
}

fn temp_csv(name: &str, body: &str) -> PathBuf {
    static NEXT: AtomicUsize = AtomicUsize::new(0);

    let path = std::env::temp_dir().join(format!(
        "tx-processor-{name}-{}-{}.csv",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::SeqCst)
    ));
    std::fs::write(&path, body).expect("write temporary csv");
    path
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

#[test]
fn missing_argument_exits_nonzero_without_stdout() {
    let output = command().output().expect("run binary");

    assert!(!output.status.success());
    assert_eq!(stdout(&output), "");
    assert!(stderr(&output).contains("missing input file argument"));
}

#[test]
fn too_many_arguments_exits_nonzero_without_stdout() {
    let output = command()
        .arg("one.csv")
        .arg("two.csv")
        .output()
        .expect("run binary");

    assert!(!output.status.success());
    assert_eq!(stdout(&output), "");
    assert!(stderr(&output).contains("too many arguments"));
}

#[test]
fn missing_input_file_exits_nonzero_without_stdout() {
    let path = std::env::temp_dir().join(format!(
        "tx-processor-missing-{}-{}.csv",
        std::process::id(),
        "input"
    ));
    let _ = std::fs::remove_file(&path);

    let output = command().arg(&path).output().expect("run binary");

    assert!(!output.status.success());
    assert_eq!(stdout(&output), "");
    assert!(stderr(&output).contains("failed to process"));
}

#[test]
fn malformed_rows_log_to_stderr_and_do_not_pollute_stdout() {
    let input = temp_csv(
        "malformed",
        "\
type, client, tx, amount
deposit, 1, 1, 5.0
foobar, 1, 2, 3.0
deposit, 1, 3, 2.0
",
    );

    let output = command().arg(&input).output().expect("run binary");
    let _ = std::fs::remove_file(&input);

    assert!(output.status.success());
    assert_eq!(
        stdout(&output),
        "client,available,held,total,locked\n1,7.0,0.0,7.0,false\n"
    );
    assert!(stderr(&output).contains("skipping malformed row"));
    assert!(!stdout(&output).contains("skipping"));
    assert!(!stdout(&output).contains("WARN"));
}

#[test]
fn rule_violations_log_to_stderr_and_do_not_pollute_stdout() {
    let input = temp_csv(
        "rule-violation",
        "\
type, client, tx, amount
deposit, 1, 1, 5.0
withdrawal, 1, 2, 10.0
deposit, 1, 3, 1.0
",
    );

    let output = command().arg(&input).output().expect("run binary");
    let _ = std::fs::remove_file(&input);

    assert!(output.status.success());
    assert_eq!(
        stdout(&output),
        "client,available,held,total,locked\n1,6.0,0.0,6.0,false\n"
    );
    assert!(stderr(&output).contains("exceeds available funds"));
    assert!(!stdout(&output).contains("withdrawal of"));
    assert!(!stdout(&output).contains("WARN"));
}
