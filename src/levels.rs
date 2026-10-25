//! Obstacle courses: level files, the Menu → Level → Results screens, and stars.
//!
//! Levels live in `assets/levels/*.level.ron` and hot-reload natively while the game runs. The web
//! can't list a folder over HTTP, so the files are named in `LEVEL_FILES`.

use bevy::asset::io::Reader;
use bevy::asset::{AssetLoader, LoadContext, LoadState};
use bevy::picking::hover::Hovered;
use bevy::prelude::*;
use bevy::ui_widgets::{Activate, ActivateOnPress, Button};
use serde::Deserialize;
use springy_evolution::sim::Rect;

/// The courses, in menu order.
const LEVEL_FILES: [&str; 4] = [
    "levels/1-first-steps.level.ron",
    "levels/2-low-bridge.level.ron",
    "levels/3-staircase.level.ron",
    "levels/4-speed-bumps.level.ron",
];

/// Population for levels unless a file sets one. Star thresholds were tuned against it: three stars
/// for about the best 40% of seeds, two for about 80%.
const LEVEL_POPULATION: usize = 500;

#[derive(States, Default, Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Screen {
    #[default]
    Menu,
    /// Playing a level, or free play.
    Level,
    /// A level's goal was reached.
    Results,
}

/// A course, as written in its `.level.ron` file. Distances are in metres, with y up.
#[derive(Asset, TypePath, Deserialize, Clone, Debug)]
pub struct Level {
    pub name: String,
    #[serde(default)]
    pub blurb: String,
    /// Distance the best creature of a generation must reach.
    pub goal: f32,
    pub stars: Stars,
    #[serde(default)]
    pub obstacles: Vec<Block>,
    #[serde(default)]
    pub population: Option<usize>,
}

/// Most generations allowed for three and for two stars. Finishing at all earns one.
#[derive(Deserialize, Clone, Copy, Debug)]
pub struct Stars {
    pub three: usize,
    pub two: usize,
}

/// A solid rectangle starting at `x` metres, `lift` metres off the ground.
#[derive(Deserialize, Clone, Copy, Debug)]
pub struct Block {
    pub x: f32,
    pub width: f32,
    pub height: f32,
    #[serde(default)]
    pub lift: f32,
}

impl Level {
    pub fn population(&self) -> usize {
        self.population.unwrap_or(LEVEL_POPULATION)
    }

    /// Obstacles in simulation units (5 per metre, y down).
    pub fn rects(&self) -> Vec<Rect> {
        self.obstacles
            .iter()
            .map(|b| Rect::new(b.x * 5.0, -b.lift * 5.0, (b.x + b.width) * 5.0, -(b.lift + b.height) * 5.0))
            .collect()
    }

    pub fn stars_for(&self, generations: usize) -> u8 {
        if generations <= self.stars.three {
            3
        } else if generations <= self.stars.two {
            2
        } else {
            1
        }
    }
}

#[derive(Default, TypePath)]
struct LevelLoader;

impl AssetLoader for LevelLoader {
    type Asset = Level;
    type Settings = ();
    type Error = BevyError;

    async fn load(&self, reader: &mut dyn Reader, _: &(), _: &mut LoadContext<'_>) -> Result<Level, BevyError> {
        let mut bytes = Vec::new();
        reader.read_to_end(&mut bytes).await?;
        Ok(ron::de::from_bytes(&bytes)?)
    }

    fn extensions(&self) -> &[&str] {
        &["level.ron"]
    }
}

#[derive(Resource)]
pub struct LevelList(pub Vec<Handle<Level>>);

/// Best stars earned on each level this session.
#[derive(Resource, Default)]
pub struct Progress(pub Vec<u8>);

/// What a button (or its keyboard shortcut) does.
#[derive(Component, Clone, Copy)]
enum UiAction {
    Play(usize),
    FreePlay,
    KeepEvolving,
    Retry,
    Next,
    Menu,
}

#[derive(Component)]
struct MenuRoot;
#[derive(Component)]
struct MenuFilled;
#[derive(Component)]
struct ResultsRoot;

pub struct LevelsPlugin;

impl Plugin for LevelsPlugin {
    fn build(&self, app: &mut App) {
        app.init_asset::<Level>()
            .init_asset_loader::<LevelLoader>()
            .init_state::<Screen>()
            .init_resource::<Progress>()
            .add_systems(Startup, load_levels)
            .add_systems(OnEnter(Screen::Menu), spawn_menu)
            .add_systems(OnExit(Screen::Menu), despawn::<MenuRoot>)
            .add_systems(OnEnter(Screen::Results), (record_progress, spawn_results).chain())
            .add_systems(OnExit(Screen::Results), despawn::<ResultsRoot>)
            .add_systems(Update, (fill_menu, button_hover, shortcut_keys, reload_level))
            .add_observer(on_activate);
    }
}

fn load_levels(mut commands: Commands, server: Res<AssetServer>) {
    let handles: Vec<Handle<Level>> = LEVEL_FILES.iter().map(|path| server.load(*path)).collect();
    commands.insert_resource(Progress(vec![0; handles.len()]));
    commands.insert_resource(LevelList(handles));
}

fn despawn<T: Component>(mut commands: Commands, roots: Query<Entity, With<T>>) {
    for e in &roots {
        commands.entity(e).despawn();
    }
}

// ------------------------------------------------------------------------------------------------
// Actions
// ------------------------------------------------------------------------------------------------

fn on_activate(
    ev: On<Activate>,
    actions: Query<&UiAction>,
    evo: Res<crate::Evo>,
    levels: Res<LevelList>,
    mut next: ResMut<NextState<Screen>>,
    mut commands: Commands,
) {
    if let Ok(action) = actions.get(ev.entity) {
        perform(*action, &evo, levels.0.len(), &mut next, &mut commands);
    }
}

fn perform(action: UiAction, evo: &crate::Evo, count: usize, next: &mut NextState<Screen>, commands: &mut Commands) {
    let current = evo.setup.level.as_ref().map(|l| l.index);
    let play = |index: Option<usize>, next: &mut NextState<Screen>, commands: &mut Commands| {
        commands.run_system_cached_with(crate::enter_level, index);
        next.set(Screen::Level);
    };
    match action {
        UiAction::Play(i) if i < count => play(Some(i), next, commands),
        UiAction::FreePlay => play(None, next, commands),
        UiAction::Retry => play(current, next, commands),
        UiAction::Next => {
            if let Some(i) = current.map(|i| i + 1).filter(|i| *i < count) {
                play(Some(i), next, commands);
            }
        }
        UiAction::KeepEvolving => next.set(Screen::Level),
        UiAction::Menu => next.set(Screen::Menu),
        UiAction::Play(_) => {}
    }
}

fn shortcut_keys(
    keys: Res<ButtonInput<KeyCode>>,
    screen: Res<State<Screen>>,
    evo: Res<crate::Evo>,
    levels: Res<LevelList>,
    mut next: ResMut<NextState<Screen>>,
    mut commands: Commands,
) {
    const DIGITS: [KeyCode; 9] = [
        KeyCode::Digit1,
        KeyCode::Digit2,
        KeyCode::Digit3,
        KeyCode::Digit4,
        KeyCode::Digit5,
        KeyCode::Digit6,
        KeyCode::Digit7,
        KeyCode::Digit8,
        KeyCode::Digit9,
    ];
    let pressed = |k| keys.just_pressed(k);
    let action = match screen.get() {
        Screen::Menu => DIGITS.iter().position(|k| pressed(*k)).map(UiAction::Play).or(pressed(KeyCode::KeyF).then_some(UiAction::FreePlay)),
        Screen::Level => pressed(KeyCode::Escape).then_some(UiAction::Menu),
        Screen::Results => [
            (KeyCode::Space, UiAction::KeepEvolving),
            (KeyCode::Enter, UiAction::Retry),
            (KeyCode::KeyN, UiAction::Next),
            (KeyCode::Escape, UiAction::Menu),
        ]
        .into_iter()
        .find(|(k, _)| pressed(*k))
        .map(|(_, a)| a),
    };
    if let Some(action) = action {
        perform(action, &evo, levels.0.len(), &mut next, &mut commands);
    }
}

/// Saving the level being played restarts it with the new layout.
fn reload_level(
    mut events: MessageReader<AssetEvent<Level>>,
    levels: Res<LevelList>,
    evo: Res<crate::Evo>,
    screen: Res<State<Screen>>,
    mut next: ResMut<NextState<Screen>>,
    mut commands: Commands,
) {
    let Some(index) = evo.setup.level.as_ref().map(|l| l.index) else { return };
    let changed = events.read().any(|e| matches!(e, AssetEvent::Modified { id } if *id == levels.0[index].id()));
    if changed && *screen.get() != Screen::Menu {
        info!("reloaded {}", LEVEL_FILES[index]);
        commands.run_system_cached_with(crate::enter_level, Some(index));
        next.set(Screen::Level);
    }
}

fn record_progress(evo: Res<crate::Evo>, mut progress: ResMut<Progress>) {
    if let (Some(run), Some(gens)) = (&evo.setup.level, evo.finished_at) {
        let stars = run.level.stars_for(gens);
        if let Some(best) = progress.0.get_mut(run.index) {
            *best = (*best).max(stars);
        }
    }
}

// ------------------------------------------------------------------------------------------------
// UI
// ------------------------------------------------------------------------------------------------

const PANEL: Color = Color::srgba(1.0, 1.0, 1.0, 0.92);
const BUTTON: Color = Color::srgb(0.93, 0.93, 0.96);
const BUTTON_HOVER: Color = Color::srgb(0.82, 0.86, 1.0);
const INK: Color = Color::srgb(0.1, 0.1, 0.15);
const FAINT: Color = Color::srgb(0.4, 0.4, 0.45);
const GOLD: Color = Color::srgb(1.0, 0.75, 0.1);
const EMPTY_STAR: Color = Color::srgb(0.82, 0.82, 0.85);

/// A full-screen layer that centres a panel.
fn overlay(marker: impl Component) -> impl Bundle {
    (
        Node {
            position_type: PositionType::Absolute,
            width: percent(100),
            height: percent(100),
            justify_content: JustifyContent::Center,
            align_items: AlignItems::Center,
            ..default()
        },
        marker,
    )
}

fn panel() -> impl Bundle {
    (
        Node {
            flex_direction: FlexDirection::Column,
            row_gap: px(10),
            padding: UiRect::all(px(24)),
            min_width: px(420),
            border_radius: BorderRadius::all(px(12)),
            ..default()
        },
        BackgroundColor(PANEL),
    )
}

fn text(s: impl Into<String>, size: f32, color: Color) -> impl Bundle {
    (Text::new(s), TextFont::from_font_size(size), TextColor(color))
}

fn button(action: UiAction) -> impl Bundle {
    (
        Button,
        // Activating on release misses quick clicks whose press and release land in one frame.
        ActivateOnPress,
        Hovered::default(),
        action,
        Node {
            flex_direction: FlexDirection::Row,
            justify_content: JustifyContent::SpaceBetween,
            align_items: AlignItems::Center,
            column_gap: px(16),
            padding: UiRect::axes(px(14), px(10)),
            border_radius: BorderRadius::all(px(8)),
            ..default()
        },
        BackgroundColor(BUTTON),
    )
}

/// Three dots, the first `n` gold. (The default font has no star glyph.)
fn star_row(parent: &mut ChildSpawnerCommands, n: u8, size: f32) {
    parent.spawn(Node { column_gap: px(size * 0.3), ..default() }).with_children(|row| {
        for i in 0..3 {
            row.spawn((
                Node { width: px(size), height: px(size), border_radius: BorderRadius::MAX, ..default() },
                BackgroundColor(if i < n { GOLD } else { EMPTY_STAR }),
            ));
        }
    });
}

type HoverChanged = (Changed<Hovered>, With<Button>);

fn button_hover(mut buttons: Query<(&Hovered, &mut BackgroundColor), HoverChanged>) {
    for (hovered, mut color) in &mut buttons {
        color.0 = if hovered.get() { BUTTON_HOVER } else { BUTTON };
    }
}

fn spawn_menu(mut commands: Commands) {
    commands.spawn(overlay(MenuRoot));
}

/// Fill the menu once every level file has loaded or failed, so errors show up there too.
fn fill_menu(
    mut commands: Commands,
    roots: Query<Entity, (With<MenuRoot>, Without<MenuFilled>)>,
    levels: Res<LevelList>,
    assets: Res<Assets<Level>>,
    server: Res<AssetServer>,
    progress: Res<Progress>,
) {
    let Ok(root) = roots.single() else { return };
    let states: Vec<LoadState> = levels.0.iter().map(|h| server.load_state(h)).collect();
    if states.iter().any(|s| matches!(s, LoadState::Loading | LoadState::NotLoaded)) {
        return;
    }
    commands.entity(root).insert(MenuFilled).with_children(|layer| {
        layer.spawn(panel()).with_children(|p| {
            p.spawn(text("Springy Evolution", 30.0, INK));
            p.spawn(text("Evolve a creature that reaches the flag. Fewer generations earn more stars.", 14.0, FAINT));
            for (i, (handle, state)) in levels.0.iter().zip(&states).enumerate() {
                let Some(level) = assets.get(handle) else {
                    let err = match state {
                        LoadState::Failed(e) => e.to_string(),
                        _ => "not loaded".into(),
                    };
                    p.spawn(text(format!("{}: {err}", LEVEL_FILES[i]), 13.0, Color::srgb(0.7, 0.0, 0.0)));
                    continue;
                };
                p.spawn(button(UiAction::Play(i))).with_children(|b| {
                    b.spawn(Node { flex_direction: FlexDirection::Column, row_gap: px(2), ..default() }).with_children(|col| {
                        col.spawn(text(format!("{}. {}", i + 1, level.name), 18.0, INK));
                        col.spawn(text(format!("{}  Goal {} m.", level.blurb, level.goal), 13.0, FAINT));
                    });
                    star_row(b, progress.0.get(i).copied().unwrap_or(0), 14.0);
                });
            }
            p.spawn(button(UiAction::FreePlay)).with_children(|b| {
                b.spawn(text("F. Free play: no goal, just evolve", 16.0, INK));
            });
            p.spawn(text("Click a level or press its number.", 12.0, FAINT));
        });
    });
}

fn spawn_results(mut commands: Commands, evo: Res<crate::Evo>, levels: Res<LevelList>) {
    let (Some(run), Some(gens)) = (&evo.setup.level, evo.finished_at) else { return };
    let level = &run.level;
    let stars = level.stars_for(gens);
    let best = evo.history.last().map_or(0.0, |s| s.percentiles[0]);
    let has_next = run.index + 1 < levels.0.len();
    commands.spawn(overlay(ResultsRoot)).with_children(|layer| {
        layer.spawn(panel()).with_children(|p| {
            p.spawn(text(format!("{} complete!", level.name), 28.0, INK));
            star_row(p, stars, 34.0);
            p.spawn(text(
                format!(
                    "Reached {:.2} m (goal {} m) in {gens} generation{}.",
                    best,
                    level.goal,
                    if gens == 1 { "" } else { "s" }
                ),
                16.0,
                INK,
            ));
            p.spawn(text(
                format!("Three stars: {} generations or fewer. Two stars: {} or fewer.", level.stars.three, level.stars.two),
                13.0,
                FAINT,
            ));
            let mut actions = vec![(UiAction::KeepEvolving, "Space. Keep evolving"), (UiAction::Retry, "Enter. Retry (new seed)")];
            if has_next {
                actions.push((UiAction::Next, "N. Next level"));
            }
            actions.push((UiAction::Menu, "Esc. Level select"));
            for (action, label) in actions {
                p.spawn(button(action)).with_children(|b| {
                    b.spawn(text(label, 16.0, INK));
                });
            }
        });
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn level_files_parse() {
        for path in LEVEL_FILES {
            let file = format!("{}/assets/{path}", env!("CARGO_MANIFEST_DIR"));
            let text = std::fs::read_to_string(&file).unwrap_or_else(|e| panic!("{file}: {e}"));
            let level: Level = ron::from_str(&text).unwrap_or_else(|e| panic!("{file}: {e}"));
            assert!(level.goal > 0.0 && level.stars.three <= level.stars.two, "{file}");
            assert_eq!(level.rects().len(), level.obstacles.len());
        }
    }
}
