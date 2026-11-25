//! Small custom controls that follow the design tokens.

use crate::icons::{self, Icon};
use crate::i18n;
use crate::theme::{self, Tokens, R_SM};
use egui::text::{LayoutJob, LayoutSection, TextFormat};
use egui::{pos2, vec2, Align2, Color32, CornerRadius, FontId, Rect, Response, Sense, Stroke, StrokeKind, Ui};

pub fn font(size: f32) -> FontId {
    FontId::new(size, egui::FontFamily::Proportional)
}
pub fn font_medium(size: f32) -> FontId {
    FontId::new(size, theme::medium())
}
pub fn font_semibold(size: f32) -> FontId {
    FontId::new(size, theme::semibold())
}
pub fn mono(size: f32) -> FontId {
    FontId::new(size, egui::FontFamily::Monospace)
}
pub fn mono_medium(size: f32) -> FontId {
    FontId::new(size, theme::mono_medium())
}

/// Small section caption.
pub fn caption(ui: &mut Ui, t: &Tokens, text: &str) {
    col(ui, |ui| label(ui, Txt::new(text).font(font_medium(12.0)).color(t.text_tertiary)));
}

#[derive(Clone, Copy, PartialEq)]
pub enum Variant {
    /// Solid accent — the one primary action per view.
    Primary,
    /// Transparent until hovered.
    Ghost,
    /// Hairline border.
    Outline,
    /// Accent hairline, for "stop" states.
    AccentOutline,
}

pub fn button(ui: &mut Ui, t: &Tokens, variant: Variant, icon: Option<Icon>, label: &str, hint: Option<&str>, width: Option<f32>, height: f32) -> Response {
    let rtl = crate::i18n::rtl();
    let label = i18n::tr(label);
    let icon = icon.map(|i| if rtl { mirror_icon(i) } else { i });
    let galley = text_galley(ui.painter(), &label, font_medium(13.0), Color32::PLACEHOLDER);
    let hint_g = hint.map(|h| ui.painter().layout_no_wrap(h.to_string(), mono(11.0), Color32::PLACEHOLDER));
    let icon_w = if icon.is_some() { 16.0 + 8.0 } else { 0.0 };
    let hint_w = hint_g.as_ref().map(|g| g.size().x + 12.0).unwrap_or(0.0);
    let natural = 12.0 * 2.0 + icon_w + galley.size().x + hint_w;
    let w = width.unwrap_or(natural);
    let (rect, resp) = ui.allocate_exact_size(vec2(w, height), Sense::click());
    let enabled = ui.is_enabled();
    let hovered = resp.hovered() && enabled;
    let pressed = resp.is_pointer_button_down_on() && enabled;
    let (fill, stroke, fg) = match variant {
        Variant::Primary => (
            if hovered { t.accent_hover } else { t.accent },
            Stroke::NONE,
            t.accent_fg,
        ),
        Variant::Ghost => (if hovered { t.hover_tint } else { Color32::TRANSPARENT }, Stroke::NONE, t.text_primary),
        Variant::Outline => (
            if hovered { t.hover_tint } else { Color32::TRANSPARENT },
            Stroke::new(1.0, if hovered { t.border_strong } else { t.border_default }),
            t.text_primary,
        ),
        Variant::AccentOutline => (
            if hovered { t.accent_soft } else { Color32::TRANSPARENT },
            Stroke::new(1.0, t.accent),
            t.accent,
        ),
    };
    let fg = if enabled { fg } else { t.text_tertiary };
    let fill = if enabled { fill } else { Color32::TRANSPARENT };
    let scale = if pressed { 0.985 } else { 1.0 };
    let r = Rect::from_center_size(rect.center(), rect.size() * scale);
    let p = ui.painter();
    p.rect(r, CornerRadius::same(R_SM + 2), fill, stroke, StrokeKind::Inside);
    if resp.has_focus() {
        p.rect_stroke(r.expand(2.0), CornerRadius::same(R_SM + 4), Stroke::new(2.0, t.accent_soft.gamma_multiply(2.5)), StrokeKind::Outside);
    }
    let content_w = icon_w + galley.size().x + hint_w;
    let hc = match variant {
        Variant::Primary => t.accent_fg.gamma_multiply(0.7),
        _ => t.text_tertiary,
    };
    if !rtl {
        let mut x = if width.is_some() { r.center().x - content_w / 2.0 } else { r.left() + 12.0 };
        if let Some(ic) = icon {
            icons::paint(p, Rect::from_center_size(pos2(x + 8.0, r.center().y), vec2(16.0, 16.0)), ic, fg);
            x += icon_w;
        }
        p.galley_with_override_text_color(pos2(x, r.center().y - galley.size().y / 2.0), galley.clone(), fg);
        x += galley.size().x + 12.0;
        if let Some(hg) = hint_g {
            p.galley_with_override_text_color(pos2(x, r.center().y - hg.size().y / 2.0), hg, hc);
        }
    } else {
        // Mirrored: icon on the right, then the label, the hint on the left.
        let mut x = if width.is_some() { r.center().x + content_w / 2.0 } else { r.right() - 12.0 };
        if let Some(ic) = icon {
            icons::paint(p, Rect::from_center_size(pos2(x - 8.0, r.center().y), vec2(16.0, 16.0)), ic, fg);
            x -= icon_w;
        }
        x -= galley.size().x;
        p.galley_with_override_text_color(pos2(x, r.center().y - galley.size().y / 2.0), galley.clone(), fg);
        x -= 12.0;
        if let Some(hg) = hint_g {
            p.galley_with_override_text_color(pos2(x - hg.size().x, r.center().y - hg.size().y / 2.0), hg, hc);
        }
    }
    resp.on_hover_cursor(if enabled { egui::CursorIcon::PointingHand } else { egui::CursorIcon::NotAllowed })
}

/// Square icon-only button.
pub fn icon_button(ui: &mut Ui, t: &Tokens, icon: Icon, tooltip: &str, active: bool) -> Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(32.0, 32.0), Sense::click());
    let p = ui.painter();
    if resp.hovered() || active {
        p.rect_filled(rect, CornerRadius::same(R_SM + 2), if active { t.accent_soft } else { t.hover_tint });
    }
    let c = if active {
        t.accent
    } else if resp.hovered() {
        t.text_primary
    } else {
        t.text_secondary
    };
    let icon = if crate::i18n::rtl() { mirror_icon(icon) } else { icon };
    icons::paint(p, Rect::from_center_size(rect.center(), vec2(18.0, 18.0)), icon, c);
    resp.on_hover_tr(tooltip).on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// iOS-style switch. Returns true when toggled.
pub fn switch(ui: &mut Ui, t: &Tokens, on: &mut bool) -> Response {
    let (rect, mut resp) = ui.allocate_exact_size(vec2(36.0, 20.0), Sense::click());
    if resp.clicked() {
        *on = !*on;
        resp.mark_changed();
    }
    let k = ui.ctx().animate_bool_with_time(resp.id, *on, 0.16);
    let p = ui.painter();
    let track = egui::lerp(egui::Rgba::from(t.border_default)..=egui::Rgba::from(t.accent), k);
    p.rect_filled(rect, CornerRadius::same(10), Color32::from(track));
    let x = egui::lerp(rect.left() + 10.0..=rect.right() - 10.0, k);
    p.circle_filled(pos2(x, rect.center().y), 7.5, if t.dark { t.text_primary } else { t.bg_elevated });
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// A row of equal segments. Returns the clicked index.
pub fn segmented(ui: &mut Ui, t: &Tokens, id: &str, options: &[&str], selected: usize, width: f32) -> Option<usize> {
    let h = 28.0;
    let (rect, _) = ui.allocate_exact_size(vec2(width, h), Sense::hover());
    let p = ui.painter().clone();
    p.rect(rect, CornerRadius::same(R_SM + 2), t.bg_inset, Stroke::new(1.0, t.border_subtle), StrokeKind::Inside);
    let n = options.len().max(1) as f32;
    let seg_w = (rect.width() - 4.0) / n;
    let mut clicked = None;
    let rtl = crate::i18n::rtl();
    let nn = options.len().max(1);
    let pos = |i: usize| if rtl { nn - 1 - i } else { i };
    let anim = ui.ctx().animate_value_with_time(egui::Id::unique(id).with("seg"), pos(selected) as f32, 0.18);
    let sel = Rect::from_min_size(pos2(rect.left() + 2.0 + anim * seg_w, rect.top() + 2.0), vec2(seg_w, h - 4.0));
    p.rect(sel, CornerRadius::same(R_SM), t.bg_elevated, Stroke::new(1.0, t.border_default), StrokeKind::Inside);
    for (i, label) in options.iter().enumerate() {
        let r = Rect::from_min_size(pos2(rect.left() + 2.0 + pos(i) as f32 * seg_w, rect.top() + 2.0), vec2(seg_w, h - 4.0));
        let resp = ui.interact(r, egui::Id::unique(id).with(i), Sense::click()).on_hover_cursor(egui::CursorIcon::PointingHand);
        if resp.clicked() {
            clicked = Some(i);
        }
        let c = if i == selected {
            t.text_primary
        } else if resp.hovered() {
            t.text_primary
        } else {
            t.text_secondary
        };
        paint_text(&p, r.center(), Align2::CENTER_CENTER, &i18n::tr(label), font_medium(12.0), c);
    }
    clicked
}

/// Card container: surface + subtle border, no shadow.
pub fn card<R>(ui: &mut Ui, t: &Tokens, add: impl FnOnce(&mut Ui) -> R) -> R {
    egui::Frame::new()
        .fill(t.bg_surface)
        .stroke(Stroke::new(1.0, t.border_subtle))
        .corner_radius(CornerRadius::same(theme::R))
        .inner_margin(egui::Margin::same(16))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            add(ui)
        })
        .inner
}

pub fn status_dot(ui: &mut Ui, color: Color32, pulse: bool) {
    let (rect, _) = ui.allocate_exact_size(vec2(8.0, 16.0), Sense::hover());
    let c = rect.center();
    if pulse {
        let time = ui.input(|i| i.time) as f32;
        let k = (time * 2.2).sin() * 0.5 + 0.5;
        ui.painter().circle_filled(c, 3.5 + 2.5 * k, color.gamma_multiply(0.25 * (1.0 - k)));
        ui.ctx().request_repaint_after(std::time::Duration::from_millis(33));
    }
    ui.painter().circle_filled(c, 3.5, color);
}

/// Keycap glyph like `1` or `⌘K`.
pub fn keycap(p: &egui::Painter, t: &Tokens, at: egui::Pos2, text: &str, align: Align2) -> Rect {
    let g = p.layout_no_wrap(text.to_string(), mono_medium(11.0), t.text_tertiary);
    let size = vec2((g.size().x + 10.0).max(20.0), 20.0);
    let r = align.anchor_size(at, size);
    p.rect(r, CornerRadius::same(5), t.bg_inset, Stroke::new(1.0, t.border_default), StrokeKind::Inside);
    p.galley(pos2(r.center().x - g.size().x / 2.0, r.center().y - g.size().y / 2.0), g, t.text_tertiary);
    r
}

/// Horizontal level meter.
pub fn meter(ui: &mut Ui, t: &Tokens, level: f32, width: f32) {
    let (rect, _) = ui.allocate_exact_size(vec2(width, 6.0), Sense::hover());
    let p = ui.painter();
    p.rect_filled(rect, CornerRadius::same(3), t.bg_inset);
    let db = 20.0 * level.max(1e-5).log10();
    let k = ((db + 60.0) / 60.0).clamp(0.0, 1.0);
    let mut fill = rect;
    fill.set_width(rect.width() * k);
    let c = if k > 0.92 { t.danger } else if k > 0.75 { t.warning } else { t.success };
    p.rect_filled(fill, CornerRadius::same(3), c);
}

/// Control-panel tile: an icon (or big number) over a short caption.
/// `on` tints it with the accent (live / recording / selected).
pub fn tile(ui: &mut Ui, t: &Tokens, size: egui::Vec2, icon: Option<Icon>, number: Option<&str>, caption: &str, on: bool, warn: bool) -> Response {
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    let hovered = resp.hovered();
    let pressed = resp.is_pointer_button_down_on();
    let p = ui.painter();
    let r = if pressed { rect.shrink(1.0) } else { rect };
    let (fill, stroke) = if on {
        (t.accent_soft, Stroke::new(1.0, t.accent.gamma_multiply(0.7)))
    } else if warn {
        (t.hover_tint, Stroke::new(1.0, t.warning.gamma_multiply(0.7)))
    } else {
        (if hovered { t.bg_elevated } else { t.bg_surface }, Stroke::new(1.0, if hovered { t.border_strong } else { t.border_subtle }))
    };
    p.rect(r, CornerRadius::same(theme::R), fill, stroke, StrokeKind::Inside);
    let fg = if on {
        t.accent
    } else if warn {
        t.warning
    } else {
        t.text_primary
    };
    let top = pos2(r.center().x, r.top() + r.height() * 0.40);
    if let Some(n) = number {
        p.text(top, Align2::CENTER_CENTER, n, font_semibold(18.0), fg);
    } else if let Some(ic) = icon {
        let ic = if crate::i18n::rtl() { mirror_icon(ic) } else { ic };
        icons::paint(p, Rect::from_center_size(top, vec2(20.0, 20.0)), ic, fg);
    }
    let cap_c = if on { t.accent } else { t.text_secondary };
    paint_text(p, pos2(r.center().x, r.bottom() - 11.0), Align2::CENTER_CENTER, &i18n::tr(caption), font_medium(10.5), cap_c);
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

// ---------------------------------------------------------------------------
// Language-aware text and mirrored layout.

pub fn language_picker(ui: &mut Ui, width: f32, id: &str, current: i18n::Lang) -> Option<i18n::Lang> {
    use egui::{ComboBox, Key};

    let combo_id = ui.make_persistent_id(egui::IdSalt::new(id));
    let was_open = ComboBox::is_open(ui.ctx(), combo_id);
    let mut picked = None;
    let picker = ComboBox::from_id_salt(id)
        .selected_text(Txt::raw(current.native()).font(font(13.0)))
        .width(width)
        .truncate()
        .show_ui(ui, |ui| {
            for &language in i18n::Lang::ALL {
                let is_current = language == current;
                let option = ui
                    .add(
                        egui::Button::selectable(is_current, Txt::raw(language.native()).font(font(13.0)))
                            .min_size(vec2(ui.available_width(), 32.0)),
                    )
                    .accessible_name(language.native());
                if option.clicked() {
                    picked = Some(language);
                    ui.close();
                }
            }
        })
        .response
        .accessible_name(i18n::tr("Language").into_owned())
        .on_hover_cursor(egui::CursorIcon::PointingHand);
    if was_open && !ComboBox::is_open(ui.ctx(), combo_id) && (picked.is_some() || ui.input(|i| i.key_pressed(Key::Escape))) {
        picker.request_focus();
    }
    picked
}

fn text_job(text: &str, font: FontId, color: Color32) -> LayoutJob {
    let format = TextFormat { font_id: font, color, ..Default::default() };
    if !i18n::has_rtl_text(text) {
        return LayoutJob::single_section(text.to_owned(), format);
    }
    let mut job = LayoutJob::default();
    for (i, line) in text.split('\n').enumerate() {
        if i > 0 {
            job.append("\n", 0.0, format.clone());
        }
        i18n::shape_line(&i18n::paragraph(line), 0..line.len(), |run| append_text_run(&mut job, run, &format));
    }
    job
}

fn append_text_run(job: &mut LayoutJob, run: &str, format: &TextFormat) {
    let start = job.text.len();
    job.text.extend(run.chars().filter(|c| {
        !matches!(c, '\u{061C}' | '\u{200E}' | '\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}')
    }));
    if job.text.len() > start {
        // LayoutJob::append merges equal formats, but the runs require
        // independent shaping even when they share a font.
        job.sections.push(LayoutSection {
            byte_range: start.into()..job.text.len().into(),
            leading_space: 0.0,
            format: format.clone(),
        });
    }
}

pub fn text_galley(painter: &egui::Painter, text: &str, font: FontId, color: Color32) -> std::sync::Arc<egui::Galley> {
    painter.layout_job(text_job(text, font, color))
}

pub fn paint_text(painter: &egui::Painter, pos: egui::Pos2, align: Align2, text: &str, font: FontId, color: Color32) {
    let galley = text_galley(painter, text, font, color);
    let rect = align.anchor_size(pos, galley.size());
    painter.galley(rect.min, galley, color);
}

/// Direction-aware arrows (Back/Next swap sides in Hebrew).
pub fn mirror_icon(i: Icon) -> Icon {
    match i {
        Icon::Back => Icon::Forward,
        Icon::Forward => Icon::Back,
        other => other,
    }
}

/// Text to show: translated, then laid out for the current language.
#[derive(Clone)]
pub struct Txt {
    pub text: String,
    pub font: Option<FontId>,
    pub color: Option<Color32>,
}

impl Txt {
    pub fn new(s: impl AsRef<str>) -> Txt {
        Txt::raw(i18n::tr(s.as_ref()))
    }
    fn raw(s: impl Into<String>) -> Txt {
        Txt { text: s.into(), font: None, color: None }
    }
    pub fn font(mut self, f: FontId) -> Txt {
        self.font = Some(f);
        self
    }
    pub fn color(mut self, c: Color32) -> Txt {
        self.color = Some(c);
        self
    }
    fn rich(&self, s: &str) -> egui::RichText {
        let mut r = egui::RichText::new(s);
        if let Some(f) = &self.font {
            r = r.font(f.clone());
        }
        if let Some(c) = self.color {
            r = r.color(c);
        }
        r
    }
}

/// Single-line use in combo boxes, links and menu items.
impl From<Txt> for egui::WidgetText {
    fn from(t: Txt) -> egui::WidgetText {
        if i18n::has_rtl_text(&t.text) {
            text_job(&t.text, t.font.unwrap_or_else(|| font(14.0)), t.color.unwrap_or(Color32::PLACEHOLDER)).into()
        } else {
            t.rich(&t.text).into()
        }
    }
}

/// A label. In Hebrew it wraps line by line and each line is reordered,
/// right-aligned — egui itself only knows left-to-right text.
pub fn label(ui: &mut Ui, t: Txt) -> Response {
    if !i18n::has_rtl_text(&t.text) {
        let r = t.rich(&t.text);
        return ui.label(r);
    }
    let right = crate::i18n::rtl();
    let font = t.font.clone().unwrap_or_else(|| egui::TextStyle::Body.resolve(ui.style()));
    let color = t.color.unwrap_or_else(|| ui.visuals().text_color());
    let max_w = ui.available_width().max(40.0);
    let format = TextFormat { font_id: font, color, ..Default::default() };
    let mut galleys = Vec::new();
    for para in t.text.split('\n') {
        let info = i18n::paragraph(para);
        let layout = |line| {
            let mut job = LayoutJob::default();
            i18n::shape_line(&info, line, |run| append_text_run(&mut job, run, &format));
            if job.sections.is_empty() {
                job.append("", 0.0, format.clone());
            }
            ui.painter().layout_job(job)
        };
        let full_line = layout(0..para.len());
        if full_line.size().x <= max_w || !para.contains(' ') {
            galleys.push(full_line);
            continue;
        }
        let mut start = 0;
        let mut end = 0;
        for word_end in para.match_indices(' ').map(|(i, _)| i).chain([para.len()]) {
            if end > start && layout(start..word_end).size().x > max_w {
                galleys.push(layout(start..end));
                start = end + 1;
            }
            end = word_end;
        }
        galleys.push(layout(start..end));
    }
    let width = galleys.iter().map(|g| g.size().x).fold(0.0, f32::max).min(max_w);
    let height = galleys.iter().map(|g| g.size().y).sum();
    let (rect, resp) = ui.allocate_exact_size(vec2(width, height), Sense::hover());
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::accesskit::Role::Label, ui.is_enabled(), &t.text));
    let painter = ui.painter().with_clip_rect(rect);
    let mut y = rect.top();
    for g in galleys {
        let x = if right { rect.right() - g.size().x } else { rect.left() };
        let height = g.size().y;
        painter.galley(pos2(x, y), g, color);
        y += height;
    }
    resp
}

/// A truncated single line (long titles, addresses).
pub fn label_trunc(ui: &mut Ui, t: Txt) -> Response {
    ui.add(egui::Label::new(t).truncate())
}

/// Horizontal row that reads in the language's direction.
pub fn row<R>(ui: &mut Ui, add: impl FnOnce(&mut Ui) -> R) -> egui::InnerResponse<R> {
    let layout = if crate::i18n::rtl() { egui::Layout::right_to_left(egui::Align::Center) } else { egui::Layout::left_to_right(egui::Align::Center) };
    let size = vec2(ui.available_size_before_wrap().x, ui.spacing().interact_size.y);
    ui.allocate_ui_with_layout(size, layout, add)
}

/// Wrapping row (status lines).
pub fn row_wrapped<R>(ui: &mut Ui, add: impl FnOnce(&mut Ui) -> R) -> egui::InnerResponse<R> {
    if crate::i18n::rtl() {
        let layout = egui::Layout::right_to_left(egui::Align::Center).with_main_wrap(true);
        let size = vec2(ui.available_size_before_wrap().x, ui.spacing().interact_size.y);
        ui.allocate_ui_with_layout(size, layout, add)
    } else {
        ui.horizontal_wrapped(add)
    }
}

/// Column aligned to the reading start (left, or right in Hebrew).
pub fn col<R>(ui: &mut Ui, add: impl FnOnce(&mut Ui) -> R) -> egui::InnerResponse<R> {
    let layout = egui::Layout::top_down(if crate::i18n::rtl() { egui::Align::Max } else { egui::Align::Min });
    ui.with_layout(layout, add)
}

/// Layout that pushes items to the far end of a row.
pub fn end(align: egui::Align) -> egui::Layout {
    if crate::i18n::rtl() {
        egui::Layout::left_to_right(align)
    } else {
        egui::Layout::right_to_left(align)
    }
}

/// Translated tooltips.
pub trait HoverTr {
    fn on_hover_tr(self, s: impl AsRef<str>) -> Self;
}

impl HoverTr for Response {
    fn on_hover_tr(self, s: impl AsRef<str>) -> Response {
        let text = Txt::new(s);
        self.on_hover_ui(|ui| {
            label(ui, text);
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hebrew_glyphs_read_from_the_right() {
        let ctx = egui::Context::default();
        theme::install_fonts(&ctx);
        ctx.run_ui(egui::RawInput::default(), |ui| {
            for (text, expected) in [
                ("מסך או חלון", "ןולח וא ךסמ"),
                ("עברית", "תירבע"),
                ("בחרו (Studio Record) עכשיו", "וישכע (Studio Record) ורחב"),
                ("Language · שפה", "Language · הפש"),
                ("פעיל · \u{2066}1920×1080\u{2069} · \u{2066}30 fps\u{2069}", "30 fps · 1920×1080 · ליעפ"),
            ] {
                let galley = text_galley(ui.painter(), text, font(14.0), Color32::PLACEHOLDER);
                let glyphs: String = galley.rows[0].glyphs.iter()
                    .filter(|glyph| glyph.advance_width > 0.0)
                    .map(|glyph| glyph.chr)
                    .collect();
                assert_eq!(glyphs, expected, "{text}");
            }
        }).drop_without_applying_deltas();
    }

    #[test]
    fn language_menu_can_be_used_from_the_keyboard() {
        use egui::{ComboBox, Event, Key, Modifiers};
        use i18n::Lang;

        let ctx = egui::Context::default();
        theme::install_fonts(&ctx);
        let mut current = Lang::En;
        let mut menu_open = false;
        let mut run_frame = |key: Option<Key>| {
            let events = key.into_iter().flat_map(|key| [true, false].map(|pressed| Event::Key {
                key,
                physical_key: None,
                pressed,
                repeat: false,
                modifiers: Modifiers::NONE,
            })).collect();
            ctx.run_ui(egui::RawInput { events, ..Default::default() }, |ui| {
                let id = ui.make_persistent_id(egui::IdSalt::new("test-language"));
                if let Some(lang) = language_picker(ui, 200.0, "test-language", current) {
                    current = lang;
                }
                menu_open = ComboBox::is_open(&ctx, id);
            }).drop_without_applying_deltas();
            (current, menu_open)
        };

        run_frame(None);
        run_frame(Some(Key::Tab));
        assert_eq!(run_frame(Some(Key::Enter)), (Lang::En, true));
        run_frame(None);
        run_frame(Some(Key::ArrowDown));
        run_frame(Some(Key::ArrowDown));
        assert_eq!(run_frame(Some(Key::Enter)), (Lang::He, false));
        run_frame(None);
        assert_eq!(run_frame(Some(Key::Space)), (Lang::He, true));
        assert_eq!(run_frame(Some(Key::Escape)), (Lang::He, false));
    }
}
