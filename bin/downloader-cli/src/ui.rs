//! Terminal progress display.
//!
//! Inline (not full screen) so finished tracks stay in the scrollback and
//! output still works in pipes / CI logs:
//!
//! ```text
//! ✔ [1/4] 翻唱合集/01 - 一样的月光.m4a  192K AAC · AAC · 3.79 MiB
//! [3/4] 01 - 上集   ██████████▌     1.81 MiB/3.79 MiB   2.13 MiB/s  1s 192K AAC ↻1
//! [4/4] 02 - 下集   ⠙ selecting stream
//! ⠧ 2/4 tracks ━━━━━━━━━━━━━━━─────  75% 10.83 MiB · 3.46 MiB/s · 2s · ETA 1s
//! ```
//!
//! Columns adapt to the terminal width so lines never wrap; CJK names are
//! measured by display width.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use console::{pad_str, style, Alignment, Term};
use download_service::network_tools::{self, ProgressSink};
use download_service::{Job, Outcome, Reporter, Summary};
use indicatif::{HumanBytes, MultiProgress, ProgressBar, ProgressStyle};

const TICK: Duration = Duration::from_millis(120);
/// Resolution of the overall bar per track.
const PER_TRACK: u64 = 1000;

/// `45s`, `3m07s`, `1h02m`.
pub fn fmt_duration(d: Duration) -> String {
    let s = d.as_secs();
    if s < 60 {
        format!("{s}s")
    } else if s < 3600 {
        format!("{}m{:02}s", s / 60, s % 60)
    } else {
        format!("{}h{:02}m", s / 3600, (s % 3600) / 60)
    }
}

/// Column layout, chosen from the terminal width so lines never wrap
/// (wrapped lines break the redraw). Messages are truncated here, by display
/// width: indicatif's own `wide_msg` truncation slices bytes and gives up on
/// multi-byte characters.
struct Layout {
    cols: usize,
    name_width: usize,
    label_width: usize,
    template: &'static str,
    overall_template: &'static str,
    /// Columns used by the overall line before its message.
    overall_fixed: usize,
}

impl Layout {
    fn new(cols: usize, index_width: usize, longest_name: usize) -> Self {
        // keep the last column free: some terminals wrap when it is written
        let cols = cols.saturating_sub(1).max(30);
        let index = 2 * index_width + 3 + 1; // "[ 3/12] "
        let (fixed, template, label, max_name) = if cols >= 100 {
            (
                index + 1 + 22 + 1 + 19 + 1 + 11 + 1 + 3 + 1,
                "{prefix} {bar:22.cyan/blue} {bytes:>9}/{total_bytes:<9} {binary_bytes_per_sec:>11} {eta:>3} {msg:.dim}",
                16,
                40,
            )
        } else if cols >= 70 {
            (
                index + 1 + 14 + 1 + 4 + 1 + 11 + 1 + 3 + 1,
                "{prefix} {bar:14.cyan/blue} {percent:>3}% {binary_bytes_per_sec:>11} {eta:>3} {msg:.dim}",
                12,
                30,
            )
        } else {
            (
                index + 1 + 10 + 1 + 4 + 1,
                "{prefix} {bar:10.cyan/blue} {percent:>3}% {msg:.dim}",
                9,
                30,
            )
        };
        let name_width = cols
            .saturating_sub(fixed + label)
            .min(longest_name)
            .clamp(6, max_name);
        let (overall_template, overall_bar) = if cols >= 70 {
            (
                "{spinner:.green} {prefix:.bold} {bar:20.green/black.bright} {percent:>3}% {msg}",
                20 + 1 + 4 + 1,
            )
        } else {
            (
                "{spinner:.green} {prefix:.bold} {bar:10.green/black.bright} {msg}",
                10 + 1,
            )
        };
        // spinner + space + "12/120 tracks" + space
        let overall_fixed = 2 + (2 * index_width + 8) + 1 + overall_bar;
        Self {
            cols,
            name_width,
            label_width: cols.saturating_sub(fixed + name_width),
            template,
            overall_template,
            overall_fixed,
        }
    }

    fn fit(text: &str, width: usize) -> String {
        crate::out::fit(text, width)
    }
}

fn term_width() -> usize {
    Term::stderr()
        .size_checked()
        .map(|(_, cols)| cols as usize)
        .unwrap_or(100)
}

fn spinner_style() -> ProgressStyle {
    ProgressStyle::with_template("{spinner:.cyan} {msg}")
        .unwrap()
        .tick_chars("⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏ ")
}

/// A spinner for the phases before downloading (resolving inputs, looking up
/// song titles). Hidden automatically when stderr is not a terminal.
pub fn phase_spinner(message: impl Into<String>) -> ProgressBar {
    let pb = ProgressBar::new_spinner();
    pb.set_style(spinner_style());
    pb.set_message(message.into());
    pb.enable_steady_tick(TICK);
    pb
}

/// Prints a line above any live progress bars (or plainly without a TTY).
fn print_line(multi: &MultiProgress, line: String) {
    if multi.is_hidden() {
        eprintln!("{line}");
    } else {
        let _ = multi.println(line);
    }
}

#[derive(Default)]
struct Stats {
    /// Bytes received in this run (resumed bytes excluded).
    received: AtomicU64,
    /// Size of the files written in this run.
    written: AtomicU64,
    finished: AtomicUsize,
    /// Smoothed download speed in bytes per second.
    speed: AtomicU64,
}

/// Per track state shared between the downloader callbacks and the UI.
struct Track {
    bar: ProgressBar,
    label_width: usize,
    label: Mutex<String>,
    retries: AtomicU32,
    downloading: AtomicBool,
}

impl Track {
    fn refresh_message(&self) {
        let label = self.label.lock().unwrap().clone();
        let retries = self.retries.load(Ordering::Relaxed);
        let text = if retries > 0 {
            format!("{label} ↻{retries}")
        } else {
            label
        };
        self.bar.set_message(Layout::fit(&text, self.label_width));
    }
}

struct Sink {
    track: Arc<Track>,
    stats: Arc<Stats>,
    multi: MultiProgress,
    bar_style: ProgressStyle,
    verbose: bool,
}

impl ProgressSink for Sink {
    fn start(&self, total: Option<u64>, already: u64) {
        let bar = &self.track.bar;
        self.track.downloading.store(true, Ordering::Relaxed);
        bar.set_style(self.bar_style.clone());
        if let Some(t) = total {
            bar.set_length(t);
        }
        bar.set_position(already);
        bar.reset_eta();
        bar.reset_elapsed();
        self.track.refresh_message();
    }

    fn advance(&self, bytes: u64) {
        self.track.bar.inc(bytes);
        self.stats.received.fetch_add(bytes, Ordering::Relaxed);
    }

    fn rewind(&self, bytes: u64) {
        let bar = &self.track.bar;
        bar.set_position(bar.position().saturating_sub(bytes));
        let _ = self
            .stats
            .received
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |v| {
                Some(v.saturating_sub(bytes))
            });
    }

    fn retry(&self, _attempt: u32, error: &network_tools::Error) {
        self.track.retries.fetch_add(1, Ordering::Relaxed);
        self.track.refresh_message();
        if self.verbose {
            let prefix = self.track.bar.prefix();
            print_line(
                &self.multi,
                format!(
                    "  {} {} {}",
                    style("↻").yellow().for_stderr(),
                    prefix.trim(),
                    style(error).dim().for_stderr()
                ),
            );
        }
    }
}

struct Inner {
    multi: MultiProgress,
    overall: ProgressBar,
    tracks: Mutex<HashMap<usize, Arc<Track>>>,
    stats: Arc<Stats>,
    total: usize,
    out_dir: PathBuf,
    layout: Layout,
    index_width: usize,
    verbose: bool,
    started: Instant,
}

pub struct TerminalReporter {
    inner: Arc<Inner>,
    stop: Arc<AtomicBool>,
    ticker: Mutex<Option<std::thread::JoinHandle<()>>>,
}

impl TerminalReporter {
    /// `longest_name`: display width of the longest track name.
    pub fn new(total: usize, longest_name: usize, out_dir: &Path, verbose: bool) -> Self {
        let multi = MultiProgress::new();
        let overall = multi.add(ProgressBar::new(total as u64 * PER_TRACK));
        overall.set_prefix(format!("0/{total} tracks"));

        let index_width = total.to_string().len();
        let cols = term_width();
        let layout = Layout::new(cols, index_width, longest_name);
        overall.set_style(
            ProgressStyle::with_template(layout.overall_template)
                .unwrap()
                .tick_chars("⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏ ")
                .progress_chars("━━─"),
        );

        let inner = Arc::new(Inner {
            multi,
            overall,
            tracks: Mutex::new(HashMap::new()),
            stats: Arc::new(Stats::default()),
            total,
            out_dir: out_dir.to_path_buf(),
            layout,
            index_width,
            verbose,
            started: Instant::now(),
        });
        let stop = Arc::new(AtomicBool::new(false));
        let ticker = {
            let inner = inner.clone();
            let stop = stop.clone();
            std::thread::spawn(move || {
                let mut last = (Instant::now(), 0u64);
                let mut speed = 0f64;
                while !stop.load(Ordering::Relaxed) {
                    std::thread::sleep(TICK);
                    let now = Instant::now();
                    let received = inner.stats.received.load(Ordering::Relaxed);
                    let dt = now.duration_since(last.0).as_secs_f64();
                    if dt > 0.0 {
                        let current = received.saturating_sub(last.1) as f64 / dt;
                        // exponential moving average, ~1.5s window
                        speed = if speed == 0.0 {
                            current
                        } else {
                            speed * 0.92 + current * 0.08
                        };
                    }
                    last = (now, received);
                    inner.stats.speed.store(speed as u64, Ordering::Relaxed);
                    inner.update_overall();
                }
            })
        };
        Self {
            inner,
            stop,
            ticker: Mutex::new(Some(ticker)),
        }
    }

    /// Removes the live bars (call before printing the summary).
    pub fn finish(&self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.ticker.lock().unwrap().take() {
            let _ = t.join();
        }
        for (_, t) in self.inner.tracks.lock().unwrap().drain() {
            t.bar.finish_and_clear();
        }
        self.inner.overall.finish_and_clear();
    }

    pub fn summary_line(&self, summary: &Summary) -> String {
        let s = &self.inner.stats;
        let elapsed = fmt_duration(self.inner.started.elapsed());
        let failed = summary.failed.len();
        let head = if failed == 0 {
            style("✔ Done").green().bold().for_stderr()
        } else {
            style("✘ Done").red().bold().for_stderr()
        };
        let mut parts = vec![format!(
            "{} downloaded ({})",
            summary.downloaded,
            HumanBytes(s.written.load(Ordering::Relaxed))
        )];
        if summary.skipped > 0 {
            parts.push(format!("{} skipped", summary.skipped));
        }
        if failed > 0 {
            parts.push(
                style(format!("{failed} failed"))
                    .red()
                    .for_stderr()
                    .to_string(),
            );
        }
        format!(
            "{head} in {elapsed} — {} → {}",
            parts.join(", "),
            style(self.inner.out_dir.display()).cyan().for_stderr()
        )
    }
}

impl Drop for TerminalReporter {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

impl Inner {
    fn prefix(&self, job: &Job) -> String {
        format!(
            "{} {}",
            style(format!(
                "[{:>w$}/{}]",
                job.index,
                self.total,
                w = self.index_width
            ))
            .dim()
            .for_stderr(),
            pad_str(
                &job.display_name(),
                self.layout.name_width,
                Alignment::Left,
                Some("…")
            )
        )
    }

    fn bar_style(&self) -> ProgressStyle {
        ProgressStyle::with_template(self.layout.template)
            .unwrap()
            .progress_chars("█▉▊▋▌▍▎▏ ")
    }

    fn waiting_style() -> ProgressStyle {
        ProgressStyle::with_template("{prefix} {spinner:.cyan} {msg:.dim}")
            .unwrap()
            .tick_chars("⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏ ")
    }

    /// Overall progress = finished tracks + fractions of the active ones.
    fn update_overall(&self) {
        let finished = self.stats.finished.load(Ordering::Relaxed);
        let mut pos = finished as u64 * PER_TRACK;
        for t in self.tracks.lock().unwrap().values() {
            if t.downloading.load(Ordering::Relaxed) {
                if let Some(len) = t.bar.length().filter(|l| *l > 0) {
                    pos += t.bar.position().min(len) * PER_TRACK / len;
                }
            }
        }
        self.overall
            .set_position(pos.min(self.total as u64 * PER_TRACK));
        self.overall
            .set_prefix(format!("{finished}/{} tracks", self.total));
        let received = self.stats.received.load(Ordering::Relaxed);
        let elapsed = self.started.elapsed();
        let eta = {
            let fraction = pos as f64 / (self.total as u64 * PER_TRACK).max(1) as f64;
            if fraction > 0.02 && fraction < 1.0 {
                let secs = elapsed.as_secs_f64() * (1.0 - fraction) / fraction;
                format!(" · ETA {}", fmt_duration(Duration::from_secs_f64(secs)))
            } else {
                String::new()
            }
        };
        let text = format!(
            "{} · {}/s · {}{}",
            HumanBytes(received),
            HumanBytes(self.stats.speed.load(Ordering::Relaxed)),
            fmt_duration(elapsed),
            eta
        );
        let budget = self.layout.cols.saturating_sub(self.layout.overall_fixed);
        self.overall.set_message(Layout::fit(&text, budget));
    }

    fn relative<'a>(&self, path: &'a Path) -> &'a Path {
        path.strip_prefix(&self.out_dir).unwrap_or(path)
    }
}

impl Reporter for TerminalReporter {
    fn started(&self, job: &Job) -> Arc<dyn ProgressSink> {
        let inner = &self.inner;
        let bar = inner
            .multi
            .insert_before(&inner.overall, ProgressBar::new(0));
        bar.set_style(Inner::waiting_style());
        bar.set_prefix(inner.prefix(job));
        bar.set_message("selecting stream");
        bar.enable_steady_tick(TICK);
        let track = Arc::new(Track {
            bar,
            label_width: inner.layout.label_width,
            label: Mutex::new(String::new()),
            retries: AtomicU32::new(0),
            downloading: AtomicBool::new(false),
        });
        inner
            .tracks
            .lock()
            .unwrap()
            .insert(job.index, track.clone());
        Arc::new(Sink {
            track,
            stats: inner.stats.clone(),
            multi: inner.multi.clone(),
            bar_style: inner.bar_style(),
            verbose: inner.verbose,
        })
    }

    fn status(&self, job: &Job, message: &str) {
        let tracks = self.inner.tracks.lock().unwrap();
        let Some(t) = tracks.get(&job.index) else {
            return;
        };
        if message.starts_with("writing") {
            // download finished: show a spinner while remuxing / tagging
            t.downloading.store(false, Ordering::Relaxed);
            t.bar.set_style(Inner::waiting_style());
            t.bar.set_message("writing tags");
        } else {
            *t.label.lock().unwrap() = message.to_owned();
            t.refresh_message();
        }
    }

    fn finished(&self, job: &Job, result: &download_service::Result<Outcome>) {
        let inner = &self.inner;
        if let Some(t) = inner.tracks.lock().unwrap().remove(&job.index) {
            t.bar.finish_and_clear();
            inner.multi.remove(&t.bar);
        }
        let index = style(format!(
            "[{:>w$}/{}]",
            job.index,
            inner.total,
            w = inner.index_width
        ))
        .dim()
        .for_stderr();
        let line = match result {
            Ok(Outcome::Downloaded {
                path,
                quality,
                codec,
                bytes,
                resumed,
                withheld,
            }) => {
                inner.stats.written.fetch_add(*bytes, Ordering::Relaxed);
                let mut details = vec![
                    quality.clone(),
                    codec.clone(),
                    HumanBytes(*bytes).to_string(),
                ];
                if *resumed > 0 {
                    details.push(format!("resumed {}", HumanBytes(*resumed)));
                }
                // The download is fine; say why it is not the top tier.
                let note = match withheld.first() {
                    Some(q) => format!(
                        "  {}",
                        style(format!(
                            "(best for this account; {} not provided to it)",
                            q.label()
                        ))
                        .yellow()
                        .for_stderr()
                    ),
                    None => String::new(),
                };
                format!(
                    "{} {index} {}  {}{note}",
                    style("✔").green().for_stderr(),
                    inner.relative(path).display(),
                    style(details.join(" · ")).dim().for_stderr()
                )
            }
            Ok(Outcome::Skipped { path }) => format!(
                "{} {index} {}",
                style("•").dim().for_stderr(),
                style(format!(
                    "{} (exists, skipped)",
                    inner.relative(path).display()
                ))
                .dim()
                .for_stderr()
            ),
            Err(e) => format!(
                "{} {index} {} {}: {}",
                style("✘").red().for_stderr(),
                job.display_name(),
                style(format!("({})", job.track.bvid)).dim().for_stderr(),
                style(e).red().for_stderr()
            ),
        };
        print_line(&inner.multi, line);
        inner.stats.finished.fetch_add(1, Ordering::Relaxed);
        inner.update_overall();
    }
}
