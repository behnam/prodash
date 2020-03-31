use crate::{
    tree::{Key, Progress, ProgressState, ProgressStep, Value},
    tui::{
        draw::{time::format_now_datetime_seconds, State},
        utils::{
            block_width, draw_text_nowrap, draw_text_nowrap_fn, rect, sanitize_offset,
            GraphemeCountWriter, VERTICAL_LINE,
        },
        InterruptDrawInfo,
    },
};
use humantime::format_duration;
use std::{
    fmt,
    time::{Duration, SystemTime},
};
use tui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Style},
};
use tui_react::fill_background;

const MIN_TREE_WIDTH: u16 = 20;

pub fn pane(entries: &[(Key, Value)], mut bound: Rect, buf: &mut Buffer, state: &mut State) {
    state.task_offset = sanitize_offset(state.task_offset, entries.len(), bound.height);
    let needs_overflow_line = if entries.len() > bound.height as usize
        || (state.task_offset).min(entries.len() as u16) > 0
    {
        bound.height = bound.height.saturating_sub(1);
        true
    } else {
        false
    };
    state.task_offset = sanitize_offset(state.task_offset, entries.len(), bound.height);

    if entries.is_empty() {
        return;
    }

    let initial_column_width = bound.width / 2;
    let desired_max_tree_draw_width = *state
        .next_tree_column_width
        .as_ref()
        .unwrap_or(&initial_column_width);
    {
        if initial_column_width >= MIN_TREE_WIDTH {
            let tree_bound = Rect {
                width: desired_max_tree_draw_width,
                ..bound
            };
            let computed = draw_tree(entries, buf, tree_bound, state.task_offset);
            state.last_tree_column_width = Some(computed);
        } else {
            state.last_tree_column_width = Some(0);
        };
    }

    {
        let progress_area = rect::offset_x(bound, desired_max_tree_draw_width);
        draw_progress(
            entries,
            buf,
            progress_area,
            if desired_max_tree_draw_width == 0 {
                false
            } else {
                true
            },
            state.task_offset,
        );
    }

    if needs_overflow_line {
        let overflow_rect = Rect {
            y: bound.height + 1,
            height: 1,
            ..bound
        };
        draw_overflow(
            entries,
            buf,
            overflow_rect,
            desired_max_tree_draw_width,
            bound.height,
            state.task_offset,
        );
    }
}

pub(crate) fn headline(
    entries: &[(Key, Value)],
    interrupt_mode: InterruptDrawInfo,
    duration_per_frame: Duration,
    buf: &mut Buffer,
    bound: Rect,
) {
    let (num_running_tasks, num_blocked_tasks, num_groups) = entries.iter().fold(
        (0, 0, 0),
        |(mut running, mut blocked, mut groups), (_key, Value { progress, .. })| {
            match progress.map(|p| p.state) {
                Some(ProgressState::Running) => running += 1,
                Some(ProgressState::Blocked(_, _)) | Some(ProgressState::Halted(_, _)) => {
                    blocked += 1
                }
                None => groups += 1,
            }
            (running, blocked, groups)
        },
    );
    let text = format!(
        " {} {} {:3} running + {:3} blocked + {:3} groups = {} ",
        match interrupt_mode {
            InterruptDrawInfo::Instantly => "'q' or CTRL+c to quit",
            InterruptDrawInfo::Deferred(interrupt_requested) => {
                if interrupt_requested {
                    "interrupt requested - please wait"
                } else {
                    "cannot interrupt current operation"
                }
            }
        },
        if duration_per_frame > Duration::from_secs(1) {
            format!(
                " Every {}s → {}",
                duration_per_frame.as_secs(),
                format_now_datetime_seconds()
            )
        } else {
            "".into()
        },
        num_running_tasks,
        num_blocked_tasks,
        num_groups,
        entries.len()
    );
    draw_text_nowrap(
        rect::snap_to_right(bound, block_width(&text) + 1),
        buf,
        text,
        None,
    );
}

struct ProgressFormat<'a>(&'a Option<Progress>, u16);

impl<'a> fmt::Display for ProgressFormat<'a> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0 {
            Some(p) => {
                match p.done_at {
                    Some(done_at) => write!(f, "{} / {}", p.step, done_at),
                    None => write!(f, "{}", p.step),
                }?;
                if let Some(unit) = p.unit {
                    write!(f, " {}", unit)?;
                }
                Ok(())
            }
            None => write!(f, "{:─<width$}", '─', width = self.1 as usize),
        }
    }
}

pub fn draw_progress(
    entries: &[(Key, Value)],
    buf: &mut Buffer,
    bound: Rect,
    draw_column_line: bool,
    offset: u16,
) {
    let title_spacing = 2u16 + 1; // 2 on the left, 1 on the right
    let column_line_width = if draw_column_line { 1 } else { 0 };
    let max_progress_label_width = entries
        .iter()
        .skip(offset as usize)
        .take(bound.height as usize)
        .map(|(_, Value { progress, .. })| progress)
        .fold(0, |state, progress| match progress {
            progress @ Some(_) => {
                use std::io::Write;
                let mut w = GraphemeCountWriter::default();
                write!(w, "{}", ProgressFormat(progress, 0)).expect("never fails");
                state.max(w.0)
            }
            None => state,
        });

    let mut prev_level = None;
    let mut entries_iter = entries
        .iter()
        .skip(offset as usize)
        .take(bound.height as usize)
        .enumerate()
        .peekable();
    while let Some((
        line,
        (
            key,
            Value {
                progress,
                name: title,
            },
        ),
    )) = entries_iter.next()
    {
        let line_bound = rect::line_bound(bound, line);
        let progress_text = format!(
            " {progress}",
            progress = ProgressFormat(progress, bound.width.saturating_sub(title_spacing))
        );

        draw_text_nowrap(line_bound, buf, VERTICAL_LINE, None);

        let tree_prefix = level_prefix(
            prev_level,
            key.level(),
            entries_iter.peek().map(|e| ((e.1).0).level()).unwrap_or(0),
        );
        prev_level = Some(key.level());

        let progress_rect = rect::offset_x(
            line_bound,
            (column_line_width + block_width(&tree_prefix)) as u16,
        );
        draw_text_nowrap(rect::offset_x(line_bound, column_line_width), buf, tree_prefix, None);
        match progress.map(|p| (p.fraction(), p.state, p.step)) {
            Some((Some(fraction), state, _step)) => {
                let mut progress_text = progress_text;
                add_block_eta(state, &mut progress_text);
                let (bound, style) =
                    draw_progress_bar_fn(buf, progress_rect, fraction, |fraction| match state {
                        ProgressState::Blocked(_, _) => Color::Red,
                        ProgressState::Halted(_, _) => Color::LightRed,
                        ProgressState::Running => {
                            if fraction >= 0.8 {
                                Color::Green
                            } else {
                                Color::Yellow
                            }
                        }
                    });
                let style_fn = move |_t: &str, x: u16, _y: u16| {
                    if x < bound.right() {
                        style
                    } else {
                        Style::default()
                    }
                };
                draw_text_nowrap_fn(progress_rect, buf, progress_text, style_fn);
            }
            Some((None, state, step)) => {
                let mut progress_text = progress_text;
                add_block_eta(state, &mut progress_text);
                draw_text_nowrap(progress_rect, buf, progress_text, None);
                let bar_rect = rect::offset_x(line_bound, max_progress_label_width as u16);
                draw_spinner(
                    buf,
                    bar_rect,
                    step,
                    line,
                    match state {
                        ProgressState::Blocked(_, _) => Color::Red,
                        ProgressState::Halted(_, _) => Color::LightRed,
                        ProgressState::Running => Color::White,
                    },
                );
            }
            None => {
                draw_text_nowrap(progress_rect, buf, progress_text, None);
                draw_text_nowrap(progress_rect, buf, format!(" {} ", title), None);
            }
        }
    }
}

fn add_block_eta(state: ProgressState, progress_text: &mut String) {
    match state {
        ProgressState::Blocked(reason, maybe_eta) | ProgressState::Halted(reason, maybe_eta) => {
            progress_text.push_str(" [");
            progress_text.push_str(reason);
            progress_text.push_str("]");
            if let Some(eta) = maybe_eta {
                let now = SystemTime::now();
                if eta > now {
                    progress_text.push_str(&format!(
                        " → {} to {}",
                        format_duration(eta.duration_since(now).expect("computation to work")),
                        if let ProgressState::Blocked(_, _) = state {
                            "unblock"
                        } else {
                            "continue"
                        }
                    ))
                }
            }
        }
        ProgressState::Running => {}
    }
}

fn draw_spinner(buf: &mut Buffer, bound: Rect, step: ProgressStep, seed: usize, color: Color) {
    if bound.width == 0 {
        return;
    }
    let step = step as usize;
    let x = bound.x + ((step + seed) % bound.width as usize) as u16;
    let width = 5;
    let bound = rect::intersect(Rect { x, width, ..bound }, bound);
    tui_react::fill_background(bound, buf, color);
}

fn draw_progress_bar_fn(
    buf: &mut Buffer,
    bound: Rect,
    fraction: f32,
    style: impl FnOnce(f32) -> Color,
) -> (Rect, Style) {
    if bound.width == 0 {
        return (Rect::default(), Style::default());
    }
    let fractional_progress_rect = Rect {
        width: ((bound.width as f32 * fraction).ceil() as u16).min(bound.width),
        ..bound
    };
    let color = style(fraction);
    tui_react::fill_background(fractional_progress_rect, buf, color);
    (
        fractional_progress_rect,
        Style::default().bg(color).fg(Color::Black),
    )
}

pub fn draw_tree(entries: &[(Key, Value)], buf: &mut Buffer, bound: Rect, offset: u16) -> u16 {
    let mut max_prefix_len = 0;
    let mut peekable = entries
        .iter()
        .skip(offset as usize)
        .take(bound.height as usize)
        .enumerate()
        .peekable();
    let mut prev_level = None;
    while let Some((line, entry)) = peekable.next() {
        let line_bound = rect::line_bound(bound, line);
        let tree_prefix = to_tree_prefix(
            entry,
            prev_level,
            peekable.peek().map(|e| ((e.1).0).level()).unwrap_or(0),
        );
        prev_level = Some(entry.0.level());
        max_prefix_len = max_prefix_len.max(block_width(&tree_prefix));
        draw_text_nowrap(line_bound, buf, tree_prefix, None);
    }
    max_prefix_len
}

fn level_prefix(prev_level: Option<u8>, cur_level: u8, next_level: u8) -> String {
    format!(
        "{:>width$}",
        match (prev_level, cur_level, next_level) {
            (Some(prev), cur, next) => match (prev, cur) {
                (prev, cur) if cur > prev =>
                    if next == cur {
                        "├" // sibling no same level
                    } else {
                        "└" // last child on this level
                    },
                (prev, cur) if cur == prev =>
                    if next == cur {
                        "├" // sibling no same level
                    } else {
                        "└" // last child on this level
                    },
                (prev, cur) if cur < prev => "",
                _ => "?",
            },
            _ => "",
        },
        width = (cur_level.saturating_sub(1) * 2) as usize
    )
}

fn to_tree_prefix(entry: &(Key, Value), prev_level: Option<u8>, next_level: u8) -> String {
    let (
        key,
        Value {
            progress: _,
            name: title,
        },
    ) = entry;
    let cur_level = key.level();

    format!(
        "{} {} ",
        level_prefix(prev_level, cur_level, next_level),
        title,
    )
}

pub fn draw_overflow<'a>(
    entries: &[(Key, Value)],
    buf: &mut Buffer,
    bound: Rect,
    label_offset: u16,
    num_entries_on_display: u16,
    offset: u16,
) {
    let (count, mut progress_fraction) = entries
        .iter()
        .take(offset as usize)
        .chain(
            entries
                .iter()
                .skip((offset + num_entries_on_display) as usize),
        )
        .fold(
            (0usize, 0f32),
            |(count, progress_fraction), (_key, value)| {
                let progress = value
                    .progress
                    .and_then(|p| p.fraction())
                    .unwrap_or_default();
                (count + 1, progress_fraction + progress)
            },
        );
    progress_fraction /= count as f32;
    let label = format!(
        "{} …{} skipped and {} more",
        if label_offset == 0 { "" } else { VERTICAL_LINE },
        offset,
        entries
            .len()
            .saturating_sub((offset + num_entries_on_display + 1) as usize)
    );
    let (progress_rect, style) =
        draw_progress_bar_fn(buf, bound, progress_fraction, |_| Color::Green);

    let bg_color = Color::Red;
    fill_background(
        rect::offset_x(bound, progress_rect.right() - 1),
        buf,
        bg_color,
    );
    let color_text_according_to_progress = move |_g: &str, x: u16, _y: u16| {
        if x < progress_rect.right() {
            style
        } else {
            style.bg(bg_color)
        }
    };
    draw_text_nowrap_fn(
        rect::offset_x(bound, label_offset),
        buf,
        label,
        color_text_according_to_progress,
    );
    let help_text = "⇊ = d|↓ = j|⇈ = u|↑ = k ";
    draw_text_nowrap_fn(
        rect::snap_to_right(bound, block_width(help_text)),
        buf,
        help_text,
        color_text_according_to_progress,
    );
}