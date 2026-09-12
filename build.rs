use std::{env, process::Command};

fn command_version(program: &str, args: &[&str]) -> String {
    Command::new(program)
        .args(args)
        .output()
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .map(|value| value.trim().replace(['\n', '\r'], " "))
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "unknown".to_string())
}

fn main() {
    // Keep crash/journal records tied to the exact source revision when a
    // binary is built from a checkout. Packaged source archives simply report
    // `unknown` without making the build depend on Git being installed.
    println!("cargo:rerun-if-changed=.git/HEAD");
    println!("cargo:rerun-if-changed=.git/index");
    println!("cargo:rerun-if-changed=.git/refs/heads");
    let commit = Command::new("git")
        .args(["rev-parse", "--short=12", "HEAD"])
        .output()
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "unknown".to_string());
    // Include untracked files as well as tracked modifications. A build from
    // a checkout with a new local module must never advertise itself as a
    // clean release, otherwise a crash report can be matched to the wrong
    // source tree.
    let dirty = Command::new("git")
        .args(["status", "--porcelain", "--untracked-files=normal"])
        .output()
        .map(|output| !output.stdout.is_empty())
        .unwrap_or(false);
    let target = env::var("TARGET").unwrap_or_else(|_| "unknown".into());
    let profile = env::var("PROFILE").unwrap_or_else(|_| "unknown".into());
    let rustc_version = command_version("rustc", &["--version"]);
    let gtk_version = command_version("pkg-config", &["--modversion", "gtk4"]);
    let adwaita_version = command_version("pkg-config", &["--modversion", "libadwaita-1"]);
    let mut features = Vec::new();
    for (key, value) in env::vars() {
        if key.starts_with("CARGO_FEATURE_") && value == "1" {
            features.push(
                key.trim_start_matches("CARGO_FEATURE_")
                    .to_ascii_lowercase(),
            );
        }
    }
    features.sort();
    let feature_list = if features.is_empty() {
        "none".to_string()
    } else {
        features.join(",")
    };
    let build_id = format!(
        "{commit}-{}-{}-{}",
        if dirty { "dirty" } else { "clean" },
        target,
        feature_list
    );
    println!("cargo:rustc-env=PULSEDECK_GIT_COMMIT={commit}");
    println!("cargo:rustc-env=PULSEDECK_GIT_DIRTY={dirty}");
    println!("cargo:rustc-env=PULSEDECK_BUILD_TARGET={target}");
    println!("cargo:rustc-env=PULSEDECK_BUILD_PROFILE={profile}");
    println!("cargo:rustc-env=PULSEDECK_FEATURES={feature_list}");
    println!("cargo:rustc-env=PULSEDECK_BUILD_RUSTC={rustc_version}");
    println!("cargo:rustc-env=PULSEDECK_BUILD_GTK={gtk_version}");
    println!("cargo:rustc-env=PULSEDECK_BUILD_ADWAITA={adwaita_version}");
    println!("cargo:rustc-env=PULSEDECK_BUILD_ID={build_id}");
}
