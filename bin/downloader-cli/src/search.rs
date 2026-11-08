//! `dc search`: result table and interactive selection.

use std::io::{IsTerminal, Write};

use console::{style, Alignment, Term};
use download_service::naming::format_date;
use download_service::source_api::{SearchItem, SearchOptions, SearchPage};
use download_service::{PageSelection, Service};

use crate::out::{self, cell};

/// `5369393` → `536.9万`.
pub(crate) fn fmt_count(n: u64) -> String {
    let short = |v: f64, unit: &str| {
        let s = format!("{v:.1}");
        format!("{}{unit}", s.strip_suffix(".0").unwrap_or(&s))
    };
    match n {
        0..=9_999 => n.to_string(),
        10_000..=9_999_999 => short(n as f64 / 1e4, "万"),
        // keep it within 7 columns: 1768.5万 → 1768万
        10_000_000..=99_999_999 => format!("{}万", n / 10_000),
        _ => short(n as f64 / 1e8, "亿"),
    }
}

pub(crate) fn fmt_secs(s: u64) -> String {
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, (s % 3600) / 60, s % 60)
    } else {
        format!("{}:{:02}", s / 60, s % 60)
    }
}

fn styled_title(item: &SearchItem) -> String {
    item.title_parts
        .iter()
        .map(|(text, hit)| {
            if *hit {
                style(text).yellow().bold().to_string()
            } else {
                text.clone()
            }
        })
        .collect()
}

fn print_page(keyword: &str, page: &SearchPage) {
    let cols = Term::stdout()
        .size_checked()
        .map(|(_, c)| c as usize)
        .unwrap_or(120)
        .saturating_sub(1);
    let show_bvid = cols >= 100;
    let show_date = cols >= 80;
    // "##  " + title + "  UP(14)  dur(7)  plays(7)  [date(10)]  [bvid(12)]"
    let fixed = 4
        + 2
        + 14
        + 2
        + 7
        + 2
        + 7
        + if show_date { 12 } else { 0 }
        + if show_bvid { 14 } else { 0 };
    let title_w = cols.saturating_sub(fixed).clamp(16, 70);

    out::line(format!(
        "{} {}  {}",
        style("Search").bold(),
        style(format!("“{keyword}”")).cyan(),
        style(format!(
            "page {}/{} · {} results",
            page.page,
            page.pages.max(1),
            page.total
        ))
        .dim()
    ));
    let mut header = format!(
        "{:>2}  {}  {}  {}  {}",
        "#",
        cell("title", title_w, Alignment::Left),
        cell("UP", 14, Alignment::Left),
        cell("length", 7, Alignment::Right),
        cell("plays", 7, Alignment::Right),
    );
    if show_date {
        header += &format!("  {}", cell("date", 10, Alignment::Left));
    }
    if show_bvid {
        header += &format!("  {}", cell("bvid", 12, Alignment::Left));
    }
    out::line(style(header).dim());

    for (i, item) in page.items.iter().enumerate() {
        let mut line = format!(
            "{}  {}  {}  {}  {}",
            style(format!("{:>2}", i + 1)).cyan().bold(),
            cell(&styled_title(item), title_w, Alignment::Left),
            cell(&item.author, 14, Alignment::Left),
            cell(&fmt_secs(item.duration_secs), 7, Alignment::Right),
            cell(&fmt_count(item.plays), 7, Alignment::Right),
        );
        if show_date {
            let date = format_date(item.pubdate).unwrap_or_default();
            line += &format!("  {}", style(cell(&date, 10, Alignment::Left)).dim());
        }
        if show_bvid {
            line += &format!("  {}", style(cell(&item.bvid, 12, Alignment::Left)).dim());
        }
        out::line(line);
    }
    if page.items.is_empty() {
        out::line(style("  (no results)").dim());
    }
}

/// Maps a selection like `1,3-5` / `all` to the items of the page.
fn select(page: &SearchPage, input: &str) -> Result<Vec<String>, String> {
    let input = input.trim();
    if matches!(input, "a" | "all" | "*") {
        return Ok(page.items.iter().map(|i| i.bvid.clone()).collect());
    }
    let sel: PageSelection = input.parse()?;
    let picked: Vec<String> = page
        .items
        .iter()
        .enumerate()
        .filter(|(i, _)| sel.contains(*i as u32 + 1))
        .map(|(_, item)| item.bvid.clone())
        .collect();
    if picked.is_empty() {
        return Err(format!("no result numbered {input} on this page"));
    }
    Ok(picked)
}

async fn read_line(prompt: String) -> std::io::Result<Option<String>> {
    tokio::task::spawn_blocking(move || {
        print!("{prompt}");
        std::io::stdout().flush()?;
        let mut line = String::new();
        let n = std::io::stdin().read_line(&mut line)?;
        Ok((n > 0).then_some(line))
    })
    .await
    .map_err(std::io::Error::other)?
}

/// Shows search results and returns the BV ids chosen for download.
///
/// * `pick` selects non-interactively;
/// * in a capable terminal a full screen picker is used ([`crate::tui`]);
/// * `plain`, `TERM=dumb` or a terminal without raw mode / alternate screen
///   support get a line based prompt (answers confirmed with Enter).
pub async fn browse(
    service: &Service,
    keyword: &str,
    mut opts: SearchOptions,
    pick: Option<&str>,
    plain: bool,
) -> anyhow::Result<Vec<String>> {
    let interactive =
        pick.is_none() && std::io::stdin().is_terminal() && std::io::stdout().is_terminal();
    let dumb = std::env::var("TERM").is_ok_and(|t| t == "dumb");
    if interactive && !plain && !dumb {
        if let Some(items) =
            crate::tui::pick(service.shared_client(), keyword, opts.clone()).await?
        {
            if !items.is_empty() {
                out::line(style(format!("Selected {}:", items.len())).bold());
                for item in &items {
                    out::line(format!("  {}  {}", style(&item.bvid).dim(), item.title));
                }
            }
            return Ok(items.into_iter().map(|i| i.bvid).collect());
        }
        // the terminal could not host the full screen picker
    }
    if keyword.trim().is_empty() {
        anyhow::bail!("give a search keyword");
    }
    opts.page = opts.page.max(1);
    let mut page = service.client().search_videos(keyword, &opts).await?;
    loop {
        print_page(keyword, &page);
        if let Some(pick) = pick {
            return select(&page, pick).map_err(anyhow::Error::msg);
        }
        if !interactive {
            out::line(format!(
                "{}",
                style(
                    "download with --pick 1,3-5 (or `dc get <bvid>`); more results with --page N"
                )
                .dim()
            ));
            return Ok(Vec::new());
        }
        loop {
            let prompt = format!(
                "\n{} {} ",
                style("Download which?").bold(),
                style("[1,3-5 / all · n next · p previous · q quit] ›").dim()
            );
            let Some(line) = read_line(prompt).await? else {
                return Ok(Vec::new()); // EOF
            };
            let answer = line.trim().to_ascii_lowercase();
            let mut turn = |to: u32| opts.page = to;
            match answer.as_str() {
                "" | "q" | "quit" | "exit" => return Ok(Vec::new()),
                "n" | "next" if page.page < page.pages => turn(page.page + 1),
                "p" | "prev" | "previous" if page.page > 1 => turn(page.page - 1),
                "n" | "next" | "p" | "prev" | "previous" => {
                    out::line(style("no more pages in that direction").yellow());
                    continue;
                }
                other => match select(&page, other) {
                    Ok(ids) => return Ok(ids),
                    Err(e) => {
                        out::line(style(e).yellow());
                        continue;
                    }
                },
            }
            out::line("");
            page = service.client().search_videos(keyword, &opts).await?;
            break;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn page() -> SearchPage {
        let item = |b: &str| SearchItem {
            aid: 1,
            bvid: b.into(),
            title: "t".into(),
            title_parts: vec![("t".into(), false)],
            author: "a".into(),
            mid: 1,
            duration_secs: 1,
            plays: 1,
            pubdate: 0,
            category: String::new(),
        };
        SearchPage {
            page: 1,
            pages: 2,
            total: 40,
            items: vec![item("BV1"), item("BV2"), item("BV3")],
        }
    }

    #[test]
    fn selection_and_formatting() {
        let p = page();
        assert_eq!(select(&p, "1,3").unwrap(), vec!["BV1", "BV3"]);
        assert_eq!(select(&p, "all").unwrap().len(), 3);
        assert!(select(&p, "7").is_err());
        assert!(select(&p, "x").is_err());
        assert_eq!(fmt_count(9_999), "9999");
        assert_eq!(fmt_count(5_369_393), "536.9万");
        assert_eq!(fmt_count(120_000), "12万");
        assert_eq!(fmt_count(230_000_000), "2.3亿");
        assert_eq!(fmt_count(17_685_000), "1768万");
        assert_eq!(fmt_secs(238), "3:58");
        assert_eq!(fmt_secs(3723), "1:02:03");
        assert_eq!(
            console::measure_text_width(&cell("一样的月光测试", 6, Alignment::Left)),
            6
        );
    }
}
