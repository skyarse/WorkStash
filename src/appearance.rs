use anyhow::Result;
use eframe::egui::{self, ThemePreference};
use std::{fs, path::Path, sync::mpsc};

const CHOICES: [(ThemePreference, &str, &str); 3] = [
    (ThemePreference::System, "Системная", "system"),
    (ThemePreference::Light, "Светлая", "light"),
    (ThemePreference::Dark, "Тёмная", "dark"),
];

pub struct Appearance {
    preference: ThemePreference,
    system: ThemePreference,
    changes: mpsc::Receiver<ThemePreference>,
    _subscription: mundy::Subscription,
}

impl Appearance {
    pub fn new(ctx: &egui::Context) -> Self {
        let (send, changes) = mpsc::channel();
        let ctx = ctx.clone();
        let subscription = mundy::Preferences::subscribe(mundy::Interest::ColorScheme, move |p| {
            let theme = match p.color_scheme {
                mundy::ColorScheme::Dark => ThemePreference::Dark,
                mundy::ColorScheme::Light => ThemePreference::Light,
                _ => ThemePreference::System,
            };
            let _ = send.send(theme);
            ctx.request_repaint();
        });
        let preference = super::config_file()
            .map(|f| load(&f.with_file_name("theme")))
            .unwrap_or(ThemePreference::System);
        Self {
            preference,
            system: ThemePreference::System,
            changes,
            _subscription: subscription,
        }
    }

    pub fn apply(&mut self, ctx: &egui::Context) {
        for value in self.changes.try_iter() {
            self.system = value;
        }
        ctx.set_theme(if self.preference == ThemePreference::System {
            self.system
        } else {
            self.preference
        });
    }

    pub fn settings(&mut self, ui: &mut egui::Ui) -> Result<()> {
        let mut chosen = self.preference;
        ui.label("Тема оформления");
        for (value, label, _) in CHOICES {
            ui.radio_value(&mut chosen, value, label);
        }
        if chosen != self.preference {
            self.preference = chosen;
            self.apply(ui.ctx());
            let path = super::config_file()
                .ok_or_else(|| anyhow::anyhow!("Не найдена папка настроек"))?
                .with_file_name("theme");
            save(&path, chosen)?;
        }
        Ok(())
    }
}

fn load(path: &Path) -> ThemePreference {
    let text = fs::read_to_string(path).unwrap_or_default();
    CHOICES
        .iter()
        .find(|(_, _, key)| *key == text.trim())
        .map(|c| c.0)
        .unwrap_or(ThemePreference::System)
}
fn save(path: &Path, preference: ThemePreference) -> Result<()> {
    use std::io::Write;
    let dir = path.parent().unwrap();
    fs::create_dir_all(dir)?;
    let mut file = tempfile::NamedTempFile::new_in(dir)?;
    let key = CHOICES.iter().find(|c| c.0 == preference).unwrap().2;
    file.write_all(key.as_bytes())?;
    file.persist(path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn old_config_and_theme_roundtrip() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("theme");
        assert_eq!(load(&path), ThemePreference::System);
        for (theme, _, _) in CHOICES {
            save(&path, theme)?;
            assert_eq!(load(&path), theme);
        }
        Ok(())
    }
}
