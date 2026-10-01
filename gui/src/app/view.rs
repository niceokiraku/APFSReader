//! Drawing the window. Pure presentation: every action goes through `App`.

use super::*;
use crate::theme::{bold_family, Palette};
use crate::widgets::{self, bold, glyph, muted, Kind as Btn, Tone};
use apfsreader_core::image::Kind;
use eframe::egui::{Align, Layout, Margin, Stroke, Ui};

/// Widest the content column grows, so lines stay readable on a big window.
const COLUMN: f32 = 880.0;

const WINFSP_DOWNLOAD: &str = "https://winfsp.dev/rel/";
/// The credit WinFsp's licence requires in the user interface and documentation.
pub const WINFSP_CREDIT: &str = "WinFsp - Windows File System Proxy, Copyright (C) Bill Zissimopoulos";
pub const WINFSP_URL: &str = "https://github.com/winfsp/winfsp";
const COPYRIGHT: &str = "Copyright (c) 2026 niceokiraku";

/// "0.9.0" and whether it is a pre-1.0 (beta) release.
fn version_line() -> String {
    let v = env!("CARGO_PKG_VERSION");
    let beta = v.starts_with("0.");
    match (crate::lang::japanese(), beta) {
        (true, true) => format!("バージョン {v}(ベータ)"),
        (true, false) => format!("バージョン {v}"),
        (false, true) => format!("Version {v} (beta)"),
        (false, false) => format!("Version {v}"),
    }
}

/// Plain-text content of the About box, for tests.
#[cfg(test)]
pub fn about_text() -> String {
    [
        version_line(),
        COPYRIGHT.to_string(),
        WINFSP_CREDIT.to_string(),
        WINFSP_URL.to_string(),
    ]
    .join("\n")
}

fn role_text(role: Option<&'static str>) -> Option<String> {
    let name = match role? {
        "none" => return None,
        "data" => t("データ", "Data"),
        "system" => t("システム", "System"),
        "preboot" => "Preboot",
        "recovery" => t("リカバリ", "Recovery"),
        "vm" => "VM",
        other => other,
    };
    Some(name.to_string())
}

fn kind_badge(ui: &mut Ui, p: &Palette, kind: Kind) {
    match kind {
        Kind::Apfs => widgets::badge(ui, "APFS", p.apfs_fg, p.apfs_bg),
        Kind::HfsPlus => widgets::badge(ui, "HFS+", p.hfs_fg, p.hfs_bg),
    }
}

fn hairline(ui: &mut Ui, p: &Palette) {
    ui.add_space(6.0);
    let rect = ui.available_rect_before_wrap();
    ui.painter().hline(rect.x_range(), ui.cursor().top(), Stroke::new(1.0, p.border));
    ui.add_space(8.0);
}

fn disk_subtitle(d: &DiskInfo) -> String {
    let mut s = format!("{:.1} GB  ·  {}", d.size as f64 / 1e9, d.bus);
    if d.removable {
        s.push_str(t("  ·  取り外し可能", "  ·  removable"));
    }
    s
}

fn open_in_explorer(path: &str) {
    let _ = std::process::Command::new("explorer.exe").arg(format!("{path}\\")).spawn();
}

impl App {
    fn pick_images(&mut self) {
        let pick = rfd::FileDialog::new()
            .set_title(t("ディスクイメージを選択", "Choose a disk image"))
            .add_filter(t("ディスクイメージ", "Disk images"), &["dmg", "img", "iso", "raw", "bin", "sparseimage"])
            .add_filter(t("すべてのファイル", "All files"), &["*"])
            .pick_files();
        for p in pick.unwrap_or_default() {
            self.add_image(p);
        }
    }

    fn header(&mut self, ui: &mut Ui, p: &Palette) {
        let frame = egui::Frame::new().fill(p.surface).inner_margin(Margin::symmetric(24, 14)).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.add(egui::Image::new(&self.icon).fit_to_exact_size(egui::vec2(40.0, 40.0)));
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 1.0;
                    ui.label(bold("APFSReader", 20.0).color(p.text));
                    ui.label(muted(p, t("Mac のディスクを読み取り専用でマウント", "Mount Mac disks, read-only")));
                });
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if widgets::button(ui, p, Btn::Ghost, None, t("終了", "Quit")).clicked() {
                        self.request_quit(&self.ctx.clone());
                    }
                    if widgets::button(ui, p, Btn::Ghost, Some(glyph::INFO), t("情報", "About")).clicked() {
                        self.show_about = true;
                    }
                    if widgets::button(ui, p, Btn::Secondary, Some(glyph::REFRESH), t("更新", "Refresh"))
                        .on_hover_text(t("ディスクの一覧を更新します", "Refresh the list of disks"))
                        .clicked()
                    {
                        self.refresh_disks();
                    }
                    if widgets::button(ui, p, Btn::Primary, Some(glyph::ADD), t("イメージを追加", "Add image")).clicked() {
                        self.pick_images();
                    }
                    if self.elevated {
                        widgets::badge(ui, t("管理者として実行中", "Running as administrator"), p.warn, p.warn_bg);
                    } else {
                        widgets::badge(ui, t("標準ユーザー", "Standard user"), p.muted, p.surface_alt);
                    }
                });
            });
        });
        let r = frame.response.rect;
        ui.painter().hline(r.x_range(), r.bottom(), Stroke::new(1.0, p.border));
    }

    /// Shown while WinFsp is missing, since nothing can be mounted without it.
    fn winfsp_banner(&mut self, ui: &mut Ui, p: &Palette) {
        if self.winfsp_ok {
            return;
        }
        widgets::card(ui, p, Some(p.danger), |ui| {
            ui.label(bold(t("WinFsp が必要です", "WinFsp is required"), 16.0).color(p.text));
            ui.add_space(2.0);
            ui.add(
                egui::Label::new(muted(
                    p,
                    t(
                        "ボリュームをドライブとして表示するために、無償のファイルシステムドライバー WinFsp が必要です。インストールしてから「再確認」を押してください。",
                        "Showing volumes as drives needs WinFsp, a free file system driver. Install it, then press Check again.",
                    ),
                ))
                .wrap(),
            );
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if widgets::button(ui, p, Btn::Primary, None, t("WinFsp のダウンロードページを開く", "Open the WinFsp download page")).clicked() {
                    ui.ctx().open_url(egui::OpenUrl::new_tab(WINFSP_DOWNLOAD));
                }
                if widgets::button(ui, p, Btn::Secondary, Some(glyph::REFRESH), t("再確認", "Check again")).clicked() {
                    self.recheck_winfsp();
                }
            });
        });
        ui.add_space(8.0);
    }

    fn mounted_section(&mut self, ui: &mut Ui, p: &Palette) {
        let rows: Vec<(u64, String, String, Kind, Vec<String>)> = self
            .shared
            .mounts
            .lock()
            .unwrap()
            .iter()
            .map(|m| {
                let i = m.handle.info();
                (m.id, i.mount_point.clone(), i.label.clone(), i.kind, i.notes.clone())
            })
            .collect();
        if rows.is_empty() {
            return;
        }
        widgets::section_header(ui, p, t("マウント中", "Mounted"), Some(rows.len()), |_| {});
        let mut release = None;
        for (id, mp, label, kind, notes) in rows {
            widgets::card(ui, p, Some(p.success), |ui| {
                ui.horizontal(|ui| {
                    widgets::drive_icon(ui, p, widgets::IconKind::Mounted, 46.0);
                    ui.vertical(|ui| {
                        ui.spacing_mut().item_spacing.y = 3.0;
                        ui.horizontal(|ui| {
                            ui.label(egui::RichText::new(&mp).font(egui::FontId::new(21.0, egui::FontFamily::Monospace)).color(p.text));
                            kind_badge(ui, p, kind);
                        });
                        ui.label(muted(p, label));
                    });
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if widgets::button(ui, p, Btn::Danger, None, t("マウント解除", "Unmount")).clicked() {
                            release = Some(id);
                        }
                        if widgets::button(ui, p, Btn::Secondary, Some(glyph::FOLDER_OPEN), t("フォルダーを開く", "Open folder"))
                            .clicked()
                        {
                            open_in_explorer(&mp);
                        }
                    });
                });
                for n in notes {
                    ui.add_space(8.0);
                    widgets::alert(ui, p, Tone::Warn, &n);
                }
            });
            ui.add_space(8.0);
        }
        if let Some(id) = release {
            self.unmount(id);
        }
    }

    fn letter_picker(&mut self, ui: &mut Ui, p: &Palette, index: usize, row: usize, card_id: u64) {
        let free = self.free_letters.clone();
        let Some(r) = self.cards[index].rows.get_mut(row) else { return };
        let current = r.letter.clone().unwrap_or_else(|| t("自動", "Auto").to_string());
        egui::ComboBox::from_id_salt(("letter", card_id, row)).width(84.0).selected_text(current).show_ui(ui, |ui| {
            ui.selectable_value(&mut r.letter, None, t("自動", "Auto"));
            for l in free {
                ui.selectable_value(&mut r.letter, Some(l.clone()), l);
            }
        });
        ui.label(muted(p, t("ドライブ", "Drive")));
    }

    fn volume_row(&mut self, ui: &mut Ui, p: &Palette, index: usize, row: usize, preferred: bool, multi: bool) {
        let card_id = self.cards[index].id;
        let ScanState::Done { found, .. } = &self.cards[index].scan else { return };
        let f = found[row].clone();
        let mounted = self.mount_of(card_id, row);
        let rowui = self.cards[index].rows.get(row).cloned().unwrap_or_default();
        let mut mount = false;
        let mut release = None;

        ui.horizontal(|ui| {
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = 4.0;
                ui.horizontal(|ui| {
                    ui.label(bold(&f.name, 15.5).color(p.text));
                    kind_badge(ui, p, f.kind);
                    if let Some(r) = role_text(f.role) {
                        widgets::badge(ui, &r, p.muted, p.surface_alt);
                    }
                    if f.encrypted {
                        widgets::badge(ui, t("暗号化", "Encrypted"), p.danger, p.danger_bg);
                    }
                    if preferred && multi {
                        widgets::badge(ui, t("おすすめ", "Recommended"), p.accent, p.surface_alt);
                    }
                });
            });
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if let Some((id, mp, _)) = &mounted {
                    if widgets::button(ui, p, Btn::Danger, None, t("マウント解除", "Unmount")).clicked() {
                        release = Some(*id);
                    }
                    if widgets::button(ui, p, Btn::Secondary, Some(glyph::FOLDER_OPEN), t("開く", "Open")).clicked() {
                        open_in_explorer(mp);
                    }
                    ui.label(egui::RichText::new(mp).font(egui::FontId::new(16.0, egui::FontFamily::Monospace)).color(p.success));
                } else if f.problem.is_some() || f.encrypted {
                    // Nothing can be done with it; the reason is shown below.
                } else if rowui.busy {
                    ui.label(muted(p, t("マウントしています…", "Mounting…")));
                    ui.spinner();
                } else {
                    if widgets::button(ui, p, Btn::Primary, None, t("マウント", "Mount")).clicked() {
                        mount = true;
                    }
                    self.letter_picker(ui, p, index, row, card_id);
                }
            });
        });
        if let Some(problem) = &f.problem {
            ui.add_space(6.0);
            widgets::alert(ui, p, Tone::Danger, problem);
        } else if f.encrypted {
            ui.add_space(6.0);
            widgets::alert(
                ui,
                p,
                Tone::Danger,
                t("このボリュームは暗号化されているためマウントできません。", "This volume is encrypted, so it cannot be mounted."),
            );
        }
        if let Some(e) = &rowui.error {
            ui.add_space(6.0);
            widgets::alert(ui, p, Tone::Danger, e);
        }
        if mount {
            self.start_mount(card_id, row);
        }
        if let Some(id) = release {
            self.unmount(id);
        }
    }

    fn source_card(&mut self, ui: &mut Ui, p: &Palette, index: usize) {
        let card_id = self.cards[index].id;
        let (icon, title, subtitle, is_disk) = match &self.cards[index].origin {
            Origin::Image(path) => (
                widgets::IconKind::Image,
                path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
                path.display().to_string(),
                false,
            ),
            Origin::Disk(d) => (
                if d.removable { widgets::IconKind::Usb } else { widgets::IconKind::Disk },
                format!("{}  (Disk {})", d.display_name(), d.number),
                disk_subtitle(d),
                true,
            ),
        };
        let mut scan = false;
        let mut remove = false;

        widgets::card(ui, p, None, |ui| {
            ui.horizontal(|ui| {
                widgets::drive_icon(ui, p, icon, 44.0);
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 2.0;
                    ui.add(egui::Label::new(bold(&title, 16.0).color(p.text)).truncate());
                    let sub = if is_disk {
                        muted(p, subtitle)
                    } else {
                        egui::RichText::new(subtitle).font(egui::FontId::new(12.0, egui::FontFamily::Monospace)).color(p.faint)
                    };
                    ui.add(egui::Label::new(sub).truncate());
                });
                if !is_disk {
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if widgets::button(ui, p, Btn::Ghost, Some(glyph::REMOVE), t("一覧から外す", "Remove"))
                            .on_hover_text(t("マウント中のボリュームも解除されます", "Anything mounted from it is unmounted too"))
                            .clicked()
                        {
                            remove = true;
                        }
                    });
                }
            });

            match &self.cards[index].scan {
                ScanState::Idle => {
                    hairline(ui, p);
                    ui.horizontal(|ui| {
                        let hint = if self.elevated {
                            t("ディスクの内容を調べて、マウントできるボリュームを探します。", "Looks for volumes that can be mounted.")
                        } else {
                            t(
                                "内容を読むには管理者の承認が必要です。承認は読み取り専用の小さなヘルパーにだけ与えられます。",
                                "Reading it needs administrator approval, which goes only to a small read-only helper.",
                            )
                        };
                        ui.add(egui::Label::new(muted(p, hint)).wrap());
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if widgets::button(ui, p, Btn::Secondary, None, t("ディスクを読み取る", "Read this disk")).clicked() {
                                scan = true;
                            }
                        });
                    });
                }
                ScanState::Working => {
                    hairline(ui, p);
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label(muted(p, t("内容を調べています…", "Looking inside…")));
                    });
                }
                ScanState::Failed(msg) => {
                    hairline(ui, p);
                    let msg = msg.clone();
                    widgets::alert(ui, p, Tone::Danger, &msg);
                    ui.add_space(8.0);
                    if widgets::button(ui, p, Btn::Secondary, Some(glyph::REFRESH), t("再試行", "Try again")).clicked() {
                        scan = true;
                    }
                }
                ScanState::Done { found, .. } if found.is_empty() => {
                    hairline(ui, p);
                    ui.label(muted(
                        p,
                        t(
                            "マウントできる HFS+ / APFS ボリュームが見つかりませんでした。",
                            "No HFS+ or APFS volume that can be mounted was found.",
                        ),
                    ));
                }
                ScanState::Done { found, .. } => {
                    let preferred = image::preferred(found);
                    let count = found.len();
                    for row in 0..count {
                        hairline(ui, p);
                        self.volume_row(ui, p, index, row, preferred == Some(row), count > 1);
                    }
                }
            }
        });
        ui.add_space(8.0);

        if scan {
            self.start_scan(card_id);
        }
        if remove {
            let ids: Vec<u64> =
                self.shared.mounts.lock().unwrap().iter().filter(|m| m.source_id == card_id).map(|m| m.id).collect();
            for id in ids {
                self.unmount(id);
            }
            self.cards.retain(|c| c.id != card_id);
        }
    }

    fn body(&mut self, ui: &mut Ui, p: &Palette, dropping: bool) {
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            let column = ui.available_width().min(COLUMN) - 40.0;
            ui.vertical_centered(|ui| {
                ui.allocate_ui_with_layout(egui::vec2(column.max(320.0), 0.0), Layout::top_down(Align::Min), |ui| {
                    ui.add_space(12.0);
                    self.winfsp_banner(ui, p);
                    self.mounted_section(ui, p);

                    let images: Vec<usize> =
                        (0..self.cards.len()).filter(|&i| matches!(self.cards[i].origin, Origin::Image(_))).collect();
                    widgets::section_header(ui, p, t("ディスクイメージ", "Disk images"), Some(images.len()), |ui| {
                        if !images.is_empty() {
                            ui.label(muted(p, t("ここにドロップして追加できます", "You can also drop files here")));
                        }
                    });
                    if images.is_empty() {
                        widgets::drop_zone(
                            ui,
                            p,
                            dropping,
                            t("ディスクイメージをここにドロップ", "Drop a disk image here"),
                            t(".dmg · .img · .iso など", ".dmg · .img · .iso and similar"),
                            |ui| {
                                if widgets::button(ui, p, Btn::Primary, Some(glyph::ADD), t("イメージを選ぶ", "Choose an image")).clicked() {
                                    self.pick_images();
                                }
                            },
                        );
                        ui.add_space(8.0);
                    }
                    for i in images {
                        if i < self.cards.len() {
                            self.source_card(ui, p, i);
                        }
                    }

                    let show_system = self.show_system_disk;
                    let disks: Vec<usize> = (0..self.cards.len())
                        .filter(|&i| match &self.cards[i].origin {
                            Origin::Disk(d) => show_system || !d.is_system,
                            Origin::Image(_) => false,
                        })
                        .collect();
                    // Folded until asked for: most people only open images.
                    let mut open = self.disks_open;
                    let _ = widgets::collapsible_header(ui, p, t("物理ディスク", "Physical disks"), Some(disks.len()), &mut open);
                    self.disks_open = open;
                    if self.disks_open {
                        let mut toggle = show_system;
                        ui.horizontal(|ui| {
                            ui.add_space(2.0);
                            ui.checkbox(
                                &mut toggle,
                                egui::RichText::new(t("システムディスクも表示", "Show the system disk")).color(p.muted).size(12.5),
                            );
                        });
                        self.show_system_disk = toggle;
                        if disks.is_empty() {
                            ui.label(muted(p, t("ディスクが見つかりません。", "No disks found.")));
                        }
                        for i in disks {
                            if i < self.cards.len() {
                                self.source_card(ui, p, i);
                            }
                        }
                    }
                    ui.add_space(16.0);
                });
            });
        });
    }

    /// A veil over the window while files are dragged onto it.
    fn drop_overlay(&self, ctx: &egui::Context, p: &Palette) {
        let rect = ctx.content_rect();
        let painter = ctx.layer_painter(egui::LayerId::new(egui::Order::Foreground, egui::Id::new("drop-overlay")));
        painter.rect_filled(
            rect,
            egui::CornerRadius::ZERO,
            egui::Color32::from_rgba_unmultiplied(p.accent.r(), p.accent.g(), p.accent.b(), 46),
        );
        painter.rect_stroke(rect.shrink(8.0), egui::CornerRadius::same(14), Stroke::new(2.5, p.accent), egui::StrokeKind::Inside);
        painter.text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            t("ここにドロップして追加", "Drop to add"),
            egui::FontId::new(26.0, bold_family()),
            p.accent,
        );
    }

    /// Draw the whole window contents.
    pub fn show(&mut self, ui: &mut Ui) {
        let ctx = ui.ctx().clone();
        let p = Palette::of(&ctx);
        let dropping = ctx.input(|i| !i.raw.hovered_files.is_empty());
        self.header(ui, &p);
        self.body(ui, &p, dropping);
        if dropping {
            self.drop_overlay(&ctx, &p);
        }
        self.about_dialog(&ctx);
    }

    /// Whether there is a recent message to show in the bottom strip.
    pub fn has_status(&self) -> bool {
        self.status.is_some()
    }

    /// The bottom strip: the latest message (for example where a volume was mounted).
    pub fn footer(&mut self, ui: &mut Ui) {
        let p = Palette::of(ui.ctx());
        if let Some((m, _)) = &self.status {
            ui.add_space(2.0);
            ui.label(egui::RichText::new(m).color(p.success).size(13.0));
            ui.add_space(2.0);
        }
    }

    /// The About box: version, copyright, licences and the required WinFsp credit.
    pub fn about_dialog(&mut self, ctx: &egui::Context) {
        if !self.show_about {
            return;
        }
        let p = Palette::of(ctx);
        let mut open = true;
        let mut close = false;
        egui::Window::new(bold(t("APFSReader について", "About APFSReader"), 17.0))
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.set_width(430.0);
                ui.horizontal(|ui| {
                    ui.add(egui::Image::new(&self.icon).fit_to_exact_size(egui::vec2(64.0, 64.0)));
                    ui.vertical(|ui| {
                        ui.label(bold("APFSReader", 22.0).color(p.text));
                        ui.label(muted(&p, version_line()));
                        ui.label(muted(&p, COPYRIGHT));
                    });
                });
                ui.add_space(8.0);
                ui.add(
                    egui::Label::new(egui::RichText::new(t(
                        "Mac の HFS+ / APFS ボリュームを、Windows から読み取り専用でマウントします。ディスクには一切書き込みません。",
                        "Mounts Mac HFS+ / APFS volumes on Windows, read-only. Nothing is ever written to the disk.",
                    )).color(p.text))
                    .wrap(),
                );
                ui.add_space(6.0);
                widgets::alert(
                    ui,
                    &p,
                    Tone::Warn,
                    t(
                        "ベータ版です。物理ディスクの読み取りは、実機での検証が済んでいません。大切なデータは、必ずコピーを残してください。",
                        "Beta software. Reading physical disks has not yet been verified on real hardware. Keep a copy of anything important.",
                    ),
                );
                ui.add_space(10.0);
                ui.label(bold(t("ライセンス", "Licences"), 14.0).color(p.text));
                ui.add(
                    egui::Label::new(muted(
                        &p,
                        t(
                            "ライブラリとコマンドラインツールは MIT、このアプリとマウント部は GPL-3.0 です。ソースコードは各リリースに同梱されています。詳細はインストール先フォルダの LICENSE.md と THIRD_PARTY_LICENSES.md を参照してください。",
                            "The library and command line tool are MIT; this app and the mount component are GPL-3.0. Source code is published with every release. See LICENSE.md and THIRD_PARTY_LICENSES.md in the install folder.",
                        ),
                    ))
                    .wrap(),
                );
                ui.add_space(10.0);
                ui.label(bold("WinFsp", 14.0).color(p.text));
                ui.label(egui::RichText::new(WINFSP_CREDIT).size(13.0).color(p.text));
                ui.hyperlink_to(WINFSP_URL, WINFSP_URL);
                ui.add_space(12.0);
                ui.horizontal(|ui| {
                    if widgets::button(ui, &p, Btn::Secondary, Some(glyph::FOLDER_OPEN), t("インストール先を開く", "Open install folder")).clicked() {
                        if let Some(dir) = std::env::current_exe().ok().and_then(|e| e.parent().map(|d| d.to_path_buf())) {
                            let _ = std::process::Command::new("explorer.exe").arg(dir).spawn();
                        }
                    }
                    if widgets::button(ui, &p, Btn::Primary, None, t("閉じる", "Close")).clicked() {
                        close = true;
                    }
                });
            });
        if !open || close {
            self.show_about = false;
        }
    }

    /// The confirmation shown when quitting with volumes mounted.
    pub fn quit_dialog(&mut self, ctx: &egui::Context) {
        if !self.confirm_quit {
            return;
        }
        let p = Palette::of(ctx);
        let n = self.shared.count();
        let mut decision = None;
        egui::Window::new(bold(t("終了しますか?", "Quit?"), 17.0))
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.set_max_width(380.0);
                ui.label(
                    egui::RichText::new(if crate::lang::japanese() {
                        format!("{n} 個のボリュームがマウント中です。終了すると、すべてのマウントが解除されます。")
                    } else {
                        format!("{n} volume(s) are mounted. Quitting unmounts them all.")
                    })
                    .color(p.text),
                );
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    if widgets::button(ui, &p, Btn::Primary, None, t("解除して終了", "Unmount and quit")).clicked() {
                        decision = Some(true);
                    }
                    if widgets::button(ui, &p, Btn::Secondary, None, t("キャンセル", "Cancel")).clicked() {
                        decision = Some(false);
                    }
                });
            });
        match decision {
            Some(true) => {
                self.confirm_quit = false;
                self.quit(ctx);
            }
            Some(false) => self.confirm_quit = false,
            None => {}
        }
    }

    #[cfg(test)]
    pub fn card_count(&self) -> usize {
        self.cards.len()
    }
}
