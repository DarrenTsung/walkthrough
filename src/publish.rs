use std::fs;
use std::path::Path;
use std::process::Command;

use anyhow::{bail, Context, Result};

pub fn run(html_path: &Path) -> Result<()> {
    let publish_dir = std::env::var("WALKTHROUGH_PUBLISH_PATH")
        .context("WALKTHROUGH_PUBLISH_PATH env var not set")?;
    let publish_dir = Path::new(&publish_dir);

    if !publish_dir.exists() {
        bail!("Publish path does not exist: {}", publish_dir.display());
    }

    let file_name = html_path
        .file_name()
        .context("HTML path has no filename")?;
    let dest = publish_dir.join(file_name);

    fs::copy(html_path, &dest)
        .with_context(|| format!("Failed to copy {} to {}", html_path.display(), dest.display()))?;

    eprintln!("Copied {} to {}", html_path.display(), dest.display());

    // Git add, commit, push in the publish repo
    let git = |args: &[&str]| -> Result<()> {
        let output = Command::new("git")
            .args(args)
            .current_dir(publish_dir)
            .output()
            .with_context(|| format!("Failed to run git {}", args.join(" ")))?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            bail!("git {} failed: {}", args.join(" "), stderr);
        }
        Ok(())
    };

    git(&["add", &file_name.to_string_lossy()])?;

    let commit_msg = format!("publish: {}", file_name.to_string_lossy());
    git(&["commit", "-m", &commit_msg])?;
    eprintln!("Committed: {}", commit_msg);

    git(&["push"])?;
    eprintln!("Pushed to remote");

    Ok(())
}
