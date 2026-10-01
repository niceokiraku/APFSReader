//! The window's building blocks: cards, badges, buttons, alerts, icons.

use crate::theme::{bold_family, icon_family, icons_available, Palette};
use eframe::egui::{
    self, text::LayoutJob, vec2, Align, Color32, CornerRadius, FontFamily, FontId, Frame, Margin, Pos2, Rect,
    Response, RichText, Sense, Shape, Stroke, StrokeKind, TextFormat, Ui,
};

/// Glyphs from Segoe Fluent Icons / Segoe MDL2 Assets.
pub mod glyph {
    pub const ADD: char = '\u{E710}';
    pub const REFRESH: char = '\u{E72C}';
    pub const FOLDER_OPEN: char = '\u{E838}';
    pub const REMOVE: char = '\u{E711}';
    pub const WARNING: char = '\u{E7BA}';
    pub const INFO: char = '\u{E946}';
}

pub fn bold(text: impl Into<String>, size: f32) -> RichText {
    RichText::new(text).family(bold_family()).size(size)
}

pub fn muted(p: &Palette, text: impl Into<String>) -> RichText {
    RichText::new(text).color(p.muted).size(12.5)
}

/// A rounded panel on the page background. `accent` paints a coloured bar down
/// its left edge.
pub fn card<R>(ui: &mut Ui, p: &Palette, accent: Option<Color32>, add: impl FnOnce(&mut Ui) -> R) -> R {
    let out = Frame::new()
        .fill(p.surface)
        .stroke(Stroke::new(1.0, p.border))
        .corner_radius(CornerRadius::same(12))
        .inner_margin(Margin::symmetric(18, 16))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            add(ui)
        });
    if let Some(color) = accent {
        let r = out.response.rect;
        ui.painter().rect_filled(
            Rect::from_min_size(r.min, vec2(4.0, r.height())),
            CornerRadius { nw: 12, sw: 12, ne: 0, se: 0 },
            color,
        );
    }
    out.inner
}

/// A small coloured pill with a short label.
pub fn badge(ui: &mut Ui, text: &str, fg: Color32, bg: Color32) {
    // Drawn by hand so the height is exactly the text plus padding, whatever
    // the surrounding layout would otherwise stretch a frame to.
    let galley = ui.painter().layout_no_wrap(text.to_string(), FontId::new(12.5, bold_family()), fg);
    let (rect, _) = ui.allocate_exact_size(galley.size() + vec2(18.0, 6.0), Sense::hover());
    ui.painter().rect_filled(rect, CornerRadius::same(99), bg);
    ui.painter().galley(rect.center() - galley.size() / 2.0, galley, fg);
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// The one main action in view.
    Primary,
    Secondary,
    /// Low emphasis: no fill until hovered.
    Ghost,
    /// A destructive or releasing action.
    Danger,
}

fn button_job(p: &Palette, icon: Option<char>, text: &str, color: Color32, size: f32) -> LayoutJob {
    let mut job = LayoutJob::default();
    let icon = icon.filter(|_| icons_available());
    if let Some(g) = icon {
        job.append(
            &format!("{g}  "),
            0.0,
            TextFormat { font_id: FontId::new(size + 1.0, icon_family()), color, valign: Align::Center, ..Default::default() },
        );
    }
    job.append(
        text,
        0.0,
        TextFormat { font_id: FontId::new(size, bold_family_or_regular(p)), color, valign: Align::Center, ..Default::default() },
    );
    job
}

fn bold_family_or_regular(_p: &Palette) -> FontFamily {
    bold_family()
}

pub fn button(ui: &mut Ui, p: &Palette, kind: Kind, icon: Option<char>, text: &str) -> Response {
    let (fill, stroke, color) = match kind {
        Kind::Primary => (p.accent, Stroke::NONE, p.on_accent),
        Kind::Secondary => (p.surface_alt, Stroke::new(1.0, p.border), p.text),
        Kind::Ghost => (Color32::TRANSPARENT, Stroke::NONE, p.muted),
        Kind::Danger => (Color32::TRANSPARENT, Stroke::new(1.0, p.danger), p.danger),
    };
    let size = if kind == Kind::Ghost { 14.0 } else { 14.5 };
    let b = egui::Button::new(button_job(p, icon, text, color, size))
        .fill(fill)
        .stroke(stroke)
        .corner_radius(CornerRadius::same(8))
        .min_size(vec2(0.0, 34.0));
    let r = ui.add(b);
    if kind == Kind::Primary && r.hovered() {
        ui.painter().rect_filled(r.rect, CornerRadius::same(8), p.accent_hover);
        // Redraw the label on the hover fill.
        let galley = ui.painter().layout_job(button_job(p, icon, text, color, size));
        let pos = r.rect.center() - galley.size() / 2.0;
        ui.painter().galley(pos, galley, color);
    }
    r.on_hover_cursor(egui::CursorIcon::PointingHand)
}

#[derive(Clone, Copy)]
pub enum Tone {
    Warn,
    Danger,
}

/// A tinted message box.
pub fn alert(ui: &mut Ui, p: &Palette, tone: Tone, text: &str) {
    let (fg, bg, g) = match tone {
        Tone::Warn => (p.warn, p.warn_bg, glyph::WARNING),
        Tone::Danger => (p.danger, p.danger_bg, glyph::WARNING),
    };
    Frame::new().fill(bg).corner_radius(CornerRadius::same(8)).inner_margin(Margin::symmetric(12, 8)).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal_top(|ui| {
            if icons_available() {
                ui.label(RichText::new(g.to_string()).family(icon_family()).color(fg).size(14.0));
            }
            ui.add(egui::Label::new(RichText::new(text).color(fg).size(13.5)).wrap());
        });
    });
}

/// Section title with an optional count and room on the right for controls.
pub fn section_header(ui: &mut Ui, p: &Palette, title: &str, count: Option<usize>, right: impl FnOnce(&mut Ui)) {
    ui.add_space(10.0);
    ui.horizontal(|ui| {
        ui.label(bold(title, 16.0).color(p.text));
        if let Some(n) = count {
            badge(ui, &n.to_string(), p.muted, p.surface_alt);
        }
        ui.with_layout(egui::Layout::right_to_left(Align::Center), right);
    });
    ui.add_space(2.0);
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum IconKind {
    Image,
    Disk,
    Usb,
    Mounted,
}

/// A small drawn icon on a tinted tile, for a card.
pub fn drive_icon(ui: &mut Ui, p: &Palette, kind: IconKind, size: f32) -> Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(size, size), Sense::hover());
    let tint = if kind == IconKind::Mounted { p.success } else { p.accent };
    let tile = Color32::from_rgba_unmultiplied(tint.r(), tint.g(), tint.b(), 30);
    let painter = ui.painter();
    painter.rect_filled(rect, CornerRadius::same(10), tile);
    let c = rect.center();
    let s = size;
    let line = Stroke::new((s * 0.055).max(1.4), tint);
    match kind {
        IconKind::Image => {
            // A disc: the thing an image file stands for.
            painter.circle_stroke(c, s * 0.27, line);
            painter.circle_stroke(c, s * 0.13, Stroke::new(line.width * 0.8, tint));
            painter.circle_filled(c, s * 0.04, tint);
        }
        IconKind::Disk | IconKind::Mounted => {
            let body = Rect::from_center_size(c, vec2(s * 0.58, s * 0.38));
            painter.rect_stroke(body, CornerRadius::same(5), line, StrokeKind::Inside);
            painter.line_segment(
                [Pos2::new(body.left() + s * 0.07, body.bottom() - s * 0.1), Pos2::new(body.center().x + s * 0.04, body.bottom() - s * 0.1)],
                Stroke::new(line.width, tint),
            );
            painter.circle_filled(Pos2::new(body.right() - s * 0.09, body.bottom() - s * 0.1), s * 0.03, tint);
            if kind == IconKind::Mounted {
                // A check mark across the top.
                let a = Pos2::new(c.x - s * 0.1, c.y - s * 0.07);
                let b = Pos2::new(c.x - s * 0.03, c.y);
                let d = Pos2::new(c.x + s * 0.11, c.y - s * 0.16);
                painter.add(Shape::line(vec![a, b, d], Stroke::new(line.width * 1.2, tint)));
            }
        }
        IconKind::Usb => {
            let body = Rect::from_center_size(Pos2::new(c.x, c.y + s * 0.05), vec2(s * 0.3, s * 0.42));
            painter.rect_stroke(body, CornerRadius::same(4), line, StrokeKind::Inside);
            let plug = Rect::from_center_size(Pos2::new(c.x, body.top() - s * 0.06), vec2(s * 0.18, s * 0.12));
            painter.rect_stroke(plug, CornerRadius::same(2), Stroke::new(line.width * 0.9, tint), StrokeKind::Inside);
            painter.circle_filled(Pos2::new(c.x, body.center().y + s * 0.05), s * 0.035, tint);
        }
    }
    resp
}

/// A dashed drop target with a message.
pub fn drop_zone(ui: &mut Ui, p: &Palette, hovered: bool, title: &str, subtitle: &str, add: impl FnOnce(&mut Ui)) {
    let width = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(vec2(width, 150.0), Sense::hover());
    let color = if hovered { p.accent } else { p.faint };
    let fill = if hovered {
        Color32::from_rgba_unmultiplied(p.accent.r(), p.accent.g(), p.accent.b(), 28)
    } else {
        Color32::TRANSPARENT
    };
    ui.painter().rect_filled(rect, CornerRadius::same(12), fill);
    let r = rect.shrink(1.0);
    let corners = [r.left_top(), r.right_top(), r.right_bottom(), r.left_bottom(), r.left_top()];
    ui.painter().extend(Shape::dashed_line(&corners, Stroke::new(1.5, color), 7.0, 5.0));
    let mut inner = ui.new_child(egui::UiBuilder::new().max_rect(rect.shrink(16.0)).layout(egui::Layout::top_down(Align::Center)));
    inner.add_space(14.0);
    inner.label(bold(title, 16.0).color(p.text));
    inner.label(muted(p, subtitle));
    inner.add_space(8.0);
    add(&mut inner);
}

/// A section title that folds its content: the whole row is a button, with a
/// chevron at the right, down when folded ("open me") and up when open.
pub fn collapsible_header(ui: &mut Ui, p: &Palette, title: &str, count: Option<usize>, open: &mut bool) -> Rect {
    ui.add_space(10.0);
    let row = ui.horizontal(|ui| {
        ui.label(bold(title, 16.0).color(p.text));
        if let Some(n) = count {
            badge(ui, &n.to_string(), p.muted, p.surface_alt);
        }
        if !*open {
            ui.label(muted(p, crate::lang::t("クリックで展開", "click to expand")));
        }
        ui.with_layout(egui::Layout::right_to_left(Align::Center), |ui| {
            let (rect, _) = ui.allocate_exact_size(vec2(22.0, 22.0), Sense::hover());
            let c = rect.center();
            let (w, h) = (6.5, 3.5);
            let pts = if *open {
                vec![Pos2::new(c.x - w, c.y + h), Pos2::new(c.x + w, c.y + h), Pos2::new(c.x, c.y - h)]
            } else {
                vec![Pos2::new(c.x - w, c.y - h), Pos2::new(c.x + w, c.y - h), Pos2::new(c.x, c.y + h)]
            };
            ui.painter().add(Shape::convex_polygon(pts, p.muted, Stroke::NONE));
        });
    });
    let id = ui.id().with(("collapsible", title));
    let click = ui.interact(row.response.rect, id, Sense::click());
    if click.hovered() {
        ui.painter().rect_filled(
            row.response.rect.expand2(vec2(6.0, 2.0)),
            CornerRadius::same(8),
            Color32::from_rgba_unmultiplied(p.muted.r(), p.muted.g(), p.muted.b(), 22),
        );
    }
    if click.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
        *open = !*open;
    }
    ui.add_space(2.0);
    row.response.rect
}

#[cfg(test)]
mod tests {
    use super::*;
    use eframe::egui::{Event, Modifiers, PointerButton, RawInput};

    /// Run one frame of the header with the given pointer events.
    fn frame(ctx: &egui::Context, open: &mut bool, events: Vec<Event>) -> Rect {
        let p = Palette::light();
        let mut rect = Rect::NOTHING;
        let input = RawInput { events, ..Default::default() };
        let mut out = ctx.run_ui(input, |ui| {
            rect = collapsible_header(ui, &p, "Disks", Some(3), open);
        });
        out.textures_delta.clear();
        rect
    }

    fn press(pos: Pos2, down: bool) -> Event {
        Event::PointerButton { pos, button: PointerButton::Primary, pressed: down, modifiers: Modifiers::NONE }
    }

    #[test]
    fn clicking_the_row_unfolds_it_and_clicking_again_folds_it() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let mut open = false;
        let row = frame(&ctx, &mut open, vec![]);
        assert!(!open && row.width() > 100.0);

        let at = row.center();
        // Move onto the row, press, then release in a later frame, as a mouse does.
        frame(&ctx, &mut open, vec![Event::PointerMoved(at)]);
        frame(&ctx, &mut open, vec![press(at, true)]);
        frame(&ctx, &mut open, vec![press(at, false)]);
        assert!(open, "one click unfolds");

        frame(&ctx, &mut open, vec![press(at, true)]);
        frame(&ctx, &mut open, vec![press(at, false)]);
        assert!(!open, "a second click folds it again");
    }

    #[test]
    fn clicking_elsewhere_does_nothing() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let mut open = false;
        let row = frame(&ctx, &mut open, vec![]);
        let away = Pos2::new(row.center().x, row.bottom() + 200.0);
        frame(&ctx, &mut open, vec![Event::PointerMoved(away)]);
        frame(&ctx, &mut open, vec![press(away, true)]);
        frame(&ctx, &mut open, vec![press(away, false)]);
        assert!(!open);
    }
}
