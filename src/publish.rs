use std::fs;
use std::path::Path;
use std::process::Command;
use std::thread;
use std::time::Duration;

use anyhow::{bail, Context, Result};

/// Get the GitHub Pages base URL via `gh api repos/{owner}/{repo}/pages`.
/// Extracts {owner}/{repo} from the git remote URL.
fn github_pages_url(publish_dir: &Path) -> Option<String> {
    // Get owner/repo from remote
    let remote = Command::new("git")
        .args(&["remote", "get-url", "origin"])
        .current_dir(publish_dir)
        .output()
        .ok()?;
    let remote_url = String::from_utf8_lossy(&remote.stdout).trim().to_string();

    let owner_repo = if let Some(rest) = remote_url.strip_prefix("git@github.com:") {
        rest.trim_end_matches(".git").trim().to_string()
    } else if let Some(rest) = remote_url.strip_prefix("https://github.com/") {
        rest.trim_end_matches(".git").trim().to_string()
    } else {
        return None;
    };

    // Query GitHub API for the Pages URL
    let output = Command::new("gh")
        .args(&["api", &format!("repos/{}/pages", owner_repo), "--jq", ".html_url"])
        .output()
        .ok()?;

    if !output.status.success() { return None; }

    let url = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if url.is_empty() { return None; }

    // Ensure trailing slash
    Some(if url.ends_with('/') { url } else { format!("{}/", url) })
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

    // Derive GitHub Pages URL and poll until the page is live.
    // The URL path is the file's path relative to the repo root,
    // not just the filename (e.g. walkthroughs/file.html).
    let repo_root = git(&["rev-parse", "--show-toplevel"]).unwrap_or_default();
    let relative_path = if !repo_root.is_empty() {
        dest.strip_prefix(&repo_root)
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|_| file_name.to_string_lossy().to_string())
    } else {
        file_name.to_string_lossy().to_string()
    };
    if let Some(base_url) = github_pages_url(publish_dir) {
        let page_url = format!("{}{}", base_url, relative_path);
        eprintln!("Waiting for GitHub Pages deploy: {}", page_url);

        let max_attempts = 30; // 30 * 5s = 150s max
        let mut published = false;
        for attempt in 1..=max_attempts {
            let result = Command::new("curl")
                .args(&["-s", "-o", "/dev/null", "-w", "%{http_code}", &page_url])
                .output();

            if let Ok(output) = result {
                let status = String::from_utf8_lossy(&output.stdout).trim().to_string();
                // 302 means private GitHub Pages redirecting to auth; page is live
                if status == "200" || status == "302" {
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
