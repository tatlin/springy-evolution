//! Bevy front end: watch creatures run, breed generations in the background, plot progress.

mod audio;
mod fling;
mod hud;
mod levels;

use audio::{AudioSettings, Landed, Milestone, SoundPlugin};
use bevy::asset::AssetMetaCheck;
use bevy::input::mouse::AccumulatedMouseScroll;
use bevy::prelude::*;
use hud::HudPlugin;
use levels::{Level, LevelList, LevelsPlugin, Screen};
use springy_evolution::sim::{self, Config, Creature, GenStats, Population, Sim};

/// Pixels per simulation unit (1 unit = 20 cm), same as the original's camzoom of 0.01.
const S: f32 = 100.0;
/// How far above the camera centre the ground sits, in pixels (original: 80% down the screen).
const GROUND_OFFSET: f32 = 216.0;
const SPEEDS: [u32; 8] = [1, 2, 4, 8, 16, 32, 64, 900];
/// Simulation steps per second at 1× (the original ran one step per frame at 60 fps).
const STEP_RATE: f32 = 60.0;
/// Seconds to hold the final pose before a replay restarts.
const HOLD_SECS: f32 = 2.0;
/// Fastest playback that still makes landing sounds; above it they'd be a wall of noise.
const LANDING_SOUND_MAX_SPEED: u32 = 4;
/// Downward speed (units per step) a node needs when it lands to make a sound.
const LANDING_SOUND_MIN_IMPACT: f32 = 0.012;
/// How far (m) behind its furthest post a creature must fall before it counts as slipping back,
/// so rocking on the line doesn't repeat the sound.
const SLIP_SLACK: f32 = 0.1;
/// A generation's best must fall this fraction below the previous best to count as a regression.
const REGRESSION_DROP: f32 = 0.05;
/// While auto-running, evolution pauses to show a new leader in full if it beats the last one shown
/// by this fraction, and by at least `RECORD_STEP_MIN` metres. Smaller gains don't stop the clock.
const RECORD_STEP_FRAC: f32 = 0.1;
const RECORD_STEP_MIN: f32 = 0.1;
/// Playback speed a level starts at (an index into `SPEEDS`): 2×.
const LEVEL_SPEED: usize = 1;

fn to_world(x: f32, y: f32) -> Vec2 {
    Vec2::new(x * S, -y * S)
}

// ------------------------------------------------------------------------------------------------
// Resources & components
// ------------------------------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq)]
enum Preset {
    Evolution3Web,
    CaryHuang,
}

impl Preset {
    fn config(self) -> Config {
        match self {
            Preset::Evolution3Web => Config::evolution3web(),
            Preset::CaryHuang => Config::cary_huang(),
        }
    }
    fn short_name(self) -> &'static str {
        match self {
            Preset::Evolution3Web => "evolution3WEB",
            Preset::CaryHuang => "Cary Huang",
        }
    }
    fn name(self) -> &'static str {
        match self {
            Preset::Evolution3Web => "evolution3WEB (truss seed, regrown muscles, hurdles)",
            Preset::CaryHuang => "Cary Huang original (random seed, full mutation)",
        }
    }
}

/// How to build a run: free play with the preset and command-line overrides, or a level.
#[derive(Clone)]
struct Setup {
    preset: Preset,
    population: usize,
    /// `--max-len` override for `Config::max_muscle_length`.
    max_muscle_length: Option<f32>,
    /// `--fixed-topology`: never add or remove nodes and muscles.
    fixed_topology: bool,
    /// `--min-connections` override for `Config::min_connections`.
    min_connections: Option<usize>,
    level: Option<LevelRun>,
}

#[derive(Clone)]
struct LevelRun {
    /// Position in `LevelList`.
    index: usize,
    level: Level,
}

impl Setup {
    fn config(&self) -> Config {
        // Levels ignore the preset and the command line, so their stars mean the same thing for
        // everyone: evolution3WEB settings with the level's obstacles and population.
        if let Some(run) = &self.level {
            let mut cfg = Config::evolution3web();
            cfg.population = run.level.population().max(2);
            cfg.rects = run.level.rects();
            return cfg;
        }
        let mut cfg = self.preset.config();
        cfg.population = self.population.max(2);
        cfg.mutate_topology = !self.fixed_topology;
        if let Some(n) = self.min_connections {
            cfg.min_connections = n;
        }
        if let Some(len) = self.max_muscle_length {
            cfg.max_muscle_length = len.max(0.5);
        }
        cfg
    }
}

/// Free play's setup, and its seed if one was given with `--seed` (otherwise the player flings).
#[derive(Resource)]
struct FreePlay {
    setup: Setup,
    seed: Option<u64>,
}

#[derive(Resource)]
struct Evo {
    setup: Setup,
    cfg: Config,
    seed: u64,
    /// `None` while a generation is being computed by `worker`.
    pop: Option<Population>,
    worker: Option<Worker>,
    history: Vec<GenStats>,
    /// A creature from the untested first generation, to watch before anything is evaluated.
    preview: Creature,
    auto: bool,
    generations_queued: u32,
    /// Nothing evolves until the player flings the creature (or presses Space), which picks the seed.
    awaiting_fling: bool,
    /// How many generations it took to reach the level's goal.
    finished_at: Option<usize>,
    /// The creature that reached the goal, until the player has watched it cross the flag.
    finale: Option<u64>,
    /// Distance of the last leader watched to the end. Auto-run pauses for leaders that beat it.
    shown_record: f32,
}

impl Evo {
    fn new(setup: Setup, seed: u64, awaiting_fling: bool) -> Self {
        let cfg = setup.config();
        let pop = Population::new(cfg.clone(), seed);
        Self {
            preview: pop.creatures[0].clone(),
            setup,
            cfg,
            seed,
            pop: Some(pop),
            worker: None,
            history: Vec::new(),
            // Evolving starts as soon as a level opens; `poll_worker` only runs in `Screen::Level`.
            auto: true,
            generations_queued: 0,
            awaiting_fling,
            finished_at: None,
            finale: None,
            shown_record: 0.0,
        }
    }

    /// Whether auto-run should wait for the leader to be shown before evolving further.
    fn record_to_show(&self) -> bool {
        let Some(i) = leader_gen(self) else { return false };
        let step = (self.shown_record.abs() * RECORD_STEP_FRAC).max(RECORD_STEP_MIN);
        self.history[i].best.fitness >= self.shown_record + step
    }

    fn busy(&self) -> bool {
        self.worker.is_some()
    }

    fn start_generation(&mut self) {
        if let Some(pop) = self.pop.take() {
            self.worker = Some(Worker::start(pop));
        }
    }
}

/// Computes one generation without stalling the window: on a background thread natively, and
/// a few milliseconds per frame on the web, where wasm has no threads.
#[cfg(not(target_arch = "wasm32"))]
struct Worker(std::thread::JoinHandle<Population>);

#[cfg(not(target_arch = "wasm32"))]
impl Worker {
    fn start(mut pop: Population) -> Self {
        Self(std::thread::spawn(move || {
            pop.step();
            pop
        }))
    }

    /// The finished population, or the worker back if it is still running.
    fn poll(self) -> Result<Population, Self> {
        if !self.0.is_finished() {
            return Err(self);
        }
        Ok(self.0.join().expect("generation thread panicked"))
    }
}

#[cfg(target_arch = "wasm32")]
struct Worker(Population);

#[cfg(target_arch = "wasm32")]
impl Worker {
    /// Per-frame time budget for testing creatures, leaving room to render at 60 fps.
    const BUDGET: std::time::Duration = std::time::Duration::from_millis(10);

    fn start(pop: Population) -> Self {
        Self(pop)
    }

    fn poll(mut self) -> Result<Population, Self> {
        let start = bevy::platform::time::Instant::now();
        while start.elapsed() < Self::BUDGET {
            if self.0.evaluate_batch(10) {
                self.0.finish_generation();
                return Ok(self.0);
            }
        }
        Err(self)
    }
}

/// A fresh seed when none is given on the command line.
#[cfg(not(target_arch = "wasm32"))]
fn random_seed() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos() as u64
}

/// The browser has no `SystemTime`, so ask it for random bits instead.
#[cfg(target_arch = "wasm32")]
fn random_seed() -> u64 {
    let half = || (js_sys::Math::random() * u32::MAX as f64) as u64;
    half() << 32 | half()
}

#[derive(Clone, Copy, PartialEq)]
enum Which {
    /// The best creature from any generation so far: the record holder.
    Leader,
    Best,
    Median,
    Worst,
}

/// Everything the player can do, from a key or an on-screen button. Keys and buttons both write
/// these, and `apply_actions` (plus `audio` for sound) carries them out.
#[derive(Message, Clone, Copy, PartialEq)]
enum GameAction {
    NextGen,
    ToggleAuto,
    Watch(Which),
    EarlierGen,
    LaterGen,
    LatestGen,
    Faster,
    Slower,
    Replay,
    Restart,
    /// Free play only.
    SwitchPreset,
    /// Sound on → music off → muted.
    CycleSound,
    Menu,
}

/// Whether the developer overlay (stats text, key list, percentile graph) shows. F3 toggles it.
#[derive(Resource)]
struct DebugUi(bool);

#[derive(Resource)]
struct View {
    /// `None` = follow the latest generation.
    selected_gen: Option<usize>,
    which: Which,
    speed_idx: usize,
    zoom: f32,
}

#[derive(Resource)]
struct Playback {
    source: Creature,
    sim: Sim,
    /// Long description for the debug overlay.
    label: String,
    /// Short description for the player's button bar, such as "Watching the leader, from gen 19".
    caption: String,
    /// Seconds spent holding the final pose.
    hold: f32,
    /// Simulation steps owed but not yet run, so playback follows real time at any frame rate.
    owed: f32,
    /// Furthest metre post passed so far this run, going forwards and backwards (0 or less).
    metre: i32,
    behind: i32,
    /// Whether the creature has fallen back behind `metre` since passing it.
    slipped: bool,
    /// Runs past the end of the 15 seconds: the kick-off creature, waiting to be flung.
    free: bool,
    /// A node the player is dragging, and where to (simulation units). See `fling.rs`.
    grab: Option<(usize, Vec2)>,
    /// The player poked this run, so its distance doesn't mean anything.
    nudged: bool,
    /// Played through to the end at least once.
    watched: bool,
}

impl Playback {
    fn new(source: &Creature, label: String, caption: String) -> Self {
        Self {
            sim: Sim::new(source),
            source: source.clone(),
            label,
            caption,
            hold: 0.0,
            owed: 0.0,
            metre: 0,
            behind: 0,
            slipped: false,
            free: false,
            grab: None,
            nudged: false,
            watched: false,
        }
    }

    /// An untested creature from generation 1, before anything has been evaluated.
    fn preview(evo: &Evo) -> Self {
        Self::new(&evo.preview, "Generation 1, creature #1 (untested)".into(), "Watching gen 1, untested".into())
    }

    /// The same creature from the start.
    fn replay(&self) -> Self {
        Self::new(&self.source, self.label.clone(), self.caption.clone())
    }
}

#[derive(Resource)]
struct NodeMesh(Handle<Mesh>);

#[derive(Component)]
struct NodeDot(usize);
#[derive(Component)]
struct MuscleBar(usize);
#[derive(Component)]
struct Obstacle;
#[derive(Component)]
struct Hud;
#[derive(Component)]
struct GraphBackdrop;
#[derive(Component)]
struct DistanceLabel;
#[derive(Component)]
struct DistanceArrow;

// ------------------------------------------------------------------------------------------------
// App
// ------------------------------------------------------------------------------------------------

fn main() {
    let args: Vec<String> = std::env::args().collect();
    fn parse_arg<T: std::str::FromStr>(args: &[String], name: &str) -> Option<T> {
        args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).and_then(|v| v.parse().ok())
    }
    let arg = |name: &str| parse_arg::<u64>(&args, name);
    let preset =
        if args.iter().any(|a| a == "--cary") { Preset::CaryHuang } else { Preset::Evolution3Web };
    let given_seed = arg("--seed");
    let population: usize = parse_arg(&args, "--pop").unwrap_or(1000);

    let debug = debug_requested(&args);
    let max_muscle_length = parse_arg(&args, "--max-len");
    let fixed_topology = args.iter().any(|a| a == "--fixed-topology");
    let min_connections = parse_arg(&args, "--min-connections");

    let free =
        Setup { preset, population, max_muscle_length, fixed_topology, min_connections, level: None };
    // The menu plays this creature in the background; nothing evolves until a run starts.
    let evo = Evo::new(free.clone(), given_seed.unwrap_or_else(random_seed), false);
    let playback = Playback::preview(&evo);

    let mut app = App::new();
    app.add_plugins(DefaultPlugins.set(WindowPlugin {
            primary_window: Some(Window {
                title: "Springy Evolution — Rust + Bevy".into(),
                resolution: (1280u32, 720u32).into(),
                // On the web, fill the page instead of a fixed 1280×720 canvas.
                fit_canvas_to_parent: true,
                ..default()
            }),
            ..default()
        }).set(AssetPlugin {
            // There are no .meta files, and on the web each check is a failed HTTP request.
            meta_check: AssetMetaCheck::Never,
            ..default()
        }));
    // Inter replaces Bevy's built-in font, which has no arrows, stars or play symbols. It must go
    // in before the first frame, when the text system reads the default font.
    let inter = Font::from_bytes(include_bytes!("../assets/fonts/Inter-Regular.ttf").to_vec());
    app.world_mut().resource_mut::<Assets<Font>>().insert(AssetId::default(), inter).unwrap();
    app.add_plugins((SoundPlugin, LevelsPlugin, HudPlugin))
        .add_message::<GameAction>()
        .insert_resource(DebugUi(debug))
        .insert_resource(FreePlay { setup: free, seed: given_seed })
        .insert_resource(ClearColor(Color::srgb_u8(120, 200, 255)))
        .insert_resource(evo)
        .insert_resource(playback)
        .insert_resource(View { selected_gen: None, which: Which::Leader, speed_idx: 0, zoom: 1.0 })
        .insert_resource(CameraRig { target_zoom: 1.0, last_view_zoom: 1.0, ..default() })
        .init_resource::<fling::Grab>()
        .add_systems(Startup, (setup, spawn_obstacles, spawn_record_marker))
        .add_systems(
            Update,
            (
                toggle_debug,
                keyboard_input.run_if(in_state(Screen::Level)),
                apply_actions.run_if(in_state(Screen::Level)),
                fling::grab_input.run_if(in_state(Screen::Level)),
                fling::kick_off.run_if(in_state(Screen::Level)),
                poll_worker.run_if(in_state(Screen::Level)),
                step_playback,
                update_record_marker,
                draw_creature,
                follow_camera,
                draw_graph,
                update_hud,
            )
                .chain(),
        );
    app.run();
}

/// `--debug` natively, or `?debug` in the page URL on the web.
#[cfg(not(target_arch = "wasm32"))]
fn debug_requested(args: &[String]) -> bool {
    args.iter().any(|a| a == "--debug")
}

#[cfg(target_arch = "wasm32")]
fn debug_requested(_: &[String]) -> bool {
    use js_sys::{JsString, Reflect};
    let search = Reflect::get(&js_sys::global(), &JsString::from("location"))
        .and_then(|location| Reflect::get(&location, &JsString::from("search")))
        .ok()
        .and_then(|search| search.as_string())
        .unwrap_or_default();
    search.trim_start_matches('?').split('&').any(|p| p == "debug" || p.starts_with("debug="))
}

fn setup(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
) {
    commands.insert_resource(NodeMesh(meshes.add(Circle::new(0.5))));

    // Camera, with the graph backdrop parented to it so it stays on screen.
    commands
        .spawn((Camera2d, Transform::from_xyz(0.0, GROUND_OFFSET, 0.0)))
        .with_children(|cam| {
            cam.spawn((
                Sprite::from_color(Color::srgba(1.0, 1.0, 1.0, 0.75), Vec2::new(480.0, 230.0)),
                Transform::from_xyz(390.0, 190.0, -10.0),
                GraphBackdrop,
            ));
        });

    // Ground.
    commands.spawn((
        // Deep enough to fill the bottom of the screen when zoomed far out on a huge creature.
        Sprite::from_color(Color::srgb_u8(0, 130, 0), Vec2::new(1.0e6, 1.0e5)),
        Transform::from_xyz(0.0, -5.0e4, 0.0),
    ));

    // Distance posts every metre (5 units), as in the original.
    for i in -20..=150 {
        let x = i as f32 * 5.0;
        commands.spawn((
            Sprite::from_color(Color::WHITE, Vec2::new(0.2 * S, 3.0 * S)),
            Transform::from_translation(to_world(x, -1.5).extend(0.1)),
        ));
        commands.spawn((
            Sprite::from_color(Color::WHITE, Vec2::new(2.0 * S, 1.0 * S)),
            Transform::from_translation(to_world(x, -2.5).extend(0.2)),
        ));
        commands.spawn((
            Text2d::new(format!("{i} m")),
            TextFont::from_font_size(42.0),
            TextColor(Color::srgb_u8(120, 120, 120)),
            Transform::from_translation(to_world(x, -2.5).extend(0.3)),
        ));
    }

    // Floating distance readout above the creature.
    commands.spawn((
        Sprite::from_color(Color::srgb_u8(120, 0, 255), Vec2::new(3.4 * S, 1.1 * S)),
        Transform::from_xyz(0.0, 4.25 * S, 3.0),
        DistanceArrow,
    ));
    commands.spawn((
        Text2d::new(""),
        TextFont::from_font_size(48.0),
        TextColor(Color::WHITE),
        Transform::from_xyz(0.0, 4.25 * S, 3.1),
        DistanceLabel,
    ));

    // HUD.
    commands.spawn((
        Text::new(""),
        TextFont::from_font_size(15.0),
        TextColor(Color::BLACK),
        // Below the player's top bar.
        Node { position_type: PositionType::Absolute, top: px(60.0), left: px(12.0), ..default() },
        Hud,
    ));
}

/// Obstacles, plus the goal flag in a level. Respawned whenever a run starts.
fn spawn_obstacles(mut commands: Commands, evo: Res<Evo>, old: Query<Entity, With<Obstacle>>) {
    for e in &old {
        commands.entity(e).despawn();
    }
    for r in &evo.cfg.rects {
        let size = Vec2::new((r.x2 - r.x1) * S, (r.y2 - r.y1) * S);
        let centre = to_world((r.x1 + r.x2) / 2.0, (r.y1 + r.y2) / 2.0);
        commands.spawn((
            // A shade darker than the ground, so roofs and steps read as obstacles.
            Sprite::from_color(Color::srgb_u8(0, 100, 0), size),
            Transform::from_translation(centre.extend(0.5)),
            Obstacle,
        ));
    }
    if let Some(run) = &evo.setup.level {
        let x = run.level.goal * 5.0;
        let red = Color::srgb_u8(230, 40, 40);
        commands.spawn((
            Sprite::from_color(Color::WHITE, Vec2::new(0.08 * S, 4.0 * S)),
            Transform::from_translation(to_world(x, -2.0).extend(0.4)),
            Obstacle,
        ));
        commands.spawn((
            Sprite::from_color(red, Vec2::new(1.4 * S, 0.8 * S)),
            Transform::from_translation(to_world(x + 0.7, -3.6).extend(0.4)),
            Obstacle,
        ));
        commands.spawn((
            Text2d::new("GOAL"),
            TextFont::from_font_size(40.0),
            TextColor(Color::WHITE),
            Transform::from_translation(to_world(x + 0.7, -3.6).extend(0.45)),
            Obstacle,
        ));
    }
}

/// The flag at the best distance so far: what the creature on screen is trying to beat.
#[derive(Component)]
struct RecordMarker;
#[derive(Component)]
struct RecordLabel;

fn spawn_record_marker(mut commands: Commands) {
    let gold = Color::srgb_u8(240, 180, 20);
    commands
        .spawn((Transform::from_xyz(0.0, 0.0, 0.35), Visibility::Hidden, RecordMarker))
        .with_children(|m| {
            m.spawn((Sprite::from_color(gold, Vec2::new(0.06 * S, 3.0 * S)), Transform::from_xyz(0.0, 1.5 * S, 0.0)));
            m.spawn((Sprite::from_color(gold, Vec2::new(1.0 * S, 0.5 * S)), Transform::from_xyz(0.5 * S, 2.75 * S, 0.0)));
            m.spawn((
                Text2d::new(""),
                TextFont::from_font_size(30.0),
                TextColor(Color::srgb_u8(80, 50, 0)),
                Transform::from_xyz(0.5 * S, 2.75 * S, 0.1),
                RecordLabel,
            ));
        });
}

fn update_record_marker(
    evo: Res<Evo>,
    screen: Res<State<Screen>>,
    mut marker: Query<(&mut Transform, &mut Visibility), With<RecordMarker>>,
    mut label: Query<&mut Text2d, With<RecordLabel>>,
) {
    let best = leader_gen(&evo).map(|i| evo.history[i].best.fitness).filter(|d| d.is_finite() && *d > 0.0);
    let Ok((mut tf, mut vis)) = marker.single_mut() else { return };
    let show = best.is_some() && *screen.get() != Screen::Menu;
    vis.set_if_neq(if show { Visibility::Inherited } else { Visibility::Hidden });
    let Some(best) = best else { return };
    tf.translation.x = best * 5.0 * S;
    if let Ok(mut text) = label.single_mut() {
        let s = format!("{best:.2}");
        if text.0 != s {
            text.0 = s;
        }
    }
}

/// Start a fresh run of `setup`. With no seed, the creature waits for the player to fling it
/// (`fling.rs`), and the fling picks the seed.
fn start_run(
    In((setup, seed)): In<(Setup, Option<u64>)>,
    mut evo: ResMut<Evo>,
    mut view: ResMut<View>,
    mut playback: ResMut<Playback>,
    mut commands: Commands,
) {
    // Any running worker is detached; its result is simply dropped.
    *evo = Evo::new(setup, seed.unwrap_or_else(random_seed), seed.is_none());
    view.selected_gen = None;
    view.which = Which::Leader;
    if evo.setup.level.is_some() {
        view.speed_idx = LEVEL_SPEED;
    }
    *playback = Playback::preview(&evo);
    playback.free = evo.awaiting_fling;
    commands.run_system_cached(spawn_obstacles);
}

/// The fling (or Space) picked `seed`: build generation 1 from it and start evolving. The playback
/// is left alone, so the flung creature keeps tumbling until the first generation is in.
fn begin_evolution(In(seed): In<u64>, mut evo: ResMut<Evo>) {
    if evo.awaiting_fling {
        *evo = Evo::new(evo.setup.clone(), seed, false);
    }
}

/// Start level `index` of `LevelList`, or free play for `None`.
fn enter_level(
    In(index): In<Option<usize>>,
    levels: Res<LevelList>,
    assets: Res<Assets<Level>>,
    free: Res<FreePlay>,
    mut commands: Commands,
) {
    let (setup, seed) = match index {
        Some(i) => {
            let Some(level) = levels.0.get(i).and_then(|h| assets.get(h)) else { return };
            let run = LevelRun { index: i, level: level.clone() };
            (Setup { level: Some(run), ..free.setup.clone() }, None)
        }
        None => (free.setup.clone(), free.seed),
    };
    commands.run_system_cached_with(start_run, (setup, seed));
}

// ------------------------------------------------------------------------------------------------
// Systems
// ------------------------------------------------------------------------------------------------

/// Index of the generation whose best scored highest. Ties go to the earliest, so the leader only
/// changes on a strictly better score.
fn leader_gen(evo: &Evo) -> Option<usize> {
    let mut leader = None;
    for (i, s) in evo.history.iter().enumerate() {
        if leader.is_none_or(|l: usize| s.percentiles[0] > evo.history[l].percentiles[0]) {
            leader = Some(i);
        }
    }
    leader
}

/// The generation being watched: the leader's, a browsed one, or the latest.
fn watched_gen(evo: &Evo, view: &View) -> Option<usize> {
    match (view.selected_gen, view.which) {
        (Some(i), _) => Some(i),
        (None, Which::Leader) => leader_gen(evo),
        (None, _) => evo.history.len().checked_sub(1),
    }
}

/// A playback of the creature the view points at, or `None` before the first generation.
fn watch_target(evo: &Evo, view: &View) -> Option<Playback> {
    let idx = watched_gen(evo, view)?;
    let stats = evo.history.get(idx)?;
    let (c, name) = match view.which {
        Which::Leader if view.selected_gen.is_none() => (&stats.best, "leader: best"),
        Which::Leader | Which::Best => (&stats.best, "best"),
        Which::Median => (&stats.median, "median"),
        Which::Worst => (&stats.worst, "worst"),
    };
    let label = format!(
        "Generation {} — {name} creature (S{:02}, scored {:.3} m)",
        idx + 1,
        c.species(),
        c.fitness
    );
    let caption = match (view.which, view.selected_gen) {
        (Which::Leader, None) => format!("Watching the leader, from gen {}", idx + 1),
        (Which::Leader | Which::Best, _) => format!("Watching gen {}'s best", idx + 1),
        (Which::Median, _) => format!("Watching gen {}'s median", idx + 1),
        (Which::Worst, _) => format!("Watching gen {}'s worst", idx + 1),
    };
    Some(Playback::new(c, label, caption))
}

/// Keys write the same actions as the on-screen buttons. Zoom stays here: it has no button.
fn keyboard_input(
    keys: Res<ButtonInput<KeyCode>>,
    scroll: Res<AccumulatedMouseScroll>,
    time: Res<Time>,
    mut view: ResMut<View>,
    mut actions: MessageWriter<GameAction>,
) {
    const KEYS: [(KeyCode, GameAction); 14] = [
        (KeyCode::Space, GameAction::NextGen),
        (KeyCode::KeyA, GameAction::ToggleAuto),
        (KeyCode::KeyL, GameAction::Watch(Which::Leader)),
        (KeyCode::Digit1, GameAction::Watch(Which::Best)),
        (KeyCode::Digit2, GameAction::Watch(Which::Median)),
        (KeyCode::Digit3, GameAction::Watch(Which::Worst)),
        (KeyCode::ArrowLeft, GameAction::EarlierGen),
        (KeyCode::ArrowRight, GameAction::LaterGen),
        (KeyCode::End, GameAction::LatestGen),
        (KeyCode::ArrowUp, GameAction::Faster),
        (KeyCode::ArrowDown, GameAction::Slower),
        (KeyCode::Enter, GameAction::Replay),
        (KeyCode::KeyR, GameAction::Restart),
        (KeyCode::KeyP, GameAction::SwitchPreset),
    ];
    for (key, action) in KEYS {
        if keys.just_pressed(key) {
            actions.write(action);
        }
    }

    // Zoom (wheel or -/=).
    let mut z = 0.0;
    if keys.pressed(KeyCode::Minus) {
        z += 1.2 * time.delta_secs();
    }
    if keys.pressed(KeyCode::Equal) {
        z -= 1.2 * time.delta_secs();
    }
    z -= scroll.delta.y.clamp(-5.0, 5.0) * 0.05;
    view.zoom = (view.zoom * (1.0 + z)).clamp(0.3, 8.0);
}

fn toggle_debug(keys: Res<ButtonInput<KeyCode>>, mut debug: ResMut<DebugUi>) {
    if keys.just_pressed(KeyCode::F3) {
        debug.0 = !debug.0;
    }
}

fn apply_actions(
    mut actions: MessageReader<GameAction>,
    mut evo: ResMut<Evo>,
    mut view: ResMut<View>,
    mut playback: ResMut<Playback>,
    mut screen: ResMut<NextState<Screen>>,
    mut commands: Commands,
) {
    let mut retarget = false;
    for action in actions.read() {
        let n = evo.history.len();
        match *action {
            // Before the fling, either one skips it with a random seed.
            GameAction::NextGen | GameAction::ToggleAuto if evo.awaiting_fling => {
                commands.run_system_cached_with(begin_evolution, random_seed());
            }
            GameAction::NextGen => evo.generations_queued += 1,
            GameAction::ToggleAuto => evo.auto = !evo.auto,
            GameAction::Watch(which) => {
                view.which = which;
                // The leader spans every generation, so watching it stops browsing.
                if which == Which::Leader {
                    view.selected_gen = None;
                }
                retarget = true;
            }
            GameAction::EarlierGen | GameAction::LaterGen if n > 0 => {
                // Browse from whichever generation is on screen.
                let at = watched_gen(&evo, &view).unwrap_or(n - 1);
                view.selected_gen = if *action == GameAction::EarlierGen {
                    Some(at.saturating_sub(1))
                } else if at + 1 >= n - 1 {
                    None
                } else {
                    Some(at + 1)
                };
                if view.which == Which::Leader {
                    view.which = Which::Best;
                }
                retarget = true;
            }
            GameAction::LatestGen => {
                view.selected_gen = None;
                retarget = true;
            }
            GameAction::Faster => view.speed_idx = (view.speed_idx + 1).min(SPEEDS.len() - 1),
            GameAction::Slower => view.speed_idx = view.speed_idx.saturating_sub(1),
            GameAction::Replay => *playback = playback.replay(),
            GameAction::Restart | GameAction::SwitchPreset => {
                let mut setup = evo.setup.clone();
                if *action == GameAction::SwitchPreset {
                    if setup.level.is_some() {
                        continue;
                    }
                    setup.preset = match setup.preset {
                        Preset::Evolution3Web => Preset::CaryHuang,
                        Preset::CaryHuang => Preset::Evolution3Web,
                    };
                }
                // A new attempt starts with a new fling.
                commands.run_system_cached_with(start_run, (setup, None));
                return;
            }
            GameAction::Menu => screen.set(Screen::Menu),
            GameAction::EarlierGen | GameAction::LaterGen | GameAction::CycleSound => {}
        }
    }
    if retarget && let Some(target) = watch_target(&evo, &view) {
        *playback = target;
    }
}

/// "Keep evolving" on the results screen goes back to auto-running.
fn resume_auto(mut evo: ResMut<Evo>) {
    evo.auto = true;
}

fn poll_worker(
    mut evo: ResMut<Evo>,
    mut view: ResMut<View>,
    mut playback: ResMut<Playback>,
    mut milestones: MessageWriter<Milestone>,
) {
    let mut goal = false;
    let finished = evo.worker.take().and_then(|w| w.poll().map_err(|w| evo.worker = Some(w)).ok());
    if let Some(pop) = finished {
        if let Some(stats) = pop.latest() {
            let best = stats.percentiles[0];
            if let Some(prev) = evo.history.last().map(|s| s.percentiles[0]) {
                let record = evo.history.iter().map(|s| s.percentiles[0]).fold(f32::NEG_INFINITY, f32::max);
                if best > record + 1e-3 {
                    milestones.write(Milestone::NewRecord);
                } else if best < prev - (prev.abs() * REGRESSION_DROP).max(0.01) {
                    milestones.write(Milestone::Regressed);
                }
            }
            evo.history.push(stats.clone());
            // Reaching a level's goal stops evolving and plays the winner. The results show when
            // the player sees it cross the flag (`step_playback`).
            let target = evo.setup.level.as_ref().map(|run| run.level.goal);
            if let Some(target) = target
                && evo.finished_at.is_none()
                && best >= target
            {
                evo.finished_at = Some(evo.history.len());
                evo.finale = Some(stats.best.id);
                evo.auto = false;
                evo.generations_queued = 0;
                view.selected_gen = None;
                view.which = Which::Leader;
                goal = true;
            }
        }
        evo.pop = Some(pop);
        // Stepping by hand, show a new target straight away. While auto-running, `step_playback`
        // moves on when the current replay ends, so runs aren't cut off mid-way.
        let first = evo.history.len() == 1;
        if (goal || view.selected_gen.is_none() && (!evo.auto || first))
            && let Some(target) = watch_target(&evo, &view)
            && (goal || first || target.source.id != playback.source.id)
        {
            *playback = target;
        }
    }
    // Auto-run pauses while a new leader is shown, so each real step forward gets watched.
    let paced = evo.auto
        && view.which == Which::Leader
        && view.selected_gen.is_none()
        && view.speed_idx < SPEEDS.len() - 1
        && evo.record_to_show();
    if !evo.busy() && !evo.awaiting_fling && !paced && (evo.auto || evo.generations_queued > 0) {
        evo.generations_queued = evo.generations_queued.saturating_sub(1);
        evo.start_generation();
    }
}

fn on_ground(n: &sim::Node) -> bool {
    n.y + n.m / 2.0 >= -1e-4
}

fn step_playback(
    mut evo: ResMut<Evo>,
    mut view: ResMut<View>,
    time: Res<Time>,
    mut pb: ResMut<Playback>,
    mut landed: MessageWriter<Landed>,
    mut milestones: MessageWriter<Milestone>,
    mut screen: ResMut<NextState<Screen>>,
) {
    // The kick-off creature runs until it's flung and the first generation is in.
    let steps = if pb.free { u32::MAX } else { evo.cfg.sim_steps };
    let dt = time.delta_secs();
    if pb.sim.timer < steps {
        let speed = SPEEDS[view.speed_idx] as f32;
        // Catch up at most a tenth of a second after a stall (or a hidden browser tab).
        pb.owed = (pb.owed + dt * STEP_RATE * speed).min(speed * STEP_RATE * 0.1);
        if view.speed_idx == SPEEDS.len() - 1 {
            pb.owed = steps as f32; // "instant"
        }
        let thuds = SPEEDS[view.speed_idx] <= LANDING_SOUND_MAX_SPEED;
        while pb.owed >= 1.0 && pb.sim.timer < steps {
            pb.owed -= 1.0;
            fling::steer(&mut pb);
            if !thuds {
                pb.sim.step(&evo.cfg);
                continue;
            }
            let before: Vec<(bool, f32)> = pb.sim.nodes.iter().map(|n| (on_ground(n), n.vy)).collect();
            pb.sim.step(&evo.cfg);
            for (n, (was_down, vy)) in pb.sim.nodes.iter().zip(before) {
                if !was_down && on_ground(n) && vy > LANDING_SOUND_MIN_IMPACT {
                    landed.write(Landed { speed: vy, friction: n.f });
                }
            }
        }
        // Progress sounds, though not in "instant" playback.
        let audible = view.speed_idx < SPEEDS.len() - 1;
        let mut say = |m: Milestone| {
            if audible {
                milestones.write(m);
            }
        };
        let d = pb.sim.distance_m();
        if d.is_finite() {
            if d.floor() as i32 > pb.metre {
                pb.metre = d.floor() as i32;
                say(Milestone::Metre(pb.metre));
            } else if (d.ceil() as i32) < pb.behind {
                pb.behind = d.ceil() as i32;
                say(Milestone::Metre(pb.behind));
            }
            if pb.metre >= 1 {
                if !pb.slipped && d < pb.metre as f32 - SLIP_SLACK {
                    pb.slipped = true;
                    say(Milestone::SlippedBack);
                } else if d >= pb.metre as f32 {
                    pb.slipped = false;
                }
            }
            if pb.sim.timer >= steps && d < 0.0 {
                say(Milestone::EndedBehind);
            }
            // The level ends when the player sees the winner cross the flag, not when it's scored.
            let goal = evo.setup.level.as_ref().map_or(f32::INFINITY, |run| run.level.goal);
            if evo.finale == Some(pb.source.id) && !pb.nudged && d >= goal {
                evo.finale = None;
                milestones.write(Milestone::GoalReached);
                screen.set(Screen::Results);
            }
        }
        if pb.sim.timer >= steps && !pb.watched {
            pb.watched = true;
            // Watching a leader to the end lets auto-run carry on (see `Evo::record_to_show`).
            if leader_gen(&evo).is_some_and(|i| evo.history[i].best.id == pb.source.id) {
                evo.shown_record = evo.shown_record.max(pb.source.fitness);
            }
        }
    } else {
        pb.hold += dt;
        if pb.hold > HOLD_SECS {
            // A finale nobody saw (the player looked away, or poked it) plays again.
            if evo.finale.is_some() {
                view.selected_gen = None;
                view.which = Which::Leader;
            }
            // Replay, or move on to whatever the view points at now (a new leader, say).
            *pb = watch_target(&evo, &view).unwrap_or_else(|| pb.replay());
        }
    }
}

fn node_color(f: f32) -> Color {
    // White → red for slippery → grippy, then red → black (the original's palette).
    if f <= 0.5 {
        let g = (1.0 - 2.0 * f).clamp(0.0, 1.0);
        Color::srgb(1.0, g, g)
    } else {
        Color::srgb((2.0 - 2.0 * f).clamp(0.0, 1.0), 0.0, 0.0)
    }
}

fn draw_creature(
    mut commands: Commands,
    pb: Res<Playback>,
    mesh: Res<NodeMesh>,
    mut materials: ResMut<Assets<ColorMaterial>>,
    mut dots: Query<
        (&NodeDot, &mut Transform, &mut Visibility, &MeshMaterial2d<ColorMaterial>),
        Without<MuscleBar>,
    >,
    mut bars: Query<(&MuscleBar, &mut Transform, &mut Visibility, &mut Sprite), Without<NodeDot>>,
    cam: Query<&Transform, (With<Camera2d>, Without<NodeDot>, Without<MuscleBar>)>,
) {
    // When zoomed far out, keep nodes and muscles a few pixels wide so they don't vanish.
    let cam_scale = cam.single().map_or(1.0, |c| c.scale.x);
    let nodes = &pb.sim.nodes;
    let muscles = &pb.sim.muscles;

    // Grow the entity pools if needed (new entities show up next frame).
    let have_dots = dots.iter().len();
    for i in have_dots..nodes.len() {
        commands.spawn((
            Mesh2d(mesh.0.clone()),
            MeshMaterial2d(materials.add(ColorMaterial::from_color(Color::WHITE))),
            Transform::default(),
            NodeDot(i),
        ));
    }
    let have_bars = bars.iter().len();
    for i in have_bars..muscles.len() {
        commands.spawn((Sprite::from_color(Color::BLACK, Vec2::ONE), Transform::default(), MuscleBar(i)));
    }

    for (dot, mut tf, mut vis, mat) in &mut dots {
        let Some(n) = nodes.get(dot.0) else {
            *vis = Visibility::Hidden;
            continue;
        };
        *vis = Visibility::Visible;
        tf.translation = to_world(n.x, n.y).extend(2.0 + dot.0 as f32 * 0.001);
        tf.scale = Vec3::splat((n.m * S).max(8.0 * cam_scale));
        if let Some(mut m) = materials.get_mut(&mat.0) {
            m.color = node_color(n.f);
        }
    }

    for (bar, mut tf, mut vis, mut sprite) in &mut bars {
        let Some(m) = muscles.get(bar.0) else {
            *vis = Visibility::Hidden;
            continue;
        };
        let (Some(a), Some(b)) = (nodes.get(m.c1), nodes.get(m.c2)) else {
            *vis = Visibility::Hidden;
            continue;
        };
        *vis = Visibility::Visible;
        let (pa, pb_) = (to_world(a.x, a.y), to_world(b.x, b.y));
        let d = pb_ - pa;
        let width = (if m.contracted { 0.2 } else { 0.1 } * S).max(3.0 * cam_scale);
        sprite.custom_size = Some(Vec2::new(d.length(), width));
        sprite.color = Color::srgba_u8(70, 35, 0, (m.rigidity * 3000.0).clamp(0.0, 255.0) as u8);
        tf.translation = ((pa + pb_) / 2.0).extend(1.0 + bar.0 as f32 * 0.001);
        tf.rotation = Quat::from_rotation_z(d.y.atan2(d.x));
    }
}

/// World-space bounding box of the creature, or `None` if it has no finite nodes.
fn creature_bounds(pb: &Playback) -> Option<Rect> {
    pb.sim
        .nodes
        .iter()
        .filter(|n| n.x.is_finite() && n.y.is_finite())
        .map(|n| {
            let r = n.m * S / 2.0;
            Rect::from_center_half_size(to_world(n.x, n.y), Vec2::splat(r))
        })
        .reduce(|a, b| a.union(b))
}

/// Fraction of the half-screen the creature can wander from the camera centre before the
/// camera starts to follow. Keeps the view still while a creature rocks in place.
const DEAD_ZONE: f32 = 0.2;
/// Zoom back in only once the creature fits at this fraction of the current zoom, so the view
/// doesn't pump in and out as limbs extend and contract.
const ZOOM_IN_SLACK: f32 = 0.7;

/// Smallest camera scale that keeps everything within `reach` of `focus_x` on screen, plus the
/// dead zone, with the ground in its usual place and room above for the distance readout.
fn zoom_to_fit(b: Rect, reach: f32, window: &Window) -> f32 {
    const MARGIN: f32 = 0.5 * S;
    const READOUT_ROOM: f32 = 2.0 * S;
    let fit_w = (reach + MARGIN) / ((1.0 - DEAD_ZONE) * window.width() / 2.0);
    let fit_h = b.max.y / (window.height() / 2.0 + GROUND_OFFSET - READOUT_ROOM);
    fit_w.max(fit_h)
}

/// Critically damped spring toward `target` (Unity's SmoothDamp): no overshoot, no jitter.
fn smooth_damp(current: f32, target: f32, vel: &mut f32, smooth_time: f32, dt: f32) -> f32 {
    let omega = 2.0 / smooth_time.max(1e-4);
    let x = omega * dt;
    let decay = 1.0 / (1.0 + x + 0.48 * x * x + 0.235 * x * x * x);
    let change = current - target;
    let temp = (*vel + omega * change) * dt;
    *vel = (*vel - omega * temp) * decay;
    target + (change + temp) * decay
}

type ArrowOnly = (With<DistanceArrow>, Without<DistanceLabel>);

/// Camera follow state that persists between frames.
#[derive(Resource, Default)]
struct CameraRig {
    /// Where the camera is trying to centre; only moves when the creature leaves the dead zone.
    focus_x: f32,
    vel_x: f32,
    target_zoom: f32,
    zoom_vel: f32,
    last_view_zoom: f32,
}

fn follow_camera(
    pb: Res<Playback>,
    view: Res<View>,
    time: Res<Time>,
    mut rig: ResMut<CameraRig>,
    window: Query<&Window, With<bevy::window::PrimaryWindow>>,
    mut cam: Query<&mut Transform, (With<Camera2d>, Without<DistanceLabel>, Without<DistanceArrow>)>,
    mut label: Query<(&mut Transform, &mut Text2d), (With<DistanceLabel>, Without<DistanceArrow>)>,
    mut arrow: Query<(&mut Transform, &mut Sprite), ArrowOnly>,
) {
    let cx = pb.sim.center_x() * S;
    let Some(bounds) = creature_bounds(&pb).filter(|_| cx.is_finite()) else { return };
    let Ok(window) = window.single() else { return };
    let Ok(mut tf) = cam.single_mut() else { return };
    let dt = time.delta_secs();

    // Zoom: out promptly when the creature doesn't fit, back in slowly and only with slack,
    // and straight away when the user changes the zoom themselves.
    let reach = (bounds.max.x - cx).max(cx - bounds.min.x);
    let wanted = view.zoom.max(zoom_to_fit(bounds, reach, window));
    let user_zoomed = view.zoom != rig.last_view_zoom;
    if user_zoomed || wanted > rig.target_zoom || wanted < rig.target_zoom * ZOOM_IN_SLACK {
        rig.target_zoom = wanted;
    }
    rig.last_view_zoom = view.zoom;
    let zoom_time = if rig.target_zoom > tf.scale.x { 0.3 } else { 1.2 };
    let target_zoom = rig.target_zoom;
    let zoom = smooth_damp(tf.scale.x, target_zoom, &mut rig.zoom_vel, zoom_time, dt);

    // Pan: drag the focus point along only when the creature's centre of mass leaves the dead
    // zone, then ease the camera toward it.
    let dead = DEAD_ZONE * zoom * window.width() / 2.0;
    rig.focus_x = rig.focus_x.clamp(cx - dead, cx + dead);
    let focus_x = rig.focus_x;
    let x = smooth_damp(tf.translation.x, focus_x, &mut rig.vel_x, 0.35, dt);
    tf.translation = Vec3::new(x, GROUND_OFFSET * zoom, tf.translation.z);
    tf.scale = Vec3::splat(zoom);

    // Keep the readout above the creature and the same size on screen.
    let readout_y = (4.25 * S).max(bounds.max.y + 0.9 * S * zoom);
    if let Ok((mut ltf, mut text)) = label.single_mut() {
        ltf.translation = Vec3::new(cx, readout_y, ltf.translation.z);
        ltf.scale = tf.scale;
        text.0 = format!("{:.2} m", pb.sim.distance_m());
    }
    if let Ok((mut atf, mut sprite)) = arrow.single_mut() {
        atf.translation = Vec3::new(cx, readout_y, atf.translation.z);
        atf.scale = tf.scale;
        // Grey once poked: the distance no longer means anything.
        sprite.color = if pb.nudged { Color::srgb_u8(130, 130, 140) } else { Color::srgb_u8(120, 0, 255) };
    }
}

fn draw_graph(
    evo: Res<Evo>,
    view: Res<View>,
    screen: Res<State<Screen>>,
    debug: Res<DebugUi>,
    cam: Query<&Transform, With<Camera2d>>,
    mut backdrop: Query<&mut Visibility, With<GraphBackdrop>>,
    mut g: Gizmos,
) {
    let Ok(cam) = cam.single() else { return };
    let show = debug.0 && *screen.get() != Screen::Menu;
    for mut vis in &mut backdrop {
        *vis = if show { Visibility::Inherited } else { Visibility::Hidden };
    }
    if !show {
        return;
    }
    let h = &evo.history;
    if h.len() < 2 {
        return;
    }
    // The camera may be zoomed out past `view.zoom` to fit the creature; follow its real scale.
    let z = cam.scale.x;
    let origin = cam.translation.truncate();
    // Graph box in screen pixels relative to the camera centre (matches the backdrop sprite).
    let (x0, x1, y0, y1) = (170.0, 610.0, 90.0, 290.0);
    let lines: [(usize, Color); 5] = [
        (28, Color::srgb(0.35, 0.35, 0.35)),
        (18, Color::srgb(0.2, 0.2, 0.2)),
        (10, Color::srgb(0.2, 0.2, 0.2)),
        (0, Color::srgb(0.0, 0.55, 0.0)),
        (14, Color::srgb(0.9, 0.0, 0.0)),
    ];
    let vals = h.iter().flat_map(|s| lines.iter().map(|(k, _)| s.percentiles[*k]));
    let (lo, hi) = vals
        .filter(|v| v.is_finite())
        .fold((0.0f32, 0.0f32), |(lo, hi), v| (lo.min(v), hi.max(v)));
    let span = (hi - lo).max(0.1);
    let pt = |i: usize, v: f32| {
        let x = x0 + (x1 - x0) * i as f32 / (h.len() - 1) as f32;
        let y = y0 + (y1 - y0) * ((v.max(lo) - lo) / span);
        origin + Vec2::new(x, y) * z
    };
    // Zero line and axes.
    g.line_2d(pt(0, 0.0), pt(h.len() - 1, 0.0), Color::srgb(0.6, 0.6, 0.6));
    g.line_2d(origin + Vec2::new(x0, y0) * z, origin + Vec2::new(x0, y1) * z, Color::BLACK);
    for (k, color) in lines {
        for i in 1..h.len() {
            g.line_2d(pt(i - 1, h[i - 1].percentiles[k]), pt(i, h[i].percentiles[k]), color);
        }
    }
    // Selected generation marker.
    let sel = view.selected_gen.unwrap_or(h.len() - 1);
    g.line_2d(pt(sel, lo), pt(sel, hi), Color::srgba(0.0, 0.6, 0.0, 0.6));
}

fn update_hud(
    evo: Res<Evo>,
    view: Res<View>,
    pb: Res<Playback>,
    sound: Res<AudioSettings>,
    screen: Res<State<Screen>>,
    debug: Res<DebugUi>,
    mut hud: Query<(&mut Text, &mut Visibility), With<Hud>>,
) {
    let Ok((mut text, mut vis)) = hud.single_mut() else { return };
    // Developer overlay only, and never over the menu.
    let show = debug.0 && *screen.get() != Screen::Menu;
    *vis = if show { Visibility::Inherited } else { Visibility::Hidden };
    if !show {
        return;
    }
    let mut s = match &evo.setup.level {
        Some(run) => {
            let l = &run.level;
            let mut s = format!(
                "Level {}: {}   goal {} m   3 stars within {} generations, 2 within {}\n",
                run.index + 1,
                l.name,
                l.goal,
                l.stars.three,
                l.stars.two
            );
            if let Some(gens) = evo.finished_at {
                s += &format!("Goal reached in generation {gens}. Keep evolving to see how far it goes.\n");
            }
            s
        }
        None => format!("Free play: {}\n", evo.setup.preset.name()),
    };
    match evo.history.last() {
        Some(st) => s += &format!(
            "Generations: {}   best {:.3} m   median {:.3} m   worst {:.3} m   top species S{:02}\n",
            evo.history.len(),
            st.percentiles[0],
            st.percentiles[14],
            st.percentiles[28],
            st.top_species
        ),
        None => s += &format!("Generations: 0   ({} untested creatures)\n", evo.cfg.population),
    }
    let status = match (evo.busy(), evo.auto) {
        (true, true) => "auto-running…",
        (true, false) => "simulating generation…",
        (false, true) => "auto",
        (false, false) => "idle",
    };
    s += &format!(
        "Status: {status}   playback {}×   t = {:.1} s\n\n{}\n\n",
        SPEEDS[view.speed_idx],
        pb.sim.timer as f32 / 60.0,
        pb.label
    );
    let pct = |v: f32| (v * 100.0).round() as u32;
    s += &match (sound.unlocked, sound.muted) {
        (false, _) => "Sound: press any key to start\n".to_string(),
        (true, true) => "Sound: muted\n".to_string(),
        (true, false) => format!("Sound: music {}%   effects {}%\n", pct(sound.music), pct(sound.sfx)),
    };
    s += "[Space] next gen   [A] auto   [L] leader   [1/2/3] best/median/worst   [←/→/End] browse gens\n\
          [↑/↓] speed   [Enter] replay   [wheel or -/=] zoom   [R] restart   [Esc] level select\n\
          [P] switch preset (free play)   [M] mute   [,/.] music volume   [[/]] effects volume   [F3] hide this";
    text.0 = s;
}
