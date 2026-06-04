// SPDX-License-Identifier: GPL-2.0

//! Terminal output styling.
//!
//! All human-facing output flows through here so it stays consistent: tagged,
//! colored, concise. Tags telegraph the message class at a glance —
//!
//! - `[+]` green   — something good happened (new coverage, a corpus add).
//! - `[*]` blue    — neutral status / progress.
//! - `[!]` yellow  — a warning worth noticing.
//! - `[x]` red     — an error.
//! - `✦` magenta  — a solution (a bug), bracketed in sparkles, impossible to miss.
//!
//! Indented sub-details (under a heartbeat or status line) print dim with no tag.
//!
//! Color is emitted only to a real terminal with `NO_COLOR` unset; piped or
//! benchmark output stays plain. In benchmark mode everything but errors and
//! the final result line is suppressed.

use std::collections::VecDeque;
use std::io::{IsTerminal, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

static QUIET: AtomicBool = AtomicBool::new(false);
static COLOR: OnceLock<bool> = OnceLock::new();

/// Whether the persistent bottom spinner is currently animating.
static SPIN_ON: AtomicBool = AtomicBool::new(false);

/// Serializes all stdout writes and tracks the bottom spinner so log lines and
/// the spinner's in-place animation never corrupt each other.
static OUT: Mutex<Out> = Mutex::new(Out {
    frame: 0,
    spinner_visible: false,
    status: None,
});

struct Out {
    /// Animation frame, advanced by the spinner thread.
    frame: usize,
    /// Whether the spinner currently occupies the terminal's last line.
    spinner_visible: bool,
    /// Live parenthetical the campaign refreshes (e.g. time/branches since the
    /// last corpus find); rendered after the spinner's action word.
    status: Option<String>,
}

/// Update the spinner's live parenthetical (see [`Out::status`]). The next
/// repaint picks it up; cheap, called a few times a second by the campaign.
pub fn set_spinner_status(status: String) {
    OUT.lock().unwrap().status = Some(status);
}

/// Print a finished log line (or block) above the spinner: clear the spinner
/// line if present, write the content, then redraw the spinner beneath it so it
/// stays pinned to the bottom. The single `OUT` lock keeps concurrent workers,
/// the monitor, and the spinner thread from interleaving.
fn emit(content: &str) {
    let mut out = OUT.lock().unwrap();
    let mut buf = String::new();
    if out.spinner_visible {
        buf.push_str("\r\x1b[2K");
        out.spinner_visible = false;
    }
    buf.push_str(content);
    buf.push('\n');
    if SPIN_ON.load(Ordering::Relaxed) && color() {
        buf.push_str(&spinner_line(out.frame, out.status.as_deref()));
        out.spinner_visible = true;
    }
    print!("{buf}");
    let _ = std::io::stdout().flush();
}

/// Configure output once at startup. `quiet` (benchmark mode) suppresses all
/// but errors and the explicit result line.
pub fn init(quiet: bool) {
    QUIET.store(quiet, Ordering::Relaxed);
}

fn quiet() -> bool {
    QUIET.load(Ordering::Relaxed)
}

fn color() -> bool {
    *COLOR.get_or_init(|| std::env::var_os("NO_COLOR").is_none() && std::io::stdout().is_terminal())
}

// 24-bit color helpers. Plain passthrough when color is off.
fn paint(s: &str, r: u8, g: u8, b: u8, bold: bool) -> String {
    if !color() {
        return s.to_string();
    }
    let bld = if bold { "1;" } else { "" };
    format!("\x1b[{bld}38;2;{r};{g};{b}m{s}\x1b[0m")
}

fn dimmed(s: &str) -> String {
    if !color() {
        s.to_string()
    } else {
        format!("\x1b[2m{s}\x1b[0m")
    }
}

const BLUE: (u8, u8, u8) = (90, 170, 255);
const GREEN: (u8, u8, u8) = (90, 220, 130);
const YELLOW: (u8, u8, u8) = (240, 200, 90);
const RED: (u8, u8, u8) = (245, 100, 100);
const MAGENTA: (u8, u8, u8) = (220, 120, 240);

fn tagged(tag: &str, c: (u8, u8, u8), msg: &str) {
    if quiet() {
        return;
    }
    emit(&format!("{} {msg}", paint(tag, c.0, c.1, c.2, true)));
}

/// `[*]` neutral status / progress.
pub fn info(msg: &str) {
    tagged("[*]", BLUE, msg);
}

/// `[+]` something good happened.
pub fn good(msg: &str) {
    tagged("[+]", GREEN, msg);
}

/// `[!]` a warning worth noticing.
pub fn warn(msg: &str) {
    tagged("[!]", YELLOW, msg);
}

/// `[x]` an error. Clears the spinner first, then prints to stderr (shown even
/// in quiet mode).
pub fn err(msg: &str) {
    let mut out = OUT.lock().unwrap();
    if out.spinner_visible {
        print!("\r\x1b[2K");
        out.spinner_visible = false;
        let _ = std::io::stdout().flush();
    }
    eprintln!("{} {msg}", paint("[x]", RED.0, RED.1, RED.2, true));
}

/// A solution (a bug). Loud on purpose: bold magenta, sparkles.
pub fn solution(headline: &str) {
    if quiet() {
        return;
    }
    let spark = paint("✦", MAGENTA.0, MAGENTA.1, MAGENTA.2, true);
    let body = paint(headline, MAGENTA.0, MAGENTA.1, MAGENTA.2, true);
    emit(&format!("{spark} {body} {spark}"));
}

/// A periodic heartbeat line: the run's vitals, printed above the spinner under
/// the plain `[*]` info marker. The always-running spinner (see
/// [`spinner_start`]) is the live "breathing" indicator; this is the slow stats
/// pulse.
pub fn heartbeat(msg: &str) {
    info(msg);
}

/// An indented sub-detail under a heartbeat/status line.
pub fn detail(msg: &str) {
    if quiet() {
        return;
    }
    emit(&format!("    {}", dimmed(msg)));
}

/// Render the bottom spinner for animation frame `frame`: a bracketed,
/// advancing ASCII spinner glyph plus a shimmering action word that rotates
/// every few seconds (a bright band sweeps the text).
fn spinner_line(frame: usize, status: Option<&str>) -> String {
    const SPIN: [&str; 4] = ["|", "/", "-", "\\"];
    let g = SPIN[frame % SPIN.len()];
    // Hold each word for ~24 frames (~3s at the 120 ms tick) before rotating.
    let word = action_word(frame / 24);
    let suffix = match status {
        Some(s) => format!(" ({s})"),
        None => String::new(),
    };
    shimmer(&format!("[{g}] {word}{suffix}"), frame)
}

/// A rotating, playful description of what the fuzzer is busy doing. Picked
/// pseudo-randomly from `group` (the word-rotation index) so successive words
/// don't just march down the list.
fn action_word(group: usize) -> &'static str {
    const WORDS: [&str; 24] = [
        "fuzzing",
        "mutating",
        "rewinding",
        "branching",
        "exploring",
        "splicing",
        "scheduling",
        "replaying",
        "time-traveling",
        "swarming",
        "scattering",
        "bit-flipping",
        "perturbing",
        "checkpointing",
        "spelunking",
        "rummaging",
        "wrangling",
        "tinkering",
        "poking bits",
        "prodding",
        "jostling",
        "shuffling",
        "nudging",
        "havocking",
    ];
    let mixed = group.wrapping_mul(0x9E37_79B9_7F4A_7C15) >> 40;
    WORDS[mixed % WORDS.len()]
}

/// Color `text` with a calm blue base and a bright band that sweeps left→right
/// as `frame` advances — the shimmer effect shared by the banner and spinner.
fn shimmer(text: &str, frame: usize) -> String {
    if !color() {
        return text.to_string();
    }
    let chars: Vec<char> = text.chars().collect();
    let n = chars.len().max(1);
    let span = n + 6;
    let pos = (frame % span) as f64 - 3.0;
    let mut out = String::new();
    for (i, &ch) in chars.iter().enumerate() {
        let (mut r, mut g, mut b) = (90u8, 150u8, 230u8);
        let d = (i as f64 - pos).abs();
        if d < 2.0 {
            let lift = (1.0 - d / 2.0) * 0.85;
            r = (r as f64 + (255.0 - r as f64) * lift) as u8;
            g = (g as f64 + (255.0 - g as f64) * lift) as u8;
            b = (b as f64 + (255.0 - b as f64) * lift) as u8;
        }
        out.push_str(&format!("\x1b[1;38;2;{r};{g};{b}m{ch}"));
    }
    out.push_str("\x1b[0m");
    out
}

/// Start the always-on, shimmering bottom spinner. No-op when output is piped
/// or quiet. Idempotent. Spawns one detached thread that repaints the spinner
/// in place a few times a second until [`spinner_stop`].
pub fn spinner_start() {
    if quiet() || !color() {
        return;
    }
    if SPIN_ON.swap(true, Ordering::Relaxed) {
        return; // already running
    }
    std::thread::spawn(|| {
        while SPIN_ON.load(Ordering::Relaxed) {
            {
                let mut out = OUT.lock().unwrap();
                out.frame = out.frame.wrapping_add(1);
                let line = spinner_line(out.frame, out.status.as_deref());
                print!("\r\x1b[2K{line}");
                out.spinner_visible = true;
                let _ = std::io::stdout().flush();
            }
            std::thread::sleep(Duration::from_millis(120));
        }
    });
}

/// Stop the spinner and clear its line. Safe to call when it was never started.
pub fn spinner_stop() {
    if !SPIN_ON.swap(false, Ordering::Relaxed) {
        return;
    }
    let mut out = OUT.lock().unwrap();
    if out.spinner_visible {
        print!("\r\x1b[2K");
        out.spinner_visible = false;
        let _ = std::io::stdout().flush();
    }
}

/// Shimmering wordmark, printed once at startup. A bright band sweeps across a
/// blue→magenta gradient title a few times, then settles. Animation runs only
/// on a color terminal; otherwise a single plain line is printed.
pub fn banner(subtitle: &str) {
    if quiet() {
        return;
    }
    const WORD: &str = "f l u x";
    if !color() {
        println!("{WORD}  —  {subtitle}");
        return;
    }
    let chars: Vec<char> = WORD.chars().collect();
    let n = chars.len();
    // Base gradient: blue (90,170,255) → magenta (220,120,240).
    let base = |i: usize| -> (u8, u8, u8) {
        let t = if n <= 1 {
            0.0
        } else {
            i as f64 / (n - 1) as f64
        };
        let lerp = |a: u8, b: u8| (a as f64 + (b as f64 - a as f64) * t) as u8;
        (lerp(90, 220), lerp(170, 120), lerp(255, 240))
    };
    let frames = 22;
    for f in 0..frames {
        // Bright band position sweeps left→right across [-2, n+2].
        let pos = (f as f64 / frames as f64) * (n as f64 + 4.0) - 2.0;
        let mut out = String::from("\r  ");
        for (i, &ch) in chars.iter().enumerate() {
            let (mut r, mut g, mut b) = base(i);
            let d = (i as f64 - pos).abs();
            if d < 1.5 {
                // Lift toward white near the band — the shimmer highlight.
                let lift = (1.0 - d / 1.5) * 0.85;
                r = (r as f64 + (255.0 - r as f64) * lift) as u8;
                g = (g as f64 + (255.0 - g as f64) * lift) as u8;
                b = (b as f64 + (255.0 - b as f64) * lift) as u8;
            }
            out.push_str(&format!("\x1b[1;38;2;{r};{g};{b}m{ch}"));
        }
        out.push_str("\x1b[0m");
        print!("{out}");
        use std::io::Write;
        let _ = std::io::stdout().flush();
        std::thread::sleep(Duration::from_millis(34));
    }
    // Settle on the static gradient + subtitle.
    let mut out = String::from("\r  ");
    for (i, &ch) in chars.iter().enumerate() {
        let (r, g, b) = base(i);
        out.push_str(&format!("\x1b[1;38;2;{r};{g};{b}m{ch}"));
    }
    out.push_str("\x1b[0m");
    println!("{out}   {}", dimmed(subtitle));
}

/// Current terminal width in columns, or 120 if it can't be determined.
fn term_width() -> usize {
    unsafe {
        let mut ws: libc::winsize = std::mem::zeroed();
        if libc::ioctl(1, libc::TIOCGWINSZ, &mut ws) == 0 && ws.ws_col > 0 {
            return ws.ws_col as usize;
        }
    }
    120
}

/// How many of the most recent boot lines stay on screen at once.
const BOOT_WINDOW: usize = 5;

/// A fixed-height, in-place scrolling view of the guest's boot log.
///
/// Booting a container workload emits thousands of kernel/journal lines; we
/// don't want all of them in the scrollback. On a terminal, [`BootLog`] keeps
/// only the last [`BOOT_WINDOW`] lines visible, redrawing them in place as new
/// lines arrive (a small marquee panel), then [`finish`](Self::finish) seals
/// the panel so the rest of the session prints below it. Piped/quiet output
/// falls back to plain line-by-line (or nothing) so captures keep the full log.
pub struct BootLog {
    lines: VecDeque<String>,
    drawn: usize,
    animate: bool,
    /// Last time the panel was actually painted. Boot emits serial faster than
    /// any terminal can usefully show, and a flush per line would throttle the
    /// guest to I/O speed — so we coalesce repaints to ~30 fps.
    last_paint: Option<std::time::Instant>,
}

impl Default for BootLog {
    fn default() -> Self {
        Self::new()
    }
}

const REPAINT_INTERVAL: Duration = Duration::from_millis(33);

impl BootLog {
    pub fn new() -> Self {
        Self {
            lines: VecDeque::with_capacity(BOOT_WINDOW),
            drawn: 0,
            // Only the in-place marquee needs a real terminal; over a pipe the
            // cursor moves would be garbage, so fall back to plain printing.
            animate: color() && !quiet(),
            last_paint: None,
        }
    }

    /// Feed one boot line (already without its trailing newline).
    ///
    /// The guest emits boot serial far faster than a terminal can show, and
    /// doing real work in this callback throttles the guest (it runs inside the
    /// VM's event dispatch). So we *sample*: a line that arrives within
    /// [`REPAINT_INTERVAL`] of the last paint is dropped untouched — only ~30
    /// lines a second are kept, formatted, and painted. The result is a smooth
    /// scrolling marquee with negligible boot slowdown.
    pub fn push(&mut self, line: &str) {
        if quiet() {
            return;
        }
        if !self.animate {
            println!("{line}");
            return;
        }
        let now = std::time::Instant::now();
        if self
            .last_paint
            .is_some_and(|t| now.duration_since(t) < REPAINT_INTERVAL)
        {
            return; // sampled out
        }
        let budget = term_width().saturating_sub(4).max(20);
        let clean = super::shape::strip_ansi(line);
        let truncated: String = clean.chars().take(budget).collect();
        if self.lines.len() == BOOT_WINDOW {
            self.lines.pop_front();
        }
        self.lines.push_back(truncated);
        self.redraw();
        self.last_paint = Some(now);
    }

    fn redraw(&mut self) {
        let mut out = String::new();
        if self.drawn > 0 {
            // Move back up to the top of the previously-drawn panel.
            out.push_str(&format!("\x1b[{}A", self.drawn));
        }
        let last = self.lines.len().saturating_sub(1);
        for (i, line) in self.lines.iter().enumerate() {
            // `\x1b[2K` clears the whole line first so a shorter new line
            // doesn't leave stale tail characters from the old one.
            let bar = if color() {
                "\x1b[2m│\x1b[0m "
            } else {
                "│ "
            };
            // The freshest line is a touch brighter than the trailing history.
            let body = if i == last { dimmed(line) } else { faint(line) };
            out.push_str(&format!("\x1b[2K{bar}{body}\n"));
        }
        self.drawn = self.lines.len();
        print!("{out}");
        let _ = std::io::stdout().flush();
    }

    /// Seal the panel: paint the final five lines (a throttled push may have
    /// left newer lines unshown), then leave them on screen so everything after
    /// prints below. Safe to call when nothing was drawn.
    pub fn finish(&mut self) {
        if self.animate && !self.lines.is_empty() {
            self.redraw();
        }
        self.lines.clear();
        self.drawn = 0;
        self.last_paint = None;
    }
}

/// Extra-dim styling for trailing boot-history lines.
fn faint(s: &str) -> String {
    if !color() {
        s.to_string()
    } else {
        format!("\x1b[2;38;2;110;110;120m{s}\x1b[0m")
    }
}
