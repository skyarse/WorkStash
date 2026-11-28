use anyhow::Result;
use eframe::egui::{self, Color32, FontId, Stroke, ThemePreference};
use std::{fs, path::Path, sync::mpsc};

const CHOICES: [(ThemePreference, &str, &str); 3] = [
    (ThemePreference::System, "Системная", "system"),
    (ThemePreference::Light, "Светлая", "light"),
    (ThemePreference::Dark, "Тёмная", "dark"),
];

pub fn configure(ctx: &egui::Context) {
    ctx.all_styles_mut(|style| {
        style.spacing.item_spacing = egui::vec2(8., 7.);
        style.spacing.button_padding = egui::vec2(10., 5.);
        style.spacing.interact_size.y = 29.;
        style
            .text_styles
            .insert(egui::TextStyle::Heading, FontId::proportional(22.));
        style
            .text_styles
            .insert(egui::TextStyle::Body, FontId::proportional(14.));
        style
            .text_styles
            .insert(egui::TextStyle::Button, FontId::proportional(13.));
        style
            .text_styles
            .insert(egui::TextStyle::Small, FontId::proportional(12.));

        let colors = &mut style.visuals;
        let (main, panel, raised, hover, border, text, accent) = if colors.dark_mode {
            (
                [19, 24, 32],
                [24, 31, 40],
                [31, 40, 51],
                [42, 55, 68],
                [51, 64, 78],
                [225, 233, 241],
                [41, 83, 86],
            )
        } else {
            (
                [251, 252, 253],
                [240, 244, 247],
                [245, 248, 250],
                [225, 237, 240],
                [213, 223, 230],
                [30, 44, 58],
                [199, 233, 227],
            )
        };
        let rgb = |[r, g, b]: [u8; 3]| Color32::from_rgb(r, g, b);
        colors.panel_fill = rgb(main);
        colors.window_fill = rgb(panel);
        colors.window_stroke = Stroke::new(1.0_f32, rgb(border));
        colors.extreme_bg_color = rgb(main);
        colors.faint_bg_color = rgb(raised);
        colors.selection.bg_fill = rgb(accent);
        colors.selection.stroke = Stroke::new(
            1.0_f32,
            if colors.dark_mode {
                Color32::from_rgb(104, 168, 164)
            } else {
                Color32::from_rgb(82, 133, 130)
            },
        );
        for widget in [
            &mut colors.widgets.noninteractive,
            &mut colors.widgets.inactive,
            &mut colors.widgets.hovered,
            &mut colors.widgets.active,
            &mut colors.widgets.open,
        ] {
            widget.corner_radius = 7.into();
            widget.fg_stroke.color = rgb(text);
            widget.expansion = 0.;
        }
        colors.widgets.noninteractive.bg_stroke.color = rgb(border);
        colors.widgets.inactive.bg_fill = rgb(raised);
        colors.widgets.inactive.weak_bg_fill = rgb(raised);
        colors.widgets.inactive.bg_stroke = Stroke::NONE;
        colors.widgets.hovered.bg_fill = rgb(hover);
        colors.widgets.hovered.weak_bg_fill = rgb(hover);
        colors.widgets.hovered.bg_stroke = Stroke::new(1.0_f32, rgb(border));
        colors.widgets.active.bg_fill = rgb(accent);
        colors.widgets.active.weak_bg_fill = rgb(accent);
    });
}

pub fn primary_button(ui: &egui::Ui, label: &'static str) -> egui::Button<'static> {
    egui::Button::new(egui::RichText::new(label).strong())
        .corner_radius(7)
        .fill(ui.visuals().selection.bg_fill)
}

pub fn secondary_text(ui: &egui::Ui) -> Color32 {
    if ui.visuals().dark_mode {
        Color32::from_rgb(169, 185, 200)
    } else {
        Color32::from_rgb(82, 100, 117)
    }
}

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
