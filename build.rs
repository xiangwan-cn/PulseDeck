use std::process::Command;

fn main() {
    // Keep crash/journal records tied to the exact source revision when a
    // binary is built from a checkout. Packaged source archives simply report
    // `unknown` without making the build depend on Git being installed.
    println!("cargo:rerun-if-changed=.git/HEAD");
    println!("cargo:rerun-if-changed=.git/index");
    let commit = Command::new("git")
        .args(["rev-parse", "--short=12", "HEAD"])
        .output()
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "unknown".to_string());
    println!("cargo:rustc-env=PULSEDECK_GIT_COMMIT={commit}");
}
