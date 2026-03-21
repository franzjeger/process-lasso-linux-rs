//! Log tab: scrolling log with auto-scroll and clear button.

use egui::Ui;

pub struct LogTab {
    pub auto_scroll: bool,
}

impl LogTab {
    pub fn new() -> Self {
        Self { auto_scroll: true }
    }

    /// Show with a clear button. Returns (clear_requested, save_requested).
    pub fn show_with_clear(&mut self, ui: &mut Ui, lines: &std::collections::VecDeque<String>) -> (bool, bool) {
        let mut clear = false;
        let mut save = false;
        ui.horizontal(|ui| {
            if ui.button("Clear").clicked() { clear = true; }
            if ui.button("Save to File…").clicked() { save = true; }
            ui.checkbox(&mut self.auto_scroll, "Auto-scroll");
        });
        ui.separator();

        egui::ScrollArea::vertical()
            .auto_shrink([false; 2])
            .stick_to_bottom(self.auto_scroll)
            .show(ui, |ui| {
                for line in lines {
                    let color = if line.contains("[Gaming Mode]") || line.contains("[Launcher]") || line.contains("[Profile]") || line.contains("[Reset]") {
                        egui::Color32::from_rgb(100, 200, 140)
                    } else if line.contains("[Rule:") || line.contains("[Default]") {
                        egui::Color32::from_rgb(100, 170, 240)
                    } else if line.contains("[Manual]") {
                        egui::Color32::from_rgb(180, 140, 240)
                    } else if line.contains("FAILED") || line.contains("failed") || line.contains("error") {
                        egui::Color32::from_rgb(240, 100, 90)
                    } else if line.contains("[HW Alert]") {
                        egui::Color32::from_rgb(240, 180, 60)
                    } else {
                        ui.visuals().text_color()
                    };
                    ui.label(egui::RichText::new(line).font(egui::FontId::monospace(11.0)).color(color));
                }
            });

        (clear, save)
    }
}
