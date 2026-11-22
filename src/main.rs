mod desktop;
mod library;
mod remote;

use anyhow::{Context, Result};
use eframe::egui::{self, Color32, RichText};
use library::{Entry, Library};
use remote::{Choice, Remote, Resource, Snapshot};
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
    sync::mpsc,
    thread,
};

fn config_file() -> Option<PathBuf> {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|v| PathBuf::from(v).join(".config")))
        .map(|p| p.join("workstash/library"))
}
struct Draft {
    id: Option<i64>,
    section: i64,
    kind: String,
    title: String,
    body: String,
}
enum Reply {
    List(Vec<Choice>),
    Downloaded(Snapshot),
}
struct ServerPanel {
    base: String,
    key: String,
    kind: Resource,
    choices: Vec<Choice>,
    job: Option<mpsc::Receiver<Result<Reply>>>,
}
impl Default for ServerPanel {
    fn default() -> Self {
        Self {
            base: String::new(),
            key: String::new(),
            kind: Resource::Workflow,
            choices: Vec::new(),
            job: None,
        }
    }
}
impl ServerPanel {
    fn start(&mut self, id: Option<String>, ctx: egui::Context) -> Result<()> {
        let remote = Remote::new(&self.base, self.key.clone())?;
        let kind = self.kind;
        let (tx, rx) = mpsc::channel();
        self.job = Some(rx);
        thread::spawn(move || {
            let result = match id {
                Some(id) => remote.download(kind, &id).map(Reply::Downloaded),
                None => remote.list(kind).map(Reply::List),
            };
            let _ = tx.send(result);
            ctx.request_repaint();
        });
        Ok(())
    }
}
#[derive(Default)]
struct App {
    library: Option<Library>,
    entries: Vec<Entry>,
    sections: Vec<(i64, String)>,
    section: Option<i64>,
    selected: Option<i64>,
    query: String,
    section_name: String,
    adding_section: bool,
    draft: Option<Draft>,
    secret: String,
    server: Option<ServerPanel>,
    status: String,
}
impl App {
    fn new() -> Self {
        let mut app = Self::default();
        if let Some(path) = config_file().and_then(|p| fs::read_to_string(p).ok()) {
            let parent = Path::new(&path);
            if parent.join("WorkStash.library/catalog.sqlite3").is_file() {
                if let Err(e) = app.load(parent) {
                    app.status = format!("{e:#}")
                }
            }
        }
        app
    }
    fn load(&mut self, path: &Path) -> Result<()> {
        let lib = Library::open(path)?;
        let entries = lib.entries()?;
        let sections = lib.sections()?;
        self.library = Some(lib);
        self.entries = entries;
        self.sections = sections;
        self.selected = None;
        self.section = None;
        self.draft = None;
        self.server = None;
        self.secret.clear();
        self.query.clear();
        self.status.clear();
        Ok(())
    }
    fn choose_library(&mut self) -> Result<()> {
        if let Some(path) = desktop::choose(true)? {
            let path = if path.file_name().is_some_and(|s| s == "WorkStash.library")
                && path.join("catalog.sqlite3").is_file()
            {
                path.parent()
                    .context("Не найдена родительская папка")?
                    .to_owned()
            } else {
                path
            };
            self.load(&path)?;
            if let Some(file) = config_file() {
                fs::create_dir_all(file.parent().unwrap())?;
                let mut tmp = tempfile::NamedTempFile::new_in(file.parent().unwrap())?;
                use std::io::Write;
                tmp.write_all(path.to_str().context("Путь не в UTF-8")?.as_bytes())?;
                tmp.persist(file)?;
            }
        }
        Ok(())
    }
    fn reload(&mut self) -> Result<()> {
        let lib = self.library.as_ref().context("Откройте библиотеку")?;
        self.sections = lib.sections()?;
        self.entries = lib.entries()?;
        Ok(())
    }
    fn report(&mut self, r: Result<()>) {
        if let Err(e) = r {
            self.status = format!("{e:#}")
        }
    }
    fn import(&mut self, paths: Vec<PathBuf>) -> Result<()> {
        let lib = self.library.as_ref().context("Откройте библиотеку")?;
        let mut failures = Vec::new();
        let mut count = 0;
        for p in paths {
            match lib.import(self.section.unwrap_or(1), &p) {
                Ok(()) => count += 1,
                Err(e) => failures.push(format!("{}: {e:#}", p.display())),
            }
        }
        self.reload()?;
        self.status = format!("Добавлено: {count}. {}", failures.join("\n"));
        Ok(())
    }
    fn new_draft(&mut self, kind: &str) {
        self.draft = Some(Draft {
            id: None,
            section: self.section.unwrap_or(1),
            kind: kind.into(),
            title: String::new(),
            body: String::new(),
        });
    }
    fn sidebar(&mut self, ui: &mut egui::Ui) {
        ui.heading("WorkStash");
        ui.add_space(18.);
        if ui
            .selectable_label(self.section.is_none(), "Все материалы")
            .clicked()
        {
            self.section = None;
            self.selected = None;
            self.secret.clear();
        }
        ui.add_space(12.);
        ui.label(RichText::new("Разделы").weak());
        for (id, name) in &self.sections {
            if ui
                .selectable_label(self.section == Some(*id), name)
                .clicked()
            {
                self.section = Some(*id);
                self.selected = None;
                self.secret.clear();
            }
        }
        if ui.small_button("+ Раздел").clicked() {
            self.adding_section = true;
        }
        if self.adding_section {
            ui.text_edit_singleline(&mut self.section_name);
            if ui.button("Создать раздел").clicked() {
                let result = self
                    .library
                    .as_ref()
                    .unwrap()
                    .add_section(&self.section_name)
                    .and_then(|id| {
                        self.section = Some(id);
                        self.selected = None;
                        self.secret.clear();
                        self.section_name.clear();
                        self.adding_section = false;
                        self.reload()
                    });
                self.report(result);
            }
        }
        ui.with_layout(egui::Layout::bottom_up(egui::Align::LEFT), |ui| {
            if ui.button("Сменить библиотеку…").clicked() {
                let r = self.choose_library();
                self.report(r);
            }
            if ui.button("Подключить n8n…").clicked() {
                self.server.get_or_insert_with(Default::default);
            }
        });
    }
    fn catalog(&mut self, ui: &mut egui::Ui) {
        ui.heading(
            self.sections
                .iter()
                .find(|(id, _)| Some(*id) == self.section)
                .map(|s| s.1.as_str())
                .unwrap_or("Все материалы"),
        );
        ui.add_space(8.);
        ui.add(
            egui::TextEdit::singleline(&mut self.query)
                .hint_text("Поиск по названию и содержимому")
                .desired_width(f32::INFINITY),
        );
        ui.add_space(8.);
        ui.horizontal_wrapped(|ui| {
            for k in ["Заметка", "Промпт", "Credential"] {
                if ui.button(format!("+ {k}")).clicked() {
                    self.new_draft(k);
                }
            }
            for (name, folder) in [("Файл…", false), ("Папка…", true)] {
                if ui.button(name).clicked() {
                    let r = desktop::choose(folder).and_then(|p| match p {
                        Some(path) => self.import(vec![path]),
                        None => Ok(()),
                    });
                    self.report(r);
                }
            }
        });
        ui.separator();
        let q = self.query.trim().to_lowercase();
        let mut any = false;
        egui::ScrollArea::vertical()
            .id_salt("catalog")
            .show(ui, |ui| {
                for e in &self.entries {
                    if self.section.is_some_and(|s| s != e.section)
                        || !(q.is_empty()
                            || e.title.to_lowercase().contains(&q)
                            || e.kind.to_lowercase().contains(&q)
                            || e.body.to_lowercase().contains(&q))
                    {
                        continue;
                    }
                    any = true;
                    ui.label(RichText::new(&e.kind).small().weak());
                    if ui
                        .selectable_label(
                            self.selected == Some(e.id),
                            RichText::new(&e.title).size(17.),
                        )
                        .clicked()
                    {
                        self.selected = Some(e.id);
                        self.secret.clear();
                    }
                    ui.separator();
                }
                if !any {
                    ui.label(if q.is_empty() {
                        "Перетащите сюда файл или папку либо создайте запись."
                    } else {
                        "Ничего не найдено."
                    });
                }
            });
    }
    fn details(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let Some(e) = self.entries.iter().find(|e| Some(e.id) == self.selected) else {
            return;
        };
        ui.label(RichText::new(&e.kind).weak());
        ui.heading(&e.title);
        ui.add_space(10.);
        if ["Заметка", "Промпт", "Credential"].contains(&e.kind.as_str())
            && ui.button("Редактировать").clicked()
        {
            self.draft = Some(Draft {
                id: Some(e.id),
                section: e.section,
                kind: e.kind.clone(),
                title: e.title.clone(),
                body: e.body.clone(),
            });
        }
        if e.kind == "Промпт" && ui.button("Скопировать промпт").clicked() {
            ctx.copy_text(e.body.clone());
            self.status = "Промпт скопирован".into();
        }
        if let Some(relative) = &e.asset {
            let root = &self.library.as_ref().unwrap().root;
            if ui.button("Показать в файловом менеджере").clicked() {
                let r = (|| -> Result<()> {
                    let path = root.join(relative).canonicalize()?;
                    anyhow::ensure!(
                        path.starts_with(root.join("files")),
                        "Копия находится вне библиотеки"
                    );
                    let dir = if path.is_dir() {
                        path.as_path()
                    } else {
                        path.parent().context("Нет папки")?
                    };
                    Command::new("xdg-open").arg(dir).spawn()?;
                    Ok(())
                })();
                if let Err(err) = r {
                    self.status = format!("{err:#}")
                }
            }
        }
        if e.kind == "Credential" {
            ui.separator();
            ui.label("Секрет в системном хранилище");
            ui.add(
                egui::TextEdit::singleline(&mut self.secret)
                    .password(true)
                    .hint_text("Новое значение"),
            );
            if ui
                .add_enabled(
                    !self.secret.is_empty(),
                    egui::Button::new("Сохранить секрет"),
                )
                .clicked()
            {
                match desktop::store_secret(&e.token, &self.secret) {
                    Ok(()) => {
                        self.secret.clear();
                        self.status = "Секрет сохранён".into()
                    }
                    Err(err) => self.status = format!("{err:#}"),
                }
            }
            if ui.button("Скопировать сохранённый секрет").clicked() {
                match desktop::read_secret(&e.token) {
                    Ok(value) => {
                        ctx.copy_text(value);
                        self.status = "Секрет в системном буфере обмена".into()
                    }
                    Err(err) => self.status = format!("{err:#}"),
                }
            }
            ui.separator();
            ui.label("Workflow с таким именем credential");
            for w in self.entries.iter().filter(|w| {
                w.kind == "Workflow"
                    && w.body
                        .split_once("\n\nCredentials\n")
                        .is_some_and(|(_, refs)| {
                            refs.lines().any(|line| {
                                line.split_once(" · ")
                                    .is_some_and(|(name, _)| name == e.title)
                            })
                        })
            }) {
                if ui.link(&w.title).clicked() {
                    self.selected = Some(w.id);
                    self.secret.clear();
                }
            }
        }
        ui.separator();
        egui::ScrollArea::vertical().id_salt("body").show(ui, |ui| {
            ui.label(&e.body);
        });
    }
    fn editor(&mut self, ctx: &egui::Context) {
        let Some(d) = self.draft.as_mut() else { return };
        let mut save = false;
        let mut open = true;
        egui::Window::new(format!("{} · запись", d.kind))
            .open(&mut open)
            .collapsible(false)
            .default_width(450.)
            .show(ctx, |ui| {
                ui.label("Название");
                ui.text_edit_singleline(&mut d.title);
                egui::ComboBox::from_label("Раздел")
                    .selected_text(
                        self.sections
                            .iter()
                            .find(|s| s.0 == d.section)
                            .map(|s| &*s.1)
                            .unwrap_or(""),
                    )
                    .show_ui(ui, |ui| {
                        for (id, name) in &self.sections {
                            ui.selectable_value(&mut d.section, *id, name);
                        }
                    });
                ui.label(if d.kind == "Credential" {
                    "Описание доступа (секрет добавляется отдельно)"
                } else {
                    "Текст"
                });
                ui.add(
                    egui::TextEdit::multiline(&mut d.body)
                        .desired_rows(9)
                        .desired_width(f32::INFINITY),
                );
                save = ui
                    .add_enabled(!d.title.trim().is_empty(), egui::Button::new("Сохранить"))
                    .clicked();
            });
        if save {
            match self
                .library
                .as_ref()
                .unwrap()
                .save_text(d.id, d.section, &d.kind, &d.title, &d.body)
            {
                Ok(id) => {
                    self.section = Some(d.section);
                    self.selected = Some(id);
                    self.secret.clear();
                    self.query.clear();
                    open = false;
                    let r = self.reload();
                    self.report(r);
                }
                Err(e) => self.status = format!("{e:#}"),
            }
        }
        if !open {
            self.draft = None;
        }
    }
    fn server_panel(&mut self, ctx: &egui::Context) {
        let Some(p) = self.server.as_mut() else {
            return;
        };
        let mut open = true;
        let mut received = None;
        if let Some(rx) = &p.job {
            match rx.try_recv() {
                Ok(msg) => {
                    received = Some(msg);
                    p.job = None
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    p.job = None;
                    self.status = "Загрузка прервана".into()
                }
                _ => {}
            }
        }
        egui::Window::new("Импорт из n8n")
            .open(&mut open)
            .collapsible(false)
            .show(ctx, |ui| {
                ui.add_enabled_ui(p.job.is_none(), |ui| {
                    ui.label("Адрес сервера");
                    if ui
                        .add(
                            egui::TextEdit::singleline(&mut p.base)
                                .hint_text("https://n8n.example.org"),
                        )
                        .changed()
                    {
                        p.choices.clear();
                    }
                    ui.label("API key · только для этого подключения");
                    if ui
                        .add(egui::TextEdit::singleline(&mut p.key).password(true))
                        .changed()
                    {
                        p.choices.clear();
                    }
                    ui.horizontal(|ui| {
                        for (k, name) in [
                            (Resource::Workflow, "Workflow"),
                            (Resource::Table, "Data Tables"),
                        ] {
                            if ui.selectable_value(&mut p.kind, k, name).changed() {
                                p.choices.clear();
                            }
                        }
                    });
                    if ui.button("Загрузить список").clicked() {
                        p.choices.clear();
                        if let Err(e) = p.start(None, ctx.clone()) {
                            self.status = format!("{e:#}")
                        }
                    }
                    let mut chosen = None;
                    egui::ScrollArea::vertical()
                        .max_height(260.)
                        .show(ui, |ui| {
                            for row in &p.choices {
                                if ui
                                    .button(&row.name)
                                    .on_hover_text("Сохранить или обновить локальную копию")
                                    .clicked()
                                {
                                    chosen = Some(row.id.clone())
                                }
                            }
                        });
                    if let Some(id) = chosen {
                        if let Err(e) = p.start(Some(id), ctx.clone()) {
                            self.status = format!("{e:#}")
                        }
                    }
                });
                if p.job.is_some() {
                    ui.spinner();
                }
            });
        match received {
            Some(Ok(Reply::List(rows))) => {
                self.status = format!("Найдено: {}", rows.len());
                p.choices = rows;
            }
            Some(Ok(Reply::Downloaded(s))) => {
                let r = self
                    .library
                    .as_ref()
                    .unwrap()
                    .snapshot(
                        self.section.unwrap_or(1),
                        s.kind.label(),
                        &s.source,
                        &s.title,
                        &s.value,
                    )
                    .and_then(|_| self.reload());
                if r.is_ok() {
                    self.status = format!("Сохранено: {}", s.title)
                }
                self.report(r);
            }
            Some(Err(e)) => self.status = format!("{e:#}"),
            None => {}
        }
        if !open {
            self.server = None
        }
    }
}
impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _: &mut eframe::Frame) {
        if !self.status.is_empty() {
            egui::TopBottomPanel::bottom("status").show(ctx, |ui| {
                ui.horizontal_wrapped(|ui| {
                    if ui.small_button("×").clicked() {
                        self.status.clear();
                    }
                    ui.label(&self.status);
                });
            });
        }
        if self.library.is_none() {
            egui::CentralPanel::default().show(ctx, |ui| {
                ui.vertical_centered(|ui| {
                    ui.add_space(100.);
                    ui.heading("WorkStash");
                    ui.add_space(16.);
                    ui.label("Выберите папку для новой или существующей библиотеки.");
                    if ui.button("Выбрать папку…").clicked() {
                        let r = self.choose_library();
                        self.report(r);
                    }
                });
            });
            return;
        }
        let paths: Vec<_> = ctx.input(|i| {
            i.raw
                .dropped_files
                .iter()
                .filter_map(|f| f.path.clone())
                .collect()
        });
        if !paths.is_empty() {
            let r = self.import(paths);
            self.report(r);
        }
        egui::SidePanel::left("sections")
            .default_width(176.)
            .show(ctx, |ui| self.sidebar(ui));
        if self.selected.is_some() {
            egui::SidePanel::right("details")
                .default_width(340.)
                .width_range(220.0..=650.0)
                .show(ctx, |ui| self.details(ui, ctx));
        }
        egui::CentralPanel::default().show(ctx, |ui| self.catalog(ui));
        self.editor(ctx);
        self.server_panel(ctx);
    }
}
fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1080., 700.])
            .with_min_inner_size([720., 480.])
            .with_app_id("WorkStash"),
        ..Default::default()
    };
    eframe::run_native(
        "WorkStash",
        options,
        Box::new(|cc| {
            cc.egui_ctx.style_mut(|s| {
                s.spacing.item_spacing = egui::vec2(9., 8.);
                s.visuals.selection.bg_fill = Color32::from_rgb(59, 100, 130);
            });
            Ok(Box::new(App::new()))
        }),
    )
}
