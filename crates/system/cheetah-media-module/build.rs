use std::path::PathBuf;
use std::process::Command;

fn output_text(cmd: &mut Command) -> Option<String> {
    let output = cmd.output().ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8(output.stdout).ok()?;
    let trimmed = text.trim().to_string();
    if trimmed.is_empty() {
        return None;
    }
    Some(trimmed)
}

fn main() {
    let branch = output_text(Command::new("git").args(["rev-parse", "--abbrev-ref", "HEAD"]))
        .unwrap_or_else(|| "unknown".to_string());

    let commit = output_text(Command::new("git").args(["rev-parse", "--short", "HEAD"]))
        .unwrap_or_else(|| "unknown".to_string());

    let build_time = output_text(Command::new("date").args(["-u", "+%Y-%m-%dT%H:%M:%SZ"]))
        .unwrap_or_else(|| "unknown".to_string());

    println!("cargo:rustc-env=ZLM_BRANCH={branch}");
    println!("cargo:rustc-env=ZLM_COMMIT={commit}");
    println!("cargo:rustc-env=ZLM_BUILD_TIME={build_time}");

    // Re-run when git HEAD or refs change so version metadata stays fresh.
    let git_dir = Command::new("git")
        .args(["rev-parse", "--git-dir"])
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| PathBuf::from(s.trim()));
    if let Some(git_dir) = git_dir {
        println!("cargo:rerun-if-changed={}", git_dir.join("HEAD").display());
        println!("cargo:rerun-if-changed={}", git_dir.join("refs").display());
    }
}
