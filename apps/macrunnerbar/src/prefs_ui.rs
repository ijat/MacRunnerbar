//! The Preferences window's content.

use lune_ui::paint::Rect;
use lune_ui::text::{Spec, Style};
use lune_ui::ui::Ui;
use lune_ui::widgets::button::ButtonKind;
use lune_ui::widgets::field::{FieldOptions, Variant};
use lune_ui::widgets::stepper::StepRange;
use macrunnerbar_core::{NotifyMode, Secret, Settings};

use crate::prefs::{CAP_RANGE, POLL_RANGE, PrefsForm};

/// The window's logical size.
pub(crate) const SIZE: (f32, f32) = (600.0, 690.0);
const PAD: f32 = 28.0;
const LABEL_W: f32 = 150.0;
/// The macOS title bar is transparent and full size: content starts below it.
const TOP: f32 = 44.0;

/// What a click in Preferences asks for.
pub(crate) enum PrefsAction {
    /// Store this token in the Keychain.
    SaveToken(Secret<String>),
    /// Remove the stored token.
    RemoveToken,
    /// Persist these settings (and apply launch at login).
    Save(Settings),
    /// Close the window.
    Close,
}

impl std::fmt::Debug for PrefsAction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::SaveToken(_) => f.write_str("SaveToken([redacted])"),
            Self::RemoveToken => f.write_str("RemoveToken"),
            Self::Save(s) => f.debug_tuple("Save").field(s).finish(),
            Self::Close => f.write_str("Close"),
        }
    }
}

fn text(ui: &mut Ui<'_>, s: &str, style: Style, x: f32, y: f32) {
    ui.painter().text(&Spec::plain(s, style), x, y);
}

fn field(ui: &mut Ui<'_>, key: &str, rect: Rect, value: &mut String, placeholder: &str) {
    let opts = FieldOptions {
        placeholder,
        ..FieldOptions::default()
    };
    ui.text_field_with(key, rect, value, &opts);
}

fn stepper(
    ui: &mut Ui<'_>,
    key: &str,
    rect: Rect,
    value: &mut u32,
    range: (u32, u32),
    step: u32,
    name: &str,
) {
    let r = StepRange {
        min: f64::from(range.0),
        max: f64::from(range.1),
        step: f64::from(step),
    };
    let resp = ui.stepper(key, rect, *value, r, name);
    if let Some(n) = resp.step {
        let moved = i64::from(*value) + i64::from(n) * i64::from(step);
        *value = moved.clamp(i64::from(range.0), i64::from(range.1)) as u32;
    }
    if let Some(v) = resp.set {
        *value = (v.round() as i64).clamp(i64::from(range.0), i64::from(range.1)) as u32;
    }
}

/// Draws the form over `area`. `base` is the saved settings (fields the form does not show are kept).
pub(crate) fn draw(
    ui: &mut Ui<'_>,
    area: Rect,
    form: &mut PrefsForm,
    base: &Settings,
) -> Option<PrefsAction> {
    let t = *ui.theme();
    let mut action = None;
    let heading = Style::ui(11.0, t.fg_tertiary).medium();
    let label = Style::ui(13.0, t.fg_primary);
    let hint = Style::ui(11.5, t.fg_tertiary);
    let x0 = area.x + PAD;
    let cx = x0 + LABEL_W;
    let cw = area.w - 2.0 * PAD - LABEL_W;
    let mut y = area.y + TOP;

    text(
        ui,
        "Preferences",
        Style::ui(18.0, t.fg_bright).medium(),
        x0,
        y,
    );
    y += 38.0;

    // GitHub token.
    text(ui, "GITHUB", heading, x0, y);
    y += 22.0;
    text(ui, "Token", label, x0, y + 6.0);
    let opts = FieldOptions {
        placeholder: "Paste a token (classic, repo scope)",
        variant: Variant::Password,
        ..FieldOptions::default()
    };
    ui.text_field_with(
        "token",
        Rect::new(cx, y, cw - 168.0, 32.0),
        &mut form.token,
        &opts,
    );
    if ui
        .button(
            "save-token",
            Rect::new(cx + cw - 160.0, y, 76.0, 32.0),
            "Save",
            ButtonKind::Secondary,
        )
        .clicked
    {
        action = Some(match form.take_token() {
            Some(secret) => PrefsAction::SaveToken(secret),
            None => {
                form.message = Some((false, "Type or paste a token first.".into()));
                return action;
            }
        });
    }
    if ui
        .button(
            "remove-token",
            Rect::new(cx + cw - 76.0, y, 76.0, 32.0),
            "Remove",
            ButtonKind::Ghost,
        )
        .clicked
    {
        action = Some(PrefsAction::RemoveToken);
    }
    y += 38.0;
    let stored = if form.token_stored {
        "A token is stored in the macOS Keychain."
    } else {
        "No token yet: nothing runs until one is saved."
    };
    text(ui, stored, hint, cx, y);
    y += 30.0;

    // Repositories.
    text(ui, "REPOSITORIES", heading, x0, y);
    y += 22.0;
    for (name, key, value, ph) in [
        (
            "Only these",
            "include",
            &mut form.include,
            "owner/repo, ... (empty: all you can administer)",
        ),
        (
            "Never these",
            "exclude",
            &mut form.exclude,
            "owner/repo, ...",
        ),
        (
            "Public repos to serve",
            "public",
            &mut form.public_allow,
            "owner/repo, ... (public repos are skipped by default)",
        ),
    ] {
        text(ui, name, label, x0, y + 6.0);
        field(ui, key, Rect::new(cx, y, cw, 32.0), value, ph);
        y += 40.0;
    }
    text(
        ui,
        "Fork pull requests on a public repo run untrusted code as you.",
        hint,
        cx,
        y - 4.0,
    );
    y += 26.0;

    // Runners.
    text(ui, "RUNNERS", heading, x0, y);
    y += 22.0;
    text(ui, "Per-owner cap", label, x0, y + 6.0);
    stepper(
        ui,
        "per-owner",
        Rect::new(cx, y, 124.0, 32.0),
        &mut form.per_owner,
        CAP_RANGE,
        1,
        "Per-owner cap",
    );
    y += 40.0;
    text(ui, "Global cap", label, x0, y + 6.0);
    stepper(
        ui,
        "global",
        Rect::new(cx, y, 124.0, 32.0),
        &mut form.global,
        CAP_RANGE,
        1,
        "Global cap",
    );
    y += 40.0;
    text(ui, "Poll interval (s)", label, x0, y + 6.0);
    stepper(
        ui,
        "poll",
        Rect::new(cx, y, 124.0, 32.0),
        &mut form.poll,
        POLL_RANGE,
        5,
        "Poll interval in seconds",
    );
    y += 40.0;
    text(ui, "Labels", label, x0, y + 6.0);
    field(
        ui,
        "labels",
        Rect::new(cx, y, cw, 32.0),
        &mut form.labels,
        "self-hosted, macos, arm64",
    );
    y += 44.0;

    // Notifications and login.
    text(ui, "BEHAVIOUR", heading, x0, y);
    y += 22.0;
    text(ui, "Notifications", label, x0, y + 6.0);
    let mut bx = cx;
    for (name, mode) in [
        ("Off", NotifyMode::Off),
        ("Failures only", NotifyMode::FailuresOnly),
        ("All", NotifyMode::All),
    ] {
        let w = if mode == NotifyMode::FailuresOnly {
            120.0
        } else {
            64.0
        };
        let kind = if form.notify == mode {
            ButtonKind::Primary
        } else {
            ButtonKind::Secondary
        };
        if ui
            .button(("notify", name), Rect::new(bx, y, w, 32.0), name, kind)
            .clicked
        {
            form.notify = mode;
        }
        bx += w + 8.0;
    }
    y += 42.0;
    // A toggle, not a checkbox: the checkbox's tick is a glyph from an icon font this app does not ship.
    ui.toggle(
        "login",
        Rect::new(cx, y, 40.0, 22.0),
        &mut form.launch_at_login,
        "Start MacRunnerbar at login",
    );
    text(ui, "Start MacRunnerbar at login", label, cx + 52.0, y + 2.0);
    y += 40.0;

    // Footer: the outcome of the last action, then Save and Close.
    if let Some((ok, msg)) = &form.message {
        let c = if *ok { t.toast_success } else { t.toast_error };
        text(ui, msg, Style::ui(12.0, c), x0, y + 8.0);
    }
    let by = area.bottom() - 56.0;
    if ui
        .button(
            "save",
            Rect::new(area.right() - PAD - 168.0, by, 80.0, 32.0),
            "Save",
            ButtonKind::Primary,
        )
        .clicked
    {
        match form.to_settings(base) {
            Ok(s) => action = Some(PrefsAction::Save(s)),
            Err(e) => form.message = Some((false, e)),
        }
    }
    if ui
        .button(
            "close",
            Rect::new(area.right() - PAD - 80.0, by, 80.0, 32.0),
            "Close",
            ButtonKind::Secondary,
        )
        .clicked
    {
        action = Some(PrefsAction::Close);
    }
    action
}

#[cfg(test)]
mod tests {
    use super::*;
    use lune_ui::input::PointerButton;
    use lune_ui::testing::UiHarness;

    fn frame(h: &mut UiHarness, form: &mut PrefsForm, base: &Settings) -> Option<PrefsAction> {
        let mut out = None;
        h.frame(|ui| {
            let area = ui.painter().window;
            out = draw(ui, area, form, base);
        });
        out
    }

    fn click(
        h: &mut UiHarness,
        form: &mut PrefsForm,
        base: &Settings,
        x: f32,
        y: f32,
    ) -> Option<PrefsAction> {
        h.input.pointer_to(x, y);
        frame(h, form, base);
        h.input.press(PointerButton::Primary);
        frame(h, form, base);
        h.input.release(PointerButton::Primary);
        frame(h, form, base)
    }

    const SAVE: (f32, f32) = (SIZE.0 - PAD - 168.0 + 40.0, SIZE.1 - 56.0 + 16.0);
    const CLOSE: (f32, f32) = (SIZE.0 - PAD - 80.0 + 40.0, SIZE.1 - 56.0 + 16.0);

    #[test]
    fn save_returns_the_validated_settings_and_close_closes() {
        let base = Settings::default();
        let mut form = PrefsForm::from_settings(&base, false, false);
        form.global = 5;
        let mut h = UiHarness::new(SIZE);
        let act = click(&mut h, &mut form, &base, SAVE.0, SAVE.1);
        match act {
            Some(PrefsAction::Save(s)) => assert_eq!(s.global_max, 5),
            other => panic!("expected Save, got {other:?}"),
        }
        let mut h = UiHarness::new(SIZE);
        assert!(matches!(
            click(&mut h, &mut form, &base, CLOSE.0, CLOSE.1),
            Some(PrefsAction::Close)
        ));
    }

    #[test]
    fn invalid_input_shows_a_message_instead_of_saving() {
        let base = Settings::default();
        let mut form = PrefsForm::from_settings(&base, false, false);
        form.include = "not-a-repo".into();
        let mut h = UiHarness::new(SIZE);
        assert!(click(&mut h, &mut form, &base, SAVE.0, SAVE.1).is_none());
        let (ok, msg) = form.message.clone().expect("a message");
        assert!(!ok && msg.starts_with("Include:"), "{msg}");
    }

    #[test]
    fn the_token_button_hands_over_the_typed_token_as_a_secret_and_clears_the_field() {
        let base = Settings::default();
        let mut form = PrefsForm::from_settings(&base, false, false);
        let typed = concat!("gh", "p_TYPEDINPREFERENCES0123456789abcdefgh");
        form.token = typed.to_string();
        let mut h = UiHarness::new(SIZE);
        let (x, y) = (
            PAD + LABEL_W + (SIZE.0 - 2.0 * PAD - LABEL_W) - 160.0 + 38.0,
            TOP + 38.0 + 22.0 + 16.0,
        );
        match click(&mut h, &mut form, &base, x, y) {
            Some(PrefsAction::SaveToken(s)) => {
                assert_eq!(s.expose(), typed);
                assert!(!format!("{:?}", PrefsAction::SaveToken(s)).contains(typed));
            }
            other => panic!("expected SaveToken, got {other:?}"),
        }
        assert!(form.token.is_empty(), "the field was cleared");
        // With nothing typed the button only complains.
        let mut h = UiHarness::new(SIZE);
        assert!(click(&mut h, &mut form, &base, x, y).is_none());
        assert!(form.message.as_ref().is_some_and(|(ok, _)| !ok));
    }

    #[test]
    fn the_notification_buttons_pick_a_mode() {
        let base = Settings::default();
        let mut form = PrefsForm::from_settings(&base, false, false);
        assert_eq!(form.notify, NotifyMode::FailuresOnly);
        // Find the row: the BEHAVIOUR section; click "All" (third button).
        let mut h = UiHarness::new(SIZE);
        let y = notify_row_y() + 16.0;
        let x_all = PAD + LABEL_W + 64.0 + 8.0 + 120.0 + 8.0 + 30.0;
        click(&mut h, &mut form, &base, x_all, y);
        assert_eq!(form.notify, NotifyMode::All);
    }

    fn notify_row_y() -> f32 {
        // Mirrors the layout above: title, GITHUB, REPOSITORIES, RUNNERS (4 rows), BEHAVIOUR.
        let mut y = TOP + 38.0;
        y += 22.0 + 38.0 + 30.0; // GITHUB
        y += 22.0 + 3.0 * 40.0 + 26.0; // REPOSITORIES
        y += 22.0 + 3.0 * 40.0 + 44.0; // RUNNERS
        y + 22.0 // BEHAVIOUR heading
    }

    #[test]
    fn the_login_toggle_flips() {
        let base = Settings::default();
        let mut form = PrefsForm::from_settings(&base, false, false);
        let mut h = UiHarness::new(SIZE);
        let y = notify_row_y() + 42.0 + 12.0;
        click(&mut h, &mut form, &base, PAD + LABEL_W + 20.0, y);
        assert!(form.launch_at_login);
    }
}
