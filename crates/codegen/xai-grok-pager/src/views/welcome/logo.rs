//! Logo component — renders the braille art logo.
//!
//! DTTN customization: the upstream single-letter mark is replaced by a
//! DTTN wordmark while retaining the original animated gradient renderer.
//!
//! Hidden entirely on legacy Windows consoles: the U+2800 braille block is
//! not covered by the ConHost raster fonts and would render as tofu.

use ratatui::buffer::Buffer;
use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Widget};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};

use crate::render::color::blend_color;
use crate::theme::Theme;

const LOGO: &str = include_str!("../../../assets/logo/dttn07.txt");
const LOGO_SMALL: &str = include_str!("../../../assets/logo/dttn05.txt");
const SPIC_ITERM_PNG: &[u8] =
    include_bytes!("../../../assets/logo/spic-iterm-seamless-groknight.png");

// SPIC palette sampled from the user-provided PNG. The mark keeps its original
// green/red/orange colors. Only the navy text (source approx. #151764) gets
// display-adjusted variants so the character fallback stays legible on dark
// terminal themes, as requested.
const SPIC_TEXT_BLUE: Color = Color::Rgb(0x4B, 0x52, 0xCC);
const SPIC_TEXT_BLUE_HILITE: Color = Color::Rgb(0x9A, 0xA2, 0xFF);
const SPIC_GREEN: Color = Color::Rgb(0x50, 0xB5, 0x06);
const SPIC_GREEN_HILITE: Color = Color::Rgb(0x90, 0xC2, 0x30);
const SPIC_DARK_GREEN: Color = Color::Rgb(0x00, 0x6C, 0x34);
const SPIC_DARK_GREEN_HILITE: Color = Color::Rgb(0x00, 0x91, 0x2C);
const SPIC_RED: Color = Color::Rgb(0xFC, 0x00, 0x00);
const SPIC_RED_HILITE: Color = Color::Rgb(0xFF, 0x6C, 0x00);
const SPIC_DARK_RED: Color = Color::Rgb(0xB4, 0x00, 0x00);
const SPIC_DARK_RED_HILITE: Color = Color::Rgb(0xE8, 0x00, 0x00);
const SPIC_ORANGE: Color = Color::Rgb(0xFC, 0x35, 0x06);
const SPIC_ORANGE_HILITE: Color = Color::Rgb(0xFF, 0x6C, 0x00);

/// Two source-image pixels are packed into each terminal cell with `▀`.
/// `G/g/R/r/O` select green/dark-green/red/dark-red/orange source colors.
const SPIC_MARK_PIXELS: [&str; 8] = [
    ".GGGGGG...",
    "GGGGGGGg..",
    "....GGggg.",
    "...RRRgg..",
    "RRRRRR....",
    "RRRRRRR...",
    "..rRRROOO.",
    "...rrrOO..",
];
const SPIC_MARK_WIDTH: u16 = 10;
const SPIC_FULL_HEIGHT: u16 = 4;
const SPIC_FULL_WIDTH: u16 = 20;
const SPIC_DTTN_GAP: u16 = 1;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct ItermLogoPlacement {
    image: Rect,
    viewport_width: u16,
    viewport_height: u16,
}

static ITERM_LOGO_PLACEMENT: OnceLock<Mutex<Option<ItermLogoPlacement>>> = OnceLock::new();
static ITERM_LOGO_VISIBLE: AtomicBool = AtomicBool::new(false);

/// Compact SPIC wordmark painted at the right edge of the interactive footer.
pub const SPIC_CORNER_WIDTH: u16 = 16;
const SPIC_CORNER_MIN_AREA_WIDTH: u16 = 44;
const SPIC_CORNER_MARK_WIDTH: u16 = 2;
const SPIC_CORNER_GAP: u16 = 1;

/// Height at or above which the small logo is shown (below it, no logo).
const SMALL_LOGO_MIN_HEIGHT: u16 = 22;
/// Height at or above which the full logo is shown.
const FULL_LOGO_MIN_HEIGHT: u16 = 26;

fn pick_logo(window_height: u16) -> Option<&'static str> {
    pick_logo_for(window_height, logo_hidden())
}

/// Pure tier selection so tests can drive the legacy-console flag directly.
fn pick_logo_for(window_height: u16, hidden: bool) -> Option<&'static str> {
    if hidden || window_height < SMALL_LOGO_MIN_HEIGHT {
        None
    } else if window_height < FULL_LOGO_MIN_HEIGHT {
        Some(LOGO_SMALL)
    } else {
        Some(LOGO)
    }
}

/// The braille art has no ASCII stand-in; see the module doc.
fn logo_hidden() -> bool {
    crate::glyphs::is_legacy_windows_console()
}

fn non_empty_lines(logo: &str) -> impl Iterator<Item = &str> {
    logo.lines().filter(|l| !l.is_empty())
}

fn count_lines(logo: &str) -> u16 {
    non_empty_lines(logo).count() as u16
}

fn visual_width(logo: &str) -> u16 {
    non_empty_lines(logo)
        .map(unicode_width::UnicodeWidthStr::width)
        .max()
        .unwrap_or(24) as u16
}

/// Animation phase in seconds since the first render. Wall-clock based so the
/// shimmer speed is independent of the frame rate.
fn anim_phase_secs() -> f32 {
    use std::sync::OnceLock;
    use std::time::Instant;
    static START: OnceLock<Instant> = OnceLock::new();
    START.get_or_init(Instant::now).elapsed().as_secs_f32()
}

/// Shimmer redraw cadence in frames per second. The sweep is slow, so a few fps
/// looks smooth while sparing the long-lived welcome screen from full-rate
/// repaints.
const SHIMMER_FPS: f32 = 12.0;

/// Quantized shimmer frame for the current wall-clock phase. The welcome screen
/// redraws only when this advances, throttling the animation to ~`SHIMMER_FPS`
/// rather than the full event-loop tick rate. Pinned to 0 when the logo is
/// hidden.
pub fn shimmer_frame() -> u64 {
    if logo_hidden() {
        return 0;
    }
    (anim_phase_secs() * SHIMMER_FPS) as u64
}

/// Per-glyph shine opacity in `[0, 1]` at normalized diagonal position `diag`
/// (0 = bottom-left .. 1 = top-right) and animation time `secs`. A raised-cosine
/// band sweeps bottom-left → top-right and parks off-screen between sweeps; a
/// gentle global pulse breathes underneath it. 0 keeps the resting gray, 1 is
/// full bright.
fn shine_opacity(diag: f32, secs: f32) -> f32 {
    const BAND: f32 = 0.38; // half-width of the shine band — wider = more gradual falloff
    const CYCLE: f32 = 4.0; // seconds per sweep + rest
    const SWEEP_FRAC: f32 = 0.32; // portion of the cycle spent sweeping (~1.3s glint, rest idles)
    const SHINE: f32 = 0.33; // peak shine strength
    const PULSE: f32 = 0.06; // global breathing amount
    const PULSE_SECS: f32 = 5.0; // breathing period

    let p = (secs % CYCLE) / CYCLE;
    let q = (p / SWEEP_FRAC).min(1.0); // parks the band off-screen during the rest
    let band_pos = -BAND + q * (1.0 + 2.0 * BAND);
    let pulse = PULSE * (0.5 - 0.5 * (std::f32::consts::TAU * secs / PULSE_SECS).cos());

    let d = (diag - band_pos).abs();
    let shine = if d < BAND {
        0.5 * (1.0 + (std::f32::consts::PI * d / BAND).cos())
    } else {
        0.0
    };
    (pulse + SHINE * shine).clamp(0.0, 1.0)
}

fn spic_palette(code: char) -> Option<(Color, Color)> {
    match code {
        'G' => Some((SPIC_GREEN, SPIC_GREEN_HILITE)),
        'g' => Some((SPIC_DARK_GREEN, SPIC_DARK_GREEN_HILITE)),
        'R' => Some((SPIC_RED, SPIC_RED_HILITE)),
        'r' => Some((SPIC_DARK_RED, SPIC_DARK_RED_HILITE)),
        'O' => Some((SPIC_ORANGE, SPIC_ORANGE_HILITE)),
        _ => None,
    }
}

fn animated_spic_color(code: char, diag: f32, secs: f32) -> Option<Color> {
    let (base, hilite) = spic_palette(code)?;
    Some(blend_color(base, hilite, shine_opacity(diag, secs)).unwrap_or(base))
}

#[allow(clippy::too_many_arguments)]
fn paint_half_cell(
    buf: &mut Buffer,
    x: u16,
    y: u16,
    top: char,
    bottom: char,
    top_diag: f32,
    bottom_diag: f32,
    secs: f32,
    bg: Color,
) {
    let top = animated_spic_color(top, top_diag, secs);
    let bottom = animated_spic_color(bottom, bottom_diag, secs);
    let Some(cell) = buf.cell_mut((x, y)) else {
        return;
    };
    cell.reset();
    match (top, bottom) {
        (None, None) => {
            cell.set_char(' ').set_style(Style::default().bg(bg));
        }
        (Some(fg), None) => {
            cell.set_char('▀').set_style(Style::default().fg(fg).bg(bg));
        }
        (None, Some(fg)) => {
            cell.set_char('▄').set_style(Style::default().fg(fg).bg(bg));
        }
        (Some(fg), Some(cell_bg)) => {
            cell.set_char('▀')
                .set_style(Style::default().fg(fg).bg(cell_bg));
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn render_spic_text(
    buf: &mut Buffer,
    text: &str,
    x: u16,
    y: u16,
    row: u16,
    total_cols: u16,
    total_rows: u16,
    secs: f32,
) {
    use unicode_width::UnicodeWidthChar;

    let mut col = 0u16;
    for ch in text.chars() {
        let width = UnicodeWidthChar::width(ch).unwrap_or(0) as u16;
        if width == 0 {
            continue;
        }
        let diag = (col as f32 + total_rows.saturating_sub(1 + row) as f32)
            / (total_cols + total_rows).max(1) as f32;
        let color = blend_color(
            SPIC_TEXT_BLUE,
            SPIC_TEXT_BLUE_HILITE,
            shine_opacity(diag, secs),
        )
        .unwrap_or(SPIC_TEXT_BLUE);
        buf.set_span(
            x + col,
            y,
            &Span::styled(
                ch.to_string(),
                Style::default().fg(color).add_modifier(Modifier::BOLD),
            ),
            width,
        );
        col = col.saturating_add(width);
    }
}

fn iterm2_inline_logo_supported() -> bool {
    let context = crate::terminal::terminal_context();
    cfg!(target_os = "macos")
        && context.brand == crate::terminal::TerminalName::Iterm2
        && context.graphics_protocol_skip_reason().is_none()
        && !logo_hidden()
}

fn iterm2_image_rect(area: Rect) -> Option<Rect> {
    if area.width < SPIC_FULL_WIDTH || area.height < SPIC_FULL_HEIGHT {
        return None;
    }
    Some(Rect {
        x: area.x + area.width.saturating_sub(SPIC_FULL_WIDTH) / 2,
        y: area.y,
        width: SPIC_FULL_WIDTH,
        height: SPIC_FULL_HEIGHT,
    })
}

fn iterm_logo_placement_state() -> &'static Mutex<Option<ItermLogoPlacement>> {
    ITERM_LOGO_PLACEMENT.get_or_init(|| Mutex::new(None))
}

/// Clear the cached iTerm2 placement and report whether an image had been
/// emitted. The app uses this signal to clear the terminal before drawing a
/// non-welcome view, because OSC 1337 has no image-id/delete primitive.
pub fn reset_iterm2_inline_logo() -> bool {
    let mut placement = iterm_logo_placement_state()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    *placement = None;
    ITERM_LOGO_VISIBLE.swap(false, Ordering::AcqRel)
}

/// Replace the character fallback's SPIC rows with clean background cells so
/// the iTerm2 image does not reveal terminal glyphs underneath it.
pub fn prepare_iterm2_inline_logo_area(area: Rect, buf: &mut Buffer, theme: &Theme, allowed: bool) {
    if !allowed || !iterm2_inline_logo_supported() {
        return;
    }
    let Some(image_rect) = iterm2_image_rect(area) else {
        return;
    };
    for y in image_rect.top()..image_rect.bottom() {
        for x in image_rect.left()..image_rect.right() {
            if let Some(cell) = buf.cell_mut((x, y)) {
                cell.reset();
                cell.set_char(' ')
                    .set_style(Style::default().bg(theme.bg_base));
            }
        }
    }
}

/// Emit the user-provided transparent SPIC PNG once per placement. The
/// viewport dimensions are part of the cache key so a resize retransmits even
/// when the centered cell rectangle happens to stay numerically unchanged.
pub fn iterm2_inline_logo_post_flush(
    area: Rect,
    viewport: Rect,
    allowed: bool,
) -> Option<crate::terminal::overlay::PostFlush> {
    if !allowed || !iterm2_inline_logo_supported() {
        return None;
    }
    let image = iterm2_image_rect(area)?;
    let next = ItermLogoPlacement {
        image,
        viewport_width: viewport.width,
        viewport_height: viewport.height,
    };
    let mut placement = iterm_logo_placement_state()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if placement.as_ref() == Some(&next) {
        ITERM_LOGO_VISIBLE.store(true, Ordering::Release);
        return None;
    }
    *placement = Some(next);
    ITERM_LOGO_VISIBLE.store(true, Ordering::Release);

    // OSC 1337 advances the cursor through the occupied rows. Save/restore it
    // so the focused prompt keeps its cursor after the post-flush upload.
    let mut escapes = format!("\x1b7\x1b[{};{}H", image.y + 1, image.x + 1);
    escapes.push_str(&crate::terminal::image::render_iterm2_named_image(
        SPIC_ITERM_PNG,
        "dttn-spic-logo.png",
        image.width,
        image.height,
    ));
    escapes.push_str("\x1b8");
    Some(crate::terminal::overlay::PostFlush::plain(escapes))
}

/// Render the full 国家电投/SPIC mark above the existing DTTN wordmark.
fn render_spic_wordmark(area: Rect, buf: &mut Buffer, theme: &Theme) {
    if area.width < SPIC_FULL_WIDTH || area.height < SPIC_FULL_HEIGHT {
        return;
    }
    let start_x = area.x + area.width.saturating_sub(SPIC_FULL_WIDTH) / 2;
    let secs = anim_phase_secs();
    let pixel_rows = SPIC_MARK_PIXELS.len() as f32;
    let pixel_cols = SPIC_MARK_WIDTH as f32;

    for cell_row in 0..SPIC_FULL_HEIGHT {
        let top_row = cell_row as usize * 2;
        let bottom_row = top_row + 1;
        for col in 0..SPIC_MARK_WIDTH as usize {
            let top = SPIC_MARK_PIXELS[top_row].as_bytes()[col] as char;
            let bottom = SPIC_MARK_PIXELS[bottom_row].as_bytes()[col] as char;
            let top_diag =
                (col as f32 + (pixel_rows - 1.0 - top_row as f32)) / (pixel_cols + pixel_rows);
            let bottom_diag =
                (col as f32 + (pixel_rows - 1.0 - bottom_row as f32)) / (pixel_cols + pixel_rows);
            paint_half_cell(
                buf,
                start_x + col as u16,
                area.y + cell_row,
                top,
                bottom,
                top_diag,
                bottom_diag,
                secs,
                theme.bg_base,
            );
        }
    }

    let text_x = start_x + SPIC_MARK_WIDTH + 2;
    render_spic_text(
        buf,
        "国家电投",
        text_x,
        area.y,
        0,
        SPIC_FULL_WIDTH,
        SPIC_FULL_HEIGHT,
        secs,
    );
    render_spic_text(
        buf,
        "SPIC",
        text_x,
        area.y + 2,
        2,
        SPIC_FULL_WIDTH,
        SPIC_FULL_HEIGHT,
        secs,
    );
}

fn branded_line_count(logo: &str) -> u16 {
    SPIC_FULL_HEIGHT + SPIC_DTTN_GAP + count_lines(logo)
}

fn branded_visual_width(logo: &str) -> u16 {
    SPIC_FULL_WIDTH.max(visual_width(logo))
}

fn render_branded_into(area: Rect, buf: &mut Buffer, theme: &Theme, logo: &str) {
    if area.height == 0 {
        return;
    }
    render_spic_wordmark(
        Rect {
            height: SPIC_FULL_HEIGHT.min(area.height),
            ..area
        },
        buf,
        theme,
    );

    let dttn_y = area.y + SPIC_FULL_HEIGHT + SPIC_DTTN_GAP;
    let dttn_height = area.bottom().saturating_sub(dttn_y);
    if dttn_height > 0 {
        render_into(
            Rect {
                x: area.x,
                y: dttn_y,
                width: area.width,
                height: dttn_height,
            },
            buf,
            theme,
            logo,
        );
    }
}

fn render_into(area: Rect, buf: &mut Buffer, theme: &Theme, logo: &str) {
    let lines: Vec<&str> = non_empty_lines(logo).collect();
    let rows = lines.len().max(1) as f32;
    let cols = lines
        .iter()
        .map(|l| l.chars().count())
        .max()
        .unwrap_or(1)
        .max(1) as f32;
    let secs = anim_phase_secs();

    // Blend each glyph from the resting gray toward the bright text color by its
    // shine opacity, so a sheen sweeps across the braille art. Adjacent glyphs
    // that land on the same blended color share one Span to hold down the
    // per-frame allocation.
    let base = theme.gray;
    let hilite = theme.text_primary;
    let logo_lines: Vec<Line> = lines
        .iter()
        .enumerate()
        .map(|(row, line)| {
            let mut spans: Vec<Span> = Vec::new();
            let mut run = String::new();
            let mut run_color: Option<Color> = None;
            for (col, ch) in line.chars().enumerate() {
                // Sweep along the bottom-left → top-right diagonal: the
                // coordinate grows as col increases and row decreases.
                let diag = (col as f32 + (rows - 1.0 - row as f32)) / (cols + rows);
                let color = blend_color(base, hilite, shine_opacity(diag, secs)).unwrap_or(base);
                if run_color != Some(color) {
                    if let Some(prev) = run_color {
                        spans.push(Span::styled(
                            std::mem::take(&mut run),
                            Style::default().fg(prev),
                        ));
                    }
                    run_color = Some(color);
                }
                run.push(ch);
            }
            if let Some(prev) = run_color {
                spans.push(Span::styled(run, Style::default().fg(prev)));
            }
            Line::from(spans).alignment(Alignment::Center)
        })
        .collect();
    Paragraph::new(logo_lines).render(area, buf);
}

pub fn logo_line_count(window_height: u16) -> u16 {
    pick_logo(window_height).map_or(0, branded_line_count)
}

pub fn logo_visual_width(window_height: u16) -> u16 {
    pick_logo(window_height).map_or(24, branded_visual_width)
}

pub fn render_logo(area: Rect, buf: &mut Buffer, theme: &Theme, window_height: u16) {
    if let Some(logo) = pick_logo(window_height) {
        render_branded_into(area, buf, theme, logo);
    }
}

/// The hero box always shows the full logo: it is laid out beside the menu, so
/// it fits whenever the box does. These report and render that logo directly,
/// independent of the height-based [`pick_logo`] tiers used by the stacked
/// layout. When [`logo_hidden`], they report 0 and render nothing.
pub fn full_logo_line_count() -> u16 {
    full_logo_line_count_for(logo_hidden())
}

fn full_logo_line_count_for(hidden: bool) -> u16 {
    if hidden { 0 } else { branded_line_count(LOGO) }
}

pub fn full_logo_visual_width() -> u16 {
    full_logo_visual_width_for(logo_hidden())
}

fn full_logo_visual_width_for(hidden: bool) -> u16 {
    if hidden {
        0
    } else {
        branded_visual_width(LOGO)
    }
}

pub fn render_full_logo(area: Rect, buf: &mut Buffer, theme: &Theme) {
    if !logo_hidden() {
        render_branded_into(area, buf, theme, LOGO);
    }
}

/// Line count of the small logo used in minimal's committed welcome card
/// (0 on a legacy Windows console, where the braille art is suppressed).
pub fn compact_logo_line_count() -> u16 {
    if logo_hidden() {
        0
    } else {
        branded_line_count(LOGO_SMALL)
    }
}

/// Render the small braille logo (centered) into `area` for minimal's welcome
/// card. No-op when the logo is hidden.
pub fn render_compact_logo(area: Rect, buf: &mut Buffer, theme: &Theme) {
    if !logo_hidden() {
        render_branded_into(area, buf, theme, LOGO_SMALL);
    }
}

/// Width reserved for the compact interactive-footer brand. It disappears on
/// very narrow terminals so command hints remain usable.
pub fn spic_corner_logo_width(area_width: u16) -> u16 {
    if logo_hidden() || area_width < SPIC_CORNER_MIN_AREA_WIDTH {
        0
    } else {
        SPIC_CORNER_WIDTH
    }
}

/// Paint a one-row, right-aligned SPIC mark in the interaction footer using
/// the same wall-clock phase and shimmer curve as the welcome logos.
pub fn render_spic_corner_logo(area: Rect, buf: &mut Buffer, theme: &Theme) {
    let width = spic_corner_logo_width(area.width);
    if width == 0 || area.height == 0 {
        return;
    }
    let start_x = area.right().saturating_sub(width);
    let y = area.y;
    for x in start_x..area.right() {
        if let Some(cell) = buf.cell_mut((x, y)) {
            cell.reset();
            cell.set_char(' ')
                .set_style(Style::default().bg(theme.bg_base));
        }
    }

    let secs = anim_phase_secs();
    // Two terminal columns are approximately one cell-height wide, producing
    // a visually square icon. The upper half is red and the lower half green,
    // echoing the user's reference without squeezing the full ribbon mark into
    // a one-row footer or adding a glyph inside the square.
    for col in 0..SPIC_CORNER_MARK_WIDTH {
        let diag = col as f32 / SPIC_CORNER_MARK_WIDTH as f32;
        paint_half_cell(
            buf,
            start_x + col,
            y,
            'R',
            'G',
            diag,
            (diag + 0.12).min(1.0),
            secs,
            theme.bg_base,
        );
    }
    render_spic_text(
        buf,
        "国家电投 SPIC",
        start_x + SPIC_CORNER_MARK_WIDTH + SPIC_CORNER_GAP,
        y,
        0,
        SPIC_CORNER_WIDTH,
        1,
        secs,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn logo_sizes_by_height() {
        assert!(pick_logo_for(SMALL_LOGO_MIN_HEIGHT - 1, false).is_none());
        assert_eq!(
            pick_logo_for(SMALL_LOGO_MIN_HEIGHT, false),
            Some(LOGO_SMALL)
        );
        assert_eq!(
            pick_logo_for(FULL_LOGO_MIN_HEIGHT - 1, false),
            Some(LOGO_SMALL)
        );
        assert_eq!(pick_logo_for(FULL_LOGO_MIN_HEIGHT, false), Some(LOGO));
    }

    // The braille art has no legacy-safe stand-in, so every height tier must
    // collapse to no logo when the legacy-console flag is set.
    #[test]
    fn logo_hidden_on_legacy_console_at_every_height() {
        for h in [0, SMALL_LOGO_MIN_HEIGHT, FULL_LOGO_MIN_HEIGHT, u16::MAX] {
            assert!(pick_logo_for(h, true).is_none(), "height {h}");
        }
    }

    #[test]
    fn hero_box_always_uses_full_logo() {
        // The box renders the full logo regardless of height (it's laid out
        // beside the menu), and it's the large variant — never the small one.
        assert_eq!(full_logo_line_count_for(false), branded_line_count(LOGO));
        assert_eq!(
            full_logo_visual_width_for(false),
            branded_visual_width(LOGO)
        );
        assert!(full_logo_line_count_for(false) > branded_line_count(LOGO_SMALL));
        assert!(full_logo_visual_width_for(false) > visual_width(LOGO_SMALL));
    }

    #[test]
    fn full_logo_helpers_collapse_when_hidden() {
        assert_eq!(full_logo_line_count_for(true), 0);
        assert_eq!(full_logo_visual_width_for(true), 0);
    }

    #[test]
    fn compact_logo_line_count_matches_small_logo_when_visible() {
        // The minimal welcome card budgets exactly the small logo's rows. When
        // the logo isn't hidden, the count equals the small art's line count and
        // is strictly shorter than the full logo.
        if !logo_hidden() {
            assert_eq!(compact_logo_line_count(), branded_line_count(LOGO_SMALL));
            assert!(compact_logo_line_count() < branded_line_count(LOGO));
            assert!(compact_logo_line_count() > 0);
        } else {
            assert_eq!(compact_logo_line_count(), 0);
        }
    }

    #[test]
    fn shine_opacity_stays_in_unit_range() {
        let mut secs = 0.0;
        while secs < 10.0 {
            for i in 0..=20 {
                let diag = i as f32 / 20.0;
                let op = shine_opacity(diag, secs);
                assert!(
                    (0.0..=1.0).contains(&op),
                    "opacity {op} out of range at diag {diag}, secs {secs}"
                );
            }
            secs += 0.13;
        }
    }

    #[test]
    fn shine_band_sweeps_across() {
        // The brightest point along the diagonal advances left → right as the
        // sweep progresses through its active phase.
        let brightest = |secs: f32| -> f32 {
            (0..=100)
                .map(|i| i as f32 / 100.0)
                .max_by(|a, b| {
                    shine_opacity(*a, secs)
                        .partial_cmp(&shine_opacity(*b, secs))
                        .unwrap()
                })
                .unwrap()
        };
        let early = brightest(0.1);
        let mid = brightest(0.4);
        let late = brightest(0.7);
        assert!(early < mid, "early {early} should precede mid {mid}");
        assert!(mid < late, "mid {mid} should precede late {late}");
    }

    #[test]
    fn shine_rests_dim_between_sweeps() {
        // During the rest phase the band is parked off-screen, so an interior
        // glyph falls back to at most the gentle pulse — never full bright.
        let op = shine_opacity(0.5, 6.0); // secs % 4.0 = 2.0 → past SWEEP_FRAC, in the rest phase
        assert!(op < 0.2, "resting opacity {op} should stay dim");
    }

    #[test]
    fn spic_palette_uses_brand_and_bright_text_values() {
        assert_eq!(spic_palette('G').unwrap().0, Color::Rgb(0x50, 0xB5, 0x06));
        assert_eq!(spic_palette('g').unwrap().0, Color::Rgb(0x00, 0x6C, 0x34));
        assert_eq!(spic_palette('R').unwrap().0, Color::Rgb(0xFC, 0x00, 0x00));
        assert_eq!(SPIC_TEXT_BLUE, Color::Rgb(0x4B, 0x52, 0xCC));
    }

    #[test]
    fn iterm_logo_uses_fixed_twenty_by_four_cell_box() {
        let area = Rect::new(10, 5, 30, 12);
        assert_eq!(iterm2_image_rect(area), Some(Rect::new(15, 5, 20, 4)));
        assert_eq!(iterm2_image_rect(Rect::new(0, 0, 19, 4)), None);
        assert_eq!(iterm2_image_rect(Rect::new(0, 0, 20, 3)), None);
        assert!(SPIC_ITERM_PNG.starts_with(b"\x89PNG\r\n\x1a\n"));
    }
}
