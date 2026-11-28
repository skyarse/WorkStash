use eframe::egui::{self, Align2, Rect, Response};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Panel {
    Add,
    Editor,
    Server,
    Settings,
    Section,
    RenameSection,
    Move,
}

#[derive(Clone, Copy)]
pub struct OpenPanel {
    pub kind: Panel,
    anchor_id: egui::Id,
    anchor: Rect,
    bounds: Rect,
}
impl OpenPanel {
    pub fn at(kind: Panel, trigger: &Response) -> Self {
        Self {
            kind,
            anchor_id: trigger.id,
            anchor: trigger.rect,
            bounds: Rect::NOTHING,
        }
    }
    pub fn anchored_at(&self, r: &Response) -> bool {
        self.anchor_id == r.id
    }
    pub fn track(&mut self, r: &Response) {
        if self.anchored_at(r) {
            self.anchor = r.rect;
        }
    }
    pub fn should_close(&self, ctx: &egui::Context) -> bool {
        ctx.input(|i| {
            i.key_pressed(egui::Key::Escape)
                || (i.pointer.any_pressed()
                    && i.pointer
                        .interact_pos()
                        .is_some_and(|p| !self.bounds.contains(p) && !self.anchor.contains(p)))
        })
    }
    pub fn draw(&self, ctx: &egui::Context, body: impl FnOnce(&mut egui::Ui)) -> Rect {
        let screen = ctx.screen_rect();
        let width = (match self.kind {
            Panel::Editor => 420.,
            Panel::Server => 340.,
            _ => 205.0_f32,
        })
        .min(screen.width() - 28.);
        let above = self.anchor.center().y > screen.center().y;
        let right = self.anchor.left() + width + 16. > screen.right();
        let x = if right {
            self.anchor.right()
        } else {
            self.anchor.left()
        };
        let y = if above {
            self.anchor.top() - 4.
        } else {
            self.anchor.bottom() + 4.
        };
        let pivot = match (above, right) {
            (true, true) => Align2::RIGHT_BOTTOM,
            (true, false) => Align2::LEFT_BOTTOM,
            (false, true) => Align2::RIGHT_TOP,
            (false, false) => Align2::LEFT_TOP,
        };
        egui::Area::new(egui::Id::new(("workstash-panel", self.kind as u8)))
            .order(egui::Order::Foreground)
            .movable(false)
            .pivot(pivot)
            .fixed_pos(egui::pos2(x, y))
            .constrain_to(screen)
            .show(ctx, |ui| {
                egui::Frame::popup(ui.style()).show(ui, |ui| {
                    ui.set_width(width);
                    egui::ScrollArea::vertical()
                        .max_height(
                            (if above {
                                y - screen.top()
                            } else {
                                screen.bottom() - y
                            } - 22.)
                                .max(100.),
                        )
                        .show(ui, body);
                })
            })
            .response
            .rect
    }
    pub fn record_bounds(&mut self, bounds: Rect) {
        self.bounds = bounds;
    }
}
