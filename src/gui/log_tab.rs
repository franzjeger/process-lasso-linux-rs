//! Log tab: scrolling log with auto-scroll and clear button.

use egui::Ui;

pub struct LogTab {
    pub auto_scroll: bool,
}

impl LogTab {
    pub fn new() -> Self {
        Self { auto_scroll: true }
    }

    /// Show with a clear button that returns true if clear was requested.
    pub fn show_with_clear(&mut self, ui: &mut Ui, lines: &std::collections::VecDeque<String>) -> bool {
        let mut clear = false;
        ui.horizontal(|ui| {
            if ui.button("Clear").clicked() { clear = true; }
            ui.checkbox(&mut self.auto_scroll, "Auto-scroll");
        });
        ui.separator();

        egui::ScrollArea::vertical()
            .auto_shrink([false; 2])
            .stick_to_bottom(self.auto_scroll)
            .show(ui, |ui| {
                for line in lines {
                    ui.label(egui::RichText::new(line).font(egui::FontId::monospace(11.0)));
                }
            });

        clear
    }
}
