use anyhow::{bail, ensure, Context, Result};
use rusqlite::{params, Connection};
use serde_json::Value;
use std::{
    collections::BTreeSet,
    fs,
    io::Read,
    path::{Path, PathBuf},
};

use std::os::unix::fs::DirBuilderExt;

pub struct Library {
    pub root: PathBuf,
    pub db: Connection,
}

#[derive(Clone)]
pub struct Entry {
    pub id: i64,
    pub section: i64,
    pub kind: String,
    pub title: String,
    pub body: String,
    pub asset: Option<String>,
    pub token: String,
}

impl Library {
    pub fn open(parent: &Path) -> Result<Self> {
        ensure!(parent.is_dir(), "Выберите существующую папку");
        let root = parent.canonicalize()?.join("WorkStash.library");
        fs::DirBuilder::new()
            .mode(0o700)
            .recursive(true)
            .create(root.join("files"))?;
        let db = Connection::open(root.join("catalog.sqlite3"))?;
        db.busy_timeout(std::time::Duration::from_secs(4))?;
        db.execute_batch("PRAGMA foreign_keys=ON;
            BEGIN;
            CREATE TABLE IF NOT EXISTS sections(id INTEGER PRIMARY KEY, name TEXT UNIQUE NOT NULL);
            INSERT OR IGNORE INTO sections VALUES (1,'Входящие');
            CREATE TABLE IF NOT EXISTS entries(
              id INTEGER PRIMARY KEY, section INTEGER NOT NULL REFERENCES sections(id),
              kind TEXT NOT NULL,title TEXT NOT NULL,body TEXT NOT NULL DEFAULT '',
              asset TEXT,source TEXT UNIQUE,token TEXT NOT NULL UNIQUE DEFAULT (lower(hex(randomblob(16))))
            ); COMMIT;")?;
        Ok(Self { root, db })
    }
    pub fn sections(&self) -> Result<Vec<(i64, String)>> {
        let mut stmt = self
            .db
            .prepare("SELECT id,name FROM sections ORDER BY id")?;
        let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }
    pub fn add_section(&self, name: &str) -> Result<i64> {
        ensure!(!name.trim().is_empty(), "Введите название раздела");
        self.db
            .execute("INSERT INTO sections(name) VALUES(?)", [name.trim()])
            .context("Не удалось создать раздел. Проверьте, свободно ли название")?;
        Ok(self.db.last_insert_rowid())
    }
    pub fn entries(&self) -> Result<Vec<Entry>> {
        let mut stmt = self.db.prepare(
            "SELECT id,section,kind,title,body,asset,token FROM entries ORDER BY id DESC",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(Entry {
                id: r.get(0)?,
                section: r.get(1)?,
                kind: r.get(2)?,
                title: r.get(3)?,
                body: r.get(4)?,
                asset: r.get(5)?,
                token: r.get(6)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }
    pub fn save_text(
        &self,
        id: Option<i64>,
        section: i64,
        kind: &str,
        title: &str,
        body: &str,
    ) -> Result<i64> {
        ensure!(!title.trim().is_empty(), "Введите название");
        if let Some(id) = id {
            ensure!(self.db.execute("UPDATE entries SET title=?,body=?,section=? WHERE id=? AND kind IN ('Заметка','Промпт','Credential')",params![title.trim(),body,section,id])?==1,"Запись недоступна");
            return Ok(id);
        }
        self.db.execute(
            "INSERT INTO entries(section,kind,title,body) VALUES(?,?,?,?)",
            params![section, kind, title.trim(), body],
        )?;
        Ok(self.db.last_insert_rowid())
    }
    pub fn import(&self, section: i64, path: &Path) -> Result<()> {
        let meta = fs::symlink_metadata(path)?;
        ensure!(
            !meta.is_symlink(),
            "Выберите исходный файл вместо символической ссылки"
        );
        let source = path.canonicalize()?;
        ensure!(
            !self.root.starts_with(&source) && !source.starts_with(&self.root),
            "Нельзя импортировать библиотеку в саму себя"
        );
        let name = source.file_name().context("У файла нет названия")?;
        let files = self.root.join("files");
        let stage = tempfile::Builder::new()
            .prefix("item-")
            .tempdir_in(&files)?;
        let copy = stage.path().join(name);
        copy_item(&source, &copy)?;
        let mut title = name.to_string_lossy().into_owned();
        let mut body = String::new();
        let kind = if copy.is_dir() {
            if copy.join("SKILL.md").is_file() {
                body = preview(&copy.join("SKILL.md"))?;
                "Навык"
            } else {
                "Папка"
            }
        } else {
            match copy
                .extension()
                .and_then(|s| s.to_str())
                .unwrap_or("")
                .to_ascii_lowercase()
                .as_str()
            {
                "json" if meta.len() <= 32 * 1024 * 1024 => {
                    let json = fs::read(&copy)?;
                    match serde_json::from_slice::<Value>(&json) {
                        Ok(v) if v["nodes"].is_array() => {
                            title = v["name"].as_str().unwrap_or(&title).to_string();
                            body = workflow_summary(&v);
                            "Workflow"
                        }
                        _ => "Файл",
                    }
                }
                "csv" => {
                    body = preview(&copy)?;
                    "Таблица"
                }
                "txt" | "md" => {
                    body = preview(&copy)?;
                    "Документ"
                }
                _ => "Файл",
            }
        };
        let relative = copy
            .strip_prefix(&self.root)?
            .to_str()
            .context("Имя файла не в UTF-8")?;
        self.db.execute(
            "INSERT INTO entries(section,kind,title,body,asset) VALUES(?,?,?,?,?)",
            params![section, kind, title, body, relative],
        )?;
        let _ = stage.keep();
        Ok(())
    }
    pub fn snapshot(
        &self,
        section: i64,
        kind: &str,
        source: &str,
        title: &str,
        value: &Value,
    ) -> Result<()> {
        let body = if kind == "Workflow" {
            workflow_summary(value)
        } else {
            format!("Строк: {}", value["rows"].as_array().map_or(0, Vec::len))
        };
        let files = self.root.join("files");
        let stage = tempfile::Builder::new()
            .prefix("snapshot-")
            .tempdir_in(&files)?;
        let file = stage.path().join("snapshot.json");
        serde_json::to_writer_pretty(fs::File::create(&file)?, value)?;
        let relative = file
            .strip_prefix(&self.root)?
            .to_str()
            .context("Путь не в UTF-8")?;
        // Keep earlier snapshots on disk so an interrupted refresh cannot destroy the prior copy.
        self.db.execute("INSERT INTO entries(section,kind,title,body,source,asset) VALUES(?,?,?,?,?,?)
            ON CONFLICT(source) DO UPDATE SET title=excluded.title,body=excluded.body,asset=excluded.asset",params![section,kind,title,body,source,relative])?;
        let _ = stage.keep();
        Ok(())
    }
}

fn preview(path: &Path) -> Result<String> {
    let mut data = Vec::new();
    fs::File::open(path)?
        .take(64 * 1024)
        .read_to_end(&mut data)?;
    Ok(String::from_utf8_lossy(&data).into_owned())
}

fn copy_item(src: &Path, dst: &Path) -> Result<()> {
    let t = fs::symlink_metadata(src)?.file_type();
    if t.is_dir() {
        fs::create_dir(dst)?;
        for child in fs::read_dir(src)? {
            let child = child?;
            copy_item(&child.path(), &dst.join(child.file_name()))?;
        }
    } else if t.is_file() {
        fs::copy(src, dst)?;
    } else {
        bail!(
            "Ссылки и специальные файлы не импортируются: {}",
            src.display()
        )
    }
    Ok(())
}

pub fn workflow_summary(v: &Value) -> String {
    let mut creds = BTreeSet::new();
    let mut nodes = Vec::new();
    for node in v["nodes"].as_array().into_iter().flatten() {
        if let Some(n) = node["name"].as_str() {
            nodes.push(n)
        }
        for (kind, c) in node["credentials"].as_object().into_iter().flatten() {
            creds.insert(format!(
                "{} · {} ({})",
                c["name"].as_str().or(c.as_str()).unwrap_or(kind),
                kind,
                c["id"].as_str().unwrap_or("—")
            ));
        }
    }
    format!(
        "Узлы ({})\n{}\n\nCredentials\n{}",
        nodes.len(),
        nodes.join("\n"),
        creds.into_iter().collect::<Vec<_>>().join("\n")
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn reopen_and_find_imported_skill_and_workflow() -> Result<()> {
        let d = tempfile::tempdir()?;
        let folder = d.path().join("sample");
        fs::create_dir(&folder)?;
        fs::write(folder.join("SKILL.md"), "layout")?;
        let w = d.path().join("example.json");
        fs::write(
            &w,
            r#"{"name":"Sort","nodes":[{"name":"Send","credentials":{"http":{"name":"Demo","id":"12"}}}]}"#,
        )?;
        let db = Library::open(d.path())?;
        let s = db.add_section("Проекты")?;
        db.import(s, &folder)?;
        db.import(s, &w)?;
        assert!(db.import(s, d.path()).is_err());
        drop(db);
        fs::remove_dir_all(folder)?;
        let db = Library::open(d.path())?;
        let items = db.entries()?;
        assert_eq!(items.len(), 2);
        assert!(items[0].body.contains("Demo"));
        assert_eq!(items[1].section, s);
        assert!(db
            .root
            .join(items[1].asset.as_ref().unwrap())
            .join("SKILL.md")
            .exists());
        Ok(())
    }
    #[test]
    fn refresh_updates_one_entry_and_preserves_its_section() -> Result<()> {
        let d = tempfile::tempdir()?;
        let db = Library::open(d.path())?;
        let s = db.add_section("A")?;
        for (section, name) in [(s, "Old"), (1, "New")] {
            db.snapshot(
                section,
                "Workflow",
                "remote:1",
                name,
                &serde_json::json!({"nodes":[]}),
            )?;
        }
        let e = db.entries()?;
        assert_eq!(e.len(), 1);
        assert_eq!((&e[0].title, e[0].section), (&"New".into(), s));
        Ok(())
    }
}

#[cfg(test)]
mod editing_tests {
    use super::*;
    #[test]
    fn edits_persist_and_failed_folder_import_leaves_no_partial_copy() -> Result<()> {
        let d = tempfile::tempdir()?;
        let db = Library::open(d.path())?;
        let section = db.add_section("Работа")?;
        let id = db.save_text(None, 1, "Промпт", "Черновик", "текст")?;
        db.save_text(Some(id), section, "Промпт", "Готово", "новый текст")?;
        let bad = d.path().join("invalid");
        fs::create_dir(&bad)?;
        std::os::unix::fs::symlink(d.path(), bad.join("loop"))?;
        assert!(db.import(1, &bad).is_err());
        assert_eq!(fs::read_dir(db.root.join("files"))?.count(), 0);
        drop(db);
        let entries = Library::open(d.path())?.entries()?;
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].body, "новый текст");
        assert_eq!(entries[0].section, section);
        Ok(())
    }
}
