use crate::{joplin, library::Library};
use anyhow::Result;
use eframe::egui;
use std::{path::PathBuf, sync::mpsc, thread};

pub enum Update {
    Progress(usize),
    Finished(Result<String>),
}

pub struct ImportJob {
    pub updates: mpsc::Receiver<Update>,
    pub done: usize,
    pub total: usize,
}

impl ImportJob {
    pub fn start(parent: PathBuf, section: i64, paths: Vec<PathBuf>, ctx: egui::Context) -> Self {
        let total = paths.len();
        let (send, updates) = mpsc::channel();
        thread::spawn(move || {
            let result = (|| {
                let library = Library::open(&parent)?;
                let mut messages = Vec::new();
                let mut added = 0;
                let mut failed = 0;
                for (index, path) in paths.into_iter().enumerate() {
                    let result = if joplin::is_archive(&path) {
                        joplin::import(&library.root, &path)
                    } else {
                        library.import(section, &path).map(|()| String::new())
                    };
                    match result {
                        Ok(message) if message.is_empty() => added += 1,
                        Ok(message) => messages.push(message),
                        Err(error) => {
                            failed += 1;
                            if failed <= 3 {
                                messages.push(format!("{}: {error:#}", path.display()));
                            }
                        }
                    }
                    let _ = send.send(Update::Progress(index + 1));
                    ctx.request_repaint();
                }
                if added > 0 || failed > 0 {
                    messages.insert(0, format!("Добавлено: {added}. Ошибок: {failed}."));
                }
                Ok(messages.join("\n"))
            })();
            let _ = send.send(Update::Finished(result));
            ctx.request_repaint();
        });
        Self {
            updates,
            done: 0,
            total,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, time::Duration};

    #[test]
    fn progress_and_results_include_good_files_from_a_mixed_batch() -> Result<()> {
        let folder = tempfile::tempdir()?;
        let workspace = tempfile::tempdir()?;
        let note = folder.path().join("Заметка.txt");
        fs::write(&note, "Содержимое")?;
        let library = Library::open(workspace.path())?;
        let section = library.add_section("Проект")?;
        let job = ImportJob::start(
            workspace.path().to_owned(),
            section,
            vec![note, folder.path().join("missing.txt")],
            egui::Context::default(),
        );
        let mut progress = Vec::new();
        let message = loop {
            match job.updates.recv_timeout(Duration::from_secs(5))? {
                Update::Progress(done) => progress.push(done),
                Update::Finished(result) => break result?,
            }
        };
        assert_eq!(progress, [1, 2]);
        assert!(message.contains("Добавлено: 1. Ошибок: 1."));
        let entries = library.entries()?;
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].section, section);
        assert_eq!(entries[0].body, "Содержимое");
        Ok(())
    }
}
