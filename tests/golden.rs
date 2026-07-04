//! Golden integration tests.
//!
//! Each `tests/cases/*.input.csv` file is run through the binary and compared
//! with the matching `*.expected.csv`, ignoring only trailing newline changes.

use std::path::{Path, PathBuf};
use std::process::Command;

const BIN: &str = env!("CARGO_BIN_EXE_tx-processor");

fn cases_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/cases")
}

fn run_case(input: &Path) -> Result<(), String> {
    let name = input
        .file_name()
        .and_then(|n| n.to_str())
        .and_then(|n| n.strip_suffix(".input.csv"))
        .expect("input file name ends in .input.csv");
    let expected_path = input.with_file_name(format!("{name}.expected.csv"));

    let output = Command::new(BIN)
        .arg(input)
        .output()
        .map_err(|e| format!("{name}: failed to run {BIN}: {e}"))?;
    let stderr = String::from_utf8_lossy(&output.stderr);

    // Bad rows are skipped and logged, never fatal: the run must still exit 0.
    if !output.status.success() {
        return Err(format!(
            "{name}: binary exited with {}\n--- stderr ---\n{stderr}",
            output.status
        ));
    }

    let expected = std::fs::read_to_string(&expected_path)
        .map_err(|e| format!("{name}: cannot read {}: {e}", expected_path.display()))?;
    let actual = String::from_utf8_lossy(&output.stdout);

    if actual.trim_end() != expected.trim_end() {
        return Err(format!(
            "{name}: output does not match expected\n--- expected ---\n{expected}\n--- actual ---\n{actual}\n--- stderr ---\n{stderr}"
        ));
    }
    Ok(())
}

#[test]
fn golden_cases() {
    let dir = cases_dir();
    let mut inputs: Vec<PathBuf> = std::fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("cannot read cases dir {}: {e}", dir.display()))
        .map(|e| e.expect("dir entry").path())
        .filter(|p| p.to_string_lossy().ends_with(".input.csv"))
        .collect();
    inputs.sort();

    assert!(
        !inputs.is_empty(),
        "no *.input.csv cases found in {}",
        dir.display()
    );

    let failures: Vec<String> = inputs.iter().filter_map(|p| run_case(p).err()).collect();

    assert!(
        failures.is_empty(),
        "{} of {} golden cases failed:\n\n{}",
        failures.len(),
        inputs.len(),
        failures.join("\n\n")
    );

    eprintln!("all {} golden cases passed", inputs.len());
}
