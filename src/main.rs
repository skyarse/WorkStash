mod appearance;
mod catalog;
mod desktop;
mod import;
mod joplin;
mod library;
mod panel;
mod remote;
mod table;

use anyhow::{ensure, Context, Result};
use catalog::{Catalog, Scope, Sort};
use desktop::Picker;
use eframe::egui::{self, RichText};
use import::ImportJob;
use library::{Entry, Library};
use panel::{OpenPanel, Panel};
use remote::{Choice, Remote, Resource, Snapshot};
use std::{
    cmp::Reverse,
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
fn is_text(kind: &str) -> bool {
    matches!(kind, "Заметка" | "Промпт" | "Joplin" | "Документ")
}
fn favorite_label(favorite: bool) -> &'static str {
    if favorite {
        "Убрать из избранного"
    } else {
        "В избранное"
    }
}
fn navigation_button(ui: &mut egui::Ui, selected: bool, title: &str) -> egui::Response {
    ui.add(
        egui::Button::new(title)
            .min_size(egui::vec2(ui.available_width(), 32.))
            .truncate()
            .corner_radius(7)
            .selected(selected),
    )
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
    catalog_view: Catalog,
    sort: Sort,
    focus_search: bool,
    table: Option<(String, Result<table::Table>)>,
    library: Option<Library>,
    entries: Vec<Entry>,
    sections: Vec<(i64, String)>,
    scope: Scope,
    selected: Option<i64>,
    query: String,
    section_name: String,

    draft: Option<Draft>,
    secret: String,
    server: Option<ServerPanel>,
    joplin_jobs: Vec<mpsc::Receiver<Result<String>>>,
    import_job: Option<ImportJob>,
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
        self.catalog_view.invalidate();
        self.sections = sections;
        self.selected = None;
        self.scope = Scope::All;
        self.kind_filter = None;
        self.table = None;
        self.panel = None;
        self.selecting = false;
        self.marked.clear();
        self.moving.clear();
        self.draft = None;
        self.server = None;
        self.joplin_jobs.clear();
        self.secret.clear();
        self.query.clear();
        self.status.clear();
        Ok(())
    }
    fn choose_library(&mut self) -> Result<()> {
        ensure!(self.import_job.is_none(), "Дождитесь завершения импорта");
        if let Some(path) = desktop::choose(Picker::Folder)? {
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
        self.catalog_view.invalidate();
        Ok(())
    }
    fn report(&mut self, r: Result<()>) {
        if let Err(e) = r {
            self.status = format!("{e:#}")
        }
    }
    fn joplin_job(
        &mut self,
        ctx: &egui::Context,
        work: impl FnOnce() -> Result<String> + Send + 'static,
    ) {
        let (tx, rx) = mpsc::channel();
        self.joplin_jobs.push(rx);
        let ctx = ctx.clone();
        thread::spawn(move || {
            let _ = tx.send(work());
            ctx.request_repaint();
        });
    }
    fn poll_joplin(&mut self) {
        let mut messages = Vec::new();
        self.joplin_jobs.retain(|rx| match rx.try_recv() {
            Ok(Ok(message)) => {
                if !message.is_empty() {
                    messages.push(message);
                }
                false
            }
            Ok(Err(error)) => {
                messages.push(format!("{error:#}"));
                false
            }
            Err(mpsc::TryRecvError::Empty) => true,
            Err(mpsc::TryRecvError::Disconnected) => {
                messages.push("Работа с Joplin прервана".into());
                false
            }
        });
        if !messages.is_empty() {
            self.status = messages.join("\n");
        }
    }
    fn import(&mut self, paths: Vec<PathBuf>, ctx: &egui::Context) -> Result<()> {
        if paths.is_empty() {
            return Ok(());
        }
        ensure!(self.import_job.is_none(), "Импорт уже идёт");
        let lib = self.library.as_ref().context("Откройте библиотеку")?;
        let parent = lib.root.parent().context("Папка библиотеки недоступна")?;
        self.status.clear();
        self.import_job = Some(ImportJob::start(
            parent.to_owned(),
            self.scope.section().unwrap_or(1),
            paths,
            ctx.clone(),
        ));
        Ok(())
    }
    fn poll_import(&mut self) {
        let Some(job) = &mut self.import_job else {
            return;
        };
        let result = loop {
            match job.updates.try_recv() {
                Ok(import::Update::Progress(done)) => job.done = done,
                Ok(import::Update::Finished(result)) => break result,
                Err(mpsc::TryRecvError::Empty) => return,
                Err(mpsc::TryRecvError::Disconnected) => {
                    break Err(anyhow::anyhow!("Импорт прерван"));
                }
            }
        };
        self.import_job = None;
        match result {
            Ok(message) => self.status = message,
            Err(error) => self.status = format!("{error:#}"),
        }
        let result = self.reload();
        self.report(result);
    }
    fn switch_scope(&mut self, scope: Scope) {
        self.scope = scope;
        self.selected = None;
        self.kind_filter = None;
        self.selecting = false;
        self.marked.clear();
        self.secret.clear();
    }
    fn entry_index(&self, id: i64) -> Option<usize> {
        self.entries
            .binary_search_by_key(&Reverse(id), |entry| Reverse(entry.id))
            .ok()
    }
    fn toggle_favorite(&mut self, index: usize) {
        let entry = &mut self.entries[index];
        let result = self
            .library
            .as_ref()
            .unwrap()
            .set_favorite(entry.id, !entry.favorite);
        match result {
            Ok(()) => {
                entry.favorite = !entry.favorite;
                self.catalog_view.invalidate();
            }
            Err(error) => self.status = format!("{error:#}"),
        }
    }
    fn new_draft(&mut self, kind: &str) {
        self.open_draft(Draft {
            id: None,
            section: self.scope.section().unwrap_or(1),
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
        ui.label(RichText::new("WorkStash").size(23.).strong());
        ui.add_space(13.);
        for (scope, label) in [
            (Scope::All, "Все материалы"),
            (Scope::Favorites, "Избранное"),
        ] {
            if navigation_button(ui, self.scope == scope, label).clicked() {
                self.switch_scope(scope);
            }
        }

        ui.add_space(14.);
        ui.horizontal(|ui| {
            ui.label(RichText::new("Разделы").strong().weak());
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let button = ui.button("+ Раздел");
                if button.clicked() {
                    self.section_name.clear();
                }
                self.trigger(&button, Panel::Section);
            });
        });
        let mut destination = None;
        egui::ScrollArea::vertical()
            .id_salt("sections-list")
            .max_height((ui.available_height() - 86.).max(35.))
            .show(ui, |ui| {
                for (id, name) in &self.sections {
                    let scope = Scope::Section(*id);
                    if navigation_button(ui, self.scope == scope, name)
                        .on_hover_text(name)
                        .clicked()
                    {
                        destination = Some(scope);
                    }
                }
            });
        if let Some(scope) = destination {
            self.switch_scope(scope);
        }

        ui.with_layout(egui::Layout::bottom_up(egui::Align::LEFT), |ui| {
            let button = ui.button("Настройки…");
            self.trigger(&button, Panel::Settings);
            if ui
                .add_enabled(
                    self.import_job.is_none(),
                    egui::Button::new("Сменить библиотеку…"),
                )
                .clicked()
            {
                let result = self.choose_library();
                self.report(result);
            }
        });
    }

    fn refresh_catalog(&mut self) {
        if !self.catalog_view.refresh(
            &self.entries,
            self.scope,
            self.sort,
            &mut self.kind_filter,
            &self.query,
        ) {
            return;
        }
        if !self.marked.is_empty() {
            let visible: BTreeSet<_> = self
                .catalog_view
                .rows
                .iter()
                .map(|&index| self.entries[index].id)
                .collect();
            self.marked.retain(|id| visible.contains(id));
        }
        if let Some(id) = self.selected {
            if !self
                .catalog_view
                .rows
                .iter()
                .any(|&index| self.entries[index].id == id)
            {
                self.selected = None;
                self.secret.clear();
            }
        }
    }

    fn catalog(&mut self, ui: &mut egui::Ui) {
        let title = match self.scope {
            Scope::All => "Все материалы",
            Scope::Favorites => "Избранное",
            Scope::Section(id) => self
                .sections
                .iter()
                .find(|s| s.0 == id)
                .map_or("Раздел", |s| &s.1),
        }
        .to_owned();
        ui.horizontal(|ui| {
            let header = egui::Label::new(RichText::new(&title).heading())
                .halign(egui::Align::LEFT)
                .truncate();
            ui.add_sized([(ui.available_width() - 126.).max(40.), 34.], header)
                .on_hover_text(&title);
            let button = ui.add_sized([112., 34.], appearance::primary_button(ui, "+ Добавить"));
            self.trigger(&button, Panel::Add);
        });
        if self.scope.section().is_some_and(|id| id != 1) {
            let button = ui.small_button("Переименовать раздел…");
            if button.clicked() {
                self.section_name = title;
            }
            self.trigger(&button, Panel::RenameSection);
        }
        ui.horizontal(|ui| {
            let input = ui
                .scope(|ui| {
                    ui.visuals_mut().widgets.inactive.bg_stroke = ui.visuals().window_stroke;
                    ui.add_sized(
                        [(ui.available_width() - 36.).max(70.), 33.],
                        egui::TextEdit::singleline(&mut self.query)
                            .hint_text("Поиск по названию и содержимому")
                            .background_color(ui.visuals().window_fill)
                            .margin(egui::vec2(10., 6.)),
                    )
                })
                .inner;
            if self.focus_search {
                input.request_focus();
                self.focus_search = false;
            }
            if ui
                .add_enabled(
                    !self.query.is_empty(),
                    egui::Button::new("×").min_size(egui::vec2(29., 30.)),
                )
                .on_hover_text("Очистить поиск")
                .clicked()
            {
                self.query.clear();
                input.request_focus();
            }
        });

        self.refresh_catalog();
        if !self.catalog_view.kinds.is_empty() {
            ui.horizontal_wrapped(|ui| {
                if ui
                    .selectable_label(self.kind_filter.is_none(), "Все")
                    .clicked()
                {
                    self.kind_filter = None;
                }
                for kind in &self.catalog_view.kinds {
                    if ui
                        .selectable_label(self.kind_filter.as_ref() == Some(kind), tab_name(kind))
                        .clicked()
                    {
                        self.kind_filter = Some(kind.clone());
                    }
                }
            });
        }
        ui.separator();
        ui.horizontal_wrapped(|ui| {
            egui::ComboBox::from_id_salt("catalog-sort")
                .selected_text(self.sort.label())
                .show_ui(ui, |ui| {
                    for order in Sort::OPTIONS {
                        ui.selectable_value(&mut self.sort, order, order.label());
                    }
                });
            if ui.selectable_label(self.selecting, "Выбрать").clicked() {
                self.selecting = !self.selecting;
                self.marked.clear();
            }
        });
        self.refresh_catalog();
        let count = self.catalog_view.rows.len();
        if self.selecting {
            ui.horizontal_wrapped(|ui| {
                let mut all = count > 0 && self.marked.len() == count;
                if ui
                    .add_enabled(count > 0, egui::Checkbox::new(&mut all, "Все в списке"))
                    .changed()
                {
                    self.marked.clear();
                    if all {
                        self.marked.extend(
                            self.catalog_view
                                .rows
                                .iter()
                                .map(|&index| self.entries[index].id),
                        );
                    }
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
        } else if count > 0 {
            ui.label(
                RichText::new(format!("Материалы · {count}"))
                    .small()
                    .color(appearance::secondary_text(ui)),
            );
        }
        if count == 0 {
            let message = if !self.query.trim().is_empty() {
                "Ничего не найдено. Попробуйте изменить запрос."
            } else if self.scope == Scope::Favorites {
                "Откройте материал и добавьте его в избранное."
            } else {
                "Перетащите сюда файл или папку либо создайте запись."
            };
            ui.vertical_centered(|ui| {
                ui.add_space(48.);
                ui.label(RichText::new(message).weak());
            });
            return;
        }

        let mut favorite = None;
        egui::ScrollArea::vertical()
            .id_salt("catalog")
            .auto_shrink([false, false])
            .show_rows(ui, 38., count, |ui, range| {
                for row in range {
                    let index = self.catalog_view.rows[row];
                    let entry = &self.entries[index];
                    ui.push_id(entry.id, |ui| {
                        ui.horizontal(|ui| {
                            if self.selecting {
                                let mut checked = self.marked.contains(&entry.id);
                                if ui.checkbox(&mut checked, "").changed() {
                                    if checked {
                                        self.marked.insert(entry.id);
                                    } else {
                                        self.marked.remove(&entry.id);
                                    }
                                }
                            }
                            let kind = kind_name(&entry.kind);
                            let metadata = if entry.favorite {
                                format!("★  {kind}")
                            } else {
                                kind.to_owned()
                            };
                            let row = ui.add_sized(
                                [ui.available_width(), 38.],
                                egui::Button::new(RichText::new(&entry.title).size(15.))
                                    .shortcut_text(
                                        RichText::new(metadata)
                                            .small()
                                            .color(appearance::secondary_text(ui)),
                                    )
                                    .truncate()
                                    .corner_radius(7)
                                    .selected(self.selected == Some(entry.id)),
                            );
                            if row.clicked() {
                                if self.selecting {
                                    if !self.marked.remove(&entry.id) {
                                        self.marked.insert(entry.id);
                                    }
                                } else {
                                    self.selected = Some(entry.id);
                                    self.secret.clear();
                                }
                            }
                            row.on_hover_text(&entry.title).context_menu(|ui| {
                                if ui.button(favorite_label(entry.favorite)).clicked() {
                                    favorite = Some(index);
                                    ui.close_menu();
                                }
                                if is_text(&entry.kind) && ui.button("Скопировать текст").clicked()
                                {
                                    ui.ctx().copy_text(entry.body.clone());
                                    self.status = "Текст скопирован".into();
                                    ui.close_menu();
                                }
                            });
                        });
                    });
                }
            });
        if let Some(index) = favorite {
            self.toggle_favorite(index);
        }
    }

    fn details(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let Some(index) = self.selected.and_then(|id| self.entry_index(id)) else {
            ui.vertical_centered(|ui| {
                ui.add_space(108.);
                ui.label(RichText::new("Выберите материал").size(18.).strong());
            });
            return;
        };
        let entry = &self.entries[index];
        let id = entry.id;
        let section = self
            .sections
            .iter()
            .find(|(id, _)| *id == entry.section)
            .map(|(_, name)| name.as_str())
            .unwrap_or("Раздел");
        ui.label(RichText::new(format!("{}  /  {section}", kind_name(&entry.kind))).weak());
        ui.heading(&entry.title);
        ui.add_space(4.);

        let mut editor = None;
        let mut open_item = false;
        let mut show_in_folder = false;
        let mut read_joplin = false;
        let mut favorite = false;
        let mut mover = None;
        ui.horizontal_wrapped(|ui| {
            if entry.kind == "Joplin" {
                read_joplin = ui
                    .add(appearance::primary_button(ui, "Открыть с оформлением…"))
                    .clicked();
            } else if entry.asset.is_some() {
                let label = if matches!(entry.kind.as_str(), "Папка" | "Навык") {
                    "Открыть папку"
                } else {
                    "Открыть файл"
                };
                open_item = ui.add(appearance::primary_button(ui, label)).clicked();
            }
            if matches!(
                entry.kind.as_str(),
                "Заметка" | "Промпт" | "Credential" | "Joplin"
            ) {
                let button = ui.button("Редактировать");
                if let Some(panel) = &mut self.panel {
                    panel.track(&button);
                }
                if button.clicked() {
                    editor = Some(button);
                }
            }
            if is_text(&entry.kind) && ui.button("Скопировать текст").clicked() {
                ctx.copy_text(entry.body.clone());
                self.status = "Текст скопирован".into();
            }
            favorite = ui.button(favorite_label(entry.favorite)).clicked();
            mover = Some(ui.button("Переместить в раздел…"));
            if entry.asset.is_some() {
                show_in_folder = ui.button("Показать в файловом менеджере").clicked();
            }
        });
        if let Some(button) = editor {
            let draft = Draft {
                id: Some(id),
                section: entry.section,
                kind: entry.kind.clone(),
                title: entry.title.clone(),
                body: entry.body.clone(),
            };
            self.panel = Some(OpenPanel::at(Panel::Editor, &button));
            self.open_draft(draft);
            return;
        }
        if open_item || show_in_folder {
            let result = (|| -> Result<()> {
                let root = &self.library.as_ref().unwrap().root;
                let path =
                    table::asset_path(root, entry.asset.as_deref().context("Файл недоступен")?)?;
                let target = if show_in_folder && !path.is_dir() {
                    path.parent().context("Нет папки")?
                } else {
                    path.as_path()
                };
                Command::new("xdg-open").arg(target).spawn()?;
                Ok(())
            })();
            self.report(result);
        }
        if read_joplin {
            let root = self.library.as_ref().unwrap().root.clone();
            self.joplin_job(ctx, move || joplin::read(&root, id));
        }
        if favorite {
            self.toggle_favorite(index);
        }
        if let Some(button) = mover {
            if button.clicked() {
                self.moving = vec![id];
            }
            self.trigger(&button, Panel::Move);
        }
        let e = &self.entries[index];
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
                    self.scope = Scope::Section(w.section);
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
        if matches!(d.kind.as_str(), "Заметка" | "Промпт") {
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
                    if d.id.is_none() || self.scope != Scope::Favorites {
                        self.scope = Scope::Section(d.section);
                    }
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
                p.destination = self.scope.section().unwrap_or(1);
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
                for (label, picker) in [
                    ("Файл…", Picker::File),
                    ("Папку…", Picker::Folder),
                    ("Из Joplin (.jex)…", Picker::Joplin),
                ] {
                    if ui
                        .add_enabled(self.import_job.is_none(), egui::Button::new(label))
                        .clicked()
                    {
                        self.panel = None;
                        let r = desktop::choose(picker).and_then(|path| {
                            if let Some(path) = path {
                                self.import(vec![path], ctx)
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
            Panel::Section | Panel::RenameSection => {
                let rename = open.kind == Panel::RenameSection;
                let label = if rename {
                    "Переименовать"
                } else {
                    "Создать раздел"
                };
                ui.label("Название раздела");
                let input = ui.text_edit_singleline(&mut self.section_name);
                let button = ui.add_enabled(
                    !self.section_name.trim().is_empty(),
                    appearance::primary_button(ui, label),
                );
                if button.clicked()
                    || (input.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)))
                {
                    let result = (|| -> Result<()> {
                        let library = self.library.as_ref().unwrap();
                        let id = if rename {
                            let id = self.scope.section().context("Раздел недоступен")?;
                            library.rename_section(id, &self.section_name)?;
                            id
                        } else {
                            library.add_section(&self.section_name)?
                        };
                        self.sections = library.sections()?;
                        if !rename {
                            self.switch_scope(Scope::Section(id));
                        }
                        self.section_name.clear();
                        self.panel = None;
                        Ok(())
                    })();
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
        self.poll_joplin();
        self.poll_import();
        if let Some((asset, _)) = &self.table {
            let selected_asset = self
                .selected
                .and_then(|id| self.entry_index(id))
                .and_then(|index| self.entries[index].asset.as_deref());
            if selected_asset != Some(asset.as_str()) {
                self.table = None;
            }
        }
        if self.import_job.is_some() || !self.status.is_empty() {
            egui::TopBottomPanel::bottom("status").show(ctx, |ui| {
                if let Some(job) = &self.import_job {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label(format!("Импорт: {} из {}", job.done, job.total));
                        ui.add(
                            egui::ProgressBar::new(job.done as f32 / job.total as f32)
                                .desired_width(130.),
                        );
                    });
                }
                if !self.status.is_empty() {
                    ui.horizontal(|ui| {
                        if ui
                            .small_button("×")
                            .on_hover_text("Закрыть сообщение")
                            .clicked()
                        {
                            self.status.clear();
                        }
                        ui.add(egui::Label::new(&self.status).truncate())
                            .on_hover_text(&self.status);
                    });
                }
            });
        }
        if self.library.is_none() {
            egui::CentralPanel::default().show(ctx, |ui| {
                ui.vertical_centered(|ui| {
                    ui.add_space((ui.available_height() * 0.25).min(178.));
                    ui.label(RichText::new("WorkStash").size(34.).strong());
                    ui.add_space(16.);
                    ui.label("Выберите папку для новой или существующей библиотеки.");
                    ui.add_space(8.);
                    if ui
                        .add(appearance::primary_button(ui, "Выбрать папку…"))
                        .clicked()
                    {
                        let result = self.choose_library();
                        self.report(result);
                    }
                    let button = ui.button("Настройки…");
                    self.trigger(&button, Panel::Settings);
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
            let r = self.import(paths, ctx);
            self.report(r);
        }
        let wide = ctx.screen_rect().width() >= 920.;
        if self.panel.is_none()
            && ctx.input_mut(|i| i.consume_key(egui::Modifiers::CTRL, egui::Key::F))
        {
            self.focus_search = true;
            if !wide {
                self.selected = None;
                self.secret.clear();
            }
        }
        let sidebar_fill = ctx.style().visuals.window_fill;
        let panel_fill = ctx.style().visuals.panel_fill;
        egui::SidePanel::left("sections")
            .default_width(206.)
            .width_range(180.0..=260.)
            .frame(
                egui::Frame::new()
                    .fill(sidebar_fill)
                    .inner_margin(egui::Margin::symmetric(13, 16)),
            )
            .show(ctx, |ui| self.sidebar(ui));
        if wide {
            let max_width = (ctx.available_rect().width() * 0.55).max(290.);
            egui::SidePanel::right("details")
                .default_width(365.)
                .width_range(275.0..=max_width)
                .frame(
                    egui::Frame::new()
                        .fill(sidebar_fill)
                        .inner_margin(egui::Margin::symmetric(18, 17)),
                )
                .show(ctx, |ui| self.details(ui, ctx));
        }
        egui::CentralPanel::default()
            .frame(
                egui::Frame::new()
                    .fill(panel_fill)
                    .inner_margin(egui::Margin::symmetric(18, 16)),
            )
            .show(ctx, |ui| {
                if !wide && self.selected.is_some() {
                    if ui.button("← К материалам").clicked() {
                        self.selected = None;
                        self.secret.clear();
                    }
                    self.details(ui, ctx);
                } else {
                    self.catalog(ui);
                }
            });
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
            .with_inner_size([1160., 740.])
            .with_min_inner_size([720., 480.])
            .with_app_id("WorkStash"),
        ..Default::default()
    };
    eframe::run_native(
        "WorkStash",
        options,
        Box::new(|cc| {
            appearance::configure(&cc.egui_ctx);
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
        app.scope = Scope::Section(section);
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

    #[test]
    fn a_material_can_be_added_and_removed_from_favorites_from_the_details() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let library = Library::open(dir.path())?;
        let id = library.save_text(None, 1, "Заметка", "Рабочая запись", "Текст")?;
        drop(library);
        let mut app = App::default();
        app.load(dir.path())?;
        let ctx = egui::Context::default();
        appearance::configure(&ctx);
        let point = |app: &mut App, label| {
            catalog_frame(app, &ctx, vec![])
                .into_iter()
                .find(|(text, _)| text == label)
                .unwrap()
                .1
        };

        let row = point(&mut app, "Рабочая запись");
        click(&mut app, &ctx, row);
        assert_eq!(app.selected, Some(id));
        let favorite = point(&mut app, "В избранное");
        click(&mut app, &ctx, favorite);
        assert!(app.entries[0].favorite);

        let favorites = point(&mut app, "Избранное");
        click(&mut app, &ctx, favorites);
        assert_eq!(app.scope, Scope::Favorites);
        let row = point(&mut app, "Рабочая запись");
        click(&mut app, &ctx, row);
        let remove = point(&mut app, "Убрать из избранного");
        click(&mut app, &ctx, remove);
        assert!(app.catalog_view.rows.is_empty());
        assert!(app.selected.is_none());
        assert!(!app.entries[0].favorite);
        Ok(())
    }
}
