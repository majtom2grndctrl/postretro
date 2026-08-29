//! Ratatui rendering of compiler steps, progress, logs, and controls.
//! See: `context/lib/build_pipeline.md`.

use ratatui::layout::{Constraint, Direction, Layout, Margin, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Padding, Paragraph, Wrap};

use crate::governor::Governor;
use crate::logger::CapturedRecord;

use super::tui_progress::progress_text;
use super::tui_steps::draw_steps;
use super::{LogScroll, StepState, StepStatus, TuiReporter, TuiState};

const MIN_FULL_WIDTH: u16 = 68;
const MIN_FULL_HEIGHT: u16 = 18;
pub(super) const PRIMARY: Color = Color::Cyan;
pub(super) const SECONDARY: Color = Color::Blue;
const DIVIDER: Color = Color::Gray;
pub(super) const MUTED: Color = Color::DarkGray;
const PROGRESS_FILLED: &str = "\u{2588}";
const PROGRESS_EMPTY: &str = "\u{2591}";
const CORE_LEGEND_COLUMN: usize = 17;
const CONTROL_LEGEND: &str =
    "[space] pause    [\u{2190}/\u{2192}] cores    [PgUp/PgDn] log    [q] leave UI";

pub(super) fn core_capacity_spans(
    permits: usize,
    max_permits: usize,
    available_width: u16,
) -> Vec<Span<'static>> {
    let max_permits = max_permits.max(1);
    let permits = permits.clamp(1, max_permits);
    let label = "Cores";
    let marker_width = usize::from(available_width)
        .saturating_sub(label.chars().count())
        .saturating_sub(2);
    let marker_slots = (marker_width.saturating_add(1) / 2).min(max_permits);
    let mut spans = vec![Span::raw(label)];
    if marker_slots == 0 {
        return spans;
    }

    let filled = if marker_slots == max_permits {
        permits
    } else {
        ((permits as u128 * marker_slots as u128).div_ceil(max_permits as u128)) as usize
    };
    spans.push(Span::raw("  "));
    for index in 0..marker_slots {
        if index > 0 {
            spans.push(Span::raw(" "));
        }
        let style = if index < filled {
            Style::default()
                .fg(Color::Gray)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(MUTED).add_modifier(Modifier::DIM)
        };
        spans.push(Span::styled(
            if index < filled {
                "\u{25a0}"
            } else {
                "\u{25a1}"
            },
            style,
        ));
    }
    spans
}

pub(super) fn draw(
    frame: &mut ratatui::Frame<'_>,
    reporter: &TuiReporter,
    governor: &Governor,
    max_permits: usize,
    phase: TuiPhase,
) {
    let area = frame.area();
    let mut state = reporter.lock();
    for step in &mut state.steps {
        if step.status == StepStatus::Active {
            step.observe_liveness();
        }
    }

    if layout_mode(area) == LayoutMode::Compact {
        draw_compact(
            frame,
            area,
            &mut state,
            reporter.logs.warning_count(),
            governor,
            max_permits,
            phase,
        );
    } else {
        draw_full(frame, area, &mut state, governor, max_permits, phase);
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum TuiPhase {
    Running,
    Done,
    Failed,
}

impl TuiPhase {
    fn status_label(self, governor: &Governor) -> &'static str {
        match self {
            Self::Running if governor.is_paused() => "PAUSED",
            Self::Running => "RUNNING",
            Self::Done => "DONE",
            Self::Failed => "FAILED",
        }
    }

    fn completion_message(self) -> Option<&'static str> {
        match self {
            Self::Running => None,
            Self::Done => Some("DONE - Press any key to exit"),
            Self::Failed => Some("FAILED - Press any key to exit"),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LayoutMode {
    Full,
    Compact,
}

fn layout_mode(area: Rect) -> LayoutMode {
    if area.width < MIN_FULL_WIDTH || area.height < MIN_FULL_HEIGHT {
        LayoutMode::Compact
    } else {
        LayoutMode::Full
    }
}

fn draw_full(
    frame: &mut ratatui::Frame<'_>,
    area: Rect,
    state: &mut TuiState,
    governor: &Governor,
    max_permits: usize,
    phase: TuiPhase,
) {
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Min(8),
            Constraint::Length(3),
        ])
        .split(area);
    let body = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(32), Constraint::Min(24)])
        .split(vertical[1]);

    draw_steps(frame, body[0], state);
    draw_logs(frame, body[1], state);
    draw_controls(frame, vertical[2], governor, max_permits, phase);
}

pub(super) fn divider_style() -> Style {
    Style::default().fg(DIVIDER).add_modifier(Modifier::DIM)
}

pub(super) fn progress_bar_line(step: &StepState, width: u16) -> Option<Line<'static>> {
    let progress = step.progress.as_ref()?;
    let total = progress.total()?;
    let track_width = usize::from(width.saturating_sub(2));
    if track_width < 2 {
        return None;
    }

    let done = progress.completed().min(total);
    let filled = if total == 0 {
        track_width
    } else {
        ((done as u128 * track_width as u128) / total as u128) as usize
    };
    let empty = track_width.saturating_sub(filled);
    Some(Line::from(vec![
        Span::styled("[", Style::default().fg(MUTED)),
        Span::styled(PROGRESS_FILLED.repeat(filled), Style::default().fg(PRIMARY)),
        Span::styled(PROGRESS_EMPTY.repeat(empty), Style::default().fg(MUTED)),
        Span::styled("]", Style::default().fg(MUTED)),
    ]))
}

fn draw_logs(frame: &mut ratatui::Frame<'_>, area: Rect, state: &mut TuiState) {
    let log_block = Block::default()
        .title(Span::styled(
            " Build log ",
            Style::default().fg(PRIMARY).add_modifier(Modifier::BOLD),
        ))
        .padding(Padding::horizontal(2));
    let log_inner = log_block.inner(area);
    let lines = state.logs.iter().map(log_line).collect::<Vec<_>>();
    frame.render_widget(log_block, area);
    let paragraph =
        scrollable_log_paragraph(lines, log_inner, state.log_revision, &mut state.log_scroll);
    frame.render_widget(paragraph, log_inner);
}

fn log_line(record: &CapturedRecord) -> Line<'static> {
    let prefix_style = if record.level <= log::Level::Warn {
        Style::default().fg(MUTED).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(MUTED)
    };
    Line::from(vec![
        Span::styled(
            format!("[{} {}] ", record.level, record.target),
            prefix_style,
        ),
        Span::styled(record.message.clone(), Style::default().fg(MUTED)),
    ])
}

fn scrollable_log_paragraph(
    lines: Vec<Line<'static>>,
    area: Rect,
    revision: usize,
    scroll_state: &mut LogScroll,
) -> Paragraph<'static> {
    let paragraph = Paragraph::new(lines).wrap(Wrap { trim: true });
    let line_count = paragraph.line_count(area.width);
    let scroll = scroll_state.paragraph_offset(line_count, area.height as usize, revision);
    paragraph.scroll((scroll, 0))
}

fn draw_controls(
    frame: &mut ratatui::Frame<'_>,
    area: Rect,
    governor: &Governor,
    max_permits: usize,
    phase: TuiPhase,
) {
    let status = phase.status_label(governor);
    let block = Block::default()
        .borders(Borders::TOP)
        .border_style(divider_style())
        .padding(Padding::horizontal(2));
    let inner = block.inner(area).inner(Margin {
        horizontal: 0,
        vertical: 0,
    });
    frame.render_widget(block, area);
    let lines = if let Some(message) = phase.completion_message() {
        vec![
            Line::default(),
            Line::from(Span::styled(
                message,
                Style::default().fg(PRIMARY).add_modifier(Modifier::BOLD),
            )),
        ]
    } else {
        let status_text = format!("{status:<CORE_LEGEND_COLUMN$}");
        let mut status_line = vec![Span::styled(
            status_text.clone(),
            Style::default().fg(PRIMARY).add_modifier(Modifier::BOLD),
        )];
        status_line.extend(core_capacity_spans(
            governor.permits(),
            max_permits,
            inner
                .width
                .saturating_sub(status_text.chars().count() as u16),
        ));
        vec![
            Line::from(status_line),
            Line::from(Span::styled(CONTROL_LEGEND, Style::default().fg(SECONDARY))),
        ]
    };
    frame.render_widget(Paragraph::new(lines), inner);
}

fn draw_compact(
    frame: &mut ratatui::Frame<'_>,
    area: Rect,
    state: &mut TuiState,
    warning_count: usize,
    governor: &Governor,
    max_permits: usize,
    phase: TuiPhase,
) {
    let (content_area, completion_area) = if phase.completion_message().is_some() {
        let sections = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(1), Constraint::Length(1)])
            .split(area);
        (sections[0], Some(sections[1]))
    } else {
        (area, None)
    };
    let focused = state
        .active_index()
        .or_else(|| {
            state
                .steps
                .iter()
                .position(|step| step.status == StepStatus::Failed)
        })
        .and_then(|index| state.steps.get(index));
    let mut header_lines = Vec::new();
    header_lines.push(Line::from(vec![
        Span::styled(
            "prl-build  ",
            Style::default().fg(PRIMARY).add_modifier(Modifier::BOLD),
        ),
        Span::raw(phase.status_label(governor)),
        Span::raw(format!("  warnings {warning_count}")),
    ]));
    header_lines.push(Line::from(core_capacity_spans(
        governor.permits(),
        max_permits,
        content_area.width.saturating_sub(4),
    )));
    if let Some(step) = focused {
        let (marker, marker_style) = if step.status == StepStatus::Failed {
            (
                "!  ",
                Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
            )
        } else {
            (
                ACTIVITY_FRAMES[step.activity_index],
                Style::default().fg(PRIMARY),
            )
        };
        header_lines.push(Line::from(vec![
            Span::styled(format!("{marker}  "), marker_style),
            Span::styled(step.label, Style::default().add_modifier(Modifier::BOLD)),
        ]));
        if step.status == StepStatus::Active {
            let (progress, eta) = progress_text(step);
            header_lines.push(Line::from(format!("{progress}  {eta}")));
            if let Some(bar) = progress_bar_line(step, content_area.width.saturating_sub(4)) {
                header_lines.push(bar);
            }
        }
    }
    let block = Block::default().padding(Padding::horizontal(2));
    let inner = block.inner(content_area);
    frame.render_widget(block, content_area);

    // Compact mode keeps operational state fixed while only the log viewport
    // scrolls. Reserve a separator row when both regions fit; tiny terminals
    // simply clip the fixed header rather than mixing it into log scrolling.
    let header_height = (header_lines.len() as u16).min(inner.height);
    let separator_height = u16::from(inner.height > header_height.saturating_add(1));
    let regions = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(header_height),
            Constraint::Length(separator_height),
            Constraint::Min(0),
        ])
        .split(inner);
    frame.render_widget(Paragraph::new(header_lines), regions[0]);
    if regions[2].height > 0 && regions[2].width > 0 {
        let log_lines = state.logs.iter().map(log_line).collect::<Vec<_>>();
        let paragraph = scrollable_log_paragraph(
            log_lines,
            regions[2],
            state.log_revision,
            &mut state.log_scroll,
        );
        frame.render_widget(paragraph, regions[2]);
    }
    if let (Some(message), Some(completion_area)) = (phase.completion_message(), completion_area) {
        frame.render_widget(
            Paragraph::new(Span::styled(
                message,
                Style::default().fg(PRIMARY).add_modifier(Modifier::BOLD),
            )),
            completion_area,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::super::ACTIVITY_FRAMES;
    use super::super::tui_steps::step_line;
    use super::*;
    use crate::logger::LogSink;
    use crate::pipeline::{StageDescriptor, StageId};
    use crate::reporter::{Reporter, StageProgress};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use std::sync::atomic::Ordering;
    use std::time::Instant;

    fn descriptor(id: StageId) -> StageDescriptor {
        StageDescriptor {
            id,
            label: id.label(),
            predicted_present: true,
        }
    }

    fn record(message: impl Into<String>) -> CapturedRecord {
        CapturedRecord {
            level: log::Level::Info,
            target: "test".to_owned(),
            message: message.into(),
        }
    }

    fn warning(message: impl Into<String>) -> CapturedRecord {
        CapturedRecord {
            level: log::Level::Warn,
            target: "test".to_owned(),
            message: message.into(),
        }
    }

    fn buffer_text(terminal: &Terminal<TestBackend>) -> String {
        terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect()
    }

    fn buffer_row(terminal: &Terminal<TestBackend>, y: u16) -> String {
        let width = terminal.size().unwrap().width;
        (0..width)
            .map(|x| terminal.backend().buffer()[(x, y)].symbol())
            .collect()
    }

    fn core_capacity_text(permits: usize, max_permits: usize, width: u16) -> String {
        core_capacity_spans(permits, max_permits, width)
            .iter()
            .map(|span| span.content.as_ref())
            .collect()
    }

    fn progress_track(text: &str) -> Option<&str> {
        text.lines().find_map(|line| {
            let start = line.find('[')?;
            let end = line[start..].find(']')? + start;
            let track = &line[start + 1..end];
            track
                .chars()
                .all(|symbol| matches!(symbol, '\u{2588}' | '\u{2591}'))
                .then_some(track)
        })
    }

    fn render_step(step: &StepState) -> Vec<String> {
        let mut terminal = Terminal::new(TestBackend::new(30, 1)).unwrap();
        terminal
            .draw(|frame| frame.render_widget(Paragraph::new(step_line(step)), frame.area()))
            .unwrap();
        terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol().to_owned())
            .collect()
    }

    fn render_progress(total: Option<usize>, completed: usize, compact: bool) -> String {
        let progress = StageProgress::indeterminate();
        progress
            .completed_handle()
            .store(completed, Ordering::Relaxed);
        if let Some(total) = total {
            progress.publish_total(total);
        }

        let mut state = TuiState::new(&[descriptor(StageId::ShBake)]);
        state.steps[0].status = StepStatus::Active;
        state.steps[0].progress = Some(progress);
        state.steps[0].started = Some(Instant::now());
        let governor = Governor::new(1, false);
        let (width, height) = if compact { (30, 10) } else { (80, 18) };
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal
            .draw(|frame| {
                if compact {
                    draw_compact(
                        frame,
                        frame.area(),
                        &mut state,
                        0,
                        &governor,
                        8,
                        TuiPhase::Running,
                    );
                } else {
                    draw_full(
                        frame,
                        frame.area(),
                        &mut state,
                        &governor,
                        8,
                        TuiPhase::Running,
                    );
                }
            })
            .unwrap();

        let buffer = terminal.backend().buffer();
        (0..height)
            .map(|y| {
                (0..width)
                    .map(|x| buffer[(x, y)].symbol())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn activity_pulse_moves_only_when_progress_moves_and_keeps_fixed_width() {
        let progress = StageProgress::with_total(10);
        let mut step = StepState::new(descriptor(StageId::ShBake));
        step.status = StepStatus::Active;
        step.progress = Some(progress.clone());
        step.observe_liveness();
        assert_eq!(step.activity_index, 0);
        assert_eq!(ACTIVITY_FRAMES[step.activity_index].chars().count(), 3);
        let initial = render_step(&step);
        progress.completed_handle().store(1, Ordering::Relaxed);
        step.observe_liveness();
        assert_eq!(step.activity_index, 1);
        assert_eq!(ACTIVITY_FRAMES[step.activity_index].chars().count(), 3);
        let advanced = render_step(&step);
        assert_ne!(&initial[..3], &advanced[..3]);
        assert_eq!(&initial[4..], &advanced[4..]);
        step.observe_liveness();
        assert_eq!(step.activity_index, 1);
        assert_eq!(advanced, render_step(&step));
    }

    #[test]
    fn determinate_progress_bars_render_empty_partial_and_full_in_both_layouts() {
        for compact in [false, true] {
            let empty = render_progress(Some(4), 0, compact);
            let empty_track = progress_track(&empty).expect("empty progress track");
            assert!(empty_track.chars().all(|symbol| symbol == '\u{2591}'));

            let partial = render_progress(Some(4), 2, compact);
            let partial_track = progress_track(&partial).expect("partial progress track");
            assert!(partial_track.contains('\u{2588}'));
            assert!(partial_track.contains('\u{2591}'));

            let full = render_progress(Some(4), 4, compact);
            let full_track = progress_track(&full).expect("full progress track");
            assert!(full_track.chars().all(|symbol| symbol == '\u{2588}'));
        }
    }

    #[test]
    fn progress_bars_use_full_cell_glyphs_for_vertical_weight() {
        for compact in [false, true] {
            let rendered = render_progress(Some(4), 2, compact);
            let track = progress_track(&rendered).expect("determinate progress track");

            assert!(track.contains(PROGRESS_FILLED));
            assert!(track.contains(PROGRESS_EMPTY));
            assert!(!track.contains('\u{2500}'));
            assert!(!track.contains('\u{2501}'));
        }
    }

    #[test]
    fn indeterminate_progress_omits_track_in_both_layouts() {
        for compact in [false, true] {
            let rendered = render_progress(None, 2, compact);
            assert!(progress_track(&rendered).is_none());
            assert!(rendered.contains("working  --%"));
        }
    }

    #[test]
    fn progress_bar_degrades_when_no_track_width_is_available() {
        let progress = StageProgress::with_total(4);
        let mut step = StepState::new(descriptor(StageId::ShBake));
        step.progress = Some(progress);

        assert!(progress_bar_line(&step, 3).is_none());
        assert!(progress_bar_line(&step, 4).is_some());
    }

    #[test]
    fn compact_layout_activates_below_either_minimum() {
        assert_eq!(layout_mode(Rect::new(0, 0, 67, 30)), LayoutMode::Compact);
        assert_eq!(layout_mode(Rect::new(0, 0, 100, 17)), LayoutMode::Compact);
        assert_eq!(layout_mode(Rect::new(0, 0, 68, 18)), LayoutMode::Full);
    }

    #[test]
    fn completed_builds_render_honest_exit_prompts_in_both_layouts() {
        for (phase, expected, rejected) in [
            (
                TuiPhase::Done,
                "DONE - Press any key to exit",
                "FAILED - Press any key to exit",
            ),
            (
                TuiPhase::Failed,
                "FAILED - Press any key to exit",
                "DONE - Press any key to exit",
            ),
        ] {
            let governor = Governor::new(1, false);
            let mut full_state = TuiState::new(&[descriptor(StageId::Parsing)]);
            full_state.steps[0].status = StepStatus::Done;
            let mut full = Terminal::new(TestBackend::new(80, 18)).unwrap();
            full.draw(|frame| {
                draw_full(frame, frame.area(), &mut full_state, &governor, 8, phase);
            })
            .unwrap();
            let full_text = buffer_text(&full);
            assert!(full_text.contains(expected));
            assert!(!full_text.contains(rejected));
            assert!(buffer_row(&full, 17).contains(expected));

            let mut compact_state = TuiState::new(&[descriptor(StageId::Parsing)]);
            compact_state.steps[0].status = StepStatus::Done;
            let mut compact = Terminal::new(TestBackend::new(40, 10)).unwrap();
            compact
                .draw(|frame| {
                    draw_compact(
                        frame,
                        frame.area(),
                        &mut compact_state,
                        0,
                        &governor,
                        8,
                        phase,
                    );
                })
                .unwrap();
            let compact_text = buffer_text(&compact);
            assert!(compact_text.contains(expected));
            assert!(!compact_text.contains(rejected));
            let bottom_row = (0..40)
                .map(|x| compact.backend().buffer()[(x, 9)].symbol())
                .collect::<String>();
            assert!(bottom_row.contains(expected));
        }
    }

    #[test]
    fn full_layout_pads_headings_and_uses_the_bottom_row_for_the_control_legend() {
        let governor = Governor::new(2, false);
        let mut state = TuiState::new(&[descriptor(StageId::Parsing)]);
        let mut terminal = Terminal::new(TestBackend::new(80, 18)).unwrap();
        terminal
            .draw(|frame| {
                draw_full(
                    frame,
                    frame.area(),
                    &mut state,
                    &governor,
                    4,
                    TuiPhase::Running,
                );
            })
            .unwrap();

        assert!(buffer_row(&terminal, 0).trim().is_empty());
        let heading_row = buffer_row(&terminal, 1);
        assert!(heading_row.contains("Compile steps"));
        assert!(heading_row.contains("Build log"));

        let status_row = buffer_row(&terminal, 16);
        let legend_row = buffer_row(&terminal, 17);
        assert_eq!(legend_row.find(CONTROL_LEGEND), Some(2));
        assert_eq!(
            status_row.find("Cores"),
            legend_row.find("[\u{2190}/\u{2192}] cores")
        );
        assert!(!legend_row.trim().is_empty());
    }

    #[test]
    fn control_legend_separates_each_item_with_four_spaces() {
        assert_eq!(
            CONTROL_LEGEND,
            "[space] pause    [\u{2190}/\u{2192}] cores    [PgUp/PgDn] log    [q] leave UI"
        );
    }

    #[test]
    fn core_capacity_indicator_renders_one_partial_and_full_capacity() {
        assert_eq!(
            core_capacity_text(1, 4, 40),
            "Cores  \u{25a0} \u{25a1} \u{25a1} \u{25a1}"
        );
        assert_eq!(
            core_capacity_text(2, 4, 40),
            "Cores  \u{25a0} \u{25a0} \u{25a1} \u{25a1}"
        );
        assert_eq!(
            core_capacity_text(4, 4, 40),
            "Cores  \u{25a0} \u{25a0} \u{25a0} \u{25a0}"
        );

        let spans = core_capacity_spans(2, 4, 40);
        assert_eq!(spans[2].style.fg, Some(Color::Gray));
        assert!(spans[2].style.add_modifier.contains(Modifier::BOLD));
        assert_eq!(spans[6].style.fg, Some(MUTED));
        assert!(spans[6].style.add_modifier.contains(Modifier::DIM));
    }

    #[test]
    fn core_capacity_indicator_compresses_high_counts_and_omits_markers_when_narrow() {
        let compressed = core_capacity_text(64, 128, 18);
        assert_eq!(
            compressed,
            "Cores  \u{25a0} \u{25a0} \u{25a0} \u{25a1} \u{25a1} \u{25a1}"
        );
        assert_eq!(compressed.chars().count(), 18);
        assert_eq!(core_capacity_text(64, 128, 7), "Cores");
    }

    #[test]
    fn compact_layout_core_row_uses_boxes_without_a_numeric_indicator() {
        let governor = Governor::new(2, false);
        let mut state = TuiState::new(&[]);
        let mut terminal = Terminal::new(TestBackend::new(40, 10)).unwrap();
        terminal
            .draw(|frame| {
                draw_compact(
                    frame,
                    frame.area(),
                    &mut state,
                    0,
                    &governor,
                    4,
                    TuiPhase::Running,
                );
            })
            .unwrap();

        let core_row = buffer_row(&terminal, 1);
        assert!(core_row.contains("Cores  \u{25a0} \u{25a0} \u{25a1} \u{25a1}"));
        assert!(!core_row.contains("2/4"));
    }

    #[test]
    fn wrapped_log_tail_stays_visible_in_full_and_compact_layouts() {
        let mut state = TuiState::new(&[]);
        state
            .logs
            .extend((0..10).map(|index| record(format!("old-{index} {}", "x".repeat(90)))));
        state.logs.push(record("NEWEST-LOG-RECORD"));
        let governor = Governor::new(1, false);

        let mut full = Terminal::new(TestBackend::new(80, 18)).unwrap();
        full.draw(|frame| {
            draw_full(
                frame,
                frame.area(),
                &mut state,
                &governor,
                8,
                TuiPhase::Running,
            );
        })
        .unwrap();
        assert!(buffer_text(&full).contains("NEWEST-LOG-RECORD"));

        let mut compact = Terminal::new(TestBackend::new(30, 10)).unwrap();
        compact
            .draw(|frame| {
                draw_compact(
                    frame,
                    frame.area(),
                    &mut state,
                    0,
                    &governor,
                    8,
                    TuiPhase::Running,
                );
            })
            .unwrap();
        assert!(buffer_text(&compact).contains("NEWEST-LOG-RECORD"));
    }

    #[test]
    fn compact_layout_keeps_active_progress_fixed_above_scrolling_logs() {
        let mut state = TuiState::new(&[descriptor(StageId::LightmapBake)]);
        state.steps[0].status = StepStatus::Active;
        state.steps[0].progress = Some(StageProgress::with_total(100));
        state.steps[0]
            .progress
            .as_ref()
            .unwrap()
            .completed_handle()
            .store(40, Ordering::Relaxed);
        state
            .logs
            .extend((0..20).map(|index| record(format!("log-{index:02} {}", "x".repeat(40)))));
        state.logs.push(record("NEWEST-COMPACT-LOG"));
        let governor = Governor::new(2, false);
        let mut terminal = Terminal::new(TestBackend::new(40, 10)).unwrap();

        terminal
            .draw(|frame| {
                draw_compact(
                    frame,
                    frame.area(),
                    &mut state,
                    0,
                    &governor,
                    4,
                    TuiPhase::Running,
                );
            })
            .unwrap();

        let rendered = buffer_text(&terminal);
        assert!(rendered.contains("RUNNING"));
        assert!(rendered.contains("Lightmap Bake"));
        assert!(rendered.contains("40/100   40%"));
        assert!(rendered.contains("NEWEST-COMPACT-LOG"));

        state.log_scroll.scroll(super::super::LogScrollAction::Home);
        terminal
            .draw(|frame| {
                draw_compact(
                    frame,
                    frame.area(),
                    &mut state,
                    0,
                    &governor,
                    4,
                    TuiPhase::Running,
                );
            })
            .unwrap();
        let scrolled = buffer_text(&terminal);
        assert!(scrolled.contains("Lightmap Bake"));
        assert!(scrolled.contains("40/100   40%"));
        assert!(!scrolled.contains("NEWEST-COMPACT-LOG"));
    }

    #[test]
    fn compact_layout_tolerates_a_one_cell_viewport() {
        let mut state = TuiState::new(&[descriptor(StageId::LightmapBake)]);
        state.steps[0].status = StepStatus::Active;
        state.logs.push(record("clipped log"));
        let governor = Governor::new(1, false);
        let mut terminal = Terminal::new(TestBackend::new(1, 1)).unwrap();

        terminal
            .draw(|frame| {
                draw_compact(
                    frame,
                    frame.area(),
                    &mut state,
                    0,
                    &governor,
                    1,
                    TuiPhase::Running,
                );
            })
            .unwrap();
    }

    #[test]
    fn log_records_match_pending_stage_color_and_emphasize_only_warning_prefixes() {
        let pending = step_line(&StepState::new(descriptor(StageId::Parsing)));
        let pending_foreground = pending.spans[1].style.fg;
        let info = log_line(&record("ordinary message"));
        let warn = log_line(&warning("warning message"));

        assert_eq!(pending_foreground, Some(MUTED));
        assert_eq!(info.spans[0].style.fg, pending_foreground);
        assert!(!info.spans[0].style.add_modifier.contains(Modifier::BOLD));
        assert_eq!(info.spans[1].style.fg, pending_foreground);
        assert!(!info.spans[1].style.add_modifier.contains(Modifier::BOLD));
        assert_eq!(warn.spans[0].style.fg, pending_foreground);
        assert!(warn.spans[0].style.add_modifier.contains(Modifier::BOLD));
        assert_eq!(warn.spans[1].content, "warning message");
        assert_eq!(warn.spans[1].style.fg, pending_foreground);
        assert!(!warn.spans[1].style.add_modifier.contains(Modifier::BOLD));
    }

    #[test]
    fn full_layout_gives_build_log_the_former_warning_panel_region() {
        let mut state = TuiState::new(&[]);
        state
            .logs
            .extend((0..13).map(|index| record(format!("log-{index:02}"))));
        let governor = Governor::new(1, false);
        let mut terminal = Terminal::new(TestBackend::new(80, 18)).unwrap();

        terminal
            .draw(|frame| {
                draw_full(
                    frame,
                    frame.area(),
                    &mut state,
                    &governor,
                    8,
                    TuiPhase::Running,
                )
            })
            .unwrap();
        let rendered = buffer_text(&terminal);

        assert!(!rendered.contains("Warnings ("));
        assert!(rendered.contains("log-00"));
        assert!(rendered.contains("log-12"));
    }

    #[test]
    fn scrolled_log_holds_position_when_new_records_arrive_and_end_restores_tail() {
        let mut state = TuiState::new(&[]);
        state.append_logs(
            (0..20)
                .map(|index| record(format!("old-{index:02}")))
                .collect(),
        );
        let governor = Governor::new(1, false);
        let mut terminal = Terminal::new(TestBackend::new(80, 18)).unwrap();

        terminal
            .draw(|frame| {
                draw_full(
                    frame,
                    frame.area(),
                    &mut state,
                    &governor,
                    8,
                    TuiPhase::Running,
                )
            })
            .unwrap();
        assert!(buffer_text(&terminal).contains("old-19"));

        state
            .log_scroll
            .scroll(super::super::LogScrollAction::PageUp);
        terminal
            .draw(|frame| {
                draw_full(
                    frame,
                    frame.area(),
                    &mut state,
                    &governor,
                    8,
                    TuiPhase::Running,
                )
            })
            .unwrap();
        let before_append = state.log_scroll.lines_from_tail;

        state.append_logs(vec![record("NEWEST-AFTER-SCROLL")]);
        terminal
            .draw(|frame| {
                draw_full(
                    frame,
                    frame.area(),
                    &mut state,
                    &governor,
                    8,
                    TuiPhase::Running,
                )
            })
            .unwrap();
        assert!(state.log_scroll.lines_from_tail >= before_append);
        assert!(!buffer_text(&terminal).contains("NEWEST-AFTER-SCROLL"));

        state.log_scroll.scroll(super::super::LogScrollAction::End);
        terminal
            .draw(|frame| {
                draw_full(
                    frame,
                    frame.area(),
                    &mut state,
                    &governor,
                    8,
                    TuiPhase::Running,
                )
            })
            .unwrap();
        assert!(buffer_text(&terminal).contains("NEWEST-AFTER-SCROLL"));
    }

    #[test]
    fn failure_finalization_marks_the_active_step_failed_and_stops_animation() {
        let reporter = TuiReporter::new(
            &[
                descriptor(StageId::Parsing),
                descriptor(StageId::LightmapBake),
            ],
            LogSink::default(),
        );
        reporter.begin_stage(StageId::LightmapBake);

        reporter.finalize_failure();

        let state = reporter.lock();
        assert!(state.active_index().is_none());
        let failed = state
            .steps
            .iter()
            .find(|step| step.id == StageId::LightmapBake)
            .unwrap();
        assert_eq!(failed.status, StepStatus::Failed);
        let rendered = step_line(failed);
        assert_eq!(rendered.spans[0].content, "!   ");
        assert_eq!(rendered.spans[1].content, "Lightmap Bake");
        drop(state);

        let governor = Governor::new(1, false);
        let mut terminal = Terminal::new(TestBackend::new(40, 10)).unwrap();
        terminal
            .draw(|frame| {
                draw_compact(
                    frame,
                    frame.area(),
                    &mut reporter.lock(),
                    0,
                    &governor,
                    4,
                    TuiPhase::Failed,
                );
            })
            .unwrap();
        let completion = buffer_text(&terminal);
        assert!(completion.contains("!    Lightmap Bake"));
        assert!(!completion.contains(ACTIVITY_FRAMES[1]));
    }
}
