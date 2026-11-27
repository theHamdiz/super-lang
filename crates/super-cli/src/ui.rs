//! Build progress UI for the Super toolchain.
//!
//! The current step name is cyan-green. `Finished` is cyan-green, `Failed` is
//! cyan-red, and `Finished with warnings` is cyan-yellow. On a terminal the
//! finished step is rewritten in place before the next one is shown, so the
//! highlighted line is the step that is running.

use std::io::{self, IsTerminal, Write};
use std::time::Instant;

use super_core::wants_color;

/// Cyan green. Current step name, and `Finished`.
const CYAN_GREEN: &str = "\x1b[38;2;0;214;170m";
/// Cyan red. `Failed`.
const CYAN_RED: &str = "\x1b[38;2;255;84;112m";
/// Neon yellow. `Finished with warnings` and forwarded `cc` warnings.
const CYAN_YELLOW: &str = "\x1b[38;2;255;255;0m";
const DIM: &str = "\x1b[38;2;120;144;156m";
const RESET: &str = "\x1b[0m";
const BOLD: &str = "\x1b[1m";

pub const NATIVE_STEPS: &[&str] = &[
    "Resolving",
    "Parsing",
    "Checking",
    "Optimizing",
    "Emitting",
    "Compiling",
];

pub const EMIT_STEPS: &[&str] = &["Resolving", "Parsing", "Checking", "Optimizing", "Emitting"];

pub const BYTECODE_STEPS: &[&str] = &["Resolving", "Parsing", "Checking", "Optimizing", "Writing"];

/// `super check` / `super c`: type-check, emit C, then `cc -fsyntax-only`.
pub const CHECK_STEPS: &[&str] = &["Resolving", "Parsing", "Checking", "Emitting", "Compiling"];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tone {
    Ok,
    Err,
    Warn,
}

/// Paint `text` in the status color when the terminal (or `FORCE_COLOR`) wants color.
pub fn paint_text(tone: Tone, text: &str) -> String {
    let code = match tone {
        Tone::Ok => CYAN_GREEN,
        Tone::Err => CYAN_RED,
        Tone::Warn => CYAN_YELLOW,
    };
    paint(wants_color(), code, text)
}

pub fn say_ok(line: &str) {
    println!("{}", paint_text(Tone::Ok, line));
}

pub fn say_err(line: &str) {
    eprintln!("{}", paint_text(Tone::Err, line));
}

pub fn say_warn(line: &str) {
    eprintln!("{}", paint_text(Tone::Warn, line));
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum StepState {
    Pending,
    Current,
    Done,
    Failed,
}

struct Step {
    name: &'static str,
    detail: String,
    state: StepState,
    printed: bool,
}

enum StatusKind {
    Finished,
    Warnings,
    Failed,
}

/// Live build report. Lines are also kept for tests.
pub struct BuildUi {
    steps: Vec<Step>,
    index: usize,
    started: Instant,
    color: bool,
    tty: bool,
    /// Write lines to stderr. Tests turn this off and read [`Self::transcript`].
    echo: bool,
    /// The current line is on screen. A terminal rewrites it when the step ends.
    on_screen: bool,
    /// Do not move the cursor up. Set once an external tool may have printed.
    sticky: bool,
    sealed: bool,
    closed: bool,
    log: Vec<String>,
}

impl BuildUi {
    pub fn new(steps: &[&'static str]) -> Self {
        Self::create(steps, wants_color(), io::stderr().is_terminal(), true)
    }

    fn create(steps: &[&'static str], color: bool, tty: bool, echo: bool) -> Self {
        Self {
            steps: steps
                .iter()
                .copied()
                .map(|name| Step {
                    name,
                    detail: String::new(),
                    state: StepState::Pending,
                    printed: false,
                })
                .collect(),
            index: 0,
            started: Instant::now(),
            color,
            tty,
            echo,
            on_screen: false,
            sticky: false,
            sealed: false,
            closed: false,
            log: Vec::new(),
        }
    }

    #[cfg(test)]
    pub fn transcript(&self) -> &[String] {
        &self.log
    }

    /// Start the first step.
    pub fn begin(&mut self, detail: impl Into<String>) {
        self.activate(0, detail.into());
    }

    /// Finish the current step and start the next one.
    pub fn next(&mut self, detail: impl Into<String>) {
        if self.closed || self.index + 1 >= self.steps.len() {
            self.set_detail(detail);
            return;
        }
        self.complete_current();
        self.activate(self.index + 1, detail.into());
    }

    pub fn set_detail(&mut self, detail: impl Into<String>) {
        if self.closed {
            return;
        }
        self.steps[self.index].detail = detail.into();
        if self.tty && self.on_screen {
            self.rewrite_current();
        }
    }

    /// The next output may come from the C compiler. Leave the current line put.
    pub fn external_follows(&mut self) {
        self.sticky = true;
    }

    /// Mark the current step done without printing the final status line.
    pub fn seal(&mut self) {
        if self.closed || self.sealed {
            return;
        }
        self.sealed = true;
        if !self.sticky {
            self.complete_current();
        } else if !self.tty && !self.steps[self.index].printed {
            self.steps[self.index].state = StepState::Done;
            self.print_current();
        }
    }

    /// `Finished` or `Finished with warnings`. Call after diagnostics are printed.
    pub fn status(&mut self, warnings: usize, detail: &str) {
        if self.closed {
            return;
        }
        let kind = if warnings > 0 {
            StatusKind::Warnings
        } else {
            StatusKind::Finished
        };
        self.write_status(kind, detail);
        self.closed = true;
    }

    #[cfg(test)]
    pub fn succeed(&mut self, warnings: usize, detail: &str) {
        self.seal();
        self.status(warnings, detail);
    }

    pub fn fail(&mut self, detail: &str) {
        if self.closed {
            return;
        }
        self.steps[self.index].state = StepState::Failed;
        if self.tty && self.on_screen && !self.sticky {
            self.rewrite_current();
        } else if !self.steps[self.index].printed {
            self.print_current();
        }
        self.write_status(StatusKind::Failed, detail);
        self.closed = true;
    }

    fn activate(&mut self, index: usize, detail: String) {
        self.index = index;
        let step = &mut self.steps[index];
        step.detail = detail;
        step.state = StepState::Current;
        step.printed = false;
        if self.tty {
            self.print_current();
            self.on_screen = true;
        }
    }

    fn complete_current(&mut self) {
        self.steps[self.index].state = StepState::Done;
        if self.tty && self.on_screen && !self.sticky {
            self.rewrite_current();
            self.on_screen = false;
        } else if !self.tty && !self.steps[self.index].printed {
            self.print_current();
        }
    }

    fn rewrite_current(&mut self) {
        let line = self.format_step(self.index);
        if self.echo {
            let mut err = io::stderr().lock();
            let _ = writeln!(err, "\x1b[1A\x1b[2K{line}");
            let _ = err.flush();
        }
        if let Some(last) = self.log.last_mut() {
            *last = line;
        } else {
            self.log.push(line);
        }
        self.steps[self.index].printed = true;
        self.on_screen = true;
    }

    fn print_current(&mut self) {
        let line = self.format_step(self.index);
        self.emit_line(&line);
        self.steps[self.index].printed = true;
    }

    fn write_status(&mut self, kind: StatusKind, detail: &str) {
        let word = match kind {
            StatusKind::Finished => "Finished",
            StatusKind::Warnings => "Finished with warnings",
            StatusKind::Failed => "Failed",
        };
        let color = match kind {
            StatusKind::Finished => CYAN_GREEN,
            StatusKind::Warnings => CYAN_YELLOW,
            StatusKind::Failed => CYAN_RED,
        };
        let label = if word.len() <= 12 {
            format!("{word:>12}")
        } else {
            word.to_string()
        };
        let painted = paint(self.color, color, &label);
        let line = format!("{painted}  {detail} in {}", elapsed(self.started));
        self.emit_line(&line);
    }

    fn format_step(&self, index: usize) -> String {
        let step = &self.steps[index];
        let name = if step.name.len() <= 12 {
            format!("{:>12}", step.name)
        } else {
            step.name.to_string()
        };
        let color = match step.state {
            StepState::Current => CYAN_GREEN,
            StepState::Failed => CYAN_RED,
            StepState::Done | StepState::Pending => DIM,
        };
        let label = paint(self.color && step.state != StepState::Pending, color, &name);
        let total = self.steps.len();
        let n = index + 1;
        let progress = if step.state == StepState::Current {
            format!("[{}] {n}/{total}", bar(n, total))
        } else {
            format!("{n}/{total}")
        };
        if step.detail.is_empty() {
            format!("{label}  {progress}")
        } else {
            format!("{label}  {}  {progress}", step.detail)
        }
    }

    fn emit_line(&mut self, line: &str) {
        self.log.push(line.to_string());
        if self.echo {
            let mut err = io::stderr().lock();
            let _ = writeln!(err, "{line}");
            let _ = err.flush();
        }
    }
}

fn paint(color: bool, code: &str, text: &str) -> String {
    if color {
        format!("{BOLD}{code}{text}{RESET}")
    } else {
        text.to_string()
    }
}

fn bar(done: usize, total: usize) -> String {
    const WIDTH: usize = 10;
    let filled = (done * WIDTH).checked_div(total).unwrap_or(0).min(WIDTH);
    format!("{}{}", "=".repeat(filled), "-".repeat(WIDTH - filled))
}

fn elapsed(started: Instant) -> String {
    let secs = started.elapsed().as_secs_f64();
    if secs < 60.0 {
        format!("{secs:.2}s")
    } else {
        let mins = (secs / 60.0) as u64;
        format!("{mins}m {:04.1}s", secs % 60.0)
    }
}

#[cfg(test)]
fn memory(steps: &[&'static str], color: bool) -> BuildUi {
    BuildUi::create(steps, color, false, false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_steps_show_progress_and_finished() {
        let mut ui = memory(&["Resolving", "Parsing", "Compiling"], false);
        ui.begin("Pod.toml");
        ui.next("src/main.sr");
        ui.next("cc -O3 -flto");
        ui.external_follows();
        ui.succeed(0, "profile release [aot c23]");
        let text = ui.transcript().join("\n");
        assert!(text.contains("Resolving"), "{text}");
        assert!(text.contains("Pod.toml"), "{text}");
        assert!(text.contains("1/3"), "{text}");
        assert!(text.contains("Parsing"), "{text}");
        assert!(text.contains("Compiling"), "{text}");
        assert!(text.contains("cc -O3 -flto"), "{text}");
        assert!(
            text.contains("[==========] 3/3") || text.contains("3/3"),
            "{text}"
        );
        assert!(text.contains("Finished"), "{text}");
        assert!(text.contains("profile release [aot c23]"), "{text}");
        assert!(!text.contains("\x1b["), "{text}");
    }

    #[test]
    fn status_uses_cyan_green_red_and_yellow() {
        let mut ok = memory(&["Compiling"], true);
        ok.begin("cc");
        ok.succeed(0, "profile dev");
        let ok_text = ok.transcript().join("\n");
        assert!(ok_text.contains(CYAN_GREEN), "{ok_text}");
        assert!(ok_text.contains("Finished"), "{ok_text}");
        assert!(!ok_text.contains("Finished with warnings"), "{ok_text}");

        let mut warn = memory(&["Compiling"], true);
        warn.begin("cc");
        warn.succeed(2, "profile dev");
        let warn_text = warn.transcript().join("\n");
        assert!(warn_text.contains(CYAN_YELLOW), "{warn_text}");
        assert!(warn_text.contains("Finished with warnings"), "{warn_text}");

        let mut bad = memory(&["Compiling"], true);
        bad.begin("cc");
        bad.fail("cc failed");
        let bad_text = bad.transcript().join("\n");
        assert!(bad_text.contains(CYAN_RED), "{bad_text}");
        assert!(bad_text.contains("Failed"), "{bad_text}");
        assert!(bad_text.contains("cc failed"), "{bad_text}");
    }

    #[test]
    fn current_step_name_is_cyan_green_on_a_terminal() {
        let mut ui = BuildUi::create(&["Resolving", "Compiling"], true, true, false);
        ui.begin("Pod.toml");
        let text = ui.transcript().join("\n");
        assert!(text.contains("Resolving"), "{text}");
        assert!(text.contains(CYAN_GREEN), "{text}");
        assert!(text.contains("1/2"), "{text}");
        ui.next("cc -O0");
        let text = ui.transcript().join("\n");
        assert!(text.contains("Compiling"), "{text}");
        assert!(text.contains(CYAN_GREEN), "{text}");
    }
}
