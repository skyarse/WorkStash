use anyhow::{ensure, Context, Result};
use std::{path::Path, process::Command};

pub fn is_archive(path: &Path) -> bool {
    path.is_file()
        && path
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("jex"))
}

fn run(script: &str, root: &Path, argument: &std::ffi::OsStr) -> Result<String> {
    let output = Command::new("/usr/bin/python3")
        .args(["-I", "-c", script])
        .arg(root)
        .arg(argument)
        .output()
        .context("Не удалось запустить модуль Joplin")?;
    ensure!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr).trim()
    );
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

pub fn import(root: &Path, archive: &Path) -> Result<String> {
    run(include_str!("joplin/import.py"), root, archive.as_os_str())
}

pub fn read(root: &Path, entry: i64) -> Result<String> {
    run(
        include_str!("joplin/reader.py"),
        root,
        entry.to_string().as_ref(),
    )
}
