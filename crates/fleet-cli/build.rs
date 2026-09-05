use std::process::Command;

fn main() {
    println!("cargo:rerun-if-env-changed=FLEET_BUILD_SHA");
    if let Some(head) = git_path("HEAD") {
        println!("cargo:rerun-if-changed={head}");
    }
    if let Some(head_ref) = git_value(&["symbolic-ref", "-q", "HEAD"])
        && let Some(path) = git_path(&head_ref)
    {
        println!("cargo:rerun-if-changed={path}");
    }

    let sha = std::env::var("FLEET_BUILD_SHA")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .or_else(git_sha)
        .unwrap_or_else(|| "dev".to_owned());
    println!("cargo:rustc-env=FLEET_GIT_SHA={sha}");
}

fn git_sha() -> Option<String> {
    git_value(&["rev-parse", "--short=7", "HEAD"])
}

fn git_path(name: &str) -> Option<String> {
    git_value(&["rev-parse", "--git-path", name])
}

fn git_value(arguments: &[&str]) -> Option<String> {
    let output = Command::new("git").args(arguments).output().ok()?;
    if !output.status.success() {
        return None;
    }
    let path = String::from_utf8(output.stdout).ok()?;
    let path = path.trim();
    (!path.is_empty()).then(|| path.to_owned())
}
