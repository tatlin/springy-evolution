//! Bevy front end: watch creatures run, breed generations in the background, plot progress.

use bevy::input::mouse::AccumulatedMouseScroll;
use bevy::prelude::*;
use springy_evolution::sim::{Config, Creature, GenStats, Population, Sim};
use std::thread::JoinHandle;

/// Pixels per simulation unit (1 unit = 20 cm), same as the original's camzoom of 0.01.
const S: f32 = 100.0;
/// How far above the camera centre the ground sits, in pixels (original: 80% down the screen).
const GROUND_OFFSET: f32 = 216.0;
const SPEEDS: [u32; 8] = [1, 2, 4, 8, 16, 32, 64, 900];

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
    fn name(self) -> &'static str {
        match self {
            Preset::Evolution3Web => "evolution3WEB (truss seed, regrown muscles, hurdles)",
            Preset::CaryHuang => "Cary Huang original (random seed, full mutation)",
        }
    }
}

#[derive(Resource)]
struct Evo {
    preset: Preset,
    cfg: Config,
    seed: u64,
    population_size: usize,
    /// `--max-len` override for `Config::max_muscle_length`.
    max_muscle_length: Option<f32>,
    /// `--fixed-topology`: never add or remove nodes and muscles.
    fixed_topology: bool,
    /// `--min-connections` override for `Config::min_connections`.
    min_connections: Option<usize>,
    /// `None` while a generation is being computed on the worker thread.
    pop: Option<Population>,
    worker: Option<JoinHandle<Population>>,
    history: Vec<GenStats>,
    /// A creature from the untested first generation, to watch before anything is evaluated.
    preview: Creature,
    auto: bool,
    generations_queued: u32,
}

impl Evo {
    fn new(
        preset: Preset,
        seed: u64,
        population_size: usize,
        max_muscle_length: Option<f32>,
        fixed_topology: bool,
        min_connections: Option<usize>,
    ) -> Self {
        let mut cfg = preset.config();
        cfg.population = population_size.max(2);
        cfg.mutate_topology = !fixed_topology;
        if let Some(n) = min_connections {
            cfg.min_connections = n;
        }
        if let Some(len) = max_muscle_length {
            cfg.max_muscle_length = len.max(0.5);
        }
        let pop = Population::new(cfg.clone(), seed);
        Self {
            preview: pop.creatures[0].clone(),
            preset,
            cfg,
            seed,
            population_size,
            max_muscle_length,
            fixed_topology,
            min_connections,
            pop: Some(pop),
            worker: None,
            history: Vec::new(),
            auto: false,
            generations_queued: 0,
        }
    }

    fn busy(&self) -> bool {
        self.worker.is_some()
    }

    fn start_generation(&mut self) {
        if let Some(mut pop) = self.pop.take() {
            self.worker = Some(std::thread::spawn(move || {
                pop.step();
                pop
            }));
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Which {
    Best,
    Median,
    Worst,
}

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
    label: String,
    hold: u32,
}

impl Playback {
    fn new(source: &Creature, label: String) -> Self {
        Self { sim: Sim::new(source), source: source.clone(), label, hold: 0 }
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
    let seed = arg("--seed").unwrap_or_else(|| {
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
            as u64
    });
    let population: usize = parse_arg(&args, "--pop").unwrap_or(1000);

    let max_muscle_length = parse_arg(&args, "--max-len");
    let fixed_topology = args.iter().any(|a| a == "--fixed-topology");
    let min_connections = parse_arg(&args, "--min-connections");

    let evo =
        Evo::new(preset, seed, population, max_muscle_length, fixed_topology, min_connections);
    let playback = Playback::new(&evo.preview, "Generation 1, creature #1 (untested)".into());

    App::new()
        .add_plugins(DefaultPlugins.set(WindowPlugin {
            primary_window: Some(Window {
                title: "Springy Evolution — Rust + Bevy".into(),
                resolution: (1280u32, 720u32).into(),
                ..default()
            }),
            ..default()
        }))
        .insert_resource(ClearColor(Color::srgb_u8(120, 200, 255)))
        .insert_resource(evo)
        .insert_resource(playback)
        .insert_resource(View { selected_gen: None, which: Which::Best, speed_idx: 0, zoom: 1.0 })
        .insert_resource(CameraRig { target_zoom: 1.0, last_view_zoom: 1.0, ..default() })
        .add_systems(Startup, (setup, spawn_obstacles))
        .add_systems(
            Update,
            (
                input,
                poll_worker,
                step_playback,
                draw_creature,
                follow_camera,
                draw_graph,
                update_hud,
            )
                .chain(),
        )
        .run();
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
                Transform::from_xyz(390.0, 220.0, -10.0),
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
        Node { position_type: PositionType::Absolute, top: px(10.0), left: px(12.0), ..default() },
        Hud,
    ));
}

fn spawn_obstacles(mut commands: Commands, evo: Res<Evo>, old: Query<Entity, With<Obstacle>>) {
    for e in &old {
        commands.entity(e).despawn();
    }
    for r in &evo.cfg.rects {
        let size = Vec2::new((r.x2 - r.x1) * S, (r.y2 - r.y1) * S);
        let centre = to_world((r.x1 + r.x2) / 2.0, (r.y1 + r.y2) / 2.0);
        commands.spawn((
            Sprite::from_color(Color::srgb_u8(0, 130, 0), size),
            Transform::from_translation(centre.extend(0.5)),
            Obstacle,
        ));
    }
}

// ------------------------------------------------------------------------------------------------
// Systems
// ------------------------------------------------------------------------------------------------

fn watch_target(evo: &Evo, view: &View) -> Option<(Creature, String)> {
    let idx = view.selected_gen.unwrap_or(evo.history.len().checked_sub(1)?);
    let stats = evo.history.get(idx)?;
    let (c, name) = match view.which {
        Which::Best => (&stats.best, "best"),
        Which::Median => (&stats.median, "median"),
        Which::Worst => (&stats.worst, "worst"),
    };
    let label = format!(
        "Generation {} — {name} creature (S{:02}, scored {:.3} m)",
        idx + 1,
        c.species(),
        c.fitness
    );
    Some((c.clone(), label))
}

fn input(
    keys: Res<ButtonInput<KeyCode>>,
    scroll: Res<AccumulatedMouseScroll>,
    mut evo: ResMut<Evo>,
    mut view: ResMut<View>,
    mut playback: ResMut<Playback>,
    mut commands: Commands,
) {
    let mut retarget = false;

    if keys.just_pressed(KeyCode::Space) {
        evo.generations_queued += 1;
    }
    if keys.just_pressed(KeyCode::KeyA) {
        evo.auto = !evo.auto;
    }
    for (key, which) in
        [(KeyCode::Digit1, Which::Best), (KeyCode::Digit2, Which::Median), (KeyCode::Digit3, Which::Worst)]
    {
        if keys.just_pressed(key) {
            view.which = which;
            retarget = true;
        }
    }
    let n = evo.history.len();
    if n > 0 {
        if keys.just_pressed(KeyCode::ArrowLeft) {
            view.selected_gen = Some(view.selected_gen.unwrap_or(n - 1).saturating_sub(1));
            retarget = true;
        }
        if keys.just_pressed(KeyCode::ArrowRight) {
            let next = view.selected_gen.unwrap_or(n - 1) + 1;
            view.selected_gen = if next >= n - 1 { None } else { Some(next) };
            retarget = true;
        }
        if keys.just_pressed(KeyCode::End) {
            view.selected_gen = None;
            retarget = true;
        }
    }
    if keys.just_pressed(KeyCode::ArrowUp) {
        view.speed_idx = (view.speed_idx + 1).min(SPEEDS.len() - 1);
    }
    if keys.just_pressed(KeyCode::ArrowDown) {
        view.speed_idx = view.speed_idx.saturating_sub(1);
    }
    if keys.just_pressed(KeyCode::Enter) {
        *playback = Playback::new(&playback.source.clone(), playback.label.clone());
    }

    // Zoom (wheel or -/=).
    let mut z = 0.0;
    if keys.pressed(KeyCode::Minus) {
        z += 0.02;
    }
    if keys.pressed(KeyCode::Equal) {
        z -= 0.02;
    }
    z -= scroll.delta.y.clamp(-5.0, 5.0) * 0.05;
    view.zoom = (view.zoom * (1.0 + z)).clamp(0.3, 8.0);

    // Restart (R) or switch preset (P).
    let restart = keys.just_pressed(KeyCode::KeyR);
    let switch = keys.just_pressed(KeyCode::KeyP);
    if restart || switch {
        let preset = match (switch, evo.preset) {
            (false, p) => p,
            (true, Preset::Evolution3Web) => Preset::CaryHuang,
            (true, Preset::CaryHuang) => Preset::Evolution3Web,
        };
        let seed = evo.seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        // Any running worker is detached; its result is simply dropped.
        *evo = Evo::new(
            preset,
            seed,
            evo.population_size,
            evo.max_muscle_length,
            evo.fixed_topology,
            evo.min_connections,
        );
        view.selected_gen = None;
        *playback = Playback::new(&evo.preview, "Generation 1, creature #1 (untested)".into());
        commands.run_system_cached(spawn_obstacles);
        return;
    }

    if retarget {
        if let Some((c, label)) = watch_target(&evo, &view) {
            *playback = Playback::new(&c, label);
        }
    }
}

fn poll_worker(mut evo: ResMut<Evo>, view: Res<View>, mut playback: ResMut<Playback>) {
    if evo.worker.as_ref().is_some_and(|h| h.is_finished()) {
        let pop = evo.worker.take().unwrap().join().expect("generation thread panicked");
        if let Some(stats) = pop.latest() {
            evo.history.push(stats.clone());
        }
        evo.pop = Some(pop);
        // When following the latest generation, start watching the new champion — but don't
        // interrupt a replay in progress while auto-running unless it has finished.
        if view.selected_gen.is_none() && (!evo.auto || playback.hold > 0 || evo.history.len() == 1)
        {
            if let Some((c, label)) = watch_target(&evo, &view) {
                *playback = Playback::new(&c, label);
            }
        }
    }
    if !evo.busy() && (evo.auto || evo.generations_queued > 0) {
        evo.generations_queued = evo.generations_queued.saturating_sub(1);
        evo.start_generation();
    }
}

fn step_playback(evo: Res<Evo>, view: Res<View>, mut pb: ResMut<Playback>) {
    let steps = evo.cfg.sim_steps;
    if pb.sim.timer < steps {
        for _ in 0..SPEEDS[view.speed_idx] {
            if pb.sim.timer >= steps {
                break;
            }
            pb.sim.step(&evo.cfg);
        }
    } else {
        pb.hold += 1;
        if pb.hold > 120 {
            *pb = Playback::new(&pb.source.clone(), pb.label.clone());
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
    mut arrow: Query<&mut Transform, (With<DistanceArrow>, Without<DistanceLabel>)>,
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
    if let Ok(mut atf) = arrow.single_mut() {
        atf.translation = Vec3::new(cx, readout_y, atf.translation.z);
        atf.scale = tf.scale;
    }
}

fn draw_graph(evo: Res<Evo>, view: Res<View>, cam: Query<&Transform, With<Camera2d>>, mut g: Gizmos) {
    let Ok(cam) = cam.single() else { return };
    let h = &evo.history;
    if h.len() < 2 {
        return;
    }
    // The camera may be zoomed out past `view.zoom` to fit the creature; follow its real scale.
    let z = cam.scale.x;
    let origin = cam.translation.truncate();
    // Graph box in screen pixels relative to the camera centre (matches the backdrop sprite).
    let (x0, x1, y0, y1) = (170.0, 610.0, 120.0, 320.0);
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

fn update_hud(evo: Res<Evo>, view: Res<View>, pb: Res<Playback>, mut hud: Query<&mut Text, With<Hud>>) {
    let Ok(mut text) = hud.single_mut() else { return };
    let mut s = format!("{}\n", evo.preset.name());
    match evo.history.last() {
        Some(st) => s += &format!(
            "Generations: {}   best {:.3} m   median {:.3} m   worst {:.3} m   top species S{:02}\n",
            evo.history.len(),
            st.percentiles[0],
            st.percentiles[14],
            st.percentiles[28],
            st.top_species
        ),
        None => s += &format!("Generations: 0   ({} untested creatures)\n", evo.population_size),
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
    s += "[Space] next gen   [A] auto   [1/2/3] best/median/worst   [←/→/End] browse gens\n\
          [↑/↓] speed   [Enter] replay   [wheel or -/=] zoom   [P] switch preset   [R] restart";
    text.0 = s;
}
