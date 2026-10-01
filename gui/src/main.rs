//! APFSReader: mount macOS (HFS+ / APFS) disk images and disks, read-only, as
//! Windows drives. Minimising moves the window to the system tray.
#![windows_subsystem = "windows"]

mod app;
mod backend;
mod lang;
mod selftest;
mod single;
mod theme;
mod tray;
mod widgets;

use app::{App, Options, Shared};
use backend::SystemBackend;
use eframe::egui;
use std::path::PathBuf;
use std::sync::Arc;

/// Command line: image files to add, `--mount` to mount them at once,
/// `--minimized` to start in the tray.
fn parse_args() -> (Options, bool, bool) {
    let mut opts = Options::default();
    let mut minimized = false;
    let mut selftest = false;
    for a in std::env::args().skip(1) {
        match a.as_str() {
            "--mount" => opts.auto_mount = true,
            "--about" => opts.show_about = true,
            "--minimized" => minimized = true,
            "--selftest" => selftest = true,
            other => opts.images.push(PathBuf::from(other)),
        }
    }
    (opts, minimized, selftest)
}

struct Gui {
    app: App,
    tray: Option<tray::Tray>,
}

impl eframe::App for Gui {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.app.logic(ctx);
        if let (Some(n), Some(t)) = (self.app.mount_count_changed(), &self.tray) {
            t.set_mounted(n);
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        let p = theme::Palette::of(&ctx);
        // The strip appears only while there is something to say.
        if self.app.has_status() {
            egui::Panel::bottom("footer")
                .frame(
                    egui::Frame::new()
                        .fill(p.surface)
                        .inner_margin(egui::Margin::symmetric(24, 6))
                        .stroke(egui::Stroke::new(1.0, p.border)),
                )
                .show(ui, |ui| self.app.footer(ui));
        }
        egui::CentralPanel::default().frame(egui::Frame::new().fill(p.bg)).show(ui, |ui| self.app.show(ui));
        self.app.quit_dialog(&ctx);
    }
}

fn main() -> eframe::Result {
    if !single::acquire() {
        single::activate_existing();
        return Ok(());
    }
    let (opts, minimized, selftest) = parse_args();
    let expect_mount = opts.auto_mount && !opts.images.is_empty();

    let viewport = egui::ViewportBuilder::default()
        .with_title(single::WINDOW_TITLE)
        .with_inner_size([900.0, 680.0])
        .with_min_inner_size([640.0, 420.0])
        .with_drag_and_drop(true)
        .with_icon(egui::IconData { rgba: apfsreader_icon::rgba(64), width: 64, height: 64 });
    let options = eframe::NativeOptions { viewport, ..Default::default() };

    eframe::run_native(
        single::WINDOW_TITLE,
        options,
        Box::new(move |cc| {
            theme::install(&cc.egui_ctx);
            let shared = Arc::new(Shared::default());
            shared.quiet.store(selftest, std::sync::atomic::Ordering::Relaxed);
            let tray = tray::create(&cc.egui_ctx, shared.clone());
            if selftest {
                selftest::spawn(cc.egui_ctx.clone(), shared.clone(), expect_mount);
            }
            let app = App::new(cc.egui_ctx.clone(), Arc::new(SystemBackend), shared, opts);
            if minimized {
                app.shared.hidden.store(true, std::sync::atomic::Ordering::Relaxed);
                cc.egui_ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
            }
            Ok(Box::new(Gui { app, tray }))
        }),
    )
}
