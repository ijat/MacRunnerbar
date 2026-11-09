//! The popover's content: the app draws its own list (LuneUI has a flyout with fixed rows and stats
//! but no list of items that carry a link, so the jobs are laid out here from the toolkit's text,
//! status dot and buttons).

use lune_ui::paint::Rect;
use lune_ui::text::{Spec, Style};
use lune_ui::ui::{Sense, Ui};
use lune_ui::widgets::button::ButtonKind;
use lune_ui::widgets::tone::Tone;

use crate::view::{Mood, PopoverView};

/// The window's width, logical px.
pub(crate) const WIDTH: f32 = 380.0;
const PAD: f32 = 16.0;
const HEADER_H: f32 = 56.0;
const STATS_H: f32 = 30.0;
const SECTION_H: f32 = 28.0;
const ROW_H: f32 = 50.0;
const EMPTY_H: f32 = 40.0;
const WAIT_H: f32 = 22.0;
const MORE_H: f32 = 22.0;
const ERROR_H: f32 = 44.0;
const FOOTER_H: f32 = 56.0;
const RADIUS: f32 = 9.0;

/// What a click in the popover asks for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PopoverAction {
    /// Open a run in the browser (a validated github.com URL).
    Open(String),
    /// Pause or resume.
    TogglePause,
    Preferences,
    Quit,
}

/// The window's height for `v`, logical px: what the layout below needs, no text measuring.
pub(crate) fn height(v: &PopoverView) -> f32 {
    let mut h = HEADER_H + STATS_H;
    h += SECTION_H;
    h += if v.jobs.is_empty() {
        EMPTY_H
    } else {
        v.jobs.len() as f32 * ROW_H
    };
    if v.more_jobs > 0 {
        h += MORE_H;
    }
    if !v.waiting.is_empty() {
        h += SECTION_H
            + v.waiting.len() as f32 * WAIT_H
            + if v.more_waiting > 0 { MORE_H } else { 0.0 };
    }
    if v.last_error.is_some() {
        h += ERROR_H;
    }
    h + FOOTER_H
}

fn tone(m: Mood) -> Tone {
    match m {
        Mood::Ok => Tone::Ok,
        Mood::Busy => Tone::Data,
        Mood::Warn => Tone::Warn,
        Mood::Paused => Tone::Paused,
    }
}

/// Draws `text` with its top left at (`x`, `y`), elided to `max_w`; returns its width.
fn line(ui: &mut Ui<'_>, text: &str, style: Style, x: f32, y: f32, max_w: f32) -> f32 {
    let p = ui.painter();
    let spec = p.elide(&Spec::plain(text, style), max_w);
    p.text(&spec, x, y).0
}

/// `text` with its right edge at `right`.
fn right_aligned(ui: &mut Ui<'_>, text: &str, style: Style, right: f32, y: f32) {
    let p = ui.painter();
    let spec = Spec::plain(text, style);
    let w = p.measure(&spec).0;
    p.text(&spec, right - w, y);
}

/// Draws the popover over `area` (the whole window) and returns what was clicked.
pub(crate) fn draw(ui: &mut Ui<'_>, area: Rect, v: &PopoverView) -> Option<PopoverAction> {
    let t = *ui.theme();
    let mut action = None;
    {
        let p = ui.painter();
        p.fill(area, RADIUS, t.menu_glass);
        p.ring(area, RADIUS, 1.0, t.menu_popup_border, true);
    }
    let (x0, x1) = (area.x + PAD, area.right() - PAD);
    let inner_w = x1 - x0;
    let mut y = area.y;

    // Header: the name and the state beside a status dot.
    let title = Style::ui(14.0, t.fg_bright).medium();
    line(ui, &v.title, title, x0, y + 14.0, inner_w);
    ui.status_dot("state", (x0, y + 34.0), tone(v.mood), &v.state_text);
    y += HEADER_H;

    // Stats: runners n/max.
    let small = Style::ui(11.5, t.fg_tertiary);
    let value = Style::ui(12.0, t.fg_secondary).medium();
    line(ui, "Runners", small, x0, y + 6.0, 120.0);
    right_aligned(ui, &v.runners, value, x1, y + 5.0);
    ui.painter().fill(
        Rect::new(x0, y + STATS_H - 1.0, inner_w, 1.0),
        0.0,
        t.menu_separator,
    );
    y += STATS_H;

    // Running jobs.
    line(
        ui,
        "RUNNING JOBS",
        Style::ui(10.5, t.fg_tertiary).medium(),
        x0,
        y + 10.0,
        inner_w,
    );
    y += SECTION_H;
    if v.jobs.is_empty() {
        line(
            ui,
            "Nothing running. Runners start when a job is queued.",
            small,
            x0,
            y + 10.0,
            inner_w,
        );
        y += EMPTY_H;
    }
    for (i, job) in v.jobs.iter().enumerate() {
        let row = Rect::new(area.x + 6.0, y, area.w - 12.0, ROW_H - 2.0);
        let sense = if job.url.is_some() {
            Sense::CLICK
        } else {
            Sense::HOVER
        };
        let resp = ui.interact(ui.id(("job", i)), row, sense);
        if resp.hovered {
            ui.painter().fill(row, 6.0, t.menu_item_hover);
        }
        let elapsed_w = 64.0;
        line(
            ui,
            &job.repo,
            Style::ui(13.0, t.fg_primary).medium(),
            x0,
            y + 7.0,
            inner_w - elapsed_w - 8.0,
        );
        line(
            ui,
            &job.detail,
            small,
            x0,
            y + 26.0,
            inner_w - elapsed_w - 8.0,
        );
        right_aligned(
            ui,
            &job.elapsed,
            Style::ui(12.0, t.fg_secondary),
            x1,
            y + 8.0,
        );
        if resp.clicked
            && let Some(url) = &job.url
        {
            action = Some(PopoverAction::Open(url.clone()));
        }
        y += ROW_H;
    }
    if v.more_jobs > 0 {
        line(
            ui,
            &format!("+{} more", v.more_jobs),
            small,
            x0,
            y + 2.0,
            inner_w,
        );
        y += MORE_H;
    }

    // Jobs held back by a cap.
    if !v.waiting.is_empty() {
        line(
            ui,
            "WAITING",
            Style::ui(10.5, t.fg_tertiary).medium(),
            x0,
            y + 10.0,
            inner_w,
        );
        y += SECTION_H;
        for w in &v.waiting {
            line(ui, w, Style::ui(12.0, t.fg_secondary), x0, y + 2.0, inner_w);
            y += WAIT_H;
        }
        if v.more_waiting > 0 {
            line(
                ui,
                &format!("+{} more", v.more_waiting),
                small,
                x0,
                y + 2.0,
                inner_w,
            );
            y += MORE_H;
        }
    }

    // The last error.
    if let Some(e) = &v.last_error {
        let box_r = Rect::new(x0, y + 4.0, inner_w, ERROR_H - 8.0);
        ui.painter().fill(box_r, 6.0, [0x70, 0x20, 0x28, 0x66]);
        line(
            ui,
            e,
            Style::ui(11.5, t.fg_primary),
            x0 + 8.0,
            y + 11.0,
            inner_w - 16.0,
        );
        y += ERROR_H;
    }

    // Footer: Pause or Resume, Preferences, Quit.
    ui.painter()
        .fill(Rect::new(x0, y, inner_w, 1.0), 0.0, t.menu_separator);
    let by = y + 12.0;
    let pause_label = if v.paused { "Resume" } else { "Pause" };
    let widths = [84.0, 112.0, 64.0];
    let gap = 8.0;
    let mut bx = x0;
    let items = [
        (
            pause_label,
            ButtonKind::Secondary,
            PopoverAction::TogglePause,
        ),
        (
            "Preferences",
            ButtonKind::Secondary,
            PopoverAction::Preferences,
        ),
        ("Quit", ButtonKind::Ghost, PopoverAction::Quit),
    ];
    for ((label, kind, act), w) in items.into_iter().zip(widths) {
        let r = ui.button(("footer", label), Rect::new(bx, by, w, 32.0), label, kind);
        if r.clicked {
            action = Some(act);
        }
        bx += w + gap;
    }
    action
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::view::{JobRow, Mood};
    use lune_ui::input::PointerButton;
    use lune_ui::testing::UiHarness;

    fn view(jobs: usize, waiting: usize, error: bool) -> PopoverView {
        PopoverView {
            title: "MacRunnerbar".into(),
            state_text: if jobs > 0 {
                format!("{jobs} running")
            } else {
                "Idle".into()
            },
            mood: if jobs > 0 { Mood::Busy } else { Mood::Ok },
            runners: format!("{jobs}/2"),
            paused: false,
            jobs: (0..jobs)
                .map(|i| JobRow {
                    repo: format!("owner/repo{i}"),
                    detail: "CI / build".into(),
                    elapsed: "1m 05s".into(),
                    url: (i % 2 == 0)
                        .then(|| format!("https://github.com/owner/repo{i}/actions/runs/{i}")),
                })
                .collect(),
            more_jobs: 0,
            waiting: (0..waiting)
                .map(|i| format!("1 job waiting on the runner cap: o/w{i}"))
                .collect(),
            more_waiting: 0,
            last_error: error.then(|| "o/r: HTTP 500".to_string()),
        }
    }

    /// Runs one frame at `size`, drawing `v`, with the input scripted so far.
    fn frame(h: &mut UiHarness, v: &PopoverView) -> Option<PopoverAction> {
        let mut out = None;
        h.frame(|ui| {
            let area = ui.painter().window;
            out = draw(ui, area, v);
        });
        out
    }

    fn click(h: &mut UiHarness, v: &PopoverView, x: f32, y: f32) -> Option<PopoverAction> {
        h.input.pointer_to(x, y);
        frame(h, v);
        h.input.press(PointerButton::Primary);
        frame(h, v);
        h.input.release(PointerButton::Primary);
        frame(h, v)
    }

    #[test]
    fn the_window_height_grows_with_the_content() {
        let empty = height(&view(0, 0, false));
        assert!(height(&view(2, 0, false)) > empty);
        assert!(height(&view(2, 2, false)) > height(&view(2, 0, false)));
        assert!(height(&view(1, 0, true)) > height(&view(1, 0, false)));
        assert!(
            height(&view(8, 3, true)) < 900.0,
            "the tallest popover still fits a laptop screen"
        );
    }

    #[test]
    fn a_job_with_a_link_opens_it_and_one_without_does_nothing() {
        let v = view(2, 0, false);
        let mut h = UiHarness::new((WIDTH, height(&v)));
        let first_row_y = HEADER_H + STATS_H + SECTION_H + 10.0;
        let second_row_y = first_row_y + ROW_H;
        assert_eq!(
            click(&mut h, &v, 100.0, first_row_y),
            Some(PopoverAction::Open(
                "https://github.com/owner/repo0/actions/runs/0".into()
            ))
        );
        let mut h = UiHarness::new((WIDTH, height(&v)));
        assert_eq!(
            click(&mut h, &v, 100.0, second_row_y),
            None,
            "no URL: not a link"
        );
    }

    #[test]
    fn the_footer_buttons_report_pause_preferences_and_quit() {
        let v = view(1, 1, true);
        let total = height(&v);
        let by = total - FOOTER_H + 12.0 + 16.0;
        for (x, expected) in [
            (PAD + 40.0, PopoverAction::TogglePause),
            (PAD + 84.0 + 8.0 + 50.0, PopoverAction::Preferences),
            (PAD + 84.0 + 8.0 + 112.0 + 8.0 + 30.0, PopoverAction::Quit),
        ] {
            let mut h = UiHarness::new((WIDTH, total));
            assert_eq!(
                click(&mut h, &v, x, by),
                Some(expected.clone()),
                "{expected:?}"
            );
        }
    }

    #[test]
    fn nothing_is_asked_without_a_click() {
        let v = view(3, 2, true);
        let mut h = UiHarness::new((WIDTH, height(&v)));
        h.input.pointer_to(120.0, 120.0);
        assert_eq!(frame(&mut h, &v), None);
        assert_eq!(frame(&mut h, &v), None);
    }

    #[test]
    fn a_paused_view_offers_resume() {
        // The label is the only difference; the action is the same toggle.
        let mut v = view(0, 0, false);
        v.paused = true;
        let total = height(&v);
        let mut h = UiHarness::new((WIDTH, total));
        assert_eq!(
            click(&mut h, &v, PAD + 40.0, total - FOOTER_H + 28.0),
            Some(PopoverAction::TogglePause)
        );
    }
}
