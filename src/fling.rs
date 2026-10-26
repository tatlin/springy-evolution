//! Grabbing the creature with the mouse or a finger. Before a run starts, flinging it kicks the run
//! off and the gesture becomes the random seed. During a replay, poking it is just for fun and marks
//! the run "nudged". Only the playback simulation is touched, never the evolution.

use crate::{Evo, Playback, S, begin_evolution};
use bevy::prelude::*;
use bevy::ui::UiGlobalTransform;
use bevy::ui_widgets::Button;
use bevy::window::PrimaryWindow;

/// How strongly a held node is pulled toward the pointer each step.
const PULL: f32 = 0.25;
/// Fastest a held or flung node may move, in simulation units per step (about 3.6 m/s).
const MAX_SPEED: f32 = 0.3;
/// Seconds the flung creature tumbles before evolution starts.
const TUMBLE_SECS: f32 = 2.5;
/// How close a press must land to a node to grab it, in screen pixels, so it's as easy zoomed out.
const REACH_PX: f32 = 36.0;

#[derive(Resource, Default)]
pub struct Grab {
    /// The pointer's path while dragging (simulation units, seconds), hashed into the seed.
    path: Vec<(Vec2, f32)>,
    /// A fling's seed and when it was let go. Evolution starts once the creature has tumbled.
    pending: Option<(u64, f32)>,
}

#[allow(clippy::too_many_arguments)]
pub fn grab_input(
    mouse: Res<ButtonInput<MouseButton>>,
    touches: Res<Touches>,
    time: Res<Time>,
    window: Query<&Window, With<PrimaryWindow>>,
    camera: Query<(&Camera, &GlobalTransform)>,
    buttons: Query<(&ComputedNode, &UiGlobalTransform, &InheritedVisibility), With<Button>>,
    evo: Res<Evo>,
    mut grab: ResMut<Grab>,
    mut pb: ResMut<Playback>,
) {
    let (Ok(window), Ok((camera, cam_tf))) = (window.single(), camera.single()) else { return };
    let touch = touches.iter().next().map(|t| t.position());
    let pressed = mouse.just_pressed(MouseButton::Left) || touches.any_just_pressed();
    let released = mouse.just_released(MouseButton::Left) || touches.any_just_released();
    let screen = touch.or_else(|| window.cursor_position());
    let pointer = screen.and_then(|p| camera.viewport_to_world_2d(cam_tf, p).ok());
    // Presses on buttons belong to the buttons. Test the rectangles rather than hover state, which
    // lags a frame: a finger has no hover before it lands.
    let on_button = screen.is_some_and(|p| {
        let p = p * window.scale_factor();
        buttons.iter().any(|(node, tf, vis)| vis.get() && node.contains_point(*tf, p))
    });
    let to_sim = |w: Vec2| Vec2::new(w.x / S, -w.y / S);
    let now = time.elapsed_secs();

    if pressed
        && pb.grab.is_none()
        && !on_button
        && let Some(world) = pointer
    {
        let reach = REACH_PX * cam_tf.compute_transform().scale.x / S;
        let at = to_sim(world);
        let nearest = pb
            .sim
            .nodes
            .iter()
            .enumerate()
            .map(|(i, n)| (i, Vec2::new(n.x, n.y).distance(at) - n.m / 2.0))
            .filter(|(_, d)| *d <= reach)
            .min_by(|a, b| a.1.total_cmp(&b.1));
        if let Some((i, _)) = nearest {
            pb.grab = Some((i, at));
            grab.path.clear();
            grab.pending = None;
            if !evo.awaiting_fling {
                pb.nudged = true;
            }
        }
    }

    let Some((i, _)) = pb.grab else { return };
    if released {
        pb.grab = None;
        if evo.awaiting_fling {
            grab.pending = Some((seed_from(&grab.path), now));
        }
    } else if let Some(world) = pointer {
        let at = to_sim(world);
        pb.grab = Some((i, at));
        grab.path.push((at, now));
    }
}

/// Once the flung creature has tumbled for a moment, start evolving from the fling's seed.
pub fn kick_off(time: Res<Time>, evo: Res<Evo>, mut grab: ResMut<Grab>, mut commands: Commands) {
    if !evo.awaiting_fling {
        grab.pending = None;
        return;
    }
    if let Some((seed, at)) = grab.pending
        && time.elapsed_secs() - at >= TUMBLE_SECS
    {
        grab.pending = None;
        commands.run_system_cached_with(begin_evolution, seed);
    }
}

/// Pull the held node toward the pointer. Called before each playback step; on release the node
/// keeps its last velocity, which is the fling.
pub fn steer(pb: &mut Playback) {
    let Some((i, target)) = pb.grab else { return };
    let Some(n) = pb.sim.nodes.get_mut(i) else { return };
    let v = ((target - Vec2::new(n.x, n.y)) * PULL).clamp_length_max(MAX_SPEED);
    (n.vx, n.vy) = (v.x, v.y);
}

/// FNV-1a over the gesture, with times relative to its start: the same fling gives the same seed.
fn seed_from(path: &[(Vec2, f32)]) -> u64 {
    let t0 = path.first().map_or(0.0, |(_, t)| *t);
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for v in path.iter().flat_map(|(p, t)| [p.x, p.y, t - t0]).chain([path.len() as f32]) {
        for b in v.to_bits().to_le_bytes() {
            h ^= b as u64;
            h = h.wrapping_mul(0x0100_0000_01b3);
        }
    }
    h
}
