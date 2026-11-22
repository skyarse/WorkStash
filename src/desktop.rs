use anyhow::{ensure, Context, Result};
use std::{
    ffi::OsString,
    io::Write,
    os::unix::ffi::OsStringExt,
    path::PathBuf,
    process::{Command, Stdio},
};

pub fn choose(folder: bool) -> Result<Option<PathBuf>> {
    let r = Command::new("kdialog")
        .arg(if folder {
            "--getexistingdirectory"
        } else {
            "--getopenfilename"
        })
        .output()
        .context("Для выбора файлов нужен kdialog")?;
    if !r.status.success() {
        ensure!(r.status.code() == Some(1), "Ошибка kdialog");
        return Ok(None);
    }
    let mut bytes = r.stdout;
    if bytes.last() == Some(&b'\n') {
        bytes.pop();
    }
    Ok(Some(PathBuf::from(OsString::from_vec(bytes))))
}
pub fn store_secret(token: &str, secret: &str) -> Result<()> {
    ensure!(!secret.is_empty(), "Введите секрет");
    let mut child = Command::new("secret-tool")
        .args([
            "store",
            "--label=WorkStash",
            "application",
            "workstash",
            "entry",
            token,
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .context("Для хранения секретов нужен secret-tool из пакета libsecret")?;
    child
        .stdin
        .take()
        .context("Нет потока ввода")?
        .write_all(secret.as_bytes())?;
    ensure!(
        child.wait()?.success(),
        "Хранилище Secret Service недоступно или заблокировано"
    );
    Ok(())
}
pub fn read_secret(token: &str) -> Result<String> {
    let out = Command::new("secret-tool")
        .args(["lookup", "application", "workstash", "entry", token])
        .output()
        .context("Не удалось запустить secret-tool")?;
    ensure!(
        out.status.success(),
        "Секрет не найден или хранилище заблокировано"
    );
    let mut value = String::from_utf8(out.stdout)?;
    if value.ends_with('\n') {
        value.pop();
    }
    Ok(value)
}
