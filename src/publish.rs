use std::fs;
use std::path::Path;
use std::process::Command;
use std::thread;
use std::time::Duration;

use anyhow::{bail, Context, Result};

/// Derive the GitHub Pages URL from a git remote URL.
/// e.g. "git@github.com:User/repo.git" → "https://user.github.io/repo/"
/// e.g. "https://github.com/User/repo.git" → "https://user.github.io/repo/"
fn github_pages_url(remote_url: &str) -> Option<String> {
    // SSH format: git@github.com:User/repo.git
    if let Some(rest) = remote_url.strip_prefix("git@github.com:") {
        let rest = rest.trim_end_matches(".git").trim();
        let parts: Vec<&str> = rest.splitn(2, '/').collect();
        if parts.len() == 2 {
            return Some(format!(
                "https://{}.github.io/{}/",
                parts[0].to_lowercase(),
                parts[1]
            ));
        }
    }
    // HTTPS format: https://github.com/User/repo.git
    if let Some(rest) = remote_url
        .strip_prefix("https://github.com/")
        .or_else(|| remote_url.strip_prefix("http://github.com/"))
    {
        let rest = rest.trim_end_matches(".git").trim();
        let parts: Vec<&str> = rest.splitn(2, '/').collect();
        if parts.len() == 2 {
            return Some(format!(
                "https://{}.github.io/{}/",
                parts[0].to_lowercase(),
                parts[1]
            ));
        }
    }
    None
}

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
    let git = |args: &[&str]| -> Result<String> {
        let output = Command::new("git")
            .args(args)
            .current_dir(publish_dir)
            .output()
            .with_context(|| format!("Failed to run git {}", args.join(" ")))?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            bail!("git {} failed: {}", args.join(" "), stderr);
        }
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    };

    git(&["add", &file_name.to_string_lossy()])?;

    let commit_msg = format!("publish: {}", file_name.to_string_lossy());
    git(&["commit", "-m", &commit_msg])?;
    eprintln!("Committed: {}", commit_msg);

    git(&["push"])?;
    eprintln!("Pushed to remote");

    // Derive GitHub Pages URL and poll until the page is live
    let remote_url = git(&["remote", "get-url", "origin"]).unwrap_or_default();
    if let Some(base_url) = github_pages_url(&remote_url) {
        let page_url = format!("{}{}", base_url, file_name.to_string_lossy());
        eprintln!("Waiting for GitHub Pages deploy: {}", page_url);

        let max_attempts = 30; // 30 * 5s = 150s max
        let mut published = false;
        for attempt in 1..=max_attempts {
            let result = Command::new("curl")
                .args(&["-s", "-o", "/dev/null", "-w", "%{http_code}", &page_url])
                .output();

            if let Ok(output) = result {
                let status = String::from_utf8_lossy(&output.stdout).trim().to_string();
                if status == "200" {
                    published = true;
                    break;
                }
                eprint!("  attempt {}/{}: HTTP {} ", attempt, max_attempts, status);
            } else {
                eprint!("  attempt {}/{}: curl failed ", attempt, max_attempts);
            }

            if attempt < max_attempts {
                eprintln!("(retrying in 5s)");
                thread::sleep(Duration::from_secs(5));
            } else {
                eprintln!();
            }
        }

        if published {
            eprintln!("Published! Opening {}", page_url);
            let _ = Command::new("open").arg(&page_url).spawn();
        } else {
            eprintln!("Timed out waiting for deploy. URL: {}", page_url);
            eprintln!("The page should appear shortly at the URL above.");
        }
    }

    Ok(())
}
