//! The Install Agent Integration dialog (T7). It shows the harness picker, the
//! folder `GitMaster` generated the agent's plugin in, and the commands that add it
//! to the agent. **Run Commands** runs them. The commands are `gitmaster harness` (gm-core `cli.rs`), run on a
//! background task: the model here is pure and tested, and the window side is the
//! `impl StatusView` at the bottom.

use gm_core::threads::cli::{Env, harness_command};
use gm_core::threads::harness::{known_ids, lookup};
use gpui::{
    ClipboardItem, Context, Div, FocusHandle, FontWeight, KeyDownEvent, MouseButton,
    MouseDownEvent, Stateful, Window, div, prelude::*, px,
};

use crate::FocusNext;
use crate::FocusPrev;
use crate::theme;
use crate::threads::INSTALL_HINT;
use crate::threads_settings::{ButtonStyle, button};
use crate::view::StatusView;

const WIDTH: f32 = 560.;
/// The dialog hangs this far below the top of the window.
const TOP: f32 = 96.;
/// The most height the output takes before it scrolls.
const OUTPUT_HEIGHT: f32 = 220.;

/// The heading that starts the numbered commands in the output of `install`.
const STEPS_HEADING: &str = "Run these commands to add it:";
/// The start of the line that names the folder in the output of `install`.
const GENERATED: &str = "Generated the ";
/// The words between the agent's name and the folder in that line.
const IN_FOLDER: &str = " integration in ";

/// The kinds of command the dialog runs. The kind tells what a result means.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    /// `install <id>`: writes the plugin and lists the commands.
    Generate,
    /// `install <id> --run`: writes the plugin and runs the commands.
    Run,
}

/// One command to run in the background, with the number of the request that asked.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Job {
    pub(crate) request: u64,
    pub(crate) kind: Kind,
    /// The words after `gitmaster harness`, or `status` for the status command.
    pub(crate) words: Vec<String>,
}

/// What a finished command printed and whether it succeeded (exit status 0).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Captured {
    pub(crate) ok: bool,
    /// Standard output, then standard error.
    pub(crate) output: String,
}

impl Job {
    /// Runs the command. Blocking: it is called on a background task, never on the UI thread.
    #[must_use]
    pub(crate) fn execute(&self) -> Captured {
        let env = Env::from_process();
        let mut out = Vec::new();
        let mut err = Vec::new();
        let code = harness_command(&self.words, &mut out, &mut err, &env);
        let mut output = String::from_utf8_lossy(&out).into_owned();
        output.push_str(&String::from_utf8_lossy(&err));
        Captured {
            ok: code == 0,
            output,
        }
    }
}

/// The plugin folder and the commands that add it, read from the output of `install`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Generated {
    pub(crate) dir: String,
    pub(crate) steps: Vec<String>,
}

/// Where the dialog is. `Generating` and `Running` are busy; the buttons wait.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum InstallState {
    Idle,
    Generating,
    Generated {
        dir: String,
        steps: Vec<String>,
    },
    Running,
    /// The output of `Run Commands`, and whether the command succeeded.
    Done {
        ok: bool,
        output: String,
    },
    Failed {
        message: String,
    },
}

/// The state of the dialog for one harness.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct InstallModel {
    pub(crate) harness: String,
    pub(crate) state: InstallState,
    /// The number of the last request. An answer to any other request is dropped.
    request: u64,
}

impl InstallModel {
    #[must_use]
    pub(crate) fn new(harness: &str) -> Self {
        Self {
            harness: harness.to_owned(),
            state: InstallState::Idle,
            request: 0,
        }
    }

    /// Whether a command is running.
    #[must_use]
    pub(crate) fn is_busy(&self) -> bool {
        matches!(self.state, InstallState::Generating | InstallState::Running)
    }

    /// Picks `harness` and generates its plugin. `None` while a command runs.
    pub(crate) fn choose(&mut self, harness: &str) -> Option<Job> {
        if self.is_busy() {
            return None;
        }
        harness.clone_into(&mut self.harness);
        self.state = InstallState::Generating;
        Some(self.job(Kind::Generate, vec!["install".into(), self.harness.clone()]))
    }

    /// Runs the commands of the generated plugin. Only after `Generate` has shown them.
    pub(crate) fn run(&mut self) -> Option<Job> {
        if !matches!(self.state, InstallState::Generated { .. }) {
            return None;
        }
        self.state = InstallState::Running;
        Some(self.job(
            Kind::Run,
            vec!["install".into(), self.harness.clone(), "--run".into()],
        ))
    }

    /// Applies the answer to request `request`. Returns `false` and changes nothing when
    /// the answer belongs to an earlier request.
    pub(crate) fn finish(&mut self, request: u64, kind: Kind, captured: &Captured) -> bool {
        if request != self.request {
            return false;
        }
        self.state = match kind {
            Kind::Generate if captured.ok => match parse_install(&captured.output) {
                Some(generated) => InstallState::Generated {
                    dir: generated.dir,
                    steps: generated.steps,
                },
                None => InstallState::Failed {
                    message: "GitMaster did not say where the integration was written.".to_owned(),
                },
            },
            Kind::Generate => InstallState::Failed {
                message: failure_text(&captured.output),
            },
            Kind::Run => InstallState::Done {
                ok: captured.ok,
                output: captured.output.clone(),
            },
        };
        true
    }

    fn job(&mut self, kind: Kind, words: Vec<String>) -> Job {
        self.request += 1;
        Job {
            request: self.request,
            kind,
            words,
        }
    }
}

/// Reads the output of `gitmaster harness install <id>`. The format it relies on
/// (`cli.rs`, `install`):
///
/// ```text
/// Generated the <Display Name> integration in <folder>
/// Run these commands to add it:
///   1. <command>
///   2. <command>
/// ```
///
/// The folder comes from the first line that starts with `Generated the ` and has
/// ` integration in ` after the name. The commands are the numbered lines after the
/// heading, up to the first other non-blank line. Lines that are neither are ignored,
/// so a line the app adds later does not stop the reading. `None` when no folder is named.
#[must_use]
pub(crate) fn parse_install(output: &str) -> Option<Generated> {
    let dir = output.lines().find_map(|line| {
        let rest = line.trim().strip_prefix(GENERATED)?;
        let (_, dir) = rest.split_once(IN_FOLDER)?;
        Some(dir.trim().to_owned())
    })?;
    if dir.is_empty() {
        return None;
    }
    let mut steps = Vec::new();
    let mut in_steps = false;
    for line in output.lines().map(str::trim) {
        if line == STEPS_HEADING {
            in_steps = true;
        } else if in_steps && !line.is_empty() {
            if let Some(step) = numbered_step(line) {
                steps.push(step);
            } else if !steps.is_empty() {
                break;
            }
        }
    }
    Some(Generated { dir, steps })
}

/// The text of a numbered line `N. text`, or `None` for any other line.
fn numbered_step(line: &str) -> Option<String> {
    let (number, step) = line.split_once(". ")?;
    (!number.is_empty() && number.bytes().all(|byte| byte.is_ascii_digit()))
        .then(|| step.trim().to_owned())
}

/// The message of a failed generation: the last non-blank line of the output, or a fixed text.
fn failure_text(output: &str) -> String {
    output
        .lines()
        .rev()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map_or_else(
            || "The integration could not be generated.".to_owned(),
            str::to_owned,
        )
}

/// The display name of a harness id, or the id itself when this build does not know it.
#[must_use]
pub(crate) fn display_name(id: &'static str) -> &'static str {
    lookup(id).map_or(id, |adapter| adapter.display_name())
}

/// The generated commands as the clipboard gets them: one shell line each, without
/// the numbers the dialog shows. `None` when no commands were generated.
#[must_use]
pub(crate) fn commands_for_clipboard(model: &InstallModel) -> Option<String> {
    let InstallState::Generated { steps, .. } = &model.state else {
        return None;
    };
    (!steps.is_empty()).then(|| steps.join("\n"))
}

/// The dialog while it is open: its model and the focus that carries Escape.
pub(crate) struct InstallDialog {
    pub(crate) model: InstallModel,
    focus: FocusHandle,
}

impl StatusView {
    /// Opens the dialog on the first harness and generates its plugin in the background.
    pub(crate) fn open_threads_install(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(first) = known_ids().first().copied() else {
            self.notify_user("No agent integration is available in this build.", cx);
            return;
        };
        let focus = cx.focus_handle();
        focus.focus(window);
        let mut model = InstallModel::new(first);
        let job = model.choose(first);
        self.threads_install = Some(InstallDialog { model, focus });
        if let Some(job) = job {
            Self::run_threads_install_job(job, cx);
        }
        cx.notify();
    }

    pub(crate) fn close_threads_install(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.threads_install.take().is_some() {
            self.list_focus.focus(window);
        }
        cx.notify();
    }

    /// Picks a harness and generates its plugin.
    pub(crate) fn choose_threads_harness(&mut self, id: &str, cx: &mut Context<Self>) {
        let job = self
            .threads_install
            .as_mut()
            .and_then(|dialog| dialog.model.choose(id));
        if let Some(job) = job {
            Self::run_threads_install_job(job, cx);
        }
        cx.notify();
    }

    /// **Run Commands**: runs `install <id> --run` after the commands were shown.
    pub(crate) fn run_threads_install(&mut self, cx: &mut Context<Self>) {
        let job = self
            .threads_install
            .as_mut()
            .and_then(|dialog| dialog.model.run());
        if let Some(job) = job {
            Self::run_threads_install_job(job, cx);
        }
        cx.notify();
    }

    /// **Copy Commands**: puts the generated commands on the clipboard, one per line.
    pub(crate) fn copy_threads_install_commands(&mut self, cx: &mut Context<Self>) {
        let commands = self
            .threads_install
            .as_ref()
            .and_then(|dialog| commands_for_clipboard(&dialog.model));
        let Some(commands) = commands else {
            return;
        };
        let count = commands.lines().count();
        cx.write_to_clipboard(ClipboardItem::new_string(commands));
        let notice = if count == 1 {
            "Copied 1 command".to_owned()
        } else {
            format!("Copied {count} commands")
        };
        self.notify_user(&notice, cx);
    }

    /// Runs `job` on a background task and applies its answer on the UI thread.
    fn run_threads_install_job(job: Job, cx: &mut Context<Self>) {
        let request = job.request;
        let kind = job.kind;
        cx.spawn(async move |this, cx| {
            let captured = cx.background_spawn(async move { job.execute() }).await;
            let _ = this.update(cx, |view, cx| {
                view.finish_threads_install(request, kind, &captured, cx);
            });
        })
        .detach();
    }

    fn finish_threads_install(
        &mut self,
        request: u64,
        kind: Kind,
        captured: &Captured,
        cx: &mut Context<Self>,
    ) {
        if let Some(dialog) = self.threads_install.as_mut() {
            dialog.model.finish(request, kind, captured);
        }
        cx.notify();
    }

    /// The dialog's layer over the window, while it is open.
    pub(crate) fn threads_install_layer(&self, window: &Window, cx: &Context<Self>) -> Option<Div> {
        let dialog = self.threads_install.as_ref()?;
        let viewport = window.viewport_size();
        let left = ((f32::from(viewport.width) - WIDTH) / 2.).max(0.);
        let outside = div().absolute().size_full().occlude().on_mouse_down(
            MouseButton::Left,
            cx.listener(|this, _: &MouseDownEvent, window, cx| {
                this.close_threads_install(window, cx);
            }),
        );
        let panel = install_panel(dialog, left, cx);
        Some(div().absolute().inset_0().child(outside).child(panel))
    }
}

fn install_panel(dialog: &InstallDialog, left: f32, cx: &Context<StatusView>) -> Stateful<Div> {
    let model = &dialog.model;
    let busy = model.is_busy();
    let harnesses = known_ids();
    let picker =
        div()
            .flex()
            .flex_row()
            .flex_wrap()
            .gap_2()
            .children(harnesses.iter().enumerate().map(|(index, &id)| {
                button(
                    ("threads-install-harness", index),
                    display_name(id),
                    ButtonStyle::Chip {
                        selected: model.harness == id,
                    },
                    !busy,
                    cx,
                    move |this, _, cx| this.choose_threads_harness(id, cx),
                )
            }));
    let panel = div()
        .id("threads-install")
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
        .track_focus(&dialog.focus)
        .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
            if event.keystroke.key.as_str() == "escape" {
                this.close_threads_install(window, cx);
            }
        }))
        .capture_action(|_: &FocusNext, _, cx| cx.stop_propagation())
        .capture_action(|_: &FocusPrev, _, cx| cx.stop_propagation())
        .child(
            div()
                .text_color(theme::text())
                .font_weight(FontWeight::SEMIBOLD)
                .child("Install Agent Integration"),
        )
        .child(
            div()
                .text_sm()
                .text_color(theme::text())
                .child("GitMaster generates a plugin for the agent. Its commands add the plugin to the agent, and nothing changes in the agent until you run them."),
        )
        .child(
            div()
                .flex()
                .flex_col()
                .gap_1()
                .child(div().text_xs().text_color(theme::muted()).child("Agent"))
                .child(picker),
        )
        .child(state_view(&model.state));
    panel.child(actions(model, cx))
}

/// The part of the dialog that shows the state: the folder and the commands, the result, or the reason it failed.
fn state_view(state: &InstallState) -> Div {
    match state {
        InstallState::Idle => note("Choose an agent."),
        InstallState::Generating => note("Generating the integration…"),
        InstallState::Running => note("Working. This can take a minute."),
        InstallState::Failed { message } => div()
            .text_sm()
            .text_color(theme::error())
            .child(message.clone()),
        InstallState::Generated { dir, steps } => {
            let mut body = div()
                .flex()
                .flex_col()
                .gap_2()
                .child(note("Written to"))
                .child(mono(std::slice::from_ref(dir)));
            if steps.is_empty() {
                body = body.child(note("No commands are needed."));
            } else {
                let numbered: Vec<String> = steps
                    .iter()
                    .enumerate()
                    .map(|(index, step)| format!("{}. {step}", index + 1))
                    .collect();
                body = body.child(note(STEPS_HEADING)).child(mono(&numbered));
            }
            body = body.child(
                div()
                    .text_xs()
                    .text_color(theme::muted())
                    .font_family(theme::code_font())
                    .child(format!("From a terminal: {INSTALL_HINT}")),
            );
            body
        }
        InstallState::Done { ok, output } => {
            let verdict = if *ok {
                "Finished."
            } else {
                "Finished with an error. The output says why."
            };
            div()
                .flex()
                .flex_col()
                .gap_2()
                .child(note(verdict))
                .child(mono(&output.lines().map(str::to_owned).collect::<Vec<_>>()))
        }
    }
}

/// The buttons: Close, Copy Commands and Run Commands, which waits for the commands to be shown.
fn actions(model: &InstallModel, cx: &Context<StatusView>) -> Div {
    let can_run = matches!(model.state, InstallState::Generated { .. });
    div()
        .flex()
        .flex_row()
        .justify_end()
        .items_center()
        .gap_2()
        .child(button(
            ("threads-install-close", 0),
            "Close",
            ButtonStyle::Secondary,
            true,
            cx,
            StatusView::close_threads_install,
        ))
        .child(button(
            ("threads-install-copy", 0),
            "Copy Commands",
            ButtonStyle::Secondary,
            commands_for_clipboard(model).is_some(),
            cx,
            |this, _, cx| this.copy_threads_install_commands(cx),
        ))
        .child(button(
            ("threads-install-run", 0),
            "Run Commands",
            ButtonStyle::Primary,
            can_run,
            cx,
            |this, _, cx| this.run_threads_install(cx),
        ))
}

fn note(text: &str) -> Div {
    div()
        .text_sm()
        .text_color(theme::muted())
        .child(text.to_owned())
}

/// Lines in the monospace font, in a box that scrolls past `OUTPUT_HEIGHT`.
fn mono(lines: &[String]) -> Stateful<Div> {
    div()
        .id("threads-install-output")
        .p_2()
        .border_1()
        .border_color(theme::hairline())
        .rounded_md()
        .max_h(px(OUTPUT_HEIGHT))
        .overflow_y_scroll()
        .font_family(theme::code_font())
        .text_xs()
        .text_color(theme::text())
        .flex()
        .flex_col()
        .children(lines.iter().map(|line| {
            div().child(if line.is_empty() {
                " ".to_owned()
            } else {
                line.clone()
            })
        }))
}

#[cfg(test)]
mod tests {
    use super::*;

    const OUTPUT: &str = "Generated the Claude Code integration in /home/u/.local/share/gitmaster/agents/claude-code\nRun these commands to add it:\n  1. claude plugin marketplace add /x\n  2. claude plugin install gitmaster@local\n";

    fn captured(ok: bool, output: &str) -> Captured {
        Captured {
            ok,
            output: output.to_owned(),
        }
    }

    #[test]
    fn the_folder_and_the_numbered_commands_are_read() {
        let generated = parse_install(OUTPUT).expect("parsed");
        assert_eq!(
            generated.dir,
            "/home/u/.local/share/gitmaster/agents/claude-code"
        );
        assert_eq!(
            generated.steps,
            [
                "claude plugin marketplace add /x",
                "claude plugin install gitmaster@local"
            ]
        );
    }

    #[test]
    fn unknown_lines_around_the_answer_do_not_stop_the_reading() {
        let output = format!("warning: something\n\n{OUTPUT}\nthe agent may need a restart\n");
        let generated = parse_install(&output).expect("parsed");
        assert_eq!(generated.steps.len(), 2);
        let with_gap = "Generated the X integration in /d\nRun these commands to add it:\n  1. a\n\n  2. b\nnote: restart\n  3. not a step\n";
        assert_eq!(parse_install(with_gap).expect("parsed").steps, ["a", "b"]);
    }

    #[test]
    fn a_missing_heading_or_steps_gives_no_steps_and_no_folder_gives_nothing() {
        let no_steps = "Generated the X integration in /d\n";
        assert_eq!(
            parse_install(no_steps),
            Some(Generated {
                dir: "/d".into(),
                steps: vec![]
            })
        );
        assert_eq!(
            parse_install("Run these commands to add it:\n  1. a\n"),
            None
        );
        assert_eq!(parse_install("Generated the X integration in \n"), None);
        assert_eq!(parse_install(""), None);
    }

    #[test]
    fn a_numbered_line_is_a_number_then_a_dot_and_a_space() {
        assert_eq!(numbered_step("12. go"), Some("go".into()));
        assert_eq!(numbered_step("a. go"), None);
        assert_eq!(numbered_step("1 go"), None);
        assert_eq!(numbered_step(". go"), None);
    }

    #[test]
    fn choosing_a_harness_generates_its_plugin_and_the_answer_shows_the_commands() {
        let mut model = InstallModel::new("claude-code");
        let job = model.choose("claude-code").expect("job");
        assert_eq!(job.kind, Kind::Generate);
        assert_eq!(job.words, ["install", "claude-code"]);
        assert_eq!(model.state, InstallState::Generating);
        assert!(model.is_busy());
        assert!(model.finish(job.request, job.kind, &captured(true, OUTPUT)));
        assert_eq!(
            model.state,
            InstallState::Generated {
                dir: "/home/u/.local/share/gitmaster/agents/claude-code".into(),
                steps: vec![
                    "claude plugin marketplace add /x".into(),
                    "claude plugin install gitmaster@local".into()
                ],
            }
        );
    }

    #[test]
    fn a_failed_generation_shows_the_last_line_of_the_output() {
        let mut model = InstallModel::new("nope");
        let job = model.choose("nope").expect("job");
        model.finish(
            job.request,
            job.kind,
            &captured(
                false,
                "gitmaster harness: unknown\nunknown harness 'nope'; known: claude-code\n",
            ),
        );
        assert_eq!(
            model.state,
            InstallState::Failed {
                message: "unknown harness 'nope'; known: claude-code".into()
            }
        );
    }

    #[test]
    fn a_generation_that_succeeds_without_a_folder_fails() {
        let mut model = InstallModel::new("claude-code");
        let job = model.choose("claude-code").expect("job");
        model.finish(job.request, job.kind, &captured(true, "something else\n"));
        assert!(matches!(model.state, InstallState::Failed { .. }));
    }

    #[test]
    fn run_is_allowed_only_after_the_commands_are_shown() {
        let mut model = InstallModel::new("claude-code");
        assert!(model.run().is_none());
        let job = model.choose("claude-code").expect("job");
        assert!(model.run().is_none(), "busy");
        model.finish(job.request, job.kind, &captured(true, OUTPUT));
        let run = model.run().expect("run");
        assert_eq!(run.kind, Kind::Run);
        assert_eq!(run.words, ["install", "claude-code", "--run"]);
        assert_eq!(model.state, InstallState::Running);
        model.finish(run.request, run.kind, &captured(false, "step 1 failed\n"));
        assert_eq!(
            model.state,
            InstallState::Done {
                ok: false,
                output: "step 1 failed\n".into()
            }
        );
    }

    #[test]
    fn an_answer_to_an_earlier_request_is_dropped() {
        let mut model = InstallModel::new("claude-code");
        let first = model.choose("claude-code").expect("job");
        model.finish(first.request, first.kind, &captured(false, "old\n"));
        let second = model.choose("claude-code").expect("job");
        assert!(!model.finish(first.request, first.kind, &captured(true, OUTPUT)));
        assert_eq!(model.state, InstallState::Generating);
        assert!(model.finish(second.request, second.kind, &captured(false, "ok\n")));
    }

    #[test]
    fn a_harness_picked_while_busy_is_not_taken() {
        let mut model = InstallModel::new("claude-code");
        model.choose("claude-code").expect("job");
        assert!(model.choose("other").is_none());
        assert_eq!(model.harness, "claude-code");
    }

    #[test]
    fn the_commands_are_copied_one_per_line_without_their_numbers() {
        let model = InstallModel {
            harness: "claude-code".to_owned(),
            state: InstallState::Generated {
                dir: "/d".to_owned(),
                steps: vec![
                    "claude plugin marketplace add /x".to_owned(),
                    "claude plugin install gitmaster@local".to_owned(),
                ],
            },
            request: 0,
        };
        assert_eq!(
            commands_for_clipboard(&model).as_deref(),
            Some("claude plugin marketplace add /x\nclaude plugin install gitmaster@local")
        );
    }

    #[test]
    fn a_generation_with_no_commands_or_no_generation_copies_nothing() {
        let empty = InstallModel {
            harness: "claude-code".to_owned(),
            state: InstallState::Generated {
                dir: "/d".to_owned(),
                steps: Vec::new(),
            },
            request: 0,
        };
        assert_eq!(commands_for_clipboard(&empty), None);
        let idle = InstallModel::new("claude-code");
        assert_eq!(commands_for_clipboard(&idle), None);
        let running = InstallModel {
            state: InstallState::Running,
            ..InstallModel::new("claude-code")
        };
        assert_eq!(commands_for_clipboard(&running), None);
    }

    #[test]
    fn the_known_harness_has_a_display_name_and_an_unknown_one_its_id() {
        assert_eq!(display_name("claude-code"), "Claude Code");
        assert_eq!(display_name("zed"), "zed");
        assert!(known_ids().contains(&"claude-code"));
    }
}
