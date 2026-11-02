//! Terminal progress display.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use bili_service::network_tools::{self, ProgressSink};
use bili_service::{Job, Outcome, Reporter};
use indicatif::{HumanBytes, MultiProgress, ProgressBar, ProgressStyle};

pub struct TerminalReporter {
    multi: MultiProgress,
    overall: ProgressBar,
    bars: Mutex<HashMap<usize, ProgressBar>>,
    verbose: bool,
}

struct BarSink {
    bar: ProgressBar,
    verbose: bool,
    multi: MultiProgress,
}

impl ProgressSink for BarSink {
    fn start(&self, total: Option<u64>, already: u64) {
        if let Some(t) = total {
            self.bar.set_length(t);
        }
        self.bar.set_position(already);
        self.bar.reset_eta();
        if already > 0 {
            self.bar
                .set_message(format!("resumed at {}", HumanBytes(already)));
        }
    }
    fn advance(&self, bytes: u64) {
        self.bar.inc(bytes);
    }
    fn rewind(&self, bytes: u64) {
        self.bar
            .set_position(self.bar.position().saturating_sub(bytes));
    }
    fn retry(&self, attempt: u32, error: &network_tools::Error) {
        self.bar.set_message(format!("retry #{attempt}"));
        if self.verbose {
            let msg = format!("  retry #{attempt}: {error}");
            if self.multi.is_hidden() {
                eprintln!("{msg}");
            } else {
                let _ = self.multi.println(msg);
            }
        }
    }
}

fn shorten(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_owned()
    } else {
        let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
        out.push('…');
        out
    }
}

impl TerminalReporter {
    pub fn new(total: usize, verbose: bool) -> Self {
        let multi = MultiProgress::new();
        let overall = multi.add(ProgressBar::new(total as u64));
        overall.set_style(
            ProgressStyle::with_template("{prefix:.bold} [{pos}/{len}] {wide_msg}").unwrap(),
        );
        overall.set_prefix("total");
        overall.enable_steady_tick(Duration::from_millis(250));
        Self {
            multi,
            overall,
            bars: Mutex::new(HashMap::new()),
            verbose,
        }
    }

    pub fn println(&self, msg: impl AsRef<str>) {
        // indicatif drops output when not attached to a terminal
        if self.multi.is_hidden() {
            eprintln!("{}", msg.as_ref());
        } else {
            let _ = self.multi.println(msg.as_ref());
        }
    }

    pub fn finish(&self) {
        self.overall.finish_and_clear();
    }
}

impl Reporter for TerminalReporter {
    fn started(&self, job: &Job) -> Arc<dyn ProgressSink> {
        let bar = self.multi.insert_before(&self.overall, ProgressBar::new(0));
        bar.set_style(
            ProgressStyle::with_template(
                "{prefix} {bar:24.cyan/blue} {bytes:>10}/{total_bytes:<10} {bytes_per_sec:>11} {msg}",
            )
            .unwrap()
            .progress_chars("█▉▊▋▌▍▎▏ "),
        );
        bar.set_prefix(format!(
            "[{:>w$}/{}] {:<28}",
            job.index,
            job.total,
            shorten(&job.display_name(), 28),
            w = job.total.to_string().len()
        ));
        bar.enable_steady_tick(Duration::from_millis(200));
        self.bars.lock().unwrap().insert(job.index, bar.clone());
        Arc::new(BarSink {
            bar,
            verbose: self.verbose,
            multi: self.multi.clone(),
        })
    }

    fn status(&self, job: &Job, message: &str) {
        if let Some(bar) = self.bars.lock().unwrap().get(&job.index) {
            bar.set_message(message.to_owned());
        }
    }

    fn finished(&self, job: &Job, result: &bili_service::Result<Outcome>) {
        if let Some(bar) = self.bars.lock().unwrap().remove(&job.index) {
            bar.finish_and_clear();
            self.multi.remove(&bar);
        }
        let line = match result {
            Ok(Outcome::Downloaded {
                path,
                quality,
                codec,
                bytes,
                resumed,
            }) => {
                let resumed = if *resumed > 0 {
                    format!(", resumed {}", HumanBytes(*resumed))
                } else {
                    String::new()
                };
                format!(
                    "✔ [{}/{}] {} ({quality}, {codec}, {}{resumed})",
                    job.index,
                    job.total,
                    path.display(),
                    HumanBytes(*bytes)
                )
            }
            Ok(Outcome::Skipped { path }) => {
                format!(
                    "• [{}/{}] exists, skipped: {}",
                    job.index,
                    job.total,
                    path.display()
                )
            }
            Err(e) => format!(
                "✘ [{}/{}] {} ({}): {e}",
                job.index,
                job.total,
                job.display_name(),
                job.track.bvid
            ),
        };
        self.println(line);
        self.overall.inc(1);
    }
}
