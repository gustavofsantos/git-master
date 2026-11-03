//! The Threads settings dialog (T7): capture, the largest stored result, the
//! built-in redaction patterns and the user's own, the sensitive paths, and a test
//! of the draft on a sample text. The rules are pure and tested here: the draft
//! is read from a `ThreadsConfig` and written back to one, the problems are the
//! ones `Redactor::new` reports (plus a group that is not a number), and the
//! numbers and paths are parsed here. The window side is the `impl StatusView` at
//! the bottom. A save writes the `gitmaster.threads.*` keys of `~/.gitconfig` in the background
//! and leaves a repository's own overrides alone (`app_settings`).

use gm_core::threads::config::{RedactConfig, ThreadsConfig, UserPattern};
use gm_core::threads::redact::{Hit, PatternProblem, Redacted, Redactor, builtin_names};
use gpui::{
    App, Context, Div, Entity, FontWeight, MouseButton, MouseDownEvent, Stateful, Subscription,
    Window, div, prelude::*, px,
};

use crate::icon::{self, Icon};
use crate::text_area::{TextArea, TextAreaEvent};
use crate::theme;
use crate::view::StatusView;

/// The smallest `max_result_bytes` the dialog accepts.
pub(crate) const MIN_RESULT_BYTES: usize = 1024;
/// The result of the test while the draft has problems.
pub(crate) const PAUSED: &str = "Capture is paused until the patterns are fixed.";
/// The warning above the buttons while the draft has problems.
pub(crate) const WARNING: &str = "Capture is paused in every repository until these patterns are fixed. Press Save again to save them anyway.";
/// The result of the test when the sample has no secret.
pub(crate) const NO_HITS: &str = "No secret found in the sample.";
/// The result of the test before a sample is typed.
const EMPTY_SAMPLE: &str = "Type sample text above to see what would be redacted.";
/// The notice after a save.
const SAVED: &str = "Threads settings saved";

const WIDTH: f32 = 620.;
/// The dialog hangs this far below the top of the window.
const TOP: f32 = 72.;
/// The height of the scrolling part of the dialog, so the buttons stay in view.
const BODY_HEIGHT: f32 = 520.;

/// One user pattern as the dialog edits it. The group is text, so a value that is
/// not a number can be shown and flagged; a blank group is the whole match.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct PatternRow {
    pub(crate) name: String,
    pub(crate) regex: String,
    pub(crate) group: String,
}

/// The settings as the dialog holds them before they are saved.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ThreadsSettingsDraft {
    pub(crate) capture: bool,
    /// The result size as typed.
    pub(crate) max_result_bytes: String,
    /// The group switch of the built-in patterns.
    pub(crate) builtin: bool,
    /// Every name in this list is off, including names this build does not know.
    pub(crate) disabled_builtins: Vec<String>,
    pub(crate) patterns: Vec<PatternRow>,
    /// The sensitive paths, one glob per line.
    pub(crate) paths: String,
}

/// What the test shows for a sample: the problems of the draft, and the redacted
/// sample when there are none.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Evaluation {
    pub(crate) problems: Vec<PatternProblem>,
    pub(crate) redacted: Option<Redacted>,
}

impl ThreadsSettingsDraft {
    /// The draft that shows `config`.
    #[must_use]
    pub(crate) fn from_config(config: &ThreadsConfig) -> Self {
        Self {
            capture: config.capture,
            max_result_bytes: config.max_result_bytes.to_string(),
            builtin: config.redact.builtin,
            disabled_builtins: config.redact.disabled_builtins.clone(),
            patterns: config
                .redact
                .patterns
                .iter()
                .map(|pattern| PatternRow {
                    name: pattern.name.clone(),
                    regex: pattern.regex.clone(),
                    group: pattern.group.to_string(),
                })
                .collect(),
            paths: config.redact.paths.join("\n"),
        }
    }

    /// The redaction part of the config this draft saves. Names are trimmed, paths are
    /// normalized, and a group that is not a number is saved as `0` (the problems say so).
    #[must_use]
    pub(crate) fn redact_config(&self) -> RedactConfig {
        RedactConfig {
            builtin: self.builtin,
            disabled_builtins: self.disabled_builtins.clone(),
            patterns: self
                .patterns
                .iter()
                .map(|row| UserPattern {
                    name: row.name.trim().to_owned(),
                    regex: row.regex.clone(),
                    group: parse_group(&row.group).unwrap_or(0),
                })
                .collect(),
            paths: normalize_paths(&self.paths),
        }
    }

    /// The config this draft saves. The result size is the only field that can refuse.
    pub(crate) fn to_config(&self) -> Result<ThreadsConfig, String> {
        Ok(ThreadsConfig {
            capture: self.capture,
            max_result_bytes: parse_result_bytes(&self.max_result_bytes)?,
            redact: self.redact_config(),
        })
    }

    /// The problems of the draft's patterns and paths, and the redacted `sample` when
    /// there are none. With a problem, capture is paused, so the sample is not redacted.
    #[must_use]
    pub(crate) fn evaluate(&self, sample: &str) -> Evaluation {
        let mut problems: Vec<PatternProblem> = self
            .patterns
            .iter()
            .filter(|row| parse_group(&row.group).is_none())
            .map(|row| PatternProblem {
                name: row.name.trim().to_owned(),
                message: "the group must be a whole number, 0 or more".to_owned(),
            })
            .collect();
        match Redactor::new(&self.redact_config()) {
            Ok(redactor) if problems.is_empty() => Evaluation {
                problems,
                redacted: Some(redactor.redact(sample)),
            },
            Ok(_) => Evaluation {
                problems,
                redacted: None,
            },
            Err(found) => {
                problems.extend(found);
                Evaluation {
                    problems,
                    redacted: None,
                }
            }
        }
    }
}

/// Whether the built-in pattern `name` is on, given the names that are off.
#[must_use]
pub(crate) fn builtin_on(disabled: &[String], name: &str) -> bool {
    !disabled.iter().any(|off| off == name)
}

/// Toggles `name` in `list`: removed when present, appended when not.
pub(crate) fn toggle_name(list: &mut Vec<String>, name: &str) {
    if let Some(index) = list.iter().position(|item| item == name) {
        list.remove(index);
    } else {
        list.push(name.to_owned());
    }
}

/// The paths of the sensitive-path field: one glob per line, trimmed, blank lines dropped.
#[must_use]
pub(crate) fn normalize_paths(text: &str) -> Vec<String> {
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_owned)
        .collect()
}

/// The result size the field holds: a whole number of bytes, at least `MIN_RESULT_BYTES`.
pub(crate) fn parse_result_bytes(text: &str) -> Result<usize, String> {
    let value: usize = text
        .trim()
        .parse()
        .map_err(|_| "Result size must be a whole number of bytes.".to_owned())?;
    if value < MIN_RESULT_BYTES {
        Err(format!(
            "Result size must be at least {MIN_RESULT_BYTES} bytes."
        ))
    } else {
        Ok(value)
    }
}

/// The group a row holds: blank is `0`, the whole match; `None` when it is not a whole number.
fn parse_group(text: &str) -> Option<usize> {
    let text = text.trim();
    if text.is_empty() {
        Some(0)
    } else {
        text.parse().ok()
    }
}

/// The problems to show on the row called `name`, joined, or `None` when it has none.
#[must_use]
pub(crate) fn row_message(problems: &[PatternProblem], name: &str) -> Option<String> {
    let messages: Vec<&str> = problems
        .iter()
        .filter(|problem| problem.name == name.trim())
        .map(|problem| problem.message.as_str())
        .collect();
    (!messages.is_empty()).then(|| messages.join(" "))
}

/// The lines of the test that list the hits: `name ×count`.
#[must_use]
pub(crate) fn hit_lines(hits: &[Hit]) -> Vec<String> {
    hits.iter()
        .map(|hit| format!("{} ×{}", hit.pattern, hit.count))
        .collect()
}

/// Whether Save must first show the warning: the draft has problems and the user has not
/// pressed Save again since they appeared.
#[must_use]
pub(crate) fn needs_confirmation(problems: &[PatternProblem], confirmed: bool) -> bool {
    !problems.is_empty() && !confirmed
}

/// One user pattern's inputs while the dialog is open. The subscriptions live as long as
/// the row, so a removed row stops reporting edits.
pub(crate) struct PatternInputs {
    name: Entity<TextArea>,
    regex: Entity<TextArea>,
    group: Entity<TextArea>,
    _subscriptions: Vec<Subscription>,
}

/// The dialog while it is open. The fields hold the text the user types, the
/// checkboxes hold their state, and `evaluation` is the test of the last draft seen.
pub(crate) struct ThreadsSettingsDialog {
    capture: bool,
    builtin: bool,
    disabled_builtins: Vec<String>,
    max_bytes: Entity<TextArea>,
    rows: Vec<PatternInputs>,
    paths: Entity<TextArea>,
    sample: Entity<TextArea>,
    pub(crate) evaluation: Evaluation,
    /// The draft and sample the last evaluation saw. The fields notify on every caret
    /// blink, so an edit that changes neither is not evaluated again.
    seen_draft: ThreadsSettingsDraft,
    seen_sample: String,
    /// Save has been pressed once with problems; the next press saves anyway.
    warned: bool,
    error: Option<String>,
    _subscriptions: Vec<Subscription>,
}

impl ThreadsSettingsDialog {
    /// The draft as the fields and checkboxes hold it now.
    fn draft(&self, cx: &App) -> ThreadsSettingsDraft {
        ThreadsSettingsDraft {
            capture: self.capture,
            max_result_bytes: text_of(&self.max_bytes, cx),
            builtin: self.builtin,
            disabled_builtins: self.disabled_builtins.clone(),
            patterns: self
                .rows
                .iter()
                .map(|row| PatternRow {
                    name: text_of(&row.name, cx),
                    regex: text_of(&row.regex, cx),
                    group: text_of(&row.group, cx),
                })
                .collect(),
            paths: text_of(&self.paths, cx),
        }
    }
}

fn text_of(input: &Entity<TextArea>, cx: &App) -> String {
    input.read(cx).text().to_owned()
}

impl StatusView {
    /// Opens the Threads settings with the saved settings in them.
    pub(crate) fn open_threads_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let draft = ThreadsSettingsDraft::from_config(&self.settings.threads);
        let max_bytes = text_input("Result size in bytes", &draft.max_result_bytes, (1, 1), cx);
        let paths = text_input("One glob per line", &draft.paths, (3, 6), cx);
        let sample = text_input("Sample text", "", (3, 6), cx);
        let rows = draft
            .patterns
            .iter()
            .map(|row| pattern_inputs(row, window, cx))
            .collect();
        let mut subscriptions = Vec::new();
        for input in [&max_bytes, &paths, &sample] {
            subscriptions.extend(watch(input, window, cx));
        }
        let evaluation = draft.evaluate("");
        let dialog = ThreadsSettingsDialog {
            capture: draft.capture,
            builtin: draft.builtin,
            disabled_builtins: draft.disabled_builtins.clone(),
            max_bytes: max_bytes.clone(),
            rows,
            paths,
            sample,
            evaluation,
            seen_draft: draft,
            seen_sample: String::new(),
            warned: false,
            error: None,
            _subscriptions: subscriptions,
        };
        self.threads_settings = Some(dialog);
        max_bytes.update(cx, |area, cx| area.focus(window, cx));
        cx.notify();
    }

    pub(crate) fn close_threads_settings(&mut self, cx: &mut Context<Self>) {
        self.threads_settings = None;
        cx.notify();
    }

    /// An input or a checkbox changed. Evaluates the draft again, unless nothing the test
    /// reads has changed; a change to the draft takes back the warning.
    pub(crate) fn on_threads_settings_edited(&mut self, cx: &mut Context<Self>) {
        let Some(dialog) = self.threads_settings.as_mut() else {
            return;
        };
        let draft = dialog.draft(cx);
        let sample = text_of(&dialog.sample, cx);
        if draft == dialog.seen_draft && sample == dialog.seen_sample {
            return;
        }
        if draft != dialog.seen_draft {
            dialog.warned = false;
            dialog.error = None;
        }
        dialog.evaluation = draft.evaluate(&sample);
        dialog.seen_draft = draft;
        dialog.seen_sample = sample;
        cx.notify();
    }

    pub(crate) fn toggle_threads_capture(&mut self, cx: &mut Context<Self>) {
        if let Some(dialog) = self.threads_settings.as_mut() {
            dialog.capture = !dialog.capture;
        }
        self.on_threads_settings_edited(cx);
    }

    pub(crate) fn toggle_threads_builtin_group(&mut self, cx: &mut Context<Self>) {
        if let Some(dialog) = self.threads_settings.as_mut() {
            dialog.builtin = !dialog.builtin;
        }
        self.on_threads_settings_edited(cx);
    }

    pub(crate) fn toggle_threads_builtin(&mut self, name: &str, cx: &mut Context<Self>) {
        if let Some(dialog) = self.threads_settings.as_mut() {
            toggle_name(&mut dialog.disabled_builtins, name);
        }
        self.on_threads_settings_edited(cx);
    }

    pub(crate) fn add_threads_pattern(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let inputs = pattern_inputs(&PatternRow::default(), window, cx);
        if let Some(dialog) = self.threads_settings.as_mut() {
            dialog.rows.push(inputs);
        }
        self.on_threads_settings_edited(cx);
    }

    pub(crate) fn remove_threads_pattern(&mut self, index: usize, cx: &mut Context<Self>) {
        if let Some(dialog) = self.threads_settings.as_mut()
            && index < dialog.rows.len()
        {
            dialog.rows.remove(index);
        }
        self.on_threads_settings_edited(cx);
    }

    /// Save: a draft with a bad result size stays open with the reason. A draft with
    /// pattern problems first shows the warning; the next press saves it. Otherwise the
    /// config is written in the background and the dialog closes.
    pub(crate) fn submit_threads_settings(&mut self, cx: &mut Context<Self>) {
        let Some(dialog) = self.threads_settings.as_mut() else {
            return;
        };
        let draft = dialog.draft(cx);
        let config = match draft.to_config() {
            Ok(config) => config,
            Err(message) => {
                dialog.error = Some(message);
                cx.notify();
                return;
            }
        };
        if needs_confirmation(&draft.evaluate("").problems, dialog.warned) {
            dialog.warned = true;
            dialog.error = None;
            cx.notify();
            return;
        }
        self.threads_settings = None;
        self.settings.threads = config;
        self.save_settings(cx);
        self.notify_user(SAVED, cx);
    }

    /// The dialog's layer over the window, while it is open.
    pub(crate) fn threads_settings_layer(
        &self,
        window: &Window,
        cx: &Context<Self>,
    ) -> Option<Div> {
        let dialog = self.threads_settings.as_ref()?;
        let viewport = window.viewport_size();
        let left = ((f32::from(viewport.width) - WIDTH) / 2.).max(0.);
        let outside = div().absolute().size_full().occlude().on_mouse_down(
            MouseButton::Left,
            cx.listener(|this, _: &MouseDownEvent, _, cx| this.close_threads_settings(cx)),
        );
        let panel = threads_settings_panel(dialog, left, cx);
        Some(div().absolute().inset_0().child(outside).child(panel))
    }
}

/// The input of a dialog field, filled with `text`. `lines` is its minimum and maximum height.
fn text_input(
    placeholder: &str,
    text: &str,
    lines: (usize, usize),
    cx: &mut Context<StatusView>,
) -> Entity<TextArea> {
    let input = cx.new(|cx| TextArea::new(placeholder, cx).with_lines(lines.0, lines.1));
    input.update(cx, |area, cx| area.set_text(text, cx));
    input
}

/// The three inputs of a pattern row.
fn pattern_inputs(
    row: &PatternRow,
    window: &mut Window,
    cx: &mut Context<StatusView>,
) -> PatternInputs {
    let name = text_input("name", &row.name, (1, 1), cx);
    let regex = text_input("regex", &row.regex, (1, 1), cx);
    let group = text_input("group (0)", &row.group, (1, 1), cx);
    let mut subscriptions = Vec::new();
    for input in [&name, &regex, &group] {
        subscriptions.extend(watch(input, window, cx));
    }
    PatternInputs {
        name,
        regex,
        group,
        _subscriptions: subscriptions,
    }
}

/// Reports every edit of `input` to the dialog, and closes the dialog on Escape.
fn watch(
    input: &Entity<TextArea>,
    window: &mut Window,
    cx: &mut Context<StatusView>,
) -> Vec<Subscription> {
    vec![
        cx.observe(input, |this, _, cx| this.on_threads_settings_edited(cx)),
        cx.subscribe_in(
            input,
            window,
            |this, _, event: &TextAreaEvent, _window, cx| {
                if *event == TextAreaEvent::Cancel {
                    this.close_threads_settings(cx);
                }
            },
        ),
    ]
}

fn threads_settings_panel(
    dialog: &ThreadsSettingsDialog,
    left: f32,
    cx: &Context<StatusView>,
) -> Div {
    let body = div()
        .id("threads-settings-body")
        .max_h(px(BODY_HEIGHT))
        .overflow_y_scroll()
        .flex()
        .flex_col()
        .gap_4()
        .child(general_section(dialog, cx))
        .child(builtin_section(dialog, cx))
        .child(patterns_section(dialog, cx))
        .child(paths_section(dialog))
        .child(test_section(dialog, cx));
    let mut panel = div()
        .absolute()
        .top(px(TOP))
        .left(px(left))
        .w(px(WIDTH))
        .flex()
        .flex_col()
        .gap_3()
        .p_4()
        .bg(theme::panel())
        .border_1()
        .border_color(theme::hairline_strong())
        .rounded_md()
        .shadow_lg()
        .occlude()
        .child(
            div()
                .text_color(theme::text())
                .font_weight(FontWeight::SEMIBOLD)
                .child("Threads Settings"),
        )
        .child(body);
    if dialog.warned {
        panel = panel.child(
            div()
                .p_2()
                .rounded_md()
                .bg(theme::warn_bg())
                .text_sm()
                .text_color(theme::warn_fg())
                .child(WARNING),
        );
    }
    if let Some(error) = dialog.error.as_deref() {
        panel = panel.child(error_text(error));
    }
    panel.child(footer(cx))
}

/// Capture and the result size.
fn general_section(dialog: &ThreadsSettingsDialog, cx: &Context<StatusView>) -> Div {
    div()
        .flex()
        .flex_col()
        .gap_4()
        .child(check_row(
            ("threads-capture", 0),
            "Capture Threads",
            dialog.capture,
            cx,
            |this, _, cx| this.toggle_threads_capture(cx),
        ))
        .child(labelled(
            "Result Size (bytes)",
            dialog.max_bytes.clone(),
            "A longer tool result is cut, and the entry records its original size. At least 1024.",
        ))
}

/// The switch of the built-in patterns and one checkbox per built-in name.
fn builtin_section(dialog: &ThreadsSettingsDialog, cx: &Context<StatusView>) -> Div {
    let names = builtin_names();
    section("Built-in Patterns")
        .child(check_row(
            ("threads-builtin-group", 0),
            "Use the built-in patterns",
            dialog.builtin,
            cx,
            |this, _, cx| this.toggle_threads_builtin_group(cx),
        ))
        .child(
            div()
                .flex()
                .flex_row()
                .flex_wrap()
                .gap_x_4()
                .gap_y_1()
                .children(names.iter().enumerate().map(|(index, name)| {
                    let name = *name;
                    div().w(px(260.)).child(check_row(
                        ("threads-builtin", index),
                        name,
                        builtin_on(&dialog.disabled_builtins, name),
                        cx,
                        move |this, _, cx| this.toggle_threads_builtin(name, cx),
                    ))
                })),
        )
}

/// The user's patterns, each row with its problems, and the Add button.
fn patterns_section(dialog: &ThreadsSettingsDialog, cx: &Context<StatusView>) -> Div {
    let problems = &dialog.evaluation.problems;
    section("Your Patterns")
        .child(muted_text(
            "A name is letters, digits, - or _, and unique. The group is the number of the capture group that holds the secret; blank is the whole match.",
        ))
        .children(dialog.rows.iter().enumerate().map(|(index, row)| {
            let name = text_of(&row.name, cx);
            pattern_row(index, row, row_message(problems, &name), cx)
        }))
        .child(button(
            ("threads-pattern-add", 0),
            "Add Pattern",
            ButtonStyle::Secondary,
            true,
            cx,
            StatusView::add_threads_pattern,
        ))
}

/// The sensitive paths, with the problems of the globs that do not build.
fn paths_section(dialog: &ThreadsSettingsDialog) -> Div {
    section("Sensitive Paths")
        .child(labelled_note(
            "One glob per line. The content of a matching file is never stored; the path is.",
            dialog.paths.clone(),
        ))
        .children(
            dialog
                .evaluation
                .problems
                .iter()
                .filter(|problem| problem.name.starts_with("path:"))
                .map(|problem| error_text(&format!("{}: {}", problem.name, problem.message))),
        )
}

/// The sample and what the draft does to it.
fn test_section(dialog: &ThreadsSettingsDialog, cx: &Context<StatusView>) -> Div {
    section("Test")
        .child(labelled("Sample text", dialog.sample.clone(), ""))
        .child(test_output(
            &dialog.evaluation,
            text_of(&dialog.sample, cx).is_empty(),
        ))
}

/// Cancel and Save.
fn footer(cx: &Context<StatusView>) -> Div {
    div()
        .flex()
        .flex_row()
        .justify_end()
        .items_center()
        .gap_2()
        .child(button(
            ("threads-settings-cancel", 0),
            "Cancel",
            ButtonStyle::Secondary,
            true,
            cx,
            |this, _, cx| this.close_threads_settings(cx),
        ))
        .child(button(
            ("threads-settings-save", 0),
            "Save",
            ButtonStyle::Primary,
            true,
            cx,
            |this, _, cx| this.submit_threads_settings(cx),
        ))
}

/// A titled group of the dialog.
fn section(title: &'static str) -> Div {
    div().flex().flex_col().gap_2().child(
        div()
            .text_sm()
            .font_weight(FontWeight::MEDIUM)
            .text_color(theme::text())
            .child(title),
    )
}

/// A caption over an input.
fn labelled(label: &'static str, input: Entity<TextArea>, note: &'static str) -> Div {
    div()
        .flex()
        .flex_col()
        .gap_1()
        .child(div().text_xs().text_color(theme::muted()).child(label))
        .child(input)
        .when(!note.is_empty(), |this| {
            this.child(div().text_xs().text_color(theme::muted()).child(note))
        })
}

/// An input with a note above it, for the paths.
fn labelled_note(note: &'static str, input: Entity<TextArea>) -> Div {
    div()
        .flex()
        .flex_col()
        .gap_1()
        .child(div().text_xs().text_color(theme::muted()).child(note))
        .child(input)
}

/// A checkbox and its label. The row is clickable; its check is the accent colour when on.
fn check_row(
    id: (&'static str, usize),
    label: &str,
    checked: bool,
    cx: &Context<StatusView>,
    on_press: impl Fn(&mut StatusView, &mut Window, &mut Context<StatusView>) + 'static,
) -> Stateful<Div> {
    let mark = if checked {
        icon::filled(Icon::CheckSquare, icon::SMALL, theme::accent())
    } else {
        icon::view(Icon::Square, icon::SMALL, theme::muted())
    };
    div()
        .id(id)
        .flex()
        .flex_row()
        .items_center()
        .gap_2()
        .cursor_pointer()
        .child(mark)
        .child(
            div()
                .text_sm()
                .text_color(theme::text())
                .child(label.to_owned()),
        )
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, _: &MouseDownEvent, window, cx| {
                on_press(this, window, cx);
            }),
        )
}

/// One user pattern: name, regex, group, Remove, and the problems of the row under it.
fn pattern_row(
    index: usize,
    row: &PatternInputs,
    problem: Option<String>,
    cx: &Context<StatusView>,
) -> Div {
    let remove = button(
        ("threads-pattern-remove", index),
        "Remove",
        ButtonStyle::Secondary,
        true,
        cx,
        move |this, _, cx| this.remove_threads_pattern(index, cx),
    );
    div()
        .flex()
        .flex_col()
        .gap_1()
        .child(
            div()
                .flex()
                .flex_row()
                .items_end()
                .gap_2()
                .child(div().w(px(150.)).child(row.name.clone()))
                .child(div().flex_1().min_w(px(0.)).child(row.regex.clone()))
                .child(div().w(px(64.)).child(row.group.clone()))
                .child(remove),
        )
        .children(problem.map(|message| error_text(&message)))
}

/// The test's output: the problems and the pause while there are any; otherwise the
/// redacted sample and the hits.
fn test_output(evaluation: &Evaluation, sample_empty: bool) -> Div {
    let frame = div()
        .flex()
        .flex_col()
        .gap_1()
        .p_2()
        .border_1()
        .border_color(theme::hairline())
        .rounded_md()
        .text_sm();
    if !evaluation.problems.is_empty() {
        return frame
            .child(div().text_color(theme::error()).child(PAUSED))
            .children(evaluation.problems.iter().map(|problem| {
                let line = if problem.name.is_empty() {
                    problem.message.clone()
                } else {
                    format!("{}: {}", problem.name, problem.message)
                };
                error_text(&line)
            }));
    }
    let Some(redacted) = evaluation.redacted.as_ref() else {
        return frame.child(muted_text(EMPTY_SAMPLE));
    };
    if sample_empty {
        return frame.child(muted_text(EMPTY_SAMPLE));
    }
    let hits = hit_lines(&redacted.hits);
    frame
        .child(
            div()
                .font_family(theme::code_font())
                .text_xs()
                .text_color(theme::text())
                .flex()
                .flex_col()
                .children(redacted.text.lines().map(|line| {
                    div().child(if line.is_empty() {
                        " ".to_owned()
                    } else {
                        line.to_owned()
                    })
                })),
        )
        .child(if hits.is_empty() {
            muted_text(NO_HITS)
        } else {
            div()
                .flex()
                .flex_col()
                .text_xs()
                .text_color(theme::muted())
                .children(hits.into_iter().map(|hit| div().child(hit)))
        })
}

fn error_text(text: &str) -> Div {
    div()
        .text_xs()
        .text_color(theme::error())
        .child(text.to_owned())
}

fn muted_text(text: &str) -> Div {
    div()
        .text_xs()
        .text_color(theme::muted())
        .child(text.to_owned())
}

/// The look of a button: the primary one is filled, a secondary has a border.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ButtonStyle {
    Primary,
    Secondary,
}

/// A text button. A disabled one looks faded and does nothing when pressed.
pub(crate) fn button(
    id: (&'static str, usize),
    label: &'static str,
    style: ButtonStyle,
    enabled: bool,
    cx: &Context<StatusView>,
    on_press: impl Fn(&mut StatusView, &mut Window, &mut Context<StatusView>) + 'static,
) -> Stateful<Div> {
    let base = div()
        .id(id)
        .px_3()
        .h(px(26.))
        .flex()
        .items_center()
        .rounded_md()
        .child(label)
        .when(!enabled, |this| this.opacity(0.5));
    let styled = match style {
        ButtonStyle::Primary => base
            .bg(theme::accent())
            .text_color(theme::on_accent())
            .font_weight(FontWeight::MEDIUM),
        ButtonStyle::Secondary => base
            .border_1()
            .border_color(theme::button_border())
            .text_color(theme::text()),
    };
    if enabled {
        styled.cursor_pointer().on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, _: &MouseDownEvent, window, cx| {
                on_press(this, window, cx);
            }),
        )
    } else {
        styled
    }
}

#[cfg(test)]
mod tests {
    use gm_core::threads::config::{RedactConfig, ThreadsConfig, UserPattern};
    use gm_core::threads::redact::builtin_names;

    use super::*;
    use crate::app_settings::AppSettings;

    fn config() -> ThreadsConfig {
        ThreadsConfig {
            capture: false,
            max_result_bytes: 4096,
            redact: RedactConfig {
                builtin: true,
                disabled_builtins: vec!["jwt".into(), "retired-name".into()],
                patterns: vec![UserPattern {
                    name: "acme-token".into(),
                    regex: "acme_[a-z]+".into(),
                    group: 0,
                }],
                paths: vec![".env".into(), "secrets/**".into()],
            },
        }
    }

    fn row(name: &str, regex: &str, group: &str) -> PatternRow {
        PatternRow {
            name: name.into(),
            regex: regex.into(),
            group: group.into(),
        }
    }

    #[test]
    fn a_config_round_trips_through_the_draft() {
        let draft = ThreadsSettingsDraft::from_config(&config());
        assert_eq!(draft.to_config(), Ok(config()));
        assert_eq!(draft.paths, ".env\nsecrets/**");
    }

    #[test]
    fn the_draft_round_trips_through_the_git_config() {
        let dir = tempfile::tempdir().unwrap();
        let global = dir.path().join("gitconfig");
        let store = crate::app_settings::Store::with_source(
            dir.path().join("settings.json"),
            gm_core::settings::Source {
                global: Some(global.clone()),
            },
        );
        let settings = AppSettings {
            threads: config(),
            ..AppSettings::default()
        };
        store.save(&settings, 1).unwrap();
        let saved = store.load(dir.path());
        assert_eq!(saved.threads, config());
        assert_eq!(
            ThreadsSettingsDraft::from_config(&saved.threads),
            ThreadsSettingsDraft::from_config(&config())
        );
        let text = std::fs::read_to_string(global).unwrap();
        assert!(text.contains("[gitmaster \"threads\"]"), "{text}");
    }

    #[test]
    fn every_builtin_is_on_by_default_and_a_toggle_turns_it_off_and_on() {
        let mut draft = ThreadsSettingsDraft::from_config(&ThreadsConfig::default());
        assert!(
            builtin_names()
                .iter()
                .all(|name| builtin_on(&draft.disabled_builtins, name))
        );
        toggle_name(&mut draft.disabled_builtins, "jwt");
        assert_eq!(draft.disabled_builtins, ["jwt"]);
        assert!(!builtin_on(&draft.disabled_builtins, "jwt"));
        toggle_name(&mut draft.disabled_builtins, "jwt");
        assert!(draft.disabled_builtins.is_empty());
    }

    #[test]
    fn a_name_this_build_does_not_know_stays_disabled_and_saved() {
        let draft = ThreadsSettingsDraft::from_config(&config());
        assert!(draft.disabled_builtins.contains(&"retired-name".to_owned()));
        assert_eq!(
            draft.to_config().unwrap().redact.disabled_builtins,
            config().redact.disabled_builtins
        );
    }

    #[test]
    fn a_result_size_must_be_a_whole_number_of_at_least_1024_bytes() {
        assert_eq!(parse_result_bytes(" 65536 "), Ok(65_536));
        assert_eq!(parse_result_bytes("1024"), Ok(1024));
        assert_eq!(
            parse_result_bytes("1023"),
            Err("Result size must be at least 1024 bytes.".to_owned())
        );
        assert_eq!(
            parse_result_bytes("64k"),
            Err("Result size must be a whole number of bytes.".to_owned())
        );
        assert!(parse_result_bytes("-1").is_err());
        let mut draft = ThreadsSettingsDraft::from_config(&ThreadsConfig::default());
        draft.max_result_bytes = "12".into();
        assert!(draft.to_config().is_err());
    }

    #[test]
    fn paths_are_trimmed_and_blank_lines_are_dropped() {
        assert_eq!(
            normalize_paths("  .env \n\n   \nsecrets/**\r\n*.pem\n"),
            [".env", "secrets/**", "*.pem"]
        );
        assert!(normalize_paths("\n  \n").is_empty());
    }

    #[test]
    fn a_name_is_trimmed_and_a_blank_group_is_the_whole_match() {
        let mut draft = ThreadsSettingsDraft::from_config(&ThreadsConfig::default());
        draft.patterns = vec![row("  db-pass ", "(a)(b)", "2"), row("x", "y", "  ")];
        let patterns = draft.redact_config().patterns;
        assert_eq!(patterns[0].name, "db-pass");
        assert_eq!(patterns[0].group, 2);
        assert_eq!(patterns[1].group, 0);
    }

    #[test]
    fn a_valid_draft_has_no_problems_and_redacts_the_sample() {
        let draft = ThreadsSettingsDraft::from_config(&config());
        let evaluation = draft.evaluate("token acme_secret here");
        assert!(evaluation.problems.is_empty());
        let redacted = evaluation.redacted.expect("redacted");
        assert_eq!(redacted.text, "token [REDACTED:acme-token] here");
        assert_eq!(hit_lines(&redacted.hits), ["acme-token ×1"]);
    }

    #[test]
    fn an_invalid_regex_pauses_the_test_and_names_the_row() {
        let mut draft = ThreadsSettingsDraft::from_config(&config());
        draft.patterns = vec![row("broken", "(", "0")];
        let evaluation = draft.evaluate("secret");
        assert!(evaluation.redacted.is_none());
        let problem = evaluation
            .problems
            .iter()
            .find(|p| p.name == "broken")
            .expect("problem");
        assert!(problem.message.starts_with("the regex does not build"));
        assert!(row_message(&evaluation.problems, "broken").is_some());
        assert!(row_message(&evaluation.problems, "other").is_none());
    }

    #[test]
    fn a_name_that_repeats_a_builtin_or_another_row_is_a_problem() {
        let mut draft = ThreadsSettingsDraft::from_config(&ThreadsConfig::default());
        draft.patterns = vec![
            row("jwt", "a", "0"),
            row("twin", "b", "0"),
            row("twin", "c", "0"),
        ];
        let problems = draft.evaluate("").problems;
        assert!(row_message(&problems, "jwt").is_some());
        assert!(row_message(&problems, "twin").is_some());
        assert!(row_message(&problems, "other").is_none());
    }

    #[test]
    fn a_group_that_is_not_a_number_is_a_problem_and_saves_as_the_whole_match() {
        let mut draft = ThreadsSettingsDraft::from_config(&ThreadsConfig::default());
        draft.patterns = vec![row("two", "(a)", "one")];
        let evaluation = draft.evaluate("");
        assert!(
            evaluation
                .problems
                .iter()
                .any(|p| p.name == "two" && p.message.contains("whole number"))
        );
        assert!(evaluation.redacted.is_none());
        assert_eq!(draft.redact_config().patterns[0].group, 0);
    }

    #[test]
    fn a_group_past_the_last_capture_group_is_a_problem() {
        let mut draft = ThreadsSettingsDraft::from_config(&ThreadsConfig::default());
        draft.patterns = vec![row("pair", "(a)", "2")];
        let problems = draft.evaluate("").problems;
        assert!(row_message(&problems, "pair").is_some_and(|m| m.contains("group 2")));
    }

    #[test]
    fn a_problem_pauses_the_sample_even_when_the_pattern_would_match() {
        let mut draft = ThreadsSettingsDraft::from_config(&config());
        draft.patterns.push(row("bad", "(", "0"));
        let evaluation = draft.evaluate("acme_secret");
        assert!(evaluation.redacted.is_none());
        assert!(!evaluation.problems.is_empty());
    }

    #[test]
    fn only_a_new_problem_asks_for_the_second_save() {
        let problem = PatternProblem {
            name: "x".into(),
            message: "m".into(),
        };
        assert!(!needs_confirmation(&[], false));
        assert!(needs_confirmation(std::slice::from_ref(&problem), false));
        assert!(!needs_confirmation(&[problem], true));
    }

    #[test]
    fn hits_are_listed_by_name_and_count() {
        let hits = vec![
            Hit {
                pattern: "jwt".into(),
                count: 2,
            },
            Hit {
                pattern: "github-token".into(),
                count: 1,
            },
        ];
        assert_eq!(hit_lines(&hits), ["jwt ×2", "github-token ×1"]);
        assert!(hit_lines(&[]).is_empty());
    }
}
