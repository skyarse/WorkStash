use anyhow::{ensure, Context, Result};
use eframe::egui;
use serde_json::Value;
use std::{
    collections::HashSet,
    fs,
    path::{Path, PathBuf},
};

const PAGE: usize = 25;

enum Source {
    Snapshot(Vec<Value>),
    Csv {
        reader: csv::Reader<fs::File>,
        starts: Vec<csv::Position>,
        has_next: bool,
    },
}

pub struct Table {
    headers: Vec<String>,
    source: Source,
    cells: Vec<Vec<String>>,
    page: usize,
}

impl Table {
    pub fn open(root: &Path, relative: &str) -> Result<Self> {
        let path = asset_path(root, relative)?;
        let (headers, source) = if path
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("csv"))
        {
            let mut reader = csv::ReaderBuilder::new().from_path(path)?;
            let headers = reader.headers()?.iter().map(str::to_owned).collect();
            let starts = vec![reader.position().clone()];
            (
                headers,
                Source::Csv {
                    reader,
                    starts,
                    has_next: false,
                },
            )
        } else {
            let mut snapshot: Value =
                serde_json::from_reader(std::io::BufReader::new(fs::File::open(path)?))?;
            let rows = snapshot["rows"]
                .as_array_mut()
                .context("В сохранённой таблице нет строк")?;
            let rows = std::mem::take(rows);
            let mut seen = HashSet::new();
            let fields = snapshot["table"]["columns"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|c| c["name"].as_str());
            let keys = rows
                .iter()
                .filter_map(Value::as_object)
                .flat_map(|r| r.keys().map(String::as_str));
            let headers = fields
                .chain(keys)
                .filter(|k| seen.insert(*k))
                .map(str::to_owned)
                .collect();
            (headers, Source::Snapshot(rows))
        };
        let mut table = Self {
            headers,
            source,
            cells: Vec::new(),
            page: 0,
        };
        table.go(0)?;
        Ok(table)
    }

    fn go(&mut self, page: usize) -> Result<()> {
        self.cells = match &mut self.source {
            Source::Snapshot(rows) => rows
                .iter()
                .skip(page * PAGE)
                .take(PAGE)
                .map(|row| {
                    self.headers
                        .iter()
                        .map(|key| match row.get(key) {
                            None => String::new(),
                            Some(Value::String(s)) => s.clone(),
                            Some(v) => v.to_string(),
                        })
                        .collect()
                })
                .collect(),
            Source::Csv {
                reader,
                starts,
                has_next,
            } => {
                reader.seek(starts.get(page).context("Страница недоступна")?.clone())?;
                let mut records = Vec::new();
                for record in reader.records().take(PAGE) {
                    records.push(record?.iter().map(str::to_owned).collect());
                }
                let next = reader.position().clone();
                *has_next = reader.read_byte_record(&mut csv::ByteRecord::new())?;
                if *has_next && page + 1 == starts.len() {
                    starts.push(next);
                }
                records
            }
        };
        self.page = page;
        Ok(())
    }

    pub fn show(&mut self, ui: &mut egui::Ui) -> Result<()> {
        let first = self.page * PAGE;
        let last = first + self.cells.len();
        let (has_next, count) = match &self.source {
            Source::Snapshot(rows) => (last < rows.len(), format!(" из {}", rows.len())),
            Source::Csv { has_next, .. } => (*has_next, String::new()),
        };
        let mut page = self.page;
        ui.horizontal_wrapped(|ui| {
            if self.cells.is_empty() {
                ui.label("Нет строк");
            } else {
                ui.label(format!("Строки {}–{}{}", first + 1, last, count));
            }
            if page > 0 || has_next {
                if ui
                    .add_enabled(self.page > 0, egui::Button::new("Назад"))
                    .clicked()
                {
                    page -= 1;
                }
                if ui
                    .add_enabled(has_next, egui::Button::new("Далее"))
                    .clicked()
                {
                    page += 1;
                }
            }
        });
        egui::ScrollArea::both()
            .auto_shrink([false, false])
            .id_salt("table-scroll")
            .show(ui, |ui| {
                egui::Grid::new("table-grid")
                    .striped(true)
                    .min_col_width(72.)
                    .max_col_width(250.)
                    .show(ui, |ui| {
                        for h in &self.headers {
                            ui.strong(h);
                        }
                        ui.end_row();
                        for row in &self.cells {
                            for cell in row {
                                ui.add(egui::Label::new(cell).wrap());
                            }
                            ui.end_row();
                        }
                    });
            });
        if page != self.page {
            self.go(page)?;
        }
        Ok(())
    }
}

pub fn asset_path(root: &Path, relative: &str) -> Result<PathBuf> {
    let path = root.join(relative).canonicalize()?;
    ensure!(
        path.starts_with(root.join("files")),
        "Копия находится вне библиотеки"
    );
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::library::Library;
    #[test]
    fn reopen_existing_snapshot_and_csv() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let lib = Library::open(dir.path())?;
        let snapshot = serde_json::json!({"table":{"columns":[{"name":"z"},{"name":"a"}]},"rows":[{"a":false,"z":"пример","id":1}]});
        lib.snapshot(1, "Таблица", "example", "demo", &snapshot)?;
        let csv = dir.path().join("demo.CSV");
        let mut text = String::from("name,text\r\n");
        for n in 0..26 {
            text.push_str(&format!("{n},\"one, two\nthree\"\r\n"));
        }
        fs::write(&csv, text)?;
        lib.import(1, &csv)?;
        drop(lib);
        let lib = Library::open(dir.path())?;
        let entries = lib.entries()?;
        let mut csv = Table::open(&lib.root, entries[0].asset.as_deref().unwrap())?;
        assert_eq!(csv.cells[0], ["0", "one, two\nthree"]);
        csv.go(1)?;
        assert_eq!(csv.cells.len(), 1);
        assert_eq!(csv.cells[0][0], "25");
        csv.go(0)?;
        assert_eq!(csv.cells.len(), 25);
        let table = Table::open(&lib.root, entries[1].asset.as_deref().unwrap())?;
        assert_eq!(table.headers, ["z", "a", "id"]);
        assert_eq!(table.cells[0], ["пример", "false", "1"]);
        Ok(())
    }
}
