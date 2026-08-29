//! Pre-bake quality configuration for the compiler TUI.
//! See: `context/lib/build_pipeline.md`.

use std::io::{self, Stdout};

use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};

use crate::{Args, lightmap_bake, resolve_lightmap_density, sdf_bake, sh_bake};

use super::tui_terminal::TerminalSession;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConfigOutcome {
    Start,
    Cancel,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum BuildMode {
    RapidIteration,
    Production,
}

impl BuildMode {
    fn toggle(&mut self) {
        *self = match self {
            Self::RapidIteration => Self::Production,
            Self::Production => Self::RapidIteration,
        };
    }

    fn label(self) -> &'static str {
        match self {
            Self::RapidIteration => "Rapid iteration",
            Self::Production => "Production",
        }
    }

    fn guidance(self) -> &'static str {
        match self {
            Self::RapidIteration => {
                "Warm cache + approximate grouped indirect SH (fast, not shippable)."
            }
            Self::Production => "Exact whole-volume SH + monolithic lightmap (slow, shippable).",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ConfigField {
    ProbeSpacing,
    LightmapDensity,
    SoftShadowSamples,
    VoxelSize,
    BuildMode,
}

impl ConfigField {
    const ALL: [Self; 5] = [
        Self::ProbeSpacing,
        Self::LightmapDensity,
        Self::SoftShadowSamples,
        Self::VoxelSize,
        Self::BuildMode,
    ];

    fn is_numeric(self) -> bool {
        self != Self::BuildMode
    }

    fn next(self, delta: isize) -> Self {
        let index = Self::ALL
            .iter()
            .position(|field| *field == self)
            .expect("config field is listed");
        let len = Self::ALL.len() as isize;
        let index = (index as isize + delta).rem_euclid(len) as usize;
        Self::ALL[index]
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DensitySource {
    Cli,
    MapKvp,
    Default,
}

impl DensitySource {
    fn label(self) -> &'static str {
        match self {
            Self::Cli => "CLI",
            Self::MapKvp => "map KVP",
            Self::Default => "default",
        }
    }
}

#[derive(Clone, Debug)]
struct FormState {
    input: String,
    output: String,
    probe_spacing: String,
    lightmap_density: String,
    soft_shadow_samples: String,
    voxel_size: String,
    lightmap_density_touched: bool,
    density_source: DensitySource,
    build_mode: BuildMode,
    selected: ConfigField,
    editing: Option<ConfigField>,
    validation_error: Option<String>,
}

impl FormState {
    fn from_args(args: &Args, worldspawn_lightmap_density: Option<f32>) -> Self {
        let density_source = if args.lightmap_density.is_some() {
            DensitySource::Cli
        } else if worldspawn_lightmap_density.is_some() {
            DensitySource::MapKvp
        } else {
            DensitySource::Default
        };
        Self {
            input: args.input.display().to_string(),
            output: args.output.display().to_string(),
            probe_spacing: args.probe_spacing.to_string(),
            lightmap_density: resolve_lightmap_density(
                args.lightmap_density,
                worldspawn_lightmap_density,
            )
            .to_string(),
            soft_shadow_samples: args.soft_shadow_samples.to_string(),
            voxel_size: args.voxel_size.to_string(),
            // The screen is gated off for CLI density overrides. Keep the form
            // defensive anyway: a density supplied to this form is already an
            // explicit value and should not be discarded if it is confirmed.
            lightmap_density_touched: args.lightmap_density.is_some(),
            density_source,
            build_mode: if args.release || args.no_cache {
                BuildMode::Production
            } else {
                BuildMode::RapidIteration
            },
            selected: ConfigField::ProbeSpacing,
            editing: None,
            validation_error: None,
        }
    }

    fn density_source_label(&self) -> &'static str {
        if self.lightmap_density_touched {
            "screen"
        } else {
            self.density_source.label()
        }
    }

    fn value(&self, field: ConfigField) -> Option<&str> {
        match field {
            ConfigField::ProbeSpacing => Some(&self.probe_spacing),
            ConfigField::LightmapDensity => Some(&self.lightmap_density),
            ConfigField::SoftShadowSamples => Some(&self.soft_shadow_samples),
            ConfigField::VoxelSize => Some(&self.voxel_size),
            ConfigField::BuildMode => None,
        }
    }

    fn value_mut(&mut self, field: ConfigField) -> Option<&mut String> {
        match field {
            ConfigField::ProbeSpacing => Some(&mut self.probe_spacing),
            ConfigField::LightmapDensity => Some(&mut self.lightmap_density),
            ConfigField::SoftShadowSamples => Some(&mut self.soft_shadow_samples),
            ConfigField::VoxelSize => Some(&mut self.voxel_size),
            ConfigField::BuildMode => None,
        }
    }

    fn commit_selected(&mut self) -> bool {
        if !self.selected.is_numeric() {
            self.validation_error = None;
            self.editing = None;
            return true;
        }

        match validate_field(
            self.selected,
            self.value(self.selected)
                .expect("numeric fields have values"),
        ) {
            Ok(_) => {
                self.validation_error = None;
                self.editing = None;
                true
            }
            Err(error) => {
                self.validation_error = Some(error);
                false
            }
        }
    }

    fn validate_all(&mut self) -> bool {
        for field in ConfigField::ALL {
            if !field.is_numeric() {
                continue;
            }
            if let Err(error) = validate_field(field, self.value(field).expect("numeric value")) {
                self.selected = field;
                self.validation_error = Some(error);
                return false;
            }
        }
        self.validation_error = None;
        self.editing = None;
        true
    }

    fn move_selection(&mut self, delta: isize) {
        if self.commit_selected() {
            self.selected = self.selected.next(delta);
        }
    }

    fn append_character(&mut self, character: char) {
        let field = self.selected;
        if !field.is_numeric() || !(character.is_ascii_digit() || character == '.') {
            return;
        }

        if self.editing != Some(field) {
            self.value_mut(field).expect("numeric field").clear();
            self.editing = Some(field);
        }
        let value = self.value_mut(field).expect("numeric field");
        if character == '.' && value.contains('.') {
            return;
        }
        value.push(character);
        if field == ConfigField::LightmapDensity {
            self.lightmap_density_touched = true;
        }
        self.validation_error = None;
    }

    fn backspace(&mut self) {
        let field = self.selected;
        if !field.is_numeric() {
            return;
        }
        self.editing = Some(field);
        self.value_mut(field).expect("numeric field").pop();
        if field == ConfigField::LightmapDensity {
            self.lightmap_density_touched = true;
        }
        self.validation_error = None;
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum ValidatedField {
    Metric(f32),
    Samples(u32),
}

/// Apply the parser's quality limits to an in-progress form field.
fn validate_field(field: ConfigField, value: &str) -> Result<ValidatedField, String> {
    match field {
        ConfigField::ProbeSpacing | ConfigField::LightmapDensity | ConfigField::VoxelSize => {
            let parsed = value
                .parse::<f32>()
                .map_err(|_| "must be a positive number of meters".to_owned())?;
            if !parsed.is_finite() || parsed <= 0.0 {
                return Err("must be a positive number of meters".to_owned());
            }
            Ok(ValidatedField::Metric(parsed))
        }
        ConfigField::SoftShadowSamples => {
            let parsed = value
                .parse::<u32>()
                .map_err(|_| "must be an integer".to_owned())?;
            if parsed < lightmap_bake::SOFT_PROBE_SAMPLES {
                return Err(format!(
                    "must be >= {} (the probe-set floor)",
                    lightmap_bake::SOFT_PROBE_SAMPLES
                ));
            }
            Ok(ValidatedField::Samples(parsed))
        }
        ConfigField::BuildMode => unreachable!("build mode has no numeric buffer"),
    }
}

/// Write a confirmed, fully validated form into the build arguments.
fn apply_outcome(args: &mut Args, form: &FormState) -> Result<(), String> {
    let metric = |field| -> Result<f32, String> {
        match validate_field(field, form.value(field).expect("numeric value"))? {
            ValidatedField::Metric(value) => Ok(value),
            ValidatedField::Samples(_) => unreachable!("metric field validated as samples"),
        }
    };
    let samples = match validate_field(
        ConfigField::SoftShadowSamples,
        form.value(ConfigField::SoftShadowSamples)
            .expect("numeric value"),
    )? {
        ValidatedField::Samples(value) => value,
        ValidatedField::Metric(_) => unreachable!("sample field validated as metric"),
    };

    args.probe_spacing = metric(ConfigField::ProbeSpacing)?;
    args.soft_shadow_samples = samples;
    args.voxel_size = metric(ConfigField::VoxelSize)?;
    args.lightmap_density = form
        .lightmap_density_touched
        .then(|| metric(ConfigField::LightmapDensity))
        .transpose()?;
    match form.build_mode {
        BuildMode::RapidIteration => {
            args.release = false;
            args.no_cache = false;
        }
        BuildMode::Production => {
            // Keep the intent and mechanical flags together so every downstream
            // cache check selects the exact ship path.
            args.release = true;
            args.no_cache = true;
        }
    }
    Ok(())
}

/// Run the pre-bake form in a terminal session, restoring the terminal before
/// returning its outcome or propagating an input/render error.
pub fn run_config_screen(
    args: &mut Args,
    worldspawn_lightmap_density: Option<f32>,
) -> anyhow::Result<ConfigOutcome> {
    let mut form = FormState::from_args(args, worldspawn_lightmap_density);
    let outcome = {
        let mut session = TerminalSession::enter()?;
        render_loop(&mut session.terminal, &mut form)
    }?;

    if outcome == ConfigOutcome::Start {
        apply_outcome(args, &form).map_err(anyhow::Error::msg)?;
    }
    Ok(outcome)
}

fn render_loop(
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    form: &mut FormState,
) -> io::Result<ConfigOutcome> {
    loop {
        terminal.draw(|frame| draw_config(frame, form))?;
        match event::read()? {
            Event::Key(key) if key.kind == KeyEventKind::Press => {
                if let Some(outcome) = handle_key(form, key) {
                    return Ok(outcome);
                }
            }
            // Redraw on the next iteration after clearing old cells from the
            // alternate buffer, matching the running-bake screen's behavior.
            Event::Resize(_, _) => terminal.clear()?,
            _ => {}
        }
    }
}

fn handle_key(form: &mut FormState, key: KeyEvent) -> Option<ConfigOutcome> {
    if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
        return Some(ConfigOutcome::Cancel);
    }
    match key.code {
        KeyCode::Esc | KeyCode::Char('q') => Some(ConfigOutcome::Cancel),
        KeyCode::Up => {
            form.move_selection(-1);
            None
        }
        KeyCode::Down => {
            form.move_selection(1);
            None
        }
        KeyCode::Tab => {
            let delta = if key.modifiers.contains(KeyModifiers::SHIFT) {
                -1
            } else {
                1
            };
            form.move_selection(delta);
            None
        }
        KeyCode::Left | KeyCode::Right | KeyCode::Char(' ')
            if form.selected == ConfigField::BuildMode =>
        {
            form.build_mode.toggle();
            form.validation_error = None;
            None
        }
        KeyCode::Backspace => {
            form.backspace();
            None
        }
        KeyCode::Char(character) => {
            form.append_character(character);
            None
        }
        KeyCode::Enter if form.validate_all() => Some(ConfigOutcome::Start),
        KeyCode::Enter => None,
        _ => None,
    }
}

fn draw_config(frame: &mut ratatui::Frame<'_>, form: &FormState) {
    let area = frame.area();
    let block = Block::default()
        .title(Span::styled(
            " Pre-bake configuration ",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Blue));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(2)])
        .split(inner);
    let mut lines = vec![
        readonly_line("Input", &form.input),
        readonly_line("Output", &form.output),
        Line::default(),
        field_line(
            form,
            ConfigField::ProbeSpacing,
            "SH probe spacing",
            &format!("{} m", form.probe_spacing),
        ),
        guidance_line(format!(
            "Default {} m; smaller = denser SH / slower.",
            sh_bake::DEFAULT_PROBE_SPACING
        )),
        field_line(
            form,
            ConfigField::LightmapDensity,
            "Lightmap density",
            &format!(
                "{} m/texel ({})",
                form.lightmap_density,
                form.density_source_label()
            ),
        ),
        guidance_line(format!(
            "Default {} m/texel; smaller = finer / slower.",
            lightmap_bake::DEFAULT_TEXEL_DENSITY_METERS
        )),
        field_line(
            form,
            ConfigField::SoftShadowSamples,
            "Soft-shadow samples",
            &form.soft_shadow_samples,
        ),
        guidance_line(format!(
            "Default {}; higher = softer penumbra / slower (minimum {}).",
            lightmap_bake::DEFAULT_AREA_SAMPLE_COUNT,
            lightmap_bake::SOFT_PROBE_SAMPLES
        )),
        field_line(
            form,
            ConfigField::VoxelSize,
            "SDF voxel size",
            &format!("{} m", form.voxel_size),
        ),
        guidance_line(format!(
            "Default {} m; smaller = finer occluders / slower.",
            sdf_bake::DEFAULT_VOXEL_SIZE_METERS
        )),
        field_line(
            form,
            ConfigField::BuildMode,
            "Build mode",
            form.build_mode.label(),
        ),
        guidance_line(form.build_mode.guidance().to_owned()),
    ];
    if let Some(error) = &form.validation_error {
        lines.push(Line::from(Span::styled(
            format!("Invalid value: {error}"),
            Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
        )));
    }
    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: true }), rows[0]);
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            "[Up/Down] field  [Left/Right] mode  [Enter] start  [Esc/q] cancel",
            Style::default().fg(Color::DarkGray),
        ))),
        rows[1],
    );
}

fn readonly_line(label: &str, value: &str) -> Line<'static> {
    Line::from(vec![
        Span::styled(format!("{label:<18}"), Style::default().fg(Color::DarkGray)),
        Span::raw(value.to_owned()),
    ])
}

fn field_line(form: &FormState, field: ConfigField, label: &str, value: &str) -> Line<'static> {
    let selected = form.selected == field;
    let marker = if selected { "> " } else { "  " };
    let style = if selected {
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default()
    };
    let cursor = (form.editing == Some(field)).then_some("|").unwrap_or("");
    Line::from(vec![
        Span::styled(marker, style),
        Span::styled(format!("{label:<18}"), style),
        Span::styled(format!("{value}{cursor}"), style),
    ])
}

fn guidance_line(text: String) -> Line<'static> {
    Line::from(Span::styled(
        format!("    {text}"),
        Style::default().fg(Color::DarkGray),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    fn default_args() -> Args {
        crate::parse_args_from(["input.map"].into_iter().map(str::to_owned)).unwrap()
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

    #[test]
    fn validation_matches_cli_metric_and_sample_boundaries() {
        for field in [
            ConfigField::ProbeSpacing,
            ConfigField::LightmapDensity,
            ConfigField::VoxelSize,
        ] {
            match validate_field(field, "0.25") {
                Ok(ValidatedField::Metric(value)) => {
                    assert!((value - 0.25).abs() < f32::EPSILON);
                }
                other => panic!("expected accepted metric, got {other:?}"),
            }
            assert!(validate_field(field, "0").is_err());
            assert!(validate_field(field, "inf").is_err());
        }

        let floor = lightmap_bake::SOFT_PROBE_SAMPLES;
        assert_eq!(
            validate_field(ConfigField::SoftShadowSamples, &floor.to_string()),
            Ok(ValidatedField::Samples(floor))
        );
        assert!(validate_field(ConfigField::SoftShadowSamples, &(floor - 1).to_string()).is_err());
        assert!(validate_field(ConfigField::SoftShadowSamples, "1.5").is_err());
    }

    #[test]
    fn untouched_density_keeps_map_precedence_and_displays_its_source() {
        let mut args = default_args();
        let form = FormState::from_args(&args, Some(0.025));

        assert_eq!(form.lightmap_density, "0.025");
        assert_eq!(form.density_source_label(), "map KVP");
        assert!(!form.lightmap_density_touched);
        apply_outcome(&mut args, &form).unwrap();
        assert_eq!(args.lightmap_density, None);
    }

    #[test]
    fn touched_density_becomes_the_cli_screen_override() {
        let mut args = default_args();
        let mut form = FormState::from_args(&args, Some(0.025));
        form.lightmap_density = "0.02".to_owned();
        form.lightmap_density_touched = true;

        apply_outcome(&mut args, &form).unwrap();
        assert!(
            matches!(args.lightmap_density, Some(value) if (value - 0.02).abs() < f32::EPSILON)
        );
    }

    #[test]
    fn build_mode_writeback_selects_exact_or_warm_cache_contract() {
        let mut args = default_args();
        let mut form = FormState::from_args(&args, None);

        form.build_mode = BuildMode::Production;
        apply_outcome(&mut args, &form).unwrap();
        assert!(args.release);
        assert!(args.no_cache);

        form.build_mode = BuildMode::RapidIteration;
        apply_outcome(&mut args, &form).unwrap();
        assert!(!args.release);
        assert!(!args.no_cache);
    }

    #[test]
    fn draw_config_renders_effective_density_and_survives_resize() {
        let form = FormState::from_args(&default_args(), Some(0.025));
        for (width, height) in [(100, 24), (52, 12)] {
            let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
            terminal.draw(|frame| draw_config(frame, &form)).unwrap();
            let text = buffer_text(&terminal);
            assert!(text.contains("Pre-bake configuration"));
            assert!(text.contains("map KVP"));
        }
    }
}
