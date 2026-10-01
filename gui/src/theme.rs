//! Fonts, colours and spacing (light theme only).
//!
//! Text uses the fonts Windows ships: Segoe UI for Latin text and Yu Gothic for
//! Japanese (egui's built-in fonts have no Japanese glyphs at all), Cascadia
//! Mono for paths and drive letters. If a file is missing the next candidate or
//! egui's own font is used, so the window never fails to start over a font.

use eframe::egui::{
    self, Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, Stroke, TextStyle, Theme,
    ThemePreference, Visuals,
};
use std::sync::Arc;

const BOLD: &str = "bold";
const ICONS: &str = "icons";

static ICONS_AVAILABLE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Whether Windows' icon font was found; if not, glyphs would show as boxes.
pub fn icons_available() -> bool {
    ICONS_AVAILABLE.load(std::sync::atomic::Ordering::Relaxed)
}

pub fn bold_family() -> FontFamily {
    FontFamily::Name(BOLD.into())
}

pub fn icon_family() -> FontFamily {
    FontFamily::Name(ICONS.into())
}

/// Every colour the window uses, for one theme.
#[derive(Clone, Copy)]
pub struct Palette {
    pub bg: Color32,
    pub surface: Color32,
    pub surface_alt: Color32,
    pub border: Color32,
    pub text: Color32,
    pub muted: Color32,
    pub faint: Color32,
    pub accent: Color32,
    pub accent_hover: Color32,
    pub on_accent: Color32,
    pub success: Color32,
    pub warn: Color32,
    pub warn_bg: Color32,
    pub danger: Color32,
    pub danger_bg: Color32,
    pub apfs_fg: Color32,
    pub apfs_bg: Color32,
    pub hfs_fg: Color32,
    pub hfs_bg: Color32,
}

const fn rgb(r: u8, g: u8, b: u8) -> Color32 {
    Color32::from_rgb(r, g, b)
}

impl Palette {
    pub fn light() -> Self {
        Palette {
            bg: rgb(0xF2, 0xF4, 0xF8),
            surface: rgb(0xFF, 0xFF, 0xFF),
            surface_alt: rgb(0xEE, 0xF1, 0xF6),
            border: rgb(0xDC, 0xE2, 0xEB),
            text: rgb(0x16, 0x1F, 0x30),
            muted: rgb(0x58, 0x64, 0x77),
            faint: rgb(0x92, 0x9C, 0xAB),
            accent: rgb(0x2B, 0x67, 0xF5),
            accent_hover: rgb(0x1F, 0x56, 0xDB),
            on_accent: rgb(0xFF, 0xFF, 0xFF),
            success: rgb(0x0B, 0x93, 0x66),
            warn: rgb(0xA8, 0x6A, 0x0A),
            warn_bg: rgb(0xFF, 0xF1, 0xD6),
            danger: rgb(0xCF, 0x32, 0x45),
            danger_bg: rgb(0xFD, 0xE6, 0xE9),
            apfs_fg: rgb(0x3B, 0x4C, 0xD0),
            apfs_bg: rgb(0xE5, 0xE9, 0xFF),
            hfs_fg: rgb(0x0A, 0x7A, 0x70),
            hfs_bg: rgb(0xD9, 0xF4, 0xF0),
        }
    }

    /// The window always uses the light theme.
    pub fn of(_ctx: &egui::Context) -> Palette {
        Palette::light()
    }
}

fn font(path: &str) -> Option<FontData> {
    std::fs::read(path).ok().map(FontData::from_owned)
}

fn first_font(dir: &str, names: &[&str]) -> Option<FontData> {
    names.iter().find_map(|n| font(&format!("{dir}\\{n}")))
}

fn install_fonts(ctx: &egui::Context) {
    let dir = std::env::var("WINDIR").map(|w| format!("{w}\\Fonts")).unwrap_or_else(|_| "C:\\Windows\\Fonts".into());
    let mut defs = FontDefinitions::default();
    let mut add = |name: &str, data: Option<FontData>| -> bool {
        match data {
            Some(d) => {
                defs.font_data.insert(name.into(), Arc::new(d));
                true
            }
            None => false,
        }
    };

    let latin = add("segoe", first_font(&dir, &["segoeui.ttf"]));
    let latin_bold = add("segoe-bold", first_font(&dir, &["segoeuib.ttf"]));
    let jp = add("yu", first_font(&dir, &["YuGothM.ttc", "YuGothR.ttc", "meiryo.ttc", "msgothic.ttc"]));
    let jp_bold = add("yu-bold", first_font(&dir, &["YuGothB.ttc", "meiryob.ttc", "YuGothM.ttc", "meiryo.ttc"]));
    let mono = add("mono", first_font(&dir, &["CascadiaMono.ttf", "consola.ttf"]));
    let icons = add("icons", first_font(&dir, &["SegoeIcons.ttf", "segmdl2.ttf"]));

    // Put ours in front of egui's own fonts, which stay as the last resort.
    let prepend = |defs: &mut FontDefinitions, family: FontFamily, names: &[(&str, bool)]| {
        let list = defs.families.entry(family).or_default();
        let mut front: Vec<String> = names.iter().filter(|(_, ok)| *ok).map(|(n, _)| n.to_string()).collect();
        front.extend(list.drain(..));
        *list = front;
    };
    let proportional = defs.families.get(&FontFamily::Proportional).cloned().unwrap_or_default();
    prepend(&mut defs, FontFamily::Proportional, &[("yu", jp), ("segoe", latin)]);
    prepend(&mut defs, FontFamily::Monospace, &[("mono", mono), ("yu", jp)]);
    defs.families.insert(bold_family(), {
        let mut v: Vec<String> = Vec::new();
        for (n, ok) in [("yu-bold", jp_bold), ("segoe-bold", latin_bold)] {
            if ok {
                v.push(n.into());
            }
        }
        v.extend(proportional);
        // Without a bold file fall back to the regular family.
        if !latin_bold && !jp_bold {
            v.splice(0..0, [("yu", jp), ("segoe", latin)].iter().filter(|(_, ok)| *ok).map(|(n, _)| n.to_string()));
        }
        v
    });
    ICONS_AVAILABLE.store(icons, std::sync::atomic::Ordering::Relaxed);
    // Without the icon font the family still needs a font to point at; the
    // buttons then leave their icons out (see `icons_available`).
    defs.families.insert(
        icon_family(),
        if icons { vec!["icons".into()] } else { defs.families[&FontFamily::Proportional].clone() },
    );
    ctx.set_fonts(defs);
}

fn visuals(p: &Palette) -> Visuals {
    let mut v = Visuals::light();
    v.panel_fill = p.bg;
    v.window_fill = p.surface;
    v.extreme_bg_color = p.surface_alt;
    v.faint_bg_color = p.surface_alt;
    v.window_stroke = Stroke::new(1.0, p.border);
    v.window_corner_radius = CornerRadius::same(12);
    v.menu_corner_radius = CornerRadius::same(8);
    v.hyperlink_color = p.accent;
    v.selection.bg_fill = Color32::from_rgba_unmultiplied(p.accent.r(), p.accent.g(), p.accent.b(), 70);
    v.selection.stroke = Stroke::new(1.0, p.accent);

    let radius = CornerRadius::same(8);
    let hover_fill = rgb(0xE4, 0xE9, 0xF1);
    let active_fill = rgb(0xD6, 0xDD, 0xE9);

    v.widgets.noninteractive.bg_fill = p.surface;
    v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, p.border);
    v.widgets.noninteractive.fg_stroke = Stroke::new(1.0, p.text);
    v.widgets.noninteractive.corner_radius = radius;

    v.widgets.inactive.bg_fill = p.surface_alt;
    v.widgets.inactive.weak_bg_fill = p.surface_alt;
    v.widgets.inactive.bg_stroke = Stroke::new(1.0, p.border);
    v.widgets.inactive.fg_stroke = Stroke::new(1.0, p.text);
    v.widgets.inactive.corner_radius = radius;

    v.widgets.hovered.bg_fill = hover_fill;
    v.widgets.hovered.weak_bg_fill = hover_fill;
    v.widgets.hovered.bg_stroke = Stroke::new(1.0, p.faint);
    v.widgets.hovered.fg_stroke = Stroke::new(1.0, p.text);
    v.widgets.hovered.corner_radius = radius;

    v.widgets.active.bg_fill = active_fill;
    v.widgets.active.weak_bg_fill = active_fill;
    v.widgets.active.bg_stroke = Stroke::new(1.0, p.accent);
    v.widgets.active.fg_stroke = Stroke::new(1.0, p.text);
    v.widgets.active.corner_radius = radius;

    v.widgets.open.bg_fill = hover_fill;
    v.widgets.open.weak_bg_fill = hover_fill;
    v.widgets.open.corner_radius = radius;
    v
}

/// Install fonts, text sizes, spacing and the light theme.
pub fn install(ctx: &egui::Context) {
    install_fonts(ctx);
    ctx.set_visuals_of(Theme::Light, visuals(&Palette::light()));
    // Light only, whatever the Windows setting is.
    ctx.set_theme(ThemePreference::Light);
    ctx.all_styles_mut(|s| {
        s.text_styles = [
            (TextStyle::Heading, FontId::new(22.0, bold_family())),
            (TextStyle::Body, FontId::new(15.0, FontFamily::Proportional)),
            (TextStyle::Button, FontId::new(14.5, FontFamily::Proportional)),
            (TextStyle::Small, FontId::new(12.5, FontFamily::Proportional)),
            (TextStyle::Monospace, FontId::new(14.0, FontFamily::Monospace)),
        ]
        .into();
        s.spacing.item_spacing = egui::vec2(10.0, 8.0);
        s.spacing.button_padding = egui::vec2(14.0, 7.0);
        s.spacing.interact_size = egui::vec2(40.0, 34.0);
        s.spacing.combo_width = 90.0;
        s.spacing.scroll.bar_width = 8.0;
        s.spacing.scroll.floating = true;
    });
}
