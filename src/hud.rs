//! The player's UI while a level runs: a status bar along the top and a button bar along the bottom.
//! Buttons write the same `GameAction`s as the keyboard. The old text HUD and graph are the debug
//! overlay in `main.rs` (F3). See `docs/ui.md`.

use crate::audio::AudioSettings;
use crate::levels::Screen;
use crate::{Evo, GameAction, Playback, SPEEDS, View, Which, leader_gen};
use bevy::picking::hover::Hovered;
use bevy::prelude::*;
use bevy::ui_widgets::{Activate, ActivateOnPress, Button};

const BAR: Color = Color::srgba(1.0, 1.0, 1.0, 0.9);
const INK: Color = Color::srgb(0.1, 0.1, 0.15);
const BUTTON: Color = Color::srgb(0.94, 0.93, 0.9);
const BUTTON_HOVER: Color = Color::srgb(0.88, 0.88, 0.95);
/// A button whose setting is on: Auto while auto-running, the creature being watched.
const BUTTON_ON: Color = Color::srgb(0.8, 0.78, 0.96);
const TEXT_SIZE: f32 = 15.0;

#[derive(Component)]
struct TopBar;
#[derive(Component)]
struct BottomBar;
#[derive(Component, Clone, Copy)]
enum Status {
    Level,
    Progress,
    Stars,
}
#[derive(Component)]
struct HudButton(GameAction);
/// Shown only while browsing an earlier generation.
#[derive(Component)]
struct LatestChip;
#[derive(Component)]
struct WatchLabel;
#[derive(Component)]
struct SpeedLabel;
#[derive(Component)]
struct SoundLabel;
/// "Grab the creature and fling it", shown until a run's seed is picked.
#[derive(Component)]
struct KickoffBanner;
/// How far the leader has got toward the level's goal.
#[derive(Component)]
struct ProgressBar;
#[derive(Component)]
struct ProgressFill;

pub struct HudPlugin;

impl Plugin for HudPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, spawn_bars)
            .add_systems(Update, (show_bars, update_status, update_progress, update_buttons))
            .add_observer(on_activate);
    }
}

fn text(s: impl Into<String>) -> impl Bundle {
    (Text::new(s), TextFont::from_font_size(TEXT_SIZE), TextColor(INK))
}

fn bar(top: bool) -> Node {
    Node {
        position_type: PositionType::Absolute,
        left: px(8),
        right: px(8),
        top: if top { px(8) } else { Val::Auto },
        bottom: if top { Val::Auto } else { px(8) },
        flex_direction: FlexDirection::Row,
        flex_wrap: FlexWrap::Wrap,
        justify_content: JustifyContent::SpaceBetween,
        align_items: AlignItems::Center,
        column_gap: px(16),
        row_gap: px(6),
        padding: UiRect::axes(px(12), px(6)),
        border_radius: BorderRadius::all(px(10)),
        ..default()
    }
}

fn group() -> Node {
    Node { flex_direction: FlexDirection::Row, align_items: AlignItems::Center, column_gap: px(4), ..default() }
}

fn button(action: GameAction) -> impl Bundle {
    (
        Button,
        // Activating on release misses quick taps whose press and release land in one frame.
        ActivateOnPress,
        Hovered::default(),
        HudButton(action),
        Node {
            justify_content: JustifyContent::Center,
            align_items: AlignItems::Center,
            min_width: px(36),
            padding: UiRect::axes(px(12), px(9)),
            border_radius: BorderRadius::all(px(7)),
            ..default()
        },
        BackgroundColor(BUTTON),
    )
}

fn spawn_bars(mut commands: Commands) {
    commands.spawn((bar(true), BackgroundColor(BAR), TopBar, Visibility::Hidden)).with_children(|top| {
        top.spawn((text(""), Status::Level));
        top.spawn(Node { flex_direction: FlexDirection::Column, align_items: AlignItems::Center, row_gap: px(3), ..default() })
            .with_children(|col| {
                col.spawn((text(""), Status::Progress));
                col.spawn((
                    Node { width: px(160), height: px(5), border_radius: BorderRadius::all(px(3)), ..default() },
                    BackgroundColor(Color::srgb(0.85, 0.85, 0.85)),
                    ProgressBar,
                ))
                .with_children(|bar| {
                    bar.spawn((
                        Node { width: percent(0), height: percent(100), border_radius: BorderRadius::all(px(3)), ..default() },
                        BackgroundColor(Color::srgb_u8(240, 180, 20)),
                        ProgressFill,
                    ));
                });
            });
        top.spawn((text(""), Status::Stars));
    });

    commands
        .spawn((
            Node {
                position_type: PositionType::Absolute,
                top: px(64),
                width: percent(100),
                justify_content: JustifyContent::Center,
                ..default()
            },
            KickoffBanner,
            Visibility::Hidden,
        ))
        .with_children(|row| {
            row.spawn((
                Node { padding: UiRect::axes(px(16), px(10)), border_radius: BorderRadius::all(px(10)), ..default() },
                BackgroundColor(BAR),
            ))
            .with_children(|b| {
                b.spawn((
                    Text::new("Grab the creature and fling it to start evolving. Or press Space."),
                    TextFont::from_font_size(18.0),
                    TextColor(INK),
                ));
            });
        });

    let labelled = |b: &mut ChildSpawnerCommands, action: GameAction, label: &str| {
        b.spawn(button(action)).with_children(|b| {
            b.spawn(text(label));
        });
    };
    commands.spawn((bar(false), BackgroundColor(BAR), BottomBar, Visibility::Hidden)).with_children(|bottom| {
        bottom.spawn(group()).with_children(|g| {
            labelled(g, GameAction::NextGen, "⇥ Next gen");
            labelled(g, GameAction::ToggleAuto, "▶ Auto");
        });
        bottom.spawn(group()).with_children(|g| {
            g.spawn((text(""), WatchLabel, Node { margin: UiRect::right(px(6)), ..default() }));
            labelled(g, GameAction::EarlierGen, "‹");
            labelled(g, GameAction::Watch(Which::Leader), "Leader");
            labelled(g, GameAction::Watch(Which::Best), "Best");
            labelled(g, GameAction::Watch(Which::Median), "Median");
            labelled(g, GameAction::Watch(Which::Worst), "Worst");
            labelled(g, GameAction::LaterGen, "›");
            g.spawn((button(GameAction::LatestGen), LatestChip)).with_children(|b| {
                b.spawn(text("Latest ⇥"));
            });
        });
        bottom.spawn(group()).with_children(|g| {
            labelled(g, GameAction::Slower, "−");
            g.spawn(Node { min_width: px(44), justify_content: JustifyContent::Center, ..default() }).with_children(|c| {
                c.spawn((text(""), SpeedLabel));
            });
            labelled(g, GameAction::Faster, "+");
            labelled(g, GameAction::Replay, "↻ Replay");
        });
        bottom.spawn(group()).with_children(|g| {
            g.spawn(button(GameAction::CycleSound)).with_children(|b| {
                b.spawn((text(""), SoundLabel));
            });
            labelled(g, GameAction::Restart, "↺ Restart");
            labelled(g, GameAction::Menu, "Menu");
        });
    });
}

fn on_activate(ev: On<Activate>, buttons: Query<&HudButton>, mut actions: MessageWriter<GameAction>) {
    if let Ok(button) = buttons.get(ev.entity) {
        actions.write(button.0);
    }
}

type TopOnly = (With<TopBar>, Without<BottomBar>, Without<KickoffBanner>);
type BottomOnly = (With<BottomBar>, Without<TopBar>, Without<KickoffBanner>);
type BannerOnly = (With<KickoffBanner>, Without<TopBar>, Without<BottomBar>);

/// The top bar shows during a level and its results; the buttons only while it's playing; the
/// kick-off banner until the run's seed is picked.
fn show_bars(
    screen: Res<State<Screen>>,
    evo: Res<Evo>,
    mut top: Query<&mut Visibility, TopOnly>,
    mut bottom: Query<&mut Visibility, BottomOnly>,
    mut banner: Query<&mut Visibility, BannerOnly>,
) {
    let shown = |on: bool| if on { Visibility::Inherited } else { Visibility::Hidden };
    let screen = *screen.get();
    for mut vis in &mut top {
        vis.set_if_neq(shown(screen != Screen::Menu));
    }
    for mut vis in &mut bottom {
        vis.set_if_neq(shown(screen == Screen::Level));
    }
    for mut vis in &mut banner {
        vis.set_if_neq(shown(screen == Screen::Level && evo.awaiting_fling));
    }
}

fn update_progress(
    evo: Res<Evo>,
    mut bar: Query<&mut Node, (With<ProgressBar>, Without<ProgressFill>)>,
    mut fill: Query<&mut Node, (With<ProgressFill>, Without<ProgressBar>)>,
) {
    let goal = evo.setup.level.as_ref().map(|run| run.level.goal);
    let best = leader_gen(&evo).map_or(0.0, |i| evo.history[i].percentiles[0]);
    for mut node in &mut bar {
        let display = if goal.is_some() { Display::Flex } else { Display::None };
        if node.display != display {
            node.display = display;
        }
    }
    let width = percent((best / goal.unwrap_or(1.0)).clamp(0.0, 1.0) * 100.0);
    for mut node in &mut fill {
        if node.width != width {
            node.width = width;
        }
    }
}

fn stars(n: u8) -> String {
    (0..3).map(|i| if i < n { '★' } else { '☆' }).collect()
}

fn update_status(evo: Res<Evo>, mut texts: Query<(&Status, &mut Text)>) {
    let n = evo.history.len();
    let record = leader_gen(&evo).map(|i| evo.history[i].percentiles[0]);
    for (slot, mut text) in &mut texts {
        let s = match (slot, &evo.setup.level) {
            (Status::Level, run) => {
                let name = match run {
                    Some(run) => format!("{} · goal {} m", run.level.name, run.level.goal),
                    None => format!("Free play · {}", evo.setup.preset.short_name()),
                };
                // The seed came from the player's fling; it's shown so a run can be told apart.
                if evo.awaiting_fling { name } else { format!("{name} · seed {:08x}", evo.seed as u32) }
            }
            (Status::Progress, _) => {
                let mut s = match record {
                    Some(best) => match &evo.setup.level {
                        Some(run) => format!("Gen {n} · best {best:.2} of {} m", run.level.goal),
                        None => format!("Gen {n} · best {best:.2} m"),
                    },
                    None if evo.awaiting_fling => "Waiting for your fling".to_string(),
                    None => format!("Gen 0 · {} creatures", evo.cfg.population),
                };
                if evo.busy() {
                    s += " · evolving…";
                }
                s
            }
            (Status::Stars, Some(run)) => {
                let l = &run.level;
                match evo.finished_at {
                    Some(gens) if evo.finale.is_some() => format!("Gen {gens} made it. Watch it cross the flag"),
                    Some(gens) => format!("Goal reached in gen {gens}  {}", stars(l.stars_for(gens))),
                    // What the generation being evolved now would earn.
                    None if n < l.stars.three => format!("{} if you finish by gen {}", stars(3), l.stars.three),
                    None if n < l.stars.two => format!("{} if you finish by gen {}", stars(2), l.stars.two),
                    None => format!("{} for finishing", stars(1)),
                }
            }
            (Status::Stars, None) => String::new(),
        };
        if text.0 != s {
            text.0 = s;
        }
    }
}

#[allow(clippy::type_complexity)]
fn update_buttons(
    evo: Res<Evo>,
    view: Res<View>,
    playback: Res<Playback>,
    sound: Res<AudioSettings>,
    mut buttons: Query<(&HudButton, &Hovered, &mut BackgroundColor, Has<LatestChip>, &mut Node)>,
    mut labels: ParamSet<(
        Query<&mut Text, With<WatchLabel>>,
        Query<&mut Text, With<SpeedLabel>>,
        Query<&mut Text, With<SoundLabel>>,
    )>,
) {
    for (button, hovered, mut color, latest_chip, mut node) in &mut buttons {
        let on = match button.0 {
            GameAction::ToggleAuto => evo.auto,
            GameAction::Watch(which) => view.which == which,
            _ => false,
        };
        let c = if on {
            BUTTON_ON
        } else if hovered.get() {
            BUTTON_HOVER
        } else {
            BUTTON
        };
        if color.0 != c {
            color.0 = c;
        }
        if latest_chip {
            let display = if view.selected_gen.is_some() { Display::Flex } else { Display::None };
            if node.display != display {
                node.display = display;
            }
        }
    }

    // What's actually playing, which can lag the view while auto-running finishes a replay.
    let watching = if playback.free {
        "Fling me".to_string()
    } else if playback.nudged {
        format!("{} (nudged)", playback.caption)
    } else {
        playback.caption.clone()
    };
    let speed = match SPEEDS[view.speed_idx] {
        900 => "max".to_string(),
        s => format!("{s}×"),
    };
    let sound = match (sound.muted, sound.music_off) {
        (true, _) => "Muted",
        (false, true) => "Music off",
        (false, false) => "Sound on",
    };
    for mut text in &mut labels.p0() {
        set_text(&mut text, watching.clone());
    }
    for mut text in &mut labels.p1() {
        set_text(&mut text, speed.clone());
    }
    for mut text in &mut labels.p2() {
        set_text(&mut text, sound.to_string());
    }
}

/// Only touch the text when it changes, so it isn't laid out again every frame.
fn set_text(text: &mut Text, s: String) {
    if text.0 != s {
        text.0 = s;
    }
}
