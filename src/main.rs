mod appearance;
mod desktop;
mod library;
mod panel;
mod remote;
mod table;

use anyhow::{Context, Result};
use eframe::egui::{self, Color32, RichText};
use library::{Entry, Library};
use panel::{OpenPanel, Panel};
use remote::{Choice, Remote, Resource, Snapshot};
use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
    process::Command,
    sync::mpsc,
    thread,
};

fn kind_name(kind: &str) -> &str {
    if kind == "Заметка" {
        "Текстовая запись"
    } else if kind == "Credential" {
        "Карточка доступа"
    } else {
        kind
    }
}
fn tab_name(kind: &str) -> &str {
    match kind {
        "Таблица" => "Таблицы",
        "Заметка" => "Текстовые записи",
        "Промпт" => "Промпты",
        "Credential" => "Доступы",
        "Навык" => "Навыки",
        "Папка" => "Папки",
        "Документ" => "Документы",
        "Файл" => "Файлы",
        other => other,
    }
}
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
    destination: i64,
    choices: Vec<Choice>,
    job: Option<mpsc::Receiver<Result<Reply>>>,
}
impl Default for ServerPanel {
    fn default() -> Self {
        Self {
            base: String::new(),
            key: String::new(),
            kind: Resource::Workflow,
            destination: 1,
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
    appearance: Option<appearance::Appearance>,
    panel: Option<OpenPanel>,
    selecting: bool,
    marked: BTreeSet<i64>,
    moving: Vec<i64>,
    kind_filter: Option<String>,
    table: Option<(String, Result<table::Table>)>,
    library: Option<Library>,
    entries: Vec<Entry>,
    sections: Vec<(i64, String)>,
    section: Option<i64>,
    selected: Option<i64>,
    query: String,
    section_name: String,

    draft: Option<Draft>,
    secret: String,
    server: Option<ServerPanel>,
    status: String,
}
impl App {
    fn new(ctx: &egui::Context) -> Self {
        let mut app = Self {
            appearance: Some(appearance::Appearance::new(ctx)),
            ..Self::default()
        };
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
        self.kind_filter = None;
        self.table = None;
        self.panel = None;
        self.selecting = false;
        self.marked.clear();
        self.moving.clear();
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
        self.open_draft(Draft {
            id: None,
            section: self.section.unwrap_or(1),
            kind: kind.into(),
            title: String::new(),
            body: String::new(),
        });
    }
    fn open_draft(&mut self, mut draft: Draft) {
        let slot = library::draft_slot(draft.id, &draft.kind);
        match self.library.as_ref().unwrap().draft(&slot) {
            Ok(Some((section, kind, title, body))) => {
                draft.section = section;
                draft.kind = kind;
                draft.title = title;
                draft.body = body;
            }
            Ok(None) => {}
            Err(err) => {
                self.status = format!("{err:#}");
                self.panel = None;
                return;
            }
        }
        self.draft = Some(draft);
        if let Some(panel) = &mut self.panel {
            panel.kind = Panel::Editor;
        }
    }
    fn trigger(&mut self, r: &egui::Response, kind: Panel) {
        if let Some(panel) = &mut self.panel {
            panel.track(r);
        }
        if r.clicked() {
            let closing = self
                .panel
                .is_some_and(|p| p.kind == kind && p.anchored_at(r));
            self.panel = if closing {
                None
            } else {
                Some(OpenPanel::at(kind, r))
            };
        }
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
            self.kind_filter = None;
            self.marked.clear();
            self.selecting = false;
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
                self.kind_filter = None;
                self.marked.clear();
                self.selecting = false;
                self.secret.clear();
            }
        }
        let button = ui.small_button("+ Раздел");
        self.trigger(&button, Panel::Section);
        ui.with_layout(egui::Layout::bottom_up(egui::Align::LEFT), |ui| {
            if ui.button("Сменить библиотеку…").clicked() {
                let r = self.choose_library();
                self.report(r);
            }
            let button = ui.button("Настройки…");
            self.trigger(&button, Panel::Settings);
        });
    }
    fn catalog(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            let section = self
                .sections
                .iter()
                .find(|s| Some(s.0) == self.section)
                .map(|s| s.1.as_str())
                .unwrap_or("Все материалы");
            ui.heading(section);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let button = ui.button("+ Добавить");
                self.trigger(&button, Panel::Add);
                if ui.selectable_label(self.selecting, "Выбрать").clicked() {
                    self.selecting = !self.selecting;
                    self.marked.clear();
                }
            });
        });
        ui.add_space(8.);
        ui.add(
            egui::TextEdit::singleline(&mut self.query)
                .hint_text("Поиск по названию и содержимому")
                .desired_width(f32::INFINITY),
        );
        let kinds: std::collections::BTreeSet<_> = self
            .entries
            .iter()
            .filter(|e| self.section.is_none_or(|s| e.section == s))
            .map(|e| e.kind.as_str())
            .collect();
        if self
            .kind_filter
            .as_deref()
            .is_some_and(|k| !kinds.contains(k))
        {
            self.kind_filter = None;
        }
        if !kinds.is_empty() {
            ui.horizontal_wrapped(|ui| {
                if ui
                    .selectable_label(self.kind_filter.is_none(), "Все")
                    .clicked()
                {
                    self.kind_filter = None;
                }
                for kind in kinds {
                    if ui
                        .selectable_label(self.kind_filter.as_deref() == Some(kind), tab_name(kind))
                        .clicked()
                    {
                        self.kind_filter = Some(kind.to_owned());
                        if self
                            .entries
                            .iter()
                            .any(|e| Some(e.id) == self.selected && e.kind != kind)
                        {
                            self.selected = None;
                            self.secret.clear();
                        }
                    }
                }
            });
        }
        ui.separator();
        let q = self.query.trim().to_lowercase();
        let visible: BTreeSet<_> = self
            .entries
            .iter()
            .filter(|e| {
                self.section.is_none_or(|s| s == e.section)
                    && self.kind_filter.as_ref().is_none_or(|k| k == &e.kind)
                    && (q.is_empty()
                        || [&e.title, &e.kind, &e.body]
                            .iter()
                            .any(|v| v.to_lowercase().contains(&q))
                        || kind_name(&e.kind).to_lowercase().contains(&q))
            })
            .map(|e| e.id)
            .collect();
        self.marked.retain(|id| visible.contains(id));
        if self.selecting {
            ui.horizontal_wrapped(|ui| {
                let mut all = !visible.is_empty() && self.marked == visible;
                if ui.checkbox(&mut all, "Все в списке").changed() {
                    self.marked = if all {
                        visible.clone()
                    } else {
                        BTreeSet::new()
                    };
                }
                let button = ui.add_enabled(
                    !self.marked.is_empty(),
                    egui::Button::new(format!("Переместить в раздел… ({})", self.marked.len())),
                );
                if button.clicked() {
                    self.moving = self.marked.iter().copied().collect();
                }
                self.trigger(&button, Panel::Move);
            });
        }
        let mut any = false;
        egui::ScrollArea::vertical()
            .id_salt("catalog")
            .show(ui, |ui| {
                for e in &self.entries {
                    if !visible.contains(&e.id) {
                        continue;
                    }
                    any = true;
                    ui.label(RichText::new(kind_name(&e.kind)).small());
                    if self.selecting {
                        let mut checked = self.marked.contains(&e.id);
                        if ui.checkbox(&mut checked, &e.title).changed() {
                            if checked {
                                self.marked.insert(e.id);
                            } else {
                                self.marked.remove(&e.id);
                            }
                        }
                    } else if ui
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
        ui.label(RichText::new(kind_name(&e.kind)));
        ui.heading(&e.title);
        ui.add_space(10.);
        let id = e.id;
        let move_button = ui.button("Переместить в раздел…");
        if move_button.clicked() {
            self.moving = vec![id];
        }
        self.trigger(&move_button, Panel::Move);
        let e = self.entries.iter().find(|e| e.id == id).unwrap();
        if ["Заметка", "Промпт", "Credential"].contains(&e.kind.as_str()) {
            let button = ui.button("Редактировать");
            if let Some(panel) = &mut self.panel {
                panel.track(&button);
            }
            if button.clicked() {
                let draft = Draft {
                    id: Some(id),
                    section: e.section,
                    kind: e.kind.clone(),
                    title: e.title.clone(),
                    body: e.body.clone(),
                };
                self.panel = Some(OpenPanel::at(Panel::Editor, &button));
                self.open_draft(draft);
                return;
            }
        }
        if ["Заметка", "Промпт"].contains(&e.kind.as_str()) && ui.button("Скопировать").clicked()
        {
            ctx.copy_text(e.body.clone());
            self.status = "Текст скопирован".into();
        }
        if let Some(relative) = &e.asset {
            let root = &self.library.as_ref().unwrap().root;
            if ui.button("Показать в файловом менеджере").clicked() {
                let r = (|| -> Result<()> {
                    let path = table::asset_path(root, relative)?;
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
                    self.section = Some(w.section);
                    self.kind_filter = Some(w.kind.clone());
                    self.query.clear();
                    self.selected = Some(w.id);
                    self.secret.clear();
                }
            }
        }
        ui.separator();
        if e.kind == "Таблица" {
            if let Some(path) = &e.asset {
                if self.table.as_ref().is_none_or(|(key, _)| key != path) {
                    self.table = Some((
                        path.clone(),
                        table::Table::open(&self.library.as_ref().unwrap().root, path),
                    ));
                }
                ui.push_id(e.id, |ui| match &mut self.table.as_mut().unwrap().1 {
                    Ok(table) => {
                        if let Err(err) = table.show(ui) {
                            self.status = format!("Не удалось открыть страницу: {err:#}");
                        }
                    }
                    Err(err) => {
                        ui.label(format!("Не удалось прочитать таблицу: {err:#}"));
                    }
                });
                return;
            }
        }
        egui::ScrollArea::vertical().id_salt("body").show(ui, |ui| {
            ui.label(&e.body);
        });
    }
    fn editor(&mut self, ui: &mut egui::Ui) {
        let Some(d) = self.draft.as_mut() else { return };
        ui.heading(if d.kind == "Credential" {
            "Карточка доступа"
        } else {
            "Текстовая запись"
        });
        ui.label("Название");
        let mut changed = ui.text_edit_singleline(&mut d.title).changed();
        if d.kind != "Credential" {
            let mut prompt = d.kind == "Промпт";
            if ui.checkbox(&mut prompt, "Промпт").changed() {
                d.kind = if prompt {
                    "Промпт"
                } else {
                    "Заметка"
                }
                .into();
                changed = true;
            }
        }
        ui.collapsing(
            format!(
                "Раздел · {}",
                self.sections
                    .iter()
                    .find(|s| s.0 == d.section)
                    .map(|s| s.1.as_str())
                    .unwrap_or("")
            ),
            |ui| {
                for (id, name) in &self.sections {
                    changed |= ui.selectable_value(&mut d.section, *id, name).changed();
                }
            },
        );
        ui.label(if d.kind == "Credential" {
            "Описание доступа (секрет добавляется отдельно)"
        } else {
            "Текст"
        });
        changed |= ui
            .add(
                egui::TextEdit::multiline(&mut d.body)
                    .desired_rows(9)
                    .desired_width(f32::INFINITY),
            )
            .changed();
        if changed {
            let slot = library::draft_slot(d.id, &d.kind);
            if let Err(err) = self
                .library
                .as_ref()
                .unwrap()
                .keep_draft(&slot, d.section, &d.kind, &d.title, &d.body)
            {
                self.status = format!("Не удалось сохранить черновик: {err:#}");
            }
        }
        let save = ui
            .add_enabled(!d.title.trim().is_empty(), egui::Button::new("Сохранить"))
            .clicked();
        if save {
            match self
                .library
                .as_ref()
                .unwrap()
                .save_text(d.id, d.section, &d.kind, &d.title, &d.body)
            {
                Ok(id) => {
                    self.section = Some(d.section);
                    self.kind_filter = Some(d.kind.clone());
                    self.selected = Some(id);
                    self.secret.clear();
                    self.query.clear();
                    self.panel = None;
                    self.draft = None;
                    let r = self.reload();
                    self.report(r);
                }
                Err(e) => self.status = format!("{e:#}"),
            }
        }
    }
    fn server_panel(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let Some(p) = self.server.as_mut() else {
            return;
        };
        ui.heading("Импорт из n8n");
        ui.add_enabled_ui(p.job.is_none(), |ui| {
            ui.label("Адрес сервера");
            if ui
                .add(egui::TextEdit::singleline(&mut p.base).hint_text("https://n8n.example.org"))
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
                p.destination = self.section.unwrap_or(1);
                if let Err(e) = p.start(Some(id), ctx.clone()) {
                    self.status = format!("{e:#}")
                }
            }
        });
        if p.job.is_some() {
            ui.spinner();
        }
    }
    fn poll_server(&mut self) {
        let Some(p) = self.server.as_mut() else {
            return;
        };
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
                    .snapshot(p.destination, s.kind.label(), &s.source, &s.title, &s.value)
                    .and_then(|_| self.reload());
                if r.is_ok() {
                    self.status = format!("Сохранено: {}", s.title)
                }
                self.report(r);
            }
            Some(Err(e)) => self.status = format!("{e:#}"),
            None => {}
        }
        if !self.panel.is_some_and(|p| p.kind == Panel::Server)
            && self.server.as_ref().is_some_and(|s| s.job.is_none())
        {
            self.server = None;
        }
    }
    fn popup(&mut self, ctx: &egui::Context) {
        let Some(open) = self.panel else { return };
        let bounds = open.draw(ctx, |ui| match open.kind {
            Panel::Add => {
                ui.label(RichText::new("Создать").small());
                for kind in ["Заметка", "Credential"] {
                    if ui.button(kind_name(kind)).clicked() {
                        self.new_draft(kind);
                    }
                }
                ui.separator();
                ui.label(RichText::new("Импортировать").small());
                for (label, folder) in [("Файл…", false), ("Папку…", true)] {
                    if ui.button(label).clicked() {
                        self.panel = None;
                        let r = desktop::choose(folder).and_then(|path| {
                            if let Some(path) = path {
                                self.import(vec![path])
                            } else {
                                Ok(())
                            }
                        });
                        self.report(r);
                    }
                }
                if ui.button("Из n8n…").clicked() {
                    self.server.get_or_insert_with(Default::default);
                    self.panel.as_mut().unwrap().kind = Panel::Server;
                }
            }
            Panel::Settings => {
                if let Some(a) = &mut self.appearance {
                    let r = a.settings(ui);
                    self.report(r);
                }
            }
            Panel::Editor => self.editor(ui),
            Panel::Server => self.server_panel(ui),
            Panel::Section => {
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
                            self.kind_filter = None;
                            self.secret.clear();
                            self.section_name.clear();
                            self.panel = None;
                            self.marked.clear();
                            self.selecting = false;
                            self.reload()
                        });
                    self.report(result);
                }
            }
            Panel::Move => {
                ui.strong("Переместить в раздел");
                let mut target = None;
                for (id, name) in &self.sections {
                    if ui.button(name).clicked() {
                        target = Some(*id);
                    }
                }
                if let Some(id) = target {
                    let r = self
                        .library
                        .as_mut()
                        .unwrap()
                        .move_entries(&self.moving, id)
                        .and_then(|_| self.reload());
                    if r.is_ok() {
                        self.status = format!("Перемещено записей: {}", self.moving.len());
                        self.panel = None;
                        self.marked.clear();
                        self.selected = None;
                        self.secret.clear();
                        self.selecting = false;
                    }
                    self.report(r);
                }
            }
        });
        if let Some(current) = &mut self.panel {
            if current.kind == open.kind {
                current.record_bounds(bounds);
            } else {
                ctx.request_repaint();
            }
        }
    }
    fn show(&mut self, ctx: &egui::Context) {
        if let Some(appearance) = &mut self.appearance {
            appearance.apply(ctx);
        }
        if self.panel.is_some_and(|p| p.should_close(ctx)) {
            self.panel = None;
        }
        self.poll_server();
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
                    let r = ui.button("Настройки…");
                    self.trigger(&r, Panel::Settings);
                    ui.label("Выберите папку для новой или существующей библиотеки.");
                    if ui.button("Выбрать папку…").clicked() {
                        let r = self.choose_library();
                        self.report(r);
                    }
                });
            });
            self.popup(ctx);
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
        self.popup(ctx);
    }
}
impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _: &mut eframe::Frame) {
        self.show(ctx);
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
            cc.egui_ctx.all_styles_mut(|s| {
                s.spacing.item_spacing = egui::vec2(9., 8.);
                if s.visuals.dark_mode {
                    s.visuals.widgets.noninteractive.fg_stroke.color = Color32::from_gray(210);
                    s.visuals.selection.bg_fill = Color32::from_rgb(52, 88, 115);
                }
            });
            Ok(Box::new(App::new(&cc.egui_ctx)))
        }),
    )
}

#[cfg(test)]
mod ui_tests {
    use super::*;

    fn catalog_frame(
        app: &mut App,
        ctx: &egui::Context,
        events: Vec<egui::Event>,
    ) -> Vec<(String, egui::Pos2)> {
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(950., 650.),
            )),
            events,
            ..Default::default()
        };
        ctx.run(input, |ctx| {
            app.show(ctx);
        })
        .shapes
        .into_iter()
        .filter_map(|s| {
            if let egui::Shape::Text(text) = s.shape {
                Some((
                    text.galley.job.text.clone(),
                    text.visual_bounding_rect().center(),
                ))
            } else {
                None
            }
        })
        .collect()
    }

    fn click(app: &mut App, ctx: &egui::Context, pos: egui::Pos2) {
        for pressed in [true, false] {
            catalog_frame(
                app,
                ctx,
                vec![
                    egui::Event::PointerMoved(pos),
                    egui::Event::PointerButton {
                        pos,
                        button: egui::PointerButton::Primary,
                        pressed,
                        modifiers: Default::default(),
                    },
                ],
            );
        }
        catalog_frame(app, ctx, vec![]);
    }

    #[test]
    fn panels_follow_buttons_and_switch_on_first_click() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let ctx = egui::Context::default();
        let mut app = App::default();
        app.load(dir.path())?;
        let labels = catalog_frame(&mut app, &ctx, vec![]);
        let point = |name: &str| labels.iter().find(|(s, _)| s == name).unwrap().1;
        let settings = point("Настройки…");
        click(&mut app, &ctx, settings);
        assert_eq!(app.panel.unwrap().kind, Panel::Settings);
        let rect = ctx
            .memory(|m| m.area_rect(egui::Id::new(("workstash-panel", Panel::Settings as u8))))
            .unwrap();
        assert!(rect.bottom() < settings.y && settings.y - rect.bottom() < 35.);
        click(&mut app, &ctx, point("+ Добавить"));
        assert_eq!(app.panel.unwrap().kind, Panel::Add);
        let menu = catalog_frame(&mut app, &ctx, vec![]);
        let text = menu
            .iter()
            .find(|(s, _)| s == "Текстовая запись")
            .unwrap()
            .1;
        click(&mut app, &ctx, text);
        assert_eq!(app.panel.unwrap().kind, Panel::Editor);
        click(&mut app, &ctx, settings);
        assert_eq!(app.panel.unwrap().kind, Panel::Settings);
        click(&mut app, &ctx, egui::pos2(550., 530.));
        assert!(app.panel.is_none());
        click(&mut app, &ctx, point("+ Добавить"));
        catalog_frame(
            &mut app,
            &ctx,
            vec![egui::Event::Key {
                key: egui::Key::Escape,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: Default::default(),
            }],
        );
        assert!(app.panel.is_none());
        Ok(())
    }
    #[test]
    fn table_tab_filters_existing_workflow_and_search() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let lib = Library::open(dir.path())?;
        let section = lib.add_section("n8n")?;
        lib.snapshot(
            section,
            "Workflow",
            "wf",
            "Пример workflow",
            &serde_json::json!({"nodes":[]}),
        )?;
        lib.snapshot(
            section,
            "Таблица",
            "tb",
            "Пример таблицы",
            &serde_json::json!({"rows":[]}),
        )?;
        drop(lib);
        let mut app = App::default();
        app.load(dir.path())?;
        app.section = Some(section);
        app.query = "пример".into();
        let ctx = egui::Context::default();
        let labels = catalog_frame(&mut app, &ctx, vec![]);
        assert!(labels.iter().any(|(s, _)| s == "Пример workflow"));
        let pos = labels.iter().find(|(s, _)| s == "Таблицы").unwrap().1;
        for pressed in [true, false] {
            catalog_frame(
                &mut app,
                &ctx,
                vec![
                    egui::Event::PointerMoved(pos),
                    egui::Event::PointerButton {
                        pos,
                        button: egui::PointerButton::Primary,
                        pressed,
                        modifiers: Default::default(),
                    },
                ],
            );
        }
        let labels = catalog_frame(&mut app, &ctx, vec![]);
        assert_eq!(app.kind_filter.as_deref(), Some("Таблица"));
        assert!(labels.iter().any(|(s, _)| s == "Пример таблицы"));
        assert!(!labels.iter().any(|(s, _)| s == "Пример workflow"));
        Ok(())
    }
}
