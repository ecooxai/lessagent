use crate::{Result, err};
use serde_json::{Value, json};
use std::path::Path;
use tokio::process::Command;

async fn run(path: &Path, args: &[&str]) -> Result<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(path)
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
        .await?;
    if !output.status.success() {
        return Err(err(String::from_utf8_lossy(&output.stderr)
            .trim()
            .to_owned()));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}
async fn root(path: &Path) -> Result<()> {
    let top = run(path, &["rev-parse", "--show-toplevel"]).await?;
    if Path::new(top.trim()).canonicalize()? != path.canonicalize()? {
        return Err(err(
            "Open the repository root as a workspace to manage Git changes",
        ));
    }
    Ok(())
}
pub async fn status(path: &Path) -> Result<Value> {
    root(path).await?;
    let branch = run(path, &["symbolic-ref", "--quiet", "--short", "HEAD"])
        .await
        .unwrap_or_else(|_| "Detached HEAD".into());
    let refs = run(path, &["for-each-ref", "--format=%(refname)\t%(refname:short)\t%(objectname:short)\t%(subject)\t%(committerdate:iso8601)\t%(upstream:short)", "refs/heads/", "refs/remotes/"]).await?;
    let branches: Vec<Value> = refs.lines().filter_map(|line| {
        let fields: Vec<_> = line.splitn(6, '\t').collect();
        if fields.len() != 6 || fields[0].ends_with("/HEAD") { return None; }
        Some(json!({"ref":fields[0],"name":fields[1],"commit":fields[2],"subject":fields[3],"date":fields[4],"upstream":fields[5],"remote":fields[0].starts_with("refs/remotes/")}))
    }).collect();
    let changes = run(
        path,
        &[
            "-c",
            "status.renames=false",
            "status",
            "--porcelain=v1",
            "-z",
            "--untracked-files=all",
        ],
    )
    .await?;
    let mut counts = std::collections::HashMap::<String, (u64, u64, bool)>::new();
    for args in [
        vec![
            "diff",
            "--numstat",
            "-z",
            "--no-renames",
            "--no-ext-diff",
            "--no-textconv",
        ],
        vec![
            "diff",
            "--cached",
            "--numstat",
            "-z",
            "--no-renames",
            "--no-ext-diff",
            "--no-textconv",
        ],
    ] {
        for line in run(path, &args)
            .await?
            .split('\0')
            .filter(|v| !v.is_empty())
        {
            let fields: Vec<_> = line.splitn(3, '\t').collect();
            if fields.len() == 3 {
                let c = counts.entry(fields[2].into()).or_default();
                c.0 += fields[0].parse::<u64>().unwrap_or(0);
                c.1 += fields[1].parse::<u64>().unwrap_or(0);
                c.2 |= fields[0] == "-";
            }
        }
    }
    let mut files = vec![];
    for line in changes.split('\0').filter(|v| v.len() >= 3) {
        let name = &line[3..];
        let (mut added, deleted, mut binary) = counts.get(name).copied().unwrap_or_default();
        if line.starts_with("??")
            && let Ok(file) = crate::context::resolve(path, name, false)
        {
            if file.is_file() && file.metadata()?.len() <= 2 * 1024 * 1024 {
                let bytes = std::fs::read(file)?;
                binary = bytes.contains(&0) || std::str::from_utf8(&bytes).is_err();
                if !binary {
                    added = String::from_utf8_lossy(&bytes).lines().count() as u64;
                }
            } else {
                binary = true;
            }
        }
        files.push(json!({"path":name,"status":&line[..2],"added":added,"deleted":deleted,"binary":binary}));
    }
    Ok(json!({"branch":branch.trim(),"branches":branches,"files":files}))
}
pub async fn file_diff(path: &Path, name: &str) -> Result<Value> {
    root(path).await?;
    let snapshot = status(path).await?;
    let file = snapshot["files"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["path"] == name)
        .ok_or_else(|| err("File is no longer changed"))?;
    let literal = format!(":(literal){name}");
    let unstaged = run(
        path,
        &["diff", "--no-ext-diff", "--no-textconv", "--", &literal],
    )
    .await?;
    let staged = run(
        path,
        &[
            "diff",
            "--cached",
            "--no-ext-diff",
            "--no-textconv",
            "--",
            &literal,
        ],
    )
    .await?;
    let mut untracked = String::new();
    if file["status"] == "??" {
        if file["binary"] == true {
            untracked = "Binary or large untracked file; preview unavailable".into();
        } else {
            let file = crate::context::resolve(path, name, false)?;
            let text = std::fs::read_to_string(file)?;
            untracked = format!(
                "--- /dev/null\n+++ {name}\n@@ -0,0 +1,{} @@\n",
                text.lines().count()
            );
            for line in text.lines() {
                untracked.push('+');
                untracked.push_str(line);
                untracked.push('\n');
            }
        }
    }
    Ok(
        json!({"unstaged":crate::clip(&unstaged,200000),"staged":crate::clip(&staged,200000),"untracked":crate::clip(&untracked,200000)}),
    )
}
pub async fn branch_info(path: &Path, branch: &str) -> Result<Value> {
    root(path).await?;
    if !(branch.starts_with("refs/heads/") || branch.starts_with("refs/remotes/")) {
        return Err(err("Select a listed branch"));
    }
    let commit = run(
        path,
        &["rev-parse", "--verify", &format!("{branch}^{{commit}}")],
    )
    .await?;
    let diff = run(
        path,
        &[
            "diff",
            "--no-ext-diff",
            "--no-textconv",
            "HEAD",
            commit.trim(),
            "--",
        ],
    )
    .await?;
    let counts = run(
        path,
        &[
            "rev-list",
            "--left-right",
            "--count",
            &format!("HEAD...{}", commit.trim()),
        ],
    )
    .await?;
    let nums: Vec<_> = counts.split_whitespace().collect();
    Ok(
        json!({"diff":crate::clip(&diff,200000),"behind":nums.first(),"ahead":nums.get(1),"commit":commit.trim()}),
    )
}

pub async fn action(path: &Path, action: &str, branch: &str, confirmed: bool) -> Result<Value> {
    root(path).await?;
    match action {
        "git_switch" | "git_create" => {
            if branch.starts_with('-') || branch.is_empty() {
                return Err(err("Invalid branch name"));
            }
            if branch.starts_with("refs/remotes/") && action == "git_switch" {
                run(path, &["switch", "--track", branch]).await?;
                return status(path).await;
            }
            run(path, &["check-ref-format", "--branch", branch]).await?;
            if action == "git_create" {
                run(path, &["switch", "-c", branch]).await?;
            } else {
                run(path, &["switch", "--no-guess", branch]).await?;
            }
        }
        "git_discard" => {
            if !confirmed {
                return Err(err("Discard requires explicit confirmation"));
            }
            // Require a committed HEAD before changing anything; never remove ignored files.
            run(path, &["rev-parse", "--verify", "HEAD"]).await?;
            run(path, &["reset", "--hard", "HEAD"]).await?;
            run(path, &["clean", "-fd"]).await?;
        }
        _ => return Err(err("Unknown Git action")),
    }
    status(path).await
}
#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn branches_and_discard() {
        let dir = std::env::temp_dir().join(format!("lessagent-git-{}", crate::id()));
        std::fs::create_dir_all(&dir).unwrap();
        run(&dir, &["init", "-b", "main"]).await.unwrap();
        run(&dir, &["config", "user.email", "test@example.com"])
            .await
            .unwrap();
        run(&dir, &["config", "user.name", "Test"]).await.unwrap();
        std::fs::write(dir.join("file"), "original").unwrap();
        run(&dir, &["add", "file"]).await.unwrap();
        run(&dir, &["commit", "-m", "initial"]).await.unwrap();
        action(&dir, "git_create", "feature", false).await.unwrap();
        assert_eq!(status(&dir).await.unwrap()["branch"], "feature");
        std::fs::write(dir.join("file"), "feature\n").unwrap();
        run(&dir, &["commit", "-am", "feature change"])
            .await
            .unwrap();
        action(&dir, "git_switch", "main", false).await.unwrap();
        let branch = branch_info(&dir, "refs/heads/feature").await.unwrap();
        assert_eq!(branch["ahead"], "1");
        assert!(branch["diff"].as_str().unwrap().contains("+feature"));
        std::fs::write(dir.join("file"), "changed").unwrap();
        std::fs::write(dir.join("untracked"), "new").unwrap();
        let snapshot = status(&dir).await.unwrap();
        let changed = snapshot["files"]
            .as_array()
            .unwrap()
            .iter()
            .find(|f| f["path"] == "file")
            .unwrap();
        assert_eq!(changed["added"], 1);
        assert_eq!(changed["deleted"], 1);
        assert!(
            file_diff(&dir, "file").await.unwrap()["unstaged"]
                .as_str()
                .unwrap()
                .contains("+changed")
        );
        assert!(
            file_diff(&dir, "untracked").await.unwrap()["untracked"]
                .as_str()
                .unwrap()
                .contains("+new")
        );
        std::fs::write(dir.join("odd\tname\n.txt"), "new\n").unwrap();
        assert!(
            file_diff(&dir, "odd\tname\n.txt").await.unwrap()["untracked"]
                .as_str()
                .unwrap()
                .contains("+new")
        );
        assert!(file_diff(&dir, "../outside").await.is_err());
        assert!(action(&dir, "git_discard", "", false).await.is_err());
        action(&dir, "git_discard", "", true).await.unwrap();
        assert_eq!(
            std::fs::read_to_string(dir.join("file")).unwrap(),
            "original"
        );
        assert!(!dir.join("untracked").exists());
        std::fs::remove_dir_all(dir).unwrap();
    }
}
