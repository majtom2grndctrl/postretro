//! Compile-steps sidebar rendering.
//! See: `context/lib/build_pipeline.md`.

use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Padding, Paragraph};

use crate::pipeline::{self, StageId};

use super::tui_progress::progress_text;
use super::tui_render::{MUTED, PRIMARY, SECONDARY, divider_style, progress_bar_line};
use super::{ACTIVITY_FRAMES, StepState, StepStatus, TuiState};

struct StepSection {
    label: &'static str,
    stages: &'static [StageId],
}

const PARSE_STAGES: &[StageId] = &[
    StageId::Parsing,
    StageId::DataScript,
    StageId::TextureValidation,
];
const WORLD_STAGES: &[StageId] = &[
    StageId::Partitioning,
    StageId::Visibility,
    StageId::Geometry,
    StageId::BvhBuild,
    StageId::CellVisibility,
    StageId::NavMesh,
];
const LIGHTING_STAGES: &[StageId] = &[
    StageId::LightmapBake,
    StageId::ShBake,
    StageId::DeltaShBake,
    StageId::DirectShBake,
    StageId::AnimatedDirectShBake,
    StageId::EntityShadowLights,
    StageId::DirectShDeltaBake,
    StageId::ShadowmaskAtlas,
    StageId::ChunkLightList,
    StageId::AnimatedLightChunks,
    StageId::AnimatedWeightMaps,
];
const PACK_STAGES: &[StageId] = &[
    StageId::SdfAtlasBake,
    StageId::TextureMips,
    StageId::Packing,
];

const STEP_SECTIONS: &[StepSection] = &[
    StepSection {
        label: "Parse",
        stages: PARSE_STAGES,
    },
    StepSection {
        label: "World",
        stages: WORLD_STAGES,
    },
    StepSection {
        label: "Lighting",
        stages: LIGHTING_STAGES,
    },
    StepSection {
        label: "Pack",
        stages: PACK_STAGES,
    },
];

enum StepRow<'a> {
    Header {
        label: &'static str,
        status: StepStatus,
        done: usize,
        total: usize,
    },
    Step(&'a StepState),
}

impl StepRow<'_> {
    fn line(&self) -> Line<'static> {
        match self {
            Self::Header {
                label,
                status,
                done,
                total,
            } => {
                let (marker, style) = step_marker(*status, 0);
                Line::from(vec![
                    Span::styled(format!("{marker} "), style),
                    Span::styled(*label, style.add_modifier(Modifier::BOLD)),
                    Span::styled(format!(" {done}/{total}"), Style::default().fg(MUTED)),
                ])
            }
            Self::Step(step) => step_line(step),
        }
    }

    fn is_active_step(&self, active: Option<StageId>) -> bool {
        matches!(self, Self::Step(step) if Some(step.id) == active)
    }
}

pub(super) fn draw_steps(frame: &mut ratatui::Frame<'_>, area: Rect, state: &TuiState) {
    let block = Block::default()
        .title(Span::styled(
            " Compile steps ",
            Style::default().fg(PRIMARY).add_modifier(Modifier::BOLD),
        ))
        .borders(Borders::RIGHT)
        .border_style(divider_style())
        .padding(Padding::horizontal(2));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let footer_height = 3.min(inner.height);
    let sections = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(footer_height)])
        .split(inner);
    let rows = step_rows(state);
    let active = state
        .active_index()
        .and_then(|index| state.steps.get(index));
    let active_row = active.and_then(|step| {
        rows.iter()
            .position(|row| row.is_active_step(Some(step.id)))
    });
    let visible = sections[0].height as usize;
    let offset = active_scroll_offset(rows.len(), visible, active_row);
    let lines = rows
        .iter()
        .skip(offset)
        .take(visible)
        .map(StepRow::line)
        .collect::<Vec<_>>();
    frame.render_widget(Paragraph::new(lines), sections[0]);

    if let Some(active) = active {
        let (progress, eta) = progress_text(active);
        let mut lines = vec![
            Line::from(Span::styled(progress, Style::default().fg(PRIMARY))),
            Line::from(Span::styled(eta, Style::default().fg(SECONDARY))),
        ];
        if let Some(bar) = progress_bar_line(active, sections[1].width) {
            lines.insert(1, bar);
        }
        frame.render_widget(Paragraph::new(lines), sections[1]);
    }
}

fn step_rows(state: &TuiState) -> Vec<StepRow<'_>> {
    let open = open_section(state);
    let mut rows = Vec::new();
    for (section_index, section) in STEP_SECTIONS.iter().enumerate() {
        let members = section_members(state, section);
        if members.is_empty() {
            continue;
        }
        rows.push(StepRow::Header {
            label: section.label,
            status: aggregate_status(&members),
            done: members
                .iter()
                .filter(|step| matches!(step.status, StepStatus::Done | StepStatus::Skipped))
                .count(),
            total: members.len(),
        });
        if Some(section_index) == open {
            rows.extend(members.into_iter().map(StepRow::Step));
        }
    }
    rows
}

fn section_members<'a>(state: &'a TuiState, section: &StepSection) -> Vec<&'a StepState> {
    section
        .stages
        .iter()
        .filter_map(|id| state.steps.iter().find(|step| step.id == *id))
        .collect()
}

fn open_section(state: &TuiState) -> Option<usize> {
    if let Some(active) = state
        .active_index()
        .and_then(|index| state.steps.get(index))
    {
        return section_index(active.id);
    }
    state
        .steps
        .iter()
        .filter(|step| step.status != StepStatus::Pending)
        .max_by_key(|step| stage_index(step.id))
        .and_then(|step| section_index(step.id))
        .or_else(|| state.steps.first().and_then(|step| section_index(step.id)))
}

fn section_index(id: StageId) -> Option<usize> {
    STEP_SECTIONS
        .iter()
        .position(|section| section.stages.contains(&id))
}

fn stage_index(id: StageId) -> usize {
    pipeline::ORDERED_STAGES
        .iter()
        .position(|candidate| *candidate == id)
        .expect("all displayed stages belong to the ordered pipeline")
}

fn aggregate_status(steps: &[&StepState]) -> StepStatus {
    if steps.iter().any(|step| step.status == StepStatus::Failed) {
        StepStatus::Failed
    } else if steps.iter().any(|step| step.status == StepStatus::Active) {
        StepStatus::Active
    } else if steps
        .iter()
        .all(|step| matches!(step.status, StepStatus::Done | StepStatus::Skipped))
    {
        StepStatus::Done
    } else {
        StepStatus::Pending
    }
}

pub(super) fn active_scroll_offset(total: usize, visible: usize, active: Option<usize>) -> usize {
    if visible == 0 || total <= visible {
        return 0;
    }
    let Some(active) = active else {
        return total.saturating_sub(visible);
    };
    active
        .saturating_sub(visible / 2)
        .min(total.saturating_sub(visible))
}

pub(super) fn step_line(step: &StepState) -> Line<'static> {
    let (marker, style) = step_marker(step.status, step.activity_index);
    Line::from(vec![
        Span::styled(format!("{marker} "), style),
        Span::styled(step.label, style),
    ])
}

fn step_marker(status: StepStatus, activity_index: usize) -> (&'static str, Style) {
    match status {
        StepStatus::Pending => ("\u{00b7}  ", Style::default().fg(MUTED)),
        StepStatus::Active => (
            ACTIVITY_FRAMES[activity_index],
            Style::default().fg(PRIMARY).add_modifier(Modifier::BOLD),
        ),
        StepStatus::Done => ("\u{2713}  ", Style::default()),
        StepStatus::Skipped => ("\u{2013}  ", Style::default().fg(MUTED)),
        StepStatus::Failed => (
            "!  ",
            Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pipeline::StageDescriptor;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    fn descriptor(id: StageId) -> StageDescriptor {
        StageDescriptor {
            id,
            label: id.label(),
            predicted_present: true,
        }
    }

    fn state() -> TuiState {
        TuiState::new(
            &pipeline::ORDERED_STAGES
                .iter()
                .copied()
                .map(descriptor)
                .collect::<Vec<_>>(),
        )
    }

    fn rendered(state: &mut TuiState, width: u16, height: u16) -> String {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal
            .draw(|frame| draw_steps(frame, frame.area(), state))
            .unwrap();
        terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect()
    }

    #[test]
    fn section_table_is_a_contiguous_total_partition() {
        let flattened = STEP_SECTIONS
            .iter()
            .flat_map(|section| section.stages.iter().copied())
            .collect::<Vec<_>>();
        assert_eq!(flattened, pipeline::ORDERED_STAGES);
        for stage in pipeline::ORDERED_STAGES {
            assert_eq!(
                STEP_SECTIONS
                    .iter()
                    .filter(|section| section.stages.contains(&stage))
                    .count(),
                1,
                "{stage:?} must appear exactly once"
            );
        }
        assert_eq!(
            section_index(StageId::EntityShadowLights),
            section_index(StageId::DirectShDeltaBake)
        );
    }

    #[test]
    fn p1_overlapping_actives_open_lighting_and_show_both() {
        let mut state = state();
        state.begin_step(StageId::EntityShadowLights);
        state.begin_step(StageId::DirectShDeltaBake);
        let text = rendered(&mut state, 40, 30);
        assert!(text.contains("Lighting 0/11"));
        assert!(text.contains("EntityShadowLights"));
        assert!(text.contains("Direct SH Delta Bake"));
        assert!(!text.contains("Parse 0/3\nParsing"));
    }

    #[test]
    fn collapsed_and_expanded_headers_always_show_summaries() {
        let mut state = state();
        state.begin_step(StageId::LightmapBake);
        let text = rendered(&mut state, 40, 30);
        assert!(text.contains("Parse 0/3"));
        assert!(text.contains("World 0/6"));
        assert!(text.contains("Lighting 0/11"));
        assert!(text.contains("Pack 0/3"));
        assert!(text.contains("Lightmap Bake"));
    }

    #[test]
    fn zero_present_sections_are_omitted() {
        let mut state =
            TuiState::new(&[descriptor(StageId::Parsing), descriptor(StageId::Packing)]);
        state.begin_step(StageId::Parsing);
        let text = rendered(&mut state, 40, 20);
        assert!(text.contains("Parse 0/1"));
        assert!(text.contains("Pack 0/1"));
        assert!(!text.contains("World"));
        assert!(!text.contains("Lighting"));
    }

    #[test]
    fn p4_skips_advance_the_open_section_monotonically() {
        let mut state = state();
        for stage in [
            StageId::AnimatedDirectShBake,
            StageId::EntityShadowLights,
            StageId::DirectShDeltaBake,
            StageId::AnimatedLightChunks,
            StageId::AnimatedWeightMaps,
        ] {
            state.step_mut(stage).unwrap().status = StepStatus::Skipped;
            assert_eq!(open_section(&state), Some(2));
        }
        state.step_mut(StageId::TextureMips).unwrap().status = StepStatus::Skipped;
        assert_eq!(open_section(&state), Some(3));
    }

    #[test]
    fn p5_overflow_keeps_the_active_step_visible() {
        let mut state = state();
        state.step_mut(StageId::NavMesh).unwrap().status = StepStatus::Done;
        assert_eq!(open_section(&state), Some(1));
        state.begin_step(StageId::LightmapBake);
        let rows = step_rows(&state);
        let active = rows
            .iter()
            .position(|row| row.is_active_step(Some(StageId::LightmapBake)));
        let offset = active_scroll_offset(rows.len(), 3, active);
        assert!(offset <= active.unwrap() && active.unwrap() < offset + 3);
        let text = rendered(&mut state, 40, 9);
        assert!(text.contains("Lightmap Bake"));
    }

    #[test]
    fn p7_completion_opens_pack_without_a_spinner() {
        let mut state = state();
        for step in &mut state.steps {
            step.status = StepStatus::Done;
        }
        let text = rendered(&mut state, 40, 30);
        assert_eq!(open_section(&state), Some(3));
        assert!(text.contains("Pack 3/3"));
        assert!(!text.contains(ACTIVITY_FRAMES[0]));
    }

    #[test]
    fn p8_failure_opens_the_failed_frontier_section() {
        let mut state = state();
        state.begin_step(StageId::EntityShadowLights);
        state.begin_step(StageId::DirectShDeltaBake);
        for step in &mut state.steps {
            if step.status == StepStatus::Active {
                step.status = StepStatus::Failed;
            }
        }
        let text = rendered(&mut state, 40, 30);
        assert_eq!(open_section(&state), Some(2));
        assert!(text.contains("EntityShadowLights"));
        assert!(text.contains("Direct SH Delta Bake"));
        assert!(text.contains("!"));
    }

    #[test]
    fn scrolling_keeps_active_step_visible() {
        assert_eq!(active_scroll_offset(21, 5, Some(0)), 0);
        let offset = active_scroll_offset(21, 5, Some(12));
        assert!(offset <= 12 && 12 < offset + 5);
        assert_eq!(active_scroll_offset(21, 5, Some(20)), 16);
    }

    #[test]
    fn no_active_step_scrolls_to_bottom_when_overflowing() {
        assert_eq!(active_scroll_offset(21, 5, None), 16);
        assert_eq!(active_scroll_offset(5, 5, None), 0);
        assert_eq!(active_scroll_offset(3, 5, None), 0);
        assert_eq!(active_scroll_offset(21, 0, None), 0);
    }
}
